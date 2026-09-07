//! Direct2D rasteriser for [`crate::primitives::split_tree::SplitTree`]
//! (issue #740).
//!
//! Painting moved to the shared
//! [`crate::primitives::split_tree::native_surface_paint::paint`] (#863,
//! `NativeSurface` Phase 2d slice 6/9, child of #811) — see that fn's
//! module doc for why the three per-backend copies were found to be
//! already identical (no divergence). This module now carries
//! [`win_split_tree_layout`], [`RawSplitTreeSurface`], and the
//! deprecated [`draw_split_tree`] compatibility shim over the shared
//! paint, mirroring `win::scrollbar::RawScrollbarSurface` (#811 slice
//! 1/9). No geometry is re-derived: [`win_split_tree_layout`] calls
//! `SplitTree::layout` directly with the identical divider thickness
//! [`DIVIDER_DIP`], which matches [`super::split::DIVIDER_DIP`] so a
//! `SplitTree` and a plain `Split` line up, same as the gtk/macos twins.
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

use crate::event::Rect;
use crate::primitives::split_tree::{SplitTree, SplitTreeLayout, SplitTreeMeasure};
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

/// Minimal [`crate::native_surface::NativeSurface`] adapter over a bare
/// `&ID2D1RenderTarget`, used only by the deprecated [`draw_split_tree`]
/// shim below and by this module's own tests — a split tree's paint
/// calls exactly one verb (`surface_fill_rect`, once per divider), so
/// every other method is `unreachable!()`. Mirrors
/// `win::scrollbar::RawScrollbarSurface`'s identical pattern (#811 slice
/// 1/9).
pub(crate) struct RawSplitTreeSurface<'a> {
    pub(crate) target: &'a ID2D1RenderTarget,
}

impl crate::native_surface::NativeSurface for RawSplitTreeSurface<'_> {
    fn surface_begin_frame(&mut self, _viewport: crate::Viewport) {
        unreachable!("RawSplitTreeSurface has no backend frame lifecycle to begin")
    }

    fn surface_end_frame(&mut self) {
        unreachable!("RawSplitTreeSurface has no backend frame lifecycle to end")
    }

    fn surface_viewport(&self) -> crate::Viewport {
        unreachable!("RawSplitTreeSurface has no backend viewport")
    }

    fn surface_line_height(&self) -> f32 {
        unreachable!("RawSplitTreeSurface has no backend line height")
    }

    fn surface_char_width(&self) -> f32 {
        unreachable!("RawSplitTreeSurface has no backend char width")
    }

    fn surface_measure_text(&self, _text: &str) -> (f32, f32) {
        unreachable!("RawSplitTreeSurface has no text measurement")
    }

    fn surface_fill_rect(&mut self, rect: crate::Rect, color: crate::Color) {
        let _ = super::text::fill_rect(self.target, rect, color);
    }

    fn surface_stroke_rect(
        &mut self,
        _rect: crate::Rect,
        _color: crate::Color,
        _stroke_width: f32,
    ) {
        unreachable!("SplitTree::paint never strokes a rect")
    }

    fn surface_draw_text_run(&mut self, _rect: crate::Rect, _text: &str, _color: crate::Color) {
        unreachable!("SplitTree::paint never draws text")
    }

    fn surface_draw_line(
        &mut self,
        _from: crate::Point,
        _to: crate::Point,
        _color: crate::Color,
        _stroke_width: f32,
    ) {
        unreachable!("SplitTree::paint never strokes a line")
    }

    fn surface_push_clip(&mut self, _rect: crate::Rect) {
        unreachable!("SplitTree::paint never clips")
    }

    fn surface_pop_clip(&mut self) {
        unreachable!("SplitTree::paint never clips")
    }

    fn surface_draw_image(
        &mut self,
        _rect: crate::Rect,
        _image: &crate::Image,
    ) -> crate::backend::ImagePaintResult {
        unreachable!("SplitTree::paint never draws an image")
    }
}

/// Deprecated free-function shim (#863, CLAUDE.md rule 8): reproduces
/// the pre-#863 signature exactly for any external caller that held a
/// direct `quadraui::win::draw_split_tree` reference rather than going
/// through [`crate::Backend::draw_split_tree`] — the sanctioned entry
/// point, and the one every in-tree call site already uses, which is
/// why this shim has no in-repo caller left to trip the
/// `-D warnings`-denied `deprecated` lint.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_split_tree` instead — this free function is a compatibility shim over the shared #863 implementation"
)]
pub fn draw_split_tree(
    target: &ID2D1RenderTarget,
    rect: Rect,
    tree: &SplitTree,
) -> SplitTreeLayout {
    let layout = win_split_tree_layout(rect, tree);
    let theme = Theme::default();
    let mut surface = RawSplitTreeSurface { target };
    crate::primitives::split_tree::native_surface_paint::paint(&layout, &mut surface, &theme);
    layout
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
    /// through a [`RawSplitTreeSurface`] over `target` — the same
    /// adapter the deprecated [`draw_split_tree`] shim uses, exercised
    /// here directly so these tests don't trip the `-D
    /// warnings`-denied `deprecated` lint (CLAUDE.md rule 3; mirrors
    /// `win::scrollbar`'s identical test-migration note).
    fn paint(target: &ID2D1RenderTarget, layout: &SplitTreeLayout) {
        let mut raw = RawSplitTreeSurface { target };
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
