//! Direct2D rasteriser for [`crate::primitives::split_tree::SplitTree`]
//! (issue #740).
//!
//! Mirrors `gtk::split_tree`'s / `macos::split_tree`'s structure:
//! [`SplitTree::layout`] (shared across every backend) computes leaf +
//! divider geometry; this module paints only the dividers as filled
//! rectangles — leaf content is the app's responsibility, same contract
//! as every other backend. No geometry is re-derived here: both
//! [`win_split_tree_layout`] and [`draw_split_tree`] call `SplitTree::layout`
//! directly with the identical divider thickness [`DIVIDER_DIP`], which
//! matches [`super::split::DIVIDER_DIP`] so a `SplitTree` and a plain
//! `Split` line up, same as the gtk/macos twins.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod split_tree;` and `backend.rs`'s
//! module docs for why the rest of this repo's `--features win` compile
//! gate stays meaningful without a Windows host.
//!
//! # Theme
//!
//! `WinBackend` does not yet carry a live [`Theme`] — see `win::status_bar`'s
//! module doc for the "placeholder until a later issue wires the app's
//! real theme through" posture this module shares.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::fill_rect;
use crate::event::Rect;
use crate::primitives::split_tree::{SplitDirection, SplitTree, SplitTreeLayout, SplitTreeMeasure};
use crate::theme::Theme;

/// Divider thickness (DIPs) — matches [`super::split::DIVIDER_DIP`], the
/// DirectWrite twin of `gtk::split_tree`'s `GTK_DIVIDER_PX`.
pub const DIVIDER_DIP: f32 = 4.0;

/// Compute a [`SplitTree`]'s layout without painting — the twin of
/// [`draw_split_tree`]. Both call [`SplitTree::layout`] with the
/// identical divider thickness, so a no-paint hit-test call always
/// agrees with what the last paint drew.
pub fn win_split_tree_layout(rect: Rect, tree: &SplitTree) -> SplitTreeLayout {
    tree.layout(rect, SplitTreeMeasure::new(DIVIDER_DIP))
}

/// Draw a [`SplitTree`]'s dividers onto `target`. Returns the layout for
/// host click/drag dispatch. Leaf content is NOT painted.
pub fn draw_split_tree(
    target: &ID2D1RenderTarget,
    rect: Rect,
    tree: &SplitTree,
) -> SplitTreeLayout {
    let layout = win_split_tree_layout(rect, tree);
    let theme = Theme::default();

    for div in &layout.dividers {
        let div_rect = match div.direction {
            SplitDirection::Horizontal => {
                Rect::new(div.position, div.cross_start, div.thickness, div.cross_size)
            }
            SplitDirection::Vertical => {
                Rect::new(div.cross_start, div.position, div.cross_size, div.thickness)
            }
        };
        let _ = fill_rect(target, div_rect, theme.separator);
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::WidgetId;
    use crate::win::testing::HeadlessSurface;

    const W: u32 = 200;
    const H: u32 = 100;

    fn wid(s: &str) -> WidgetId {
        WidgetId::new(s)
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

        let layout = surface
            .paint(|target| {
                draw_split_tree(target, rect, &tree);
            })
            .map(|_| win_split_tree_layout(rect, &tree))
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

        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let painted = surface
            .paint(|target| {
                draw_split_tree(target, rect, &tree);
            })
            .map(|_| win_split_tree_layout(rect, &tree))
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
            super::super::split::DIVIDER_DIP
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
