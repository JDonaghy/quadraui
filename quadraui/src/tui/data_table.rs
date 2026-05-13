//! TUI rasteriser for [`crate::DataTable`].
//!
//! Renders column headers with sort indicators, then body rows with
//! per-cell text aligned within resolved column bounds. Selected row
//! uses `theme.selection_bg`. Focused table highlights the selected
//! row more prominently.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;

use super::{ratatui_color, set_cell};
use crate::primitives::data_table::{
    ColumnAlign, ColumnMeasure, DataTable, DataTableLayout, SortDirection,
};
use crate::theme::Theme;

/// Draw a `DataTable` into `area`. Returns the layout used for
/// painting so callers can hit-test at the same coordinates.
pub fn draw_data_table(
    buf: &mut Buffer,
    area: Rect,
    table: &DataTable,
    theme: &Theme,
) -> DataTableLayout {
    let layout = table.layout(
        area.width as f32,
        area.height as f32,
        1.0,
        1.0,
        1.0,
        |col| ColumnMeasure::new(col.title.chars().count() as f32),
    );

    if area.width == 0 || area.height == 0 {
        return layout;
    }

    let header_bg = ratatui_color(theme.tab_bar_bg);
    let header_fg = ratatui_color(theme.foreground);
    let body_bg = ratatui_color(theme.background);
    let body_fg = ratatui_color(theme.foreground);
    let sel_bg = ratatui_color(theme.selection_bg);
    let muted_fg = ratatui_color(theme.muted_fg);

    // ── Header row ────────────────────────────────────────────────────
    let header_y = area.y;
    for x in 0..area.width {
        set_cell(buf, area.x + x, header_y, ' ', header_fg, header_bg);
    }

    for (col_idx, rc) in layout.columns.iter().enumerate() {
        if col_idx >= table.columns.len() {
            break;
        }
        let col = &table.columns[col_idx];
        let col_x = area.x + rc.x.round() as u16;
        let col_w = rc.width.round() as u16;
        if col_w == 0 {
            continue;
        }

        let sort_suffix = match &table.sort {
            Some((si, dir)) if *si == col_idx => match dir {
                SortDirection::Ascending => " ▲",
                SortDirection::Descending => " ▼",
            },
            _ => "",
        };
        let title = format!("{}{}", col.title, sort_suffix);
        let text_len = title.chars().count() as u16;
        let start = align_offset(col.align, text_len, col_w);

        for (i, ch) in title.chars().enumerate() {
            let cx = col_x + start + i as u16;
            if cx >= area.x + area.width {
                break;
            }
            set_cell(buf, cx, header_y, ch, header_fg, header_bg);
            if let Some(cell) = buf.cell_mut(ratatui::prelude::Position::new(cx, header_y)) {
                cell.set_style(ratatui::style::Style::default().add_modifier(Modifier::BOLD));
            }
        }
    }

    // ── Body rows ─────────────────────────────────────────────────────
    let body_y = area.y + 1;
    let visible = layout
        .visible_rows
        .min(table.rows.len().saturating_sub(table.scroll_offset));

    for row_idx in 0..visible {
        let abs_idx = table.scroll_offset + row_idx;
        let row = &table.rows[abs_idx];
        let y = body_y + row_idx as u16;
        let is_selected = table.selected_idx == Some(abs_idx);

        let (row_fg, row_bg) = if is_selected {
            (body_fg, sel_bg)
        } else {
            (body_fg, body_bg)
        };

        // Fill row background
        for x in 0..area.width {
            set_cell(buf, area.x + x, y, ' ', row_fg, row_bg);
        }

        for (col_idx, rc) in layout.columns.iter().enumerate() {
            let cell_text: String = row
                .cells
                .get(col_idx)
                .map(|c| c.spans.iter().map(|s| s.text.as_str()).collect())
                .unwrap_or_default();
            let text = cell_text.as_str();
            let col_x = area.x + rc.x.round() as u16;
            let col_w = rc.width.round() as u16;
            if col_w == 0 || text.is_empty() {
                continue;
            }

            let align = table
                .columns
                .get(col_idx)
                .map(|c| c.align)
                .unwrap_or(ColumnAlign::Left);
            let text_len = text.chars().count() as u16;
            let start = align_offset(align, text_len, col_w);

            let cell_fg = if row.decoration == crate::types::Decoration::Muted {
                muted_fg
            } else {
                row_fg
            };

            for (i, ch) in text.chars().enumerate() {
                let cx = col_x + start + i as u16;
                if cx >= area.x + area.width {
                    break;
                }
                set_cell(buf, cx, y, ch, cell_fg, row_bg);
            }
        }
    }

    // ── Scrollbar ──────────────────────────────────────────────────────
    if table.show_scrollbar
        && table.rows.len() > layout.visible_rows
        && layout.scrollbar_width > 0.0
    {
        let sb_x = area.x + area.width - layout.scrollbar_width.round() as u16;
        let sb_track = crate::event::Rect::new(
            sb_x as f32,
            (area.y + 1) as f32,
            1.0,
            (area.height.saturating_sub(1)) as f32,
        );
        let sb = crate::primitives::scrollbar::Scrollbar::vertical(
            table.id.clone(),
            sb_track,
            table.scroll_offset as f32,
            table.rows.len() as f32,
            layout.visible_rows as f32,
            1.0,
        );
        super::draw_scrollbar(buf, &sb, theme, theme.background);
    }

    layout
}

fn align_offset(align: ColumnAlign, text_len: u16, col_w: u16) -> u16 {
    match align {
        ColumnAlign::Left => 0,
        ColumnAlign::Center => col_w.saturating_sub(text_len) / 2,
        ColumnAlign::Right => col_w.saturating_sub(text_len),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::data_table::{Column, ColumnWidth, DataRow, DataTable, DataTableHit};
    use crate::types::{Decoration, StyledText, WidgetId};

    fn make_table() -> DataTable {
        DataTable {
            id: WidgetId::new("test"),
            columns: vec![
                Column {
                    title: "Name".into(),
                    width: ColumnWidth::Flex(2.0),
                    align: ColumnAlign::Left,
                },
                Column {
                    title: "Status".into(),
                    width: ColumnWidth::Flex(1.0),
                    align: ColumnAlign::Left,
                },
                Column {
                    title: "Age".into(),
                    width: ColumnWidth::Fixed(5.0),
                    align: ColumnAlign::Right,
                },
            ],
            rows: vec![
                DataRow {
                    cells: vec![
                        StyledText::plain("pod-abc"),
                        StyledText::plain("Running"),
                        StyledText::plain("3d"),
                    ],
                    decoration: Decoration::Normal,
                },
                DataRow {
                    cells: vec![
                        StyledText::plain("pod-xyz"),
                        StyledText::plain("Pending"),
                        StyledText::plain("1h"),
                    ],
                    decoration: Decoration::Normal,
                },
            ],
            selected_idx: Some(0),
            scroll_offset: 0,
            sort: Some((0, SortDirection::Ascending)),
            has_focus: true,
            show_scrollbar: false,
        }
    }

    #[test]
    fn header_paints_column_titles() {
        let table = make_table();
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        draw_data_table(&mut buf, area, &table, &Theme::default());

        // Header row at y=0 should contain "Name" somewhere
        let header: String = (0..40)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            header.contains("Name"),
            "header should contain 'Name', got: {header}"
        );
        assert!(
            header.contains("Status"),
            "header should contain 'Status', got: {header}"
        );
        assert!(
            header.contains("Age"),
            "header should contain 'Age', got: {header}"
        );
        assert!(
            header.contains("▲"),
            "sorted column should show ▲, got: {header}"
        );
    }

    #[test]
    fn body_paints_row_content() {
        let table = make_table();
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        draw_data_table(&mut buf, area, &table, &Theme::default());

        // Row 0 at y=1 should contain "pod-abc"
        let row0: String = (0..40)
            .map(|x| buf[(x, 1)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            row0.contains("pod-abc"),
            "row 0 should contain 'pod-abc', got: {row0}"
        );
        assert!(
            row0.contains("Running"),
            "row 0 should contain 'Running', got: {row0}"
        );
    }

    #[test]
    fn paint_click_round_trip_header() {
        let table = make_table();
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let layout = draw_data_table(&mut buf, area, &table, &Theme::default());

        // Find "Status" in the header row
        let header: String = (0..40)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
            .collect();
        let status_pos = header.find("Status").expect("Status should be in header");
        // Click in the middle of "Status" (not at the left edge where the
        // divider grab zone would match).
        let hit = layout.hit_test(status_pos as f32 + 3.0, 0.5, 0, table.rows.len());
        assert_eq!(hit, DataTableHit::Header { col: 1 });
    }

    #[test]
    fn paint_click_round_trip_row() {
        let table = make_table();
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let layout = draw_data_table(&mut buf, area, &table, &Theme::default());

        // Find "pod-xyz" in row 1 (y=2)
        let row1: String = (0..40)
            .map(|x| buf[(x, 2)].symbol().chars().next().unwrap_or(' '))
            .collect();
        let pod_pos = row1.find("pod-xyz").expect("pod-xyz should be in row 1");
        let hit = layout.hit_test(pod_pos as f32, 2.5, 0, table.rows.len());
        assert_eq!(hit, DataTableHit::Row { idx: 1 });
    }
}
