//! Backend-agnostic app code for the panel example
//! ([`tui_panel`] / [`gtk_panel`]).
//!
//! [`PanelApp`] demonstrates a [`Panel`] with a title bar, close and
//! maximize action buttons, and a content area showing a label.
//!
//! Controls:
//! - click close button        quit
//! - click maximize button     toggle collapsed
//! - click title bar           log "title clicked"
//! - c                         toggle collapsed
//! - q / Esc                   quit

use quadraui::{
    AppLogic, Backend, Color, Key, NamedKey, Panel, PanelAction, PanelHit, Reaction, Rect,
    StatusBar, StatusBarSegment, StyledSpan, StyledText, UiEvent, WidgetId,
};

pub struct PanelApp {
    collapsed: bool,
    last_message: String,
}

impl PanelApp {
    pub fn new() -> Self {
        Self {
            collapsed: false,
            last_message: "Click panel chrome or press keys".into(),
        }
    }

    fn panel(&self) -> Panel {
        Panel {
            id: WidgetId::new("demo-panel"),
            title: Some(StyledText {
                spans: vec![StyledSpan::plain("Demo Panel")],
            }),
            actions: vec![
                PanelAction {
                    id: WidgetId::new("close"),
                    icon: "×".into(),
                    tooltip: "Close".into(),
                    is_active: false,
                },
                PanelAction {
                    id: WidgetId::new("maximize"),
                    icon: if self.collapsed { "+" } else { "□" }.into(),
                    tooltip: if self.collapsed { "Expand" } else { "Maximize" }.into(),
                    is_active: self.collapsed,
                },
            ],
            accent: Some(Color::rgb(40, 80, 120)),
            collapsed: self.collapsed,
        }
    }

    fn status_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.last_message),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " c=collapse | q=quit ".into(),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    fn fill_content(&self, backend: &mut dyn Backend, bounds: Rect) {
        if bounds.width < 1.0 || bounds.height < 1.0 {
            return;
        }
        let lh = backend.line_height();
        let label_rect = Rect::new(bounds.x, bounds.y, bounds.width, lh);
        let bar = StatusBar {
            id: WidgetId::new("content-label"),
            left_segments: vec![StatusBarSegment {
                text: " Panel content area ".into(),
                fg: Color::rgb(200, 200, 200),
                bg: Color::rgb(30, 30, 50),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        };
        let _ = backend.draw_status_bar(label_rect, &bar);
    }
}

impl Default for PanelApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for PanelApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let panel_rect = Rect::new(0.0, 0.0, viewport.width, viewport.height - lh);
        let panel = self.panel();
        let layout = backend.draw_panel(panel_rect, &panel);

        self.fill_content(backend, layout.content_bounds);

        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let _ = backend.draw_status_bar(status_rect, &self.status_bar());
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
                key: Key::Char('c'),
                ..
            } => {
                self.collapsed = !self.collapsed;
                self.last_message = if self.collapsed {
                    "Collapsed".into()
                } else {
                    "Expanded".into()
                };
                Reaction::Redraw
            }
            UiEvent::MouseDown { position, .. } => {
                let viewport = backend.viewport();
                let lh = backend.line_height();
                let panel_rect = Rect::new(0.0, 0.0, viewport.width, viewport.height - lh);
                let panel = self.panel();
                let layout = backend.panel_layout(panel_rect, &panel);
                match layout.hit_test(position.x, position.y) {
                    PanelHit::Action(id) if id.as_str() == "close" => {
                        return Reaction::Exit;
                    }
                    PanelHit::Action(id) if id.as_str() == "maximize" => {
                        self.collapsed = !self.collapsed;
                        self.last_message = if self.collapsed {
                            "Collapsed".into()
                        } else {
                            "Expanded".into()
                        };
                    }
                    PanelHit::TitleBar(_) => {
                        self.last_message = "Title bar clicked".into();
                    }
                    PanelHit::Content(_) => {
                        self.last_message = "Content clicked".into();
                    }
                    _ => {}
                }
                Reaction::Redraw
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
