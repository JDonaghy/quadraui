//! macOS rasteriser for [`crate::DataTable`].
//!
//! Mirrors [`crate::gtk::data_table::draw_data_table`]: header row at
//! `(line_height * 1.2)` with bold-ish accent (rendered as the default
//! font for now — bold lands with the unified text-attribute pass),
//! sort glyphs (`▲` / `▼`) suffixed onto the active sort column's
//! title, body rows at `line_height` pitch with hover/select tints,
//! column separators, and vertical + horizontal scrollbars when
//! configured.
//!
//! ## Scope omissions (follow-up)
//!
//! - **Bold header text** — needs `CTFontCreateCopyWithSymbolicTraits`
//!   bold variant, deferred with the text-attribute pass.
//! - **Per-row selection alpha blending** — GTK uses `selection_alpha`
//!   for translucent selection; macOS paints a solid `selection_bg`
//!   pixel today. Visual parity tracked separately.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::primitives::data_table::{
    ColumnAlign, ColumnMeasure, DataTable, DataTableLayout, SortDirection,
};
use crate::theme::Theme;
use crate::types::{Color, Decoration};

const SCROLLBAR_WIDTH: f32 = 8.0;

/// Compute the layout the macOS rasteriser would produce for `table`
/// at `(x, y, w, h)` and `line_height`.
pub fn mac_data_table_layout(
    table: &DataTable,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    line_height: f64,
) -> DataTableLayout {
    let header_height = (line_height * 1.2).round();
    let measure = |col: &crate::primitives::data_table::Column| -> ColumnMeasure {
        let (tw, _) = measure_text(font, &col.title);
        ColumnMeasure::new(tw.max(0.0) as f32)
    };
    let _ = (x, y); // hit_test consumes table-local coords; bounds stay
                    // relative to the table's origin.
    table.layout(
        w as f32,
        h as f32,
        line_height as f32,
        header_height as f32,
        SCROLLBAR_WIDTH,
        measure,
    )
}

/// Draw `table` into `(x, y, w, h)` on `ctx`. Returns the same layout
/// `mac_data_table_layout` would produce.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_data_table(
    ctx: CGContextRef,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    table: &DataTable,
    theme: &Theme,
    line_height: f64,
    hovered_idx: Option<usize>,
) -> DataTableLayout {
    let layout = mac_data_table_layout(table, font, x, y, w, h, line_height);
    if w <= 0.0 || h <= 0.0 {
        return layout;
    }

    CGContextSaveGState(ctx);
    CGContextClipToRect(ctx, CGRect::new_xywh(x, y, w, h));

    // Header background.
    fill_rect(ctx, x, y, w, layout.header_height as f64, theme.tab_bar_bg);

    let h_off = table.h_scroll as f64;
    let header_height = layout.header_height as f64;

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
        let (text_w, _) = measure_text(font, &title);
        let col_x = x + rc.x as f64 - h_off;
        let col_w = rc.width as f64;

        let text_x = match col.align {
            ColumnAlign::Left => col_x,
            ColumnAlign::Center => col_x + (col_w - text_w) / 2.0,
            ColumnAlign::Right => col_x + col_w - text_w,
        };
        draw_text(
            ctx,
            font,
            &title,
            text_x,
            y + (header_height - measure_text(font, &title).1) / 2.0,
            color_to_cg(theme.foreground),
        );
    }

    // Header column separators.
    for (col_idx, rc) in layout.columns.iter().enumerate() {
        if col_idx + 1 >= layout.columns.len() {
            break;
        }
        let sep_x = x + (rc.x + rc.width) as f64 - h_off;
        fill_rect(ctx, sep_x - 0.5, y, 1.0, header_height, theme.separator);
    }

    // Body rows.
    let body_y = y + header_height;
    let visible = layout
        .visible_rows
        .min(table.rows.len().saturating_sub(table.scroll_offset));

    for row_idx in 0..visible {
        let abs_idx = table.scroll_offset + row_idx;
        let row = &table.rows[abs_idx];
        let row_y = body_y + row_idx as f64 * line_height;
        let is_selected = table.selected_idx == Some(abs_idx);
        let is_hovered = hovered_idx == Some(abs_idx) && !is_selected;

        if is_selected {
            fill_rect(ctx, x, row_y, w, line_height, theme.selection_bg);
        } else if is_hovered {
            // Half-mix hover tint: blend tab_bar_bg into row bg.
            fill_rect(ctx, x, row_y, w, line_height, theme.tab_bar_bg);
        }

        for (col_idx, rc) in layout.columns.iter().enumerate() {
            let styled = match row.cells.get(col_idx) {
                Some(c) if !c.spans.is_empty() => c,
                _ => continue,
            };
            if rc.width <= 0.0 {
                continue;
            }
            let col_w = rc.width as f64;
            let col_x = x + rc.x as f64 - h_off;
            let is_muted = matches!(row.decoration, Decoration::Muted);

            let full_text: String = styled.spans.iter().map(|s| s.text.as_str()).collect();
            let (text_w, text_h) = measure_text(font, &full_text);
            let align = table
                .columns
                .get(col_idx)
                .map(|c| c.align)
                .unwrap_or(ColumnAlign::Left);
            let text_x = match align {
                ColumnAlign::Left => col_x,
                ColumnAlign::Center => col_x + (col_w - text_w) / 2.0,
                ColumnAlign::Right => col_x + col_w - text_w,
            };
            let fg = if is_muted {
                theme.muted_fg
            } else {
                theme.foreground
            };
            draw_text(
                ctx,
                font,
                &full_text,
                text_x,
                row_y + (line_height - text_h) / 2.0,
                color_to_cg(fg),
            );
        }
    }

    // Vertical scrollbar — simple thumb on track.
    if table.show_scrollbar
        && table.rows.len() > layout.visible_rows
        && layout.scrollbar_width > 0.0
    {
        let sb_x = x + w - layout.scrollbar_width as f64;
        let track_y = body_y;
        let track_h = (h - header_height - layout.h_scrollbar_height as f64).max(0.0);
        fill_rect(
            ctx,
            sb_x,
            track_y,
            layout.scrollbar_width as f64,
            track_h,
            theme.tab_bar_bg,
        );
        // Thumb proportional to visible / total.
        let total = table.rows.len() as f64;
        let visible = layout.visible_rows.max(1) as f64;
        let thumb_h = (track_h * (visible / total)).max(4.0);
        let thumb_y = track_y
            + (track_h - thumb_h) * (table.scroll_offset as f64 / (total - visible).max(1.0));
        fill_rect(
            ctx,
            sb_x,
            thumb_y,
            layout.scrollbar_width as f64,
            thumb_h,
            theme.scrollbar_thumb,
        );
    }

    // Horizontal scrollbar — same shape.
    if layout.h_scrollbar_height > 0.0 && layout.content_width > 0.0 {
        let hsb_y = y + h - layout.h_scrollbar_height as f64;
        let track_w = (w - layout.scrollbar_width as f64).max(1.0);
        fill_rect(
            ctx,
            x,
            hsb_y,
            track_w,
            layout.h_scrollbar_height as f64,
            theme.tab_bar_bg,
        );
        let visible = track_w as f32;
        let total = layout.content_width;
        let thumb_w = (track_w * (visible as f64 / total as f64)).max(4.0);
        let thumb_x =
            x + (track_w - thumb_w) * (table.h_scroll as f64 / (total - visible).max(1.0) as f64);
        fill_rect(
            ctx,
            thumb_x,
            hsb_y,
            thumb_w,
            layout.h_scrollbar_height as f64,
            theme.scrollbar_thumb,
        );
    }

    CGContextRestoreGState(ctx);
    layout
}

fn color_to_cg(c: Color) -> (f64, f64, f64, f64) {
    (
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    )
}

unsafe fn fill_rect(ctx: CGContextRef, x: f64, y: f64, w: f64, h: f64, c: Color) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r, g, b, a);
    CGContextFillRect(ctx, CGRect::new_xywh(x, y, w, h));
}

trait CGRectExt {
    fn new_xywh(x: f64, y: f64, w: f64, h: f64) -> Self;
}
impl CGRectExt for CGRect {
    fn new_xywh(x: f64, y: f64, w: f64, h: f64) -> Self {
        use core_graphics::geometry::{CGPoint, CGSize};
        CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h))
    }
}

extern "C" {
    fn CGContextSaveGState(c: CGContextRef);
    fn CGContextRestoreGState(c: CGContextRef);
    fn CGContextClipToRect(c: CGContextRef, rect: CGRect);
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::data_table::{Column, ColumnWidth, DataRow, DataTable, DataTableHit};
    use crate::theme::Theme;
    use crate::types::{StyledText, WidgetId};
    use crate::Backend;

    const W: u32 = 320;
    const H: u32 = 200;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn sample_table() -> DataTable {
        DataTable {
            id: WidgetId::new("dt"),
            columns: vec![
                Column {
                    title: "Name".into(),
                    width: ColumnWidth::Flex(2.0),
                    align: ColumnAlign::Left,
                },
                Column {
                    title: "Value".into(),
                    width: ColumnWidth::Flex(1.0),
                    align: ColumnAlign::Right,
                },
            ],
            rows: vec![
                DataRow {
                    cells: vec![StyledText::plain("alpha"), StyledText::plain("1")],
                    decoration: Decoration::Normal,
                },
                DataRow {
                    cells: vec![StyledText::plain("beta"), StyledText::plain("2")],
                    decoration: Decoration::Normal,
                },
                DataRow {
                    cells: vec![StyledText::plain("gamma"), StyledText::plain("3")],
                    decoration: Decoration::Normal,
                },
            ],
            selected_idx: Some(1),
            scroll_offset: 0,
            sort: Some((0, SortDirection::Ascending)),
            has_focus: true,
            show_scrollbar: false,
            min_total_width: None,
            h_scroll: 0.0,
            column_overrides: vec![],
        }
    }

    fn paint_via_backend(
        table: &DataTable,
        hovered: Option<usize>,
    ) -> (BitmapSurface, DataTableLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b.draw_data_table(QRect::new(0.0, 0.0, W as f32, H as f32), table, hovered);
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    /// Probe a known glyph-free spot inside `col_idx`'s body area at
    /// `body_row` (0-based) — column-internal padding region between
    /// glyph runs and the column's right edge.
    fn probe_cell_bg(
        surface: &BitmapSurface,
        layout: &DataTableLayout,
        col_idx: usize,
        body_row: usize,
    ) -> (u8, u8, u8) {
        let col = &layout.columns[col_idx];
        // Right 30% of the column is typically glyph-free for plain
        // short labels like "alpha" / "1".
        let px = (col.x + col.width * 0.85) as u32;
        let py = (layout.header_height + layout.row_height * (body_row as f32 + 0.5)) as u32;
        let (r, g, b, _) = surface.pixel(px, py);
        (r, g, b)
    }

    #[test]
    fn header_strip_paints_tab_bar_bg() {
        let table = sample_table();
        let (surface, layout) = paint_via_backend(&table, None);
        let theme = Theme::default();
        // Probe header strip in the right-third of column 0 (after
        // "Name ▲" glyphs, before the column-0/1 separator).
        let col0 = &layout.columns[0];
        let px = (col0.x + col0.width * 0.85) as u32;
        let py = 1_u32; // top scanline — above glyph cap.
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
        );
    }

    #[test]
    fn selected_row_paints_selection_bg() {
        let table = sample_table();
        let (surface, layout) = paint_via_backend(&table, None);
        let theme = Theme::default();
        // selected_idx = 1 → second body row. Probe glyph-free area of
        // col 0 (after "beta").
        let (r, g, b) = probe_cell_bg(&surface, &layout, 0, 1);
        assert_eq!(
            (r, g, b),
            (
                theme.selection_bg.r,
                theme.selection_bg.g,
                theme.selection_bg.b
            ),
        );
    }

    #[test]
    fn hover_tint_painted_when_hovered_idx_set() {
        let mut table = sample_table();
        table.selected_idx = None;
        let (surface, layout) = paint_via_backend(&table, Some(2));
        let theme = Theme::default();
        let (r, g, b) = probe_cell_bg(&surface, &layout, 0, 2);
        assert_eq!(
            (r, g, b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
        );
    }

    #[test]
    fn hit_test_resolves_header_vs_row() {
        let table = sample_table();
        let (_surface, layout) = paint_via_backend(&table, None);
        let total = table.rows.len();

        // Header row center → Header { col: 0 }.
        let hit = layout.hit_test(
            layout.columns[0].x + layout.columns[0].width * 0.5,
            layout.header_height * 0.5,
            table.scroll_offset,
            total,
        );
        assert!(matches!(hit, DataTableHit::Header { col: 0 }));

        // Body row 1 (selected) → Row { idx: 1 }.
        let hit = layout.hit_test(
            10.0,
            layout.header_height + layout.row_height * 1.5,
            table.scroll_offset,
            total,
        );
        assert!(matches!(hit, DataTableHit::Row { idx: 1 }));
    }
}
