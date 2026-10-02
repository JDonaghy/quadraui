//! macOS rasteriser for [`crate::ProgressBar`].
//!
//! Painting moved to the shared
//! [`crate::primitives::progress::native_surface_paint::paint`] (#1085,
//! `NativeSurface` Phase 4 slice 8/8) — see that fn's doc for the one
//! named divergence (Windows previously ignored the host's theme
//! entirely) found while unifying `gtk::progress::draw_progress`,
//! `macos::progress::draw_progress` and `win::progress::draw_progress`
//! into one implementation. This module now only carries
//! [`mac_progress_layout`] (still real, backend-specific pure geometry —
//! no painting involved); the deprecated `draw_progress` compatibility
//! shim over the shared [`super::surface::CgSurface`] adapter was
//! removed in issue #1109 (zero uses in coord-tui's `main` and
//! vimcode's `develop`).

#[cfg(test)]
use core_text::font::CTFont;

use crate::primitives::layout_metrics::pixel_progress_layout;
use crate::primitives::progress::{ProgressBar, ProgressBarLayout};
#[cfg(test)]
use crate::theme::Theme;

/// Compute the macOS pixel-unit layout for a [`ProgressBar`]. Shares its
/// cancel-affordance width with `gtk_progress_layout` /
/// `win_progress_layout` via [`pixel_progress_layout`] (issue #1079).
pub fn mac_progress_layout(bar: &ProgressBar, x: f64, y: f64, w: f64, h: f64) -> ProgressBarLayout {
    pixel_progress_layout(bar, x as f32, y as f32, w as f32, h as f32)
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::progress::ProgressBarHit;
    use crate::types::WidgetId;
    use crate::Backend;

    const W: u32 = 240;
    const H: u32 = 20;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    /// Paint via the real `Backend::draw_progress` path — which now
    /// routes through the shared `native_surface_paint::paint` — avoids
    /// tripping the `-D warnings`-denied `deprecated` lint (CLAUDE.md
    /// rule 3) by not calling the deprecated free-function shim above.
    fn paint_via_backend(bar: &ProgressBar) -> (BitmapSurface, ProgressBarLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_progress(QRect::new(0.0, 0.0, W as f32, H as f32), bar);
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    #[test]
    fn determinate_fill_proportional_to_value() {
        let bar = ProgressBar {
            id: WidgetId::new("pb"),
            label: String::new(),
            value: Some(0.5),
            frame_idx: 0,
            cancellable: false,
            accent: None,
        };
        let (surface, layout) = paint_via_backend(&bar);
        let fb = layout.fill_bounds.expect("fill bounds present");
        assert!(
            (fb.width - (W as f32) * 0.5).abs() < 1.0,
            "fill width should be ~half: got {}",
            fb.width,
        );
        // Sample inside the fill: should be theme.accent_bg.
        let theme = Theme::default();
        let (r, g, b, _) = surface.pixel(10, H / 2);
        assert_eq!(
            (r, g, b),
            (theme.accent_bg.r, theme.accent_bg.g, theme.accent_bg.b),
        );
    }

    #[test]
    fn track_background_is_surface_bg() {
        // value=0 -> fill_bounds covers 0 width; full track shows
        // surface_bg.
        let bar = ProgressBar {
            id: WidgetId::new("pb"),
            label: String::new(),
            value: Some(0.0),
            frame_idx: 0,
            cancellable: false,
            accent: None,
        };
        let (surface, _layout) = paint_via_backend(&bar);
        let theme = Theme::default();
        let (r, g, b, _) = surface.pixel(W - 4, H / 2);
        assert_eq!(
            (r, g, b),
            (theme.surface_bg.r, theme.surface_bg.g, theme.surface_bg.b),
        );
    }

    /// Shared body for the cancel-bounds↔hit_test round trip, run at
    /// both the origin and a non-zero origin (quadraui#494 /
    /// LESSONS.md "Layout helpers must return coords in the same
    /// frame across backends"). `mac_progress_layout` bakes `x`/`y`
    /// straight into the returned `cancel_bounds` (absolute frame,
    /// matching the GTK/TUI twins) — call it directly (pure fn, no
    /// paint needed) and prove a click at the resulting absolute
    /// cancel-bounds position still resolves through `hit_test`.
    fn cancellable_bar_reserves_cancel_bounds_at(origin_x: f64, origin_y: f64) {
        let bar = ProgressBar {
            id: WidgetId::new("pb"),
            label: String::new(),
            value: Some(0.5),
            frame_idx: 0,
            cancellable: true,
            accent: None,
        };
        let layout = mac_progress_layout(&bar, origin_x, origin_y, W as f64, H as f64);
        let cb = layout.cancel_bounds.expect("cancel bounds present");
        assert!(
            (cb.width - crate::primitives::layout_metrics::pixel::PROGRESS_CANCEL_WIDTH).abs()
                < 0.01
        );
        // Hit-test at the cancel center returns Cancel.
        let cx = cb.x + cb.width * 0.5;
        let cy = cb.y + cb.height * 0.5;
        assert!(matches!(layout.hit_test(cx, cy), ProgressBarHit::Cancel(_),));
    }

    #[test]
    fn cancellable_bar_reserves_cancel_bounds() {
        cancellable_bar_reserves_cancel_bounds_at(0.0, 0.0);
    }

    /// Non-zero-origin regression guard (quadraui#494).
    #[test]
    fn cancellable_bar_reserves_cancel_bounds_at_nonzero_origin() {
        cancellable_bar_reserves_cancel_bounds_at(7.0, 13.0);
    }

    #[test]
    fn indeterminate_no_fill_bounds_still_paints_pulse() {
        // Indeterminate: value=None, frame_idx selects pulse position.
        // We can't easily assert on the pulse's exact location, but
        // the bar's leading region should show fill colour (pulse
        // starts at pos=0 when frame_idx=0).
        let bar = ProgressBar {
            id: WidgetId::new("pb"),
            label: String::new(),
            value: None,
            frame_idx: 0,
            cancellable: false,
            accent: None,
        };
        let (surface, layout) = paint_via_backend(&bar);
        assert!(layout.fill_bounds.is_none());
        let theme = Theme::default();
        // x=4 should be inside the 40px pulse at pos=0.
        let (r, g, b, _) = surface.pixel(4, H / 2);
        assert_eq!(
            (r, g, b),
            (theme.accent_bg.r, theme.accent_bg.g, theme.accent_bg.b),
        );
    }
}
