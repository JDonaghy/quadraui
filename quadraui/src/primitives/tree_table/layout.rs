//! The two unit systems a [`TreeTable`] lays out in. Both resolve the
//! table through [`TreeTable::layout`] with the shared tree cell's
//! geometry (`crate::primitives::tree::tree_cell`) for the chevron and
//! the narrow-rect floor; they differ only in how a column title and a
//! chevron are measured.

use super::{TreeTable, TreeTableLayout};
use crate::primitives::data_table::ColumnMeasure;
use crate::primitives::layout_metrics::{pixel, TextMeasure};
use crate::primitives::tree::tree_cell::{self, TreeCellUnits};
use crate::types::TreeStyle;

/// The narrow-rect floor for `table` in `units`: the deepest indent
/// across every row (not just the visible ones, so which columns drop
/// does not change as the table scrolls), plus a chevron and one label
/// character.
fn floor(table: &TreeTable, style: &TreeStyle, units: TreeCellUnits) -> f32 {
    let max_indent = table.tree.iter().map(|n| n.indent).max().unwrap_or(0);
    tree_cell::floor_width(style, max_indent, units)
}

/// `table`'s layout on a cell grid `width` × `height` cells: one-cell
/// rows, header and scrollbar, column titles measured by character
/// count, and `h_scroll` rounded to the whole cell the cells paint at.
pub(crate) fn cell_layout(table: &TreeTable, width: f32, height: f32) -> TreeTableLayout {
    let style = TreeStyle::default();
    let units = TreeCellUnits::Cells;
    let mut layout = table.layout(
        width,
        height,
        1.0,
        1.0,
        1.0,
        |col| ColumnMeasure::new(col.title.chars().count() as f32),
        |node| tree_cell::chevron_extent(&style, node.indent, node.is_expanded, units),
        floor(table, &style, units),
    );
    // Hit-testing does no rounding of its own, so the layout carries the
    // exact offset the cells paint at.
    layout.table.h_scroll = table.table.h_scroll.round();
    layout
}

/// `table`'s layout on a pixel surface `w` × `h`, with column titles and
/// chevrons measured through `measure`.
pub(crate) fn pixel_layout(
    table: &TreeTable,
    w: f32,
    h: f32,
    line_height: f32,
    measure: &dyn TextMeasure,
) -> TreeTableLayout {
    let style = TreeStyle::default();
    let units = TreeCellUnits::Pixels {
        line_height,
        measure,
    };
    table.layout(
        w,
        h,
        line_height,
        (line_height * 1.2).round(),
        pixel::DATA_TABLE_SCROLLBAR_WIDTH,
        |col| ColumnMeasure::new(measure.width_of(&col.title)),
        |node| tree_cell::chevron_extent(&style, node.indent, node.is_expanded, units),
        floor(table, &style, units),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::data_table::{Column, ColumnAlign, ColumnWidth, DataRow, DataTable};
    use crate::primitives::tree_table::TreeTableNode;
    use crate::types::{StyledText, WidgetId};

    struct CharMeasure;

    impl TextMeasure for CharMeasure {
        fn width_of(&self, text: &str) -> f32 {
            text.chars().count() as f32 * 8.0
        }
    }

    fn deep_table(width: ColumnWidth) -> TreeTable {
        let table = DataTable {
            id: WidgetId::new("tt"),
            columns: vec![Column {
                title: "N".into(),
                width,
                align: ColumnAlign::Left,
            }],
            rows: (0..2)
                .map(|i| DataRow {
                    cells: vec![StyledText::plain(format!("r{i}"))],
                    decoration: Default::default(),
                })
                .collect(),
            selected_idx: None,
            scroll_offset: 0,
            sort: None,
            has_focus: true,
            show_scrollbar: false,
            min_total_width: None,
            h_scroll: 0.0,
            column_overrides: vec![],
            footer: None,
            bordered: false,
            striped: false,
            glyphs: None,
        };
        TreeTable::new(
            table,
            vec![
                TreeTableNode::new(vec![0], 0).with_expanded(true),
                TreeTableNode::new(vec![0, 0], 6).with_expanded(false),
            ],
        )
    }

    /// The pixel floor measures through the same tree cell geometry the
    /// chevrons lay out with, so the deepest row's chevron always fits
    /// inside column 0.
    #[test]
    fn the_pixel_floor_fits_the_deepest_chevron() {
        let tt = deep_table(ColumnWidth::Native(10.0));
        let layout = pixel_layout(&tt, 400.0, 100.0, 16.0, &CharMeasure);
        let col0 = layout.table.columns[0].width;
        let deep = layout.chevrons.iter().find(|(i, _)| *i == 1).unwrap().1;
        // Indent 6 at 14 px per level.
        assert_eq!(deep.x, 84.0);
        assert!(deep.x + deep.width < col0, "{deep:?} vs column 0 {col0}");
    }

    #[test]
    fn the_cell_floor_is_the_deepest_indent_plus_chevron_and_one_char() {
        let tt = deep_table(ColumnWidth::Flex(1.0));
        // Indent 6 × 2 cells + a one-character chevron + one label char.
        let layout = cell_layout(&tt, 4.0, 10.0);
        assert_eq!(layout.table.columns[0].width, 14.0);
    }

    #[test]
    fn cell_layout_rounds_h_scroll_to_the_painted_cell() {
        let mut tt = deep_table(ColumnWidth::Flex(1.0));
        tt.table.min_total_width = Some(60.0);
        tt.table.h_scroll = 2.6;
        assert_eq!(cell_layout(&tt, 20.0, 10.0).table.h_scroll, 3.0);
    }
}
