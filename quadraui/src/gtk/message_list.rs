//! GTK rasteriser for [`crate::MessageList`].
//!
//! Walks `rows[scroll_top..]` row by row, drawing each row's text via
//! the supplied Pango layout at `(x + row.indent, y + i*line_height)`
//! in the row's `fg`. The panel background fill is the caller's
//! responsibility (typically a single-rect fill done before the
//! message list paints) — this rasteriser only paints text, since
//! repeated per-row bg fills would overdraw any header / separator the
//! caller has already drawn outside the message area.

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use super::cairo_rgb;
use crate::primitives::message_list::MessageList;

/// Draw a [`MessageList`] into a rectangular region.
///
/// `(x, y)` is the top-left of the message area in pixels; `w` is the
/// width (used to clip text); `max_y` is the bottom edge — rows whose
/// baseline would fall at or past `max_y` are skipped (the caller's
/// input row sits below `max_y`). `line_height` is the per-row pixel
/// height.
#[allow(clippy::too_many_arguments)]
pub fn draw_message_list(
    cr: &Context,
    layout: &pango::Layout,
    list: &MessageList,
    x: f64,
    y: f64,
    w: f64,
    max_y: f64,
    line_height: f64,
) {
    if w <= 0.0 || line_height <= 0.0 {
        return;
    }
    layout.set_attributes(None);
    for (i, row) in list.rows.iter().skip(list.scroll_top).enumerate() {
        let ry = y + i as f64 * line_height;
        if ry + line_height > max_y {
            break;
        }
        let (r, g, b) = cairo_rgb(row.fg);
        cr.set_source_rgb(r, g, b);
        layout.set_text(&row.text);
        let (_, lh) = layout.pixel_size();
        cr.move_to(x + row.indent as f64, ry + (line_height - lh as f64) / 2.0);
        pcfn::show_layout(cr, layout);
    }
}
