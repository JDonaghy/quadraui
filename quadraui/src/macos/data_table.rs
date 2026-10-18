//! macOS rasteriser for [`crate::DataTable`].
//!
//! Painting uses the shared
//! [`crate::primitives::data_table::native_surface_paint::paint`] —
//! see that function's documentation for the full per-backend
//! divergence survey. [`draw_data_table`] is a thin wrapper over the
//! shared paint, using [`crate::macos::surface::CgSurface`] as the
//! `PaintSurface` adapter, mirroring
//! [`crate::win::data_table::draw_data_table`]'s equivalent approach.
//! [`mac_data_table_layout`] (pure geometry, no paint) handles layout.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use crate::primitives::data_table::DataTable;
use crate::primitives::layout_metrics::pixel_data_table_layout;
use crate::theme::Theme;
use crate::DataTableLayout;

/// Compute the layout the macOS rasteriser would produce for `table`
/// at `(x, y, w, h)` and `line_height`. Shares its column-measurement
/// path (and `font: &CTFont` passed straight through as `&dyn
/// TextMeasure` — `CTFont` implements it directly, see `macos::text`'s
/// module doc) with `gtk_data_table_layout` / `win_data_table_layout`
/// via [`pixel_data_table_layout`].
pub fn mac_data_table_layout(
    table: &DataTable,
    font: &CTFont,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    line_height: f64,
) -> DataTableLayout {
    let _ = (x, y); // hit_test consumes table-local coords; bounds stay
                    // relative to the table's origin.
    pixel_data_table_layout(table, w as f32, h as f32, line_height as f32, font)
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
    let mut surface = super::surface::CgSurface {
        ctx,
        font: Some(font),
    };
    let rect = crate::event::Rect::new(x as f32, y as f32, w as f32, h as f32);
    crate::primitives::data_table::native_surface_paint::paint(
        table,
        &mut surface,
        theme,
        rect,
        line_height as f32,
        hovered_idx,
    )
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::make_font;
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::data_table::{
        Column, ColumnAlign, ColumnWidth, DataRow, DataTable, DataTableHit, SortDirection,
    };
    use crate::primitives::scrollbar::scroll_rows;
    use crate::theme::Theme;
    use crate::types::{Decoration, StyledText, WidgetId};
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
            scroll_offset: 0.0,
            sort: Some((0, SortDirection::Ascending)),
            has_focus: true,
            show_scrollbar: false,
            min_total_width: None,
            h_scroll: 0.0,
            column_overrides: vec![],
            footer: None,
        }
    }

    fn paint_via_backend(
        table: &DataTable,
        hovered: Option<usize>,
    ) -> (BitmapSurface, DataTableLayout) {
        paint_via_backend_themed(table, hovered, Theme::default())
    }

    /// Like [`paint_via_backend`], but with an explicit theme. Tests use
    /// this with `selection_alpha: 1.0` to ensure exact pixel matching,
    /// since the selection/hover row tints are now real alpha blends.
    fn paint_via_backend_themed(
        table: &DataTable,
        hovered: Option<usize>,
        theme: Theme,
    ) -> (BitmapSurface, DataTableLayout) {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.set_theme(theme);
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
        // `selection_alpha: 1.0` makes the alpha blend opaque, so the
        // exact-colour assertion below pins the right colour without
        // needing to reason about compositing math.
        let table = sample_table();
        let theme = Theme {
            selection_alpha: 1.0,
            ..Theme::default()
        };
        let (surface, layout) = paint_via_backend_themed(&table, None, theme);
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

    /// Verify that the selected row's tint is translucent at the real
    /// default `selection_alpha` (`0.50`). Painting the tint over the
    /// surface's initial fully-transparent fill must read back darker
    /// than the pure `selection_bg` colour, proving the shared `paint`
    /// composites a translucent tint rather than replacing the pixel.
    #[test]
    fn selected_row_tint_is_translucent_at_the_real_default_alpha() {
        let table = sample_table();
        let theme = Theme::default();
        assert!(
            (0.0..1.0).contains(&theme.selection_alpha),
            "this test only proves something if the default alpha is translucent"
        );
        let (surface, layout) = paint_via_backend(&table, None);
        let (r, g, b) = probe_cell_bg(&surface, &layout, 0, 1);
        assert_ne!(
            (r, g, b),
            (
                theme.selection_bg.r,
                theme.selection_bg.g,
                theme.selection_bg.b
            ),
            "a translucent selection tint composited over a transparent-black \
             surface must read back darker than the pure selection colour"
        );
    }

    #[test]
    fn hover_tint_painted_when_hovered_idx_set() {
        let mut table = sample_table();
        table.selected_idx = None;
        let theme = Theme::default();
        let (surface, layout) = paint_via_backend_themed(&table, Some(2), theme);
        let (r, g, b) = probe_cell_bg(&surface, &layout, 0, 2);
        assert_ne!(
            (r, g, b),
            (0, 0, 0),
            "hovered row should paint a visible tint, not leave the transparent-black surface untouched",
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
            scroll_rows(table.scroll_offset),
            total,
        );
        assert!(matches!(hit, DataTableHit::Header { col: 0 }));

        // Body row 1 (selected) → Row { idx: 1 }.
        let hit = layout.hit_test(
            10.0,
            layout.header_height + layout.row_height * 1.5,
            scroll_rows(table.scroll_offset),
            total,
        );
        assert!(matches!(hit, DataTableHit::Row { idx: 1 }));
    }

    #[test]
    fn layout_returns_local_coords_when_area_offset() {
        // Cross-backend contract: `mac_data_table_layout` returns LOCAL
        // coordinates (origin 0, 0) regardless of where the table is
        // painted, matching the tree/form/list/palette family. Only
        // `draw_data_table`'s paint path adds x/y back in. Verify the
        // origin has no effect on the returned layout, then round-trip a
        // click through hit_test.
        let table = sample_table();
        let f = font();
        let layout_origin = mac_data_table_layout(&table, &f, 0.0, 0.0, W as f64, H as f64, 16.0);
        let layout_offset = mac_data_table_layout(&table, &f, 7.0, 13.0, W as f64, H as f64, 16.0);
        assert_eq!(
            layout_origin, layout_offset,
            "mac_data_table_layout must ignore (x, y) — bounds are local by construction",
        );

        let total = table.rows.len();

        // Header row center → Header { col: 0 }.
        let hit = layout_offset.hit_test(
            layout_offset.columns[0].x + layout_offset.columns[0].width * 0.5,
            layout_offset.header_height * 0.5,
            scroll_rows(table.scroll_offset),
            total,
        );
        assert!(matches!(hit, DataTableHit::Header { col: 0 }));

        // Body row 1 (selected) → Row { idx: 1 }.
        let hit = layout_offset.hit_test(
            10.0,
            layout_offset.header_height + layout_offset.row_height * 1.5,
            scroll_rows(table.scroll_offset),
            total,
        );
        assert!(matches!(hit, DataTableHit::Row { idx: 1 }));
    }
}
