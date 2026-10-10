//! `BottomPanel` demo — adapted from
//! `quadraui/examples/common/bottom_panel_demo.rs`.
//!
//! Drives `compose::bottom_panel::BottomPanelController` directly rather
//! than through `ShellConfig::with_bottom_panel_config` (that path needs
//! a full `AppShell`, which this gallery's own shell already is) — the
//! controller is a stateful value with its own `render`/`handle_click`,
//! so a host can mount it inside any rect, exactly as done here.

use std::cell::RefCell;

use quadraui::compose::bottom_panel::{
    BackendWidget, BottomPanelConfig, BottomPanelController, BottomPanelEvent, BottomPanelTab,
};
use quadraui::{
    Backend, BackendCaps, Color, InteractionState, Reaction, Rect, StatusBar, StatusBarSegment,
    UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("bottom_panel.rs");

// gallery:begin
struct TerminalContent {
    lines: Vec<String>,
}

impl BackendWidget for TerminalContent {
    fn render(&self, backend: &mut dyn Backend, rect: Rect) {
        if rect.height < 1.0 {
            return;
        }
        let lh = backend.line_height();
        let prompt = self
            .lines
            .last()
            .map(|l| format!("$ {l}"))
            .unwrap_or_else(|| "$ _".to_string());
        let bar = StatusBar {
            id: WidgetId::new("gallery:bottom-panel:terminal"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {prompt} "),
                fg: Color::rgb(100, 200, 100),
                bg: Color::rgb(20, 20, 20),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        };
        backend.draw_status_bar_interactive(
            Rect::new(rect.x, rect.y, rect.width, lh),
            &bar,
            &InteractionState::new(),
        );
    }
}

struct ProblemsContent {
    problems: Vec<String>,
}

impl BackendWidget for ProblemsContent {
    fn render(&self, backend: &mut dyn Backend, rect: Rect) {
        if rect.height < 1.0 {
            return;
        }
        let lh = backend.line_height();
        for (i, problem) in self.problems.iter().enumerate() {
            let y = rect.y + i as f32 * lh;
            if y + lh > rect.y + rect.height {
                break;
            }
            let bar = StatusBar {
                id: WidgetId::new(format!("gallery:bottom-panel:problem:{i}")),
                left_segments: vec![StatusBarSegment {
                    text: format!("  \u{26a0} {problem} "),
                    fg: Color::rgb(255, 180, 50),
                    bg: Color::rgb(25, 25, 25),
                    bold: false,
                    action_id: None,
                }],
                right_segments: vec![],
            };
            backend.draw_status_bar_interactive(
                Rect::new(rect.x, y, rect.width, lh),
                &bar,
                &InteractionState::new(),
            );
        }
    }
}

fn build_tabs() -> Vec<BottomPanelTab> {
    vec![
        BottomPanelTab {
            id: "bp:terminal".into(),
            label: "TERMINAL".into(),
            closable: true,
            badge: None,
            content: Box::new(TerminalContent {
                lines: vec!["echo hello".into()],
            }),
        },
        BottomPanelTab {
            id: "bp:problems".into(),
            label: "PROBLEMS".into(),
            closable: true,
            badge: Some("2".into()),
            content: Box::new(ProblemsContent {
                problems: vec![
                    "src/main.rs:12 — unused variable `x`".into(),
                    "src/lib.rs:4 — missing semicolon".into(),
                ],
            }),
        },
    ]
}

/// `variant == 1` starts already maximised, showing the full-height tab
/// strip a user would get after clicking `^`.
fn build_controller(variant: usize) -> BottomPanelController {
    let tabs = build_tabs();
    let active_tab_id = tabs[0].id.clone();
    BottomPanelController::new(BottomPanelConfig {
        tabs,
        active_tab_id,
        maximised: variant == 1,
        height_fraction: 1.0,
    })
}

pub struct BottomPanelDemo {
    // `BottomPanelController::render` needs `&mut self` to cache its hit
    // map for `handle_click` — `Demo::render` only hands out `&self` —
    // so each variant's controller lives behind its own `RefCell`.
    controllers: [RefCell<BottomPanelController>; 2],
    last_event: String,
}

impl BottomPanelDemo {
    pub fn new() -> Self {
        Self {
            controllers: [
                RefCell::new(build_controller(0)),
                RefCell::new(build_controller(1)),
            ],
            last_event: "click a tab, × to close, ^ to maximise".into(),
        }
    }
}

impl Default for BottomPanelDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for BottomPanelDemo {
    fn name(&self) -> &'static str {
        "Bottom Panel"
    }

    fn group(&self) -> &'static str {
        "Chrome"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Two tabs", "Maximised"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let lh = backend.line_height();
        let panel_rect = Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0));
        self.controllers[variant]
            .borrow_mut()
            .render(backend, panel_rect);

        let hint_rect = Rect::new(area.x, area.y + panel_rect.height, area.width, lh);
        let bar = StatusBar {
            id: WidgetId::new("gallery:bottom-panel:hint"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.last_event),
                fg: Color::rgb(200, 200, 200),
                bg: Color::rgb(30, 30, 30),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        };
        let _ = backend.draw_status_bar_interactive(hint_rect, &bar, &InteractionState::new());
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        _backend: &mut dyn Backend,
        _area: Rect,
    ) -> Reaction {
        if let UiEvent::MouseDown { position, .. } = event {
            // `last_hits` / `last_strip_bounds` were populated by the
            // `render` call this same frame already made.
            if let Some(ev) = self.controllers[variant]
                .borrow_mut()
                .handle_click(position.x, position.y)
            {
                self.last_event = match ev {
                    BottomPanelEvent::TabActivated(id) => format!("Tab activated: {id}"),
                    BottomPanelEvent::TabClosed(id) => format!("Tab closed: {id}"),
                    BottomPanelEvent::MaximiseToggled => "Maximise toggled".into(),
                    BottomPanelEvent::Resized(h) => format!("Resized: {h:.0}"),
                };
                return Reaction::Redraw;
            }
        }
        Reaction::Continue
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        let controller = self.controllers[variant].borrow();
        serde_json::json!({
            "active_tab_id": controller.active_tab_id,
            "maximised": controller.maximised,
            "tabs": controller.tabs().iter().map(|t| &t.label).collect::<Vec<_>>(),
        })
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
    fn closing_a_tab_removes_it() {
        let demo = BottomPanelDemo::new();
        assert_eq!(demo.controllers[0].borrow().tabs().len(), 2);
        demo.controllers[0].borrow_mut().close_tab("bp:problems");
        assert_eq!(demo.controllers[0].borrow().tabs().len(), 1);
    }

    #[test]
    fn variant_one_starts_maximised() {
        let demo = BottomPanelDemo::new();
        assert!(!demo.controllers[0].borrow().maximised);
        assert!(demo.controllers[1].borrow().maximised);
    }
}
