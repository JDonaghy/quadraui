//! [`KubeApp`] — the `quadraui::AppLogic` implementation both `kubeui`
//! (TUI) and `kubeui-gtk` run through [`quadraui::tui::run`] /
//! [`quadraui::gtk::run`].
//!
//! Having one `AppLogic` means one key map and one click router: a
//! backend binary's `main` is just "build a `KubeApp`, hand it to the
//! runner" (see `kubeui/src/main.rs` / `kubeui-gtk/src/main.rs`). Both
//! runners already own panic-safe terminal/window teardown, so this
//! crate doesn't need its own.

use quadraui::{
    AppLogic, Backend, Color, DualModePaletteEvent, InteractionState, Key, MouseButton, NamedKey,
    Point, Reaction, Rect, StatusBarHit, Theme, UiEvent, PALETTE_CHROME_ROWS,
};

use crate::action::{apply_action, commit_picker_selection, Action};
use crate::shell::{bootstrap_state, theme};
use crate::state::AppState;
use crate::view::{build_list, build_status_bar, build_yaml_view};

/// Backend-agnostic kubeui application. Owns the `AppState`, the
/// theme it paints with, and the `tokio::Runtime` the k8s client's
/// `block_on` calls run on (one runtime per process, owned here
/// rather than by the backend binary, since `AppLogic` methods take
/// no extra arguments to thread one through).
pub struct KubeApp {
    state: AppState,
    theme: Theme,
    rt: tokio::runtime::Runtime,
}

impl KubeApp {
    /// Install the rustls crypto provider, spin up a `tokio::Runtime`,
    /// and bootstrap `AppState` (current context + namespace list)
    /// before the runner's first frame.
    pub fn new() -> anyhow::Result<Self> {
        crate::install_crypto_provider()?;
        let rt = tokio::runtime::Runtime::new()?;
        let state = bootstrap_state(&rt);
        Ok(Self {
            state,
            theme: theme(),
            rt,
        })
    }

    /// Centred popup rect for the namespace/kind picker — `DualModePaletteController`
    /// has no opinion on placement, so every consumer picks one; this
    /// is the same "60% of the viewport, centred" shape the dual-mode
    /// palette demo uses.
    fn popup_rect(backend: &dyn Backend) -> Rect {
        let vp = backend.viewport();
        let lh = backend.line_height();
        let w = (vp.width * 0.6).max(30.0);
        let h = (vp.height * 0.6).max(10.0 * lh);
        Rect::new((vp.width - w) / 2.0, (vp.height - h) / 2.0, w, h)
    }

    /// Status bar occupies the bottom `line_height` row of the
    /// viewport in every backend's unit (cells for TUI, pixels for
    /// GTK) — shared by `render` and the mouse-click resolver below so
    /// paint and click can't drift apart.
    fn status_bar_rect(backend: &dyn Backend) -> Rect {
        let vp = backend.viewport();
        let lh = backend.line_height();
        Rect::new(0.0, vp.height - lh, vp.width, lh)
    }

    /// `theme.background`, a touch darker — the YAML pane's
    /// background tints slightly off the resource list's so the split
    /// reads as two panes without a hard border. Derived from whatever
    /// `Theme` the app is running (dark default, `Theme::light_default`,
    /// or a loaded VS Code theme) rather than a fixed RGB literal, so it
    /// still looks intentional under every theme instead of only the
    /// one it was picked to match.
    fn yaml_pane_theme(theme: &Theme) -> Theme {
        const SHADE: f32 = 0.85;
        let shade = |c: u8| (c as f32 * SHADE).round() as u8;
        let bg = theme.background;
        Theme {
            background: Color::rgb(shade(bg.r), shade(bg.g), shade(bg.b)),
            ..*theme
        }
    }

    fn handle_key(&mut self, key: Key) -> Reaction {
        let action = match key {
            Key::Char('q') | Key::Named(NamedKey::Escape) => return Reaction::Exit,
            Key::Char('r') => Action::Refresh,
            Key::Char('n') => Action::OpenNamespacePicker,
            Key::Char('K') => Action::OpenKindPicker,
            Key::Named(NamedKey::Tab) | Key::Named(NamedKey::BackTab) => Action::ToggleFocus,
            Key::Char('j') | Key::Named(NamedKey::Down) => Action::MoveDown,
            Key::Char('k') | Key::Named(NamedKey::Up) => Action::MoveUp,
            Key::Named(NamedKey::PageDown) => Action::YamlPageDown,
            Key::Named(NamedKey::PageUp) => Action::YamlPageUp,
            _ => return Reaction::Continue,
        };
        apply_action(&mut self.state, action, &self.rt);
        if self.state.should_quit {
            return Reaction::Exit;
        }
        Reaction::Redraw
    }

    /// Resolve a left-click against the status bar — the only
    /// non-picker clickable surface. Walks the same `StatusBar`
    /// `render` painted via `Backend::status_bar_layout`, so paint and
    /// click never drift apart (same convention
    /// `DualModePaletteController::handle_mouse` uses for the palette
    /// below).
    fn handle_status_bar_click(&mut self, backend: &dyn Backend, position: Point) -> Reaction {
        let rect = Self::status_bar_rect(backend);
        if position.y < rect.y {
            return Reaction::Continue;
        }
        let bar = build_status_bar(&self.state, &self.theme);
        let layout = backend.status_bar_layout(rect, &bar);
        if let StatusBarHit::Segment(id) = layout.hit_test(position.x - rect.x, position.y - rect.y)
        {
            apply_action(
                &mut self.state,
                Action::StatusBarSegmentClicked(id),
                &self.rt,
            );
            return Reaction::Redraw;
        }
        Reaction::Continue
    }

    /// Route an event to the open `PickerOverlay`'s
    /// `DualModePaletteController`. Called instead of the ordinary key
    /// map whenever `state.picker.is_some()` — the picker owns every
    /// keystroke while it's open (including letters that would
    /// otherwise be app accelerators, e.g. `q`, which types into the
    /// filter query instead of quitting).
    fn handle_picker_event(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        let popup_rect = Self::popup_rect(backend);
        let lh = backend.line_height();
        let popup_rows = if lh > 0.0 {
            (popup_rect.height / lh) as usize
        } else {
            20
        };
        let visible_rows = popup_rows.saturating_sub(PALETTE_CHROME_ROWS);

        let Some(picker) = self.state.picker.as_mut() else {
            return Reaction::Continue;
        };
        let outcome = match &event {
            UiEvent::MouseDown { .. } => {
                picker
                    .controller_mut()
                    .handle_mouse(&event, backend, popup_rect, visible_rows)
            }
            _ => picker.controller_mut().handle(&event, visible_rows),
        };

        match outcome {
            DualModePaletteEvent::ItemConfirmed { idx } => {
                let purpose = picker.purpose;
                let orig = picker.orig_index_of_row(idx);
                self.state.picker = None;
                if let Some(orig) = orig {
                    commit_picker_selection(&mut self.state, purpose, orig, &self.rt);
                }
                Reaction::Redraw
            }
            DualModePaletteEvent::QueryChanged { .. } => {
                picker.refilter();
                Reaction::Redraw
            }
            DualModePaletteEvent::Cancelled => {
                self.state.picker = None;
                Reaction::Redraw
            }
            DualModePaletteEvent::Consumed => Reaction::Redraw,
            // Namespace/kind pickers have no free-text "create new"
            // concept, so Input mode (reachable via Tab) has nothing
            // meaningful to confirm — both are no-ops beyond redrawing
            // whatever the controller's own chrome changed to.
            DualModePaletteEvent::ModeToggled { .. }
            | DualModePaletteEvent::TextConfirmed { .. } => Reaction::Redraw,
            DualModePaletteEvent::Ignored => Reaction::Continue,
        }
    }
}

impl AppLogic for KubeApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        let lh = backend.line_height();
        let cw = backend.char_width();
        let body_h = (vp.height - lh).max(0.0);
        // Unified split math (was duplicated per backend with
        // different minimums before the `AppLogic` port): 40% of the
        // viewport for the resource list, floored at 20 character
        // cells so a narrow terminal/window still shows legible rows.
        let list_w = (vp.width * 0.4).max(20.0 * cw).min(vp.width);

        let list_rect = Rect::new(0.0, 0.0, list_w, body_h);
        let yaml_rect = Rect::new(list_w, 0.0, vp.width - list_w, body_h);
        let status_rect = Self::status_bar_rect(backend);

        // Ambient theme, per `AppLogic::render`'s doc: every `draw_*`
        // call below reads colours off whatever `Theme` was last set
        // here, not an explicit per-call argument.
        backend.set_theme(self.theme);
        let list = build_list(&self.state, &self.theme);
        backend.draw_list(list_rect, &list);

        let yaml = build_yaml_view(&self.state, &self.theme);
        backend.set_theme(Self::yaml_pane_theme(&self.theme));
        backend.draw_text_display(yaml_rect, &yaml);
        backend.set_theme(self.theme);

        let bar = build_status_bar(&self.state, &self.theme);
        backend.draw_status_bar_interactive(status_rect, &bar, &InteractionState::new());

        if let Some(picker) = self.state.picker.as_ref() {
            picker
                .controller()
                .render(Self::popup_rect(backend), backend);
        }
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        if self.state.picker.is_some() {
            return self.handle_picker_event(event, backend);
        }
        match event {
            UiEvent::KeyPressed { key, .. } => self.handle_key(key),
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => self.handle_status_bar_click(backend, position),
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
