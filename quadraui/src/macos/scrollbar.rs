//! macOS rasteriser for [`crate::Scrollbar`].
//!
//! Painting is implemented in the shared
//! [`crate::primitives::scrollbar::native_surface_paint::paint`].
//! This module no longer carries any public rasteriser of its own.
//! Callers reach the paint through [`crate::Backend::draw_scrollbar`].

#[cfg(test)]
use crate::primitives::scrollbar::Scrollbar;

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
        // These tests probe paint geometry, not the idle fade, so pin
        // the scrollbar to full opacity (just-interacted) rather than
        // the default "never scrolled" zero opacity — the fade curve
        // itself is covered by the "Scroll-driven idle fade" tests below.
        backend.set_scroll_last_interaction(Some(std::time::Instant::now()));
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

    // ── Scroll-driven idle fade ─────────────────────────────────────

    /// Builds a fresh backend + white surface, paints `sb` once, and
    /// returns the surface — doesn't touch `set_scroll_last_interaction`
    /// so each fade test controls the backend's interaction/clock state
    /// itself rather than inheriting `paint_via_backend`'s "just
    /// interacted" default.
    fn paint_via_backend_raw(
        sb: &Scrollbar,
        configure: impl FnOnce(&mut MacBackend),
    ) -> BitmapSurface {
        let surface = BitmapSurface::new(W, H);
        surface.fill(1.0, 1.0, 1.0, 1.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(make_font("Menlo", 14.0).expect("Menlo installed"));
        configure(&mut backend);
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_scrollbar(sb.track, sb);
        });
        backend.end_frame();
        surface
    }

    #[test]
    fn scrollbar_visible_immediately_after_backend_construction() {
        // `MacBackend::new` seeds the fade clock with its own
        // construction instant, so a scrollbar painted right away — a
        // live app's first frame, a gallery capture, this test — still
        // shows, rather than starting pre-faded before anything has had
        // a chance to interact with it.
        let sb = vertical_bar(0.0, 200.0, 50.0);
        let surface = paint_via_backend_raw(&sb, |_| {});
        let (r, _, _, _) = surface.pixel(4, 5);
        assert!(
            r < 255,
            "scrollbar should be visible right after construction, got r={r}"
        );
    }

    #[test]
    fn scrollbar_absent_if_never_scrolled_once_idle_interval_elapses() {
        use crate::primitives::scrollbar::{SCROLLBAR_FADE_HOLD, SCROLLBAR_FADE_OUT};

        // No real scroll ever arrived, but enough time has passed since
        // construction that the fade has fully run its course — same
        // terminal state `scrollbar_fades_back_out_after_the_idle_interval`
        // below reaches after a real scroll.
        let sb = vertical_bar(0.0, 200.0, 50.0);
        let surface = paint_via_backend_raw(&sb, |b| {
            let last = b
                .scroll_last_interaction()
                .expect("new() seeds a timestamp");
            b.set_fade_clock_override(Some(last + SCROLLBAR_FADE_HOLD + SCROLLBAR_FADE_OUT));
        });
        let (r, _, _, _) = surface.pixel(4, 5);
        assert_eq!(
            r, 255,
            "scrollbar should be fully faded once idle, even if never scrolled, got r={r}"
        );
    }

    #[test]
    fn scrollbar_absent_when_explicitly_marked_never_interacted() {
        // Test seam: `set_scroll_last_interaction(None)` simulates
        // "never interacted, already fully faded" directly, without
        // needing to fast-forward the clock.
        let sb = vertical_bar(0.0, 200.0, 50.0);
        let surface = paint_via_backend_raw(&sb, |b| {
            b.set_scroll_last_interaction(None);
        });
        let (r, _, _, _) = surface.pixel(4, 5);
        assert_eq!(
            r, 255,
            "scrollbar should be fully faded when explicitly marked never-interacted, got r={r}"
        );
    }

    #[test]
    fn scrollbar_appears_at_full_opacity_right_after_a_scroll_event() {
        let sb = vertical_bar(0.0, 200.0, 50.0);
        let now = std::time::Instant::now();
        let surface = paint_via_backend_raw(&sb, |b| {
            // Mirrors what `super::run`'s `scrollWheel:` handler does on
            // a real trackpad delta: `note_scroll_activity` stamps "now".
            // The clock override pins that "now" to a fixed instant so
            // the probe below is deterministic.
            b.set_fade_clock_override(Some(now));
            b.note_scroll_activity();
        });
        let (r, _, _, _) = surface.pixel(4, 5);
        assert!(
            r < 255,
            "scrollbar should be visible right after a scroll event, got r={r}"
        );
    }

    #[test]
    fn scrollbar_fades_back_out_after_the_idle_interval() {
        use crate::primitives::scrollbar::{SCROLLBAR_FADE_HOLD, SCROLLBAR_FADE_OUT};

        let sb = vertical_bar(0.0, 200.0, 50.0);
        let last = std::time::Instant::now();
        let surface = paint_via_backend_raw(&sb, |b| {
            b.set_scroll_last_interaction(Some(last));
            b.set_fade_clock_override(Some(last + SCROLLBAR_FADE_HOLD + SCROLLBAR_FADE_OUT));
        });
        let (r, _, _, _) = surface.pixel(4, 5);
        assert_eq!(
            r, 255,
            "scrollbar should be fully faded after the idle interval, got r={r}"
        );
    }

    #[test]
    fn scrollbar_still_full_opacity_during_the_hold_window() {
        use crate::primitives::scrollbar::SCROLLBAR_FADE_HOLD;

        let sb = vertical_bar(0.0, 200.0, 50.0);
        let last = std::time::Instant::now();
        let surface = paint_via_backend_raw(&sb, |b| {
            b.set_scroll_last_interaction(Some(last));
            b.set_fade_clock_override(Some(last + SCROLLBAR_FADE_HOLD));
        });
        let (r, _, _, _) = surface.pixel(4, 5);
        assert!(
            r < 255,
            "scrollbar should still be visible at the end of the hold window, got r={r}"
        );
    }
}
