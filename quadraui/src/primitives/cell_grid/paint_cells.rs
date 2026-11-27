//! The cell painter and cell layout for [`CellGrid`].
//!
//! Paints the column-letter header strip, the row-number gutter, body
//! cells clipped to their column width with a trailing ellipsis, a
//! frozen-pane boundary where the frozen rows/columns meet the scrolled
//! body, the cursor cell highlighted, and the selection range tinted.
//! Cell text never wraps — one line per row, the same cell-granular
//! convention as the data table's cell painter. When
//! [`CellGrid::editing`] is `Some`, the inline editor paints over the
//! cursor cell through the text field's own row painter
//! ([`crate::primitives::text_field::cell_paint::put_text_row`]): a
//! reverse-video block caret and a reverse-video selection, with the
//! placeholder dimmed when the buffer is empty.
//!
//! The frozen-column boundary reserves its last column's right-edge
//! cell for a `│` glyph, the same way the data table reserves a column's
//! right-edge cell for its separator. The frozen-row boundary has no
//! spare row to reserve — a cell row is a single line — so it underlines
//! that row's already-painted glyphs instead of overwriting them.

use super::{CellGrid, CellGridLayout, ResolvedSpan};
use crate::cell_surface::{CellStyle, CellSurface};
use crate::event::Rect;
use crate::primitives::data_table::ColumnAlign;
use crate::theme::Theme;
use crate::types::Color;

/// Row-header gutter width in cells: wide enough for the largest 1-based
/// row number or [`CellGrid::row_labels`] override in the whole sheet
/// (not just the visible rows), plus one cell of padding — so the
/// gutter never resizes as the user scrolls. `0.0` when
/// [`CellGrid::show_row_headers`] is `false`.
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

/// The cell-grid layout of `grid` over `area` (whole cells): one header
/// row, a gutter sized from the sheet's own row numbers, and a 1-cell
/// scrollbar strip on each axis when [`CellGrid::show_scrollbar`] is
/// set. The single definition of cell-grid geometry: the painter and
/// [`crate::Primitive::layout`] both use it, so a hit test always runs
/// against the layout that was painted.
pub(crate) fn layout(grid: &CellGrid, area: Rect) -> CellGridLayout {
    let area = whole_cells(area);
    let col_header_height = if grid.show_col_headers { 1.0 } else { 0.0 };
    let (scrollbar_w, scrollbar_h) = if grid.show_scrollbar {
        (1.0, 1.0)
    } else {
        (0.0, 0.0)
    };
    grid.layout(
        area.width,
        area.height,
        col_header_height,
        row_header_width(grid),
        scrollbar_w,
        scrollbar_h,
    )
}

/// `rect` snapped to whole cells, each edge value rounded — the grid a
/// cell backend paints `rect` onto.
fn whole_cells(rect: Rect) -> Rect {
    Rect::new(
        rect.x.max(0.0).round(),
        rect.y.max(0.0).round(),
        rect.width.max(0.0).round(),
        rect.height.max(0.0).round(),
    )
}

/// `area`-relative screen bounds `(origin, len)` for one resolved
/// row/column span, rounded independently at each edge so adjacent spans
/// stay contiguous. `None` once rounding collapses the span to zero
/// width.
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

/// An opaque cell style with `fg` on `bg`.
fn solid(fg: Color, bg: Color) -> CellStyle {
    crate::cell_surface::solid_style(fg, bg)
}

/// Paint `text` left-aligned/centered/right-aligned (per `align`) within
/// `width` cells starting at `x0`, ellipsis-truncated via
/// [`crate::text_util::fit_chars_to_cells`] — the one shared overflow
/// rule every text-painting primitive in this crate uses.
fn paint_text(
    cells: &mut dyn CellSurface,
    (x0, width, y): (u16, u16, u16),
    text: &str,
    align: ColumnAlign,
    style: CellStyle,
) {
    if width == 0 || text.is_empty() {
        return;
    }
    let (visible_chars, needs_ellipsis, displayed_len) =
        crate::text_util::fit_chars_to_cells(text, width);
    let start = crate::tui::align_offset(align, displayed_len, width);

    let mut col = start;
    for ch in text.chars().take(visible_chars as usize) {
        if col >= width {
            break;
        }
        let w = crate::text_util::char_cell_width(ch);
        if w == 2 && col + 1 < width {
            cells.put_wide(x0 + col, y, ch, style);
        } else {
            cells.put_char(x0 + col, y, ch, style);
        }
        col += w;
    }
    if needs_ellipsis && col < width {
        cells.put_char(x0 + col, y, '…', style);
    }
}

/// Fill `(x0..x0+width, y0..y0+height)` with a blank glyph in `style` —
/// a cell's resting background before its text (if any) paints over it.
fn fill_band(
    cells: &mut dyn CellSurface,
    x0: u16,
    y0: u16,
    width: u16,
    height: u16,
    style: CellStyle,
) {
    let rect = Rect::new(x0 as f32, y0 as f32, width as f32, height as f32);
    cells.fill(rect, style);
}

/// Overlay a vertical `│` rule down `height` rows from `(x, y0)` — every
/// column divider this painter draws, the frozen-pane boundary and a
/// [`CellGrid::column_separators`] separator alike. Each glyph keeps
/// whatever background was already painted under it, so the rule reads
/// as drawn on top of the header/gutter/body fill. Cells off the surface
/// are skipped.
fn overlay_divider_run(cells: &mut dyn CellSurface, x: u16, y0: u16, height: u16, fg: Color) {
    for y in y0..y0.saturating_add(height) {
        let Some((_, under)) = cells.get(x, y) else {
            continue;
        };
        cells.put_char(
            x,
            y,
            '│',
            CellStyle {
                fg: Some(fg),
                bg: under.bg,
                ..CellStyle::default()
            },
        );
    }
}

/// Draw `grid` at `layout` (resolved by [`layout`] over `area`) onto
/// `cells`.
pub(crate) fn paint(
    grid: &CellGrid,
    area: Rect,
    layout: &CellGridLayout,
    cells: &mut dyn CellSurface,
    theme: &Theme,
) {
    let area = whole_cells(area);
    let (ax, ay, aw, ah) = (
        area.x as u16,
        area.y as u16,
        area.width as u16,
        area.height as u16,
    );
    if aw == 0 || ah == 0 {
        return;
    }

    let fg = theme.foreground;
    let bg = theme.background;
    let header_bg = theme.tab_bar_bg;
    let sep_fg = theme.separator;
    let cursor_bg = theme.selection_bg;
    let selection_bg = theme.background.blend(theme.selection_bg, 0.35);

    fill_band(cells, ax, ay, aw, ah, solid(fg, bg));

    let row_header_w = layout.row_header_width.round() as u16;
    let col_header_h = layout.col_header_height.round() as u16;

    // ── Corner ───────────────────────────────────────────────────────
    if row_header_w > 0 && col_header_h > 0 {
        fill_band(
            cells,
            ax,
            ay,
            row_header_w,
            col_header_h,
            solid(fg, header_bg),
        );
    }

    // ── Column headers ───────────────────────────────────────────────
    if col_header_h > 0 {
        let y = ay;
        for (ci, col) in layout.cols.iter().enumerate() {
            if let Some((cx, cw)) = span_bounds(aw, ax, col) {
                fill_band(cells, cx, y, cw, col_header_h, solid(fg, header_bg));
                let usable = grid.col_text_cells(layout, ci, cw);
                paint_text(
                    cells,
                    (cx, usable, y),
                    &grid.col_label(col.index),
                    ColumnAlign::Center,
                    CellStyle {
                        bold: true,
                        ..solid(fg, header_bg)
                    },
                );
                if grid.paints_column_separator(layout, ci) {
                    overlay_divider_run(cells, cx + usable, y, col_header_h, sep_fg);
                }
            }
        }
    }

    // ── Row headers ──────────────────────────────────────────────────
    if row_header_w > 0 {
        for row in &layout.rows {
            if let Some((ry, rh)) = span_bounds(ah, ay, row) {
                fill_band(cells, ax, ry, row_header_w, rh, solid(fg, header_bg));
                paint_text(
                    cells,
                    (ax, row_header_w, ry),
                    &grid.row_label(row.index),
                    ColumnAlign::Right,
                    solid(fg, header_bg),
                );
            }
        }
    }

    // ── Body cells ───────────────────────────────────────────────────
    let (sel_top, sel_left, sel_bottom, sel_right) = grid.selection_bounds();
    for (ri, row) in layout.rows.iter().enumerate() {
        let Some((ry, rh)) = span_bounds(ah, ay, row) else {
            continue;
        };
        let in_sel_row = row.index >= sel_top && row.index <= sel_bottom;
        for (ci, col) in layout.cols.iter().enumerate() {
            let Some((cx, cw)) = span_bounds(aw, ax, col) else {
                continue;
            };
            let cell = grid.cells.get(ri).and_then(|r| r.get(ci));
            let is_cursor = grid.cursor == (row.index, col.index);
            let in_selection = in_sel_row && col.index >= sel_left && col.index <= sel_right;

            let style = cell.map(|c| &c.style);
            // An explicit `CellStyle::bg` wins over the row's zebra
            // stripe, which in turn only shows through on a cell the
            // cursor/selection branch below leaves alone.
            let base_bg = style
                .and_then(|s| s.bg)
                .or_else(|| grid.row_stripe_bg(layout, ri, row.index, theme))
                .unwrap_or(bg);
            let cell_bg = if is_cursor {
                cursor_bg
            } else if in_selection {
                selection_bg
            } else {
                base_bg
            };
            let cell_fg = style.and_then(|s| s.fg).unwrap_or(fg);

            fill_band(cells, cx, ry, cw, rh, solid(cell_fg, cell_bg));

            let usable = grid.col_text_cells(layout, ci, cw);
            // The cursor cell's plain text is skipped while `editing` is
            // `Some`: the overlay below paints the inline editor over this
            // same span instead, on the background `fill_band` just
            // cleared it to.
            let is_editing_this_cell = is_cursor && grid.editing.is_some();
            if let Some(cell) = cell {
                if !cell.text.is_empty() && !is_editing_this_cell {
                    let flag = |f: Option<bool>| f.unwrap_or(false);
                    let text_style = CellStyle {
                        bold: flag(style.and_then(|s| s.bold)),
                        italic: flag(style.and_then(|s| s.italic)),
                        underline: flag(style.and_then(|s| s.underline)).then_some(cell_fg),
                        ..solid(cell_fg, cell_bg)
                    };
                    let align = cell.style.effective_align(&cell.text);
                    paint_text(cells, (cx, usable, ry), &cell.text, align, text_style);
                }
            }
            if grid.paints_column_separator(layout, ci) {
                overlay_divider_run(cells, cx + usable, ry, rh, sep_fg);
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
            if let (Some((ry, _)), Some((cx, cw))) =
                (span_bounds(ah, ay, row), span_bounds(aw, ax, &col))
            {
                let usable = grid.col_text_cells(layout, ci, cw);
                crate::primitives::text_field::cell_paint::put_text_row(
                    cells,
                    ry,
                    cx,
                    cx + usable,
                    usable as usize,
                    0,
                    &edit.text,
                    edit.placeholder.as_deref().unwrap_or(""),
                    Some(edit.cursor),
                    edit.selection_anchor,
                    fg,
                    theme.muted_fg,
                    cursor_bg,
                );
            }
        }
    }

    // ── Frozen-pane boundaries ───────────────────────────────────────
    // Column axis: the last frozen column already reserved its
    // right-edge cell above (`CellGrid::col_text_cells`), so the glyph
    // lands in a cell no text was ever painted into.
    if layout.frozen_cols > 0 && layout.frozen_cols < layout.cols.len() {
        if let Some((cx, cw)) = span_bounds(aw, ax, &layout.cols[layout.frozen_cols - 1]) {
            let div_x = cx + cw.saturating_sub(1);
            overlay_divider_run(cells, div_x, ay, ah, sep_fg);
        }
    }
    // Row axis: there is no spare row to reserve, so the boundary is an
    // underline over the row's already-painted glyphs rather than a
    // glyph that would replace them. No `CellSurface` verb adds a
    // modifier to a painted cell in place (re-writing a double-width
    // glyph's continuation cell would split it), so this reaches the
    // backing buffer.
    if layout.frozen_rows > 0 && layout.frozen_rows < layout.rows.len() {
        if let Some((ry, rh)) = span_bounds(ah, ay, &layout.rows[layout.frozen_rows - 1]) {
            let div_y = ry + rh.saturating_sub(1);
            // CSR: ratatui_buffer escape hatch
            if let Some(buf) = cells.ratatui_buffer() {
                let b = buf.area;
                if div_y >= b.y && div_y < b.y + b.height {
                    for x in ax.max(b.x)..ax.saturating_add(aw).min(b.x + b.width) {
                        buf[(x, div_y)].modifier |= ratatui::style::Modifier::UNDERLINED;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::cell_grid::{Cell, CellAlign, CellGridHit, CellStyle};
    use crate::tui::{ratatui_color, BufferCellSurface};
    use crate::types::{Color, WidgetId};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Modifier;

    fn quad(area: Rect) -> crate::Rect {
        crate::Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        )
    }

    fn tui_cell_grid_layout(area: Rect, grid: &CellGrid) -> CellGridLayout {
        layout(grid, quad(area))
    }

    /// Lay `grid` out over `area` and paint it into `buf`, the way the TUI
    /// backend does.
    fn draw_cell_grid(
        buf: &mut Buffer,
        area: Rect,
        grid: &CellGrid,
        theme: &Theme,
    ) -> CellGridLayout {
        let layout = tui_cell_grid_layout(area, grid);
        paint(
            grid,
            quad(area),
            &layout,
            &mut BufferCellSurface::new(buf),
            theme,
        );
        layout
    }

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
            column_separators: false,
            striped: false,
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
        let usable = grid.col_text_cells(&layout, ci, cw);

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

    #[test]
    fn column_separators_off_paints_no_extra_divider_glyphs() {
        let grid = grid();
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let layout = draw_cell_grid(&mut buf, area, &grid, &theme);

        // Only the frozen-pane boundary's own divider column may ever
        // carry a `│` — `column_separators` is `false` here, the
        // default.
        let frozen_div_x =
            layout.cols[0].start.round() as u16 + layout.cols[0].size.round() as u16 - 1;
        for col in &layout.cols {
            let Some((cx, cw)) = span_bounds(area.width, area.x, col) else {
                continue;
            };
            let div_x = cx + cw - 1;
            if div_x == frozen_div_x {
                continue;
            }
            for y in area.y..area.y.saturating_add(area.height) {
                assert_ne!(
                    buf[(div_x, y)].symbol(),
                    "│",
                    "column {} must carry no separator when `column_separators` is off",
                    col.index
                );
            }
        }
    }

    #[test]
    fn column_separators_on_paint_a_divider_at_every_visible_columns_right_edge() {
        let mut grid = grid();
        grid.column_separators = true;
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let layout = draw_cell_grid(&mut buf, area, &grid, &theme);

        let body_row = layout
            .rows
            .iter()
            .find(|r| r.index == 2)
            .expect("row 2 is visible");
        let (ry, _) =
            span_bounds(area.height, area.y, body_row).expect("row 2's screen bounds resolve");

        // Every column but the sheet's very last one carries a divider
        // at its own right edge — header row and a scrolled body row
        // alike.
        for col in &layout.cols {
            if col.index + 1 == grid.col_count {
                continue;
            }
            let (cx, cw) =
                span_bounds(area.width, area.x, col).expect("every layout column is visible here");
            let div_x = cx + cw - 1;
            assert_eq!(
                buf[(div_x, area.y)].symbol(),
                "│",
                "column {}'s header must carry a separator",
                col.index
            );
            assert_eq!(
                buf[(div_x, ry)].symbol(),
                "│",
                "column {}'s body row must carry a separator",
                col.index
            );
        }
    }

    #[test]
    fn column_separators_persist_at_every_visible_column_across_a_horizontal_scroll() {
        let mut grid = grid();
        grid.column_separators = true;
        grid.col_scroll = 2;
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let layout = draw_cell_grid(&mut buf, area, &grid, &theme);

        for col in &layout.cols {
            if col.index + 1 == grid.col_count {
                continue;
            }
            let (cx, cw) =
                span_bounds(area.width, area.x, col).expect("every layout column is visible here");
            let div_x = cx + cw - 1;
            assert_eq!(
                buf[(div_x, area.y)].symbol(),
                "│",
                "scrolled column {} must still carry a separator",
                col.index
            );
        }
    }

    #[test]
    fn frozen_boundary_column_reserves_only_one_cell_even_with_column_separators_on() {
        let mut grid = grid();
        grid.column_separators = true;
        let area = Rect::new(0, 0, 40, 10);
        let probe = tui_cell_grid_layout(area, &grid);
        let (_, cw) =
            span_bounds(area.width, area.x, &probe.cols[0]).expect("frozen column 0 is visible");
        let usable = grid.col_text_cells(&probe, 0, cw);
        assert_eq!(
            usable,
            cw - 1,
            "the frozen-pane boundary and a plain separator must never both reserve a cell on the same column"
        );
    }

    #[test]
    fn striped_off_by_default_paints_no_extra_background_tint() {
        let grid = grid();
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        draw_cell_grid(&mut buf, area, &grid, &theme);
        let stripe_bg = ratatui_color(theme.row_alt_bg(theme.background));

        for y in area.y..area.y.saturating_add(area.height) {
            for x in area.x..area.x.saturating_add(area.width) {
                assert_ne!(
                    buf[(x, y)].bg,
                    stripe_bg,
                    "no cell may carry the stripe colour when `striped` is off"
                );
            }
        }
    }

    #[test]
    fn striped_tints_odd_absolute_rows_but_never_a_frozen_or_even_row() {
        let mut grid = grid();
        grid.striped = true;
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let layout = draw_cell_grid(&mut buf, area, &grid, &theme);
        let stripe_bg = ratatui_color(theme.row_alt_bg(theme.background));

        let col3 = layout
            .cols
            .iter()
            .find(|c| c.index == 3)
            .expect("column 3 is visible");
        let (cx, _) = span_bounds(area.width, area.x, col3).unwrap();

        let bg_at_row = |abs_row: usize| {
            let row = layout.rows.iter().find(|r| r.index == abs_row).unwrap();
            let (ry, _) = span_bounds(area.height, area.y, row).unwrap();
            buf[(cx, ry)].bg
        };

        assert_ne!(bg_at_row(0), stripe_bg, "frozen row 0 must never stripe");
        assert_eq!(bg_at_row(1), stripe_bg, "scrolled odd row 1 must stripe");
        assert_ne!(
            bg_at_row(2),
            stripe_bg,
            "scrolled even row 2 must not stripe"
        );
    }

    #[test]
    fn striped_cursor_cell_never_tints_even_on_an_odd_row() {
        let mut grid = grid();
        grid.striped = true;
        grid.cursor = (3, 3);
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let layout = draw_cell_grid(&mut buf, area, &grid, &theme);
        let stripe_bg = ratatui_color(theme.row_alt_bg(theme.background));

        let row3 = layout.rows.iter().find(|r| r.index == 3).unwrap();
        let (ry, _) = span_bounds(area.height, area.y, row3).unwrap();
        let col3 = layout.cols.iter().find(|c| c.index == 3).unwrap();
        let (cx3, _) = span_bounds(area.width, area.x, col3).unwrap();
        assert_eq!(
            buf[(cx3, ry)].bg,
            ratatui_color(theme.selection_bg),
            "the cursor cell wins over the stripe"
        );

        // A neighbour on the same odd row, not the cursor, still stripes.
        let col2 = layout.cols.iter().find(|c| c.index == 2).unwrap();
        let (cx2, _) = span_bounds(area.width, area.x, col2).unwrap();
        assert_eq!(buf[(cx2, ry)].bg, stripe_bg);
    }

    #[test]
    fn clicking_a_separator_cell_hit_tests_to_the_column_on_its_left() {
        // The separator glyph lives inside the column's own resolved
        // span (`CellGrid::col_text_cells` only narrows the *text*, never
        // the span `hit_test` walks), so a click there needs no dedicated
        // hit-test handling — it already resolves to that same column.
        let mut grid = grid();
        grid.column_separators = true;
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let layout = draw_cell_grid(&mut buf, area, &grid, &theme);

        let col2 = layout
            .cols
            .iter()
            .find(|c| c.index == 2)
            .expect("column 2 is visible");
        let (cx, cw) = span_bounds(area.width, area.x, col2).unwrap();
        let div_x = cx + cw - 1;
        let row2 = layout.rows.iter().find(|r| r.index == 2).unwrap();
        let (ry, _) = span_bounds(area.height, area.y, row2).unwrap();
        assert_eq!(
            buf[(div_x, ry)].symbol(),
            "│",
            "sanity: the separator glyph is where expected"
        );

        assert_eq!(
            layout.hit_test(div_x as f32 + 0.5, ry as f32 + 0.5),
            CellGridHit::Cell(2, 2),
            "a click on column 2's own separator cell selects column 2, not column 3"
        );
    }

    #[test]
    fn striped_tracks_absolute_row_index_after_scrolling() {
        let mut grid = grid();
        grid.striped = true;
        grid.row_scroll = 2;
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let layout = draw_cell_grid(&mut buf, area, &grid, &theme);
        let stripe_bg = ratatui_color(theme.row_alt_bg(theme.background));

        // Absolute row 3 is still odd after scrolling past rows 1-2, so
        // it keeps its stripe wherever it now lands on screen.
        let row3 = layout
            .rows
            .iter()
            .find(|r| r.index == 3)
            .expect("row 3 is visible after scrolling");
        let (ry, _) = span_bounds(area.height, area.y, row3).unwrap();
        let col3 = layout.cols.iter().find(|c| c.index == 3).unwrap();
        let (cx, _) = span_bounds(area.width, area.x, col3).unwrap();
        assert_eq!(buf[(cx, ry)].bg, stripe_bg);
    }

    /// The inline editor paints through the text field's row painter: a
    /// selection inside the edit buffer is reverse video (the field's
    /// foreground behind the selected glyphs), not a blended tint.
    #[test]
    fn inline_editor_selection_is_reverse_video_like_a_text_field() {
        use crate::primitives::cell_grid::CellEditState;

        let mut grid = grid();
        grid.cursor = (3, 3);
        grid.editing = Some(CellEditState {
            text: "abcd".into(),
            cursor: 3,
            selection_anchor: Some(1),
            placeholder: None,
        });
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let layout = draw_cell_grid(&mut buf, area, &grid, &theme);
        let row = layout.rows.iter().find(|r| r.index == 3).unwrap();
        let col = layout.cols.iter().find(|c| c.index == 3).unwrap();
        let (cx, _) = span_bounds(area.width, area.x, col).unwrap();
        let (cy, _) = span_bounds(area.height, area.y, row).unwrap();

        // "b" and "c" (bytes 1..3) are selected.
        for x in [cx + 1, cx + 2] {
            assert_eq!(buf[(x, cy)].bg, ratatui_color(theme.foreground));
            assert_eq!(buf[(x, cy)].fg, ratatui_color(theme.selection_bg));
        }
        // "a" is not.
        assert_eq!(buf[(cx, cy)].bg, ratatui_color(theme.selection_bg));
    }

    /// An empty edit buffer shows its placeholder dimmed, with no caret
    /// block painted over it — the text field's empty-value rendering.
    #[test]
    fn inline_editor_empty_buffer_shows_a_dim_placeholder_without_a_caret_block() {
        use crate::primitives::cell_grid::CellEditState;

        let mut grid = grid();
        grid.cursor = (3, 3);
        grid.editing = Some(CellEditState {
            text: String::new(),
            cursor: 0,
            selection_anchor: None,
            placeholder: Some("fx".into()),
        });
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let layout = draw_cell_grid(&mut buf, area, &grid, &theme);
        let row = layout.rows.iter().find(|r| r.index == 3).unwrap();
        let col = layout.cols.iter().find(|c| c.index == 3).unwrap();
        let (cx, _) = span_bounds(area.width, area.x, col).unwrap();
        let (cy, _) = span_bounds(area.height, area.y, row).unwrap();

        assert_eq!(buf[(cx, cy)].symbol(), "f");
        assert_eq!(buf[(cx, cy)].fg, ratatui_color(theme.muted_fg));
        assert_eq!(buf[(cx, cy)].bg, ratatui_color(theme.selection_bg));
    }

    /// The grid painter with frozen panes (the row boundary underlined in
    /// place, including over a wide glyph), a scrolled body, the cursor
    /// and a selection, cell for cell against the golden.
    #[test]
    fn cell_paint_matches_the_golden() {
        use crate::tui::cell_surface::oracle;

        let theme = Theme::default();
        let mut g = grid();
        g.cells[0][2] = cell_at("\u{4e2d}\u{6587}x");
        g.row_scroll = 1;
        g.col_scroll = 1;
        g.cursor = (3, 3);
        g.selection_anchor = Some((2, 2));
        let mut plain = grid();
        plain.column_separators = true;
        plain.striped = true;
        let paint = |g: &CellGrid, area: Rect, w, h| {
            oracle::paint(w, h, |s| {
                let _ = draw_cell_grid(s, area, g, &theme);
            })
        };
        oracle::assert_golden(
            "cell_grid",
            &[
                ("frozen_scrolled", &paint(&g, Rect::new(0, 0, 40, 8), 40, 8)),
                ("offset", &paint(&g, Rect::new(2, 1, 30, 6), 33, 8)),
                ("separators", &paint(&plain, Rect::new(0, 0, 40, 8), 40, 8)),
            ],
        );
    }
}
