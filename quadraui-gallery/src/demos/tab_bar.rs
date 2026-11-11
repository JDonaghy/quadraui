//! `TabBar` demo — adapted from `quadraui/examples/common/tab_chrome_demo.rs`,
//! `tab_icons_demo.rs` and `wide_tab_bar_demo.rs`.
//!
//! Three variants show sidecar decorations that are requested through a
//! `Backend::draw_tab_bar_*` call rather than baked into `TabItem` fields:
//! bracket chrome around the active tab (`TabChrome`), per-tab colour icons
//! (`TabIcon`), and a CJK double-width label (a vt100 conformance fixture).

use quadraui::{
    Backend, BackendCaps, Color, Reaction, Rect, Style, TabBar, TabBarHit, TabChrome, TabFrame,
    TabIcon, TabItem, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("tab_bar.rs");

/// `Style`'s chrome-metric `token` (`tab_height`, `status_bar_height`,
/// `toolbar_height`, …), floored at the chrome line height on a pixel
/// backend — never shrunk below it, so a chrome font taller than the
/// design token isn't clipped — or the plain chrome line height on TUI's
/// fixed cell grid, where every `Style` chrome metric is a documented
/// no-op. Mirrors `AppShell`'s own token-vs-line-height carve
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
const CHROME_LABELS: [&str; 2] = ["main.rs", "lib.rs"];
const ICON_LABELS: [&str; 3] = [" main.rs ", " Cargo.toml ", " README.md "];
const WIDE_LABEL: &str = " 1: 日本語.rs ";
const ASCII_LABEL: &str = " 2: main.rs ";

pub struct TabBarDemo {
    chrome_active: usize,
    chrome_action: String,
    icons_active: usize,
    icons_on: bool,
    wide_active: usize,
}

impl TabBarDemo {
    pub fn new() -> Self {
        Self {
            chrome_active: 0,
            chrome_action: "ready".into(),
            icons_active: 0,
            icons_on: true,
            wide_active: 0,
        }
    }

    // ── Variant 0: bracket chrome ───────────────────────────────────

    fn chrome_bar(&self) -> TabBar {
        TabBar {
            id: WidgetId::new("gallery:tab-bar:chrome"),
            tabs: CHROME_LABELS
                .iter()
                .enumerate()
                .map(|(i, label)| TabItem {
                    label: (*label).to_string(),
                    is_active: i == self.chrome_active,
                    is_closable: i == self.chrome_active,
                    ..Default::default()
                })
                .collect(),
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: true,
            compact: false,
        }
    }

    fn chrome(&self) -> TabChrome {
        TabChrome::new(TabFrame::Brackets)
    }

    // ── Variant 1: per-tab icons ─────────────────────────────────────

    fn icons(&self) -> Vec<Option<TabIcon>> {
        vec![
            Some(TabIcon {
                glyph: "R".to_string(),
                color: Color::rgb(222, 165, 132),
            }),
            Some(TabIcon {
                glyph: "T".to_string(),
                color: Color::rgb(160, 190, 210),
            }),
            Some(TabIcon {
                glyph: "M".to_string(),
                color: Color::rgb(120, 170, 240),
            }),
        ]
    }

    fn icons_bar(&self) -> TabBar {
        TabBar {
            id: WidgetId::new("gallery:tab-bar:icons"),
            tabs: ICON_LABELS
                .iter()
                .enumerate()
                .map(|(i, label)| TabItem {
                    label: (*label).to_string(),
                    is_active: i == self.icons_active,
                    ..Default::default()
                })
                .collect(),
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: None,
            show_tab_close: true,
            compact: false,
        }
    }

    // ── Variant 2: wide (CJK) glyph ───────────────────────────────────

    fn wide_bar(&self) -> TabBar {
        TabBar {
            id: WidgetId::new("gallery:tab-bar:wide"),
            tabs: vec![
                TabItem {
                    label: WIDE_LABEL.to_string(),
                    is_active: self.wide_active == 0,
                    is_closable: true,
                    ..Default::default()
                },
                TabItem {
                    label: ASCII_LABEL.to_string(),
                    is_active: self.wide_active == 1,
                    is_closable: true,
                    ..Default::default()
                },
            ],
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: Some(Color::rgb(80, 160, 240)),
            show_tab_close: true,
            compact: false,
        }
    }

    /// `area`'s top strip, sized off `Style::tab_height` on a pixel
    /// backend (floored at the chrome line height so a chrome font taller
    /// than the token isn't clipped) and off the plain chrome line height
    /// on TUI's fixed cell grid — mirrors `AppShell`'s own tab-strip
    /// sizing (`compose/shell_layout.rs`'s `header_h`).
    fn rect(area: Rect, backend: &dyn Backend) -> Rect {
        let h = chrome_strip_height(backend, |s| s.tab_height);
        Rect::new(area.x, area.y, area.width, h)
    }

    fn toggle_icons(&mut self) {
        self.icons_on = !self.icons_on;
    }

    fn close_chrome_tab(&mut self, i: usize) {
        self.chrome_action = format!("closed {}", CHROME_LABELS[i]);
    }

    fn activate_chrome_tab(&mut self, i: usize) {
        self.chrome_active = i;
        self.chrome_action = format!("activated {}", CHROME_LABELS[i]);
    }
}

impl Default for TabBarDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for TabBarDemo {
    fn name(&self) -> &'static str {
        "Tab Bar"
    }

    fn group(&self) -> &'static str {
        "Chrome"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Chrome frame", "Icons", "Wide glyphs"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let rect = Self::rect(area, backend);
        match variant {
            0 => {
                backend.draw_tab_bar_with_chrome_layout(
                    rect,
                    &self.chrome_bar(),
                    None,
                    &self.chrome(),
                );
            }
            1 => {
                let icons = if self.icons_on { self.icons() } else { vec![] };
                backend.draw_tab_bar_icons_layout(rect, &self.icons_bar(), &icons, None);
            }
            _ => {
                backend.draw_tab_bar_layout(rect, &self.wide_bar(), None);
            }
        }
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        let rect = Self::rect(area, backend);
        match event {
            UiEvent::MouseDown { position, .. } => {
                if position.y < rect.y || position.y >= rect.y + rect.height {
                    return Reaction::Continue;
                }
                let local_x = position.x - rect.x;
                let local_y = position.y - rect.y;
                match variant {
                    0 => {
                        let layout = backend.resolve_tab_bar_layout_with_chrome(
                            rect,
                            &self.chrome_bar(),
                            &self.chrome(),
                        );
                        match layout.hit_test(local_x, local_y) {
                            TabBarHit::TabClose(i) => {
                                self.close_chrome_tab(i);
                                Reaction::Redraw
                            }
                            TabBarHit::Tab(i) => {
                                self.activate_chrome_tab(i);
                                Reaction::Redraw
                            }
                            _ => Reaction::Continue,
                        }
                    }
                    1 => {
                        let icons = if self.icons_on { self.icons() } else { vec![] };
                        let layout =
                            backend.resolve_tab_bar_layout_icons(rect, &self.icons_bar(), &icons);
                        if let TabBarHit::Tab(i) = layout.hit_test(local_x, local_y) {
                            self.icons_active = i;
                            Reaction::Redraw
                        } else {
                            Reaction::Continue
                        }
                    }
                    _ => {
                        let layout = backend.resolve_tab_bar_layout(rect, &self.wide_bar());
                        if let TabBarHit::Tab(i) = layout.hit_test(local_x, local_y) {
                            self.wide_active = i;
                            Reaction::Redraw
                        } else {
                            Reaction::Continue
                        }
                    }
                }
            }
            UiEvent::KeyPressed {
                key: quadraui::Key::Char('i'),
                ..
            } if variant == 1 => {
                self.toggle_icons();
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        match variant {
            0 => {
                let mut value =
                    serde_json::to_value(self.chrome_bar()).unwrap_or(serde_json::Value::Null);
                if let Some(obj) = value.as_object_mut() {
                    obj.insert(
                        "chrome_action".into(),
                        serde_json::Value::String(self.chrome_action.clone()),
                    );
                }
                value
            }
            1 => serde_json::to_value(self.icons_bar()).unwrap_or(serde_json::Value::Null),
            _ => serde_json::to_value(self.wide_bar()).unwrap_or(serde_json::Value::Null),
        }
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
    fn icons_toggle_flips_icons_on() {
        let mut demo = TabBarDemo::new();
        assert!(demo.icons_on);
        demo.toggle_icons();
        assert!(!demo.icons_on);
    }

    #[test]
    fn closing_the_active_chrome_tab_records_the_action() {
        let mut demo = TabBarDemo::new();
        assert_eq!(demo.chrome_action, "ready");
        demo.close_chrome_tab(0);
        assert_eq!(demo.chrome_action, format!("closed {}", CHROME_LABELS[0]));
    }

    /// On a pixel backend (a GTK-shaped chrome line height, simulated via
    /// `RecordingBackend`), the painted tab-bar rect is exactly
    /// `Style::tab_height` tall — not the raw chrome line height, the bug
    /// this test guards against regressing.
    #[test]
    fn rect_height_matches_tab_height_token_on_a_pixel_backend() {
        let backend = quadraui::testing::RecordingBackend {
            chrome_line_height: Some(20.0),
            ..quadraui::testing::RecordingBackend::new()
        };
        let area = Rect::new(0.0, 0.0, 200.0, 100.0);
        let rect = TabBarDemo::rect(area, &backend);
        assert_eq!(rect.height, Style::default().tab_height);
    }

    #[test]
    fn activating_a_chrome_tab_updates_both_active_index_and_action() {
        let mut demo = TabBarDemo::new();
        assert_eq!(demo.chrome_active, 0);
        demo.activate_chrome_tab(1);
        assert_eq!(demo.chrome_active, 1);
        assert_eq!(
            demo.chrome_action,
            format!("activated {}", CHROME_LABELS[1])
        );
    }
}

#[cfg(all(test, feature = "tui"))]
mod tui_tests {
    use super::*;

    /// TUI's fixed cell grid keeps its one-row degrade — `Style::tab_height`
    /// is a documented no-op there.
    #[test]
    fn rect_height_stays_one_chrome_line_on_tui() {
        let backend = quadraui::tui::TuiBackend::new();
        let area = Rect::new(0.0, 0.0, 80.0, 24.0);
        let rect = TabBarDemo::rect(area, &backend);
        assert_eq!(rect.height, backend.chrome_line_height());
    }
}
