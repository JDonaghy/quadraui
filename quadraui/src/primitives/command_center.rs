//! `CommandCenter` primitive: a horizontal strip with back/forward nav
//! arrows and a clickable search box. Lives in the menu bar row,
//! centered between the menu labels and any trailing chrome (window
//! controls, etc.).

use crate::event::Rect;
use crate::types::WidgetId;
use serde::{Deserialize, Serialize};

/// Declarative description of a command center strip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandCenter {
    pub id: WidgetId,
    pub back_enabled: bool,
    pub forward_enabled: bool,
    /// Text shown inside the search box (e.g. "🔍 project-name").
    /// Empty string hides the search box entirely.
    #[serde(default)]
    pub search_label: String,
}

/// Measurement for a `CommandCenter`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommandCenterMeasure {
    /// Width of each nav arrow slot.
    pub arrow_width: f32,
    /// Gap between arrows and between arrow group and search box.
    pub gap: f32,
    /// Width of the search box. `0.0` when `search_label` is empty.
    pub search_box_width: f32,
    /// Height of the command center (matches the row).
    pub height: f32,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandCenterHit {
    Back,
    Forward,
    SearchBox,
    Bar,
    Outside,
}

/// Fully-resolved command-center layout.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandCenterLayout {
    pub bounds: Rect,
    pub back_bounds: Option<Rect>,
    pub forward_bounds: Option<Rect>,
    pub search_bounds: Option<Rect>,
    pub hit_regions: Vec<(Rect, CommandCenterHit)>,
}

impl CommandCenterLayout {
    /// A layout for an area the rasteriser did not (or could not) paint
    /// into: no hit regions at all, so [`Self::hit_test`] returns
    /// [`CommandCenterHit::Outside`] for every point. Use this instead of
    /// [`CommandCenter::layout`]'s geometric result whenever the paint
    /// loop skips the widget entirely — a `width == 0` / `height == 0`
    /// area, for instance — so the returned layout never describes cells
    /// that were never actually drawn (quadraui#649).
    pub fn empty(bounds: Rect) -> Self {
        CommandCenterLayout {
            bounds,
            back_bounds: None,
            forward_bounds: None,
            search_bounds: None,
            hit_regions: Vec::new(),
        }
    }

    pub fn hit_test(&self, x: f32, y: f32) -> CommandCenterHit {
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        CommandCenterHit::Outside
    }
}

impl CommandCenterMeasure {
    /// Standard nav-arrow width (px/DIPs) for every pixel backend that
    /// measures the search box from a plain `char_width` average rather
    /// than a live font layout — see [`Self::from_char_width`]. Matches
    /// the value `gtk_command_center_layout` / `mac_command_center_layout`
    /// use for their own (pixel-exact) measurement, so a command center
    /// looks the same size across backends regardless of which
    /// measurement path produced its layout.
    pub const ARROW_WIDTH_PX: f32 = 24.0;
    /// Gap (px/DIPs) between arrows and between the arrow group and the
    /// search box. See [`Self::ARROW_WIDTH_PX`].
    pub const GAP_PX: f32 = 8.0;
    /// Horizontal padding added on top of the estimated search-label
    /// width, in px/DIPs. See [`Self::from_char_width`].
    const SEARCH_H_PAD_PX: f32 = 16.0;
    /// Minimum search-box width (px/DIPs), regardless of how short the
    /// label is. See [`Self::from_char_width`].
    const SEARCH_MIN_WIDTH_PX: f32 = 280.0;

    /// Build a [`CommandCenterMeasure`] from a plain average `char_width`
    /// rather than a live per-glyph text layout — the shape a backend
    /// needs when it has only a `char_width` number on hand (e.g. a
    /// `*_layout` query made outside a frame, with no live font context
    /// to measure against).
    ///
    /// Search-box width is `search_label.len() as f32 * char_width +
    /// 16.0`, floored at `280.0`, or `0.0` when `search_label` is empty
    /// (hides the search box — see [`CommandCenter::search_label`]'s
    /// doc). This is the formula `GtkBackend::command_center_layout`
    /// used to inline directly before quadraui#732 lifted it here, so
    /// `win::command_center` (and any future char-width-only backend)
    /// shares the exact same computation instead of writing a second
    /// copy that only agrees with the first by luck.
    ///
    /// Distinct from `gtk_command_center_layout` / `mac_command_center_layout`
    /// (the *pixel-exact* measurement path used when painting, which lays
    /// the label out against a live Pango/Core Text font and reads back
    /// its real width) — this constructor is deliberately the cheaper
    /// approximation for callers without one.
    pub fn from_char_width(search_label: &str, char_width: f32, height: f32) -> Self {
        let search_box_width = if search_label.is_empty() {
            0.0
        } else {
            (search_label.len() as f32 * char_width + Self::SEARCH_H_PAD_PX)
                .max(Self::SEARCH_MIN_WIDTH_PX)
        };
        CommandCenterMeasure {
            arrow_width: Self::ARROW_WIDTH_PX,
            gap: Self::GAP_PX,
            search_box_width,
            height,
        }
    }
}

impl CommandCenter {
    /// Compute layout. The entire command center is centered within `bounds`.
    pub fn layout(&self, bounds: Rect, measure: CommandCenterMeasure) -> CommandCenterLayout {
        let content_width = measure.arrow_width * 2.0
            + measure.gap
            + if measure.search_box_width > 0.0 {
                measure.gap + measure.search_box_width
            } else {
                0.0
            };

        let center_x = bounds.x + (bounds.width - content_width).max(0.0) / 2.0;
        let y = bounds.y;
        let h = measure.height;

        let back_rect = Rect::new(center_x, y, measure.arrow_width, h);
        let fwd_rect = Rect::new(
            center_x + measure.arrow_width + measure.gap,
            y,
            measure.arrow_width,
            h,
        );

        let search_rect = if measure.search_box_width > 0.0 {
            Some(Rect::new(
                fwd_rect.x + fwd_rect.width + measure.gap,
                y,
                measure.search_box_width,
                h,
            ))
        } else {
            None
        };

        let mut hit_regions = Vec::new();
        hit_regions.push((back_rect, CommandCenterHit::Back));
        hit_regions.push((fwd_rect, CommandCenterHit::Forward));
        if let Some(sb) = search_rect {
            hit_regions.push((sb, CommandCenterHit::SearchBox));
        }
        hit_regions.push((bounds, CommandCenterHit::Bar));

        CommandCenterLayout {
            bounds,
            back_bounds: Some(back_rect),
            forward_bounds: Some(fwd_rect),
            search_bounds: search_rect,
            hit_regions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Shared char-width measure formula (#732) ───────────────────────

    #[test]
    fn from_char_width_empty_label_hides_search_box() {
        let m = CommandCenterMeasure::from_char_width("", 8.0, 24.0);
        assert_eq!(m.search_box_width, 0.0);
        assert_eq!(m.arrow_width, CommandCenterMeasure::ARROW_WIDTH_PX);
        assert_eq!(m.gap, CommandCenterMeasure::GAP_PX);
        assert_eq!(m.height, 24.0);
    }

    #[test]
    fn from_char_width_short_label_floors_at_min_width() {
        // "hi" is nowhere near wide enough to clear the 280px floor at
        // any sane char width.
        let m = CommandCenterMeasure::from_char_width("hi", 8.0, 24.0);
        assert_eq!(m.search_box_width, 280.0);
    }

    #[test]
    fn from_char_width_long_label_exceeds_min_width() {
        // 40 chars * 8.0 + 16.0 = 336.0, comfortably past the 280 floor —
        // pins the exact formula, not just the floor clamp above.
        let label = "x".repeat(40);
        let m = CommandCenterMeasure::from_char_width(&label, 8.0, 24.0);
        assert_eq!(m.search_box_width, 40.0 * 8.0 + 16.0);
    }

    /// Any backend that measures search width from a plain `char_width`
    /// average (rather than a live per-glyph font layout) computes this
    /// exact value for identical inputs — because they all call through
    /// this one constructor rather than keeping a private copy of the
    /// formula. This is what makes
    /// `gtk::backend::GtkBackend::command_center_layout` and
    /// `win::command_center::win_command_center_layout` agree on the
    /// search-box width for the same char width (#732's acceptance bar):
    /// each delegates straight to `from_char_width` (see their own
    /// call-site tests: `gtk::backend::tests::
    /// command_center_layout_delegates_to_shared_char_width_formula` and
    /// `win::command_center::tests::
    /// win_command_center_layout_delegates_to_shared_char_width_formula`),
    /// so proving the formula once here proves it for both.
    #[test]
    fn from_char_width_is_deterministic_for_the_same_inputs() {
        let label = "project-name";
        let a = CommandCenterMeasure::from_char_width(label, 7.5, 20.0);
        let b = CommandCenterMeasure::from_char_width(label, 7.5, 20.0);
        assert_eq!(a, b);
    }

    #[test]
    fn from_char_width_feeds_into_layout_consistently() {
        let cc = CommandCenter {
            id: WidgetId::new("cc"),
            back_enabled: true,
            forward_enabled: true,
            search_label: "project".into(),
        };
        let measure = CommandCenterMeasure::from_char_width(&cc.search_label, 8.0, 24.0);
        let layout = cc.layout(Rect::new(0.0, 0.0, 500.0, 24.0), measure);
        let search = layout
            .search_bounds
            .expect("non-empty label has search bounds");
        assert_eq!(search.width, measure.search_box_width);
    }
}
