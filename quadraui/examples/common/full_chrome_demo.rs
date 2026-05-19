//! Full-chrome AppShell demo — proves all four chrome slots
//! (title bar, bottom panel, command line, status bar) plus the
//! existing activity bar / sidebar / main layout.

use quadraui::compose::app_shell::{AppShellEvent, AppShellLayout, PanelDefinition};
use quadraui::{
    Backend, Color, Key, NamedKey, Reaction, Rect, ShellApp as ShellAppTrait, ShellConfig,
    ShellContext, StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

pub struct FullChromeDemo {
    last_event: String,
}

impl FullChromeDemo {
    pub fn new() -> Self {
        Self {
            last_event: "click icons | drag dividers | q=quit".into(),
        }
    }

    pub fn config() -> ShellConfig {
        ShellConfig::new(
            "Full Chrome Demo",
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
                PanelDefinition {
                    id: WidgetId::new("panel:debug"),
                    icon: "D".into(),
                    tooltip: "Debug".into(),
                    title: "RUN AND DEBUG".into(),
                },
            ],
        )
        .with_bottom_items(vec![PanelDefinition {
            id: WidgetId::new("panel:settings"),
            icon: "*".into(),
            tooltip: "Settings".into(),
            title: "Settings".into(),
        }])
        .with_title_bar(1.5)
        .with_bottom_panel(8.0)
        .with_bottom_panel_limits(3.0, 25.0)
        .with_command_line()
        .with_status_bar()
    }

    fn draw_label(
        backend: &mut dyn Backend,
        bounds: Rect,
        text: &str,
        fg: Color,
        bg: Color,
        bold: bool,
    ) {
        let lh = backend.line_height();
        let bar = StatusBar {
            id: WidgetId::new("chrome-label"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {text} "),
                fg,
                bg,
                bold,
                action_id: None,
            }],
            right_segments: vec![],
        };
        let rect = Rect::new(bounds.x, bounds.y, bounds.width, lh.min(bounds.height));
        backend.draw_status_bar(rect, &bar, None, None);
    }
}

impl Default for FullChromeDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellAppTrait for FullChromeDemo {
    fn render_content(&self, backend: &mut dyn Backend, layout: &AppShellLayout) {
        let title_fg = Color::rgb(220, 220, 220);
        let title_bg = Color::rgb(50, 50, 55);
        let content_fg = Color::rgb(140, 140, 140);
        let content_bg = Color::rgb(30, 30, 30);
        let status_fg = Color::rgb(200, 200, 220);
        let status_bg = Color::rgb(0, 120, 210);

        if let Some(tb) = layout.title_bar_bounds {
            Self::draw_label(
                backend,
                tb,
                "TITLE BAR  |  File  Edit  View  Help",
                title_fg,
                title_bg,
                true,
            );
        }

        if let Some(content) = layout.sidebar_content_bounds {
            Self::draw_label(
                backend,
                content,
                "(sidebar content)",
                content_fg,
                content_bg,
                false,
            );
        }

        Self::draw_label(
            backend,
            layout.main_content_bounds,
            &self.last_event,
            Color::rgb(200, 200, 200),
            content_bg,
            false,
        );

        if let Some(bp) = layout.bottom_panel_bounds {
            Self::draw_label(
                backend,
                bp,
                "TERMINAL  |  bottom panel (drag top edge to resize)",
                content_fg,
                Color::rgb(25, 25, 28),
                false,
            );
        }

        if let Some(cl) = layout.command_line_bounds {
            Self::draw_label(
                backend,
                cl,
                ":command-line-slot",
                Color::rgb(180, 180, 180),
                Color::rgb(35, 35, 40),
                false,
            );
        }

        if let Some(sb) = layout.status_bar_bounds {
            Self::draw_label(
                backend,
                sb,
                "main  |  UTF-8  |  Ln 1, Col 1",
                status_fg,
                status_bg,
                false,
            );
        }
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
                if ctx.in_title_bar(position.x, position.y) {
                    self.last_event = "Title bar click".into();
                    Reaction::Redraw
                } else if ctx.in_sidebar(position.x, position.y) {
                    self.last_event = format!(
                        "Sidebar click (panel: {})",
                        ctx.active_panel_id.map(|id| id.as_str()).unwrap_or("none")
                    );
                    Reaction::Redraw
                } else if ctx.in_main(position.x, position.y) {
                    self.last_event = "Main area click".into();
                    Reaction::Redraw
                } else if ctx.in_bottom_panel(position.x, position.y) {
                    self.last_event = "Bottom panel click".into();
                    Reaction::Redraw
                } else if ctx.in_command_line(position.x, position.y) {
                    self.last_event = "Command line click".into();
                    Reaction::Redraw
                } else if ctx.in_status_bar(position.x, position.y) {
                    self.last_event = "Status bar click".into();
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
                format!("Sidebar resized: {new_width:.0}px")
            }
            AppShellEvent::BottomPanelResized { new_height } => {
                format!("Bottom panel resized: {new_height:.0}px")
            }
            AppShellEvent::BottomPanelHidden => "Bottom panel hidden".into(),
            AppShellEvent::BottomItemClicked { id } => {
                format!("Bottom item: {}", id.as_str())
            }
            _ => return,
        };
    }
}
