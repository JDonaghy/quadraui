//! GTK rasteriser for [`crate::Form`].
//!
//! Cairo + Pango equivalent of `quadraui::tui::draw_form`. Per-field
//! row height is `(line_height * 1.4).round()` (the established GTK
//! convention shared with `TreeView` leaves and `ListView` items).
//!
//! The new `#143` field kinds (`Slider` / `ColorPicker` / `Dropdown`)
//! are not yet rendered — those land with their own migration. For
//! now the row is blank on the right side, so existing forms keep
//! working.

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use super::cairo_rgb;
use crate::primitives::form::{FieldKind, Form};
use crate::theme::Theme;

/// Draw a [`Form`] into `(x, y, w, h)` on `cr` using `layout` for text
/// measurement.
///
/// Visual contract mirrors `quadraui::tui::draw_form` — see that
/// module's docs. Per-row height = `(line_height * 1.4).round()`.
#[allow(clippy::too_many_arguments)]
pub fn draw_form(
    cr: &Context,
    layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    form: &Form,
    theme: &Theme,
    line_height: f64,
) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }

    let bg = cairo_rgb(theme.tab_bar_bg);
    let hdr_bg = cairo_rgb(theme.header_bg);
    let hdr_fg = cairo_rgb(theme.header_fg);
    let fg = cairo_rgb(theme.foreground);
    let dim = cairo_rgb(theme.muted_fg);
    let sel = cairo_rgb(theme.selected_bg);
    let accent = cairo_rgb(theme.accent_fg);

    cr.set_source_rgb(bg.0, bg.1, bg.2);
    cr.rectangle(x, y, w, h);
    cr.fill().ok();
    layout.set_attributes(None);

    let row_h = (line_height * 1.4).round();
    let mut y_off = y;
    let y_end = y + h;

    for field in form.fields.iter().skip(form.scroll_offset) {
        if y_off + row_h > y_end {
            break;
        }

        let is_focused = form.has_focus
            && form
                .focused_field
                .as_ref()
                .is_some_and(|id| id == &field.id);
        let is_header = matches!(field.kind, FieldKind::Label);

        let (default_fg, row_bg) = if is_focused {
            (fg, sel)
        } else if is_header {
            (hdr_fg, hdr_bg)
        } else {
            (fg, bg)
        };

        cr.set_source_rgb(row_bg.0, row_bg.1, row_bg.2);
        cr.rectangle(x, y_off, w, row_h);
        cr.fill().ok();

        let field_fg = if field.disabled { dim } else { default_fg };

        let label_text: String = field.label.spans.iter().map(|s| s.text.as_str()).collect();
        cr.set_source_rgb(field_fg.0, field_fg.1, field_fg.2);
        layout.set_text(&label_text);
        let (label_w, label_h) = layout.pixel_size();
        let label_x = x + 6.0;
        cr.move_to(label_x, y_off + (row_h - label_h as f64) / 2.0);
        pcfn::show_layout(cr, layout);
        let label_right = label_x + label_w as f64;

        let input_right = x + w - 8.0;
        match &field.kind {
            FieldKind::Label => {}
            FieldKind::Toggle { value } => {
                let glyph = if *value { "[x]" } else { "[ ]" };
                let fg_color = if *value && !field.disabled {
                    accent
                } else {
                    field_fg
                };
                cr.set_source_rgb(fg_color.0, fg_color.1, fg_color.2);
                layout.set_text(glyph);
                let (iw, ih) = layout.pixel_size();
                let ix = input_right - iw as f64;
                if ix > label_right + 8.0 {
                    cr.move_to(ix, y_off + (row_h - ih as f64) / 2.0);
                    pcfn::show_layout(cr, layout);
                }
            }
            FieldKind::TextInput {
                value,
                placeholder,
                cursor,
                selection_anchor,
            } => {
                let shown = if value.is_empty() {
                    placeholder.as_str()
                } else {
                    value.as_str()
                };
                let input_fg = if value.is_empty() { dim } else { field_fg };

                layout.set_text(shown);
                let (shown_w, shown_h) = layout.pixel_size();

                let max_width = (w * 0.6).max(80.0);
                let draw_w = (shown_w as f64).min(max_width);
                let ix = input_right - draw_w - 14.0;
                if ix > label_right + 8.0 {
                    cr.set_source_rgb(dim.0, dim.1, dim.2);
                    layout.set_text("[");
                    cr.move_to(ix, y_off + (row_h - shown_h as f64) / 2.0);
                    pcfn::show_layout(cr, layout);

                    if let (Some(cur), Some(anchor)) = (cursor, selection_anchor) {
                        if *cur != *anchor && !value.is_empty() {
                            let (lo, hi) = (*cur.min(anchor), *cur.max(anchor));
                            let prefix = &shown[..lo.min(shown.len())];
                            let sel_text = &shown[lo.min(shown.len())..hi.min(shown.len())];
                            layout.set_text(prefix);
                            let (prefix_w, _) = layout.pixel_size();
                            layout.set_text(sel_text);
                            let (sel_w, _) = layout.pixel_size();
                            cr.set_source_rgb(sel.0, sel.1, sel.2);
                            cr.rectangle(
                                ix + 8.0 + prefix_w as f64,
                                y_off + 2.0,
                                sel_w as f64,
                                row_h - 4.0,
                            );
                            cr.fill().ok();
                        }
                    }

                    cr.set_source_rgb(input_fg.0, input_fg.1, input_fg.2);
                    layout.set_text(shown);
                    cr.move_to(ix + 8.0, y_off + (row_h - shown_h as f64) / 2.0);
                    pcfn::show_layout(cr, layout);

                    cr.set_source_rgb(dim.0, dim.1, dim.2);
                    layout.set_text("]");
                    cr.move_to(
                        ix + 8.0 + draw_w + 2.0,
                        y_off + (row_h - shown_h as f64) / 2.0,
                    );
                    pcfn::show_layout(cr, layout);

                    if let Some(cur) = cursor {
                        if !value.is_empty() {
                            let prefix = &shown[..(*cur).min(shown.len())];
                            layout.set_text(prefix);
                            let (prefix_w, _) = layout.pixel_size();
                            let cx = ix + 8.0 + prefix_w as f64;
                            cr.set_source_rgb(accent.0, accent.1, accent.2);
                            cr.rectangle(cx, y_off + 3.0, 1.5, row_h - 6.0);
                            cr.fill().ok();
                        }
                    }
                }
            }
            FieldKind::Button => {
                cr.set_source_rgb(row_bg.0, row_bg.1, row_bg.2);
                cr.rectangle(x, y_off, label_right - x + 1.0, row_h);
                cr.fill().ok();

                let cap_text: String = field.label.spans.iter().map(|s| s.text.as_str()).collect();
                layout.set_text(&cap_text);
                let (cap_w, cap_h) = layout.pixel_size();
                let total_w = cap_w as f64 + 24.0;
                let ix = input_right - total_w;
                if ix > x + 8.0 {
                    let brk = if is_focused { accent } else { dim };
                    cr.set_source_rgb(brk.0, brk.1, brk.2);
                    layout.set_text("<");
                    cr.move_to(ix, y_off + (row_h - cap_h as f64) / 2.0);
                    pcfn::show_layout(cr, layout);

                    cr.set_source_rgb(field_fg.0, field_fg.1, field_fg.2);
                    layout.set_text(&cap_text);
                    cr.move_to(ix + 12.0, y_off + (row_h - cap_h as f64) / 2.0);
                    pcfn::show_layout(cr, layout);

                    cr.set_source_rgb(brk.0, brk.1, brk.2);
                    layout.set_text(">");
                    cr.move_to(
                        ix + 12.0 + cap_w as f64 + 4.0,
                        y_off + (row_h - cap_h as f64) / 2.0,
                    );
                    pcfn::show_layout(cr, layout);
                }
            }
            FieldKind::ReadOnly { value } => {
                let value_text: String = value.spans.iter().map(|s| s.text.as_str()).collect();
                layout.set_text(&value_text);
                let (vw, vh) = layout.pixel_size();
                let ix = input_right - vw as f64;
                if ix > label_right + 8.0 {
                    cr.set_source_rgb(dim.0, dim.1, dim.2);
                    cr.move_to(ix, y_off + (row_h - vh as f64) / 2.0);
                    pcfn::show_layout(cr, layout);
                }
            }
            FieldKind::Slider { .. }
            | FieldKind::ColorPicker { .. }
            | FieldKind::Dropdown { .. } => {}
        }

        y_off += row_h;
    }

    layout.set_attributes(None);
}

/// Settings panel chrome: a 2-row strip with a header row and a search
/// input row, designed to sit immediately above a [`Form`] body.
///
/// Total chrome height = `2 * line_height` pixels — the first
/// `line_height` is the header (`header_bg` / `header_fg`), the second
/// is the search input (full-width tinted `selected_bg` when `active`,
/// otherwise the panel `tab_bar_bg`). Layout from left to right inside
/// the search row: ` /  ` prefix in `muted_fg`, then either `query` (in
/// `foreground`) or `placeholder` (in `muted_fg`) when the query is
/// empty + inactive. A 1.5px-wide `accent_fg` cursor follows the query
/// when `active`.
///
/// Chrome only — the form body and any scrollbar layered below are
/// painted separately by the caller.
#[allow(clippy::too_many_arguments)]
pub fn draw_settings_chrome(
    cr: &Context,
    layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    line_height: f64,
    header_text: &str,
    query: &str,
    placeholder: &str,
    active: bool,
    theme: &Theme,
) {
    if w <= 0.0 || line_height <= 0.0 {
        return;
    }

    let bg = cairo_rgb(theme.tab_bar_bg);
    let hdr_bg = cairo_rgb(theme.header_bg);
    let hdr_fg = cairo_rgb(theme.header_fg);
    let fg = cairo_rgb(theme.foreground);
    let dim = cairo_rgb(theme.muted_fg);
    let sel = cairo_rgb(theme.selected_bg);
    let accent = cairo_rgb(theme.accent_fg);

    layout.set_attributes(None);

    // Row 0: header bar.
    cr.set_source_rgb(hdr_bg.0, hdr_bg.1, hdr_bg.2);
    cr.rectangle(x, y, w, line_height);
    cr.fill().ok();
    cr.set_source_rgb(hdr_fg.0, hdr_fg.1, hdr_fg.2);
    layout.set_text(header_text);
    let (_, header_lh) = layout.pixel_size();
    cr.move_to(x + 2.0, y + (line_height - header_lh as f64) / 2.0);
    pcfn::show_layout(cr, layout);

    // Row 1: search input.
    let search_y = y + line_height;
    let (sb_r, sb_g, sb_b) = if active { sel } else { bg };
    cr.set_source_rgb(sb_r, sb_g, sb_b);
    cr.rectangle(x, search_y, w, line_height);
    cr.fill().ok();

    let prefix = " /  ";
    cr.set_source_rgb(dim.0, dim.1, dim.2);
    layout.set_text(prefix);
    let (prefix_w, _) = layout.pixel_size();
    cr.move_to(x + 2.0, search_y + (line_height - header_lh as f64) / 2.0);
    pcfn::show_layout(cr, layout);

    let q_x = x + 2.0 + prefix_w as f64;
    let show_placeholder = query.is_empty() && !placeholder.is_empty() && !active;
    let (text, color) = if show_placeholder {
        (placeholder, dim)
    } else if query.is_empty() {
        (query, dim)
    } else {
        (query, fg)
    };
    cr.set_source_rgb(color.0, color.1, color.2);
    layout.set_text(text);
    let (q_w, _) = layout.pixel_size();
    cr.move_to(q_x, search_y + (line_height - header_lh as f64) / 2.0);
    pcfn::show_layout(cr, layout);

    if active {
        let cur_x = q_x + if query.is_empty() { 0.0 } else { q_w as f64 };
        cr.set_source_rgb(accent.0, accent.1, accent.2);
        cr.rectangle(cur_x, search_y + 2.0, 1.5, line_height - 4.0);
        cr.fill().ok();
    }
}
