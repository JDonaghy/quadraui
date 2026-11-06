//! GTK rasteriser for [`crate::Scrollbar`].
//!
//! Painting is implemented in the shared
//! [`crate::primitives::scrollbar::native_surface_paint::paint`] — see that
//! function's documentation for details. Callers reach the paint through
//! [`crate::Backend::draw_scrollbar`], which scales it by
//! [`GtkBackend::scroll_last_interaction`]'s idle-fade opacity — see that
//! method's own doc, and `macos::scrollbar` / `win::scrollbar`'s matching
//! modules for the twin implementations on the other two GUI backends.

#[cfg(test)]
mod tests {
    use crate::event::Rect as QRect;
    use crate::gtk::GtkBackend;
    use crate::primitives::scrollbar::Scrollbar;
    use crate::types::WidgetId;
    use crate::Backend;
    use pangocairo::cairo::{Context, Format, ImageSurface};

    const W: i32 = 80;
    const H: i32 = 200;

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

    fn pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        (data[off + 2], data[off + 1], data[off])
    }

    /// Paint `sb` through the real `GtkBackend::draw_scrollbar` trait
    /// method, after letting `configure` set up the backend's
    /// scroll-fade state. Mirrors `macos::scrollbar::paint_via_backend_raw`.
    fn paint_via_backend_raw(
        sb: &Scrollbar,
        configure: impl FnOnce(&mut GtkBackend),
    ) -> ImageSurface {
        let surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("Context::new");
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.paint().ok();
            let pango_layout = pangocairo::functions::create_layout(&cr);
            let mut backend = GtkBackend::new();
            configure(&mut backend);
            backend.enter_frame_scope(&cr, &pango_layout, |b| {
                b.draw_scrollbar(sb.track, sb);
            });
        }
        surface
    }

    /// Paint `sb` right after a fresh `GtkBackend::new()` — full opacity,
    /// since `new()` seeds the fade clock with its own construction
    /// instant (see that field's doc). These tests probe paint geometry,
    /// not the idle fade curve itself — see the "Scroll-driven idle
    /// fade" tests below for that.
    fn paint_via_backend(sb: &Scrollbar) -> ImageSurface {
        paint_via_backend_raw(sb, |_| {})
    }

    #[test]
    fn track_paints_inside_bounds() {
        let sb = vertical_bar(0.0, 200.0, 50.0);
        let mut surface = paint_via_backend(&sb);
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        // Probe mid-track at a y the thumb does NOT cover (thumb is at
        // top with thumb_len ≈ 50px when scroll=0).
        let (r, _, _) = pixel(&data, stride, 4, 120);
        assert!(
            r < 255,
            "expected track to darken white background, got r={r}"
        );
    }

    #[test]
    fn thumb_paints_brighter_than_track() {
        let sb = vertical_bar(0.0, 200.0, 50.0);
        let mut surface = paint_via_backend(&sb);
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        // Thumb at top: probe near (4, 5) — inside thumb_len.
        let (tr, _, _) = pixel(&data, stride, 4, 5);
        // Track only: probe well below thumb.
        let (tk_r, _, _) = pixel(&data, stride, 4, 150);
        assert!(
            tr < tk_r,
            "thumb should be darker (more opaque thumb_fg) than track-only zone, \
             got thumb_r={tr}, track_r={tk_r}",
        );
    }

    #[test]
    fn dragging_makes_thumb_more_opaque() {
        let mut sb = vertical_bar(0.0, 200.0, 50.0);
        let mut normal_surface = paint_via_backend(&sb);
        let normal_stride = normal_surface.stride() as usize;
        let (normal_r, _, _) = {
            let data = normal_surface.data().expect("surface data");
            pixel(&data, normal_stride, 4, 5)
        };

        sb.dragging = true;
        let mut dragging_surface = paint_via_backend(&sb);
        let dragging_stride = dragging_surface.stride() as usize;
        let (dragging_r, _, _) = {
            let data = dragging_surface.data().expect("surface data");
            pixel(&data, dragging_stride, 4, 5)
        };

        assert!(
            dragging_r < normal_r,
            "dragging thumb should be more opaque, normal_r={normal_r}, dragging_r={dragging_r}",
        );
    }

    #[test]
    fn full_scroll_lands_thumb_at_track_bottom() {
        // scroll = total - visible should align thumb's bottom edge to
        // track end.
        let sb = vertical_bar(150.0, 200.0, 50.0);
        let mut surface = paint_via_backend(&sb);
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        // Probe near the bottom of the track — should be thumb, not
        // just track.
        let (bottom_r, _, _) = pixel(&data, stride, 4, H - 4);
        // Probe mid-track — should now be track-only (thumb has moved
        // off the middle).
        let (mid_r, _, _) = pixel(&data, stride, 4, 80);
        assert!(
            bottom_r < mid_r,
            "full-scroll: thumb should be at bottom, got bottom_r={bottom_r}, mid_r={mid_r}",
        );
    }

    #[test]
    fn horizontal_orientation_uses_width() {
        let track = QRect::new(0.0, 50.0, W as f32, 8.0);
        let sb = Scrollbar::horizontal(WidgetId::new("h"), track, 0.0, 200.0, 40.0, 10.0);
        let mut surface = paint_via_backend(&sb);
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        // Thumb at left: x ∈ [0, thumb_len) should show thumb;
        // x past thumb should show only track.
        let (left_r, _, _) = pixel(&data, stride, 2, 54);
        let (right_r, _, _) = pixel(&data, stride, W - 4, 54);
        assert!(
            left_r < right_r,
            "horizontal: thumb should be at left, got left_r={left_r}, right_r={right_r}",
        );
    }

    // ── Scroll-driven idle fade ─────────────────────────────────────

    #[test]
    fn scrollbar_visible_immediately_after_backend_construction() {
        // `GtkBackend::new` seeds the fade clock with its own
        // construction instant, so a scrollbar painted right away — a
        // live app's first frame, a gallery capture, this test — still
        // shows, rather than starting pre-faded before anything has had
        // a chance to interact with it.
        let sb = vertical_bar(0.0, 200.0, 50.0);
        let mut surface = paint_via_backend_raw(&sb, |_| {});
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        let (r, _, _) = pixel(&data, stride, 4, 5);
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
        let mut surface = paint_via_backend_raw(&sb, |b| {
            let last = b
                .scroll_last_interaction()
                .expect("new() seeds a timestamp");
            b.set_fade_clock_override(Some(last + SCROLLBAR_FADE_HOLD + SCROLLBAR_FADE_OUT));
        });
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        let (r, _, _) = pixel(&data, stride, 4, 5);
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
        let mut surface = paint_via_backend_raw(&sb, |b| {
            b.set_scroll_last_interaction(None);
        });
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        let (r, _, _) = pixel(&data, stride, 4, 5);
        assert_eq!(
            r, 255,
            "scrollbar should be fully faded when explicitly marked never-interacted, got r={r}"
        );
    }

    #[test]
    fn scrollbar_appears_at_full_opacity_right_after_a_scroll_event() {
        let sb = vertical_bar(0.0, 200.0, 50.0);
        let now = std::time::Instant::now();
        let mut surface = paint_via_backend_raw(&sb, |b| {
            // Mirrors what `super::run`'s scroll controller does on a
            // real wheel/trackpad delta: `note_scroll_activity` stamps
            // "now". The clock override pins that "now" to a fixed
            // instant so the probe below is deterministic.
            b.set_fade_clock_override(Some(now));
            b.note_scroll_activity();
        });
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        let (r, _, _) = pixel(&data, stride, 4, 5);
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
        let mut surface = paint_via_backend_raw(&sb, |b| {
            b.set_scroll_last_interaction(Some(last));
            b.set_fade_clock_override(Some(last + SCROLLBAR_FADE_HOLD + SCROLLBAR_FADE_OUT));
        });
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        let (r, _, _) = pixel(&data, stride, 4, 5);
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
        let mut surface = paint_via_backend_raw(&sb, |b| {
            b.set_scroll_last_interaction(Some(last));
            b.set_fade_clock_override(Some(last + SCROLLBAR_FADE_HOLD));
        });
        let stride = surface.stride() as usize;
        let data = surface.data().expect("surface data");
        let (r, _, _) = pixel(&data, stride, 4, 5);
        assert!(
            r < 255,
            "scrollbar should still be visible at the end of the hold window, got r={r}"
        );
    }
}
