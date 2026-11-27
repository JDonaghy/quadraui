//! Cell-grid paint for [`TreeTable`]: `DataTable`'s own cell rasteriser
//! (header, body, scrollbars, footer) with column 0 of every row painted
//! as the TUI tree cell followed by its label.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{TreeTable, TreeTableLayout, TreeTableNode};
use crate::primitives::data_table::paint_cells::{
    draw_cell, draw_data_table_body, draw_row, resolve_row_colors, RowPaintCtx,
};
use crate::primitives::data_table::DataTablePaintState;
use crate::theme::Theme;
use crate::types::TreeStyle;

/// Resolve the layout [`paint_tree_table_cells`] paints `table` at, without
/// touching a buffer: the cell-grid layout, so a layout-on-demand
/// hit-test agrees with what was painted.
#[cfg(test)]
pub(crate) fn cell_tree_table_layout(area: Rect, table: &TreeTable) -> TreeTableLayout {
    super::layout::cell_layout(table, area.width as f32, area.height as f32, None)
}

/// Draw a `TreeTable` into `area` with `hovered_idx` tinted, returning
/// the layout it painted at so callers hit-test at the same coordinates.
#[cfg(test)]
pub(crate) fn paint_tree_table_cells(
    buf: &mut Buffer,
    area: Rect,
    table: &TreeTable,
    theme: &Theme,
    hovered_idx: Option<usize>,
    nerd_fonts_enabled: bool,
) -> TreeTableLayout {
    let state = DataTablePaintState::default().with_hovered_idx(hovered_idx);
    paint_laid_out(
        buf,
        area,
        table,
        theme,
        &state,
        nerd_fonts_enabled,
        cell_tree_table_layout(area, table),
    )
}

/// Paint `table` at an already-resolved `layout` with `state`'s hover,
/// press and stripes: exactly what `DataTable` paints, except column 0 of
/// every row with a `table.tree` entry carries indent + chevron + icon
/// ahead of its label. Rows past `table.tree`'s length paint as plain
/// `DataTable` rows.
pub(super) fn paint_laid_out(
    buf: &mut Buffer,
    area: Rect,
    table: &TreeTable,
    theme: &Theme,
    state: &DataTablePaintState,
    nerd_fonts_enabled: bool,
    layout: TreeTableLayout,
) -> TreeTableLayout {
    let style = TreeStyle::default();
    let dt_layout = draw_data_table_body(
        buf,
        area,
        &table.table,
        theme,
        state,
        None,
        nerd_fonts_enabled,
        layout.table,
        |buf, y, ctx, idx, hovered, striped| match table.tree.get(idx) {
            Some(node) => draw_tree_table_row(
                buf,
                y,
                ctx,
                idx,
                node,
                (hovered, striped),
                &style,
                nerd_fonts_enabled,
            ),
            None => draw_row(buf, y, ctx, idx, hovered, striped, None),
        },
    );
    TreeTableLayout {
        table: dt_layout,
        chevrons: layout.chevrons,
    }
}

/// Paint one body row: `DataTable`'s selection, hover and stripe colors
/// and every column but 0 as `DataTable` paints it, with column 0 painted
/// as the tree cell followed by its label through the same cell painter
/// every other column uses.
#[allow(clippy::too_many_arguments)]
fn draw_tree_table_row(
    buf: &mut Buffer,
    y: u16,
    ctx: &RowPaintCtx<'_>,
    abs_idx: usize,
    node: &TreeTableNode,
    (hovered_idx, striped): (Option<usize>, bool),
    style: &TreeStyle,
    nerd_fonts_enabled: bool,
) {
    let (row_fg, row_bg) = resolve_row_colors(ctx, abs_idx, hovered_idx, striped);
    let mut hook = |buf: &mut Buffer, col_idx: usize, cell_rect: Rect| -> bool {
        if col_idx != 0 {
            return false;
        }
        let (_, text_start) = crate::tui::paint_tree_cell(
            buf,
            cell_rect,
            style,
            node.indent,
            node.is_expanded,
            node.icon.as_ref(),
            nerd_fonts_enabled,
            (row_fg, row_bg),
        );
        let text_start = (text_start as u16).min(cell_rect.width);
        draw_cell(
            buf,
            ctx,
            &ctx.table.rows[abs_idx],
            0,
            (
                cell_rect.x as i32 + text_start as i32,
                cell_rect.width - text_start,
            ),
            y,
            (row_fg, row_bg),
        );
        true
    };
    draw_row(buf, y, ctx, abs_idx, hovered_idx, striped, Some(&mut hook));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::data_table::DataTableHit;
    use crate::primitives::data_table::{Column, ColumnAlign, ColumnWidth, DataRow, DataTable};
    use crate::primitives::tree_table::TreeTableHit;
    use crate::types::{Decoration, StyledText, WidgetId};

    fn make_tree_table() -> TreeTable {
        TreeTable::new(
            DataTable {
                id: WidgetId::new("files"),
                columns: vec![
                    Column {
                        title: "Name".into(),
                        width: ColumnWidth::Flex(2.0),
                        align: ColumnAlign::Left,
                    },
                    Column {
                        title: "Size".into(),
                        width: ColumnWidth::FixedCells(6.0),
                        align: ColumnAlign::Right,
                    },
                ],
                rows: vec![
                    DataRow {
                        cells: vec![StyledText::plain("src"), StyledText::plain("")],
                        decoration: Decoration::Normal,
                    },
                    DataRow {
                        cells: vec![StyledText::plain("main.rs"), StyledText::plain("2kb")],
                        decoration: Decoration::Normal,
                    },
                    DataRow {
                        cells: vec![StyledText::plain("docs"), StyledText::plain("")],
                        decoration: Decoration::Normal,
                    },
                ],
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
            },
            vec![
                TreeTableNode::new(vec![0], 0).with_expanded(true),
                TreeTableNode::new(vec![0, 0], 1),
                TreeTableNode::new(vec![1], 0).with_expanded(false),
            ],
        )
    }

    fn line(buf: &Buffer, y: u16, width: u16) -> String {
        (0..width)
            .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
            .collect()
    }

    /// Char-cell index of `needle`'s first occurrence in `line` — unlike
    /// `str::find`, which returns a *byte* offset and desyncs from the
    /// screen column as soon as a row contains a multi-byte glyph (the
    /// chevron here).
    fn find_char_pos(line: &str, needle: &str) -> Option<usize> {
        let chars: Vec<char> = line.chars().collect();
        let needle: Vec<char> = needle.chars().collect();
        if needle.is_empty() || chars.len() < needle.len() {
            return None;
        }
        chars
            .windows(needle.len())
            .position(|w| w == needle.as_slice())
    }

    #[test]
    fn branch_row_paints_a_chevron_and_the_label_still_appears() {
        let table = make_tree_table();
        let area = Rect::new(0, 0, 30, 10);
        let mut buf = Buffer::empty(area);
        paint_tree_table_cells(&mut buf, area, &table, &Theme::default(), None, false);
        let row0 = line(&buf, 1, 20);
        assert!(
            row0.contains('▾'),
            "expanded branch row should paint the expanded chevron: {row0:?}"
        );
        assert!(
            row0.contains("src"),
            "the row's label should still paint after the tree-cell prefix: {row0:?}"
        );
    }

    #[test]
    fn indented_leaf_row_has_no_chevron_but_keeps_its_label() {
        let table = make_tree_table();
        let area = Rect::new(0, 0, 30, 10);
        let mut buf = Buffer::empty(area);
        paint_tree_table_cells(&mut buf, area, &table, &Theme::default(), None, false);
        let row1 = line(&buf, 2, 20);
        assert!(!row1.contains('▾') && !row1.contains('▸'));
        assert!(row1.contains("main.rs"));
        let row0 = line(&buf, 1, 20);
        let src_at = find_char_pos(&row0, "src").unwrap();
        let main_at = find_char_pos(&row1, "main.rs").unwrap();
        assert!(
            main_at > src_at,
            "indent 1 should start further right than indent 0: {row0:?} / {row1:?}"
        );
    }

    #[test]
    fn column_1_stays_aligned_across_every_indent_level() {
        let table = make_tree_table();
        let area = Rect::new(0, 0, 30, 10);
        let mut buf = Buffer::empty(area);
        paint_tree_table_cells(&mut buf, area, &table, &Theme::default(), None, false);
        // The indent only pushes column 0's label, never column 1's.
        let layout = cell_tree_table_layout(area, &table);
        let col1_x = layout.table.columns[1].x.round() as usize;
        let row0 = line(&buf, 1, 30);
        let row1 = line(&buf, 2, 30);
        let row0_tail: String = row0.chars().skip(col1_x).collect();
        let row1_tail: String = row1.chars().skip(col1_x).collect();
        assert!(row1_tail.trim_start().starts_with("2kb"));
        assert!(row0_tail.trim_end().is_empty());
    }

    #[test]
    fn clicking_the_chevron_resolves_to_a_chevron_hit_not_a_row_hit() {
        let table = make_tree_table();
        let area = Rect::new(0, 0, 30, 10);
        let layout = cell_tree_table_layout(area, &table);
        let (idx, bounds) = layout.chevrons[0];
        assert_eq!(idx, 0);
        let hit = layout.hit_test(bounds.x, bounds.y, 0, table.table.rows.len());
        assert_eq!(hit, TreeTableHit::Chevron(0));
    }

    #[test]
    fn clicking_the_header_still_resolves_to_header_not_chevron() {
        let table = make_tree_table();
        let area = Rect::new(0, 0, 30, 10);
        let layout = cell_tree_table_layout(area, &table);
        let hit = layout.hit_test(0.0, 0.0, 0, table.table.rows.len());
        assert_eq!(hit, TreeTableHit::Table(DataTableHit::Header { col: 0 }));
    }

    #[test]
    fn a_leaf_row_has_no_chevron_entry_and_clicking_it_is_a_row_hit() {
        let table = make_tree_table();
        let area = Rect::new(0, 0, 30, 10);
        let layout = cell_tree_table_layout(area, &table);
        assert!(!layout.chevrons.iter().any(|(idx, _)| *idx == 1));
        let hit = layout.hit_test(0.0, 2.0, 0, table.table.rows.len());
        assert_eq!(hit, TreeTableHit::Table(DataTableHit::Row { idx: 1 }));
    }

    #[test]
    fn a_collapsed_branch_paints_the_collapsed_chevron() {
        let table = make_tree_table();
        let area = Rect::new(0, 0, 30, 10);
        let mut buf = Buffer::empty(area);
        paint_tree_table_cells(&mut buf, area, &table, &Theme::default(), None, false);
        let row2 = line(&buf, 3, 20);
        assert!(
            row2.contains('▸'),
            "collapsed branch row should paint the collapsed chevron: {row2:?}"
        );
    }

    #[test]
    fn narrow_rect_drops_the_size_column_before_starving_the_tree_column() {
        let table = make_tree_table();
        // The deepest row is indent 1, so the floor is
        // `1 * TreeStyle::default().indent (2) + chevron (1) + label (1)`
        // = 4.0. At width 8 the plain `DataTable` squeeze would leave
        // column 0 at 2.0, so `Size` is dropped and column 0 takes the
        // rest.
        let area = Rect::new(0, 0, 8, 10);
        let mut buf = Buffer::empty(area);
        let layout = paint_tree_table_cells(&mut buf, area, &table, &Theme::default(), None, false);
        assert_eq!(layout.table.columns.len(), 1);
        assert!(layout.table.columns[0].width >= 4.0);
    }

    #[test]
    fn narrow_rect_never_panics_even_when_the_viewport_is_narrower_than_the_floor() {
        let table = make_tree_table();
        let area = Rect::new(0, 0, 2, 10);
        let mut buf = Buffer::empty(area);
        let layout = paint_tree_table_cells(&mut buf, area, &table, &Theme::default(), None, false);
        assert_eq!(layout.table.columns.len(), 1);
        assert_eq!(layout.table.columns[0].width, 4.0);
    }

    /// The descriptor's `striped` tints every other unselected tree row,
    /// the same as on a plain `DataTable`.
    #[test]
    fn striped_tints_alternate_tree_rows() {
        let mut table = make_tree_table();
        table.table.striped = true;
        let area = Rect::new(0, 0, 30, 10);
        let mut buf = Buffer::empty(area);
        paint_tree_table_cells(&mut buf, area, &table, &Theme::default(), None, false);
        let theme = Theme::default();
        let mut plain = make_tree_table();
        plain.table.striped = false;
        let mut plain_buf = Buffer::empty(area);
        paint_tree_table_cells(&mut plain_buf, area, &plain, &theme, None, false);
        // Row 1 (odd) differs from the unstriped paint; row 0 (even)
        // does not. Column 20 is in column 0, past every label.
        assert_ne!(buf[(20, 2)].bg, plain_buf[(20, 2)].bg, "odd row tinted");
        assert_eq!(buf[(20, 1)].bg, plain_buf[(20, 1)].bg, "even row untouched");
    }
    /// The tree-table painter — tree cells with icons and chevrons in
    /// column 0, plain rows past the tree, hover/press/stripes and the
    /// selection — cell for cell against the golden.
    #[test]
    fn cell_paint_matches_the_golden() {
        use crate::tui::cell_surface::oracle;
        use crate::types::Icon;

        let theme = Theme::default();
        let mut table = make_tree_table();
        table.tree[1].icon = Some(Icon::new("\u{e7a8}", "r"));
        table.tree[0].icon = Some(Icon::new("\u{4e2d}", "d"));
        table.table.selected_idx = Some(1);
        table.table.rows.push(DataRow {
            cells: vec![StyledText::plain("orphan"), StyledText::plain("1b")],
            decoration: Decoration::Muted,
        });
        let state = DataTablePaintState::default()
            .with_hovered_idx(Some(2))
            .with_pressed_idx(Some(0))
            .with_striped(true);
        let paint = |area: Rect, st: &DataTablePaintState, nerd: bool, w, h| {
            oracle::paint(w, h, |s| {
                paint_laid_out(s, area, &table, &theme, st, nerd, cell_tree_table_layout(area, &table));
            })
        };
        oracle::assert_golden(
            "tree_table",
            &[
                ("plain", &paint(Rect::new(0, 0, 26, 7), &DataTablePaintState::default(), false, 26, 7)),
                ("states_nerd", &paint(Rect::new(1, 1, 26, 7), &state, true, 28, 9)),
                ("narrow", &paint(Rect::new(0, 0, 9, 7), &state, false, 9, 7)),
            ],
        );
    }
}
