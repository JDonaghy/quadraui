//! Direct2D rasteriser for [`crate::primitives::split_tree::SplitTree`]
//! (issue #740).
//!
//! Painting moved to the shared
//! [`crate::primitives::split_tree::native_surface_paint::paint`] (#863,
//! `PaintSurface` Phase 2d slice 6/9, child of #811) — see that fn's
//! module doc for why the three per-backend copies were found to be
//! already identical (no divergence). This module now carries
//! [`win_split_tree_layout`]; the deprecated `draw_split_tree`
//! compatibility shim over the shared [`super::surface::D2dSurface`]
//! adapter was removed in issue #1109 (zero uses in coord-tui's `main`
//! and vimcode's `develop`). No geometry is re-derived:
//! [`win_split_tree_layout`] delegates to
//! [`crate::primitives::layout_metrics::pixel_split_tree_layout`] (issue
//! #1079), the shared divider thickness
//! [`crate::primitives::layout_metrics::pixel::DIVIDER`] `super::split`
//! also uses, so a `SplitTree` and a plain `Split` line up, same as the
//! gtk/macos twins.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod split_tree;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.
//!
//! # Theme
//!
//! `WinBackend::draw_split_tree` passes `&self.current_theme` to the
//! shared [`crate::primitives::split_tree::native_surface_paint::paint`]
//! — the same live theme `Backend::set_theme` writes, the same shape
//! `win::status_bar`'s module doc documents.

#[cfg(test)]
use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use crate::event::Rect;
use crate::primitives::layout_metrics::pixel_split_tree_layout;
use crate::primitives::split_tree::{SplitTree, SplitTreeLayout};
#[cfg(test)]
use crate::theme::Theme;

/// Compute a [`SplitTree`]'s layout without painting — the twin of
/// `crate::Backend::draw_split_tree` (the removed `draw_split_tree`
/// free function shim this backed — see module doc). Both call
/// [`SplitTree::layout`] with the identical divider thickness, so a
/// no-paint hit-test call always agrees with what the last paint drew.
pub fn win_split_tree_layout(rect: Rect, tree: &SplitTree) -> SplitTreeLayout {
    pixel_split_tree_layout(tree, rect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::split_tree::SplitDirection;
    use crate::types::WidgetId;
    use crate::win::testing::HeadlessSurface;

    const W: u32 = 200;
    const H: u32 = 100;

    fn wid(s: &str) -> WidgetId {
        WidgetId::new(s)
    }

    /// Paint `tree` via the shared
    /// [`crate::primitives::split_tree::native_surface_paint::paint`]
    /// through a [`super::super::surface::D2dSurface`] over `target` — the same
    /// adapter the now-removed `draw_split_tree` shim used (issue
    /// #1109), exercised here directly.
    fn paint(target: &ID2D1RenderTarget, layout: &SplitTreeLayout) {
        let mut raw = super::super::surface::D2dSurface {
            target,
            dwrite: None,
        };
        crate::primitives::split_tree::native_surface_paint::paint(
            layout,
            &mut raw,
            &Theme::default(),
        );
    }

    fn two_pane() -> SplitTree {
        SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::leaf(wid("a")),
            SplitTree::leaf(wid("b")),
        )
    }

    fn nested() -> SplitTree {
        SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::split(
                SplitDirection::Vertical,
                0.5,
                SplitTree::leaf(wid("a")),
                SplitTree::leaf(wid("c")),
            ),
            SplitTree::leaf(wid("b")),
        )
    }

    /// Paint↔click round trip: the divider's painted bg and the
    /// layout's own `hit_test_divider`/`hit_test_leaf` over that same
    /// bounds must agree.
    #[test]
    fn paint_and_hit_test_round_trip() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let tree = two_pane();
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        let layout = win_split_tree_layout(rect, &tree);
        surface
            .paint(|target| {
                paint(target, &layout);
            })
            .expect("paint split tree");

        let theme = Theme::default();
        let d = &layout.dividers[0];
        let cx = (d.position + d.thickness / 2.0) as u32;
        let cy = (d.cross_start + d.cross_size / 2.0) as u32;
        let div_px = surface.pixel_at(cx, cy);
        assert_eq!(
            (div_px.r, div_px.g, div_px.b),
            (theme.separator.r, theme.separator.g, theme.separator.b)
        );

        use crate::event::Point;
        assert_eq!(
            layout.hit_test_divider(
                Point {
                    x: d.position + 1.0,
                    y: cy as f32
                },
                1.0
            ),
            Some(d.split_index)
        );
        assert_eq!(
            layout.hit_test_leaf(Point {
                x: 1.0,
                y: H as f32 / 2.0
            }),
            Some(&wid("a"))
        );
        assert_eq!(
            layout.hit_test_leaf(Point {
                x: W as f32 - 1.0,
                y: H as f32 / 2.0
            }),
            Some(&wid("b"))
        );
    }

    /// `win_split_tree_layout` (no-paint) must produce byte-identical
    /// layout to what `draw_split_tree` used to paint — same tree, same
    /// rect.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let tree = two_pane();
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        let painted = win_split_tree_layout(rect, &tree);
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        surface
            .paint(|target| {
                paint(target, &painted);
            })
            .expect("paint");
        let no_paint = win_split_tree_layout(rect, &tree);

        assert_eq!(painted, no_paint);
    }

    #[test]
    fn nested_tree_produces_all_dividers() {
        let tree = nested();
        let rect = Rect::new(0.0, 0.0, 400.0, 300.0);
        let layout = win_split_tree_layout(rect, &tree);

        assert_eq!(layout.leaves.len(), 3);
        assert_eq!(layout.dividers.len(), 2);
        assert_eq!(layout.dividers[0].split_index, 0);
        assert_eq!(layout.dividers[1].split_index, 1);
    }

    #[test]
    fn divider_thickness_matches_the_plain_split_rasteriser() {
        let layout = win_split_tree_layout(Rect::new(0.0, 0.0, 100.0, 50.0), &two_pane());
        assert_eq!(
            layout.dividers[0].thickness,
            crate::primitives::layout_metrics::pixel::DIVIDER
        );
        // available = 96, 0.5 * 96 = 48 — identical to the GTK/macOS twins.
        assert!((layout.dividers[0].position - 48.0).abs() < 0.001);
    }

    /// `split_tree_layout` is documented **ABSOLUTE** (issue #505): leaf
    /// bounds are shifted by the origin.
    #[test]
    fn non_zero_origin_shifts_leaves() {
        let tree = two_pane();
        let layout = win_split_tree_layout(Rect::new(7.0, 13.0, 100.0, 40.0), &tree);
        assert_eq!(layout.leaves[0].1.x, 7.0);
        assert_eq!(layout.leaves[0].1.y, 13.0);
    }
}
