//! GTK rasteriser for [`crate::ProgressBar`].
//!
//! Paints a horizontal bar with filled portion, optional label, and
//! optional cancel affordance.

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use super::set_source;
use crate::primitives::progress::{ProgressBar, ProgressBarLayout, ProgressBarMeasure};
use crate::theme::Theme;

const GTK_CANCEL_WIDTH_PX: f32 = 28.0;

/// Compute the GTK pixel-unit layout for a [`ProgressBar`] without painting.
pub fn gtk_progress_layout(bar: &ProgressBar, x: f64, y: f64, w: f64, h: f64) -> ProgressBarLayout {
    let cancel_w = if bar.cancellable {
        GTK_CANCEL_WIDTH_PX
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

/// Draw a [`ProgressBar`] onto `cr`. Returns the layout for host
/// click dispatch.
#[allow(clippy::too_many_arguments)]
pub fn draw_progress(
    cr: &Context,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    bar: &ProgressBar,
    theme: &Theme,
) -> ProgressBarLayout {
    let layout = gtk_progress_layout(bar, x, y, w, h);

    // Track background.
    set_source(cr, theme.surface_bg);
    cr.rectangle(x, y, w, h);
    cr.fill().ok();

    // Fill.
    if let Some(fb) = layout.fill_bounds {
        let fill_color = bar.accent.unwrap_or(theme.accent_bg);
        set_source(cr, fill_color);
        cr.rectangle(fb.x as f64, fb.y as f64, fb.width as f64, fb.height as f64);
        cr.fill().ok();
    } else {
        // Indeterminate pulse.
        let bar_w = if bar.cancellable {
            (w - GTK_CANCEL_WIDTH_PX as f64).max(0.0)
        } else {
            w
        };
        if bar_w > 0.0 {
            let pulse_w = 40.0_f64.min(bar_w);
            let pos = (bar.frame_idx as f64 * 4.0) % bar_w;
            let fill_color = bar.accent.unwrap_or(theme.accent_bg);
            set_source(cr, fill_color);
            cr.rectangle(x + pos, y, pulse_w.min(bar_w - pos), h);
            cr.fill().ok();
        }
    }

    // Label.
    if !bar.label.is_empty() {
        pango_layout.set_text(&bar.label);
        pango_layout.set_attributes(None);
        set_source(cr, theme.foreground);
        cr.move_to(x + 4.0, y);
        pcfn::show_layout(cr, pango_layout);
    }

    // Cancel affordance.
    if let Some(cb) = layout.cancel_bounds {
        pango_layout.set_text("×");
        set_source(cr, theme.foreground);
        let text_w = pango_layout.pixel_size().0 as f64;
        cr.move_to(cb.x as f64 + (cb.width as f64 - text_w) / 2.0, cb.y as f64);
        pcfn::show_layout(cr, pango_layout);
    }

    layout
}
