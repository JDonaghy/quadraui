//! `Panel` demo — adapted from `quadraui/examples/common/panel_app.rs`.
//!
//! A titled panel with a close and a maximize/collapse action, plus a
//! content area painted into `PanelLayout::content_bounds` the way any
//! `Panel` host paints its own content below the chrome.

use quadraui::{
    Backend, BackendCaps, Color, InteractionState, Panel, PanelAction, PanelHit, Reaction, Rect,
    StatusBar, StatusBarSegment, StyledSpan, StyledText, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("panel.rs");

// gallery:begin
const CONTENT_LINES: &[&str] = &[
    "The quick brown fox jumps over the lazy dog.",
    "Pack my box with five dozen liquor jugs.",
    "How vexingly quick daft zebras jump!",
];

const VARIANT_COUNT: usize = 3;

pub struct PanelDemo {
    // Per-variant collapsed state: variant 0 starts expanded, variant 1
    // starts collapsed, variant 2 (no maximize action) stays expanded.
    collapsed: [bool; VARIANT_COUNT],
    last_message: String,
}

impl PanelDemo {
    pub fn new() -> Self {
        Self {
            collapsed: [false, true, false],
            last_message: "Click the title bar or the actions".into(),
        }
    }

    /// Variant 2 drops the accent colour and the maximize action — a
    /// minimal panel with only a close button, the shape a transient
    /// popover host would use.
    fn panel(&self, variant: usize) -> Panel {
        let collapsed = self.collapsed[variant];
        let mut actions = vec![PanelAction {
            id: WidgetId::new("gallery:panel:close"),
            icon: "×".into(),
            tooltip: "Close".into(),
            is_active: false,
        }];
        if variant != 2 {
            actions.push(PanelAction {
                id: WidgetId::new("gallery:panel:maximize"),
                icon: if collapsed { "+" } else { "□" }.into(),
                tooltip: if collapsed { "Expand" } else { "Maximize" }.into(),
                is_active: collapsed,
            });
        }
        Panel {
            id: WidgetId::new("gallery:panel"),
            title: Some(StyledText {
                spans: vec![StyledSpan::plain("Demo Panel")],
            }),
            actions,
            accent: if variant == 2 {
                None
            } else {
                Some(Color::rgb(40, 80, 120))
            },
            collapsed,
        }
    }

    fn dispatch_action(&mut self, variant: usize, hit: PanelHit) -> bool {
        match hit {
            PanelHit::Action(id) if id.as_str() == "gallery:panel:close" => {
                self.last_message = "Close clicked".into();
                true
            }
            PanelHit::Action(id) if id.as_str() == "gallery:panel:maximize" => {
                self.collapsed[variant] = !self.collapsed[variant];
                self.last_message = if self.collapsed[variant] {
                    "Collapsed".into()
                } else {
                    "Expanded".into()
                };
                true
            }
            PanelHit::TitleBar(_) => {
                self.last_message = "Title bar clicked".into();
                true
            }
            _ => false,
        }
    }

    fn hint_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("gallery:panel:hint"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.last_message),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }
}

impl Default for PanelDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for PanelDemo {
    fn name(&self) -> &'static str {
        "Panel"
    }

    fn group(&self) -> &'static str {
        "Chrome"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Expanded", "Collapsed", "No accent, close only"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let lh = backend.line_height();
        let panel_rect = Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0));
        let panel = self.panel(variant);
        let layout = backend.draw_panel(panel_rect, &panel);

        let cb = layout.content_bounds;
        for (i, line) in CONTENT_LINES.iter().enumerate() {
            let y = cb.y + i as f32 * lh;
            if y + lh > cb.y + cb.height {
                break;
            }
            let bar = StatusBar {
                id: WidgetId::new(format!("gallery:panel:line:{i}")),
                left_segments: vec![StatusBarSegment {
                    text: format!(" {line} "),
                    fg: Color::rgb(210, 210, 210),
                    bg: Color::rgb(20, 20, 35),
                    bold: false,
                    action_id: None,
                }],
                right_segments: vec![],
            };
            let _ = backend.draw_status_bar_interactive(
                Rect::new(cb.x, y, cb.width, lh),
                &bar,
                &InteractionState::new(),
            );
        }

        let hint_rect = Rect::new(area.x, area.y + panel_rect.height, area.width, lh);
        let _ = backend.draw_status_bar_interactive(
            hint_rect,
            &self.hint_bar(),
            &InteractionState::new(),
        );
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        match event {
            UiEvent::MouseDown { position, .. } => {
                let lh = backend.line_height();
                let panel_rect = Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0));
                let panel = self.panel(variant);
                let layout = backend.panel_layout(panel_rect, &panel);
                let hit = layout.hit_test(position.x, position.y);
                if self.dispatch_action(variant, hit) {
                    Reaction::Redraw
                } else {
                    Reaction::Continue
                }
            }
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        serde_json::to_value(self.panel(variant)).unwrap_or(serde_json::Value::Null)
    }

    fn caps_note(&self, _variant: usize, _caps: &BackendCaps) -> Option<String> {
        None
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maximize_action_toggles_collapsed() {
        let mut demo = PanelDemo::new();
        assert!(!demo.panel(0).collapsed);
        let maximize = WidgetId::new("gallery:panel:maximize");
        assert!(demo.dispatch_action(0, PanelHit::Action(maximize.clone())));
        assert!(demo.panel(0).collapsed);
        assert!(demo.dispatch_action(0, PanelHit::Action(maximize)));
        assert!(!demo.panel(0).collapsed);
    }

    #[test]
    fn variant_two_has_no_maximize_action() {
        let demo = PanelDemo::new();
        let panel = demo.panel(2);
        assert!(panel.accent.is_none());
        assert!(!panel
            .actions
            .iter()
            .any(|a| a.id.as_str() == "gallery:panel:maximize"));
    }
}
