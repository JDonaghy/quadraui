//! GTK rasteriser for [`crate::DropOverlay`].
//!
//! Highlight: semi-transparent filled rect in `theme.accent_fg`.
//! Insertion bar: solid 2px rect in `theme.accent_fg`.

use gtk4::cairo::Context;

use crate::primitives::drop_zone::DropOverlay;
use crate::theme::Theme;

pub fn draw_drop_overlay(cr: &Context, overlay: &DropOverlay, theme: &Theme) {
    let (ar, ag, ab) = (
        theme.accent_fg.r as f64 / 255.0,
        theme.accent_fg.g as f64 / 255.0,
        theme.accent_fg.b as f64 / 255.0,
    );

    if let Some(h) = overlay.highlight {
        cr.set_source_rgba(ar, ag, ab, 0.15);
        cr.rectangle(h.x as f64, h.y as f64, h.width as f64, h.height as f64);
        cr.fill().ok();
    }

    if let Some(bar) = overlay.insertion_bar {
        cr.set_source_rgb(ar, ag, ab);
        cr.rectangle(
            bar.x as f64,
            bar.y as f64,
            (bar.width as f64).max(2.0),
            bar.height as f64,
        );
        cr.fill().ok();
    }
}
