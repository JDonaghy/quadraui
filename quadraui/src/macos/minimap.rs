//! macOS layout-only stand-in for the [`Minimap`] primitive (#802).
//!
//! #382 scoped macOS's Core Graphics/Core Text *paint* calls out of
//! scope, and #738 lifted the shared legibility/render-mode thresholds
//! and geometry constants (`ROW_PITCH_PX`, `MinimapSizing::FixedPitch`,
//! …) into [`crate::primitives::minimap`] specifically so a future
//! rasteriser here (GTK/Win-GUI style: fixed-pitch colour blocks) can
//! consume them without re-deriving anything. That paint work is still
//! not done — see `MacBackend::draw_minimap`'s doc comment — but leaving
//! [`Backend::minimap_layout`](crate::Backend::minimap_layout) itself as
//! a `todo!()` was a bug, not a scope decision: it is pure geometry, the
//! exact same [`Minimap::layout_with_sizing`] call GTK and Win-GUI
//! already make, and costs nothing to compute correctly today. Real
//! layout now, real pixels later.
//!
//! [`Minimap`]: crate::primitives::minimap::Minimap

use crate::event::Rect;
use crate::primitives::minimap::{Minimap, MinimapLayout, MinimapSizing, ROW_PITCH_PX};

/// Same grouping factor as GTK — see `gtk::minimap::LINES_PER_ROW`'s doc.
/// A future macOS rasteriser painting real glyphs/blocks would want the
/// same one-buffer-line-per-row layout GTK uses; nothing here depends on
/// it being any particular value, since this module never paints.
pub const LINES_PER_ROW: usize = 1;

/// Compute the macOS pixel-unit layout for a [`Minimap`] without
/// painting — real geometry, shared with GTK/Win-GUI via
/// [`Minimap::layout_with_sizing`], even though `MacBackend::draw_minimap`
/// does not yet rasterise pixels (#382, #802).
pub fn mac_minimap_layout(minimap: &Minimap, rect: Rect) -> MinimapLayout {
    minimap.layout_with_sizing(
        rect,
        LINES_PER_ROW,
        MinimapSizing::FixedPitch(ROW_PITCH_PX as f32),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::minimap::MinimapLine;
    use crate::types::WidgetId;

    fn sample_minimap(line_count: usize) -> Minimap {
        Minimap {
            id: WidgetId::new("minimap"),
            lines: (0..line_count)
                .map(|i| MinimapLine {
                    text: format!("line {i}"),
                    line_idx: i,
                })
                .collect(),
            syntax_spans: Vec::new(),
            visible_row_start: 0,
            visible_row_count: line_count.min(10),
            total_buffer_lines: line_count,
        }
    }

    #[test]
    fn mac_minimap_layout_matches_the_input_bounds() {
        let minimap = sample_minimap(50);
        let rect = Rect::new(10.0, 5.0, 20.0, 200.0);
        let layout = mac_minimap_layout(&minimap, rect);
        assert_eq!(layout.bounds, rect);
        assert!(
            !layout.visible_lines.is_empty(),
            "a non-empty Minimap should produce visible rows even though nothing paints them"
        );
    }

    #[test]
    fn mac_minimap_layout_is_empty_for_an_empty_minimap() {
        let minimap = sample_minimap(0);
        let rect = Rect::new(0.0, 0.0, 20.0, 200.0);
        let layout = mac_minimap_layout(&minimap, rect);
        assert!(layout.visible_lines.is_empty());
    }
}
