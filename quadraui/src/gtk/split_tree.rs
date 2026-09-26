//! GTK rasteriser for [`crate::SplitTree`].
//!
//! Painting moved to the shared
//! [`crate::primitives::split_tree::native_surface_paint::paint`] (#863,
//! `NativeSurface` Phase 2d slice 6/9, child of #811) — see that fn's
//! module doc for why the three per-backend copies were found to be
//! already identical (no divergence). This module now carries
//! [`gtk_split_tree_layout`] and the deprecated [`draw_split_tree`]
//! compatibility shim over the shared [`super::surface::CairoSurface`]
//! adapter (#1072 — consolidated from this module's own private
//! `RawSplitTreeSurface`).

use gtk4::cairo::Context;

use crate::event::Rect;
use crate::primitives::split_tree::{SplitTree, SplitTreeLayout, SplitTreeMeasure};
use crate::theme::Theme;

const GTK_DIVIDER_PX: f32 = 4.0;

/// Compute the GTK pixel-unit layout for a [`SplitTree`] without
/// painting.
pub fn gtk_split_tree_layout(tree: &SplitTree, x: f64, y: f64, w: f64, h: f64) -> SplitTreeLayout {
    let bounds = Rect::new(x as f32, y as f32, w as f32, h as f32);
    tree.layout(bounds, SplitTreeMeasure::new(GTK_DIVIDER_PX))
}

/// Deprecated free-function shim (#863, CLAUDE.md rule 8): reproduces
/// the pre-#863 signature exactly for any external caller that held a
/// direct `quadraui::gtk::draw_split_tree` reference rather than going
/// through [`crate::Backend::draw_split_tree`] — the sanctioned entry
/// point, and the one every in-tree call site already uses, which is
/// why this shim has no in-repo caller left to trip the `-D
/// warnings`-denied `deprecated` lint.
#[allow(clippy::too_many_arguments)]
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_split_tree` instead — this free function is a compatibility shim over the shared #863 implementation"
)]
pub fn draw_split_tree(
    cr: &Context,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    tree: &SplitTree,
    theme: &Theme,
) -> SplitTreeLayout {
    let layout = gtk_split_tree_layout(tree, x, y, w, h);
    // Dividers are always opaque `theme.separator` —
    // `translucent_fill: false` matches this module's pre-migration
    // behaviour exactly, unlike `gtk::scrollbar`'s translucent-overlay
    // fill.
    let mut surface = super::surface::CairoSurface {
        cr,
        layout: None,
        translucent_fill: false,
    };
    crate::primitives::split_tree::native_surface_paint::paint(&layout, &mut surface, theme);
    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Point;
    use crate::primitives::split_tree::SplitDirection;
    use crate::types::WidgetId;

    fn wid(s: &str) -> WidgetId {
        WidgetId::new(s)
    }

    /// `split_tree_layout` is documented **ABSOLUTE** (issue #505):
    /// leaf/divider bounds are shifted by the origin, so `(x, y) = (0, 0)`
    /// is exactly the case where a LOCAL/ABSOLUTE mixup is invisible.
    fn round_trip_at(x: f64, y: f64) {
        let tree = SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::leaf(wid("a")),
            SplitTree::leaf(wid("b")),
        );
        let layout = gtk_split_tree_layout(&tree, x, y, 100.0, 40.0);

        assert_eq!(
            layout.leaves[0].1.x as f64, x,
            "first leaf must start at the split's own x"
        );
        assert_eq!(
            layout.leaves[0].1.y as f64, y,
            "first leaf must start at the split's own y"
        );

        let d = &layout.dividers[0];
        let cross_mid = y as f32 + 20.0;
        assert_eq!(
            layout.hit_test_divider(
                Point {
                    x: d.position,
                    y: cross_mid
                },
                1.0
            ),
            Some(d.split_index)
        );
        assert_eq!(
            layout.hit_test_leaf(Point {
                x: x as f32 + 1.0,
                y: cross_mid
            }),
            Some(&wid("a"))
        );
        assert_eq!(
            layout.hit_test_leaf(Point {
                x: d.position + 10.0,
                y: cross_mid
            }),
            Some(&wid("b"))
        );
    }

    #[test]
    fn paint_and_click_round_trip() {
        round_trip_at(0.0, 0.0);
    }

    /// Non-zero-origin regression guard (issue #505 / LESSONS.md).
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        round_trip_at(7.0, 13.0);
    }

    #[test]
    fn layout_matches_split_layout_semantics() {
        let tree = SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::leaf(wid("a")),
            SplitTree::leaf(wid("b")),
        );
        let layout = gtk_split_tree_layout(&tree, 0.0, 0.0, 100.0, 50.0);
        assert_eq!(layout.leaves.len(), 2);
        assert_eq!(layout.dividers.len(), 1);
        assert_eq!(layout.dividers[0].thickness, GTK_DIVIDER_PX);
        // available = 96, 0.5*96 = 48.
        assert!((layout.dividers[0].position - 48.0).abs() < 0.001);
    }

    #[test]
    fn nested_tree_produces_all_dividers() {
        let tree = SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::split(
                SplitDirection::Vertical,
                0.5,
                SplitTree::leaf(wid("a")),
                SplitTree::leaf(wid("c")),
            ),
            SplitTree::leaf(wid("b")),
        );
        let layout = gtk_split_tree_layout(&tree, 0.0, 0.0, 400.0, 300.0);
        assert_eq!(layout.leaves.len(), 3);
        assert_eq!(layout.dividers.len(), 2);
        assert_eq!(layout.dividers[0].split_index, 0);
        assert_eq!(layout.dividers[1].split_index, 1);
    }
}
