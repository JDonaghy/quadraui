//! GTK rasteriser for [`crate::ListView`].
//!
//! Paints the list onto a [`Context`] using a [`pango::Layout`] for
//! text measurement. Title / item rows / decoration colouring follow
//! the same visual contract as the TUI rasteriser; pixel positioning
//! comes from Pango.
//!
//! The `bordered` flag is **not yet honoured** — no GTK consumer
//! today sets `bordered = true`. If a consumer needs it, add the
//! rounded-rectangle border + title overlay in this module
//! (a separate slice of work; see TUI's `tui::list` for the visual
//! reference).

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use super::cairo_rgb;
use crate::primitives::list::{ListItemMeasure, ListView};
use crate::theme::Theme;
use crate::types::Decoration;

/// Draw a [`ListView`] into `(x, y, w, h)` on `cr`.
///
/// Caller owns `layout`'s font choice — the rasteriser doesn't
/// switch fonts. `nerd_fonts_enabled` controls which icon variant
/// the consumer's icon registry exposes; pass `false` to always
/// use the ASCII fallback.
///
/// # Visual contract
///
/// - **Background:** [`Theme::background`] (matches the editor surface
///   the list is embedded in).
/// - **Optional title:** painted as a flat [`Theme::header_bg`] /
///   [`Theme::header_fg`] strip at the top.
/// - **Selected row:** [`Theme::selected_bg`] background and a `▶`
///   selection prefix.
/// - **Header decoration:** items with [`Decoration::Header`] use
///   [`Theme::header_bg`] / [`Theme::header_fg`] (used by the source
///   control panel for section titles).
/// - **Per-item decoration → fg:** `Error → error_fg`, `Warning →
///   warning_fg`, `Muted → muted_fg`, `Header → header_fg`, others
///   → [`Theme::surface_fg`].
/// - **Detail span:** right-aligned in [`Theme::muted_fg`], skipped
///   when there isn't room past the main text.
#[allow(clippy::too_many_arguments)]
pub fn draw_list(
    cr: &Context,
    layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    list: &ListView,
    theme: &Theme,
    line_height: f64,
    nerd_fonts_enabled: bool,
) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }

    let bg = cairo_rgb(theme.background);
    let hdr_bg = cairo_rgb(theme.header_bg);
    let hdr_fg = cairo_rgb(theme.header_fg);
    let fg = cairo_rgb(theme.surface_fg);
    let dim = cairo_rgb(theme.muted_fg);
    let sel = cairo_rgb(theme.selected_bg);
    let err = cairo_rgb(theme.error_fg);
    let warn = cairo_rgb(theme.warning_fg);

    cr.set_source_rgb(bg.0, bg.1, bg.2);
    cr.rectangle(x, y, w, h);
    cr.fill().ok();

    layout.set_attributes(None);

    let title_h = if list.title.is_some() {
        line_height as f32
    } else {
        0.0
    };
    let list_layout = list.layout(w as f32, h as f32, title_h, |_| {
        ListItemMeasure::new(line_height as f32)
    });

    if let (Some(title_bounds), Some(title)) = (list_layout.title_bounds, list.title.as_ref()) {
        let ty = y + title_bounds.y as f64;
        let th_px = title_bounds.height as f64;
        cr.set_source_rgb(hdr_bg.0, hdr_bg.1, hdr_bg.2);
        cr.rectangle(x, ty, w, th_px);
        cr.fill().ok();

        cr.set_source_rgb(hdr_fg.0, hdr_fg.1, hdr_fg.2);
        let title_text: String = title.spans.iter().map(|s| s.text.as_str()).collect();
        layout.set_text(&title_text);
        let (_, text_h) = layout.pixel_size();
        cr.move_to(x + 2.0, ty + (th_px - text_h as f64) / 2.0);
        pcfn::show_layout(cr, layout);
    }

    for vis_item in &list_layout.visible_items {
        let item = &list.items[vis_item.item_idx];
        let row_y = y + vis_item.bounds.y as f64;
        let row_w = vis_item.bounds.width as f64;
        let row_h = vis_item.bounds.height as f64;

        let is_selected = vis_item.item_idx == list.selected_idx && list.has_focus;

        let decoration_fg = match item.decoration {
            Decoration::Error => err,
            Decoration::Warning => warn,
            Decoration::Muted => dim,
            Decoration::Header => hdr_fg,
            _ => fg,
        };
        let row_bg = if is_selected {
            sel
        } else if matches!(item.decoration, Decoration::Header) {
            hdr_bg
        } else {
            bg
        };

        cr.set_source_rgb(row_bg.0, row_bg.1, row_bg.2);
        cr.rectangle(x, row_y, row_w, row_h);
        cr.fill().ok();

        let mut cursor_x = x + 2.0;

        let prefix = if is_selected { "▶ " } else { "  " };
        cr.set_source_rgb(decoration_fg.0, decoration_fg.1, decoration_fg.2);
        layout.set_text(prefix);
        let (pw, ph) = layout.pixel_size();
        cr.move_to(cursor_x, row_y + (row_h - ph as f64) / 2.0);
        pcfn::show_layout(cr, layout);
        cursor_x += pw as f64;

        if let Some(ref icon) = item.icon {
            let glyph = if nerd_fonts_enabled {
                icon.glyph.as_str()
            } else {
                icon.fallback.as_str()
            };
            cr.set_source_rgb(decoration_fg.0, decoration_fg.1, decoration_fg.2);
            layout.set_text(glyph);
            let (iw, ih) = layout.pixel_size();
            cr.move_to(cursor_x, row_y + (row_h - ih as f64) / 2.0);
            pcfn::show_layout(cr, layout);
            cursor_x += iw as f64 + 6.0;
        }

        let detail_info = item.detail.as_ref().map(|detail| {
            let detail_text: String = detail.spans.iter().map(|s| s.text.as_str()).collect();
            layout.set_text(&detail_text);
            let (dw, _) = layout.pixel_size();
            (detail_text, dw as f64)
        });
        let detail_reserve = detail_info.as_ref().map(|(_, dw)| *dw + 8.0).unwrap_or(0.0);
        let text_right_limit = x + row_w - detail_reserve - 4.0;

        for span in &item.text.spans {
            if cursor_x >= text_right_limit {
                break;
            }
            let span_fg = if let Some(c) = span.fg {
                cairo_rgb(c)
            } else {
                decoration_fg
            };
            if let Some(sbg) = span.bg {
                let span_bg = cairo_rgb(sbg);
                layout.set_text(&span.text);
                let (sw, _) = layout.pixel_size();
                cr.set_source_rgb(span_bg.0, span_bg.1, span_bg.2);
                cr.rectangle(
                    cursor_x,
                    row_y,
                    (sw as f64).min(text_right_limit - cursor_x),
                    row_h,
                );
                cr.fill().ok();
            }
            cr.set_source_rgb(span_fg.0, span_fg.1, span_fg.2);
            layout.set_text(&span.text);
            let (sw, sh) = layout.pixel_size();
            cr.move_to(cursor_x, row_y + (row_h - sh as f64) / 2.0);
            pcfn::show_layout(cr, layout);
            cursor_x += sw as f64;
        }

        if let Some((detail_text, dw)) = detail_info {
            let dx = x + row_w - dw - 4.0;
            if dx > cursor_x {
                cr.set_source_rgb(dim.0, dim.1, dim.2);
                layout.set_text(&detail_text);
                let (_, dh) = layout.pixel_size();
                cr.move_to(dx, row_y + (row_h - dh as f64) / 2.0);
                pcfn::show_layout(cr, layout);
            }
        }
    }

    layout.set_attributes(None);
}
