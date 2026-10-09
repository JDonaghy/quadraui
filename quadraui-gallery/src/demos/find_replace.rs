//! `Find & Replace` demo — adapted from
//! `quadraui/examples/common/find_replace_app.rs`.
//!
//! Exercises `FindReplacePanel::hit_test`: clicking a toggle/nav/close
//! button reports which `FindReplaceClickTarget` was hit. Two variants
//! show the panel at its two documented widths — compact and wide.
//!
//! Click routing scales the panel's column/row geometry by the running
//! backend's real [`Backend::char_width`] / [`Backend::line_height`]
//! (1.0/1.0 on TUI, real pixel metrics on GTK/macOS/Win), so a click
//! lands in the right neighbourhood of cells on every backend rather
//! than being off by whole screens on a pixel backend. It is still an
//! **approximation** of each backend's actual panel position: every
//! backend's `draw_find_replace` anchors the popup with its own formula
//! (TUI clamps to whole cells; the shared GUI `paint()` uses a fixed
//! 10px right inset and 2px top gap that don't reduce to a `char_width`/
//! `line_height` multiple), and there is no `Backend`-owned method yet
//! that reports a backend's resolved content origin back to the caller.
//! A consumer that needs pixel-exact routing on a GUI backend should
//! track that gap as a follow-up rather than copy this approximation
//! verbatim.

use quadraui::{
    compute_find_replace_hit_regions, Backend, BackendCaps, Color, FindReplaceClickTarget,
    FindReplaceHit, FindReplacePanel, InteractionState, MouseButton, Reaction, Rect, StatusBar,
    StatusBarSegment, UiEvent, WidgetId, FR_PANEL_WIDTH,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("find_replace.rs");

// gallery:begin
/// Narrower than [`FR_PANEL_WIDTH`] — the "Compact" variant.
const COMPACT_WIDTH: u16 = 36;

pub struct FindReplaceDemo {
    panel: FindReplacePanel,
    last_click: Option<String>,
}

impl FindReplaceDemo {
    pub fn new() -> Self {
        Self {
            panel: FindReplacePanel {
                query: "needle".into(),
                replacement: String::new(),
                show_replace: true,
                focus: 0,
                cursor: 6,
                sel_anchor: None,
                match_info: "1 of 3".into(),
                case_sensitive: false,
                whole_word: false,
                use_regex: false,
                preserve_case: false,
                in_selection: false,
                group_bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
                panel_width: FR_PANEL_WIDTH,
                replace_one_glyph: "R1".into(),
                replace_all_glyph: "R*".into(),
                hit_regions: Vec::new(),
            },
            last_click: None,
        }
    }

    fn panel_width_for(variant: usize) -> u16 {
        if variant == 0 {
            COMPACT_WIDTH
        } else {
            FR_PANEL_WIDTH
        }
    }

    /// Build the panel actually painted/hit-tested for `variant`:
    /// `self.panel`'s persisted fields (query, toggles, `show_replace`,
    /// ...) with `panel_width` and `group_bounds` set for this variant
    /// and `hit_regions` recomputed to match — `hit_regions` is laid out
    /// *for a specific width*, so reusing the regions computed for one
    /// variant while painting another would paint/hit-test columns that
    /// don't exist in the narrower box.
    fn panel_for(&self, variant: usize, group_bounds: Rect) -> FindReplacePanel {
        let panel_width = Self::panel_width_for(variant);
        let (hit_regions, _input_w) = compute_find_replace_hit_regions(
            panel_width,
            self.panel.show_replace,
            &self.panel.match_info,
            self.panel.replace_one_glyph.chars().count() as u16,
            self.panel.replace_all_glyph.chars().count() as u16,
        );
        let mut panel = self.panel.clone();
        panel.panel_width = panel_width;
        panel.group_bounds = group_bounds;
        panel.hit_regions = hit_regions;
        panel
    }

    /// Same `area` `render` paints the panel into, used to keep
    /// `panel.group_bounds` and click routing in agreement.
    fn editor_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0))
    }

    /// The panel's content corner (inside its one-cell border), in the
    /// same absolute coordinates the rasteriser paints at, scaled by the
    /// real `char_width`/`line_height` of the backend that is about to
    /// hit-test against it (see the module doc for why this is a
    /// cross-backend *approximation*, not each backend's exact anchor
    /// formula).
    fn content_origin(
        panel: &FindReplacePanel,
        rect: Rect,
        char_width: f32,
        line_height: f32,
    ) -> (f32, f32) {
        let cw = char_width.max(1.0);
        let lh = line_height.max(1.0);
        let panel_w = (panel.panel_width as f32 * cw).min((rect.width - 2.0 * cw).max(0.0));
        let gb = &panel.group_bounds;
        let gb_right = rect.x + gb.x + gb.width;
        let x = (gb_right - (panel_w + cw)).max(rect.x);
        let y = gb.y.max(lh);
        (x + cw, y + lh)
    }

    fn status_bar(&self) -> StatusBar {
        let msg = match &self.last_click {
            Some(m) => format!(" {m} "),
            None => " click a toggle or nav button ".into(),
        };
        StatusBar {
            id: WidgetId::new("gallery:find-replace:status"),
            left_segments: vec![StatusBarSegment {
                text: msg,
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }
}

impl Default for FindReplaceDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for FindReplaceDemo {
    fn name(&self) -> &'static str {
        "Find & Replace"
    }

    fn group(&self) -> &'static str {
        "Content"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Compact", "Wide"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let rect = Self::editor_rect(area, backend);
        let panel = self.panel_for(variant, Rect::new(0.0, 0.0, rect.width, rect.height));
        backend.draw_find_replace(rect, &panel);

        let lh = backend.line_height();
        let status_rect = Rect::new(area.x, rect.y + rect.height, area.width, lh);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status_bar(),
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
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => {
                let rect = Self::editor_rect(area, backend);
                let group_bounds = Rect::new(0.0, 0.0, rect.width, rect.height);
                let probe = self.panel_for(variant, group_bounds);
                let char_width = backend.char_width();
                let line_height = backend.line_height();
                let origin = Self::content_origin(&probe, rect, char_width, line_height);
                let hit = probe.hit_test(position.x, position.y, origin, char_width, line_height);
                self.panel.group_bounds = group_bounds;
                self.last_click = Some(match hit {
                    FindReplaceHit::Target(target) => {
                        match target {
                            FindReplaceClickTarget::ToggleCase => {
                                self.panel.case_sensitive = !self.panel.case_sensitive
                            }
                            FindReplaceClickTarget::ToggleWholeWord => {
                                self.panel.whole_word = !self.panel.whole_word
                            }
                            FindReplaceClickTarget::ToggleRegex => {
                                self.panel.use_regex = !self.panel.use_regex
                            }
                            FindReplaceClickTarget::TogglePreserveCase => {
                                self.panel.preserve_case = !self.panel.preserve_case
                            }
                            FindReplaceClickTarget::ToggleInSelection => {
                                self.panel.in_selection = !self.panel.in_selection
                            }
                            FindReplaceClickTarget::Chevron => {
                                self.panel.show_replace = !self.panel.show_replace;
                            }
                            _ => {}
                        }
                        format!("clicked {target:?}")
                    }
                    FindReplaceHit::Empty => "clicked empty area".into(),
                });
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        // `FindReplacePanel` doesn't derive `Serialize` (it's not meant
        // to round-trip, unlike most primitives) — report the fields a
        // consumer would actually care about by hand.
        serde_json::json!({
            "query": self.panel.query,
            "replacement": self.panel.replacement,
            "show_replace": self.panel.show_replace,
            "panel_width": Self::panel_width_for(variant),
            "case_sensitive": self.panel.case_sensitive,
            "whole_word": self.panel.whole_word,
            "use_regex": self.panel.use_regex,
            "preserve_case": self.panel.preserve_case,
            "in_selection": self.panel.in_selection,
            "match_info": self.panel.match_info,
            "last_click": self.last_click,
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
    fn panel_width_for_differs_between_variants() {
        assert!(FindReplaceDemo::panel_width_for(0) < FindReplaceDemo::panel_width_for(1));
    }

    #[test]
    fn chevron_click_toggles_show_replace_via_hit_test() {
        let demo = FindReplaceDemo::new();
        assert!(demo.panel.show_replace);
        let panel = demo.panel_for(0, Rect::new(0.0, 0.0, 80.0, 20.0));
        // Find the chevron's own region and click its center directly
        // (cell units: `col/row + 1` for the 1-cell border, `cell_w`/
        // `cell_h` both `1.0`), bypassing backend geometry so this test
        // stays independent of any particular rasteriser's pixel math.
        let (region, _) = panel
            .hit_regions
            .iter()
            .find(|(_, target)| *target == FindReplaceClickTarget::Chevron)
            .expect("chevron region should exist");
        let origin = (0.0, 0.0);
        let x = origin.0 + region.col as f32 + region.width as f32 / 2.0;
        let y = origin.1 + region.row as f32 + 0.5;
        match panel.hit_test(x, y, origin, 1.0, 1.0) {
            FindReplaceHit::Target(FindReplaceClickTarget::Chevron) => {}
            other => panic!("expected to hit the chevron, got {other:?}"),
        }
    }

    #[test]
    fn panel_for_recomputes_hit_regions_at_the_requested_width() {
        let demo = FindReplaceDemo::new();
        let compact = demo.panel_for(0, Rect::new(0.0, 0.0, 80.0, 20.0));
        let wide = demo.panel_for(1, Rect::new(0.0, 0.0, 80.0, 20.0));
        assert_eq!(compact.panel_width, COMPACT_WIDTH);
        assert_eq!(wide.panel_width, FR_PANEL_WIDTH);
        assert!(!compact.hit_regions.is_empty());
        assert!(!wide.hit_regions.is_empty());
    }
}
