//! macOS rasteriser for [`crate::ProgressBar`].
//!
//! Paints a horizontal bar with filled portion, optional label, and
//! optional cancel `×` affordance. Indeterminate bars animate a 40-px
//! pulse that slides across the track using `frame_idx`.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::primitives::progress::{ProgressBar, ProgressBarLayout, ProgressBarMeasure};
use crate::theme::Theme;
use crate::types::Color;

/// 28-pt cancel affordance width, matching GTK.
const CANCEL_WIDTH_PX: f32 = 28.0;

/// Compute the macOS pixel-unit layout for a [`ProgressBar`].
pub fn mac_progress_layout(bar: &ProgressBar, x: f64, y: f64, w: f64, h: f64) -> ProgressBarLayout {
    let cancel_w = if bar.cancellable {
        CANCEL_WIDTH_PX
    } else {
        0.0
    };
    bar.layout(
        x as f32,
        y as f32,
        ProgressBarMeasure {
            width: w as f32,
            height: h as f32,
            cancel_width: cancel_w,
        },
    )
}

/// Draw a [`ProgressBar`] onto `ctx`. Returns the layout for host
/// click dispatch.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_progress(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    bar: &ProgressBar,
    theme: &Theme,
) -> ProgressBarLayout {
    let layout = mac_progress_layout(bar, x, y, w, h);

    // Track background.
    fill_rect(ctx, x, y, w, h, theme.surface_bg);

    let fill_color = bar.accent.unwrap_or(theme.accent_bg);
    if let Some(fb) = layout.fill_bounds {
        fill_rect(
            ctx,
            fb.x as f64,
            fb.y as f64,
            fb.width as f64,
            fb.height as f64,
            fill_color,
        );
    } else {
        // Indeterminate pulse — same cadence as GTK.
        let bar_w = if bar.cancellable {
            (w - CANCEL_WIDTH_PX as f64).max(0.0)
        } else {
            w
        };
        if bar_w > 0.0 {
            let pulse_w = 40.0_f64.min(bar_w);
            let pos = (bar.frame_idx as f64 * 4.0) % bar_w;
            fill_rect(ctx, x + pos, y, pulse_w.min(bar_w - pos), h, fill_color);
        }
    }

    // Label.
    if !bar.label.is_empty() {
        draw_text(
            ctx,
            font,
            &bar.label,
            x + 4.0,
            y,
            color_to_cg(theme.foreground),
        );
    }

    // Cancel `×` affordance.
    if let Some(cb) = layout.cancel_bounds {
        let (tw, _) = measure_text(font, "×");
        draw_text(
            ctx,
            font,
            "×",
            cb.x as f64 + (cb.width as f64 - tw) / 2.0,
            cb.y as f64,
            color_to_cg(theme.foreground),
        );
    }

    layout
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
    use crate::primitives::progress::ProgressBarHit;
    use crate::types::WidgetId;
    use crate::Backend;

    const W: u32 = 240;
    const H: u32 = 20;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

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

    #[test]
    fn cancellable_bar_reserves_cancel_bounds() {
        let bar = ProgressBar {
            id: WidgetId::new("pb"),
            label: String::new(),
            value: Some(0.5),
            frame_idx: 0,
            cancellable: true,
            accent: None,
        };
        let (_surface, layout) = paint_via_backend(&bar);
        let cb = layout.cancel_bounds.expect("cancel bounds present");
        assert!((cb.width - CANCEL_WIDTH_PX).abs() < 0.01);
        // Hit-test at the cancel center returns Cancel.
        let cx = cb.x + cb.width * 0.5;
        let cy = cb.y + cb.height * 0.5;
        assert!(matches!(layout.hit_test(cx, cy), ProgressBarHit::Cancel(_),));
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
