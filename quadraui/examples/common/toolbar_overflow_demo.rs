//! Backend-agnostic app code for the toolbar overflow example
//! ([`tui_toolbar_overflow`]).
//!
//! [`ToolbarOverflowDemo`] paints the same five-button `Toolbar` into a
//! bar deliberately narrower than its buttons' combined width, cycling
//! `ToolbarLayoutOptions::overflow` through the three
//! [`quadraui::ToolbarOverflow`] variants:
//!
//! - `Clip` (today's default): the trailing-most button(s) clip at the
//!   bar's edge.
//! - `Wrap`: the overflow buttons continue on additional terminal rows
//!   instead of clipping.
//! - `Menu`: the lowest-[`quadraui::ToolbarOverflowPriorities`]-first
//!   buttons are dropped and replaced by a trailing `[ » ]` affordance;
//!   clicking it lists what was dropped via
//!   [`quadraui::ToolbarLayout::overflowed_items`].
//!
//! Controls:
//! - m                cycle Clip -> Wrap -> Menu
//! - Click a button    report it in the status line
//! - Click `[ » ]` (Menu mode)   list the dropped items in the status line
//! - q / Esc          quit
//!
//! TUI and GTK both render `Wrap`'s extra rows and `Menu`'s `[ » ]`
//! affordance for real, through each backend's own
//! [`quadraui::Backend::draw_toolbar_with_overflow`] /
//! [`quadraui::Backend::toolbar_layout_with_overflow`] override. macOS and
//! Win-GUI still inherit the trait's default, which forwards to today's
//! Clip-only geometry until a per-backend issue gives them one (see
//! `CLAUDE.md`'s "One backend per issue").

use quadraui::{
    AppLogic, Backend, Color, InteractionState, Key, NamedKey, Reaction, Rect, StatusBar,
    StatusBarSegment, Toolbar, ToolbarButton, ToolbarHit, ToolbarLayoutOptions, ToolbarOverflow,
    ToolbarOverflowPriorities, UiEvent, WidgetId,
};

/// Rows reserved for the toolbar slot — generous enough for `Wrap` to
/// place every button on its own row at this demo's bar width.
const BAR_ROWS: f32 = 5.0;

pub struct ToolbarOverflowDemo {
    mode: ToolbarOverflow,
    interaction: InteractionState,
    last_message: String,
}

impl ToolbarOverflowDemo {
    pub fn new() -> Self {
        Self {
            mode: ToolbarOverflow::Clip,
            interaction: InteractionState::new(),
            last_message: "m cycles Clip -> Wrap -> Menu. q=quit".into(),
        }
    }

    /// Cycle [`Self::mode`] Clip → Wrap → Menu → Clip (the 'm' key).
    fn cycle_mode(&mut self) {
        self.mode = match self.mode {
            ToolbarOverflow::Clip => ToolbarOverflow::Wrap,
            ToolbarOverflow::Wrap => ToolbarOverflow::Menu,
            ToolbarOverflow::Menu => ToolbarOverflow::Clip,
        };
    }

    fn mode_label(&self) -> &'static str {
        match self.mode {
            ToolbarOverflow::Clip => "Clip",
            ToolbarOverflow::Wrap => "Wrap",
            ToolbarOverflow::Menu => "Menu",
        }
    }

    fn toolbar(&self) -> Toolbar {
        Toolbar::new(WidgetId::new("overflow-demo:toolbar")).with_buttons(vec![
            action("new", "New File"),
            action("open", "Open..."),
            action("save", "Save"),
            action("build", "Build Project"),
            action("deploy", "Deploy"),
        ])
    }

    /// `build` is lowest priority (dropped first in `Menu` mode), `deploy`
    /// highest (dropped last) — demonstrates the "higher survives longer"
    /// contract.
    fn priorities(&self) -> ToolbarOverflowPriorities {
        ToolbarOverflowPriorities::new()
            .with(WidgetId::new("build"), 0)
            .with(WidgetId::new("open"), 1)
            .with(WidgetId::new("new"), 2)
            .with(WidgetId::new("save"), 3)
            .with(WidgetId::new("deploy"), 4)
    }

    fn options(&self) -> ToolbarLayoutOptions {
        ToolbarLayoutOptions::new().with_overflow(self.mode)
    }

    /// Fixed at roughly half the viewport (floored at 24 cells) — always
    /// narrower than the five buttons' combined width, so every mode has
    /// overflow to show.
    fn toolbar_rect(backend: &dyn Backend) -> Rect {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let width = (viewport.width * 0.5).max(24.0).min(viewport.width);
        Rect::new(0.0, lh, width, lh * BAR_ROWS)
    }
}

impl Default for ToolbarOverflowDemo {
    fn default() -> Self {
        Self::new()
    }
}

fn action(id: &str, label: &str) -> ToolbarButton {
    ToolbarButton::Action {
        id: WidgetId::new(id),
        label: label.to_string(),
        icon: None,
        key_hint: None,
        enabled: true,
        is_active: false,
        tooltip: String::new(),
        spec: None,
    }
}

impl AppLogic for ToolbarOverflowDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();

        let title_rect = Rect::new(0.0, 0.0, viewport.width, lh);
        let _ = backend.draw_status_bar_interactive(
            title_rect,
            &StatusBar {
                id: WidgetId::new("title"),
                left_segments: vec![StatusBarSegment {
                    text: format!(
                        "  Toolbar overflow demo — mode: {} (m to cycle) ",
                        self.mode_label()
                    ),
                    fg: Color::rgb(255, 255, 255),
                    bg: Color::rgb(30, 30, 30),
                    bold: true,
                    action_id: None,
                }],
                right_segments: vec![],
            },
            &InteractionState::new(),
        );

        let rect = Self::toolbar_rect(backend);
        let bar = self.toolbar();
        let _ = backend.draw_toolbar_with_overflow(
            rect,
            &bar,
            &self.interaction,
            self.options(),
            &self.priorities(),
        );

        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &StatusBar {
                id: WidgetId::new("status"),
                left_segments: vec![StatusBarSegment {
                    text: format!("  {} ", self.last_message),
                    fg: Color::rgb(220, 220, 220),
                    bg: Color::rgb(20, 20, 20),
                    bold: false,
                    action_id: None,
                }],
                right_segments: vec![],
            },
            &InteractionState::new(),
        );
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            }
            | UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,

            UiEvent::KeyPressed {
                key: Key::Char('m'),
                ..
            } => {
                self.cycle_mode();
                self.last_message = format!("Mode: {}", self.mode_label());
                Reaction::Redraw
            }

            // Same `InteractionState`-owns-hover/press convention as
            // `ToolbarApp` — see that example's own doc on this arm.
            UiEvent::MouseMoved { .. } | UiEvent::MouseDown { .. } | UiEvent::MouseUp { .. } => {
                let rect = Self::toolbar_rect(backend);
                let bar = self.toolbar();
                let layout = backend.toolbar_layout_with_overflow(
                    rect,
                    &bar,
                    self.options(),
                    &self.priorities(),
                );
                let hit_test = |x: f32, y: f32| match layout.hit_test(x, y) {
                    ToolbarHit::Button(id) => Some(id),
                    _ => None,
                };
                let was_pressed = self.interaction.pressed().cloned();
                let changed = self.interaction.handle_mouse(&event, hit_test);

                if let UiEvent::MouseUp { position, .. } = &event {
                    match layout.hit_test(position.x, position.y) {
                        ToolbarHit::Button(release_id) if was_pressed == Some(release_id) => {
                            self.last_message = format!("Clicked: {}", release_id.as_str());
                        }
                        ToolbarHit::Overflow => {
                            let names: Vec<String> = layout
                                .overflowed_items()
                                .iter()
                                .map(|item| {
                                    item.label
                                        .spans
                                        .iter()
                                        .map(|s| s.text.as_str())
                                        .collect::<String>()
                                })
                                .collect();
                            self.last_message = if names.is_empty() {
                                "Nothing overflowed".into()
                            } else {
                                format!("Overflowed: {}", names.join(", "))
                            };
                        }
                        _ => {}
                    }
                    return Reaction::Redraw;
                }

                if changed {
                    Reaction::Redraw
                } else {
                    Reaction::Continue
                }
            }

            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
