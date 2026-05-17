//! Minimal AppShell runner demo — proves `run_with_shell()` pattern.
//!
//! The consumer implements [`ShellApp`] (~30 lines) instead of
//! `AppLogic` (~80 lines). The shell owns activity bar, sidebar header,
//! divider drag, and panel switching. The consumer renders sidebar
//! content + main content into the bounds the shell provides.

use quadraui::compose::app_shell::{AppShellEvent, AppShellLayout, PanelDefinition};
use quadraui::{
    Backend, Color, Key, NamedKey, Reaction, Rect, ShellApp, ShellConfig, ShellContext, StatusBar,
    StatusBarSegment, UiEvent, WidgetId,
};

pub struct AppShellDemo {
    last_event: String,
}

impl AppShellDemo {
    pub fn new() -> Self {
        Self {
            last_event: "click icons | drag divider | q=quit".into(),
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
            UiEvent::KeyPressed { key, .. } => match key {
                Key::Char('q') | Key::Named(NamedKey::Escape) => Reaction::Exit,
                _ => Reaction::Continue,
            },
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
            AppShellEvent::BottomItemClicked { id } => {
                format!("Bottom: {}", id.as_str())
            }
            _ => return,
        };
    }
}
