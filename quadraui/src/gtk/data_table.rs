//! GTK rasteriser for [`crate::DataTable`].
//!
//! Renders column headers with Pango-measured text and sort indicators,
//! then body rows with per-cell text aligned within resolved column
//! bounds. Uses Pango for measurement and Cairo for painting.

use gtk4::cairo::Context;
use gtk4::pango;
use pangocairo::functions as pcfn;

use super::{cairo_rgb, set_source};
use crate::primitives::data_table::{
    ColumnAlign, ColumnMeasure, DataTable, DataTableLayout, SortDirection,
};
use crate::theme::Theme;

/// Draw a `DataTable` onto `cr`. Returns the layout used for painting.
#[allow(clippy::too_many_arguments)]
pub fn draw_data_table(
    cr: &Context,
    pango_layout: &pango::Layout,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    table: &DataTable,
    theme: &Theme,
    line_height: f64,
) -> DataTableLayout {
    let header_height = (line_height * 1.2).round();
    let measure = |col: &crate::primitives::data_table::Column| -> ColumnMeasure {
        pango_layout.set_text(&col.title);
        pango_layout.set_attributes(None);
        let (w, _) = pango_layout.pixel_size();
        ColumnMeasure::new(w.max(0) as f32)
    };
    let layout = table.layout(
        width as f32,
        height as f32,
        line_height as f32,
        header_height as f32,
        8.0,
        measure,
    );

    if width <= 0.0 || height <= 0.0 {
        return layout;
    }

    cr.save().ok();
    cr.rectangle(x, y, width, height);
    cr.clip();

    // ── Header background ─────────────────────────────────────────────
    let (hbr, hbg, hbb) = cairo_rgb(theme.tab_bar_bg);
    cr.set_source_rgb(hbr, hbg, hbb);
    cr.rectangle(x, y, width, header_height);
    cr.fill().ok();

    // ── Header text ───────────────────────────────────────────────────
    set_source(cr, theme.foreground);
    let bold_attrs = pango::AttrList::new();
    bold_attrs.insert(pango::AttrInt::new_weight(pango::Weight::Bold));

    for (col_idx, rc) in layout.columns.iter().enumerate() {
        if col_idx >= table.columns.len() || rc.width <= 0.0 {
            break;
        }
        let col = &table.columns[col_idx];
        let sort_suffix = match &table.sort {
            Some((si, dir)) if *si == col_idx => match dir {
                SortDirection::Ascending => " ▲",
                SortDirection::Descending => " ▼",
            },
            _ => "",
        };
        let title = format!("{}{}", col.title, sort_suffix);
        pango_layout.set_text(&title);
        pango_layout.set_attributes(Some(&bold_attrs));
        let (text_w, _) = pango_layout.pixel_size();

        let col_x = x + rc.x as f64;
        let col_w = rc.width as f64;

        cr.save().ok();
        cr.rectangle(col_x, y, col_w, header_height);
        cr.clip();

        let text_x = match col.align {
            ColumnAlign::Left => col_x,
            ColumnAlign::Center => col_x + (col_w - text_w as f64) / 2.0,
            ColumnAlign::Right => col_x + col_w - text_w as f64,
        };
        cr.move_to(text_x, y);
        pcfn::show_layout(cr, pango_layout);
        cr.restore().ok();
    }
    pango_layout.set_attributes(None);

    // ── Body rows ─────────────────────────────────────────────────────
    let body_y = y + header_height;
    let visible = layout
        .visible_rows
        .min(table.rows.len().saturating_sub(table.scroll_offset));

    for row_idx in 0..visible {
        let abs_idx = table.scroll_offset + row_idx;
        let row = &table.rows[abs_idx];
        let row_y = body_y + row_idx as f64 * line_height;
        let is_selected = table.selected_idx == Some(abs_idx);

        if is_selected {
            let (sr, sg, sb) = cairo_rgb(theme.selection_bg);
            cr.set_source_rgba(sr, sg, sb, theme.selection_alpha as f64);
            cr.rectangle(x, row_y, width, line_height);
            cr.fill().ok();
        }

        for (col_idx, rc) in layout.columns.iter().enumerate() {
            let cell_text: String = row
                .cells
                .get(col_idx)
                .map(|c| c.spans.iter().map(|s| s.text.as_str()).collect())
                .unwrap_or_default();
            let text = cell_text.as_str();
            if text.is_empty() || rc.width <= 0.0 {
                continue;
            }

            let col_w = rc.width as f64;
            let col_x = x + rc.x as f64;

            cr.save().ok();
            cr.rectangle(col_x, row_y, col_w, line_height);
            cr.clip();

            pango_layout.set_text(text);
            pango_layout.set_attributes(None);
            let (text_w, _) = pango_layout.pixel_size();

            let align = table
                .columns
                .get(col_idx)
                .map(|c| c.align)
                .unwrap_or(ColumnAlign::Left);
            let text_x = match align {
                ColumnAlign::Left => col_x,
                ColumnAlign::Center => col_x + (col_w - text_w as f64) / 2.0,
                ColumnAlign::Right => col_x + col_w - text_w as f64,
            };

            if row.decoration == crate::types::Decoration::Muted {
                set_source(cr, theme.muted_fg);
            } else {
                set_source(cr, theme.foreground);
            }
            cr.move_to(text_x, row_y);
            pcfn::show_layout(cr, pango_layout);
            cr.restore().ok();
        }
    }

    // ── Scrollbar ──────────────────────────────────────────────────────
    if table.show_scrollbar
        && table.rows.len() > layout.visible_rows
        && layout.scrollbar_width > 0.0
    {
        let sb_x = x + width - layout.scrollbar_width as f64;
        let sb_track = crate::event::Rect::new(
            sb_x as f32,
            (y + header_height) as f32,
            layout.scrollbar_width,
            (height - header_height).max(0.0) as f32,
        );
        let sb = crate::primitives::scrollbar::Scrollbar::vertical(
            table.id.clone(),
            sb_track,
            table.scroll_offset as f32,
            table.rows.len() as f32,
            layout.visible_rows as f32,
            line_height as f32,
        );
        super::draw_scrollbar(cr, &sb, theme);
    }

    cr.restore().ok();
    layout
}

/// Compute layout without painting (for hit-test from handle()).
pub fn gtk_data_table_layout(
    pango_layout: &pango::Layout,
    table: &DataTable,
    width: f64,
    height: f64,
    line_height: f64,
) -> DataTableLayout {
    let header_height = (line_height * 1.2).round();
    let measure = |col: &crate::primitives::data_table::Column| -> ColumnMeasure {
        pango_layout.set_text(&col.title);
        pango_layout.set_attributes(None);
        let (w, _) = pango_layout.pixel_size();
        ColumnMeasure::new(w.max(0) as f32)
    };
    table.layout(
        width as f32,
        height as f32,
        line_height as f32,
        header_height as f32,
        8.0,
        measure,
    )
}
