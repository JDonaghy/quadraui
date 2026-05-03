//! GTK rasteriser for [`crate::Spinner`].
//!
//! Paints a Unicode animation glyph + label using Pango text layout.
//! Same braille frame table as TUI for visual consistency across
//! backends.

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use super::set_source;
use crate::primitives::spinner::{Spinner, SpinnerLayout, SpinnerMeasure};
use crate::theme::Theme;

const FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Compute the GTK pixel-unit layout for a [`Spinner`] without painting.
pub fn gtk_spinner_layout(
    spinner: &Spinner,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
) -> SpinnerLayout {
    let glyph = FRAMES[spinner.frame_idx % FRAMES.len()];
    let text = if spinner.label.is_empty() {
        glyph.to_string()
    } else {
        format!("{glyph} {}", spinner.label)
    };
    pango_layout.set_text(&text);
    pango_layout.set_attributes(None);
    let (pw, ph) = pango_layout.pixel_size();
    spinner.layout(
        x as f32,
        y as f32,
        SpinnerMeasure::new(pw.max(0) as f32, ph.max(0) as f32),
    )
}

/// Draw a [`Spinner`] onto `cr`. Returns the layout for host
/// hit-testing.
pub fn draw_spinner(
    cr: &Context,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
    spinner: &Spinner,
    theme: &Theme,
) -> SpinnerLayout {
    let layout = gtk_spinner_layout(spinner, pango_layout, x, y);

    let glyph = FRAMES[spinner.frame_idx % FRAMES.len()];
    let text = if spinner.label.is_empty() {
        glyph.to_string()
    } else {
        format!("{glyph} {}", spinner.label)
    };
    pango_layout.set_text(&text);
    pango_layout.set_attributes(None);

    let fg = spinner.accent.unwrap_or(theme.foreground);
    set_source(cr, fg);
    cr.move_to(x, y);
    pcfn::show_layout(cr, pango_layout);

    layout
}
