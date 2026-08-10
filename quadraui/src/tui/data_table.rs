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

/// Draw a `DataTable` into `area`. `hovered_idx` carries per-frame
/// hover state so the rasteriser can tint the hovered row. Returns
/// the layout used for painting so callers can hit-test at the same
/// coordinates.
pub fn draw_data_table(
    buf: &mut Buffer,
    area: Rect,
    table: &DataTable,
    theme: &Theme,
    hovered_idx: Option<usize>,
) -> DataTableLayout {
    let mut layout = table.layout(
        area.width as f32,
        area.height as f32,
        1.0,
        1.0,
        1.0,
        |col| ColumnMeasure::new(col.title.chars().count() as f32),
    );

    // TUI is cell-granular: every column paints at a *rounded* offset
    // (`h_off` below), not the raw fractional `h_scroll`. `hit_test` /
    // `column_hit` do no rounding of their own — they trust
    // `DataTableLayout::h_scroll` to already equal what was painted — so
    // this must be rounded here, once, before the layout is handed back
    // to callers for hit-testing. Skipping this left a gap whenever
    // `h_scroll`'s fractional part crossed 0.5: painting rounded up a
    // full cell while hit-testing added the un-rounded value back,
    // misrouting clicks on the leftmost visible cell of whichever column
    // scrolled into view (#550 round 2).
    layout.h_scroll = table.h_scroll.round();

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

    let sep_fg = ratatui_color(theme.separator);
    // Same rounded value now carried on `layout.h_scroll` — kept as a
    // local `i16` here since every paint-position computation below
    // works in signed cell-offset arithmetic.
    let h_off = layout.h_scroll as i16;

    for (col_idx, rc) in layout.columns.iter().enumerate() {
        if col_idx >= table.columns.len() {
            break;
        }
        let col = &table.columns[col_idx];
        let col_x_raw = area.x as i32 + rc.x.round() as i32 - h_off as i32;
        let col_x_end = col_x_raw + rc.width.round() as i32;
        if col_x_end <= area.x as i32 || col_x_raw >= (area.x + area.width) as i32 {
            continue;
        }
        let col_w = rc.width.round() as u16;
        if col_w == 0 {
            continue;
        }

        // Column separator on the right edge (skip last column).
        if col_idx + 1 < table.columns.len() {
            let sep_cx = col_x_raw + col_w as i32 - 1;
            if sep_cx >= area.x as i32 && sep_cx < (area.x + area.width) as i32 {
                set_cell(buf, sep_cx as u16, header_y, '│', sep_fg, header_bg);
            }
        }

        let sort_suffix = match &table.sort {
            Some((si, dir)) if *si == col_idx => match dir {
                SortDirection::Ascending => " ▲",
                SortDirection::Descending => " ▼",
            },
            _ => "",
        };
        let title = format!("{}{}", col.title, sort_suffix);
        let text_len = title.chars().count() as i32;
        let usable_w = if col_idx + 1 < table.columns.len() {
            col_w.saturating_sub(1) as i32
        } else {
            col_w as i32
        };
        let start = align_offset(col.align, text_len as u16, usable_w as u16) as i32;

        for (i, ch) in title.chars().enumerate() {
            let cx = col_x_raw + start + i as i32;
            if cx >= col_x_raw + usable_w || cx >= (area.x + area.width) as i32 {
                break;
            }
            if cx >= area.x as i32 {
                set_cell(buf, cx as u16, header_y, ch, header_fg, header_bg);
                if let Some(cell) =
                    buf.cell_mut(ratatui::prelude::Position::new(cx as u16, header_y))
                {
                    cell.set_style(ratatui::style::Style::default().add_modifier(Modifier::BOLD));
                }
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
        let is_hovered = hovered_idx == Some(abs_idx) && !is_selected;

        let (row_fg, row_bg) = if is_selected {
            (body_fg, sel_bg)
        } else if is_hovered {
            (body_fg, ratatui_color(theme.tab_bar_bg))
        } else {
            (body_fg, body_bg)
        };

        // Fill row background
        for x in 0..area.width {
            set_cell(buf, area.x + x, y, ' ', row_fg, row_bg);
        }

        for (col_idx, rc) in layout.columns.iter().enumerate() {
            let col_x_raw = area.x as i32 + rc.x.round() as i32 - h_off as i32;
            let col_x_end = col_x_raw + rc.width.round() as i32;
            if col_x_end <= area.x as i32 || col_x_raw >= (area.x + area.width) as i32 {
                continue;
            }
            let col_w = rc.width.round() as u16;
            if col_w == 0 {
                continue;
            }

            // Column separator on the right edge (skip last column) —
            // drawn regardless of whether this cell has text, mirroring
            // the header separator. Body rows previously drew none at
            // all, so adjacent cells butted directly together (#516
            // defect 2).
            if col_idx + 1 < table.columns.len() {
                let sep_cx = col_x_raw + col_w as i32 - 1;
                if sep_cx >= area.x as i32 && sep_cx < (area.x + area.width) as i32 {
                    set_cell(buf, sep_cx as u16, y, '│', sep_fg, row_bg);
                }
            }

            let styled = match row.cells.get(col_idx) {
                Some(c) if !c.spans.is_empty() => c,
                _ => continue,
            };
            let full_text: String = styled.spans.iter().map(|s| s.text.as_str()).collect();
            if full_text.is_empty() {
                continue;
            }

            let align = table
                .columns
                .get(col_idx)
                .map(|c| c.align)
                .unwrap_or(ColumnAlign::Left);

            // Reserve one cell for the separator (except on the last
            // column) — the same `usable_w` term the header already
            // uses — so an over-long body cell is clipped at its own
            // column boundary instead of painting across its neighbours
            // (#516 defect 1: this was previously clipped only to the
            // table's right edge, so it interleaved with every column
            // to its right).
            let usable_w: u16 = if col_idx + 1 < table.columns.len() {
                col_w.saturating_sub(1)
            } else {
                col_w
            };

            let text_len = full_text.chars().count() as u16;
            let (visible_len, needs_ellipsis) = if text_len <= usable_w {
                (text_len, false)
            } else if usable_w == 0 {
                (0, false)
            } else {
                (usable_w - 1, true)
            };
            let displayed_len = visible_len + u16::from(needs_ellipsis);
            let start = align_offset(align, displayed_len, usable_w) as i32;

            let is_muted = row.decoration == crate::types::Decoration::Muted;
            let mut char_idx = 0u16;
            'cell: for span in &styled.spans {
                let span_fg = if is_muted {
                    muted_fg
                } else {
                    span.fg.map(ratatui_color).unwrap_or(row_fg)
                };
                for ch in span.text.chars() {
                    if char_idx == visible_len {
                        if needs_ellipsis {
                            let cx = col_x_raw + start + char_idx as i32;
                            if cx >= area.x as i32 && cx < (area.x + area.width) as i32 {
                                set_cell(buf, cx as u16, y, '…', span_fg, row_bg);
                            }
                        }
                        break 'cell;
                    }
                    let cx = col_x_raw + start + char_idx as i32;
                    if cx >= (area.x + area.width) as i32 {
                        break 'cell;
                    }
                    if cx >= area.x as i32 {
                        set_cell(buf, cx as u16, y, ch, span_fg, row_bg);
                    }
                    char_idx += 1;
                }
            }
        }
    }

    // ── Scrollbar ──────────────────────────────────────────────────────
    let footer_h = layout.footer_height.round() as u16;
    if table.show_scrollbar
        && table.rows.len() > layout.visible_rows
        && layout.scrollbar_width > 0.0
    {
        let sb_x = area.x + area.width - layout.scrollbar_width.round() as u16;
        let sb_track = crate::event::Rect::new(
            sb_x as f32,
            (area.y + 1) as f32,
            1.0,
            (area.height.saturating_sub(1).saturating_sub(footer_h)) as f32,
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

    // ── Horizontal scrollbar ─────────────────────────────────────────
    if layout.h_scrollbar_height > 0.0 && layout.content_width > 0.0 {
        let hsb_y = area.y + area.height - footer_h - layout.h_scrollbar_height.round() as u16;
        let track_w = (area.width as f32 - layout.scrollbar_width).max(1.0);
        let hsb_track = crate::event::Rect::new(area.x as f32, hsb_y as f32, track_w, 1.0);
        let visible_w = (area.width as f32 - layout.scrollbar_width).max(1.0);
        let hsb = crate::primitives::scrollbar::Scrollbar::horizontal(
            table.id.clone(),
            hsb_track,
            table.h_scroll,
            layout.content_width,
            visible_w,
            1.0,
        );
        super::draw_scrollbar(buf, &hsb, theme, theme.background);
    }

    // ── Footer (pinned summary row) ────────────────────────────────────
    if let Some(footer) = &table.footer {
        if footer_h > 0 {
            // Content always sits on the viewport's bottom-most row;
            // the divider gets the row directly above it. Both rows
            // are reserved via `footer_height == row_height * 2.0`
            // (see `DataTable::layout`), so neither is ever painted
            // over by a body row.
            let footer_y = area.y + area.height - 1;
            let footer_bg = ratatui_color(theme.tab_bar_bg);
            let footer_fg = ratatui_color(theme.foreground);

            // Divider rule directly above the footer row.
            if footer_y > area.y {
                for x in 0..area.width {
                    set_cell(buf, area.x + x, footer_y - 1, '─', sep_fg, header_bg);
                }
            }

            for x in 0..area.width {
                set_cell(buf, area.x + x, footer_y, ' ', footer_fg, footer_bg);
            }

            for (col_idx, rc) in layout.columns.iter().enumerate() {
                let styled = match footer.cells.get(col_idx) {
                    Some(c) if !c.spans.is_empty() => c,
                    _ => continue,
                };
                let full_text: String = styled.spans.iter().map(|s| s.text.as_str()).collect();
                let col_x_raw = area.x as i32 + rc.x.round() as i32 - h_off as i32;
                let col_x_end = col_x_raw + rc.width.round() as i32;
                if col_x_end <= area.x as i32 || col_x_raw >= (area.x + area.width) as i32 {
                    continue;
                }
                let col_w = rc.width.round() as u16;
                if col_w == 0 || full_text.is_empty() {
                    continue;
                }

                let align = table
                    .columns
                    .get(col_idx)
                    .map(|c| c.align)
                    .unwrap_or(ColumnAlign::Left);
                let text_len = full_text.chars().count() as u16;
                let start = align_offset(align, text_len, col_w) as i32;

                let mut char_offset = 0i32;
                for span in &styled.spans {
                    let span_fg = span.fg.map(ratatui_color).unwrap_or(footer_fg);
                    for ch in span.text.chars() {
                        let cx = col_x_raw + start + char_offset;
                        if cx >= (area.x + area.width) as i32 {
                            break;
                        }
                        if cx >= area.x as i32 {
                            set_cell(buf, cx as u16, footer_y, ch, span_fg, footer_bg);
                            if let Some(cell) =
                                buf.cell_mut(ratatui::prelude::Position::new(cx as u16, footer_y))
                            {
                                cell.set_style(
                                    ratatui::style::Style::default().add_modifier(Modifier::BOLD),
                                );
                            }
                        }
                        char_offset += 1;
                    }
                }
            }
        }
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

    /// Char-cell index of `needle`'s first occurrence in `line` — unlike
    /// `str::find`, which returns a *byte* offset and desyncs from the
    /// screen column as soon as a row contains a multi-byte glyph (e.g.
    /// the `'│'` separator, #516 defect 2).
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
            min_total_width: None,
            h_scroll: 0.0,
            column_overrides: Vec::new(),
            footer: None,
        }
    }

    #[test]
    fn header_paints_column_titles() {
        let table = make_table();
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        draw_data_table(&mut buf, area, &table, &Theme::default(), None);

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
        draw_data_table(&mut buf, area, &table, &Theme::default(), None);

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

    // ── #516 defect 1 & 2: body clipping, ellipsis, body separators ─────

    #[test]
    fn body_rows_draw_column_separator_at_same_x_as_header() {
        // Previously `'│'` was only drawn in the header loop; body rows
        // had none at all, so adjacent cells butted directly together.
        let table = make_table();
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        draw_data_table(&mut buf, area, &table, &Theme::default(), None);

        let find_seps =
            |y: u16| -> Vec<u16> { (0..40).filter(|&x| buf[(x, y)].symbol() == "│").collect() };

        let header_seps = find_seps(0);
        assert_eq!(
            header_seps.len(),
            table.columns.len() - 1,
            "header should have one separator per internal column boundary, got {header_seps:?}"
        );

        // `make_table()` has 2 rows, painted at y=1 and y=2.
        for row_y in [1u16, 2u16] {
            let body_seps = find_seps(row_y);
            assert_eq!(
                body_seps, header_seps,
                "body row {row_y} separators should sit at the same x as the header's"
            );
        }
    }

    #[test]
    fn body_cell_exactly_filling_column_still_gets_a_separator() {
        // Acceptance: a cell whose text exactly fills its column must
        // still be separated from its neighbour — no zero-gap abutment.
        let mut table = make_table();
        // Column 0 ("Name") is Flex(2.0) of a 40-wide, 3-Flex-weight-unit
        // area minus the 5-wide fixed "Age" column: (40-5)*2/3 ≈ 23.33 →
        // resolves to 23 cells wide, of which 22 are usable (one cell is
        // reserved for the separator, same as the header). Fill exactly
        // that usable width.
        table.rows[0].cells[0] = StyledText::plain("x".repeat(22));
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let layout = draw_data_table(&mut buf, area, &table, &Theme::default(), None);

        let sep_x = (layout.columns[0].x.round() as i32 + layout.columns[0].width.round() as i32
            - 1) as u16;
        let ch = buf[(sep_x, 1)].symbol().to_string();
        assert_eq!(
            ch, "│",
            "an exactly-filling cell must not paint over the separator column"
        );
    }

    #[test]
    fn body_cell_wider_than_column_is_clipped_and_does_not_bleed_into_neighbour() {
        // The core regression: an over-long *middle* column previously
        // painted straight across every column to its right —
        // interleaving, not truncation — because the body loop only
        // clipped to the table's right edge (`area.x + area.width`),
        // never to the column's own boundary the header loop already
        // used.
        let table = DataTable {
            id: WidgetId::new("clip-test"),
            columns: vec![
                Column {
                    title: "A".into(),
                    width: ColumnWidth::Flex(1.0),
                    align: ColumnAlign::Left,
                },
                Column {
                    title: "B".into(),
                    width: ColumnWidth::Flex(1.0),
                    align: ColumnAlign::Left,
                },
                Column {
                    title: "C".into(),
                    width: ColumnWidth::Flex(1.0),
                    align: ColumnAlign::Left,
                },
            ],
            rows: vec![DataRow {
                cells: vec![
                    StyledText::plain("A"),
                    StyledText::plain("this-value-is-far-wider-than-its-resolved-column-share"),
                    StyledText::plain("C"),
                ],
                decoration: Decoration::Normal,
            }],
            selected_idx: None,
            scroll_offset: 0,
            sort: None,
            has_focus: false,
            show_scrollbar: false,
            min_total_width: None,
            h_scroll: 0.0,
            column_overrides: Vec::new(),
            footer: None,
        };
        let area = Rect::new(0, 0, 30, 10);
        let mut buf = Buffer::empty(area);
        let layout = draw_data_table(&mut buf, area, &table, &Theme::default(), None);

        let row: Vec<char> = (0..30)
            .map(|x| buf[(x, 1)].symbol().chars().next().unwrap_or(' '))
            .collect();
        let row_str: String = row.iter().collect();

        let c_col = &layout.columns[2];
        let c_start = c_col.x.round() as usize;

        // Column C's own value paints intact at its own column start...
        assert_eq!(row[c_start], 'C', "row: {row_str:?}");
        // ...and every other cell inside C's column is untouched
        // background, never a stray character bled over from B.
        for (x, &ch) in row.iter().enumerate().skip(c_start + 1) {
            assert_eq!(
                ch, ' ',
                "cell at x={x} inside column C should be blank, not corrupted by B's overflow \
                 (row: {row_str:?})"
            );
        }

        // The overflowing B cell shows an ellipsis rather than a hard cut.
        let b_col = &layout.columns[1];
        let b_range = (b_col.x.round() as usize)..c_start;
        assert!(
            b_range.clone().any(|x| row[x] == '…'),
            "over-long body cell should end in an ellipsis, not a hard cut: {row_str:?}"
        );
    }

    #[test]
    fn paint_click_round_trip_header() {
        let table = make_table();
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let layout = draw_data_table(&mut buf, area, &table, &Theme::default(), None);

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
        let layout = draw_data_table(&mut buf, area, &table, &Theme::default(), None);

        // Find "pod-xyz" in row 1 (y=2)
        let row1: String = (0..40)
            .map(|x| buf[(x, 2)].symbol().chars().next().unwrap_or(' '))
            .collect();
        let pod_pos = row1.find("pod-xyz").expect("pod-xyz should be in row 1");
        let hit = layout.hit_test(pod_pos as f32, 2.5, 0, table.rows.len());
        assert_eq!(hit, DataTableHit::Row { idx: 1 });
    }

    /// A table with many more rows than fit the viewport, for footer
    /// pinning + scroll tests.
    fn make_scrolling_table(nrows: usize) -> DataTable {
        let mut table = make_table();
        table.rows = (0..nrows)
            .map(|r| DataRow {
                cells: vec![
                    StyledText::plain(format!("pod-{r}")),
                    StyledText::plain("Running"),
                    StyledText::plain(format!("{r}d")),
                ],
                decoration: Decoration::Normal,
            })
            .collect();
        table
    }

    #[test]
    fn footer_row_renders_pinned_regardless_of_scroll_offset() {
        let mut table = make_scrolling_table(50);
        table.footer = Some(DataRow {
            cells: vec![
                StyledText::plain("TOTAL"),
                StyledText::plain(""),
                StyledText::plain("50d"),
            ],
            decoration: Decoration::Normal,
        });
        let area = Rect::new(0, 0, 40, 10);

        for scroll_offset in [0, 5, 30] {
            table.scroll_offset = scroll_offset;
            let mut buf = Buffer::empty(area);
            let layout = draw_data_table(&mut buf, area, &table, &Theme::default(), None);
            assert!(layout.footer_height > 0.0);

            // Footer text is always painted on the last row of the
            // viewport, no matter how far the body has scrolled.
            let last_row: String = (0..40)
                .map(|x| {
                    buf[(x, area.height - 1)]
                        .symbol()
                        .chars()
                        .next()
                        .unwrap_or(' ')
                })
                .collect();
            assert!(
                last_row.contains("TOTAL"),
                "footer should stay pinned at scroll_offset={scroll_offset}, got: {last_row}"
            );
        }
    }

    #[test]
    fn footer_cell_aligns_under_its_column() {
        // Right-aligned "Age" column → its footer total should sit at
        // the same right edge as a normal right-aligned body cell.
        let mut table = make_table();
        table.footer = Some(DataRow {
            cells: vec![
                StyledText::plain(""),
                StyledText::plain(""),
                StyledText::plain("4d"),
            ],
            decoration: Decoration::Normal,
        });
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let layout = draw_data_table(&mut buf, area, &table, &Theme::default(), None);
        let footer_y = area.height - 1;

        let footer_line: String = (0..40)
            .map(|x| buf[(x, footer_y)].symbol().chars().next().unwrap_or(' '))
            .collect();
        let body_line: String = (0..40)
            .map(|x| buf[(x, 1)].symbol().chars().next().unwrap_or(' '))
            .collect();
        // Char-cell position, not byte offset: `str::find` returns a byte
        // index, and body rows now paint a `'│'` separator (#516 defect
        // 2) which is 3 bytes but exactly 1 cell — a byte-offset
        // comparison would fail even though the two cells line up.
        let footer_pos = find_char_pos(&footer_line, "4d").expect("footer total painted");
        let body_pos = find_char_pos(&body_line, "3d").expect("body cell painted");
        assert_eq!(
            footer_pos, body_pos,
            "footer's Age total should align at the same x as a body row's Age cell"
        );

        // And it should land inside the Age column's resolved bounds.
        let age_col = &layout.columns[2];
        assert!(footer_pos as f32 >= age_col.x && (footer_pos as f32) < age_col.x + age_col.width);
    }

    #[test]
    fn footer_click_is_footer_hit_not_row() {
        let mut table = make_scrolling_table(50);
        table.footer = Some(DataRow {
            cells: vec![StyledText::plain("TOTAL")],
            decoration: Decoration::Normal,
        });
        let area = Rect::new(0, 0, 40, 10);
        let layout = {
            let mut buf = Buffer::empty(area);
            draw_data_table(&mut buf, area, &table, &Theme::default(), None)
        };

        let hit = layout.hit_test(
            2.0,
            (area.height - 1) as f32 + 0.5,
            table.scroll_offset,
            table.rows.len(),
        );
        assert_eq!(hit, DataTableHit::Footer);
    }

    #[test]
    fn none_footer_body_unaffected() {
        // Regression guard (#432 req 5): with `footer: None`, the last
        // paintable row is still a body row, not blank footer space.
        let table = make_scrolling_table(3);
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let layout = draw_data_table(&mut buf, area, &table, &Theme::default(), None);
        assert_eq!(layout.footer_height, 0.0);
        let row2: String = (0..40)
            .map(|x| buf[(x, 3)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            row2.contains("pod-2"),
            "row 2 should paint normally, got: {row2}"
        );
    }

    // ── #550: paint/click round-trip under horizontal scrolling ────────

    /// A table laid out wider than its viewport, so `h_scroll` is
    /// genuinely drivable: 4 × `Fixed(12.0)` columns (48 content cells)
    /// inside a 24-cell viewport.
    fn make_h_scrolling_table() -> DataTable {
        let titles = ["Alpha", "Bravo", "Charlie", "Delta"];
        DataTable {
            id: WidgetId::new("hscroll"),
            columns: titles
                .iter()
                .map(|t| Column {
                    title: (*t).into(),
                    width: ColumnWidth::Fixed(12.0),
                    align: ColumnAlign::Left,
                })
                .collect(),
            rows: vec![DataRow {
                cells: vec![
                    StyledText::plain("a-one"),
                    StyledText::plain("b-two"),
                    StyledText::plain("c-three"),
                    StyledText::plain("d-four"),
                ],
                decoration: Decoration::Normal,
            }],
            selected_idx: None,
            scroll_offset: 0,
            sort: None,
            has_focus: false,
            show_scrollbar: false,
            min_total_width: Some(48.0),
            h_scroll: 0.0,
            column_overrides: Vec::new(),
            footer: None,
        }
    }

    /// The core #550 round-trip: whatever header title the rasteriser
    /// paints at a given screen cell, hit-testing that cell must name the
    /// same column — at every scroll offset, including one large enough
    /// to push the first column fully off-screen.
    #[test]
    fn h_scrolled_header_click_resolves_to_the_painted_column() {
        let area = Rect::new(0, 0, 24, 6);
        // 0 → nothing scrolled; 14 → "Alpha" partially off; 30 → "Alpha"
        // and "Bravo" both fully off-screen.
        for h_scroll in [0.0_f32, 6.0, 14.0, 30.0] {
            let mut table = make_h_scrolling_table();
            table.h_scroll = h_scroll;
            let mut buf = Buffer::empty(area);
            let layout = draw_data_table(&mut buf, area, &table, &Theme::default(), None);

            let header: String = (0..area.width)
                .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
                .collect();

            let mut checked = 0;
            for (col_idx, col) in table.columns.iter().enumerate() {
                let Some(pos) = find_char_pos(&header, &col.title) else {
                    continue; // title scrolled off (or clipped) — nothing to click
                };
                checked += 1;
                // Click the last painted char of the title: still inside
                // the column, and for a fully-visible title far enough
                // from the left edge that the previous column's divider
                // grab zone can't claim it.
                let x = pos + col.title.chars().count() - 1;
                assert_eq!(
                    layout.hit_test(x as f32 + 0.5, 0.5, 0, table.rows.len()),
                    DataTableHit::Header { col: col_idx },
                    "h_scroll={h_scroll}: cell {x} paints '{}' but hit-tests elsewhere\n\
                     header: {header:?}",
                    col.title
                );
            }
            assert!(
                checked > 0,
                "h_scroll={h_scroll} painted no fully-visible header title — the assertion \
                 loop above would be vacuous\nheader: {header:?}"
            );
        }
    }

    #[test]
    fn h_scrolled_body_cell_click_resolves_to_the_painted_column() {
        let area = Rect::new(0, 0, 24, 6);
        for h_scroll in [0.0_f32, 6.0, 14.0, 30.0] {
            let mut table = make_h_scrolling_table();
            table.h_scroll = h_scroll;
            let mut buf = Buffer::empty(area);
            let layout = draw_data_table(&mut buf, area, &table, &Theme::default(), None);

            let row: String = (0..area.width)
                .map(|x| buf[(x, 1)].symbol().chars().next().unwrap_or(' '))
                .collect();

            let mut checked = 0;
            for (col_idx, cell) in table.rows[0].cells.iter().enumerate() {
                let text: String = cell.spans.iter().map(|s| s.text.as_str()).collect();
                let Some(pos) = find_char_pos(&row, &text) else {
                    continue;
                };
                checked += 1;
                // The body band still routes to a row...
                assert_eq!(
                    layout.hit_test(pos as f32 + 0.5, 1.5, 0, table.rows.len()),
                    DataTableHit::Row { idx: 0 },
                    "h_scroll={h_scroll}: body click should stay row 0"
                );
                // ...and `column_hit` names the cell painted there.
                assert_eq!(
                    layout.column_hit(pos as f32 + 0.5),
                    Some(col_idx),
                    "h_scroll={h_scroll}: cell {pos} paints '{text}' but column_hit disagrees\n\
                     row: {row:?}"
                );
            }
            assert!(
                checked > 0,
                "h_scroll={h_scroll} painted no fully-visible body cell — the assertion loop \
                 above would be vacuous\nrow: {row:?}"
            );
        }
    }

    // ── #550 round 2: raw integer pointer coordinates, fractional h_scroll ──
    //
    // Every test above (and the primitive-layer sweep in
    // `primitives::data_table::tests`) drives `hit_test` with cell-centre
    // coordinates (`cell as f32 + 0.5`). Real TUI pointer input is never
    // that: `tui::events` builds it as
    // `Point::new(event.column as f32, event.row as f32)` — a raw
    // integer cell index. The two only coincide when `h_scroll`'s
    // fractional part is `< 0.5`; above that, painting rounds up a full
    // cell while a `+ 0.5` click coordinate still lands inside the
    // "old" rounding bucket and hides the gap. These tests use raw
    // integer `x`, matching real input, at deliberately fractional
    // `h_scroll` values whose fractional part is `>= 0.5`.

    /// Exactly the geometry from the #550 round-2 review: 4 ×
    /// `Fixed(30.0)` columns (content boundaries 0/30/60/90/120) inside a
    /// 60-wide viewport.
    fn make_wide_hscroll_table() -> DataTable {
        let titles = ["Alpha", "Bravo", "Charlie", "Delta"];
        DataTable {
            id: WidgetId::new("wide-hscroll"),
            columns: titles
                .iter()
                .map(|t| Column {
                    title: (*t).into(),
                    width: ColumnWidth::Fixed(30.0),
                    align: ColumnAlign::Left,
                })
                .collect(),
            rows: vec![DataRow {
                cells: vec![
                    StyledText::plain("a-one"),
                    StyledText::plain("b-two"),
                    StyledText::plain("c-three"),
                    StyledText::plain("d-four"),
                ],
                decoration: Decoration::Normal,
            }],
            selected_idx: None,
            scroll_offset: 0,
            sort: None,
            has_focus: false,
            show_scrollbar: false,
            min_total_width: Some(120.0),
            h_scroll: 0.0,
            column_overrides: Vec::new(),
            footer: None,
        }
    }

    #[test]
    fn raw_integer_click_at_fractional_h_scroll_hits_the_painted_column() {
        // The exact repro from the #550 round-2 review: h_scroll = 15.6
        // → h_off = round(15.6) = 16. The renderer paints column c1
        // (content [30,60)) at screen [14,44). A real click at integer
        // screen x=14 is squarely on c1 as painted, and `column_hit` —
        // the divider-agnostic "which column is painted here" query — is
        // the API the review's repro exercises directly (it has no
        // resize-grab zone to land in, unlike `hit_test`'s header
        // branch; see the next test for that case).
        let area = Rect::new(0, 0, 60, 3);
        let mut table = make_wide_hscroll_table();
        table.h_scroll = 15.6;
        let mut buf = Buffer::empty(area);
        let layout = draw_data_table(&mut buf, area, &table, &Theme::default(), None);

        assert_eq!(layout.h_scroll, 16.0, "TUI layout must carry the rounded offset");

        // Raw integer coordinate (no `+ 0.5`), exactly as `tui::events`
        // constructs a real pointer position.
        assert_eq!(
            layout.column_hit(14.0),
            Some(1),
            "integer click x=14 sits on c1's painted left edge at h_scroll=15.6"
        );

        // The pre-fix bug: adding the *un-rounded* 15.6 back would put
        // this at content x=29.6, inside c0's [0,30) — never c1's.
        assert_ne!(layout.column_hit(14.0), Some(0));

        // The header branch of `hit_test` sits at the exact same screen
        // x, but content x=30 lands precisely on the c0/c1 divider's
        // grab zone (`DIVIDER_GRAB_PX = 3.0`), so per the review's own
        // analysis it resolves to `HeaderDivider` rather than either
        // header — never the *wrong* header (`Header { col: 0 }`, the
        // original defect this fix closes).
        assert_eq!(
            layout.hit_test(14.0, 0.0, 0, table.rows.len()),
            DataTableHit::HeaderDivider { col: 0 },
            "residual at an exact boundary falls in the pre-existing divider grab zone"
        );
        assert_ne!(
            layout.hit_test(14.0, 0.0, 0, table.rows.len()),
            DataTableHit::Header { col: 0 },
        );
    }

    #[test]
    fn raw_integer_body_click_at_fractional_h_scroll_hits_the_painted_column() {
        // Same geometry, but a body-row click — the "cell hit" half of
        // the same repro.
        let area = Rect::new(0, 0, 60, 3);
        let mut table = make_wide_hscroll_table();
        table.h_scroll = 15.6;
        let mut buf = Buffer::empty(area);
        let layout = draw_data_table(&mut buf, area, &table, &Theme::default(), None);

        assert_eq!(
            layout.hit_test(14.0, 1.0, 0, table.rows.len()),
            DataTableHit::Row { idx: 0 },
            "row resolution is unaffected by h_scroll rounding"
        );
        assert_eq!(layout.column_hit(14.0), Some(1));
    }

    #[test]
    fn raw_integer_header_click_at_fractional_h_scroll_does_not_silently_arm_a_resize_drag() {
        // The header-click half of the same repro, at several fractional
        // offsets whose fractional part is >= 0.5: the residual always
        // falls inside DIVIDER_GRAB_PX of *some* boundary (by
        // construction — see the module doc), so per #550's own
        // analysis this resolves to `HeaderDivider`, not a wrong
        // `Header`. This test pins that this is the ONLY acceptable
        // non-exact-header outcome — never a header for a different,
        // wrong column (the original defect: a sort click silently
        // hitting the wrong column instead of silently arming a
        // resize-drag).
        let area = Rect::new(0, 0, 60, 3);
        for h_scroll in [15.6_f32, 45.7, 75.9] {
            let mut table = make_wide_hscroll_table();
            table.h_scroll = h_scroll;
            let mut buf = Buffer::empty(area);
            let layout = draw_data_table(&mut buf, area, &table, &Theme::default(), None);
            let h_off = h_scroll.round();

            for x in 0..60u16 {
                let hit = layout.hit_test(x as f32, 0.0, 0, table.rows.len());
                if let DataTableHit::Header { col } = hit {
                    // The column this integer x resolves to must be the
                    // column actually painted there — content x is
                    // `x + h_off` (the rounded offset baked into
                    // `layout.h_scroll`).
                    let content_x = x as f32 + h_off;
                    let painted = layout
                        .columns
                        .iter()
                        .position(|rc| content_x >= rc.x && content_x < rc.x + rc.width);
                    assert_eq!(
                        painted,
                        Some(col),
                        "h_scroll={h_scroll}: integer x={x} resolved to Header{{col: {col}}} \
                         but the renderer paints column {painted:?} there"
                    );
                }
            }
        }
    }

    #[test]
    fn h_scrolled_header_divider_click_resolves_to_the_painted_separator() {
        let area = Rect::new(0, 0, 24, 6);
        for h_scroll in [0.0_f32, 6.0, 14.0, 30.0] {
            let mut table = make_h_scrolling_table();
            table.h_scroll = h_scroll;
            let mut buf = Buffer::empty(area);
            let layout = draw_data_table(&mut buf, area, &table, &Theme::default(), None);

            // The rasteriser paints '│' on the right edge cell of every
            // column but the last, i.e. one cell *before* the boundary
            // `hit_test` measures the grab zone from.
            let painted_seps: Vec<u16> = (0..area.width)
                .filter(|&x| buf[(x, 0)].symbol() == "│")
                .collect();
            assert!(
                !painted_seps.is_empty(),
                "h_scroll={h_scroll}: expected at least one painted separator"
            );
            for sep_x in painted_seps {
                let hit = layout.hit_test(sep_x as f32 + 0.5, 0.5, 0, table.rows.len());
                let DataTableHit::HeaderDivider { col } = hit else {
                    panic!(
                        "h_scroll={h_scroll}: separator painted at x={sep_x} must hit-test as a \
                         divider, got {hit:?}"
                    );
                };
                // Round-trip the column index back to the boundary it
                // owns and confirm it is the one painted here.
                let boundary = layout.columns[col].x + layout.columns[col].width;
                assert!(
                    ((boundary - h_scroll) - (sep_x as f32 + 1.0)).abs() < 0.01,
                    "h_scroll={h_scroll}: separator at x={sep_x} resolved to divider {col}, \
                     whose boundary paints at {}",
                    boundary - h_scroll
                );
            }
        }
    }

    #[test]
    fn vertical_scrollbar_track_excludes_footer_band() {
        // Regression guard: the vertical scrollbar's track must span
        // only the body band (header..footer), not header..bottom-of
        // -viewport. With area.height=10, header_height=1 and a footer
        // present (footer_height=2), the track should be exactly
        // `visible_rows` cells tall — not `area.height - 1` (9).
        let mut table = make_scrolling_table(50);
        table.show_scrollbar = true;
        table.footer = Some(DataRow {
            cells: vec![StyledText::plain("TOTAL")],
            decoration: Decoration::Normal,
        });
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let layout = draw_data_table(&mut buf, area, &table, &Theme::default(), None);
        assert_eq!(layout.footer_height, 2.0);
        assert_eq!(layout.visible_rows, 7);

        // Scrollbar column sits at the right edge of the area.
        let sb_x = area.width - 1;
        let track_cells = (0..area.height)
            .filter(|&y| matches!(buf[(sb_x, y)].symbol(), "█" | "░"))
            .count();
        assert_eq!(
            track_cells, layout.visible_rows,
            "scrollbar track should span exactly the body band (visible_rows), \
             not bleed into the reserved footer band"
        );

        // The footer band itself (last `footer_height` rows) must be
        // free of scrollbar track/thumb glyphs.
        for y in (area.height - 2)..area.height {
            let sym = buf[(sb_x, y)].symbol();
            assert_ne!(
                sym, "█",
                "footer row {y} should not be painted over by the scrollbar thumb"
            );
            assert_ne!(
                sym, "░",
                "footer row {y} should not be painted over by the scrollbar track"
            );
        }
    }
}
