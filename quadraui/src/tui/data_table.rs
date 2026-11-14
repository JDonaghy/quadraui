//! TUI rasteriser for [`crate::DataTable`].
//!
//! Renders column headers with sort indicators, then body rows with
//! per-cell text aligned within resolved column bounds. Selected row
//! uses `theme.selection_bg`. Focused table highlights the selected
//! row more prominently.
//!
//! **Border** (`table.bordered`): a rounded `╭─╮ │ ╰─╯` frame in
//! [`Theme::border_fg`] around `area`, insetting the header, every row,
//! the footer and both scrollbars by 1 cell on each side — the same
//! margin [`DataTable::layout`](crate::primitives::data_table::DataTable::layout)
//! reserves in `visible_rows`/`inset_x`/`inset_y`, so a host that always
//! sets this can safely overlay a focus ring on `area` without it ever
//! landing on the header, first column or last row.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;

use super::{ratatui_color, set_cell, set_cell_wide};
use crate::primitives::data_table::{
    ColumnAlign, ColumnMeasure, DataTable, DataTableGlyphs, DataTableLayout, DataTablePaintState,
    SortDirection,
};
use crate::theme::Theme;

/// Resolve the layout `draw_data_table` paints `table` at, without
/// touching a buffer.
///
/// This is the **single** definition of TUI DataTable geometry: both
/// [`draw_data_table`] and `Backend::data_table_layout` go through it,
/// so the layout a cache-free, layout-on-demand hit-test runs against is
/// bit-for-bit the layout that was painted.
pub fn data_table_layout(area: Rect, table: &DataTable) -> DataTableLayout {
    let mut layout = table.layout(
        area.width as f32,
        area.height as f32,
        1.0,
        1.0,
        1.0,
        |col| ColumnMeasure::new(col.title.chars().count() as f32),
    );

    // TUI is cell-granular: every column paints at a *rounded* offset,
    // not the raw fractional `h_scroll`. `hit_test` / `column_hit` do no
    // rounding of their own — they trust `DataTableLayout::h_scroll` to
    // already equal what was painted — so this must be rounded here, once,
    // before the layout is handed back to callers for hit-testing.
    layout.h_scroll = table.h_scroll.round();
    layout
}

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
    draw_data_table_with_glyphs(buf, area, table, theme, hovered_idx, None, false)
}

/// [`draw_data_table`] with an optional app-supplied [`DataTableGlyphs`]
/// override for the column-header sort-direction indicators.
/// `nerd_fonts_enabled` selects which half of each override [`Icon`][crate::Icon] —
/// `glyph` or `fallback` — is resolved (see
/// [`crate::Backend::nerd_fonts_enabled`]); it is ignored when `glyphs`
/// does not set the indicator being drawn.
///
/// `glyphs: None` reproduces [`draw_data_table`] cell for cell — the
/// default `▲`/`▼` indicators are unchanged. Passing `Some` only
/// overrides the glyph it sets; a field left `None` keeps the default for
/// that direction. Unlike the GUI backends' codicon defaults
/// (`crate::codicon::SORT_ASCENDING`/`SORT_DESCENDING`), TUI's own default
/// is the plain Unicode arrow — this function preserves that default
/// rather than adopting the codicon one.
pub fn draw_data_table_with_glyphs(
    buf: &mut Buffer,
    area: Rect,
    table: &DataTable,
    theme: &Theme,
    hovered_idx: Option<usize>,
    glyphs: Option<&DataTableGlyphs>,
    nerd_fonts_enabled: bool,
) -> DataTableLayout {
    draw_data_table_inner(
        buf,
        area,
        table,
        theme,
        hovered_idx,
        glyphs,
        nerd_fonts_enabled,
        None,
    )
}

/// [`draw_data_table`], plus a pixel-precise scroll position via
/// [`DataTablePaintState::scroll_px`] instead of only
/// [`DataTable::scroll_offset`]'s whole-row value.
///
/// TUI is a cell grid: `scroll_px` wins over `scroll_offset` (see
/// [`DataTable::effective_scroll_offset`]) but is quantised to a whole
/// row — the sub-row remainder is computed and then simply never read,
/// so TUI never paints a partial row. `state.scroll_px: None` (the
/// default a `state` built with [`DataTablePaintState::default`]
/// carries) resolves to exactly the pre-existing whole-row behaviour.
pub fn draw_data_table_with_state(
    buf: &mut Buffer,
    area: Rect,
    table: &DataTable,
    theme: &Theme,
    state: &DataTablePaintState,
) -> DataTableLayout {
    draw_data_table_inner(
        buf,
        area,
        table,
        theme,
        state.hovered_idx,
        None,
        false,
        state.scroll_px,
    )
}

#[allow(clippy::too_many_arguments)]
fn draw_data_table_inner(
    buf: &mut Buffer,
    area: Rect,
    table: &DataTable,
    theme: &Theme,
    hovered_idx: Option<usize>,
    glyphs: Option<&DataTableGlyphs>,
    nerd_fonts_enabled: bool,
    scroll_px: Option<f32>,
) -> DataTableLayout {
    let layout = data_table_layout(area, table);

    if area.width == 0 || area.height == 0 {
        return layout;
    }

    let header_bg = ratatui_color(theme.tab_bar_bg);
    let header_fg = ratatui_color(theme.foreground);
    let body_bg = ratatui_color(theme.background);
    let body_fg = ratatui_color(theme.foreground);
    let sel_bg = ratatui_color(theme.selection_bg);
    let muted_fg = ratatui_color(theme.muted_fg);

    // `table.bordered` reserves a 1-cell margin on every edge — see
    // `DataTable::bordered`'s doc — so the header, every row, the footer
    // and both scrollbars paint inset from `area` by this much, matching
    // the inset `DataTable::layout` already baked into `layout.inset_x`/
    // `layout.inset_y` for hit-testing. `0` when not bordered, so every
    // position below is an identity add-on to the pre-existing behaviour.
    let (ox, oy): (u16, u16) = if table.bordered { (1, 1) } else { (0, 0) };
    if table.bordered {
        draw_data_table_border(buf, area, ratatui_color(theme.border_fg), body_fg, body_bg);
    }
    // Interior bounds (right edge exclusive) every paint position below
    // clips to, so nothing ever lands on the border frame itself.
    let inner_x0 = area.x as i32 + ox as i32;
    let inner_x1 = area.x as i32 + area.width as i32 - ox as i32;

    // ── Header row ────────────────────────────────────────────────────
    let header_y = area.y + oy;
    for x in ox..area.width.saturating_sub(ox) {
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
        let col_x_raw = inner_x0 + rc.x.round() as i32 - h_off as i32;
        let col_x_end = col_x_raw + rc.width.round() as i32;
        if col_x_end <= inner_x0 || col_x_raw >= inner_x1 {
            continue;
        }
        let col_w = rc.width.round() as u16;
        if col_w == 0 {
            continue;
        }

        // Column separator on the right edge (skip last column).
        if col_idx + 1 < table.columns.len() {
            let sep_cx = col_x_raw + col_w as i32 - 1;
            if sep_cx >= inner_x0 && sep_cx < inner_x1 {
                set_cell(buf, sep_cx as u16, header_y, '│', sep_fg, header_bg);
            }
        }

        let sort_glyph: Option<&str> = match &table.sort {
            Some((si, dir)) if *si == col_idx => Some(match dir {
                SortDirection::Ascending => glyphs
                    .and_then(DataTableGlyphs::sort_ascending_glyph)
                    .map(|icon| icon.resolve(nerd_fonts_enabled))
                    .unwrap_or("▲"),
                SortDirection::Descending => glyphs
                    .and_then(DataTableGlyphs::sort_descending_glyph)
                    .map(|icon| icon.resolve(nerd_fonts_enabled))
                    .unwrap_or("▼"),
            }),
            _ => None,
        };
        let title = match sort_glyph {
            Some(glyph) => format!("{} {glyph}", col.title),
            None => col.title.clone(),
        };
        let text_len = title.chars().count() as i32;
        let usable_w = if col_idx + 1 < table.columns.len() {
            col_w.saturating_sub(1) as i32
        } else {
            col_w as i32
        };
        let start = align_offset(col.align, text_len as u16, usable_w as u16) as i32;

        for (i, ch) in title.chars().enumerate() {
            let cx = col_x_raw + start + i as i32;
            if cx >= col_x_raw + usable_w || cx >= inner_x1 {
                break;
            }
            if cx >= inner_x0 {
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
    // `scroll_px` wins over `scroll_offset` when set, quantised to a
    // whole row here — TUI is cell-granular, so the sub-row remainder
    // `scroll_sub_row_px` pairs with `effective_scroll_offset` for a
    // pixel-precise painter is simply never read on this backend.
    let scroll_offset = table.effective_scroll_offset(layout.row_height, scroll_px);
    let body_y = area.y + oy + 1;
    let visible = layout
        .visible_rows
        .min(table.rows.len().saturating_sub(scroll_offset));

    for row_idx in 0..visible {
        let abs_idx = scroll_offset + row_idx;
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
        for x in ox..area.width.saturating_sub(ox) {
            set_cell(buf, area.x + x, y, ' ', row_fg, row_bg);
        }

        for (col_idx, rc) in layout.columns.iter().enumerate() {
            let col_x_raw = inner_x0 + rc.x.round() as i32 - h_off as i32;
            let col_x_end = col_x_raw + rc.width.round() as i32;
            if col_x_end <= inner_x0 || col_x_raw >= inner_x1 {
                continue;
            }
            let col_w = rc.width.round() as u16;
            if col_w == 0 {
                continue;
            }

            // Column separator on the right edge (skip last column) —
            // drawn regardless of whether this cell has text, mirroring
            // the header separator.
            if col_idx + 1 < table.columns.len() {
                let sep_cx = col_x_raw + col_w as i32 - 1;
                if sep_cx >= inner_x0 && sep_cx < inner_x1 {
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
            // column boundary instead of painting across its neighbours.
            let usable_w: u16 = if col_idx + 1 < table.columns.len() {
                col_w.saturating_sub(1)
            } else {
                col_w
            };

            // Measure and clip by terminal display width, not char count:
            // a char-count budget lets a CJK/emoji run overflow its column
            // (each wide glyph claims two cells, not one) and never adds an
            // ellipsis within budget. `visible_chars` is how many of
            // `full_text`'s *characters* to paint; `displayed_len` is their
            // actual cell width (plus the ellipsis's one cell, if needed) —
            // the two diverge whenever a wide glyph is in play, which is why
            // `displayed_len` (not `visible_chars`) feeds `align_offset`.
            let text_width = crate::text_util::display_width(&full_text) as u16;
            let (visible_chars, needs_ellipsis, displayed_len): (u16, bool, u16) = if text_width
                <= usable_w
            {
                (full_text.chars().count() as u16, false, text_width)
            } else if usable_w == 0 {
                (0, false, 0)
            } else {
                let (fitted, fitted_w) =
                    crate::text_util::truncate_with_ellipsis(&full_text, usable_w as f32, |s| {
                        crate::text_util::display_width(s) as f32
                    });
                if fitted.is_empty() {
                    (0, false, 0)
                } else {
                    // `fitted` is always `<content>…` here — strip the
                    // trailing ellipsis char to get the content count.
                    let content_chars = fitted.chars().count() as u16 - 1;
                    (content_chars, true, fitted_w.round() as u16)
                }
            };
            let start = align_offset(align, displayed_len, usable_w) as i32;

            let is_muted = row.decoration == crate::types::Decoration::Muted;
            let mut chars_emitted = 0u16;
            let mut width_used = 0u16;
            'cell: for span in &styled.spans {
                let span_fg = if is_muted {
                    muted_fg
                } else {
                    span.fg.map(ratatui_color).unwrap_or(row_fg)
                };
                for ch in span.text.chars() {
                    if chars_emitted == visible_chars {
                        if needs_ellipsis {
                            let cx = col_x_raw + start + width_used as i32;
                            if cx >= inner_x0 && cx < inner_x1 {
                                set_cell(buf, cx as u16, y, '…', span_fg, row_bg);
                            }
                        }
                        break 'cell;
                    }
                    let cx = col_x_raw + start + width_used as i32;
                    if cx >= inner_x1 {
                        break 'cell;
                    }
                    let w = crate::text_util::char_cell_width(ch);
                    if cx >= inner_x0 {
                        if w == 2 && cx + 1 < inner_x1 {
                            set_cell_wide(buf, cx as u16, y, ch, span_fg, row_bg);
                        } else {
                            set_cell(buf, cx as u16, y, ch, span_fg, row_bg);
                        }
                    }
                    width_used += w;
                    chars_emitted += 1;
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
        let sb_x = area.x + area.width - ox - layout.scrollbar_width.round() as u16;
        let sb_track = crate::event::Rect::new(
            sb_x as f32,
            (area.y + oy + 1) as f32,
            1.0,
            (area
                .height
                .saturating_sub(2 * oy)
                .saturating_sub(1)
                .saturating_sub(footer_h)) as f32,
        );
        let sb = crate::primitives::scrollbar::Scrollbar::vertical(
            table.id,
            sb_track,
            scroll_offset as f32,
            table.rows.len() as f32,
            layout.visible_rows as f32,
            1.0,
        );
        super::draw_scrollbar(buf, &sb, theme, theme.background);
    }

    // ── Horizontal scrollbar ─────────────────────────────────────────
    if layout.h_scrollbar_height > 0.0 && layout.content_width > 0.0 {
        let hsb_y = area.y + area.height - oy - footer_h - layout.h_scrollbar_height.round() as u16;
        let track_w = (area.width as f32 - 2.0 * ox as f32 - layout.scrollbar_width).max(1.0);
        let hsb_track = crate::event::Rect::new((area.x + ox) as f32, hsb_y as f32, track_w, 1.0);
        let visible_w = (area.width as f32 - 2.0 * ox as f32 - layout.scrollbar_width).max(1.0);
        let hsb = crate::primitives::scrollbar::Scrollbar::horizontal(
            table.id,
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
            let footer_y = area.y + area.height - 1 - oy;
            let footer_bg = ratatui_color(theme.tab_bar_bg);
            let footer_fg = ratatui_color(theme.foreground);

            // Divider rule directly above the footer row.
            if footer_y > area.y {
                for x in ox..area.width.saturating_sub(ox) {
                    set_cell(buf, area.x + x, footer_y - 1, '─', sep_fg, header_bg);
                }
            }

            for x in ox..area.width.saturating_sub(ox) {
                set_cell(buf, area.x + x, footer_y, ' ', footer_fg, footer_bg);
            }

            for (col_idx, rc) in layout.columns.iter().enumerate() {
                let styled = match footer.cells.get(col_idx) {
                    Some(c) if !c.spans.is_empty() => c,
                    _ => continue,
                };
                let full_text: String = styled.spans.iter().map(|s| s.text.as_str()).collect();
                let col_x_raw = inner_x0 + rc.x.round() as i32 - h_off as i32;
                let col_x_end = col_x_raw + rc.width.round() as i32;
                if col_x_end <= inner_x0 || col_x_raw >= inner_x1 {
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
                        if cx >= inner_x1 {
                            break;
                        }
                        if cx >= inner_x0 {
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

/// Paint a rounded border frame (`╭─╮ │ ╰─╯`) around `area` in `border_fg`
/// on `bg`, filling every interior cell with `(fg, bg)` first — reserving
/// the 1-cell margin [`DataTable::layout`](crate::primitives::data_table::DataTable::layout)
/// insets the header, rows, footer and scrollbars into when
/// [`DataTable::bordered`](crate::primitives::data_table::DataTable::bordered)
/// is set. Mirrors `tui::tree::draw_tree_border`'s glyphs plus the
/// interior fill `tui::list::draw_list`'s own bordered branch applies, so
/// a focus ring painted over the same `area` afterwards only ever
/// recolours the frame glyphs, never the header/row content painted on
/// top of this fill.
fn draw_data_table_border(
    buf: &mut Buffer,
    area: Rect,
    border_fg: ratatui::style::Color,
    fg: ratatui::style::Color,
    bg: ratatui::style::Color,
) {
    let top_y = area.y;
    for col in 0..area.width {
        let cx = area.x + col;
        let ch = if col == 0 {
            '╭'
        } else if col + 1 == area.width {
            '╮'
        } else {
            '─'
        };
        set_cell(buf, cx, top_y, ch, border_fg, bg);
    }
    if area.height >= 2 {
        let bot_y = area.y + area.height - 1;
        for col in 0..area.width {
            let cx = area.x + col;
            let ch = if col == 0 {
                '╰'
            } else if col + 1 == area.width {
                '╯'
            } else {
                '─'
            };
            set_cell(buf, cx, bot_y, ch, border_fg, bg);
        }
    }
    if area.height >= 3 {
        for row in (area.y + 1)..(area.y + area.height - 1) {
            set_cell(buf, area.x, row, '│', border_fg, bg);
            set_cell(buf, area.x + area.width - 1, row, '│', border_fg, bg);
            for col in 1..area.width.saturating_sub(1) {
                set_cell(buf, area.x + col, row, ' ', fg, bg);
            }
        }
    }
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
    /// screen column as soon as a row contains a multi-byte glyph.
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
                    width: ColumnWidth::FixedCells(5.0),
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
            bordered: false,
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

    /// An app-supplied [`DataTableGlyphs`] override replaces the
    /// sort-direction indicator appended to the sorted column's header;
    /// with no override, [`draw_data_table_with_glyphs`] (what
    /// [`draw_data_table`] always calls) appends the same `▲`/`▼` as
    /// today's default.
    #[test]
    fn data_table_glyphs_override_swaps_sort_arrow_default_keeps_arrow() {
        let area = Rect::new(0, 0, 40, 10);

        // Default (no override): ascending sort shows '▲'.
        let table = make_table();
        let mut buf = Buffer::empty(area);
        draw_data_table_with_glyphs(&mut buf, area, &table, &Theme::default(), None, None, false);
        let header: String = (0..40)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            header.contains('▲'),
            "expected default '▲' sort glyph, got: {header}"
        );

        // Override: the app's own ascending glyph replaces '▲', and '▲'
        // no longer appears in the header.
        let glyphs = DataTableGlyphs::new().with_sort_ascending_glyph("^");
        let mut buf = Buffer::empty(area);
        draw_data_table_with_glyphs(
            &mut buf,
            area,
            &table,
            &Theme::default(),
            None,
            Some(&glyphs),
            false,
        );
        let header: String = (0..40)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            !header.contains('▲'),
            "default '▲' sort glyph leaked through override, got: {header}"
        );
        assert!(
            header.contains('^'),
            "expected overridden '^' sort glyph, got: {header}"
        );

        // Descending sort with no override still shows the default '▼'.
        let mut desc_table = make_table();
        desc_table.sort = Some((0, SortDirection::Descending));
        let mut buf = Buffer::empty(area);
        draw_data_table_with_glyphs(
            &mut buf,
            area,
            &desc_table,
            &Theme::default(),
            None,
            None,
            false,
        );
        let header: String = (0..40)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            header.contains('▼'),
            "expected default '▼' sort glyph, got: {header}"
        );

        // Override: the app's own descending glyph replaces '▼', and '▼'
        // no longer appears in the header.
        let desc_glyphs = DataTableGlyphs::new().with_sort_descending_glyph("v");
        let mut buf = Buffer::empty(area);
        draw_data_table_with_glyphs(
            &mut buf,
            area,
            &desc_table,
            &Theme::default(),
            None,
            Some(&desc_glyphs),
            false,
        );
        let header: String = (0..40)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            !header.contains('▼'),
            "default '▼' sort glyph leaked through override, got: {header}"
        );
        assert!(
            header.contains('v'),
            "expected overridden 'v' sort glyph, got: {header}"
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

    #[test]
    fn body_rows_draw_column_separator_at_same_x_as_header() {
        // Column separator is drawn in body rows at the same position as
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
        // An over-long column cell is clipped to its own boundary, not to
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
            bordered: false,
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
    fn body_cell_with_wide_glyphs_truncates_by_display_width_not_char_count() {
        // A 6-column-wide cell holding 6 CJK chars (12 display columns)
        // must truncate to fit the column's *display* width, not its char
        // count — a char-count budget would paint all 6 chars (12 cells)
        // across the neighbour column with no ellipsis at all.
        let table = DataTable {
            id: WidgetId::new("wide-glyph-test"),
            columns: vec![
                Column {
                    title: "A".into(),
                    width: ColumnWidth::FixedCells(6.0),
                    align: ColumnAlign::Left,
                },
                Column {
                    title: "B".into(),
                    width: ColumnWidth::FixedCells(4.0),
                    align: ColumnAlign::Left,
                },
            ],
            rows: vec![DataRow {
                cells: vec![StyledText::plain("日本語日本語"), StyledText::plain("tail")],
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
            bordered: false,
        };
        let area = Rect::new(0, 0, 10, 10);
        let mut buf = Buffer::empty(area);
        let layout = draw_data_table(&mut buf, area, &table, &Theme::default(), None);

        let row: Vec<char> = (0..10)
            .map(|x| buf[(x, 1)].symbol().chars().next().unwrap_or(' '))
            .collect();
        let row_str: String = row.iter().collect();

        let b_col = &layout.columns[1];
        let b_start = b_col.x.round() as usize;
        assert!(b_start > 0, "column B must not start at x=0: {row_str:?}");

        // Column A's cell (reserving one cell for the separator) never
        // overflows into column B's start — only an ellipsis (or blank
        // padding) appears in the cell right before B, never a CJK glyph
        // that would occupy two columns and collide with its neighbour.
        assert_ne!(
            row[b_start - 1],
            '語',
            "a wide glyph must not be painted into the separator cell: {row_str:?}"
        );
        assert!(
            (0..b_start).any(|x| row[x] == '…'),
            "over-wide CJK cell should end in an ellipsis: {row_str:?}"
        );
        // Column B's own content paints intact, undisturbed by A's overflow.
        assert_eq!(row[b_start], 't', "row: {row_str:?}");

        // The three assertions above hold under a char-count budget too
        // (it also stops at the separator and also ends in '…' at this
        // column width), so they alone don't pin the display-width fix.
        // What distinguishes the two: a char-count budget treats each of
        // the 2-cell-wide glyphs as 1 cell and packs four of them — 日,
        // 本, 語, 日 — into columns 0..4 before the ellipsis, so column 2
        // holds '語'. A display-width budget fits only two whole glyphs
        // (日, 本 — 4 cells) before the ellipsis, leaving each glyph's
        // second cell as the wide-char continuation marker that
        // `set_cell_wide` writes, so column 2 holds '本' and column 1 is
        // the empty continuation of column 0's glyph.
        assert_eq!(
            row[2], '本',
            "a display-width budget fits only 2 whole glyphs before the ellipsis, \
             not 4 char-counted ones: {row_str:?}"
        );
        assert_eq!(
            buf[(1, 1)].symbol(),
            "",
            "column 1 must be the wide-char continuation cell of column 0's glyph, \
             not a second glyph painted one-char-per-cell"
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

    /// LESSONS.md non-zero-origin regression guard:
    /// `data_table_layout` never reads `area.x`/`area.y` (see its doc
    /// comment) — it, and therefore `hit_test`, is table-**local**, unlike
    /// most of this batch's other layout fns which bake the origin in.
    /// Painting still happens at the absolute `area.x`/`area.y` (mirrors
    /// `tui/activity_bar.rs`'s relative-hit-regions / absolute-paint split),
    /// so this scans the buffer at the shifted absolute position but keeps
    /// the resulting index — and the `hit_test` call — table-local.
    #[test]
    fn paint_click_round_trip_header_at_nonzero_origin() {
        let table = make_table();
        let area = Rect::new(7, 13, 40, 10);
        let mut buf = Buffer::empty(area);
        let layout = draw_data_table(&mut buf, area, &table, &Theme::default(), None);

        let header: String = (area.x..area.x + 40)
            .map(|x| buf[(x, area.y)].symbol().chars().next().unwrap_or(' '))
            .collect();
        let status_pos = header.find("Status").expect("Status should be in header");
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

    /// Non-zero-origin sibling of `paint_click_round_trip_row` — see
    /// `paint_click_round_trip_header_at_nonzero_origin`.
    #[test]
    fn paint_click_round_trip_row_at_nonzero_origin() {
        let table = make_table();
        let area = Rect::new(7, 13, 40, 10);
        let mut buf = Buffer::empty(area);
        let layout = draw_data_table(&mut buf, area, &table, &Theme::default(), None);

        // Row 1 paints on the *absolute* screen row `area.y + 2`.
        let row1: String = (area.x..area.x + 40)
            .map(|x| buf[(x, area.y + 2)].symbol().chars().next().unwrap_or(' '))
            .collect();
        let pod_pos = row1.find("pod-xyz").expect("pod-xyz should be in row 1");
        let hit = layout.hit_test(pod_pos as f32, 2.5, 0, table.rows.len());
        assert_eq!(hit, DataTableHit::Row { idx: 1 });
    }

    /// Direct "bounds do not move" guard, mirroring
    /// `tui/activity_bar.rs`'s `hit_regions_do_not_move_when_the_bar_does`:
    /// the resolved column bounds must be byte-for-byte identical no matter
    /// where `area` sits on screen, since `data_table_layout` is table-local
    /// by construction.
    #[test]
    fn column_bounds_do_not_move_when_the_table_does() {
        let table = make_table();
        let cols_at = |origin_x: u16, origin_y: u16| {
            let area = Rect::new(origin_x, origin_y, 40, 10);
            let mut buf = Buffer::empty(area);
            let layout = draw_data_table(&mut buf, area, &table, &Theme::default(), None);
            layout
                .columns
                .iter()
                .map(|c| (c.x, c.width))
                .collect::<Vec<_>>()
        };

        let at_zero = cols_at(0, 0);
        assert!(!at_zero.is_empty(), "sanity: some columns were resolved");
        assert_eq!(
            cols_at(7, 13),
            at_zero,
            "column bounds must be table-local, independent of area.x/area.y"
        );
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

    /// `scroll_px` wins over `scroll_offset`, but on
    /// TUI's cell grid (`row_height == 1.0`) it quantises to the same
    /// whole row a GUI backend's sub-row remainder would float between
    /// — the "TUI quantises scroll_px to whole rows" half of the
    /// issue's acceptance criterion.
    #[test]
    fn scroll_px_quantises_to_a_whole_row_on_tui() {
        let table = make_scrolling_table(50);
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let state = DataTablePaintState::default().with_scroll_px(Some(10.7)); // 10.7 cells deep → row 10, floored
        draw_data_table_with_state(&mut buf, area, &table, &Theme::default(), &state);

        let first_body_row: String = (0..40)
            .map(|x| buf[(x, 1)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            first_body_row.contains("pod-10"),
            "scroll_px=10.7 should quantise to row 10 at the top of the body, got: {first_body_row}"
        );

        // A whole-row scroll_offset of 10 paints byte-for-byte the same
        // first body row — scroll_px degrades to the pre-existing
        // row-quantised behaviour, it does not add a new visual.
        let mut via_offset = make_scrolling_table(50);
        via_offset.scroll_offset = 10;
        let mut buf2 = Buffer::empty(area);
        draw_data_table(&mut buf2, area, &via_offset, &Theme::default(), None);
        let first_body_row_via_offset: String = (0..40)
            .map(|x| buf2[(x, 1)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert_eq!(first_body_row, first_body_row_via_offset);
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
        // index, and body rows now paint a `'│'` separator which is 3 bytes
        // but exactly 1 cell — a byte-offset comparison would fail even
        // though the two cells line up.
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
        // With `footer: None`, the last paintable row is still a body row,
        // not blank footer space.
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
                    width: ColumnWidth::FixedCells(12.0),
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
            bordered: false,
        }
    }

    /// Whatever header title the rasteriser paints at a given screen cell,
    /// hit-testing that cell must name the same column — at every scroll
    /// offset, including one large enough to push the first column fully
    /// off-screen.
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

    // TUI pointer input is a raw integer cell index (`Point::new(event.column as f32,
    // event.row as f32)`), not cell-centre coordinates. Painting rounds fractional
    // `h_scroll` values; these tests use raw integer `x` at deliberately fractional
    // `h_scroll` values whose fractional part is `>= 0.5` to verify the layout stays
    // consistent across rounding.

    /// 4 × `Fixed(30.0)` columns (content boundaries 0/30/60/90/120) inside a
    /// 60-wide viewport.
    fn make_wide_hscroll_table() -> DataTable {
        let titles = ["Alpha", "Bravo", "Charlie", "Delta"];
        DataTable {
            id: WidgetId::new("wide-hscroll"),
            columns: titles
                .iter()
                .map(|t| Column {
                    title: (*t).into(),
                    width: ColumnWidth::FixedCells(30.0),
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
            bordered: false,
        }
    }

    #[test]
    fn raw_integer_click_at_fractional_h_scroll_hits_the_painted_column() {
        // At h_scroll = 15.6, h_off = round(15.6) = 16. The renderer paints column c1
        // (content [30,60)) at screen [14,44). A real click at integer screen x=14 is
        // squarely on c1 as painted, and `column_hit` (the divider-agnostic
        // "which column is painted here" query) resolves it correctly.
        let area = Rect::new(0, 0, 60, 3);
        let mut table = make_wide_hscroll_table();
        table.h_scroll = 15.6;
        let mut buf = Buffer::empty(area);
        let layout = draw_data_table(&mut buf, area, &table, &Theme::default(), None);

        assert_eq!(
            layout.h_scroll, 16.0,
            "TUI layout must carry the rounded offset"
        );

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
        // At fractional offsets whose fractional part is >= 0.5, the residual always
        // falls inside DIVIDER_GRAB_PX of some boundary, so the header click resolves
        // to `HeaderDivider` rather than a wrong `Header` column.
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

    // ── `DataTable::bordered` focus-ring inset ──────────────────────────

    /// With `table.bordered`, the header and row 0's content paint one
    /// cell inset from `area`'s top-left — the corner `draw_focus_ring`
    /// would otherwise overwrite stays blank (border glyph territory),
    /// not table content.
    #[test]
    fn bordered_insets_header_and_first_row_away_from_the_top_left_corner() {
        let area = Rect::new(0, 0, 40, 6);
        let mut buf = Buffer::empty(area);
        let mut table = make_table();
        table.bordered = true;
        draw_data_table(&mut buf, area, &table, &Theme::default(), None);

        // Top-left/right and bottom-left/right corners are border glyphs.
        assert_eq!(buf[(0, 0)].symbol(), "╭");
        assert_eq!(buf[(39, 0)].symbol(), "╮");
        assert_eq!(buf[(0, 5)].symbol(), "╰");
        assert_eq!(buf[(39, 5)].symbol(), "╯");
        // The left border column continues down the header row.
        assert_eq!(buf[(0, 1)].symbol(), "│");
        // The header's first column paints at local x=1 (the 1-cell
        // inset), not x=0.
        assert_eq!(buf[(1, 1)].symbol(), "N");
        // Row 0 ("pod-abc") paints one row below the header, inset the
        // same 1 cell.
        assert_eq!(buf[(1, 2)].symbol(), "p");
    }

    /// `draw_focus_ring` painted over the *same* `area` a bordered table
    /// was painted into recolours the border cells it shares with the
    /// table's own frame, but never touches the inset header/row content.
    #[test]
    fn focus_ring_over_bordered_table_never_overwrites_content() {
        let area = Rect::new(0, 0, 40, 6);
        let mut buf = Buffer::empty(area);
        let mut table = make_table();
        table.bordered = true;
        draw_data_table(&mut buf, area, &table, &Theme::default(), None);

        assert_eq!(buf[(1, 1)].symbol(), "N");
        assert_eq!(buf[(1, 2)].symbol(), "p");

        crate::tui::draw_focus_ring(&mut buf, area, &Theme::default());

        // The ring repaints the same corner glyph; header/row content a
        // cell in is untouched.
        assert_eq!(buf[(0, 0)].symbol(), "╭");
        assert_eq!(
            buf[(1, 1)].symbol(),
            "N",
            "focus ring must not overwrite the bordered table's header"
        );
        assert_eq!(
            buf[(1, 2)].symbol(),
            "p",
            "focus ring must not overwrite the bordered table's row content"
        );
    }

    /// Hit-testing stays in sync with the inset paint: clicking the
    /// painted header/row cells (local coords, i.e. relative to `area`)
    /// still resolves to the right header/row; clicking the border
    /// column/row (x=0, y=0, or the opposite edges) misses everything.
    #[test]
    fn bordered_hit_test_matches_inset_paint() {
        let area = Rect::new(0, 0, 40, 6);
        let mut buf = Buffer::empty(area);
        let mut table = make_table();
        table.bordered = true;
        let layout = draw_data_table(&mut buf, area, &table, &Theme::default(), None);

        // Clicking inside the header row (local y=1) resolves to a
        // header hit.
        assert!(matches!(
            layout.hit_test(2.0, 1.0, table.scroll_offset, table.rows.len()),
            DataTableHit::Header { col: 0 }
        ));
        // Clicking row 0's content (local y=2) resolves to row 0.
        assert_eq!(
            layout.hit_test(2.0, 2.0, table.scroll_offset, table.rows.len()),
            DataTableHit::Row { idx: 0 }
        );
        // The border frame itself is never a column/row.
        assert_eq!(
            layout.hit_test(0.0, 1.0, table.scroll_offset, table.rows.len()),
            DataTableHit::Empty,
            "left border column must not resolve to a header hit"
        );
        assert_eq!(
            layout.hit_test(2.0, 0.0, table.scroll_offset, table.rows.len()),
            DataTableHit::Empty,
            "top border row must not resolve to a header hit"
        );
    }
}

/// Multi-select keyboard wiring over [`crate::tui::testing::TuiDriver`].
///
/// `DataTable` stays purely declarative (no selection-set field of its
/// own — see `crate::types::apply_selection_gesture`'s doc), so the app
/// owns the selected-index set, the keyboard cursor, and the gesture
/// anchor. TUI has no mouse, so unlike the click-driven
/// `crate::gtk::data_table`/`crate::win::data_table` counterparts, a
/// plain Up/Down only moves the cursor; Shift+Up/Down extends a range
/// from the anchor via [`SelectionGesture::from_modifiers`], and Space
/// toggles the cursor row without moving it.
#[cfg(test)]
mod multi_select_driver_tests {
    use crate::primitives::data_table::{Column, DataRow, DataTable};
    use crate::runner::{AppLogic, Reaction};
    use crate::tui::testing::TuiDriver;
    use crate::{
        apply_selection_gesture, Backend, DataTableEvent, Decoration, Key, Modifiers, NamedKey,
        Rect, SelectionGesture, StyledText, UiEvent, WidgetId,
    };

    const VIEWPORT_W: u16 = 40;
    const VIEWPORT_H: u16 = 10;

    fn row(label: &str) -> DataRow {
        DataRow {
            cells: vec![StyledText::plain(label.to_string())],
            decoration: Decoration::Normal,
        }
    }

    fn sample_table() -> DataTable {
        DataTable {
            id: WidgetId::new("dt"),
            columns: vec![Column {
                title: "Name".to_string(),
                width: Default::default(),
                align: Default::default(),
            }],
            rows: (0..5).map(|i| row(&format!("row {i}"))).collect(),
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
        }
    }

    struct TableApp {
        table: DataTable,
        cursor: usize,
        selected: Vec<usize>,
        anchor: usize,
        events: Vec<DataTableEvent>,
    }

    impl TableApp {
        fn new() -> Self {
            Self {
                table: sample_table(),
                cursor: 0,
                selected: Vec::new(),
                anchor: 0,
                events: Vec::new(),
            }
        }
    }

    impl AppLogic for TableApp {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            backend.draw_data_table(
                Rect::new(0.0, 0.0, VIEWPORT_W as f32, VIEWPORT_H as f32),
                &self.table,
                None,
            );
        }

        fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            let UiEvent::KeyPressed { key, modifiers, .. } = event else {
                return Reaction::Continue;
            };
            let last = self.table.rows.len().saturating_sub(1);
            match key {
                Key::Named(NamedKey::Down) | Key::Named(NamedKey::Up) => {
                    let delta: isize = if key == Key::Named(NamedKey::Down) {
                        1
                    } else {
                        -1
                    };
                    self.cursor = (self.cursor as isize + delta).clamp(0, last as isize) as usize;
                    if !modifiers.shift {
                        // A plain move repositions the cursor only; the
                        // selection changes explicitly via Space/Shift.
                        return Reaction::Redraw;
                    }
                    let indices = apply_selection_gesture(
                        &self.selected,
                        self.anchor,
                        self.cursor,
                        SelectionGesture::Range,
                    );
                    self.selected = indices.clone();
                    self.events.push(DataTableEvent::SelectionSet { indices });
                    Reaction::Redraw
                }
                Key::Char(' ') => {
                    let indices = apply_selection_gesture(
                        &self.selected,
                        self.anchor,
                        self.cursor,
                        SelectionGesture::Toggle,
                    );
                    self.selected = indices.clone();
                    self.anchor = self.cursor;
                    self.events.push(DataTableEvent::SelectionSet { indices });
                    Reaction::Redraw
                }
                _ => Reaction::Continue,
            }
        }
    }

    fn key_event(key: Key, modifiers: Modifiers) -> UiEvent {
        UiEvent::KeyPressed {
            key,
            modifiers,
            repeat: false,
        }
    }

    fn driver() -> TuiDriver<TableApp> {
        TuiDriver::new(TableApp::new(), VIEWPORT_W, VIEWPORT_H)
    }

    #[test]
    fn plain_arrow_moves_the_cursor_without_changing_selection() {
        let mut driver = driver();
        driver.dispatch(key_event(Key::Named(NamedKey::Down), Modifiers::default()));
        assert_eq!(driver.app().cursor, 1);
        assert_eq!(driver.app().selected, Vec::<usize>::new());
    }

    #[test]
    fn space_toggles_rows_into_a_multi_selection() {
        let mut driver = driver();
        driver.dispatch(key_event(Key::Char(' '), Modifiers::default()));
        driver.dispatch(key_event(Key::Named(NamedKey::Down), Modifiers::default()));
        driver.dispatch(key_event(Key::Named(NamedKey::Down), Modifiers::default()));
        driver.dispatch(key_event(Key::Char(' '), Modifiers::default()));
        assert_eq!(driver.app().selected, vec![0, 2]);
        // Space toggles the same row back off.
        driver.dispatch(key_event(Key::Named(NamedKey::Up), Modifiers::default()));
        driver.dispatch(key_event(Key::Named(NamedKey::Up), Modifiers::default()));
        driver.dispatch(key_event(Key::Char(' '), Modifiers::default()));
        assert_eq!(driver.app().selected, vec![2]);
        assert_eq!(
            driver.app().events.last(),
            Some(&DataTableEvent::SelectionSet { indices: vec![2] })
        );
    }

    #[test]
    fn shift_arrow_extends_a_range_from_the_anchor() {
        let mut driver = driver();
        driver.dispatch(key_event(Key::Char(' '), Modifiers::default()));
        let shift = Modifiers {
            shift: true,
            ..Default::default()
        };
        driver.dispatch(key_event(Key::Named(NamedKey::Down), shift));
        driver.dispatch(key_event(Key::Named(NamedKey::Down), shift));
        driver.dispatch(key_event(Key::Named(NamedKey::Down), shift));
        assert_eq!(driver.app().selected, vec![0, 1, 2, 3]);
        assert_eq!(
            driver.app().events.last(),
            Some(&DataTableEvent::SelectionSet {
                indices: vec![0, 1, 2, 3]
            })
        );
    }
}
