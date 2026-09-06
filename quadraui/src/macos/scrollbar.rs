//! macOS rasteriser for [`crate::Scrollbar`].
//!
//! Painting moved to the shared
//! [`crate::primitives::scrollbar::native_surface_paint::paint`] (#811,
//! `NativeSurface` Phase 2d) — see that fn's doc for the one named
//! divergence (quadraui#791) re-verified (already fixed) while unifying
//! `gtk::draw_scrollbar`, `macos::scrollbar::draw_scrollbar` and
//! `win::scrollbar::draw_scrollbar` into one implementation. This module
//! now only carries [`RawScrollbarSurface`] and the deprecated
//! [`draw_scrollbar`] compatibility shim over it, mirroring
//! `macos::form::RawFormSurface` (#808).

use core_graphics::sys::CGContextRef;

use crate::native_surface::NativeSurface;
use crate::primitives::scrollbar::Scrollbar;
use crate::theme::Theme;

/// Minimal [`NativeSurface`] adapter over a bare `CGContextRef`, used
/// only by the deprecated [`draw_scrollbar`] shim below — a scrollbar's
/// paint calls exactly one verb (`surface_fill_rect`), so every other
/// method is `unreachable!()`. Mirrors `macos::form::RawFormSurface`'s
/// identical pattern (#808).
pub(crate) struct RawScrollbarSurface {
    pub(crate) ctx: CGContextRef,
}

impl NativeSurface for RawScrollbarSurface {
    fn surface_begin_frame(&mut self, _viewport: crate::Viewport) {
        unreachable!("RawScrollbarSurface has no backend frame lifecycle to begin")
    }

    fn surface_end_frame(&mut self) {
        unreachable!("RawScrollbarSurface has no backend frame lifecycle to end")
    }

    fn surface_viewport(&self) -> crate::Viewport {
        unreachable!("RawScrollbarSurface has no backend viewport")
    }

    fn surface_line_height(&self) -> f32 {
        unreachable!("RawScrollbarSurface has no backend line height")
    }

    fn surface_char_width(&self) -> f32 {
        unreachable!("RawScrollbarSurface has no backend char width")
    }

    fn surface_measure_text(&self, _text: &str) -> (f32, f32) {
        unreachable!("RawScrollbarSurface has no text measurement")
    }

    fn surface_fill_rect(&mut self, rect: crate::Rect, color: crate::Color) {
        // SAFETY: `ctx` is a valid `CGContextRef` for the caller's paint
        // pass — see this struct's construction site. `ns_fill_rect`
        // already honours `color.a` with a real alpha blend (unlike the
        // GTK `NativeSurface::surface_fill_rect` bug this same issue
        // fixed — see the module doc).
        unsafe { super::backend::ns_fill_rect(self.ctx, rect, color) };
    }

    fn surface_stroke_rect(
        &mut self,
        _rect: crate::Rect,
        _color: crate::Color,
        _stroke_width: f32,
    ) {
        unreachable!("Scrollbar::paint never strokes a rect")
    }

    fn surface_draw_text_run(&mut self, _rect: crate::Rect, _text: &str, _color: crate::Color) {
        unreachable!("Scrollbar::paint never draws text")
    }

    fn surface_draw_line(
        &mut self,
        _from: crate::Point,
        _to: crate::Point,
        _color: crate::Color,
        _stroke_width: f32,
    ) {
        unreachable!("Scrollbar::paint never strokes a line")
    }

    fn surface_push_clip(&mut self, _rect: crate::Rect) {
        unreachable!("Scrollbar::paint never clips")
    }

    fn surface_pop_clip(&mut self) {
        unreachable!("Scrollbar::paint never clips")
    }

    fn surface_draw_image(
        &mut self,
        _rect: crate::Rect,
        _image: &crate::Image,
    ) -> crate::backend::ImagePaintResult {
        unreachable!("Scrollbar::paint never draws an image")
    }
}

/// Deprecated free-function shim (#811, CLAUDE.md rule 8): reproduces
/// the pre-#811 signature exactly for any external caller that held a
/// direct `quadraui::macos::draw_scrollbar` reference rather than going
/// through [`crate::Backend::draw_scrollbar`] — the sanctioned entry
/// point, and the one every in-tree call site already uses, which is
/// why this shim has no in-repo caller left to trip the
/// `-D warnings`-denied `deprecated` lint.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
#[deprecated(
    since = "0.0.1",
    note = "call `Backend::draw_scrollbar` instead — this free function is a compatibility shim over the shared #811 implementation"
)]
pub unsafe fn draw_scrollbar(ctx: CGContextRef, scrollbar: &Scrollbar, theme: &Theme) {
    let mut surface = RawScrollbarSurface { ctx };
    crate::primitives::scrollbar::native_surface_paint::paint(scrollbar, &mut surface, theme);
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::types::WidgetId;
    use crate::Backend;

    const W: u32 = 80;
    const H: u32 = 200;

    fn paint_via_backend(scrollbar: &Scrollbar) -> BitmapSurface {
        let surface = BitmapSurface::new(W, H);
        // Fill with opaque white so alpha blending of the track lands on
        // a known background — makes probes deterministic.
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(make_font("Menlo", 14.0).expect("Menlo installed"));
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_scrollbar(scrollbar.track, scrollbar);
        });
        backend.end_frame();
        surface
    }

    fn vertical_bar(scroll: f32, total: f32, visible: f32) -> Scrollbar {
        Scrollbar::vertical(
            WidgetId::new("sb"),
            QRect::new(0.0, 0.0, 8.0, H as f32),
            scroll,
            total,
            visible,
            20.0,
        )
    }

    #[test]
    fn track_paints_inside_bounds() {
        let sb = vertical_bar(0.0, 200.0, 50.0);
        let surface = paint_via_backend(&sb);
        // Probe mid-track at a y the thumb does NOT cover (thumb is at
        // top with thumb_len ≈ 50px when scroll=0).
        let (r, _g, _b, _) = surface.pixel(4, 120);
        // White (255) blended with scrollbar_track at 0.2 alpha:
        // result.r < 255 (track has a non-white component).
        assert!(
            r < 255,
            "expected track to darken white background, got r={}",
            r,
        );
    }

    #[test]
    fn thumb_paints_brighter_than_track() {
        let sb = vertical_bar(0.0, 200.0, 50.0);
        let surface = paint_via_backend(&sb);
        // Thumb at top: probe near (4, 5) — inside thumb_len.
        let (tr, _, _, _) = surface.pixel(4, 5);
        // Track only: probe well below thumb.
        let (tk_r, _, _, _) = surface.pixel(4, 150);
        assert!(
            tr < tk_r,
            "thumb should be darker (more opaque thumb_fg) than track-only zone, got thumb_r={}, track_r={}",
            tr,
            tk_r,
        );
    }

    #[test]
    fn dragging_makes_thumb_more_opaque() {
        let mut sb = vertical_bar(0.0, 200.0, 50.0);
        let normal = paint_via_backend(&sb).pixel(4, 5);
        sb.dragging = true;
        let dragging = paint_via_backend(&sb).pixel(4, 5);
        // Higher alpha → r drops further from white toward thumb fg.
        assert!(
            dragging.0 < normal.0,
            "dragging thumb should be more opaque, normal_r={}, dragging_r={}",
            normal.0,
            dragging.0,
        );
    }

    #[test]
    fn full_scroll_lands_thumb_at_track_bottom() {
        // scroll = total - visible should align thumb's bottom edge to
        // track end.
        let sb = vertical_bar(150.0, 200.0, 50.0);
        let surface = paint_via_backend(&sb);
        // Probe near the bottom of the track — should be thumb, not
        // just track.
        let (bottom_r, _, _, _) = surface.pixel(4, H - 4);
        // Probe mid-track — should now be track-only (thumb has moved
        // off the middle).
        let (mid_r, _, _, _) = surface.pixel(4, 80);
        assert!(
            bottom_r < mid_r,
            "full-scroll: thumb should be at bottom, got bottom_r={}, mid_r={}",
            bottom_r,
            mid_r,
        );
    }

    #[test]
    fn horizontal_orientation_uses_width() {
        let track = QRect::new(0.0, 50.0, W as f32, 8.0);
        let sb = Scrollbar::horizontal(WidgetId::new("h"), track, 0.0, 200.0, 40.0, 10.0);
        let surface = paint_via_backend(&sb);
        // Thumb at left: x ∈ [0, thumb_len) should show thumb;
        // x past thumb should show only track.
        let (left_r, _, _, _) = surface.pixel(2, 54);
        let (right_r, _, _, _) = surface.pixel(W - 4, 54);
        assert!(
            left_r < right_r,
            "horizontal: thumb should be at left, got left_r={}, right_r={}",
            left_r,
            right_r,
        );
    }
}
