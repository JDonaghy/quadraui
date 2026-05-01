//! GTK rasteriser for [`crate::RichTextPopup`].
//!
//! Cairo + Pango equivalent of `quadraui::tui::draw_rich_text_popup`.
//! Returns per-link hit regions in `(x, y, w, h, url)` form so the
//! caller's click handler can resolve a link click without re-running
//! the layout.
//!
//! Each visible line is rendered as a SINGLE Pango call with an
//! `AttrList` — per-span fg/bold/italic + per-character selection bg
//! become attribute ranges. This avoids the per-span manual-advance bug
//! where proportional Pango widths drift from monospace
//! `char_width * char_count` math (#214 first-cut regression).

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use super::cairo_rgb;
use crate::primitives::rich_text_popup::{RichTextPopup, RichTextPopupLayout, TextSelection};
use crate::theme::Theme;
use crate::types::Color;

/// Visible width of the rich-text-popup scrollbar in pixels. Wider
/// than the layout's 1px border so the bar is paint+click-friendly.
/// Callers that hit-test the scrollbar should match this constant so
/// paint and hit-test geometry stay in sync (#215).
pub const RICH_TEXT_POPUP_SB_WIDTH: f64 = 8.0;
/// Pixels of inset between the scrollbar's right edge and the popup's
/// right border.
pub const RICH_TEXT_POPUP_SB_INSET: f64 = 1.0;

/// Draw a [`RichTextPopup`] at its resolved layout. Returns per-link
/// hit regions in `(x, y, w, h, url)` form.
///
/// `pango_layout` is the editor's monospace Pango layout — the
/// rasteriser temporarily swaps in `ui_font_desc` for popup body
/// rendering, then restores the layout's original font description
/// before returning so subsequent paints in the same frame keep
/// rendering in the editor font (#247).
///
/// The frame border uses [`Theme::link_fg`] when `popup.has_focus`,
/// otherwise [`Theme::hover_border`]. Per-popup `popup.bg` / `popup.fg`
/// overrides win over [`Theme::hover_bg`] / [`Theme::hover_fg`].
#[allow(clippy::too_many_arguments)]
pub fn draw_rich_text_popup(
    cr: &Context,
    pango_layout: &pango::Layout,
    ui_font_desc: &pango::FontDescription,
    popup: &RichTextPopup,
    layout: &RichTextPopupLayout,
    theme: &Theme,
) -> Vec<(f64, f64, f64, f64, String)> {
    let bx = layout.bounds.x as f64;
    let by = layout.bounds.y as f64;
    let bw = layout.bounds.width as f64;
    let bh = layout.bounds.height as f64;
    if bw <= 0.0 || bh <= 0.0 {
        return Vec::new();
    }

    let (bg_r, bg_g, bg_b) = popup
        .bg
        .map(cairo_rgb)
        .unwrap_or_else(|| cairo_rgb(theme.hover_bg));
    let (fg_r, fg_g, fg_b) = popup
        .fg
        .map(cairo_rgb)
        .unwrap_or_else(|| cairo_rgb(theme.hover_fg));
    let (border_r, border_g, border_b) = if popup.has_focus {
        cairo_rgb(theme.link_fg)
    } else {
        cairo_rgb(theme.hover_border)
    };

    // Background.
    cr.set_source_rgb(bg_r, bg_g, bg_b);
    cr.rectangle(bx, by, bw, bh);
    cr.fill().ok();
    // Border.
    cr.set_source_rgb(border_r, border_g, border_b);
    cr.set_line_width(1.0);
    cr.rectangle(bx, by, bw, bh);
    cr.stroke().ok();

    // Save the layout's current font_description before our per-line
    // `set_font_description(ui_font_desc)` calls inside the loop
    // below. Without this, the UI font leaks into subsequent draw
    // calls in the same frame — most visibly: the palette / dialog
    // / context-menu rendering immediately after the hover popup
    // would render in the UI font instead of the editor font (#247
    // second symptom).
    let saved_font = pango_layout.font_description();

    // Clip text rendering to the content area so long lines (Pango is
    // unbounded) and selection rectangles don't bleed past the popup
    // boundary into the editor area behind. Restored at end of draw.
    let content = layout.content_bounds;
    cr.save().ok();
    cr.rectangle(
        content.x as f64,
        content.y as f64,
        content.width as f64,
        content.height as f64,
    );
    cr.clip();

    for vis in &layout.visible_lines {
        let row_y = vis.bounds.y as f64;
        let line_x = vis.bounds.x as f64;
        let line_idx = vis.line_idx;
        let raw_text = popup
            .line_text
            .get(line_idx)
            .map(String::as_str)
            .unwrap_or("");

        // Single-Pango-call render with per-span AttrList.
        if let Some(styled) = popup.lines.get(line_idx) {
            pango_layout.set_text(raw_text);
            pango_layout.set_font_description(Some(ui_font_desc));
            let attrs = pango::AttrList::new();
            // Per-line font scale (markdown headings render larger).
            let line_scale = popup.line_scales.get(line_idx).copied().unwrap_or(1.0);
            if (line_scale - 1.0).abs() > 0.01 {
                let mut a = pango::AttrFloat::new_scale(line_scale as f64);
                a.set_start_index(0);
                a.set_end_index(raw_text.len() as u32);
                attrs.insert(a);
            }
            // Compute selection byte range once for this line.
            let (sel_start_byte, sel_end_byte) = popup
                .selection
                .map(|sel| selection_byte_range(sel, line_idx, raw_text))
                .unwrap_or((0, 0));
            let in_selection = |byte_start: usize, byte_end: usize| -> bool {
                sel_end_byte > sel_start_byte
                    && byte_start >= sel_start_byte
                    && byte_end <= sel_end_byte
            };
            let to_u16 = |c: u8| ((c as u16) << 8) | c as u16;
            let bg_color = popup.bg.unwrap_or(Color::rgb(0, 0, 0));

            // Selection bg used to be a single Pango background attr, but
            // adjacent text runs (one per fg colour change) produced
            // hairline antialiasing gaps where the per-run rects met
            // (#219). The fix paints the selection rect once in Cairo
            // BEFORE the Pango render so the bg is a single solid fill.
            // The Pango call below still inverts fg per-character within
            // the selected range so the text remains legible.

            // Per-span fg + bold + italic. Each span is split by the
            // selection boundary so we can swap the fg colour to the
            // inverted (popup bg) for the in-selection chunk without
            // an attr-override conflict.
            let push_fg_attr = |attrs: &pango::AttrList, start: usize, end: usize, fg: Color| {
                let mut a =
                    pango::AttrColor::new_foreground(to_u16(fg.r), to_u16(fg.g), to_u16(fg.b));
                a.set_start_index(start as u32);
                a.set_end_index(end as u32);
                attrs.insert(a);
            };
            let push_bold = |attrs: &pango::AttrList, start: usize, end: usize| {
                let mut a = pango::AttrInt::new_weight(pango::Weight::Bold);
                a.set_start_index(start as u32);
                a.set_end_index(end as u32);
                attrs.insert(a);
            };
            let push_italic = |attrs: &pango::AttrList, start: usize, end: usize| {
                let mut a = pango::AttrInt::new_style(pango::Style::Italic);
                a.set_start_index(start as u32);
                a.set_end_index(end as u32);
                attrs.insert(a);
            };
            let mut byte_pos: usize = 0;
            for span in &styled.spans {
                let len = span.text.len();
                let start = byte_pos;
                let end = byte_pos + len;

                // Split the span into up-to-three chunks based on
                // selection boundary: pre-selection / in-selection /
                // post-selection. Each chunk gets its own fg attr
                // (with inverted colour for the in-selection chunk).
                let span_fg = span.fg.unwrap_or(bg_color);
                let inv_fg = bg_color;

                let chunk_start_pre = start;
                let chunk_end_pre = end.min(sel_start_byte).max(start);
                let chunk_start_in = start.max(sel_start_byte).min(end);
                let chunk_end_in = end.min(sel_end_byte).max(chunk_start_in);
                let chunk_start_post = end.min(sel_end_byte).max(start);
                let chunk_end_post = end.max(chunk_start_post);

                if span.fg.is_some() && chunk_end_pre > chunk_start_pre {
                    push_fg_attr(&attrs, chunk_start_pre, chunk_end_pre, span_fg);
                }
                if chunk_end_in > chunk_start_in && in_selection(chunk_start_in, chunk_end_in) {
                    push_fg_attr(&attrs, chunk_start_in, chunk_end_in, inv_fg);
                }
                if span.fg.is_some() && chunk_end_post > chunk_start_post {
                    push_fg_attr(&attrs, chunk_start_post, chunk_end_post, span_fg);
                }
                if span.bold {
                    push_bold(&attrs, start, end);
                }
                if span.italic {
                    push_italic(&attrs, start, end);
                }
                byte_pos += len;
            }
            // Focused-link underline.
            if popup.has_focus {
                if let Some(focused) = popup.focused_link {
                    if let Some(link) = popup.links.get(focused) {
                        if link.line == line_idx {
                            let mut ul = pango::AttrInt::new_underline(pango::Underline::Single);
                            ul.set_start_index(link.start_byte as u32);
                            ul.set_end_index(link.end_byte as u32);
                            attrs.insert(ul);
                        }
                    }
                }
            }
            pango_layout.set_attributes(Some(&attrs));

            // Selection bg fill (Cairo rect underneath the text). With
            // attrs applied so `index_to_pos` honours the font scale on
            // heading rows. Pango byte indices clamp to text length, so
            // a sel_end_byte at end-of-line maps to the line's right
            // edge correctly.
            if sel_end_byte > sel_start_byte {
                let fg_color = popup.fg.unwrap_or_else(|| Color::rgb(255, 255, 255));
                let start_pos = pango_layout.index_to_pos(sel_start_byte as i32);
                let end_pos = pango_layout.index_to_pos(sel_end_byte as i32);
                let x0 = line_x + start_pos.x() as f64 / pango::SCALE as f64;
                let x1 = line_x + end_pos.x() as f64 / pango::SCALE as f64;
                let row_h = vis.bounds.height as f64;
                cr.set_source_rgb(
                    fg_color.r as f64 / 255.0,
                    fg_color.g as f64 / 255.0,
                    fg_color.b as f64 / 255.0,
                );
                cr.rectangle(x0.min(x1), row_y, (x1 - x0).abs(), row_h);
                cr.fill().ok();
            }

            cr.set_source_rgb(fg_r, fg_g, fg_b);
            cr.move_to(line_x, row_y);
            pcfn::show_layout(cr, pango_layout);
            pango_layout.set_attributes(None);
        }
    }

    cr.restore().ok(); // pop the content clip

    // Scrollbar — wider than the 1px border so it's visually + clickably
    // present. Draw at the right inside edge of the popup. Constants
    // shared with the caller's hit-test so click hit-test matches what's
    // painted (#215).
    if let Some(sb) = layout.scrollbar {
        let sb_w = RICH_TEXT_POPUP_SB_WIDTH;
        let sb_x = bx + bw - sb_w - RICH_TEXT_POPUP_SB_INSET;
        let track_y = sb.track.y as f64;
        let track_h = sb.track.height as f64;
        // Track background.
        let (sr, sg, sbb) = cairo_rgb(theme.muted_fg);
        cr.set_source_rgba(sr, sg, sbb, 0.3);
        cr.rectangle(sb_x, track_y, sb_w, track_h);
        cr.fill().ok();
        // Thumb.
        let thumb_top_off = (sb.thumb.y - sb.track.y) as f64;
        let thumb_h = sb.thumb.height as f64;
        cr.set_source_rgb(border_r, border_g, border_b);
        cr.rectangle(sb_x + 1.0, track_y + thumb_top_off, sb_w - 2.0, thumb_h);
        cr.fill().ok();
    }

    // Restore the layout's font_description so subsequent popup /
    // overlay paints in the same frame use the editor font, not the
    // UI font we set above. (#247 second symptom.)
    pango_layout.set_font_description(saved_font.as_ref());

    // Link hit regions in (x, y, w, h, url) form.
    layout
        .link_hit_regions
        .iter()
        .map(|(rect, idx)| {
            let url = popup
                .links
                .get(*idx)
                .map(|l| l.url.clone())
                .unwrap_or_default();
            (
                rect.x as f64,
                rect.y as f64,
                rect.width as f64,
                rect.height as f64,
                url,
            )
        })
        .collect()
}

/// Translate a `TextSelection` (in char columns) into the byte range
/// that this line contributes to the selection. Returns `(0, 0)` when
/// the line is outside the selection.
fn selection_byte_range(sel: TextSelection, line_idx: usize, line_text: &str) -> (usize, usize) {
    if line_idx < sel.start_line || line_idx > sel.end_line {
        return (0, 0);
    }
    let char_to_byte = |col: usize| -> usize {
        line_text
            .char_indices()
            .nth(col)
            .map(|(b, _)| b)
            .unwrap_or(line_text.len())
    };
    let (start_col, end_col) = if sel.start_line == sel.end_line {
        (sel.start_col, sel.end_col)
    } else if line_idx == sel.start_line {
        (sel.start_col, line_text.chars().count())
    } else if line_idx == sel.end_line {
        (0, sel.end_col)
    } else {
        (0, line_text.chars().count())
    };
    if end_col <= start_col {
        return (0, 0);
    }
    (char_to_byte(start_col), char_to_byte(end_col))
}
