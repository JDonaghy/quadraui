//! `TabBar` demo — adapted from `quadraui/examples/common/tab_chrome_demo.rs`,
//! `tab_icons_demo.rs` and `wide_tab_bar_demo.rs`.
//!
//! Three variants show sidecar decorations that are requested through a
//! `Backend::draw_tab_bar_*` call rather than baked into `TabItem` fields:
//! bracket chrome around the active tab (`TabChrome`, quadraui#631),
//! per-tab colour icons (`TabIcon`, quadraui#620), and a CJK double-width
//! label (the vt100 conformance fixture for quadraui#555).

use quadraui::{
    Backend, BackendCaps, Color, Reaction, Rect, TabBar, TabBarHit, TabChrome, TabFrame, TabIcon,
    TabItem, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("tab_bar.rs");

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

    fn rect(area: Rect, backend: &dyn Backend) -> Rect {
        Rect::new(area.x, area.y, area.width, backend.line_height())
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
                                self.chrome_action = format!("closed {}", CHROME_LABELS[i]);
                                Reaction::Redraw
                            }
                            TabBarHit::Tab(i) => {
                                self.chrome_active = i;
                                self.chrome_action = format!("activated {}", CHROME_LABELS[i]);
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
                self.icons_on = !self.icons_on;
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
            0 => serde_json::to_value(self.chrome_bar()).unwrap_or(serde_json::Value::Null),
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
        demo.icons_on = !demo.icons_on;
        assert!(!demo.icons_on);
    }
}
