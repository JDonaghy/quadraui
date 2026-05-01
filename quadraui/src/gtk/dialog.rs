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
use pangocairo::functions as pcfn;

use super::cairo_rgb;
use crate::primitives::dialog::{Dialog, DialogLayout};
use crate::theme::Theme;
use crate::types::StyledText;

fn flatten(text: &StyledText) -> String {
    text.spans.iter().map(|s| s.text.as_str()).collect()
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
        pcfn::show_layout(cr, pango_layout);
    }

    // Body + input render in the layout's saved (editor / mono) font.
    pango_layout.set_font_description(saved_font.as_ref());

    let body_b = dialog_layout.body_bounds;
    cr.set_source_rgb(fg.0, fg.1, fg.2);
    for (i, line) in flatten(&dialog.body).split('\n').enumerate() {
        let row_y = body_b.y as f64 + i as f64 * line_height;
        if row_y + line_height > body_b.y as f64 + body_b.height as f64 {
            break;
        }
        pango_layout.set_text(line);
        pango_layout.set_attributes(None);
        cr.move_to(body_b.x as f64, row_y);
        pcfn::show_layout(cr, pango_layout);
    }

    if let (Some(input_b), Some(input)) = (dialog_layout.input_bounds, dialog.input.as_ref()) {
        let ix = input_b.x as f64;
        let iy = input_b.y as f64;
        let iw = input_b.width as f64;
        let ih = input_b.height as f64;
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
        pcfn::show_layout(cr, pango_layout);
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
        pcfn::show_layout(cr, pango_layout);
    }

    // Restore the layout's font_description so subsequent paints in
    // the same frame use the editor font, not the UI font we left
    // active for the buttons (#247).
    pango_layout.set_font_description(saved_font.as_ref());

    rects
}
