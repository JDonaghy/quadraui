//! GTK rasteriser for [`crate::Dialog`].
//!
//! Cairo + Pango equivalent of `quadraui::tui::draw_dialog`. Returns
//! the per-button hit-rectangles `(x, y, w, h)` so the caller's click
//! handler can resolve a click to a button without re-running the
//! layout.
//!
//! Takes a single `pango_layout` (typically the editor's monospace
//! layout) plus a separate `ui_font_desc` for the title + buttons.
//! The rasteriser swaps fonts on the layout per-region (same pattern
//! `tab_bar` and `rich_text_popup` use). Saves the layout's original
//! font description on entry and restores it before returning so
//! subsequent paints in the same frame keep rendering in the editor
//! font (#247).

use gtk4::cairo::Context;
use gtk4::pango;

use super::cairo_rgb;
use crate::primitives::dialog::{Dialog, DialogInput, DialogLayout, DialogTable};
use crate::theme::Theme;
use crate::types::StyledText;

fn flatten(text: &StyledText) -> String {
    text.spans.iter().map(|s| s.text.as_str()).collect()
}

/// Draw a [`DialogTable`] at `bounds` using Pango.
///
/// Column widths are auto-computed from content using `pango_layout` pixel
/// metrics. Columns are separated by a space-│-space glyph run. When
/// `table.headers` is `Some`, a header row is drawn first, followed by a
/// plain `──────` dash separator row (no `┼` junction — full-width dashes
/// span the table), then the data rows.
#[allow(clippy::too_many_arguments)]
fn draw_table_gtk(
    cr: &Context,
    pango_layout: &pango::Layout,
    table: &DialogTable,
    table_x: f64,
    table_y: f64,
    line_height: f64,
    fg: (f64, f64, f64),
    border: (f64, f64, f64),
) {
    let ncols = table.num_cols();
    if ncols == 0 {
        return;
    }

    // Measure each column width in pixels by iterating headers + rows.
    let mut col_widths_px = vec![0.0f64; ncols];

    let measure_text = |text: &str| -> f64 {
        pango_layout.set_text(text);
        pango_layout.set_attributes(None);
        let (w, _) = pango_layout.pixel_size();
        w as f64
    };

    if let Some(headers) = &table.headers {
        for (j, h) in headers.iter().enumerate() {
            if j < ncols {
                let w = measure_text(h);
                if w > col_widths_px[j] {
                    col_widths_px[j] = w;
                }
            }
        }
    }
    for row in &table.rows {
        for (j, cell) in row.iter().enumerate() {
            if j < ncols {
                let w = measure_text(cell);
                if w > col_widths_px[j] {
                    col_widths_px[j] = w;
                }
            }
        }
    }

    // If column_widths is provided, use them as minimums (convert from
    // char-cell hint to pixels roughly via line_height ratio — backends
    // may override this with proper font metrics).
    if let Some(explicit) = &table.column_widths {
        for (j, &w) in explicit.iter().enumerate() {
            if j < ncols {
                let px = w as f64 * (line_height * 0.6); // approx char width
                if px > col_widths_px[j] {
                    col_widths_px[j] = px;
                }
            }
        }
    }

    // Build column x positions.
    let sep_w = measure_text(" │ ");
    let mut col_x = vec![0.0f64; ncols];
    let mut cursor_x = table_x;
    for j in 0..ncols {
        col_x[j] = cursor_x;
        cursor_x += col_widths_px[j];
        if j + 1 < ncols {
            cursor_x += sep_w;
        }
    }

    let mut row_y = table_y;

    // Header row.
    if let Some(headers) = &table.headers {
        cr.set_source_rgb(fg.0, fg.1, fg.2);
        for (j, h) in headers.iter().enumerate() {
            if j < ncols {
                pango_layout.set_text(h);
                pango_layout.set_attributes(None);
                cr.move_to(col_x[j], row_y);
                super::painted_text::show_layout(cr, pango_layout);
            }
        }
        // Separators.
        cr.set_source_rgb(border.0, border.1, border.2);
        for j in 0..ncols.saturating_sub(1) {
            let sep_x = col_x[j] + col_widths_px[j];
            pango_layout.set_text(" │ ");
            pango_layout.set_attributes(None);
            cr.move_to(sep_x, row_y);
            super::painted_text::show_layout(cr, pango_layout);
        }
        row_y += line_height;

        // Separator row: ────── (plain dashes; no ┼ junction on GTK)
        cr.set_source_rgb(border.0, border.1, border.2);
        let total_w = col_x[ncols - 1] + col_widths_px[ncols - 1] - table_x;
        let dash_count = (total_w / (line_height * 0.6)).ceil() as usize + 4;
        let dash_str: String = "─".repeat(dash_count);
        pango_layout.set_text(&dash_str);
        pango_layout.set_attributes(None);
        cr.move_to(table_x, row_y);
        super::painted_text::show_layout(cr, pango_layout);
        row_y += line_height;
    }

    // Data rows.
    for row in &table.rows {
        cr.set_source_rgb(fg.0, fg.1, fg.2);
        for (j, cell) in row.iter().enumerate() {
            if j < ncols {
                pango_layout.set_text(cell);
                pango_layout.set_attributes(None);
                cr.move_to(col_x[j], row_y);
                super::painted_text::show_layout(cr, pango_layout);
            }
        }
        // Separators.
        cr.set_source_rgb(border.0, border.1, border.2);
        for j in 0..ncols.saturating_sub(1) {
            let sep_x = col_x[j] + col_widths_px[j];
            pango_layout.set_text(" │ ");
            pango_layout.set_attributes(None);
            cr.move_to(sep_x, row_y);
            super::painted_text::show_layout(cr, pango_layout);
        }
        row_y += line_height;
    }
}

/// Draw a [`Dialog`] at its resolved layout. Returns
/// `Vec<(x, y, w, h)>` per visible button.
///
/// `pango_layout` is the editor's monospace Pango layout — the
/// rasteriser temporarily swaps in `ui_font_desc` for title +
/// button rendering, then restores the layout's original font
/// description before returning.
#[allow(clippy::too_many_arguments)]
pub fn draw_dialog(
    cr: &Context,
    pango_layout: &pango::Layout,
    ui_font_desc: &pango::FontDescription,
    dialog: &Dialog,
    dialog_layout: &DialogLayout,
    line_height: f64,
    theme: &Theme,
) -> Vec<(f64, f64, f64, f64)> {
    let bounds = dialog_layout.bounds;
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return Vec::new();
    }

    let bg = cairo_rgb(theme.surface_bg);
    let fg = cairo_rgb(theme.surface_fg);
    let border = cairo_rgb(theme.border_fg);
    let sel = cairo_rgb(theme.selected_bg);
    let input_bg = cairo_rgb(theme.input_bg);
    let title = cairo_rgb(theme.title_fg);

    let bx = bounds.x as f64;
    let by = bounds.y as f64;
    let bw = bounds.width as f64;
    let bh = bounds.height as f64;

    cr.set_source_rgb(bg.0, bg.1, bg.2);
    cr.rectangle(bx, by, bw, bh);
    cr.fill().ok();

    cr.set_source_rgb(border.0, border.1, border.2);
    cr.set_line_width(1.0);
    cr.rectangle(bx, by, bw, bh);
    cr.stroke().ok();

    // Save the layout's existing (editor / mono) font description so
    // we can swap to `ui_font_desc` for title + buttons and restore at
    // the end. Without this, the UI font would leak into subsequent
    // draw calls in the same frame (#247).
    let saved_font = pango_layout.font_description();

    if let Some(title_rect) = dialog_layout.title_bounds {
        cr.set_source_rgb(title.0, title.1, title.2);
        pango_layout.set_font_description(Some(ui_font_desc));
        pango_layout.set_text(&flatten(&dialog.title));
        pango_layout.set_attributes(None);
        cr.move_to(title_rect.x as f64, title_rect.y as f64);
        super::painted_text::show_layout(cr, pango_layout);
    }

    // Body + input render in the layout's saved (editor / mono) font.
    pango_layout.set_font_description(saved_font.as_ref());

    let body_b = dialog_layout.body_bounds;
    cr.set_source_rgb(fg.0, fg.1, fg.2);
    for (i, line) in dialog.body.iter().enumerate() {
        let row_y = body_b.y as f64 + i as f64 * line_height;
        if row_y + line_height > body_b.y as f64 + body_b.height as f64 {
            break;
        }
        let text = flatten(line);
        pango_layout.set_text(&text);
        pango_layout.set_attributes(None);
        cr.move_to(body_b.x as f64, row_y);
        super::painted_text::show_layout(cr, pango_layout);
    }

    // Optional table slot.
    if let (Some(table_b), Some(table)) = (dialog_layout.table_bounds, dialog.table.as_ref()) {
        draw_table_gtk(
            cr,
            pango_layout,
            table,
            table_b.x as f64,
            table_b.y as f64,
            line_height,
            fg,
            border,
        );
    }

    if let (Some(input_b), Some(input_kind)) = (dialog_layout.input_bounds, dialog.input.as_ref()) {
        let ix = input_b.x as f64;
        let iy = input_b.y as f64;
        let iw = input_b.width as f64;
        let ih = input_b.height as f64;
        match input_kind {
            DialogInput::TextInput(input) => {
                cr.set_source_rgb(input_bg.0, input_bg.1, input_bg.2);
                cr.rectangle(ix, iy, iw, ih);
                cr.fill().ok();
                cr.set_source_rgb(border.0, border.1, border.2);
                cr.rectangle(ix, iy, iw, ih);
                cr.stroke().ok();
                cr.set_source_rgb(fg.0, fg.1, fg.2);
                let display = if input.value.is_empty() {
                    format!(" {}", input.placeholder)
                } else {
                    format!(" {}", input.value)
                };
                pango_layout.set_text(&display);
                pango_layout.set_attributes(None);
                let (_, ilh) = pango_layout.pixel_size();
                cr.move_to(ix + 2.0, iy + (ih - ilh as f64) / 2.0);
                super::painted_text::show_layout(cr, pango_layout);
            }
            DialogInput::Toolbar(toolbar) => {
                // Render the embedded toolbar using the GTK toolbar
                // rasteriser. Background fill uses the toolbar's own bg
                // (or header_bg fallback) so the slot reads as chrome.
                super::toolbar::draw_toolbar(
                    cr,
                    pango_layout,
                    ix,
                    iy,
                    iw,
                    ih,
                    toolbar,
                    theme,
                    None,
                    None,
                );
                // Restore body font after the toolbar rasteriser may
                // have swapped it.
                pango_layout.set_font_description(saved_font.as_ref());
            }
        }
    }

    // Buttons render in the UI font.
    pango_layout.set_font_description(Some(ui_font_desc));

    let mut rects = Vec::with_capacity(dialog_layout.visible_buttons.len());
    for vis in &dialog_layout.visible_buttons {
        let btn = &dialog.buttons[vis.button_idx];
        let btn_bx = vis.bounds.x as f64;
        let btn_by = vis.bounds.y as f64;
        let btn_bw = vis.bounds.width as f64;
        let btn_bh = vis.bounds.height as f64;
        rects.push((btn_bx, btn_by, btn_bw, btn_bh));

        if btn.is_default {
            cr.set_source_rgb(sel.0, sel.1, sel.2);
            cr.rectangle(btn_bx, btn_by, btn_bw, btn_bh);
            cr.fill().ok();
        }

        let label = if dialog.vertical_buttons {
            let prefix = if btn.is_default { "▸ " } else { "  " };
            format!("{}{}", prefix, btn.label)
        } else {
            format!("  {}  ", btn.label)
        };
        cr.set_source_rgb(fg.0, fg.1, fg.2);
        pango_layout.set_text(&label);
        pango_layout.set_attributes(None);
        let (lw, lh) = pango_layout.pixel_size();
        let lw = lw as f64;
        let lh = lh as f64;
        let label_x = if dialog.vertical_buttons {
            btn_bx + 4.0
        } else {
            btn_bx + (btn_bw - lw) / 2.0
        };
        let label_y = btn_by + (btn_bh - lh) / 2.0;
        cr.move_to(label_x, label_y);
        super::painted_text::show_layout(cr, pango_layout);
    }

    // Restore the layout's font_description so subsequent paints in
    // the same frame use the editor font, not the UI font we left
    // active for the buttons (#247).
    pango_layout.set_font_description(saved_font.as_ref());

    rects
}
