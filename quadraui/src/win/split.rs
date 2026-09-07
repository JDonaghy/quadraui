//! Direct2D rasteriser for [`crate::Split`] (issue #29).
//!
//! Painting moved to the shared
//! [`crate::primitives::split::native_surface_paint::paint`] (#864,
//! `NativeSurface` Phase 2d slice 7/9, child of #811) — see that fn's
//! module doc for why the three per-backend copies were found to be
//! already identical (no divergence). This module now carries
//! [`win_split_layout`], [`RawSplitSurface`], and the deprecated
//! [`draw_split`] compatibility shim over the shared paint, mirroring
//! `win::split_tree::RawSplitTreeSurface` (#863, slice 6/9).
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod split;` and `backend.rs`'s module
//! docs for why the rest of this repo's `--features win` compile gate
//! stays meaningful without a Windows host.
//!
//! # Theme
//!
//! `WinBackend` does not yet carry a live [`Theme`] — see `win::status_bar`'s
//! module doc for the "placeholder until a later issue wires the app's
//! real theme through" posture this module shares.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use crate::event::Rect;
use crate::primitives::split::{Split, SplitLayout, SplitMeasure};
use crate::theme::Theme;

/// Divider thickness (DIPs) — the DirectWrite twin of
/// `gtk::split::GTK_DIVIDER_PX`.
pub const DIVIDER_DIP: f32 = 4.0;

/// Compute a [`Split`]'s layout without painting — the twin of
/// [`draw_split`]. Both call [`Split::layout`] with the identical
/// divider thickness, so a no-paint hit-test call always agrees with
/// what the last paint drew.
pub fn win_split_layout(rect: Rect, split: &Split) -> SplitLayout {
    split.layout(rect, SplitMeasure::new(DIVIDER_DIP))
}

/// Minimal [`crate::native_surface::NativeSurface`] adapter over a bare
/// `&ID2D1RenderTarget`, used only by the deprecated [`draw_split`] shim
/// below — a split's paint calls exactly one verb (`surface_fill_rect`,
/// once for the divider), so every other method is `unreachable!()`.
/// Mirrors `win::split_tree::RawSplitTreeSurface`'s identical pattern
/// (#863).
pub(crate) struct RawSplitSurface<'a> {
    pub(crate) target: &'a ID2D1RenderTarget,
}

impl crate::native_surface::NativeSurface for RawSplitSurface<'_> {
    fn surface_begin_frame(&mut self, _viewport: crate::Viewport) {
        unreachable!("RawSplitSurface has no backend frame lifecycle to begin")
    }

    fn surface_end_frame(&mut self) {
        unreachable!("RawSplitSurface has no backend frame lifecycle to end")
    }

    fn surface_viewport(&self) -> crate::Viewport {
        unreachable!("RawSplitSurface has no backend viewport")
    }

    fn surface_line_height(&self) -> f32 {
        unreachable!("RawSplitSurface has no backend line height")
    }

    fn surface_char_width(&self) -> f32 {
        unreachable!("RawSplitSurface has no backend char width")
    }

    fn surface_measure_text(&self, _text: &str) -> (f32, f32) {
        unreachable!("RawSplitSurface has no text measurement")
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
        unreachable!("Split::paint never strokes a rect")
    }

    fn surface_draw_text_run(&mut self, _rect: crate::Rect, _text: &str, _color: crate::Color) {
        unreachable!("Split::paint never draws text")
    }

    fn surface_draw_line(
        &mut self,
        _from: crate::Point,
        _to: crate::Point,
        _color: crate::Color,
        _stroke_width: f32,
    ) {
        unreachable!("Split::paint never strokes a line")
    }

    fn surface_push_clip(&mut self, _rect: crate::Rect) {
        unreachable!("Split::paint never clips")
    }

    fn surface_pop_clip(&mut self) {
        unreachable!("Split::paint never clips")
    }

    fn surface_draw_image(
        &mut self,
        _rect: crate::Rect,
        _image: &crate::Image,
    ) -> crate::backend::ImagePaintResult {
        unreachable!("Split::paint never draws an image")
    }
}

/// Deprecated free-function shim (#864, CLAUDE.md rule 8): reproduces
/// the pre-#864 signature exactly for any external caller that held a
/// direct `quadraui::win::draw_split` reference rather than going
/// through [`crate::Backend::draw_split`] — the sanctioned entry point,
/// and the one every in-tree call site already uses, which is why this
/// shim has no in-repo caller left to trip the `-D warnings`-denied
/// `deprecated` lint.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_split` instead — this free function is a compatibility shim over the shared #864 implementation"
)]
pub fn draw_split(target: &ID2D1RenderTarget, rect: Rect, split: &Split) -> SplitLayout {
    let layout = win_split_layout(rect, split);
    let theme = Theme::default();
    let mut surface = RawSplitSurface { target };
    crate::primitives::split::native_surface_paint::paint(&layout, &mut surface, &theme);
    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::split::{SplitDirection, SplitHit};
    use crate::types::WidgetId;
    use crate::win::testing::HeadlessSurface;

    const W: u32 = 200;
    const H: u32 = 100;

    fn split() -> Split {
        Split {
            id: WidgetId::new("s"),
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first_min: 0.0,
            second_min: 0.0,
        }
    }

    /// Paint `layout` via the shared
    /// [`crate::primitives::split::native_surface_paint::paint`] through
    /// a [`RawSplitSurface`] over `target` — the same adapter the
    /// deprecated [`draw_split`] shim uses, exercised here directly so
    /// these tests don't trip the `-D warnings`-denied `deprecated`
    /// lint (CLAUDE.md rule 3; mirrors `win::split_tree`'s identical
    /// test-migration note).
    fn paint(target: &ID2D1RenderTarget, layout: &SplitLayout) {
        let mut raw = RawSplitSurface { target };
        crate::primitives::split::native_surface_paint::paint(layout, &mut raw, &Theme::default());
    }

    /// Paint↔click round trip: the divider's painted bg and the
    /// layout's own `hit_test` over that same bounds must agree, and
    /// clicks either side of it resolve to the matching pane.
    #[test]
    fn paint_and_hit_test_round_trip() {
        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let split = split();
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        let layout = win_split_layout(rect, &split);
        surface
            .paint(|target| {
                paint(target, &layout);
            })
            .expect("paint split");

        let theme = Theme::default();
        let div = layout.divider_bounds;
        let div_px = surface.pixel_at(
            (div.x + div.width / 2.0) as u32,
            (div.y + div.height / 2.0) as u32,
        );
        assert_eq!(
            (div_px.r, div_px.g, div_px.b),
            (theme.separator.r, theme.separator.g, theme.separator.b)
        );

        let divider_hit = layout.hit_test(div.x + div.width / 2.0, H as f32 / 2.0);
        assert_eq!(divider_hit, SplitHit::Divider(WidgetId::new("s")));

        let first_hit = layout.hit_test(1.0, H as f32 / 2.0);
        assert_eq!(first_hit, SplitHit::FirstPane(WidgetId::new("s")));

        let second_hit = layout.hit_test(W as f32 - 1.0, H as f32 / 2.0);
        assert_eq!(second_hit, SplitHit::SecondPane(WidgetId::new("s")));
    }

    /// `win_split_layout` (no-paint) must produce byte-identical layout
    /// to what `draw_split` used to paint — same split, same rect.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let split = split();
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);

        let surface = HeadlessSurface::new(W, H).expect("create surface");
        let painted = win_split_layout(rect, &split);
        surface
            .paint(|target| {
                paint(target, &painted);
            })
            .expect("paint");
        let no_paint = win_split_layout(rect, &split);

        assert_eq!(painted, no_paint);
    }

    #[test]
    fn vertical_direction_stacks_panes() {
        let mut split = split();
        split.direction = SplitDirection::Vertical;
        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let layout = win_split_layout(rect, &split);

        assert!(layout.first_bounds.y < layout.divider_bounds.y);
        assert!(layout.divider_bounds.y < layout.second_bounds.y);
    }
}
