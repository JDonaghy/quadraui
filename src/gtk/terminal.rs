//! GTK rasteriser for [`crate::Terminal`] cell grids.
//!
//! Iterates rows then per-cell, painting cell background then per-cell
//! glyph (skipped for spaces and `\0`). Overlay flags
//! (`is_cursor`, `is_find_active`, `is_find_match`, `selected`)
//! override the cell's `bg`/`fg` to match the legacy bespoke
//! renderer's behaviour. Bold / italic / underline applied via Pango
//! `AttrList` per cell.

use gtk4::cairo::Context;
use gtk4::pango;
use gtk4::pango::AttrList;

use crate::primitives::terminal::Terminal;
use crate::theme::Theme;

/// Draw `term`'s cell grid into the rectangular region starting at
/// `(x, content_y)` on `cr`. The caller is responsible for filling
/// the surrounding background (vimcode does this with
/// `theme.terminal_bg` before calling so the area outside the cell
/// grid stays consistent).
///
/// `cell_area_w` clips per-row painting — cells past the right edge
/// stop being drawn rather than wrapping. `line_height` and
/// `char_width` are the per-cell dimensions in DIPs.
#[allow(clippy::too_many_arguments)]
pub fn draw_terminal_cells(
    cr: &Context,
    layout: &pango::Layout,
    term: &Terminal,
    x: f64,
    content_y: f64,
    cell_area_w: f64,
    line_height: f64,
    char_width: f64,
    theme: &Theme,
) {
    for (row_idx, row) in term.cells.iter().enumerate() {
        let row_y = content_y + row_idx as f64 * line_height;
        let mut cell_x = x;
        for cell in row {
            if cell_x + char_width > x + cell_area_w {
                break;
            }
            let (br, bg, bb) = (cell.bg.r, cell.bg.g, cell.bg.b);
            let (fr, fg2, fb) = (cell.fg.r, cell.fg.g, cell.fg.b);
            let (draw_br, draw_bg, draw_bb) = if cell.is_cursor {
                (fr, fg2, fb)
            } else if cell.is_find_active {
                (255u8, 165u8, 0u8)
            } else if cell.is_find_match {
                (100u8, 80u8, 20u8)
            } else if cell.selected {
                (
                    theme.selection_bg.r,
                    theme.selection_bg.g,
                    theme.selection_bg.b,
                )
            } else {
                (br, bg, bb)
            };
            cr.set_source_rgb(
                draw_br as f64 / 255.0,
                draw_bg as f64 / 255.0,
                draw_bb as f64 / 255.0,
            );
            cr.rectangle(cell_x, row_y, char_width, line_height);
            cr.fill().ok();

            if cell.ch != ' ' && cell.ch != '\0' {
                let (draw_fr, draw_fg, draw_fb) = if cell.is_cursor {
                    (br, bg, bb)
                } else if cell.is_find_active {
                    (0u8, 0u8, 0u8)
                } else {
                    (fr, fg2, fb)
                };
                cr.set_source_rgb(
                    draw_fr as f64 / 255.0,
                    draw_fg as f64 / 255.0,
                    draw_fb as f64 / 255.0,
                );

                let attrs = AttrList::new();
                if cell.bold {
                    attrs.insert(pango::AttrInt::new_weight(pango::Weight::Bold));
                }
                if cell.italic {
                    attrs.insert(pango::AttrInt::new_style(pango::Style::Italic));
                }
                if cell.underline {
                    attrs.insert(pango::AttrInt::new_underline(pango::Underline::Single));
                }
                layout.set_attributes(Some(&attrs));
                let s = cell.ch.to_string();
                layout.set_text(&s);
                cr.move_to(cell_x, row_y);
                pangocairo::functions::show_layout(cr, layout);
                layout.set_attributes(None);
            }

            cell_x += char_width;
        }
    }
}
