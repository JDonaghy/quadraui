//! GTK rasteriser for [`crate::TextDisplay`].
//!
//! Cairo + Pango equivalent of `quadraui::tui::draw_text_display`.
//! Per-line height is `line_height` (uniform); apps that need wrap or
//! varying heights compute the layout themselves with a custom
//! measurer and call this function repeatedly.

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use super::cairo_rgb;
use crate::primitives::text_display::{TextDisplay, TextDisplayLineMeasure};
use crate::theme::Theme;
use crate::types::Decoration;

/// Draw a [`TextDisplay`] into `(x, y, w, h)` on `cr`.
///
/// Background is filled with [`Theme::background`]. Each visible
/// line's spans are painted with their own `fg` (falling back to the
/// per-line decoration colour or [`Theme::foreground`]). Optional
/// timestamp prefix renders in [`Theme::muted_fg`].
#[allow(clippy::too_many_arguments)]
pub fn draw_text_display(
    cr: &Context,
    layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    display: &TextDisplay,
    theme: &Theme,
    line_height: f64,
) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }

    let bg = cairo_rgb(theme.background);
    let fg = cairo_rgb(theme.foreground);
    let muted = cairo_rgb(theme.muted_fg);
    let error = cairo_rgb(theme.error_fg);
    let warning = cairo_rgb(theme.warning_fg);

    cr.set_source_rgb(bg.0, bg.1, bg.2);
    cr.rectangle(x, y, w, h);
    cr.fill().ok();

    layout.set_attributes(None);

    // Optional title row at the top. Body shrinks by `line_height` when
    // present.
    let (body_y, body_h) = if let Some(ref title) = display.title {
        let mut cursor_x = x;
        for span in &title.spans {
            let span_fg = span.fg.map(cairo_rgb).unwrap_or(fg);
            cr.set_source_rgb(span_fg.0, span_fg.1, span_fg.2);
            layout.set_text(&span.text);
            layout.set_attributes(None);
            cr.move_to(cursor_x, y);
            pcfn::show_layout(cr, layout);
            let (sw, _) = layout.pixel_size();
            cursor_x += sw as f64;
        }
        (y + line_height, (h - line_height).max(0.0))
    } else {
        (y, h)
    };
    if body_h <= 0.0 {
        return;
    }

    let display_layout = display.layout(w as f32, body_h as f32, |_| {
        TextDisplayLineMeasure::new(line_height as f32)
    });

    for vis in &display_layout.visible_lines {
        let line = &display.lines[vis.line_idx];
        let row_y = body_y + vis.bounds.y as f64;
        if row_y + line_height > body_y + body_h {
            break;
        }

        let line_fg = match line.decoration {
            Decoration::Error => error,
            Decoration::Warning => warning,
            Decoration::Muted => muted,
            _ => fg,
        };

        let mut cursor_x = x;

        if let Some(ref ts) = line.timestamp {
            cr.set_source_rgb(muted.0, muted.1, muted.2);
            layout.set_text(ts);
            layout.set_attributes(None);
            cr.move_to(cursor_x, row_y);
            pcfn::show_layout(cr, layout);
            let (tw, _) = layout.pixel_size();
            cursor_x += tw as f64 + 6.0;
        }

        for span in &line.spans {
            let span_fg = span.fg.map(cairo_rgb).unwrap_or(line_fg);
            if let Some(span_bg) = span.bg {
                let sbg = cairo_rgb(span_bg);
                layout.set_text(&span.text);
                layout.set_attributes(None);
                let (sw, _) = layout.pixel_size();
                cr.set_source_rgb(sbg.0, sbg.1, sbg.2);
                cr.rectangle(cursor_x, row_y, sw as f64, line_height);
                cr.fill().ok();
            }
            cr.set_source_rgb(span_fg.0, span_fg.1, span_fg.2);
            layout.set_text(&span.text);
            layout.set_attributes(None);
            cr.move_to(cursor_x, row_y);
            pcfn::show_layout(cr, layout);
            let (sw, _) = layout.pixel_size();
            cursor_x += sw as f64;
        }
    }
}
