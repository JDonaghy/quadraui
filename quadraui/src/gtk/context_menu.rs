//! GTK rasteriser for [`crate::ContextMenu`].
//!
//! Cairo + Pango equivalent of `quadraui::tui::draw_context_menu`.
//! Draws a rectangle (background fill + 1 px stroke border), then
//! per-item rows (selection bg for the focused item, separator as a
//! thin horizontal line, optional right-aligned shortcut text).
//!
//! Returns per-clickable-item hit rectangles `(x, y, w, h, WidgetId)`
//! so the caller's click handler can resolve mouse events without
//! re-running layout.

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use super::cairo_rgb;
use crate::primitives::context_menu::{ContextMenu, ContextMenuLayout};
use crate::theme::Theme;
use crate::types::WidgetId;

/// Draw a [`ContextMenu`] popup. Returns the per-clickable-item hit
/// rectangles in target-surface pixels.
#[allow(clippy::too_many_arguments)]
pub fn draw_context_menu(
    cr: &Context,
    layout: &pango::Layout,
    menu: &ContextMenu,
    menu_layout: &ContextMenuLayout,
    line_height: f64,
    theme: &Theme,
) -> Vec<(f64, f64, f64, f64, WidgetId)> {
    let _ = line_height;
    let bounds = menu_layout.bounds;
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return Vec::new();
    }

    let bx = bounds.x as f64;
    let by = bounds.y as f64;
    let bw = bounds.width as f64;
    let bh = bounds.height as f64;

    let bg = menu
        .bg
        .map(cairo_rgb)
        .unwrap_or_else(|| cairo_rgb(theme.hover_bg));
    let border = cairo_rgb(theme.hover_border);
    let fg = cairo_rgb(theme.foreground);
    let sel = cairo_rgb(theme.selected_bg);
    let dim = cairo_rgb(theme.muted_fg);

    cr.set_source_rgb(bg.0, bg.1, bg.2);
    cr.rectangle(bx, by, bw, bh);
    cr.fill().ok();

    cr.set_source_rgb(border.0, border.1, border.2);
    cr.set_line_width(1.0);
    cr.rectangle(bx, by, bw, bh);
    cr.stroke().ok();

    let mut rects: Vec<(f64, f64, f64, f64, WidgetId)> = Vec::new();

    for vis in &menu_layout.visible_items {
        let item = &menu.items[vis.item_idx];
        let row_x = vis.bounds.x as f64;
        let row_y = vis.bounds.y as f64;
        let row_w = vis.bounds.width as f64;
        let row_h = vis.bounds.height as f64;

        if vis.is_separator {
            cr.set_source_rgb(dim.0, dim.1, dim.2);
            cr.set_line_width(0.5);
            let sep_y = row_y + row_h * 0.5;
            cr.move_to(row_x + 4.0, sep_y);
            cr.line_to(row_x + row_w - 4.0, sep_y);
            cr.stroke().ok();
            continue;
        }

        let is_selected = vis.item_idx == menu.selected_idx && vis.clickable;
        if is_selected {
            cr.set_source_rgb(sel.0, sel.1, sel.2);
            cr.rectangle(row_x + 1.0, row_y, row_w - 2.0, row_h);
            cr.fill().ok();
        }

        let label_text: String = item.label.spans.iter().map(|s| s.text.as_str()).collect();
        let label_fg = if vis.clickable { fg } else { dim };
        cr.set_source_rgb(label_fg.0, label_fg.1, label_fg.2);
        layout.set_text(&label_text);
        layout.set_attributes(None);
        let (_, lh) = layout.pixel_size();
        let text_y = row_y + (row_h - lh as f64) * 0.5;
        cr.move_to(row_x + 8.0, text_y);
        pcfn::show_layout(cr, layout);

        if let Some(ref det) = item.detail {
            let det_text: String = det.spans.iter().map(|s| s.text.as_str()).collect();
            if !det_text.is_empty() {
                layout.set_text(&det_text);
                let (sw, _) = layout.pixel_size();
                cr.set_source_rgb(dim.0, dim.1, dim.2);
                cr.move_to(row_x + row_w - sw as f64 - 8.0, text_y);
                pcfn::show_layout(cr, layout);
            }
        }

        if vis.clickable {
            if let Some(ref id) = item.id {
                rects.push((row_x, row_y, row_w, row_h, id.clone()));
            }
        }
    }
    rects
}
