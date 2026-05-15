//! macOS rasteriser for [`crate::Scrollbar`].
//!
//! Overlay-style scrollbar matching [`crate::gtk::scrollbar::draw_scrollbar`]:
//! a thin alpha-blended track with a brighter thumb on top. Hover / drag
//! state bumps both alphas so the bar pops while the user is interacting
//! with it.
//!
//! Both axes share this implementation — the `axis` field of the
//! primitive determines whether `thumb_start` / `thumb_len` are applied
//! vertically or horizontally.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;

use crate::primitives::scrollbar::{ScrollAxis, Scrollbar};
use crate::theme::Theme;
use crate::types::Color;

/// Paint `scrollbar` onto `ctx`.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
pub unsafe fn draw_scrollbar(ctx: CGContextRef, scrollbar: &Scrollbar, theme: &Theme) {
    let track = scrollbar.track;
    if track.width <= 0.0 || track.height <= 0.0 {
        return;
    }

    let track_alpha = if scrollbar.hovered || scrollbar.dragging {
        0.35
    } else {
        0.20
    };
    let thumb_alpha = if scrollbar.dragging {
        0.85
    } else if scrollbar.hovered {
        0.70
    } else {
        0.50
    };

    fill_rect(
        ctx,
        track.x as f64,
        track.y as f64,
        track.width as f64,
        track.height as f64,
        with_alpha(theme.scrollbar_track, track_alpha),
    );

    let (tx, ty, tw, th) = match scrollbar.axis {
        ScrollAxis::Vertical => (
            track.x as f64,
            track.y as f64 + scrollbar.thumb_start as f64,
            track.width as f64,
            scrollbar.thumb_len as f64,
        ),
        ScrollAxis::Horizontal => (
            track.x as f64 + scrollbar.thumb_start as f64,
            track.y as f64,
            scrollbar.thumb_len as f64,
            track.height as f64,
        ),
    };
    fill_rect(
        ctx,
        tx,
        ty,
        tw,
        th,
        with_alpha(theme.scrollbar_thumb, thumb_alpha),
    );
}

fn with_alpha(c: Color, alpha: f64) -> Color {
    Color {
        r: c.r,
        g: c.g,
        b: c.b,
        a: (255.0 * alpha).round().clamp(0.0, 255.0) as u8,
    }
}

fn color_to_cg(c: Color) -> (f64, f64, f64, f64) {
    (
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    )
}

unsafe fn fill_rect(ctx: CGContextRef, x: f64, y: f64, w: f64, h: f64, c: Color) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r, g, b, a);
    use core_graphics::geometry::{CGPoint, CGSize};
    CGContextFillRect(ctx, CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h)));
}

extern "C" {
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
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
