#![cfg(feature = "tui")]
//! `DataTable::bordered` on the TUI rasteriser: the rounded frame, the
//! 1-cell inset of the header/rows/footer/scrollbars, hit-testing that
//! matches the inset paint, and a focus ring over the same `area` that
//! never overwrites table content.

use quadraui::tui::{draw_data_table, draw_focus_ring, ratatui_color};
use quadraui::{
    Column, ColumnAlign, ColumnWidth, DataRow, DataTable, DataTableHit, Decoration, SortDirection,
    StyledText, Theme, WidgetId,
};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

fn row(cells: [&str; 3]) -> DataRow {
    DataRow {
        cells: cells.iter().map(|c| StyledText::plain(*c)).collect(),
        decoration: Decoration::Normal,
    }
}

fn col(title: &str, width: ColumnWidth, align: ColumnAlign) -> Column {
    Column {
        title: title.into(),
        width,
        align,
    }
}

fn make_table() -> DataTable {
    DataTable {
        id: WidgetId::new("test"),
        columns: vec![
            col("Name", ColumnWidth::Flex(2.0), ColumnAlign::Left),
            col("Status", ColumnWidth::Flex(1.0), ColumnAlign::Left),
            col("Age", ColumnWidth::FixedCells(5.0), ColumnAlign::Right),
        ],
        rows: vec![
            row(["pod-abc", "Running", "3d"]),
            row(["pod-xyz", "Pending", "1h"]),
        ],
        selected_idx: Some(0),
        scroll_offset: 0,
        sort: Some((0, SortDirection::Ascending)),
        has_focus: true,
        show_scrollbar: false,
        min_total_width: None,
        h_scroll: 0.0,
        column_overrides: Vec::new(),
        footer: None,
        bordered: true,
    }
}

fn paint(table: &DataTable, area: Rect) -> (Buffer, quadraui::DataTableLayout) {
    let mut buf = Buffer::empty(area);
    let layout = draw_data_table(&mut buf, area, table, &Theme::default(), None);
    (buf, layout)
}

/// The header and row 0 paint one cell inset from `area`'s top-left; the
/// corners and the left edge are border glyphs.
#[test]
fn bordered_insets_header_and_first_row_away_from_the_top_left_corner() {
    let (buf, _) = paint(&make_table(), Rect::new(0, 0, 40, 6));
    assert_eq!(buf[(0, 0)].symbol(), "╭");
    assert_eq!(buf[(39, 0)].symbol(), "╮");
    assert_eq!(buf[(0, 5)].symbol(), "╰");
    assert_eq!(buf[(39, 5)].symbol(), "╯");
    assert_eq!(buf[(0, 1)].symbol(), "│");
    assert_eq!(buf[(1, 1)].symbol(), "N");
    assert_eq!(buf[(1, 2)].symbol(), "p");
}

/// A focus ring painted over the same `area` only recolours the frame,
/// never the inset header/row content.
#[test]
fn focus_ring_over_bordered_table_never_overwrites_content() {
    let area = Rect::new(0, 0, 40, 6);
    let (mut buf, _) = paint(&make_table(), area);
    draw_focus_ring(&mut buf, area, &Theme::default());
    assert_eq!(buf[(0, 0)].symbol(), "╭");
    assert_eq!(buf[(1, 1)].symbol(), "N", "ring overwrote the header");
    assert_eq!(buf[(1, 2)].symbol(), "p", "ring overwrote row content");
}

/// Hit-testing matches the inset paint; the frame itself is never a
/// header or row.
#[test]
fn bordered_hit_test_matches_inset_paint() {
    let table = make_table();
    let (_, layout) = paint(&table, Rect::new(0, 0, 40, 6));
    let n = table.rows.len();
    assert!(matches!(
        layout.hit_test(2.0, 1.0, 0, n),
        DataTableHit::Header { col: 0 }
    ));
    assert_eq!(
        layout.hit_test(2.0, 2.0, 0, n),
        DataTableHit::Row { idx: 0 }
    );
    assert_eq!(layout.hit_test(0.0, 1.0, 0, n), DataTableHit::Empty);
    assert_eq!(layout.hit_test(2.0, 0.0, 0, n), DataTableHit::Empty);
}

/// A 1-cell-wide bordered area with a vertical scrollbar must not
/// underflow computing the scrollbar column.
#[test]
fn bordered_one_cell_wide_area_with_scrollbar_does_not_panic() {
    let mut table = make_table();
    table.rows = (0..50)
        .map(|r| row([&format!("pod-{r}"), "Running", &format!("{r}d")]))
        .collect();
    table.show_scrollbar = true;
    paint(&table, Rect::new(0, 0, 1, 10));
}

/// A 1-cell-tall bordered area with a horizontal scrollbar and footer
/// must not underflow computing either row.
#[test]
fn bordered_one_cell_tall_area_with_h_scrollbar_and_footer_does_not_panic() {
    let mut table = make_table();
    table.min_total_width = Some(500.0);
    table.footer = Some(row(["TOTAL", "", ""]));
    paint(&table, Rect::new(0, 0, 40, 1));
}

/// At the minimum bordered height that fits a footer (top border +
/// divider + footer row), the footer divider leaves the top border's
/// glyphs and colours intact.
#[test]
fn bordered_footer_divider_never_overwrites_the_top_border() {
    let mut table = make_table();
    table.footer = Some(row(["TOTAL", "", ""]));
    let theme = Theme::default();
    let area = Rect::new(0, 0, 40, 3);
    let (buf, _) = paint(&table, area);
    assert_eq!(buf[(0, 0)].symbol(), "╭");
    let border = (
        ratatui_color(theme.border_fg),
        ratatui_color(theme.background),
    );
    for x in 1..area.width - 1 {
        let cell = &buf[(x, 0)];
        assert_eq!(cell.symbol(), "─", "top border lost at x={x}");
        assert_eq!((cell.fg, cell.bg), border, "top border recoloured at x={x}");
    }
}
