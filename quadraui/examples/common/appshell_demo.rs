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

use quadraui::compose::app_shell::{AppShellEvent, AppShellLayout, PanelDefinition};
use quadraui::{
    Backend, Color, Key, NamedKey, Reaction, Rect, ShellApp, ShellConfig, ShellContext, StatusBar,
    StatusBarSegment, UiEvent, WidgetId,
};

pub struct AppShellDemo {
    last_event: String,
    /// Set by the `p` key binding below; polled once via
    /// `take_requested_panel` to exercise the programmatic panel-switch
    /// hook (quadraui consumer coord-tui #1029 bug A) — proves an app can
    /// jump straight to a panel (no ActivityBar click) and still get the
    /// ActivityBar highlight + sidebar header updated to match.
    pending_panel: Option<WidgetId>,
}

impl AppShellDemo {
    pub fn new() -> Self {
        Self {
            last_event: "Tab=focus bar | click icons | drag divider | p=jump to Source Control | q=quit".into(),
            pending_panel: None,
        }
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

    fn on_shell_event(&mut self, event: &AppShellEvent) {
        self.last_event = match event {
            AppShellEvent::PanelChanged { panel_id } => {
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
