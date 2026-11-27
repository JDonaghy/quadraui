//! `BottomPanel` demo — adapted from
//! `quadraui/examples/common/bottom_panel_demo.rs`.
//!
//! Drives the `BottomPanel` descriptor directly rather than through
//! `ShellConfig::with_bottom_panel_config` (that path needs a full
//! `ShellView`, which this gallery's own shell already is): the demo owns
//! the descriptor, its state and each tab's content, paints the strip
//! through `draw_dyn`, resolves clicks with `BottomPanel::click` and
//! applies what they ask for — so a host can mount it inside any rect,
//! exactly as done here.

use quadraui::DrawsPrimitives;
use std::cell::RefCell;

use quadraui::compose::bottom_panel::{
    BackendWidget, BottomPanel, BottomPanelConfig, BottomPanelEvent, BottomPanelPrimitiveLayout,
    BottomPanelState, BottomPanelTab,
};
use quadraui::{
    Backend, BackendCaps, Color, Handled, Reaction, Rect, StatusBar, StatusBarSegment, Style,
    UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("bottom_panel.rs");

/// `Style`'s chrome-metric `token`, floored at the chrome line height on
/// a pixel backend — never shrunk below it, so a chrome font taller than
/// the design token isn't clipped — or the plain chrome line height on
/// TUI's fixed cell grid, where every `Style` chrome metric is a
/// documented no-op. Mirrors `ShellView`'s own token-vs-line-height carve
/// (`compose::shell_layout::compute_layout`'s `header_h`/`status_h`).
fn chrome_strip_height(backend: &dyn Backend, token: impl Fn(&Style) -> f32) -> f32 {
    let lh = backend.chrome_line_height();
    if lh <= 1.0 {
        backend.snap_height(lh)
    } else {
        backend.snap_height(lh.max(token(&backend.style())))
    }
}

// gallery:begin
struct TerminalContent {
    lines: Vec<String>,
}

impl BackendWidget for TerminalContent {
    fn render(&self, backend: &mut dyn Backend, rect: Rect) {
        if rect.height < 1.0 {
            return;
        }
        let row_h = chrome_strip_height(backend, |s| s.status_bar_height);
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
        backend.draw_dyn(Rect::new(rect.x, rect.y, rect.width, row_h), &bar, &());
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
        let row_h = chrome_strip_height(backend, |s| s.status_bar_height);
        for (i, problem) in self.problems.iter().enumerate() {
            let y = rect.y + i as f32 * row_h;
            if y + row_h > rect.y + rect.height {
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
            backend.draw_dyn(Rect::new(rect.x, y, rect.width, row_h), &bar, &());
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

/// One mounted panel: the descriptor, its state, each tab's content and
/// the layout the last frame painted, which clicks resolve against.
struct MountedPanel {
    panel: BottomPanel,
    state: BottomPanelState,
    contents: Vec<BottomPanelTab>,
    last_layout: Option<BottomPanelPrimitiveLayout>,
}

impl MountedPanel {
    /// `variant == 1` starts already maximised, showing the full-height
    /// tab strip a user would get after clicking `^`.
    fn new(variant: usize) -> Self {
        let tabs = build_tabs();
        let active_tab_id = tabs[0].id.clone();
        let config = BottomPanelConfig {
            tabs,
            active_tab_id,
            maximised: variant == 1,
            height_fraction: 1.0,
        };
        Self {
            panel: BottomPanel::from_config(&config),
            state: BottomPanelState::default(),
            contents: config.tabs,
            last_layout: None,
        }
    }

    fn render(&mut self, backend: &mut dyn Backend, rect: Rect) {
        self.last_layout = backend
            .draw_dyn(rect, &self.panel, &())
            .downcast_ref::<BottomPanelPrimitiveLayout>()
            .cloned();
        let Some(content) = self.last_layout.as_ref().map(|l| l.content_bounds) else {
            return;
        };
        if content.width > 0.0 && content.height > 0.0 {
            if let Some(tab) = self
                .contents
                .iter()
                .find(|t| t.id == self.panel.active_tab_id)
            {
                tab.content.render(backend, content);
            }
        }
    }

    fn click(&mut self, position: quadraui::Point) -> Option<BottomPanelEvent> {
        let layout = self.last_layout.as_ref()?;
        match self.panel.click(&mut self.state, layout, position) {
            Handled::Event(event) => {
                self.panel.apply(&event);
                Some(event)
            }
            _ => None,
        }
    }
}

pub struct BottomPanelDemo {
    // Painting records the layout clicks resolve against, and
    // `Demo::render` only hands out `&self`, so each variant's panel lives
    // behind its own `RefCell`.
    panels: [RefCell<MountedPanel>; 2],
    last_event: String,
}

impl BottomPanelDemo {
    pub fn new() -> Self {
        Self {
            panels: [
                RefCell::new(MountedPanel::new(0)),
                RefCell::new(MountedPanel::new(1)),
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
        let row_h = chrome_strip_height(backend, |s| s.status_bar_height);
        let panel_rect = Rect::new(area.x, area.y, area.width, (area.height - row_h).max(0.0));
        self.panels[variant]
            .borrow_mut()
            .render(backend, panel_rect);

        let hint_rect = Rect::new(area.x, area.y + panel_rect.height, area.width, row_h);
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
        let _ = backend.draw_dyn(hint_rect, &bar, &());
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        _backend: &mut dyn Backend,
        _area: Rect,
    ) -> Reaction {
        if let UiEvent::MouseDown { position, .. } = event {
            // The layout was recorded by the `render` call this same
            // frame already made.
            if let Some(ev) = self.panels[variant].borrow_mut().click(*position) {
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
        let mounted = self.panels[variant].borrow();
        let panel = &mounted.panel;
        serde_json::json!({
            "active_tab_id": panel.active_tab_id,
            "maximised": panel.maximised,
            "tabs": panel.tabs.iter().map(|t| &t.label).collect::<Vec<_>>(),
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
        assert_eq!(demo.panels[0].borrow().panel.tabs.len(), 2);
        demo.panels[0].borrow_mut().panel.close_tab("bp:problems");
        assert_eq!(demo.panels[0].borrow().panel.tabs.len(), 1);
    }

    #[test]
    fn variant_one_starts_maximised() {
        let demo = BottomPanelDemo::new();
        assert!(!demo.panels[0].borrow().panel.maximised);
        assert!(demo.panels[1].borrow().panel.maximised);
    }

    /// On a pixel backend (a GTK-shaped chrome line height, simulated via
    /// `RecordingBackend`), the hint strip below the panel — and every
    /// `StatusBar`-shaped row painted inside a tab's own content (the
    /// terminal prompt, a problem row) — is exactly `Style::status_bar_height`
    /// tall, not the raw chrome line height, the bug this test guards
    /// against regressing.
    #[test]
    fn row_height_matches_status_bar_height_token_on_a_pixel_backend() {
        let backend = quadraui::testing::RecordingBackend {
            chrome_line_height: Some(20.0),
            ..quadraui::testing::RecordingBackend::new()
        };
        assert_eq!(
            chrome_strip_height(&backend, |s| s.status_bar_height),
            quadraui::Style::default().status_bar_height
        );
    }
}
