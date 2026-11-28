//! The cell-grid painter for [`Editor`], written against
//! [`CellSurface`].
//!
//! Paints every editor category: backgrounds (active / cursorline /
//! DAP-stopped / diff), gutter (breakpoint / git / diagnostics /
//! lightbulb / line numbers), syntax-highlighted text, indent guides,
//! colour columns, ghost continuation, diagnostic and spell underlines,
//! bracket match, selections (primary / multi-cursor / yank flash), the
//! primary caret, secondary carets and AI ghost text, then both
//! scrollbars.
//!
//! Geometry comes from the [`EditorLayout`] the caller resolved, so paint
//! and hit-testing share one source: the gutter, text and scrollbar
//! columns are that layout's rects rounded to whole cells.
//!
//! # Caret
//!
//! A [`CursorShape::Block`] caret is painted by inverting its cell. A
//! `Bar` or `Underline` caret cannot be drawn inside a cell, so it is
//! reported through [`CellSurface::set_cursor`] for the terminal to show;
//! nothing is reported when there is no caret.
//!
//! The per-window status line is not painted here: a host reserves a row
//! below the editor for it.

use super::{
    CursorShape, DiagnosticSeverity, DiffLine, Editor, EditorLayout, EditorLine, EditorSelection,
    GitLineStatus,
};
use crate::cell_surface::{self, CellStyle, CellSurface};
use crate::primitives::scrollbar::{cell_paint as scrollbar_cells, Scrollbar, ScrollbarLayout};
use crate::text_util::{char_cell_width, char_col_to_visual, snap_to_char_boundary};
use crate::theme::Theme;
use crate::types::Color;

/// A plain glyph style: both colours, no modifier.
fn solid(fg: Color, bg: Color) -> CellStyle {
    CellStyle {
        fg: Some(fg),
        bg: Some(bg),
        ..CellStyle::default()
    }
}

/// Rewrite the cell at `(x, y)` in place — its glyph and style as `edit`
/// leaves them. Out-of-bounds cells are left alone.
fn restyle(
    cells: &mut dyn CellSurface,
    x: u16,
    y: u16,
    edit: impl FnOnce(&mut char, &mut CellStyle),
) {
    if let Some((mut ch, mut style)) = cells.get(x, y) {
        edit(&mut ch, &mut style);
        cells.put_char(x, y, ch, style);
    }
}

/// Round a layout coordinate to a whole cell.
fn cell(v: f32) -> u16 {
    v.max(0.0).round() as u16
}

/// Paint `editor` into the cells `layout` resolved. `right_reserve_w` is
/// the strip (in cells) the layout left between the text and the
/// vertical scrollbar; it is left untouched for the host to paint into.
pub(crate) fn paint(
    editor: &Editor,
    layout: &EditorLayout,
    right_reserve_w: u16,
    cells: &mut dyn CellSurface,
    theme: &Theme,
) {
    let area_x = cell(layout.bounds.x);
    let area_y = cell(layout.bounds.y);
    let area_w = cell(layout.bounds.width);
    let area_h = cell(layout.bounds.height);

    let window_bg = if editor.show_active_bg {
        theme.editor_active_background
    } else {
        theme.background
    };
    let default_fg = theme.foreground;
    let gutter_w = editor.gutter_char_width as u16;
    let viewport_lines = area_h as usize;
    let has_scrollbar = layout.v_scrollbar_bounds.is_some();
    let viewport_cols = layout.visible_cols;
    let has_h_scrollbar = layout.h_scrollbar_bounds.is_some();
    let v_scrollbar_w: u16 = if has_scrollbar { 1 } else { 0 };
    // Shared by the per-row text, every text-column overlay and the
    // horizontal scrollbar track: everything up to (not including) the
    // reserved strip and the vertical scrollbar column.
    let text_width = cell(layout.text_bounds.width);
    // The background wash also covers the gutter.
    let bg_width = area_w
        .saturating_sub(right_reserve_w)
        .saturating_sub(v_scrollbar_w);

    // ── Background fill ─────────────────────────────────────────
    let bg_style = solid(default_fg, window_bg);
    for row in 0..area_h {
        for col in 0..bg_width {
            cells.put_char(area_x + col, area_y + row, ' ', bg_style);
        }
    }

    for (row_idx, line) in editor.lines.iter().enumerate() {
        let screen_y = area_y + row_idx as u16;
        if screen_y >= area_y + area_h {
            break;
        }

        // Cursorline / Diff / DAP stopped-line background.
        let line_bg = if line.is_dap_current {
            theme.dap_stopped_bg
        } else {
            match line.diff_status {
                Some(DiffLine::Added) => theme.diff_added_bg,
                Some(DiffLine::Removed) => theme.diff_removed_bg,
                Some(DiffLine::Padding) => theme.diff_padding_bg,
                _ if line.is_current_line && editor.is_active && editor.cursorline => {
                    theme.cursorline_bg
                }
                _ => window_bg,
            }
        };
        if line_bg != window_bg {
            // The row highlight stops short of the vertical scrollbar
            // column and the reserved strip.
            let style = solid(default_fg, line_bg);
            for col in 0..bg_width {
                cells.put_char(area_x + col, screen_y, ' ', style);
            }
        }

        // ── Gutter ──────────────────────────────────────────────
        if gutter_w > 0 {
            paint_gutter_row(editor, line, cells, theme, area_x, screen_y, line_bg);
        }

        // ── Text (narrowed by the scrollbar column and any reserve) ─
        let text_area_x = area_x + gutter_w;
        render_text_line(
            cells,
            text_area_x,
            screen_y,
            text_width,
            line,
            editor.scroll_left,
            theme,
            line_bg,
            editor.tabstop,
        );

        // Screen column of visual column `vcol`, or `None` when it is
        // scrolled off the left or past the text width.
        let screen_col = |vcol: usize| -> Option<u16> {
            if vcol < editor.scroll_left {
                return None;
            }
            let vis_col = (vcol - editor.scroll_left) as u16;
            (vis_col < text_width).then_some(text_area_x + vis_col)
        };

        // ── Indent guides ───────────────────────────────────────
        for &guide_col in &line.indent_guides {
            if guide_col < editor.scroll_left {
                continue;
            }
            let Some(cx) = screen_col(guide_col) else {
                break;
            };
            let is_active = editor.active_indent_col == Some(guide_col);
            let fg = if is_active {
                theme.indent_guide_active_fg
            } else {
                theme.indent_guide_fg
            };
            restyle(cells, cx, screen_y, |ch, style| {
                if *ch == ' ' {
                    *ch = '│';
                    style.fg = Some(fg);
                }
            });
        }

        // ── Color columns ───────────────────────────────────────
        for &cc_col in &line.colorcolumns {
            if cc_col < editor.scroll_left {
                continue;
            }
            let Some(cx) = screen_col(cc_col) else {
                break;
            };
            restyle(cells, cx, screen_y, |_, style| {
                style.bg = Some(theme.colorcolumn_bg);
            });
        }

        // ── Ghost continuation lines ────────────────────────────
        if line.is_ghost_continuation {
            if let Some(ghost) = &line.ghost_suffix {
                let style = solid(theme.ghost_text_fg, line_bg);
                for (i, ch) in ghost.chars().enumerate() {
                    let gx = text_area_x + i as u16;
                    if gx >= text_area_x + text_width {
                        break;
                    }
                    cells.put_char(gx, screen_y, ch, style);
                }
            }
        }

        // ── Diagnostic underlines ───────────────────────────────
        for dm in &line.diagnostics {
            let diag_fg = severity_color(dm.severity, theme);
            underline_range(
                cells,
                editor,
                line,
                (dm.start_col, dm.end_col),
                diag_fg,
                screen_y,
                &screen_col,
            );
        }

        // ── Spell error underlines ──────────────────────────────
        for sm in &line.spell_errors {
            underline_range(
                cells,
                editor,
                line,
                (sm.start_col, sm.end_col),
                theme.spell_error,
                screen_y,
                &screen_col,
            );
        }

        // ── Bracket match highlighting ──────────────────────────
        for &(view_line, col) in &editor.bracket_match_positions {
            if view_line != row_idx {
                continue;
            }
            let vis = char_col_to_visual(&line.raw_text, col, editor.tabstop);
            if let Some(cx) = screen_col(vis) {
                restyle(cells, cx, screen_y, |_, style| {
                    style.bg = Some(theme.bracket_match_bg);
                });
            }
        }
    }

    // ── Selection overlays ──────────────────────────────────────
    let text_area_x = area_x + gutter_w;
    if let Some(sel) = &editor.selection {
        render_selection(
            cells,
            area_y,
            text_area_x,
            editor,
            sel,
            theme,
            theme.selection,
            text_width,
        );
    }
    for esel in &editor.extra_selections {
        render_selection(
            cells,
            area_y,
            text_area_x,
            editor,
            esel,
            theme,
            theme.selection,
            text_width,
        );
    }
    if let Some(yh) = &editor.yank_highlight {
        render_selection(
            cells,
            area_y,
            text_area_x,
            editor,
            yh,
            theme,
            theme.yank_highlight_bg,
            text_width,
        );
    }

    // ── Vertical scrollbar ──────────────────────────────────────
    if let Some(track) = layout.v_scrollbar_bounds {
        if cell(track.height) > 0 {
            let track = crate::event::Rect::new(
                (area_x + area_w - 1) as f32,
                area_y as f32,
                1.0,
                cell(track.height) as f32,
            );
            let scrollbar = Scrollbar::vertical(
                "tui:editor:v_scrollbar",
                track,
                editor.scroll_top as f32,
                editor.total_lines as f32,
                viewport_lines as f32,
                1.0,
            );
            let sb_layout = ScrollbarLayout::resolve(&scrollbar, scrollbar.track);
            scrollbar_cells::paint(&sb_layout, cells, theme, theme.background);
        }
    }

    // ── Horizontal scrollbar ────────────────────────────────────
    if has_h_scrollbar && editor.max_col > 0 && viewport_cols > 0 {
        let cell_bg = window_bg;
        let sb_y = area_y + area_h - 1;
        // The track spans exactly the text columns, stopping short of
        // the vertical scrollbar column and the reserved strip.
        if text_width > 0 {
            let track =
                crate::event::Rect::new(text_area_x as f32, sb_y as f32, text_width as f32, 1.0);
            let scrollbar = Scrollbar::horizontal(
                "tui:editor:h_scrollbar",
                track,
                editor.scroll_left as f32,
                editor.max_col as f32,
                viewport_cols as f32,
                1.0,
            );
            let sb_layout = ScrollbarLayout::resolve(&scrollbar, scrollbar.track);
            scrollbar_cells::paint(&sb_layout, cells, theme, cell_bg);

            // Corner cell where the two scrollbars meet.
            if has_scrollbar {
                cells.put_char(
                    area_x + area_w - 1,
                    sb_y,
                    '┘',
                    solid(theme.separator, cell_bg),
                );
            }
        }
    }

    // Exclusive right bound of the columns the carets and ghost text may
    // paint into: the gutter plus the text width.
    let text_right = text_area_x + text_width;

    // ── Cursor ──────────────────────────────────────────────────
    if let Some(cursor) = &editor.cursor {
        let cursor_screen_y = area_y + cursor.pos.view_line as u16;
        let raw = editor
            .lines
            .get(cursor.pos.view_line)
            .map(|l| l.raw_text.as_str())
            .unwrap_or("");
        let vis_col = char_col_to_visual(raw, cursor.pos.col, editor.tabstop)
            .saturating_sub(editor.scroll_left) as u16;
        let cursor_screen_x = text_area_x + vis_col;

        match cursor.shape {
            CursorShape::Block => {
                if cursor_screen_x < text_right {
                    restyle(cells, cursor_screen_x, cursor_screen_y, |_, style| {
                        style.bg = Some(theme.cursor);
                        style.fg = Some(theme.background);
                    });
                }
            }
            CursorShape::Bar => {
                cells.set_cursor(
                    cursor_screen_x,
                    cursor_screen_y,
                    cell_surface::CursorShape::Bar,
                );
            }
            CursorShape::Underline => {
                cells.set_cursor(
                    cursor_screen_x,
                    cursor_screen_y,
                    cell_surface::CursorShape::Underline,
                );
            }
        }
    }

    // ── AI ghost text (after the caret on its line) ─────────────
    if let Some(cursor) = &editor.cursor {
        if let Some(rl) = editor.lines.get(cursor.pos.view_line) {
            if let Some(ghost) = &rl.ghost_suffix {
                let ghost_screen_y = area_y + cursor.pos.view_line as u16;
                let vis_col = char_col_to_visual(&rl.raw_text, cursor.pos.col, editor.tabstop)
                    .saturating_sub(editor.scroll_left) as u16;
                let ghost_start_x = text_area_x + vis_col;
                let style = solid(theme.ghost_text_fg, window_bg);
                for (i, ch) in ghost.chars().enumerate() {
                    let gx = ghost_start_x + i as u16;
                    if gx >= text_right {
                        break;
                    }
                    cells.put_char(gx, ghost_screen_y, ch, style);
                }
            }
        }
    }

    // ── Secondary cursors (multi-cursor) ────────────────────────
    let has_extra_sels = !editor.extra_selections.is_empty();
    for extra_pos in &editor.extra_cursors {
        let sy = area_y + extra_pos.view_line as u16;
        let col = if has_extra_sels {
            extra_pos.col + 1
        } else {
            extra_pos.col
        };
        let raw = editor
            .lines
            .get(extra_pos.view_line)
            .map(|l| l.raw_text.as_str())
            .unwrap_or("");
        let vis_col =
            char_col_to_visual(raw, col, editor.tabstop).saturating_sub(editor.scroll_left) as u16;
        let sx = text_area_x + vis_col;
        if sx < text_right {
            restyle(cells, sx, sy, |_, style| {
                style.bg = Some(theme.cursor);
                style.fg = Some(theme.background);
            });
        }
    }
}

/// The theme colour for a diagnostic of `severity`.
fn severity_color(severity: DiagnosticSeverity, theme: &Theme) -> Color {
    match severity {
        DiagnosticSeverity::Error => theme.diagnostic_error,
        DiagnosticSeverity::Warning => theme.diagnostic_warning,
        DiagnosticSeverity::Information => theme.diagnostic_info,
        DiagnosticSeverity::Hint => theme.diagnostic_hint,
    }
}

/// Underline the character columns `cols` of `line` in `color`, recolouring
/// their glyphs to match.
fn underline_range(
    cells: &mut dyn CellSurface,
    editor: &Editor,
    line: &EditorLine,
    cols: (usize, usize),
    color: Color,
    screen_y: u16,
    screen_col: &dyn Fn(usize) -> Option<u16>,
) {
    let vis_start = char_col_to_visual(&line.raw_text, cols.0, editor.tabstop);
    let vis_end = char_col_to_visual(&line.raw_text, cols.1, editor.tabstop);
    for vcol in vis_start..vis_end {
        if vcol < editor.scroll_left {
            continue;
        }
        let Some(cx) = screen_col(vcol) else {
            break;
        };
        restyle(cells, cx, screen_y, |_, style| {
            style.fg = Some(color);
            style.underline = Some(color);
        });
    }
}

/// One row of the gutter: the gutter text (breakpoint, git and line-number
/// columns each in their own colour), then the diagnostic or lightbulb
/// glyph in the breakpoint-adjacent column.
fn paint_gutter_row(
    editor: &Editor,
    line: &EditorLine,
    cells: &mut dyn CellSurface,
    theme: &Theme,
    area_x: u16,
    screen_y: u16,
    line_bg: Color,
) {
    let gutter_w = editor.gutter_char_width as u16;
    let line_num_fg = if line.is_current_line {
        theme.line_number_active_fg
    } else {
        theme.line_number_fg
    };
    // The breakpoint column is column 0 when present; the git column
    // follows it.
    let bp_offset = if editor.has_breakpoints { 1 } else { 0 };
    for (i, ch) in line.gutter_text.chars().enumerate() {
        let gx = area_x + i as u16;
        if gx >= area_x + gutter_w {
            break;
        }
        let fg = if editor.has_breakpoints && i == 0 {
            if line.is_dap_current || line.is_breakpoint {
                theme.diagnostic_error
            } else {
                line_num_fg
            }
        } else if editor.has_git_diff && i == bp_offset {
            match line.git_diff {
                Some(GitLineStatus::Added) => theme.git_added,
                Some(GitLineStatus::Modified) => theme.git_modified,
                Some(GitLineStatus::Deleted) => theme.git_deleted,
                None => theme.line_number_fg,
            }
        } else {
            line_num_fg
        };
        cells.put_char(gx, screen_y, ch, solid(fg, line_bg));
    }
    // The diagnostic glyph sits beside the breakpoint marker so both stay
    // visible.
    let diag_x = area_x + bp_offset as u16;
    if let Some(severity) = editor.diagnostic_gutter.get(&line.line_idx) {
        cells.put_char(
            diag_x,
            screen_y,
            '●',
            solid(severity_color(*severity, theme), line_bg),
        );
    } else if !line.is_wrap_continuation
        && editor.code_action_lines.contains(&line.line_idx)
        && editor.lightbulb_glyph != '\0'
    {
        cells.put_char(
            diag_x,
            screen_y,
            editor.lightbulb_glyph,
            solid(theme.lightbulb, line_bg),
        );
    }
}

/// The character index of UTF-8 byte offset `byte_offset` in `text`,
/// snapped to the char boundary at or before it; the total char count
/// past the end.
fn byte_to_char_idx(text: &str, byte_offset: usize) -> usize {
    text[..snap_to_char_boundary(text, byte_offset)]
        .chars()
        .count()
}

/// Paint one [`EditorLine`]'s text, scrolled by `scroll_left`, expanding
/// tabs to `tabstop` and applying each span's foreground, background,
/// bold and italic. Inline annotations (virtual text) paint in
/// `theme.annotation_fg` after the line content.
#[allow(clippy::too_many_arguments)]
fn render_text_line(
    cells: &mut dyn CellSurface,
    x_start: u16,
    y: u16,
    max_width: u16,
    line: &EditorLine,
    scroll_left: usize,
    theme: &Theme,
    window_bg: Color,
    tabstop: usize,
) {
    let raw = &line.raw_text;
    let chars: Vec<char> = raw.chars().filter(|&c| c != '\n' && c != '\r').collect();

    let mut char_fgs: Vec<Color> = vec![theme.foreground; chars.len()];
    let mut char_bgs: Vec<Option<Color>> = vec![None; chars.len()];
    let mut char_bold: Vec<bool> = vec![false; chars.len()];
    let mut char_italic: Vec<bool> = vec![false; chars.len()];

    for span in &line.spans {
        let start = byte_to_char_idx(raw, span.start_byte);
        let end = byte_to_char_idx(raw, span.end_byte).min(chars.len());
        for i in start..end {
            char_fgs[i] = span.style.fg;
            char_bgs[i] = span.style.bg;
            char_bold[i] = span.style.bold;
            char_italic[i] = span.style.italic;
        }
    }

    let tabstop = tabstop.max(1);
    let mut vis_col: usize = 0;
    let mut glyphs: Vec<(usize, char, usize)> = Vec::with_capacity(chars.len());
    for (i, &ch) in chars.iter().enumerate() {
        if ch == '\t' {
            let next_stop = ((vis_col / tabstop) + 1) * tabstop;
            while vis_col < next_stop {
                glyphs.push((vis_col, ' ', i));
                vis_col += 1;
            }
        } else {
            glyphs.push((vis_col, ch, i));
            vis_col += char_cell_width(ch) as usize;
        }
    }
    let total_vis_cols = vis_col;

    for &(vcol, ch, ci) in &glyphs {
        if vcol < scroll_left {
            continue;
        }
        let col = (vcol - scroll_left) as u16;
        let width = char_cell_width(ch);
        if col + width.max(1) > max_width {
            break;
        }
        let style = CellStyle {
            bold: char_bold[ci],
            italic: char_italic[ci],
            ..solid(char_fgs[ci], char_bgs[ci].unwrap_or(window_bg))
        };
        match width {
            0 => {}
            2 => cells.put_wide(x_start + col, y, ch, style),
            _ => cells.put_char(x_start + col, y, ch, style),
        }
    }

    // Inline annotation / virtual text (e.g. git blame).
    if let Some(ann) = &line.annotation {
        let visible_cols = total_vis_cols.saturating_sub(scroll_left);
        let ann_start = x_start + visible_cols.min(max_width as usize) as u16;
        let style = solid(theme.annotation_fg, window_bg);
        for (i, ch) in ann.chars().enumerate() {
            let col = ann_start + i as u16;
            if col >= x_start + max_width {
                break;
            }
            cells.put_char(col, y, ch, style);
        }
    }
}

/// Paint one selection overlay. [`EditorSelection::cols_on`] maps each
/// visible row onto its own segment-local columns (so a wrapped line
/// highlights only the overlapping segment, and ghost-continuation and
/// diff-padding rows are skipped). Each touched cell takes `color` as its
/// background; a glyph whose foreground matched the window background
/// switches to the theme foreground so it stays visible.
#[allow(clippy::too_many_arguments)]
fn render_selection(
    cells: &mut dyn CellSurface,
    area_y: u16,
    text_area_x: u16,
    editor: &Editor,
    sel: &EditorSelection,
    theme: &Theme,
    color: Color,
    text_width: u16,
) {
    let window_bg = if editor.show_active_bg {
        theme.editor_active_background
    } else {
        theme.background
    };

    for (row_idx, line) in editor.lines.iter().enumerate() {
        let Some(cols) = sel.cols_on(line) else {
            continue;
        };
        let screen_y = area_y + row_idx as u16;

        let vis_start = char_col_to_visual(&line.raw_text, cols.start, editor.tabstop);
        let vis_end = char_col_to_visual(&line.raw_text, cols.end, editor.tabstop);

        for vis in vis_start..vis_end {
            if vis < editor.scroll_left {
                continue;
            }
            let screen_col = (vis - editor.scroll_left) as u16;
            if screen_col >= text_width {
                break;
            }
            restyle(cells, text_area_x + screen_col, screen_y, |_, style| {
                let old_fg = style.fg.map(|c| (c.r, c.g, c.b));
                style.bg = Some(color);
                if old_fg == Some((window_bg.r, window_bg.g, window_bg.b)) {
                    style.fg = Some(theme.foreground);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{DiagnosticMark, SelectionKind, SpellMark};
    use super::*;
    use crate::tui::{ratatui_color as qc, BufferCellSurface};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect as RtRect;
    use ratatui::style::Color as RColor;

    /// The caret a test paint reported: `Some` for a `Bar`/`Underline`
    /// caret the painter placed through [`crate::CellSurface::set_cursor`].
    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    struct CaretReport {
        cursor_position: Option<(u16, u16)>,
    }

    fn paint_editor(buf: &mut Buffer, area: RtRect, editor: &Editor, theme: &Theme) -> CaretReport {
        paint_editor_with_reserve(buf, area, editor, theme, 0)
    }

    /// Paints `editor` into `buf` with a `right_reserve_w`-cell strip left
    /// unpainted between the text and the vertical scrollbar.
    fn paint_editor_with_reserve(
        buf: &mut Buffer,
        area: RtRect,
        editor: &Editor,
        theme: &Theme,
        right_reserve_w: u16,
    ) -> CaretReport {
        let layout = editor.layout_with_options_and_reserve(
            crate::Rect::new(
                area.x as f32,
                area.y as f32,
                area.width as f32,
                area.height as f32,
            ),
            1.0,
            1.0,
            super::super::EditorPaintOptions::default(),
            None,
            right_reserve_w as f32,
        );
        let mut surface = BufferCellSurface::new(buf);
        paint(editor, &layout, right_reserve_w, &mut surface, theme);
        CaretReport {
            cursor_position: surface.cursor().map(|(x, y, _)| (x, y)),
        }
    }

    /// Minimal one-line [`EditorLine`] carrying decoration-overlay
    /// categories at the given columns. All columns land past `raw_text`'s
    /// own length — none of the paint sites require character content under
    /// them to safely index `buf[(cx, screen_y)]`.
    fn decorated_line(line_idx: usize, col: usize) -> EditorLine {
        EditorLine {
            raw_text: "x".repeat(col + 2),
            gutter_text: String::new(),
            spans: vec![],
            line_idx,
            is_current_line: false,
            is_fold_header: false,
            folded_line_count: 0,
            git_diff: None,
            diff_status: None,
            diagnostics: vec![DiagnosticMark {
                start_col: col,
                end_col: col + 1,
                severity: DiagnosticSeverity::Error,
                message: String::new(),
            }],
            spell_errors: vec![SpellMark {
                start_col: col,
                end_col: col + 1,
            }],
            is_breakpoint: false,
            is_conditional_bp: false,
            is_dap_current: false,
            is_wrap_continuation: false,
            segment_col_offset: 0,
            annotation: None,
            ghost_suffix: None,
            is_ghost_continuation: false,
            indent_guides: vec![col],
            colorcolumns: vec![col],
        }
    }

    fn test_editor(lines: Vec<EditorLine>, bracket_match_positions: Vec<(usize, usize)>) -> Editor {
        Editor {
            id: crate::types::WidgetId::new("editor"),
            rect: crate::event::Rect::new(0.0, 0.0, 20.0, 5.0),
            lines,
            cursor: None,
            extra_cursors: vec![],
            selection: None,
            extra_selections: vec![],
            yank_highlight: None,
            scroll_top: 0,
            scroll_left: 0,
            total_lines: 2,
            max_col: 20,
            gutter_char_width: 0,
            is_active: true,
            show_active_bg: false,
            has_git_diff: false,
            has_breakpoints: false,
            diagnostic_gutter: Default::default(),
            code_action_lines: Default::default(),
            bracket_match_positions,
            active_indent_col: None,
            tabstop: 4,
            cursorline: false,
            lightbulb_glyph: '\0',
            paint_options: Default::default(),
        }
    }

    /// A span with a font scale paints one cell per character, so the
    /// text after it stays in its own columns: the cell grid cannot
    /// scale glyphs.
    #[test]
    fn scaled_span_paints_one_cell_per_char_without_column_drift() {
        use super::super::{EditorStyledSpan, Style};
        let mut line = decorated_line(0, 0);
        line.raw_text = "# Head body".into();
        line.diagnostics.clear();
        line.spell_errors.clear();
        line.indent_guides.clear();
        line.colorcolumns.clear();
        line.spans = vec![EditorStyledSpan {
            start_byte: 0,
            end_byte: 6,
            style: Style {
                fg: Color::rgb(220, 60, 60),
                bg: None,
                bold: true,
                italic: false,
                font_scale: 1.6,
            },
        }];
        let editor = test_editor(vec![line], vec![]);
        let theme = Theme::default();
        let mut buf = Buffer::empty(RtRect::new(0, 0, 20, 5));
        let _ = paint_editor(&mut buf, RtRect::new(0, 0, 20, 5), &editor, &theme);
        let row: String = (0..11).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert_eq!(row, "# Head body");
        assert_eq!(buf[(5, 0)].fg, qc(Color::rgb(220, 60, 60)));
        assert_eq!(buf[(7, 0)].fg, qc(theme.foreground));
    }

    /// `paint_editor` must bounds-check every overlay write against
    /// `buf.area` (the buffer's real, already-resized extent), not the
    /// caller-supplied `area`. A resize between layout computation and
    /// `ratatui::Terminal::draw` can leave `area` describing a larger
    /// region than `buf` actually has. This test reproduces that gap:
    /// `area` is wider and taller than `buf`, and the overlay categories
    /// (indent guides, color columns, diagnostics, spell errors, bracket
    /// match) target cells inside `area` but outside `buf.area` both
    /// horizontally and vertically. The function must silently skip the
    /// off-buffer writes and return normally.
    #[test]
    fn draw_editor_does_not_panic_when_area_outlives_buf() {
        let lines = vec![
            decorated_line(0, 15), // triggers the *horizontal* guard
            decorated_line(1, 2),  // triggers the *vertical* guard
        ];
        let editor = test_editor(lines, vec![(0, 15), (1, 2)]);
        let theme = Theme::default();

        // `buf` is the real, already-shrunk buffer (10x1); `area` is the
        // stale, larger layout rect (20x5) from before a terminal resize.
        let mut buf = Buffer::empty(RtRect::new(0, 0, 10, 1));
        let area = RtRect::new(0, 0, 20, 5);

        // Must not panic.
        let _ = paint_editor(&mut buf, area, &editor, &theme);
    }

    /// `right_reserve_w` must carve out a strip at the trailing edge of
    /// `area` that the background fill and text paint leave untouched,
    /// while the vertical scrollbar still hugs the true right edge of
    /// `area` — unaffected by the reserve, per this function's doc.
    #[test]
    fn draw_editor_right_reserve_w_leaves_strip_untouched_and_keeps_scrollbar_at_edge() {
        let lines = vec![sel_test_line(0, "hi", 0, false, None)];
        let mut editor = test_editor(lines, vec![]);
        // > viewport_lines (area.height below) so a v-scrollbar is drawn.
        editor.total_lines = 100;
        let theme = Theme::default();

        let area = RtRect::new(0, 0, 20, 5);
        let mut buf = Buffer::empty(area);
        let right_reserve_w: u16 = 3;

        // Sentinel: mark the whole buffer with a glyph/style the paint
        // never produces, so any cell still carrying it afterward proves
        // this function skipped writing there.
        for y in 0..area.height {
            for x in 0..area.width {
                buf[(x, y)]
                    .set_char('?')
                    .set_fg(RColor::Reset)
                    .set_bg(RColor::Reset);
            }
        }

        let _ = paint_editor_with_reserve(&mut buf, area, &editor, &theme, right_reserve_w);

        let scrollbar_x = area.x + area.width - 1;
        for y in 0..area.height {
            let scrollbar_cell = &buf[(scrollbar_x, y)];
            assert_ne!(
                scrollbar_cell.symbol(),
                "?",
                "vertical scrollbar must hug the true right edge regardless of right_reserve_w"
            );

            // Every column inside the reserved strip (between the text
            // area and the scrollbar column) must still carry the
            // sentinel — untouched by background fill or text paint.
            for x in (scrollbar_x - right_reserve_w)..scrollbar_x {
                let cell = &buf[(x, y)];
                assert_eq!(
                    cell.symbol(),
                    "?",
                    "column {x} is inside the right_reserve_w strip and must be left untouched"
                );
            }
        }

        // A column inside the text area (left of the reserved strip)
        // must have been painted over by the background fill.
        let painted_cell = &buf[(0, 0)];
        assert_ne!(
            painted_cell.symbol(),
            "?",
            "columns left of the reserve strip must still be painted"
        );
    }

    // ── Selection column math via `EditorSelection::cols_on` ────────────

    fn sel_test_line(
        line_idx: usize,
        raw_text: &str,
        segment_col_offset: usize,
        is_ghost_continuation: bool,
        diff_status: Option<DiffLine>,
    ) -> EditorLine {
        EditorLine {
            raw_text: raw_text.into(),
            gutter_text: String::new(),
            spans: vec![],
            line_idx,
            is_current_line: false,
            is_fold_header: false,
            folded_line_count: 0,
            git_diff: None,
            diff_status,
            diagnostics: vec![],
            spell_errors: vec![],
            is_breakpoint: false,
            is_conditional_bp: false,
            is_dap_current: false,
            is_wrap_continuation: segment_col_offset > 0,
            segment_col_offset,
            annotation: None,
            ghost_suffix: is_ghost_continuation.then(|| "ai suggestion".to_string()),
            is_ghost_continuation,
            indent_guides: vec![],
            colorcolumns: vec![],
        }
    }

    /// One `Char` selection spanning buffer lines `[0, 2]` painted
    /// across a plain row, a wrapped-segment row, a ghost-continuation
    /// row, and a diff-padding row — same matrix
    /// `tests/conformance/editor.rs` proves against
    /// `EditorSelection::cols_on` directly, proven again here through a
    /// real `paint_editor` paint so this backend's own call site is
    /// covered too.
    #[test]
    fn draw_editor_selection_respects_wrapped_ghost_and_diff_padding_rows() {
        let lines = vec![
            // Row 0: buffer line 0, plain, selection's own start line.
            sel_test_line(0, "0123456789", 0, false, None),
            // Row 1: a wrap-continuation segment of buffer line 1,
            // starting at buffer column 5 — its own text is only 5
            // chars, entirely inside the selection.
            sel_test_line(1, "abcde", 5, false, None),
            // Row 2: the AI-ghost continuation of buffer line 1 — same
            // `line_idx` as row 1, but virtual, must never be selected.
            sel_test_line(1, "", 0, true, None),
            // Row 3: buffer line 2 (the selection's own end line), but a
            // diff-padding filler row — must never be selected either.
            sel_test_line(2, "", 0, false, Some(DiffLine::Padding)),
        ];
        let mut editor = test_editor(lines, vec![]);
        // `test_editor`'s fixed `max_col: 20` would otherwise overflow
        // this 10-wide buffer and reserve the bottom row (row 3, exactly
        // the diff-padding row this test samples) for a horizontal
        // scrollbar track — unrelated to selection paint, and would
        // clobber the very background this test asserts on.
        editor.max_col = 0;
        editor.total_lines = 4;
        editor.selection = Some(EditorSelection {
            kind: SelectionKind::Char,
            start_line: 0,
            start_col: 0,
            end_line: 2,
            end_col: 10,
        });
        let theme = Theme::default();

        let mut buf = Buffer::empty(RtRect::new(0, 0, 10, 4));
        let area = RtRect::new(0, 0, 10, 4);
        let _ = paint_editor(&mut buf, area, &editor, &theme);

        let sel_bg = qc(theme.selection);
        let window_bg = qc(theme.background);
        let diff_padding_bg = qc(theme.diff_padding_bg);

        assert_eq!(
            buf[(0, 0)].bg,
            sel_bg,
            "row 0 (plain, selection's start line) should be tinted"
        );
        assert_eq!(
            buf[(0, 1)].bg,
            sel_bg,
            "row 1 (wrapped segment of a fully-selected line) should be tinted"
        );
        assert_eq!(
            buf[(0, 2)].bg,
            window_bg,
            "row 2 (ghost continuation) must never be tinted by a selection"
        );
        assert_eq!(
            buf[(0, 3)].bg,
            diff_padding_bg,
            "row 3 (diff padding) must show only its diff background, not a selection tint"
        );
    }

    /// `paint_editor_with_reserve(..., 0)` must reproduce `paint_editor`
    /// cell-for-cell — the convenience wrapper is a thin forward, not an
    /// independent code path that could silently drift from it.
    #[test]
    fn draw_editor_with_reserve_zero_matches_draw_editor() {
        let lines = vec![sel_test_line(0, "0123456789", 0, false, None)];
        let mut editor = test_editor(lines, vec![]);
        editor.total_lines = 10;
        editor.max_col = 20;
        let theme = Theme::default();

        let area = RtRect::new(0, 0, 10, 4);
        let mut buf_a = Buffer::empty(area);
        let mut buf_b = Buffer::empty(area);
        let result_a = paint_editor(&mut buf_a, area, &editor, &theme);
        let result_b = paint_editor_with_reserve(&mut buf_b, area, &editor, &theme, 0);

        assert_eq!(result_a, result_b);
        for y in 0..area.height {
            for x in 0..area.width {
                let a = &buf_a[(x, y)];
                let b = &buf_b[(x, y)];
                assert_eq!(a.symbol(), b.symbol(), "symbol diverged at ({x}, {y})");
                assert_eq!(a.fg, b.fg, "fg diverged at ({x}, {y})");
                assert_eq!(a.bg, b.bg, "bg diverged at ({x}, {y})");
            }
        }
    }

    /// `right_reserve_w` narrows the per-row background highlight (the
    /// cursorline tint here, a different code path from the whole-area
    /// background fill covered by
    /// `draw_editor_right_reserve_w_leaves_strip_untouched_and_keeps_scrollbar_at_edge`),
    /// but the vertical scrollbar stays anchored to `area`'s true right
    /// edge — the whole point of a paint-side reserve: a host can dock a
    /// strip (a minimap, say) between the narrowed text and the
    /// scrollbar without the scrollbar sliding left to hug the text.
    #[test]
    fn draw_editor_with_reserve_keeps_scrollbar_at_area_right_edge() {
        let mut line = sel_test_line(0, "0123456789ABCDEF", 0, false, None);
        line.is_current_line = true;
        let mut editor = test_editor(vec![line], vec![]);
        editor.cursorline = true;
        editor.total_lines = 10; // forces has_scrollbar
        editor.max_col = 0; // no horizontal scrollbar — keep this test focused

        let theme = Theme::default();
        let area = RtRect::new(0, 0, 10, 4);
        let mut buf = Buffer::empty(area);

        // Sentinel the whole buffer with a glyph/style the paint never
        // produces, so a cell still carrying it afterwards proves the
        // paint skipped writing there.
        for y in 0..area.height {
            for x in 0..area.width {
                buf[(x, y)]
                    .set_char('?')
                    .set_fg(RColor::Reset)
                    .set_bg(RColor::Reset);
            }
        }

        let right_reserve_w = 2;
        let _ = paint_editor_with_reserve(&mut buf, area, &editor, &theme, right_reserve_w);

        let cursorline_bg = qc(theme.cursorline_bg);

        // bg_width = area.width(10) - has_scrollbar(1) - right_reserve_w(2) = 7:
        // the cursorline tint covers columns 0..7 and stops there.
        assert_eq!(
            buf[(6, 0)].bg,
            cursorline_bg,
            "column 6 is inside the narrowed cursorline highlight"
        );
        for x in 7..9 {
            let cell = &buf[(x, 0)];
            assert_eq!(
                cell.symbol(),
                "?",
                "column {x} is inside the reserved strip: no glyph may spill into it"
            );
            assert_eq!(
                cell.bg,
                RColor::Reset,
                "column {x} is inside the reserved strip: the cursorline tint must stop short of it"
            );
        }

        // The vertical scrollbar track still hugs area's true right
        // edge (column 9 = area.x + area.width - 1), undisturbed by the
        // reserve — it must not have slid left to column 7 or 8.
        assert_ne!(
            buf[(9, 0)].symbol(),
            "?",
            "scrollbar track at area's right edge (column 9) must be painted"
        );
    }
}
