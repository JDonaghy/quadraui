//! Minimal AppShell runner demo — proves `run_with_shell()` pattern.
//!
//! The consumer implements [`ShellApp`] (~30 lines) instead of
//! `AppLogic` (~80 lines). The shell owns activity bar, sidebar header,
//! divider drag, and panel switching. The consumer renders sidebar
//! content + main content into the bounds the shell provides.
//!
//! Also demonstrates the activity-bar keyboard-cursor hook (#409):
//! `Tab` calls [`ShellContext::request_activity_keyboard_focus`] to enter
//! keyboard-cursor mode; from there `j`/`k` (or arrows) move the cursor,
//! `l`/`Enter`/`Space` activates the selected panel, and `Esc`/`h`/`Left`
//! cancels — all driven internally by the shell runner, not this app. A
//! `ShellApp` only needs to pick its own trigger key(s) and call
//! `request_activity_keyboard_focus()`; a different consumer could bind
//! `Ctrl+W` instead of `Tab` with no quadraui changes.
//!
//! `Ctrl+B` demonstrates the #454 fix: [`ShellContext::shell_mut`] reaches
//! the real `AppShell` instance quadraui's internal `ShellAdapter` actually
//! renders, so a consumer-driven binding can call
//! `ctx.shell_mut().toggle_sidebar()` directly instead of tracking a shadow
//! `AppShell` that can drift from what's on screen (the class of `Ctrl+B`
//! bug this issue fixes).

use std::cell::Cell;
use std::rc::Rc;

use quadraui::compose::app_shell::{AppShellEvent, AppShellLayout, PanelDefinition};
use quadraui::{
    Backend, Color, Key, Modifiers, NamedKey, Reaction, Rect, ShellApp, ShellConfig, ShellContext,
    StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

/// Shared handle onto the activity-bar rect the shell last handed this
/// app in `render_content`, in the backend's native units.
///
/// Exists for the issue #552 regression tests: they must click the
/// *n*-th activity-bar row after revealing the title bar, and the row
/// origin differs per backend (TUI cells vs GTK pixels). Reading the
/// real painted bounds beats hardcoding a coordinate that would be
/// silently wrong on one backend — and `driver_with_shell` hands back a
/// driver over the opaque `ShellAdapter`, so `driver.app()` cannot reach
/// this `ShellApp` directly.
#[derive(Clone, Default)]
pub struct ActivityProbe {
    bounds: Rc<Cell<Option<Rect>>>,
    hovered: Rc<Cell<Option<usize>>>,
}

// This module is `#[path]`-included by several test binaries and by the
// `tui_/gtk_appshell_demo` examples. Only `cross_backend_parity` reads the
// probe, so every other target sees these accessors as dead — the standard
// cost of a shared example module, not a sign they're unused.
#[allow(dead_code)]
impl ActivityProbe {
    /// The activity bar's rect as of the most recent frame.
    pub fn bounds(&self) -> Option<Rect> {
        self.bounds.get()
    }

    /// `AppShell::hovered_activity_idx()` as of the last pointer move
    /// this app saw. `AppShell` reports `MouseMoved` as `Ignored`, so the
    /// event reaches `ShellApp::handle` *after* the shell has already
    /// updated its hover state — making this an honest read of what the
    /// shell decided, not a re-derivation.
    pub fn hovered_idx(&self) -> Option<usize> {
        self.hovered.get()
    }
}

pub struct AppShellDemo {
    last_event: String,
    /// Set by the `p` key binding below; polled once via
    /// `take_requested_panel` to exercise the programmatic panel-switch
    /// hook (a bug class seen in consumer apps) — proves an app can
    /// jump straight to a panel (no ActivityBar click) and still get the
    /// ActivityBar highlight + sidebar header updated to match.
    pending_panel: Option<WidgetId>,
    /// Written every frame from `render_content`; read by tests.
    probe: ActivityProbe,
}

impl AppShellDemo {
    pub fn new() -> Self {
        Self {
            last_event: "Tab=focus bar | click icons | drag divider | p=jump to Source Control \
                         | Ctrl+B=toggle sidebar | t=toggle title bar | q=quit"
                .into(),
            pending_panel: None,
            probe: ActivityProbe::default(),
        }
    }

    /// Clone the activity-bar geometry handle before moving `self` into a
    /// driver. See [`ActivityProbe`].
    ///
    /// Dead in every target except `cross_backend_parity` — see the note
    /// on `impl ActivityProbe`.
    #[allow(dead_code)]
    pub fn probe(&self) -> ActivityProbe {
        self.probe.clone()
    }

    pub fn config() -> ShellConfig {
        ShellConfig::new(
            "AppShell Demo",
            vec![
                PanelDefinition {
                    id: WidgetId::new("panel:explorer"),
                    icon: "E".into(),
                    tooltip: "Explorer".into(),
                    title: "EXPLORER".into(),
                },
                PanelDefinition {
                    id: WidgetId::new("panel:search"),
                    icon: "S".into(),
                    tooltip: "Search".into(),
                    title: "SEARCH".into(),
                },
                PanelDefinition {
                    id: WidgetId::new("panel:git"),
                    icon: "G".into(),
                    tooltip: "Source Control".into(),
                    title: "SOURCE CONTROL".into(),
                },
            ],
        )
        .with_bottom_items(vec![PanelDefinition {
            id: WidgetId::new("panel:settings"),
            icon: "*".into(),
            tooltip: "Settings".into(),
            title: "Settings".into(),
        }])
    }
}

impl Default for AppShellDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellApp for AppShellDemo {
    fn render_content(&self, backend: &mut dyn Backend, layout: &AppShellLayout) {
        let lh = backend.line_height();
        // Publish the activity bar's real painted bounds for the #552
        // regression tests (see `ActivityProbe`). Harmless in the demo.
        self.probe.bounds.set(Some(layout.activity_bar_bounds));

        if let Some(content) = layout.sidebar_content_bounds {
            let label = StatusBar {
                id: WidgetId::new("sidebar-content"),
                left_segments: vec![StatusBarSegment {
                    text: " (sidebar content here) ".into(),
                    fg: Color::rgb(140, 140, 140),
                    bg: Color::rgb(30, 30, 30),
                    bold: false,
                    action_id: None,
                }],
                right_segments: vec![],
            };
            let rect = Rect::new(content.x, content.y, content.width, lh);
            backend.draw_status_bar(rect, &label, None, None);
        }

        let main_label = StatusBar {
            id: WidgetId::new("main-label"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.last_event),
                fg: Color::rgb(200, 200, 200),
                bg: Color::rgb(30, 30, 30),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        };
        let rect = Rect::new(
            layout.main_content_bounds.x,
            layout.main_content_bounds.y,
            layout.main_content_bounds.width,
            lh,
        );
        backend.draw_status_bar(rect, &main_label, None, None);
    }

    fn handle(
        &mut self,
        event: UiEvent,
        _backend: &mut dyn Backend,
        ctx: &ShellContext,
    ) -> Reaction {
        match &event {
            UiEvent::KeyPressed {
                key: Key::Char('q') | Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            // Tab is this demo's chosen trigger for entering activity-bar
            // keyboard-cursor mode (#409). The shell runner (not this app)
            // owns j/k/Enter/Esc navigation from here — see
            // `ShellAdapter::handle`. A different `ShellApp` is free to
            // bind a different key (e.g. `Ctrl+W`) to the same request.
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Tab),
                ..
            } => {
                ctx.request_activity_keyboard_focus();
                self.last_event = "Activity bar focused (j/k, Enter, Esc)".into();
                Reaction::Redraw
            }
            // `p` = jump straight to the Source Control panel the way an
            // action handler would (no ActivityBar click) — queues a
            // `take_requested_panel()` switch for `ShellAdapter` to apply.
            UiEvent::KeyPressed {
                key: Key::Char('p'),
                ..
            } => {
                self.pending_panel = Some(WidgetId::new("panel:git"));
                self.last_event = "Requested programmatic switch to panel:git".into();
                Reaction::Redraw
            }
            // `Ctrl+B` = the #454 fix: `ctx.shell_mut()` reaches the real
            // `AppShell` `ShellAdapter` renders, so this app can call
            // `toggle_sidebar()` on it directly — no shadow `AppShell`, no
            // drift between what this app thinks is visible and what's
            // actually painted (the class of bug #454 fixes).
            UiEvent::KeyPressed {
                key: Key::Char('b'),
                modifiers: Modifiers { ctrl: true, .. },
                ..
            } => {
                let now_visible = {
                    let mut shell = ctx.shell_mut();
                    shell.toggle_sidebar();
                    shell.sidebar_visible()
                };
                self.last_event = if now_visible {
                    "Sidebar shown (Ctrl+B via ctx.shell_mut())".into()
                } else {
                    "Sidebar hidden (Ctrl+B via ctx.shell_mut())".into()
                };
                Reaction::Redraw
            }
            // `t` = reveal/hide the title bar at runtime, the way vimcode
            // toggles its menu bar (`engine.menu_bar_visible`). This is the
            // transition issue #552 lives in: while the bar is hidden the
            // activity bar's origin is 0, so a hit-region that wrongly
            // folded that origin in still lined up. The moment the title
            // bar is revealed the origin becomes nonzero and the error
            // appears — which is why static construction tests never saw
            // it and only a toggle exercises the defect (same trap #547
            // documented).
            UiEvent::KeyPressed {
                key: Key::Char('t'),
                ..
            } => {
                let now_visible = {
                    let mut shell = ctx.shell_mut();
                    let next = !shell.title_bar_visible();
                    shell.set_title_bar_visible(next);
                    next
                };
                self.last_event = if now_visible {
                    "Title bar shown (t)".into()
                } else {
                    "Title bar hidden (t)".into()
                };
                Reaction::Redraw
            }
            // Pointer moves are `Ignored` by `AppShell` (it updates hover
            // and passes the event on), so by the time we see one the
            // shell's hover state is already settled — record it for the
            // #552 hover regression test.
            UiEvent::MouseMoved { .. } => {
                self.probe.hovered.set(ctx.shell().hovered_activity_idx());
                Reaction::Continue
            }
            UiEvent::MouseDown { position, .. } => {
                if ctx.in_sidebar(position.x, position.y) {
                    self.last_event = format!(
                        "Sidebar click (panel: {})",
                        ctx.active_panel_id.map(|id| id.as_str()).unwrap_or("none")
                    );
                    Reaction::Redraw
                } else if ctx.in_main(position.x, position.y) {
                    self.last_event = "Main area click".into();
                    Reaction::Redraw
                } else {
                    Reaction::Continue
                }
            }
            _ => Reaction::Continue,
        }
    }

    fn on_shell_event_ctx(&mut self, event: &AppShellEvent, ctx: &ShellContext) {
        self.last_event = match event {
            AppShellEvent::PanelChanged { panel_id } => {
                // #617: `on_shell_event_ctx` receives a `ShellContext`, so
                // an app can push shell state back on the *same* frame
                // this notification fires — no intervening `handle`
                // dispatch required. Reveal the title bar the moment
                // Source Control becomes active, the same shape vimcode's
                // menu-bar-reveal path needed (issue #617's downstream
                // motivation): before this, `ShellAdapter::handle`
                // returned immediately after this call, so a
                // `set_title_bar_visible` here would have been skipped
                // for the frame and only taken effect once some unrelated
                // later event happened to reach `handle`.
                if panel_id.as_str() == "panel:git" {
                    ctx.shell_mut().set_title_bar_visible(true);
                }
                format!("Panel: {}", panel_id.as_str())
            }
            AppShellEvent::SidebarHidden => "Sidebar hidden".into(),
            AppShellEvent::SidebarResized { new_width } => {
                format!("Resized: {new_width:.0}px")
            }
            AppShellEvent::BottomPanelResized { new_height } => {
                format!("Bottom panel: {new_height:.0}px")
            }
            AppShellEvent::BottomPanelHidden => "Bottom panel hidden".into(),
            AppShellEvent::BottomItemClicked { id } => {
                format!("Bottom: {}", id.as_str())
            }
            _ => return,
        };
    }

    fn take_requested_panel(&mut self) -> Option<WidgetId> {
        self.pending_panel.take()
    }
}
