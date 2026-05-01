//! GTK rasteriser for [`crate::TreeView`].
//!
//! Paints the tree onto a [`Context`] using a [`pango::Layout`] for
//! text measurement. Per-row heights are **non-uniform**: header rows
//! use `line_height`, leaves and ordinary branches use
//! `(line_height * 1.4).round()` (the established GTK convention).
//! The primitive's `tree.layout()` measurer reports each row's
//! height so the visible-row positions stack accurately.

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use super::cairo_rgb;
use crate::primitives::tree::{TreeRowMeasure, TreeView};
use crate::theme::Theme;
use crate::types::Decoration;

/// Draw a [`TreeView`] into `(x, y, w, h)` on `cr`. `nerd_fonts_enabled`
/// controls which icon variant the consumer's icon registry exposes.
///
/// # Visual contract
///
/// - **Background:** [`Theme::tab_bar_bg`].
/// - **Header rows** (`Decoration::Header`):
///   [`Theme::header_bg`] / [`Theme::header_fg`], shorter row
///   (`line_height`).
/// - **Selected row** (when `tree.has_focus`): [`Theme::selected_bg`]
///   with [`Theme::header_fg`] text.
/// - **Muted row** (`Decoration::Muted`): [`Theme::muted_fg`] text on
///   the default row bg.
/// - **Other rows**: [`Theme::foreground`] text on
///   [`Theme::tab_bar_bg`]. Branches and leaves get the same row
///   styling — `is_expanded`-ness only affects chevron rendering.
/// - **Indent:** `(line_height * 0.9).round()` pixels per depth level.
/// - **Chevrons:** [`tree.style.chevron_expanded`] /
///   [`tree.style.chevron_collapsed`] for branches when
///   `tree.style.show_chevrons` is true; leaves get a `line_height *
///   0.8` leading offset for visual alignment.
/// - **Badge** (right-aligned): rendered in `badge.fg`/`badge.bg`
///   (falling back to [`Theme::muted_fg`] / row bg) when there's
///   room past the text.
#[allow(clippy::too_many_arguments)]
pub fn draw_tree(
    cr: &Context,
    layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    tree: &TreeView,
    theme: &Theme,
    line_height: f64,
    nerd_fonts_enabled: bool,
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

    cr.set_source_rgb(bg.0, bg.1, bg.2);
    cr.rectangle(x, y, w, h);
    cr.fill().ok();

    layout.set_attributes(None);

    let item_height = (line_height * 1.4).round();
    let indent_px = (line_height * 0.9).round();

    let tree_layout = tree.layout(w as f32, h as f32, |i| {
        let is_header = matches!(tree.rows[i].decoration, Decoration::Header);
        TreeRowMeasure::new(if is_header {
            line_height as f32
        } else {
            item_height as f32
        })
    });

    for vis_row in &tree_layout.visible_rows {
        let row = &tree.rows[vis_row.row_idx];
        let row_y = y + vis_row.bounds.y as f64;
        let row_h = vis_row.bounds.height as f64;

        let is_header = matches!(row.decoration, Decoration::Header);
        let is_selected =
            tree.has_focus && tree.selected_path.as_ref().is_some_and(|p| p == &row.path);

        let (def_fg, row_bg) = if is_selected {
            (hdr_fg, sel)
        } else if is_header {
            (hdr_fg, hdr_bg)
        } else if matches!(row.decoration, Decoration::Muted) {
            (dim, bg)
        } else {
            (fg, bg)
        };

        cr.set_source_rgb(row_bg.0, row_bg.1, row_bg.2);
        cr.rectangle(x, row_y, w, row_h);
        cr.fill().ok();

        let mut cursor_x = x + 2.0 + (row.indent as f64) * indent_px;

        if let Some(expanded) = row.is_expanded {
            if tree.style.show_chevrons {
                let chevron = if expanded {
                    &tree.style.chevron_expanded
                } else {
                    &tree.style.chevron_collapsed
                };
                cr.set_source_rgb(def_fg.0, def_fg.1, def_fg.2);
                layout.set_text(chevron);
                let (cw, ch) = layout.pixel_size();
                cr.move_to(cursor_x, row_y + (row_h - ch as f64) / 2.0);
                pcfn::show_layout(cr, layout);
                cursor_x += cw as f64 + 4.0;
            }
        } else {
            cursor_x += line_height * 0.8;
        }

        if let Some(ref icon) = row.icon {
            let glyph = if nerd_fonts_enabled {
                icon.glyph.as_str()
            } else {
                icon.fallback.as_str()
            };
            cr.set_source_rgb(def_fg.0, def_fg.1, def_fg.2);
            layout.set_text(glyph);
            let (iw, ih) = layout.pixel_size();
            cr.move_to(cursor_x, row_y + (row_h - ih as f64) / 2.0);
            pcfn::show_layout(cr, layout);
            cursor_x += iw as f64 + 6.0;
        }

        let badge_info = row.badge.as_ref().map(|badge| {
            layout.set_text(&badge.text);
            let (bw, _) = layout.pixel_size();
            let bfg = badge.fg.map(cairo_rgb).unwrap_or(dim);
            let bbg = badge.bg.map(cairo_rgb).unwrap_or(row_bg);
            (badge.text.clone(), bw as f64, bfg, bbg)
        });
        let badge_reserve = badge_info
            .as_ref()
            .map(|(_, bw, ..)| *bw + 8.0)
            .unwrap_or(0.0);
        let text_right_limit = x + w - badge_reserve - 4.0;

        for span in &row.text.spans {
            if cursor_x >= text_right_limit {
                break;
            }
            let span_fg = if let Some(c) = span.fg {
                cairo_rgb(c)
            } else if matches!(row.decoration, Decoration::Muted) {
                dim
            } else {
                def_fg
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

        if let Some((btext, bw, bfg, bbg)) = badge_info {
            let bx = x + w - bw - 4.0;
            if bx > cursor_x {
                if bbg != row_bg {
                    cr.set_source_rgb(bbg.0, bbg.1, bbg.2);
                    cr.rectangle(bx - 2.0, row_y, bw + 4.0, row_h);
                    cr.fill().ok();
                }
                cr.set_source_rgb(bfg.0, bfg.1, bfg.2);
                layout.set_text(&btext);
                let (_, bh) = layout.pixel_size();
                cr.move_to(bx, row_y + (row_h - bh as f64) / 2.0);
                pcfn::show_layout(cr, layout);
            }
        }
    }

    layout.set_attributes(None);
}
