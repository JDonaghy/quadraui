//! TUI rasteriser for [`crate::DataTable`].
//!
//! Renders column headers with sort indicators, then body rows with
//! per-cell text aligned within resolved column bounds. Selected row
//! uses `theme.selection_bg`. Focused table highlights the selected
//! row more prominently.
//!
//! `table.bordered` draws a rounded frame around `area` and insets all
//! content by 1 cell, matching the inset `DataTable::layout` reserves.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::Position;
use ratatui::style::{Color as RatatuiColor, Modifier, Style};

use super::{ratatui_color, set_cell, set_cell_wide};
use crate::primitives::data_table::{
    ColumnAlign, ColumnMeasure, DataRow, DataTable, DataTableGlyphs, DataTableLayout,
    DataTablePaintState, ResolvedColumn, SortDirection,
};
use crate::primitives::row_stack::stack_rows_from;
use crate::primitives::tree_table::{TreeTable, TreeTableLayout, TreeTableNode};
use crate::theme::Theme;
use crate::types::TreeStyle;

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
/// override for the column-header sort-direction indicators;
/// `nerd_fonts_enabled` picks each override [`Icon`][crate::Icon]'s
/// `glyph` vs. `fallback` form (see
/// [`crate::Backend::nerd_fonts_enabled`]).
///
/// `glyphs: None`, and any direction the override leaves `None`, keeps
/// TUI's own plain Unicode `▲`/`▼` — not the GUI backends' codicon
/// defaults (`crate::codicon::SORT_ASCENDING`/`SORT_DESCENDING`).
pub fn draw_data_table_with_glyphs(
    buf: &mut Buffer,
    area: Rect,
    table: &DataTable,
    theme: &Theme,
    hovered_idx: Option<usize>,
    glyphs: Option<&DataTableGlyphs>,
    nerd_fonts_enabled: bool,
) -> DataTableLayout {
    let state = DataTablePaintState::default().with_hovered_idx(hovered_idx);
    draw_data_table_inner(buf, area, table, theme, &state, glyphs, nerd_fonts_enabled)
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
    draw_data_table_inner(buf, area, table, theme, state, None, false)
}

/// Shared paint context for [`draw_header`], [`draw_row`] and
/// [`draw_cell`]; columns clip against `area`.
#[derive(Clone, Copy)]
pub(crate) struct RowPaintCtx<'a> {
    pub table: &'a DataTable,
    pub layout: &'a DataTableLayout,
    pub theme: &'a Theme,
    pub area: Rect,
}

impl RowPaintCtx<'_> {
    /// 1-cell inset on every edge when bordered (matches `layout.inset_*`).
    fn ox(&self) -> u16 {
        u16::from(self.table.bordered)
    }

    /// Interior x bounds (right exclusive) all content clips to.
    fn bounds(&self) -> (i32, i32) {
        let ox = self.ox() as i32;
        let x = self.area.x as i32;
        (x + ox, x + self.area.width as i32 - ox)
    }

    /// Column screen x + cell-rounded width after the rounded `h_scroll`
    /// `data_table_layout` carries, or `None` when clipped fully outside
    /// [`Self::bounds`] or rounded to zero width.
    fn column(&self, rc: &ResolvedColumn) -> Option<(i32, u16)> {
        let (x0, x1) = self.bounds();
        let x = x0 + rc.x.round() as i32 - self.layout.h_scroll as i32;
        let w = rc.width.round() as u16;
        (x + w as i32 > x0 && x < x1 && w != 0).then_some((x, w))
    }
}

/// Width a cell may paint into: one less than the column, reserving the
/// right-edge separator cell every column but the last one carries.
fn usable_width(ctx: &RowPaintCtx<'_>, col_idx: usize, col_w: u16) -> u16 {
    col_w.saturating_sub(u16::from(col_idx + 1 < ctx.table.columns.len()))
}

/// Fill a full-width band at `y` with `(fg, bg)`, then walk every
/// visible column, handing its index and screen bounds to `cell`. With
/// `separators`, each column but the last also gets its right-edge
/// divider glyph first — the header and body rows want it, the footer
/// does not. The shared skeleton of all three bands.
fn draw_band(
    buf: &mut Buffer,
    ctx: &RowPaintCtx<'_>,
    y: u16,
    (fg, bg): (RatatuiColor, RatatuiColor),
    separators: bool,
    mut cell: impl FnMut(&mut Buffer, usize, i32, u16),
) {
    let (area, ox) = (ctx.area, ctx.ox());
    let (x0, x1) = ctx.bounds();
    let sep_fg = ratatui_color(ctx.theme.separator);
    for x in ox..area.width.saturating_sub(ox) {
        set_cell(buf, area.x + x, y, ' ', fg, bg);
    }
    for (col_idx, rc) in ctx.layout.columns.iter().enumerate() {
        let Some((col_x, col_w)) = ctx.column(rc) else {
            continue;
        };
        let sep_cx = col_x + col_w as i32 - 1;
        if separators && col_idx + 1 < ctx.table.columns.len() && sep_cx >= x0 && sep_cx < x1 {
            set_cell(buf, sep_cx as u16, y, '│', sep_fg, bg);
        }
        cell(buf, col_idx, col_x, col_w);
    }
}

/// Paint `chars` left to right from `x` at `y` over `bg` in bold, each
/// with its own foreground, skipping cells left of `clip.0` and stopping
/// at `clip.1`. Shared by the header titles and the footer spans.
fn draw_bold_chars(
    buf: &mut Buffer,
    x: i32,
    y: u16,
    clip: (i32, i32),
    bg: RatatuiColor,
    chars: impl Iterator<Item = (char, RatatuiColor)>,
) {
    for (i, (ch, fg)) in chars.enumerate() {
        let cx = x + i as i32;
        if cx >= clip.1 {
            break;
        }
        if cx >= clip.0 {
            set_cell(buf, cx as u16, y, ch, fg, bg);
            if let Some(cell) = buf.cell_mut(Position::new(cx as u16, y)) {
                cell.set_style(Style::default().add_modifier(Modifier::BOLD));
            }
        }
    }
}

/// Header row: background fill, title + sort glyph per column,
/// separators.
pub(crate) fn draw_header(
    buf: &mut Buffer,
    header_y: u16,
    ctx: &RowPaintCtx<'_>,
    glyphs: Option<&DataTableGlyphs>,
    nerd_fonts_enabled: bool,
) {
    let table = ctx.table;
    let fg = ratatui_color(ctx.theme.foreground);
    let bg = ratatui_color(ctx.theme.tab_bar_bg);
    let (x0, x1) = ctx.bounds();
    let sort = table.sort.map(|(si, dir)| {
        let (icon, plain) = match dir {
            SortDirection::Ascending => (glyphs.and_then(|g| g.sort_ascending_glyph()), "▲"),
            SortDirection::Descending => (glyphs.and_then(|g| g.sort_descending_glyph()), "▼"),
        };
        (
            si,
            icon.map_or(plain, |icon| icon.resolve(nerd_fonts_enabled)),
        )
    });
    let paint = |buf: &mut Buffer, col_idx: usize, col_x: i32, col_w: u16| {
        let Some(col) = table.columns.get(col_idx) else {
            return;
        };
        let title = match sort {
            Some((si, glyph)) if si == col_idx => format!("{} {glyph}", col.title),
            _ => col.title.clone(),
        };
        let usable_w = usable_width(ctx, col_idx, col_w);
        let start = align_offset(col.align, title.chars().count() as u16, usable_w) as i32;
        let clip = (x0, x1.min(col_x + usable_w as i32));
        let chars = title.chars().map(|ch| (ch, fg));
        draw_bold_chars(buf, col_x + start, header_y, clip, bg, chars);
    };
    draw_band(buf, ctx, header_y, (fg, bg), true, paint);
}

/// Given a column index + visible (clipped) cell rect, paint it and
/// return `true`, or decline (`false`) to fall through to [`draw_cell`].
/// The hook owns the whole visible column, separator cell included: the
/// separator paints before the hook runs, so a hook that fills its rect
/// erases it. The default [`draw_cell`] reserves it via [`usable_width`].
pub(crate) type CellPaintHook<'a> = dyn FnMut(&mut Buffer, usize, Rect) -> bool + 'a;

/// Default body-cell text painter — ellipsis-truncated by display
/// width, muted or per-span coloured — [`draw_row`]'s `cell_hook`
/// stands in for.
pub(crate) fn draw_cell(
    buf: &mut Buffer,
    ctx: &RowPaintCtx<'_>,
    row: &DataRow,
    col_idx: usize,
    (col_x_raw, col_w): (i32, u16),
    y: u16,
    (row_fg, row_bg): (RatatuiColor, RatatuiColor),
) {
    let styled = match row.cells.get(col_idx) {
        Some(c) if !c.spans.is_empty() => c,
        _ => return,
    };
    let full_text: String = styled.spans.iter().map(|s| s.text.as_str()).collect();
    if full_text.is_empty() {
        return;
    }
    let (table, (inner_x0, inner_x1)) = (ctx.table, ctx.bounds());

    let align = column_align(table, col_idx);
    let usable_w = usable_width(ctx, col_idx, col_w);

    // `displayed_len` is the run's cell width, which diverges from
    // `visible_chars` whenever a wide glyph is in play — so it, not the
    // char count, is what `align_offset` positions by.
    let (visible_chars, needs_ellipsis, displayed_len) =
        crate::text_util::fit_chars_to_cells(&full_text, usable_w);
    let start = align_offset(align, displayed_len, usable_w) as i32;

    let is_muted = row.decoration == crate::types::Decoration::Muted;
    let mut chars_emitted = 0u16;
    let mut width_used = 0u16;
    'cell: for span in &styled.spans {
        let span_fg = if is_muted {
            ratatui_color(ctx.theme.muted_fg)
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

/// Selected/hovered/zebra-striped `(row_fg, row_bg)` for body row
/// `abs_idx` — shared by [`draw_row`] and `tree_table`'s per-row tree-cell
/// hook (`draw_tree_table_row`) so both resolve the exact same colors
/// instead of each re-deriving this precedence.
///
/// Zebra stripe on the absolute row index (not the visible on-screen
/// position), so a given row's shading survives scrolling, the same
/// convention Finder/Explorer use. Selection and hover both take
/// priority over the stripe — it only shows through on an otherwise
/// plain row. A terminal quantised to `ColorDepth::Ansi16` rounds
/// `theme.row_alt_bg` back to the background's own nearest ANSI colour
/// (see `tui::color::quantize`), which is how a low-colour terminal
/// drops the stripe for free.
pub(crate) fn resolve_row_colors(
    ctx: &RowPaintCtx<'_>,
    abs_idx: usize,
    hovered_idx: Option<usize>,
    striped: bool,
) -> (RatatuiColor, RatatuiColor) {
    let theme = ctx.theme;
    let is_selected = ctx.table.selected_idx == Some(abs_idx);
    let is_hovered = hovered_idx == Some(abs_idx) && !is_selected;
    let row_fg = ratatui_color(theme.foreground);
    let row_bg = ratatui_color(if is_selected {
        theme.selection_bg
    } else if is_hovered {
        theme.tab_bar_bg
    } else if striped && abs_idx % 2 == 1 {
        theme.row_alt_bg(theme.background)
    } else {
        theme.background
    });
    (row_fg, row_bg)
}

/// Paint one body row: selection/hover/zebra-stripe background, every
/// column's cell (via `cell_hook` when present, else [`draw_cell`]),
/// then the column-divider separators.
pub(crate) fn draw_row(
    buf: &mut Buffer,
    y: u16,
    ctx: &RowPaintCtx<'_>,
    abs_idx: usize,
    hovered_idx: Option<usize>,
    striped: bool,
    mut cell_hook: Option<&mut CellPaintHook<'_>>,
) {
    let table = ctx.table;
    let row = &table.rows[abs_idx];
    let (row_fg, row_bg) = resolve_row_colors(ctx, abs_idx, hovered_idx, striped);

    let (x0, x1) = ctx.bounds();
    let paint = |buf: &mut Buffer, col_idx: usize, col_x: i32, col_w: u16| {
        if let Some(hook) = cell_hook.as_mut() {
            let vx0 = col_x.max(x0);
            let vx1 = (col_x + col_w as i32).min(x1);
            let cell_rect = Rect::new(vx0 as u16, y, (vx1 - vx0).max(0) as u16, 1);
            if hook(buf, col_idx, cell_rect) {
                return;
            }
        }
        draw_cell(buf, ctx, row, col_idx, (col_x, col_w), y, (row_fg, row_bg));
    };
    draw_band(buf, ctx, y, (row_fg, row_bg), true, paint);
}

fn draw_data_table_inner(
    buf: &mut Buffer,
    area: Rect,
    table: &DataTable,
    theme: &Theme,
    state: &DataTablePaintState,
    glyphs: Option<&DataTableGlyphs>,
    nerd_fonts_enabled: bool,
) -> DataTableLayout {
    let layout = data_table_layout(area, table);
    draw_data_table_body(
        buf,
        area,
        table,
        theme,
        state,
        glyphs,
        nerd_fonts_enabled,
        layout,
        |buf, y, ctx, idx, hovered, striped| draw_row(buf, y, ctx, idx, hovered, striped, None),
    )
}

/// [`draw_data_table_inner`]'s header/body/scrollbar/footer skeleton,
/// generalised over how a body row paints: `body_row(buf, y, ctx, abs_idx,
/// hovered_idx, striped)` stands in for [`draw_row`]'s own default call,
/// so [`draw_tree_table`] can paint column 0 as a tree cell while reusing
/// every other line of this function unchanged — header, scrollbars,
/// footer and the rest of the body row included. Takes `layout` already
/// resolved (rather than computing it from `table`) so a caller that also
/// needs the layout for its own purposes (e.g. [`tree_table_layout`]'s
/// chevron bounds) never resolves it twice.
#[allow(clippy::too_many_arguments)]
fn draw_data_table_body(
    buf: &mut Buffer,
    area: Rect,
    table: &DataTable,
    theme: &Theme,
    state: &DataTablePaintState,
    glyphs: Option<&DataTableGlyphs>,
    nerd_fonts_enabled: bool,
    layout: DataTableLayout,
    mut body_row: impl FnMut(&mut Buffer, u16, &RowPaintCtx<'_>, usize, Option<usize>, bool),
) -> DataTableLayout {
    if area.width == 0 || area.height == 0 {
        return layout;
    }

    let ctx = RowPaintCtx {
        table,
        layout: &layout,
        theme,
        area,
    };
    let (ox, oy) = (ctx.ox(), ctx.ox());
    let (inner_x0, inner_x1) = ctx.bounds();
    if table.bordered {
        draw_data_table_border(buf, area, theme);
    }

    // ── Header row ────────────────────────────────────────────────────
    draw_header(buf, area.y + oy, &ctx, glyphs, nerd_fonts_enabled);

    // ── Body rows ─────────────────────────────────────────────────────
    // `scroll_px` wins over `scroll_offset` when set, quantised to a
    // whole row — TUI never reads the sub-row remainder, so the
    // `scroll_sub_px` [`stack_rows_from`] shifts the first row by is
    // always `0.0` here and every row lands on a whole cell.
    let scroll_offset = table.effective_scroll_offset(layout.row_height, state.scroll_px);
    let visible = layout
        .visible_rows
        .min(table.rows.len().saturating_sub(scroll_offset));
    let body_y = (area.y + oy + 1) as f32;
    let (hovered, striped) = (state.hovered_idx, state.striped);
    for r in stack_rows_from(scroll_offset, 0.0, visible, 0.0, body_y, 0.0, |_| (1.0, ())) {
        body_row(buf, r.bounds.y as u16, &ctx, r.idx, hovered, striped);
    }

    // ── Scrollbar ──────────────────────────────────────────────────────
    let footer_h = layout.footer_height.round() as u16;
    if table.show_scrollbar
        && table.rows.len() > layout.visible_rows
        && layout.scrollbar_width > 0.0
    {
        let sb_x = (area.x + area.width).saturating_sub(ox + layout.scrollbar_width.round() as u16);
        let sb_h = area.height.saturating_sub(2 * oy + 1 + footer_h) as f32;
        let sb_track = crate::event::Rect::new(sb_x as f32, (area.y + oy + 1) as f32, 1.0, sb_h);
        // `scroll_px: None`: `scroll_offset` is already quantised to a
        // whole row, and the thumb must not reintroduce a fractional part.
        let sb = crate::primitives::row_stack::vscrollbar_thumb(
            table.id,
            sb_track,
            scroll_offset,
            None,
            table.rows.len(),
            1.0,
            layout.visible_rows as f32,
        );
        super::draw_scrollbar(buf, &sb, theme, theme.background);
    }

    // ── Horizontal scrollbar ─────────────────────────────────────────
    if layout.h_scrollbar_height > 0.0 && layout.content_width > 0.0 {
        let hsb_y = (area.y + area.height)
            .saturating_sub(oy + footer_h + layout.h_scrollbar_height.round() as u16);
        let visible_w = (area.width as f32 - 2.0 * ox as f32 - layout.scrollbar_width).max(1.0);
        let hsb_track = crate::event::Rect::new((area.x + ox) as f32, hsb_y as f32, visible_w, 1.0);
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
    let Some(footer) = table.footer.as_ref().filter(|_| footer_h > 0) else {
        return layout;
    };
    // Content always sits on the viewport's bottom-most row; the divider
    // gets the row directly above it. Both rows are reserved via
    // `footer_height == row_height * 2.0` (see `DataTable::layout`), so
    // neither is ever painted over by a body row.
    let footer_y = (area.y + area.height).saturating_sub(1).saturating_sub(oy);
    let bg = ratatui_color(theme.tab_bar_bg);
    let fg = ratatui_color(theme.foreground);

    // Divider rule directly above the footer row, never on the top border.
    if footer_y > area.y + oy {
        let sep_fg = ratatui_color(theme.separator);
        for x in ox..area.width.saturating_sub(ox) {
            set_cell(buf, area.x + x, footer_y - 1, '─', sep_fg, bg);
        }
    }

    let paint = |buf: &mut Buffer, col_idx: usize, col_x: i32, col_w: u16| {
        let Some(styled) = footer.cells.get(col_idx).filter(|c| !c.spans.is_empty()) else {
            return;
        };
        let chars: Vec<_> = styled
            .spans
            .iter()
            .flat_map(|span| {
                let span_fg = span.fg.map(ratatui_color).unwrap_or(fg);
                span.text.chars().map(move |ch| (ch, span_fg))
            })
            .collect();
        let align = column_align(table, col_idx);
        let start = align_offset(align, chars.len() as u16, col_w) as i32;
        let (clip, chars) = ((inner_x0, inner_x1), chars.into_iter());
        draw_bold_chars(buf, col_x + start, footer_y, clip, bg, chars);
    };
    draw_band(buf, &ctx, footer_y, (fg, bg), false, paint);

    layout
}

// ── TreeTable ───────────────────────────────────────────────────────────
//
// No `tui/tree_table.rs`: `TreeTable` is `DataTable` plus a tree cell on
// column 0, so its TUI story is "the functions above, plus the tree-cell
// prefix" — not a parallel painter.

/// `(start, width)`, relative to column 0's left edge, of `node`'s
/// chevron glyph — [`crate::primitives::tree_table::TreeTable::layout`]'s
/// `measure_chevron` callback for TUI. `None` for a leaf or a hidden
/// chevron. Reuses [`super::tree::chevron_glyph_end`] — the exact formula
/// [`crate::tui::draw_tree`] already paints a `TreeView` branch's chevron
/// with — instead of re-deriving the indent arithmetic.
fn tui_tree_chevron_measure(style: &TreeStyle, node: &TreeTableNode) -> Option<(f32, f32)> {
    if !style.show_chevrons {
        return None;
    }
    let expanded = node.is_expanded?;
    let (glyph, end) = super::tree::chevron_glyph_end(style, node.indent, expanded);
    let width = glyph.chars().count();
    Some(((end - width) as f32, width as f32))
}

/// The narrow-rect floor [`TreeTable::layout`]'s `min_tree_col_width`
/// takes: the deepest indent level across every row in `tree`, plus one
/// chevron glyph (when `style` shows chevrons at all — a leaf still pays
/// the indent even though it paints no chevron of its own) and at least
/// one character for the label. Computed from the whole tree rather than
/// only the rows that happen to be visible today, so the floor — and
/// therefore which columns are dropped — doesn't change as the table
/// scrolls.
fn tree_column_floor_width(style: &TreeStyle, tree: &[TreeTableNode]) -> f32 {
    let chevron_width = if style.show_chevrons {
        style
            .chevron_expanded
            .chars()
            .count()
            .max(style.chevron_collapsed.chars().count())
    } else {
        0
    };
    let max_indent = tree.iter().map(|n| n.indent).max().unwrap_or(0);
    (max_indent as usize * style.indent as usize + chevron_width + 1) as f32
}

/// Resolve the layout [`draw_tree_table`] paints `table` at, without
/// touching a buffer — the `TreeTable` counterpart of [`data_table_layout`].
///
/// Applies the narrow-rect rule: column 0 (the tree cell) never resolves
/// below [`tree_column_floor_width`] (indent plus chevron plus one label
/// character) — rightmost columns are dropped, and failing that column 0
/// is widened past the viewport, before that floor is ever truncated.
/// See [`crate::primitives::tree_table::TreeTable::layout`]'s
/// `min_tree_col_width` doc for the exact rule.
pub fn tree_table_layout(area: Rect, table: &TreeTable) -> TreeTableLayout {
    let style = TreeStyle::default();
    let floor = tree_column_floor_width(&style, &table.tree);
    let mut layout = table.layout(
        area.width as f32,
        area.height as f32,
        1.0,
        1.0,
        1.0,
        |col| ColumnMeasure::new(col.title.chars().count() as f32),
        |node| tui_tree_chevron_measure(&style, node),
        floor,
    );
    // Same cell-granular rounding `data_table_layout` applies, and for the
    // same reason: `draw_tree_table` paints every column at `h_scroll.round()`.
    layout.table.h_scroll = table.table.h_scroll.round();
    // Clamp chevron hit bounds to column 0's resolved width — `measure`
    // above returns bounds relative to column 0's left edge, unclipped,
    // but a narrow column 0 (whether from the floor above or a plain
    // override) can resolve narrower than the deepest indent's chevron.
    // `paint_tree_cell` already clips what it paints; without this, a
    // click just past the clipped glyph would still hit-test as
    // `Chevron` even though nothing painted there.
    if let Some(col0_width) = layout.table.columns.first().map(|c| c.width) {
        layout.chevrons.retain_mut(|(_, bounds)| {
            if bounds.x >= col0_width {
                return false;
            }
            bounds.width = bounds.width.min(col0_width - bounds.x);
            true
        });
    }
    layout
}

/// Paint one `TreeTable` body row: [`draw_row`]'s selection/hover
/// background (zebra striping isn't offered — mirrors
/// [`draw_tree_table`] never taking a `striped` argument) and every
/// column but 0 exactly as [`draw_cell`] already paints it, with column 0
/// painted as indent + chevron + icon via [`super::tree::paint_tree_cell`],
/// followed by its label through the same [`draw_cell`] every other
/// column uses — offset past the tree-cell prefix rather than
/// re-implementing text painting.
#[allow(clippy::too_many_arguments)]
fn draw_tree_table_row(
    buf: &mut Buffer,
    y: u16,
    ctx: &RowPaintCtx<'_>,
    abs_idx: usize,
    node: &TreeTableNode,
    hovered_idx: Option<usize>,
    style: &TreeStyle,
    nerd_fonts_enabled: bool,
) {
    let (row_fg, row_bg) = resolve_row_colors(ctx, abs_idx, hovered_idx, false);
    let mut hook = |buf: &mut Buffer, col_idx: usize, cell_rect: Rect| -> bool {
        if col_idx != 0 {
            return false;
        }
        let (_, text_start) = super::tree::paint_tree_cell(
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
    draw_row(buf, y, ctx, abs_idx, hovered_idx, false, Some(&mut hook));
}

/// Draw a `TreeTable` into `area`. Identical to [`draw_data_table`] except
/// column 0 of every row carries indent + chevron + icon ahead of its
/// label (rows beyond `table.tree`'s length paint as a plain `DataTable`
/// row — see [`TreeTable::tree`](crate::primitives::tree_table::TreeTable::tree)'s
/// doc). Returns the layout used for painting, including chevron hit
/// bounds, so callers hit-test at the same coordinates.
pub fn draw_tree_table(
    buf: &mut Buffer,
    area: Rect,
    table: &TreeTable,
    theme: &Theme,
    hovered_idx: Option<usize>,
    nerd_fonts_enabled: bool,
) -> TreeTableLayout {
    let style = TreeStyle::default();
    let tt_layout = tree_table_layout(area, table);
    let state = DataTablePaintState::default().with_hovered_idx(hovered_idx);
    let dt_layout = draw_data_table_body(
        buf,
        area,
        &table.table,
        theme,
        &state,
        None,
        nerd_fonts_enabled,
        tt_layout.table.clone(),
        |buf, y, ctx, idx, hovered, striped| match table.tree.get(idx) {
            Some(node) => {
                draw_tree_table_row(buf, y, ctx, idx, node, hovered, &style, nerd_fonts_enabled)
            }
            None => draw_row(buf, y, ctx, idx, hovered, striped, None),
        },
    );
    TreeTableLayout {
        table: dt_layout,
        chevrons: tt_layout.chevrons,
    }
}

/// Paint a rounded `╭─╮ │ ╰─╯` frame around `area` in `border_fg` and fill
/// the interior with `(fg, bg)`, so a focus ring painted over the same
/// `area` only recolours the frame glyphs, never the content inside it.
fn draw_data_table_border(buf: &mut Buffer, area: Rect, theme: &Theme) {
    let border_fg = ratatui_color(theme.border_fg);
    let fg = ratatui_color(theme.foreground);
    let bg = ratatui_color(theme.background);
    let (w, h) = (area.width, area.height);
    for row in 0..h {
        let (l, mid, r) = match row {
            0 => ('╭', '─', '╮'),
            _ if row + 1 == h => ('╰', '─', '╯'),
            _ => ('│', ' ', '│'),
        };
        for col in 0..w {
            let edge = col == 0 || col + 1 == w;
            let ch = match col {
                0 => l,
                c if c + 1 == w => r,
                _ => mid,
            };
            let cfg = if edge || mid == '─' { border_fg } else { fg };
            set_cell(buf, area.x + col, area.y + row, ch, cfg, bg);
        }
    }
}

/// Column `col_idx`'s alignment, `Left` when the column is missing.
fn column_align(table: &DataTable, col_idx: usize) -> ColumnAlign {
    table
        .columns
        .get(col_idx)
        .map_or(ColumnAlign::Left, |c| c.align)
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

    /// `state.striped: true` tints every other non-selected body row
    /// with `theme.row_alt_bg(theme.background)`; the selected row keeps
    /// `theme.selection_bg` regardless of its parity, and `striped:
    /// false` (the default) paints every row with the plain background.
    #[test]
    fn striped_tints_alternate_rows_selection_wins_over_stripe() {
        let mut table = make_scrolling_table(6);
        table.selected_idx = Some(1); // odd row — would be striped if not selected
        let area = Rect::new(0, 0, 40, 10);
        let theme = Theme::default();

        let mut unstriped = Buffer::empty(area);
        draw_data_table(&mut unstriped, area, &table, &theme, None);

        let mut striped_buf = Buffer::empty(area);
        let state = DataTablePaintState::default().with_striped(true);
        draw_data_table_with_state(&mut striped_buf, area, &table, &theme, &state);

        let body_bg = ratatui_color(theme.background);
        let alt_bg = ratatui_color(theme.row_alt_bg(theme.background));
        let sel_bg = ratatui_color(theme.selection_bg);
        assert_ne!(
            alt_bg, body_bg,
            "fixture theme must make the stripe visible"
        );

        // Row 0 (even, not selected): plain background without striping,
        // tinted with it.
        assert_eq!(unstriped[(0, 1)].bg, body_bg);
        assert_eq!(striped_buf[(0, 1)].bg, body_bg);

        // Row 1 (odd, selected): selection colour either way — the
        // stripe never overrides the selected row.
        assert_eq!(unstriped[(0, 2)].bg, sel_bg);
        assert_eq!(striped_buf[(0, 2)].bg, sel_bg);

        // Row 2 (even, not selected): unaffected by striping.
        assert_eq!(striped_buf[(0, 3)].bg, body_bg);

        // Row 3 (odd, not selected): only tinted when striped.
        assert_eq!(unstriped[(0, 4)].bg, body_bg);
        assert_eq!(striped_buf[(0, 4)].bg, alt_bg);
    }

    /// `theme.row_alt_bg`'s tint is faint enough that a terminal
    /// quantised to [`crate::backend::ColorDepth::Ansi16`] rounds it
    /// back to the same ANSI colour as the plain background, so the
    /// stripe disappears on a low-colour terminal instead of being
    /// substituted with something misleading.
    #[test]
    fn striped_row_colour_drops_on_ansi16_terminals() {
        use crate::backend::ColorDepth;
        use crate::tui::color::quantize;

        let theme = Theme::default();
        let body_bg = ratatui_color(theme.background);
        let alt_bg = ratatui_color(theme.row_alt_bg(theme.background));
        assert_ne!(
            alt_bg, body_bg,
            "fixture theme must make the stripe visible at full colour depth"
        );
        assert_eq!(
            quantize(alt_bg, ColorDepth::Ansi16),
            quantize(body_bg, ColorDepth::Ansi16),
            "an Ansi16 terminal must quantise the stripe back to the row's \
             own background, dropping it rather than showing a wrong colour"
        );
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

    /// [`draw_row`]'s `cell_hook` is tried for every column before the
    /// default [`draw_cell`] text painter: declining (`false`) for a
    /// column falls through to the default, so a consumer (e.g.
    /// `TreeTable`) can override only column 0 while every other column
    /// still renders exactly as [`draw_cell`] always has.
    #[test]
    fn draw_row_hook_paints_column_0_default_paints_the_rest() {
        let table = make_table();
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let layout = data_table_layout(area, &table);

        let mut hook_calls: Vec<usize> = Vec::new();
        let mut hook = |buf: &mut Buffer, col_idx: usize, cell_rect: Rect| -> bool {
            hook_calls.push(col_idx);
            let reset = RatatuiColor::Reset;
            for x in cell_rect.x..cell_rect.x + cell_rect.width * u16::from(col_idx == 0) {
                set_cell(buf, x, cell_rect.y, '#', reset, reset);
            }
            col_idx == 0
        };
        let (table, layout, theme) = (&table, &layout, &Theme::default());
        let ctx = RowPaintCtx {
            table,
            layout,
            theme,
            area,
        };
        draw_row(&mut buf, 1, &ctx, 0, None, false, Some(&mut hook));

        assert_eq!(hook_calls, vec![0, 1, 2]);
        // Column 0 ("pod-abc") was painted by the hook's own '#' fill,
        // not the default text painter.
        let row: String = (0..40).map(|x| buf[(x, 1)].symbol()).collect();
        assert!(row.starts_with('#'), "column 0 not hook-painted: {row}");
        assert!(find_char_pos(&row, "pod-abc").is_none());
        // Columns 1 ("Running") and 2 ("3d") declined by the hook, so
        // they fell through to the default `draw_cell` text painter.
        assert!(find_char_pos(&row, "Running").is_some());
        assert!(find_char_pos(&row, "3d").is_some());
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

#[cfg(test)]
mod tree_table_tests {
    use super::*;
    use crate::primitives::data_table::{Column, ColumnAlign, ColumnWidth, DataRow};
    use crate::types::{Decoration, StyledText, WidgetId};

    fn make_tree_table() -> TreeTable {
        TreeTable {
            table: DataTable {
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
            },
            tree: vec![
                TreeTableNode::new(vec![0], 0).with_expanded(true),
                TreeTableNode::new(vec![0, 0], 1),
                TreeTableNode::new(vec![1], 0).with_expanded(false),
            ],
        }
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
        draw_tree_table(&mut buf, area, &table, &Theme::default(), None, false);
        // Row 0 ("src", expanded, indent 0) paints its chevron at column 0
        // and the label somewhere after it.
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
        draw_tree_table(&mut buf, area, &table, &Theme::default(), None, false);
        let row1 = line(&buf, 2, 20);
        assert!(!row1.contains('▾') && !row1.contains('▸'));
        assert!(row1.contains("main.rs"));
        // Indented further right than row 0's un-indented label.
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
        draw_tree_table(&mut buf, area, &table, &Theme::default(), None, false);
        // Row 0 (indent 0, "src") and row 1 (indent 1, "main.rs") paint
        // different-length tree-cell prefixes, but column 1's own text
        // ("2kb" on row 1; row 0's cell is empty) still starts at the
        // same screen column on both rows — the indent only pushes
        // column 0's label, never column 1's.
        let layout = tree_table_layout(area, &table);
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
        let layout = tree_table_layout(area, &table);
        let (idx, bounds) = layout.chevrons[0];
        assert_eq!(idx, 0);
        let hit = layout.hit_test(bounds.x, bounds.y, 0, table.table.rows.len());
        assert_eq!(
            hit,
            crate::primitives::tree_table::TreeTableHit::Chevron { idx: 0 }
        );
    }

    #[test]
    fn clicking_the_header_still_resolves_to_header_not_chevron() {
        let table = make_tree_table();
        let area = Rect::new(0, 0, 30, 10);
        let layout = tree_table_layout(area, &table);
        let hit = layout.hit_test(0.0, 0.0, 0, table.table.rows.len());
        assert_eq!(
            hit,
            crate::primitives::tree_table::TreeTableHit::Header { col: 0 }
        );
    }

    #[test]
    fn a_leaf_row_has_no_chevron_entry_and_clicking_it_is_a_row_hit() {
        let table = make_tree_table();
        let area = Rect::new(0, 0, 30, 10);
        let layout = tree_table_layout(area, &table);
        assert!(!layout.chevrons.iter().any(|(idx, _)| *idx == 1));
        let hit = layout.hit_test(0.0, 2.0, 0, table.table.rows.len());
        assert_eq!(
            hit,
            crate::primitives::tree_table::TreeTableHit::Row { idx: 1 }
        );
    }

    #[test]
    fn a_collapsed_branch_paints_the_collapsed_chevron() {
        let table = make_tree_table();
        let area = Rect::new(0, 0, 30, 10);
        let mut buf = Buffer::empty(area);
        draw_tree_table(&mut buf, area, &table, &Theme::default(), None, false);
        let row2 = line(&buf, 3, 20);
        assert!(
            row2.contains('▸'),
            "collapsed branch row should paint the collapsed chevron: {row2:?}"
        );
    }

    #[test]
    fn narrow_rect_drops_the_size_column_before_starving_the_tree_column() {
        let table = make_tree_table();
        // `make_tree_table`'s deepest row is indent 1 — floor is
        // `1 * TreeStyle::default().indent (2) + chevron (1) + label (1)`
        // = 4.0. At width 8 the plain `DataTable` squeeze alone (no
        // dropping) would leave column 0 at 2.0, below that floor, so
        // `Size` is dropped entirely and column 0 takes the rest.
        let area = Rect::new(0, 0, 8, 10);
        let mut buf = Buffer::empty(area);
        let layout = draw_tree_table(&mut buf, area, &table, &Theme::default(), None, false);
        assert_eq!(layout.table.columns.len(), 1);
        assert!(layout.table.columns[0].width >= 4.0);
    }

    #[test]
    fn narrow_rect_never_panics_even_when_the_viewport_is_narrower_than_the_floor() {
        let table = make_tree_table();
        // Narrower than the floor itself (4.0) — every other column is
        // already gone; column 0 is widened past the viewport rather
        // than truncated below the floor.
        let area = Rect::new(0, 0, 2, 10);
        let mut buf = Buffer::empty(area);
        let layout = draw_tree_table(&mut buf, area, &table, &Theme::default(), None, false);
        assert_eq!(layout.table.columns.len(), 1);
        assert_eq!(layout.table.columns[0].width, 4.0);
    }
}
