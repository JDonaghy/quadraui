//! TUI rasteriser for [`crate::CellGrid`].
//!
//! Paints the column-letter header strip, the row-number gutter, body
//! cells clipped to their column width with a trailing ellipsis, a
//! frozen-pane boundary where the frozen rows/columns meet the scrolled
//! body, the cursor cell highlighted, and the selection range tinted.
//! Cell text never wraps — one line per row, same cell-granular
//! convention as [`crate::tui::draw_data_table`]. When
//! [`CellGrid::editing`] is `Some`, an inline single-line text editor
//! paints over the cursor cell in place of its plain text, through the
//! same shared [`super::paint_inline_text_editor`]
//! [`crate::tui::tree`]'s own inline rename editor uses — no second
//! cursor/selection/placeholder rasteriser.
//!
//! The frozen-column boundary reserves its last column's right-edge
//! cell for a `│` glyph, the same way [`crate::tui::data_table`]
//! reserves a column's right-edge cell for its separator. The
//! frozen-row boundary has no spare row to reserve — TUI's
//! architectural row height is a single line — so it is expressed as
//! [`Modifier::UNDERLINED`] on that row's already-painted glyphs
//! instead of a glyph that would overwrite them.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color as RatatuiColor, Modifier};

use super::{align_offset, ratatui_color, set_cell, set_cell_wide};
use crate::primitives::cell_grid::{CellGrid, CellGridLayout, ResolvedSpan};
use crate::primitives::data_table::ColumnAlign;
use crate::theme::Theme;

/// Row-header gutter width in cells: wide enough for the largest 1-based
/// row number or [`CellGrid::row_labels`] override in the whole sheet
/// (not just the visible rows), plus one cell of padding — so the
/// gutter never resizes as the user scrolls. `0.0` when
/// [`CellGrid::show_row_headers`] is `false`; [`CellGrid::layout`]
/// would zero it anyway, but computing it unconditionally here would
/// still measure labels nobody is about to paint.
fn row_header_width(grid: &CellGrid) -> f32 {
    if !grid.show_row_headers {
        return 0.0;
    }
    let default_digits = if grid.row_count == 0 {
        1
    } else {
        grid.row_count.to_string().chars().count()
    };
    let override_len = grid
        .row_labels
        .values()
        .map(|s| s.chars().count())
        .max()
        .unwrap_or(0);
    (default_digits.max(override_len) as f32) + 1.0
}

/// Resolve the layout [`draw_cell_grid`] paints `grid` at, without
/// touching a buffer.
///
/// This is the **single** definition of TUI `CellGrid` geometry: both
/// [`draw_cell_grid`] and `<CellGrid as Primitive>::layout` go through
/// it, so the layout a cache-free, layout-on-demand hit-test runs
/// against is bit-for-bit the layout that was painted. TUI is
/// cell-granular: one header row, a gutter sized from the sheet's own
/// row numbers, and a 1-cell scrollbar strip on each axis when
/// [`CellGrid::show_scrollbar`] is set.
pub fn tui_cell_grid_layout(area: Rect, grid: &CellGrid) -> CellGridLayout {
    let col_header_height = if grid.show_col_headers { 1.0 } else { 0.0 };
    let row_header_w = row_header_width(grid);
    let (scrollbar_w, scrollbar_h) = if grid.show_scrollbar {
        (1.0, 1.0)
    } else {
        (0.0, 0.0)
    };
    grid.layout(
        area.width as f32,
        area.height as f32,
        col_header_height,
        row_header_w,
        scrollbar_w,
        scrollbar_h,
    )
}

/// `area`-relative screen bounds `(origin, len)` for one resolved
/// row/column span, rounded independently at each edge — the same
/// per-span rounding [`crate::tui::data_table_layout`]'s `RowPaintCtx::
/// column` uses, which keeps adjacent spans visually contiguous because
/// [`crate::primitives::cell_grid::resolve_spans`] already stacks them
/// with no gap. `None` once rounding collapses the span to zero width
/// (never happens for a span `resolve_spans` returned, but keeps this
/// safe against future float edge cases rather than painting a
/// zero-width/negative-width slice).
fn span_bounds(area_len: u16, area_origin: u16, span: &ResolvedSpan) -> Option<(u16, u16)> {
    let x0 = span.start.round();
    let x1 = (span.start + span.size).round();
    if x1 <= x0 {
        return None;
    }
    let x0 = x0.max(0.0) as u16;
    let x1 = (x1.max(0.0) as u16).min(area_len);
    (x1 > x0).then_some((area_origin + x0, x1 - x0))
}

/// OR `modifier` into one already-painted cell — [`set_cell`]/
/// [`set_cell_wide`] always reset it to [`Modifier::empty`], so bold/
/// italic/underline from a [`crate::primitives::cell_grid::CellStyle`]
/// or a header's bold title is a second pass over the same coordinate.
/// OR-ing rather than assigning means a later pass (the frozen-row
/// boundary's [`Modifier::UNDERLINED`]) layers onto an earlier one
/// (a cell style's bold) instead of replacing it.
fn apply_modifier(buf: &mut Buffer, x: u16, y: u16, modifier: Modifier) {
    if modifier.is_empty() {
        return;
    }
    let area = buf.area;
    if x >= area.x && y >= area.y && x < area.x + area.width && y < area.y + area.height {
        buf[(x, y)].modifier |= modifier;
    }
}

/// Paint `text` left-aligned/centered/right-aligned (per `align`) within
/// `width` cells starting at `x0`, ellipsis-truncated via
/// [`crate::text_util::fit_chars_to_cells`] — the one shared overflow
/// rule every text-painting primitive in this crate uses.
#[allow(clippy::too_many_arguments)]
fn paint_text(
    buf: &mut Buffer,
    x0: u16,
    width: u16,
    y: u16,
    text: &str,
    fg: RatatuiColor,
    bg: RatatuiColor,
    align: ColumnAlign,
    modifier: Modifier,
) {
    if width == 0 || text.is_empty() {
        return;
    }
    let (visible_chars, needs_ellipsis, displayed_len) =
        crate::text_util::fit_chars_to_cells(text, width);
    let start = align_offset(align, displayed_len, width);

    let mut col = start;
    for ch in text.chars().take(visible_chars as usize) {
        if col >= width {
            break;
        }
        let w = crate::text_util::char_cell_width(ch);
        if w == 2 && col + 1 < width {
            set_cell_wide(buf, x0 + col, y, ch, fg, bg);
        } else {
            set_cell(buf, x0 + col, y, ch, fg, bg);
        }
        apply_modifier(buf, x0 + col, y, modifier);
        col += w;
    }
    if needs_ellipsis && col < width {
        set_cell(buf, x0 + col, y, '…', fg, bg);
        apply_modifier(buf, x0 + col, y, modifier);
    }
}

/// Fill `(x0..x0+width, y0..y0+height)` with `bg` and a blank glyph —
/// a cell's resting background before its text (if any) paints over it.
fn fill_band(
    buf: &mut Buffer,
    x0: u16,
    y0: u16,
    width: u16,
    height: u16,
    fg: RatatuiColor,
    bg: RatatuiColor,
) {
    for y in y0..y0.saturating_add(height) {
        for x in x0..x0.saturating_add(width) {
            set_cell(buf, x, y, ' ', fg, bg);
        }
    }
}

/// Width the column at `layout.cols[col_idx]` may paint text into: one
/// less than its screen width when it is the last frozen column before
/// a scrolled region, reserving that column's right-edge cell for the
/// frozen-column boundary glyph — the same reservation
/// [`crate::tui::data_table`]'s `usable_width` makes for its separator
/// column, so an exactly-filling or overflowing value never loses its
/// last glyph (or its `…`) to the divider painted over it.
fn frozen_col_usable_width(layout: &CellGridLayout, col_idx: usize, cw: u16) -> u16 {
    let is_last_frozen = layout.frozen_cols > 0
        && layout.frozen_cols < layout.cols.len()
        && col_idx + 1 == layout.frozen_cols;
    if is_last_frozen {
        cw.saturating_sub(1)
    } else {
        cw
    }
}

/// Overlay one divider glyph at `(x, y)`, keeping whatever background
/// was already painted there (read back from the buffer) rather than
/// flattening it to a fixed colour — so the frozen-pane divider reads as
/// a rule drawn *on top of* the header/gutter/body fill, not a hole in
/// it.
fn overlay_divider(buf: &mut Buffer, x: u16, y: u16, ch: char, fg: RatatuiColor) {
    let area = buf.area;
    if x < area.x || y < area.y || x >= area.x + area.width || y >= area.y + area.height {
        return;
    }
    let bg = buf[(x, y)].bg;
    set_cell(buf, x, y, ch, fg, bg);
}

/// Draw `grid` into `area` and return its layout for hit-testing.
/// Coordinate frame: **LOCAL** — `hit_test` on the returned layout takes
/// `area`-relative coordinates, matching [`tui_cell_grid_layout`].
pub fn draw_cell_grid(
    buf: &mut Buffer,
    area: Rect,
    grid: &CellGrid,
    theme: &Theme,
) -> CellGridLayout {
    let layout = tui_cell_grid_layout(area, grid);
    if area.width == 0 || area.height == 0 {
        return layout;
    }

    let fg = ratatui_color(theme.foreground);
    let bg = ratatui_color(theme.background);
    let header_bg = ratatui_color(theme.tab_bar_bg);
    let sep_fg = ratatui_color(theme.separator);
    let cursor_bg = ratatui_color(theme.selection_bg);
    let selection_bg = ratatui_color(theme.background.blend(theme.selection_bg, 0.35));
    let dim_fg = ratatui_color(theme.muted_fg);
    // Within-text selection inside the inline editor: a stronger blend
    // than `selection_bg`'s multi-cell tint above, so a byte range
    // selected *inside* the edit buffer still reads as distinct from
    // `cursor_bg`, the cell's own already-highlighted background it
    // paints on top of.
    let edit_sel_bg = ratatui_color(theme.background.blend(theme.selection_bg, 0.65));

    fill_band(buf, area.x, area.y, area.width, area.height, fg, bg);

    let row_header_w = layout.row_header_width.round() as u16;
    let col_header_h = layout.col_header_height.round() as u16;

    // ── Corner ───────────────────────────────────────────────────────
    if row_header_w > 0 && col_header_h > 0 {
        fill_band(
            buf,
            area.x,
            area.y,
            row_header_w,
            col_header_h,
            fg,
            header_bg,
        );
    }

    // ── Column headers ───────────────────────────────────────────────
    if col_header_h > 0 {
        let y = area.y;
        for (ci, col) in layout.cols.iter().enumerate() {
            if let Some((cx, cw)) = span_bounds(area.width, area.x, col) {
                fill_band(buf, cx, y, cw, col_header_h, fg, header_bg);
                let usable = frozen_col_usable_width(&layout, ci, cw);
                paint_text(
                    buf,
                    cx,
                    usable,
                    y,
                    &grid.col_label(col.index),
                    fg,
                    header_bg,
                    ColumnAlign::Center,
                    Modifier::BOLD,
                );
            }
        }
    }

    // ── Row headers ──────────────────────────────────────────────────
    if row_header_w > 0 {
        for row in &layout.rows {
            if let Some((ry, rh)) = span_bounds(area.height, area.y, row) {
                fill_band(buf, area.x, ry, row_header_w, rh, fg, header_bg);
                paint_text(
                    buf,
                    area.x,
                    row_header_w,
                    ry,
                    &grid.row_label(row.index),
                    fg,
                    header_bg,
                    ColumnAlign::Right,
                    Modifier::empty(),
                );
            }
        }
    }

    // ── Body cells ───────────────────────────────────────────────────
    let (sel_top, sel_left, sel_bottom, sel_right) = grid.selection_bounds();
    for (ri, row) in layout.rows.iter().enumerate() {
        let Some((ry, rh)) = span_bounds(area.height, area.y, row) else {
            continue;
        };
        let in_sel_row = row.index >= sel_top && row.index <= sel_bottom;
        for (ci, col) in layout.cols.iter().enumerate() {
            let Some((cx, cw)) = span_bounds(area.width, area.x, col) else {
                continue;
            };
            let cell = grid.cells.get(ri).and_then(|r| r.get(ci));
            let is_cursor = grid.cursor == (row.index, col.index);
            let in_selection = in_sel_row && col.index >= sel_left && col.index <= sel_right;

            let style = cell.map(|c| &c.style);
            let base_bg = style.and_then(|s| s.bg).map(ratatui_color).unwrap_or(bg);
            let cell_bg = if is_cursor {
                cursor_bg
            } else if in_selection {
                selection_bg
            } else {
                base_bg
            };
            let cell_fg = style.and_then(|s| s.fg).map(ratatui_color).unwrap_or(fg);

            fill_band(buf, cx, ry, cw, rh, cell_fg, cell_bg);

            // The cursor cell's plain text is skipped while `editing` is
            // `Some`: the overlay below paints the inline editor over this
            // same span instead, and the earlier `fill_band` call already
            // cleared it to `cell_bg`. Painting the text here too would
            // leave the cell's old glyphs (e.g. a right-aligned value's
            // digits past the editor's cursor, or a truncated label's
            // tail) visible around the shorter editor buffer.
            let is_editing_this_cell = is_cursor && grid.editing.is_some();
            if let Some(cell) = cell {
                if !cell.text.is_empty() && !is_editing_this_cell {
                    let mut modifier = Modifier::empty();
                    if style.and_then(|s| s.bold).unwrap_or(false) {
                        modifier |= Modifier::BOLD;
                    }
                    if style.and_then(|s| s.italic).unwrap_or(false) {
                        modifier |= Modifier::ITALIC;
                    }
                    if style.and_then(|s| s.underline).unwrap_or(false) {
                        modifier |= Modifier::UNDERLINED;
                    }
                    let align = cell.style.effective_align(&cell.text);
                    let usable = frozen_col_usable_width(&layout, ci, cw);
                    paint_text(
                        buf, cx, usable, ry, &cell.text, cell_fg, cell_bg, align, modifier,
                    );
                }
            }
        }
    }

    // ── Inline cell editor overlay ──────────────────────────────────
    // Painted after the body loop above, so it replaces the cursor
    // cell's plain text rather than racing it, and before the
    // frozen-pane boundaries below, so a divider still draws on top of
    // it exactly as it would over any other cell's text.
    if let Some(edit) = &grid.editing {
        let cursor_row = layout.rows.iter().find(|r| r.index == grid.cursor.0);
        let cursor_col = layout.cols.iter().position(|c| c.index == grid.cursor.1);
        if let (Some(row), Some(ci)) = (cursor_row, cursor_col) {
            let col = layout.cols[ci];
            if let (Some((ry, rh)), Some((cx, cw))) = (
                span_bounds(area.height, area.y, row),
                span_bounds(area.width, area.x, &col),
            ) {
                let usable = frozen_col_usable_width(&layout, ci, cw);
                super::paint_inline_text_editor(
                    buf,
                    Rect::new(cx, ry, usable, rh),
                    ry,
                    0,
                    &edit.text,
                    edit.cursor,
                    edit.selection_anchor,
                    edit.placeholder.as_deref(),
                    fg,
                    cursor_bg,
                    edit_sel_bg,
                    dim_fg,
                );
            }
        }
    }

    // ── Frozen-pane boundaries ───────────────────────────────────────
    // Column axis: the last frozen column already reserved its
    // right-edge cell above (`frozen_col_usable_width`), so the glyph
    // lands in a cell no text was ever painted into.
    if layout.frozen_cols > 0 && layout.frozen_cols < layout.cols.len() {
        if let Some((cx, cw)) =
            span_bounds(area.width, area.x, &layout.cols[layout.frozen_cols - 1])
        {
            let div_x = cx + cw.saturating_sub(1);
            for y in area.y..area.y.saturating_add(area.height) {
                overlay_divider(buf, div_x, y, '│', sep_fg);
            }
        }
    }
    // Row axis: there is no spare row to reserve (TUI rows are a
    // single line), so the boundary is an underline over the row's
    // already-painted glyphs rather than a glyph that would replace
    // them.
    if layout.frozen_rows > 0 && layout.frozen_rows < layout.rows.len() {
        if let Some((ry, rh)) =
            span_bounds(area.height, area.y, &layout.rows[layout.frozen_rows - 1])
        {
            let div_y = ry + rh.saturating_sub(1);
            for x in area.x..area.x.saturating_add(area.width) {
                apply_modifier(buf, x, div_y, Modifier::UNDERLINED);
            }
        }
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::cell_grid::{Cell, CellAlign, CellGridHit, CellStyle};
    use crate::types::{Color, WidgetId};

    fn cell_at(text: &str) -> Cell {
        Cell::plain(text)
    }

    /// A 6x6 sheet, one frozen row and column, cells carrying a
    /// distinctive `"R{row}C{col}"` label so a paint/click round trip
    /// can find a specific cell by its painted glyphs.
    fn grid() -> CellGrid {
        let rows = 6usize;
        let cols = 6usize;
        let cells = (0..rows)
            .map(|r| (0..cols).map(|c| cell_at(&format!("R{r}C{c}"))).collect())
            .collect();
        CellGrid {
            id: WidgetId::new("grid:test"),
            row_count: rows,
            col_count: cols,
            frozen_rows: 1,
            frozen_cols: 1,
            row_scroll: 0,
            col_scroll: 0,
            default_row_height: 1.0,
            default_col_width: 6.0,
            row_heights: Default::default(),
            col_widths: Default::default(),
            show_row_headers: true,
            show_col_headers: true,
            row_labels: Default::default(),
            col_labels: Default::default(),
            cursor: (0, 0),
            selection_anchor: None,
            cells,
            loaded_row_start: 0,
            loaded_col_start: 0,
            has_focus: true,
            show_scrollbar: false,
            editing: None,
        }
    }

    fn find_text(buf: &Buffer, needle: &str) -> Option<(u16, u16)> {
        find_text_in(buf, buf.area, needle)
    }

    /// [`find_text`], restricted to `search_area` — needed wherever the
    /// needle could also appear inside an unrelated body cell's own
    /// `"R{row}C{col}"` text (e.g. row header `"4"` inside `"R0C4"`).
    fn find_text_in(buf: &Buffer, search_area: Rect, needle: &str) -> Option<(u16, u16)> {
        for y in search_area.y..search_area.y + search_area.height {
            let mut line = String::new();
            for x in search_area.x..search_area.x + search_area.width {
                line.push(buf[(x, y)].symbol().chars().next().unwrap_or(' '));
            }
            if let Some(byte_idx) = line.find(needle) {
                // `byte_idx` is a byte offset into an ASCII-only probe
                // string (every glyph this test paints is a single-byte
                // ASCII character), so it is also the column.
                return Some((search_area.x + byte_idx as u16, y));
            }
        }
        None
    }

    #[test]
    fn paint_click_round_trip_for_a_body_cell() {
        let grid = grid();
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let layout = draw_cell_grid(&mut buf, area, &grid, &theme);

        // Row 3 / col 2 is past both frozen panes, so it is a scrolled
        // body cell rather than a frozen one.
        let (x, y) = find_text(&buf, "R3C2").expect("R3C2's glyphs are painted somewhere");
        assert_eq!(
            layout.hit_test(x as f32 + 0.5, y as f32 + 0.5),
            CellGridHit::Cell(3, 2)
        );
    }

    #[test]
    fn paint_click_round_trip_for_header_and_corner() {
        let grid = grid();
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let layout = draw_cell_grid(&mut buf, area, &grid, &theme);

        // Column header "C" (absolute column 2 → letter "C") — restricted
        // to the header strip's own row so this can't match a body
        // cell's "R{row}C{col}" text instead.
        let header_row = Rect::new(area.x, area.y, area.width, 1);
        let (cx, cy) = find_text_in(&buf, header_row, "C").expect("column header C is painted");
        assert_eq!(cy, 0, "the column header strip is the very first row");
        assert_eq!(
            layout.hit_test(cx as f32 + 0.5, cy as f32 + 0.5),
            CellGridHit::ColumnHeader(2)
        );

        // Row header "4" (absolute row 3 → 1-based label "4") —
        // restricted to the gutter column so this can't match a body
        // cell's "R{row}C{col}" text instead.
        let gutter = Rect::new(
            area.x,
            area.y,
            layout.row_header_width.round() as u16,
            area.height,
        );
        let (rx, ry) = find_text_in(&buf, gutter, "4").expect("row header 4 is painted");
        assert_eq!(
            layout.hit_test(rx as f32 + 0.5, ry as f32 + 0.5),
            CellGridHit::RowHeader(3)
        );

        // The top-left corner, where both header strips meet.
        assert_eq!(layout.hit_test(0.0, 0.0), CellGridHit::Corner);
    }

    #[test]
    fn cursor_highlight_lands_on_the_cursor_cell_not_its_neighbours() {
        let mut grid = grid();
        // Scroll the body so the cursor sits past the frozen pane
        // boundary, exercising the frozen-then-scrolled layout order,
        // not just an unscrolled frozen cell.
        grid.row_scroll = 2;
        grid.col_scroll = 2;
        grid.cursor = (3, 3);
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let layout = draw_cell_grid(&mut buf, area, &grid, &theme);

        let cursor_row = layout
            .rows
            .iter()
            .find(|r| r.index == 3)
            .expect("cursor row 3 is visible");
        let cursor_col = layout
            .cols
            .iter()
            .find(|c| c.index == 3)
            .expect("cursor col 3 is visible");
        let cell_x = cursor_col.start.round() as u16;
        let cell_y = cursor_row.start.round() as u16;
        let cursor_bg = buf[(cell_x, cell_y)].bg;
        assert_eq!(cursor_bg, ratatui_color(theme.selection_bg));

        // The frozen (0, 0) cell is neither the cursor nor selected —
        // it must not carry the cursor's highlight.
        let frozen_row = layout.rows[0];
        let frozen_col = layout.cols[0];
        let other_bg = buf[(
            frozen_col.start.round() as u16,
            frozen_row.start.round() as u16,
        )]
            .bg;
        assert_ne!(other_bg, ratatui_color(theme.selection_bg));
    }

    #[test]
    fn selection_range_is_tinted_but_the_cursor_cell_reads_stronger() {
        let mut grid = grid();
        grid.cursor = (0, 0);
        grid.selection_anchor = Some((2, 2));
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let layout = draw_cell_grid(&mut buf, area, &grid, &theme);

        let r1 = layout.rows.iter().find(|r| r.index == 1).unwrap();
        let c1 = layout.cols.iter().find(|c| c.index == 1).unwrap();
        let selected_not_cursor_bg = buf[(c1.start.round() as u16, r1.start.round() as u16)].bg;
        let cursor_bg = buf[(
            layout.cols[0].start.round() as u16,
            layout.rows[0].start.round() as u16,
        )]
            .bg;

        assert_eq!(cursor_bg, ratatui_color(theme.selection_bg));
        assert_ne!(selected_not_cursor_bg, ratatui_color(theme.background));
        assert_ne!(
            selected_not_cursor_bg,
            ratatui_color(theme.selection_bg),
            "a selected-but-not-cursor cell is tinted, not fully highlighted"
        );
    }

    #[test]
    fn frozen_pane_dividers_paint_at_the_frozen_scrolled_boundary() {
        let mut grid = grid();
        grid.row_scroll = 2;
        grid.col_scroll = 2;
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let layout = draw_cell_grid(&mut buf, area, &grid, &theme);

        let div_x = layout.cols[0].start.round() as u16 + layout.cols[0].size.round() as u16 - 1;
        let div_y = layout.rows[0].start.round() as u16 + layout.rows[0].size.round() as u16 - 1;
        // Away from `(div_x, div_y)` itself.
        let body_y = layout.rows.last().unwrap().start.round() as u16;
        let body_x = layout.cols.last().unwrap().start.round() as u16;
        assert_eq!(buf[(div_x, body_y)].symbol(), "│");
        // The row boundary is an underline over the frozen row's own
        // glyph, not a glyph that replaces it — the cell's text must
        // survive.
        assert_eq!(buf[(body_x, div_y)].symbol(), "R");
        assert!(
            buf[(body_x, div_y)].modifier.contains(Modifier::UNDERLINED),
            "the frozen-row boundary is expressed as underline, not an overwritten glyph"
        );
    }

    #[test]
    fn frozen_column_cell_exactly_filling_its_usable_width_keeps_its_last_glyph_and_divider() {
        // Acceptance: a frozen cell whose text exactly fills the column's
        // *usable* width (the column width minus the reserved divider
        // cell) must not lose its last glyph to the divider, mirroring
        // `data_table::body_cell_exactly_filling_column_still_gets_a_separator`.
        let mut grid = grid();
        let area = Rect::new(0, 0, 40, 10);

        // Learn the frozen column's screen geometry first — it doesn't
        // depend on cell text, only on sheet shape/sizing.
        let probe = tui_cell_grid_layout(area, &grid);
        let (cx, cw) =
            span_bounds(area.width, area.x, &probe.cols[0]).expect("frozen column 0 is visible");
        let (ry, _) =
            span_bounds(area.height, area.y, &probe.rows[0]).expect("frozen row 0 is visible");
        let usable = (cw - 1) as usize;
        grid.cells[0][0] = cell_at(&"X".repeat(usable));

        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        draw_cell_grid(&mut buf, area, &grid, &theme);

        for i in 0..usable {
            assert_eq!(
                buf[(cx + i as u16, ry)].symbol(),
                "X",
                "glyph {i} of the exactly-filling frozen cell must survive"
            );
        }
        let div_x = cx + cw - 1;
        assert_eq!(
            buf[(div_x, ry)].symbol(),
            "│",
            "the reserved cell still carries the frozen-column divider"
        );
    }

    #[test]
    fn cell_style_overrides_map_onto_terminal_colours() {
        let mut grid = grid();
        grid.cells[3][3].style = CellStyle {
            bold: Some(true),
            italic: None,
            underline: None,
            fg: Some(Color::rgb(10, 20, 30)),
            bg: Some(Color::rgb(200, 210, 220)),
            align: Some(CellAlign::Left),
        };
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        draw_cell_grid(&mut buf, area, &grid, &theme);

        let (x, y) = find_text(&buf, "R3C3").expect("styled cell's text is painted");
        assert_eq!(buf[(x, y)].fg, ratatui_color(Color::rgb(10, 20, 30)));
        assert_eq!(buf[(x, y)].bg, ratatui_color(Color::rgb(200, 210, 220)));
        assert!(buf[(x, y)].modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn inline_editor_overlays_the_cursor_cell_replacing_its_plain_text() {
        use crate::primitives::cell_grid::CellEditState;

        let mut grid = grid();
        grid.cursor = (3, 3);
        grid.editing = Some(CellEditState {
            text: "=1+2".into(),
            cursor: 4,
            selection_anchor: None,
            placeholder: None,
        });
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let layout = draw_cell_grid(&mut buf, area, &grid, &theme);

        // The cell's own plain "R3C3" text must be gone, replaced by the
        // editor's buffer.
        assert!(find_text(&buf, "R3C3").is_none());
        let cursor_row = layout.rows.iter().find(|r| r.index == 3).unwrap();
        let cursor_col = layout.cols.iter().find(|c| c.index == 3).unwrap();
        let (cx, _) =
            span_bounds(area.width, area.x, cursor_col).expect("cursor column is visible");
        let (cy, _) = span_bounds(area.height, area.y, cursor_row).expect("cursor row is visible");
        for (i, ch) in "=1+2".chars().enumerate() {
            assert_eq!(
                buf[(cx + i as u16, cy)].symbol(),
                ch.to_string(),
                "glyph {i} of the editor's own text must be painted"
            );
        }
    }

    #[test]
    fn inline_editor_clears_the_cursor_cells_leftover_glyphs_past_a_shorter_edit_buffer() {
        use crate::primitives::cell_grid::CellEditState;

        let mut grid = grid();
        grid.cursor = (3, 3);
        grid.cells[3][3] = Cell {
            text: "$1,200.50".into(),
            style: CellStyle {
                align: Some(CellAlign::Right),
                ..Default::default()
            },
        };
        grid.editing = Some(CellEditState {
            text: "=1".into(),
            cursor: 2,
            selection_anchor: None,
            placeholder: None,
        });
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let layout = draw_cell_grid(&mut buf, area, &grid, &theme);

        let cursor_row = layout.rows.iter().find(|r| r.index == 3).unwrap();
        let (ci, col) = layout
            .cols
            .iter()
            .enumerate()
            .find(|(_, c)| c.index == 3)
            .unwrap();
        let (cx, cw) = span_bounds(area.width, area.x, col).expect("cursor column is visible");
        let (cy, _) = span_bounds(area.height, area.y, cursor_row).expect("cursor row is visible");
        let usable = frozen_col_usable_width(&layout, ci, cw);

        // The cell's own right-aligned "$1,200.50" must not survive
        // anywhere in the cursor cell's span — the defect this guards
        // against painted the editor flush left while the original
        // right-aligned value kept sitting at the cell's right edge.
        assert!(
            find_text_in(&buf, Rect::new(cx, cy, usable, 1), "1,200").is_none(),
            "the cell's own right-aligned text must not survive under the inline editor"
        );

        // "=1" plus one inverted cursor cell at offset 2 is everything
        // the overlay paints; every cell past that, out to the column's
        // usable width, must be blank rather than showing a leftover
        // glyph from the body loop's unconditional `paint_text`.
        for x in (cx + 3)..(cx + usable) {
            assert_eq!(
                buf[(x, cy)].symbol(),
                " ",
                "cell past the editor's own text/cursor span must be blank"
            );
        }
    }

    #[test]
    fn inline_editor_absent_leaves_the_cursor_cell_as_plain_text() {
        let grid = grid();
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        draw_cell_grid(&mut buf, area, &grid, &theme);

        // The cursor cell is (0, 0) by default — its plain "R0C0" text
        // must survive when `editing` is `None`.
        find_text(&buf, "R0C0").expect("cursor cell paints its own text with no active edit");
    }
}
