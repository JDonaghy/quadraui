//! GTK rasteriser for [`crate::MessageList`].
//!
//! Walks `rows[scroll_top..]` row by row, drawing each row's text via
//! the supplied Pango layout at `(x + row.indent, y + i*line_height)`
//! in the row's `fg`. The panel background fill is the caller's
//! responsibility (typically a single-rect fill done before the
//! message list paints) — this rasteriser only paints text, since
//! repeated per-row bg fills would overdraw any header / separator the
//! caller has already drawn outside the message area.
//!
//! # Styled rows
//!
//! When a row's `spans` vector is **non-empty**, the rasteriser builds a
//! Pango [`AttrList`] and attaches:
//!
//! * `AttrColor::new_foreground` per span that has an explicit `fg`.
//! * `AttrInt::new_weight(Bold)` for bold spans.
//! * `AttrInt::new_style(Italic)` for italic spans.
//! * `AttrInt::new_underline(Single)` for underlined spans.
//! * `AttrFloat::new_scale(row.scale)` over the full row text when
//!   `row.scale` differs from `1.0` (heading rows).
//!
//! This mirrors the rich path in [`crate::gtk::rich_text_popup`] (lines
//! ~118–249).  When `spans` is **empty** the rasteriser falls back to the
//! flat `row.text` + `row.fg` path — output is identical to the
//! pre-styled-row behaviour.

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
///
/// When a row's `spans` is non-empty, per-span Pango attributes are
/// applied (fg, bold, italic, underline) and the row's `scale` is
/// applied via `AttrFloat::new_scale`.  When `spans` is empty the flat
/// `text` + `fg` path is used unchanged.
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
    // Helper: widen a u8 channel to a 16-bit Pango colour component.
    let to_u16 = |c: u8| -> u16 { ((c as u16) << 8) | c as u16 };

    layout.set_attributes(None);
    for (i, row) in list.rows.iter().skip(list.scroll_top).enumerate() {
        let ry = y + i as f64 * line_height;
        if ry + line_height > max_y {
            break;
        }

        if !row.spans.is_empty() {
            // ── Styled path ─────────────────────────────────────────────
            let attrs = pango::AttrList::new();

            // Per-row scale for heading rows (H1=2.0, H2=1.5, H3=1.2).
            if (row.scale - 1.0).abs() > 0.01 {
                let mut a = pango::AttrFloat::new_scale(row.scale as f64);
                a.set_start_index(0);
                a.set_end_index(row.text.len() as u32);
                attrs.insert(a);
            }

            // Per-span fg / bold / italic / underline.
            let mut byte_pos = 0usize;
            for span in &row.spans {
                let start = byte_pos as u32;
                let end = (byte_pos + span.text.len()) as u32;

                if let Some(c) = span.fg {
                    let mut a =
                        pango::AttrColor::new_foreground(to_u16(c.r), to_u16(c.g), to_u16(c.b));
                    a.set_start_index(start);
                    a.set_end_index(end);
                    attrs.insert(a);
                }
                if span.bold {
                    let mut a = pango::AttrInt::new_weight(pango::Weight::Bold);
                    a.set_start_index(start);
                    a.set_end_index(end);
                    attrs.insert(a);
                }
                if span.italic {
                    let mut a = pango::AttrInt::new_style(pango::Style::Italic);
                    a.set_start_index(start);
                    a.set_end_index(end);
                    attrs.insert(a);
                }
                if span.underline {
                    let mut a = pango::AttrInt::new_underline(pango::Underline::Single);
                    a.set_start_index(start);
                    a.set_end_index(end);
                    attrs.insert(a);
                }
                byte_pos += span.text.len();
            }

            let (r, g, b) = cairo_rgb(row.fg);
            cr.set_source_rgb(r, g, b);
            layout.set_text(&row.text);
            layout.set_attributes(Some(&attrs));
            let (_, lh) = layout.pixel_size();
            cr.move_to(x + row.indent as f64, ry + (line_height - lh as f64) / 2.0);
            pcfn::show_layout(cr, layout);
            layout.set_attributes(None);
        } else {
            // ── Flat path (unchanged from before spans were added) ───────
            let (r, g, b) = cairo_rgb(row.fg);
            cr.set_source_rgb(r, g, b);
            layout.set_text(&row.text);
            let (_, lh) = layout.pixel_size();
            cr.move_to(x + row.indent as f64, ry + (line_height - lh as f64) / 2.0);
            pcfn::show_layout(cr, layout);
        }
    }
}
