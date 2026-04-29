//! TUI rasteriser for the [`Editor`] primitive (#276 Stage 1C).
//!
//! Verbatim port of vimcode's `src/tui_main/render_impl::render_window`
//! body. Handles every editor paint category: backgrounds (active /
//! cursorline / DAP-stopped / diff), gutter (BP / git / diagnostics /
//! lightbulb / line numbers), syntax-highlighted text, indent guides,
//! color columns, ghost continuation, diagnostic + spell underlines,
//! bracket-match, selections (primary / multi-cursor / yank-flash),
//! cursor (Block via fg/bg swap, Bar/Underline returned to host),
//! secondary cursors, and AI ghost text.
//!
//! ## Scrollbars
//!
//! The rasteriser internally calls [`super::draw_scrollbar`] for both
//! vertical and horizontal scrollbars when overflow is present. This
//! mirrors the pre-#276 layout where `render_window` painted both
//! scrollbars inline. Externalising the scrollbar paint to the host
//! is filed as a follow-up (see PLAN.md "Sharp edges").
//!
//! ## Status line
//!
//! The per-window status line (lifted Session 241) is **not** painted
//! by this rasteriser. The host is responsible for reserving a row at
//! the bottom of `area` if `Editor.status_line` would be drawn there.
//! Vimcode's delegator does this: shrinks `area` by 1 if a status
//! line is present, calls `draw_editor`, then paints the status line
//! at the reserved row.
//!
//! ## Cursor side-effect
//!
//! The TUI [`Frame`](ratatui::Frame) cursor (used by Bar / Underline
//! shapes) is set by calling `Frame::set_cursor_position` — a method
//! on `Frame`, not `Buffer`. To keep this rasteriser pure-paint, the
//! Bar / Underline cursor *position* is returned via
//! [`EditorPaintResult::cursor_position`]; the host calls
//! `set_cursor_position` after this function returns. Block cursors
//! are painted directly by inverting the buffer cell's fg/bg.

use crate::primitives::editor::{
    CursorShape, DiagnosticSeverity, DiffLine, Editor, EditorLine, EditorSelection, GitLineStatus,
    SelectionKind,
};
use crate::primitives::scrollbar::Scrollbar;
use crate::theme::Theme;
use crate::types::Color;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color as RColor, Modifier};

use super::{qc, set_cell, set_cell_styled};

/// Output of [`draw_editor`] — carries side-effects the host needs
/// to apply *after* the buffer paint completes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EditorPaintResult {
    /// Screen-space `(x, y)` for `Frame::set_cursor_position` when the
    /// cursor shape is `Bar` or `Underline`. `None` when the cursor is
    /// off-screen, missing, or in `Block` mode (Block is painted
    /// directly into the buffer via fg/bg inversion).
    pub cursor_position: Option<(u16, u16)>,
}

/// Paint an [`Editor`] into `buf`. See module docs for paint-category
/// inventory and exclusions (status line stays with the host).
pub fn draw_editor(
    buf: &mut Buffer,
    area: Rect,
    editor: &Editor,
    theme: &Theme,
) -> EditorPaintResult {
    let mut result = EditorPaintResult::default();

    let window_bg = qc(if editor.show_active_bg {
        theme.editor_active_background
    } else {
        theme.background
    });
    let default_fg = qc(theme.foreground);
    let gutter_w = editor.gutter_char_width as u16;
    let viewport_lines = area.height as usize;
    let has_scrollbar = editor.total_lines > viewport_lines && area.width > gutter_w + 1;
    let viewport_cols =
        (area.width as usize).saturating_sub(gutter_w as usize + if has_scrollbar { 1 } else { 0 });
    let has_h_scrollbar = editor.max_col > viewport_cols && area.height > 1;

    // ── Background fill ─────────────────────────────────────────
    for row in 0..area.height {
        for col in 0..area.width {
            set_cell(buf, area.x + col, area.y + row, ' ', default_fg, window_bg);
        }
    }

    for (row_idx, line) in editor.lines.iter().enumerate() {
        let screen_y = area.y + row_idx as u16;
        if screen_y >= area.y + area.height {
            break;
        }

        // Cursorline / Diff / DAP stopped-line background.
        let line_bg = if line.is_dap_current {
            qc(theme.dap_stopped_bg)
        } else {
            match line.diff_status {
                Some(DiffLine::Added) => qc(theme.diff_added_bg),
                Some(DiffLine::Removed) => qc(theme.diff_removed_bg),
                Some(DiffLine::Padding) => qc(theme.diff_padding_bg),
                _ if line.is_current_line && editor.is_active && editor.cursorline => {
                    qc(theme.cursorline_bg)
                }
                _ => window_bg,
            }
        };
        if line_bg != window_bg {
            for col in 0..area.width {
                set_cell(buf, area.x + col, screen_y, ' ', default_fg, line_bg);
            }
        }

        // ── Gutter ──────────────────────────────────────────────
        if gutter_w > 0 {
            let line_num_fg = qc(if line.is_current_line {
                theme.line_number_active_fg
            } else {
                theme.line_number_fg
            });
            // The bp column offset: 1 when has_breakpoints, else 0.
            // The git column offset: bp_offset + 1 when has_git_diff, else bp_offset.
            let bp_offset = if editor.has_breakpoints { 1 } else { 0 };
            let git_offset = if editor.has_git_diff {
                bp_offset + 1
            } else {
                bp_offset
            };
            for (i, ch) in line.gutter_text.chars().enumerate() {
                let gx = area.x + i as u16;
                if gx >= area.x + gutter_w {
                    break;
                }
                let fg = if editor.has_breakpoints && i == 0 {
                    if line.is_dap_current || line.is_breakpoint {
                        qc(theme.diagnostic_error)
                    } else {
                        line_num_fg
                    }
                } else if editor.has_git_diff && i == bp_offset {
                    qc(match line.git_diff {
                        Some(GitLineStatus::Added) => theme.git_added,
                        Some(GitLineStatus::Modified) => theme.git_modified,
                        Some(GitLineStatus::Deleted) => theme.git_deleted,
                        None => theme.line_number_fg,
                    })
                } else {
                    let _ = git_offset;
                    line_num_fg
                };
                set_cell(buf, gx, screen_y, ch, fg, line_bg);
            }
            // Diagnostic gutter icon (overwrite leftmost gutter char).
            if let Some(severity) = editor.diagnostic_gutter.get(&line.line_idx) {
                let (diag_ch, diag_color) = match severity {
                    DiagnosticSeverity::Error => ('●', qc(theme.diagnostic_error)),
                    DiagnosticSeverity::Warning => ('●', qc(theme.diagnostic_warning)),
                    DiagnosticSeverity::Information => ('●', qc(theme.diagnostic_info)),
                    DiagnosticSeverity::Hint => ('●', qc(theme.diagnostic_hint)),
                };
                set_cell(buf, area.x, screen_y, diag_ch, diag_color, line_bg);
            } else if !line.is_wrap_continuation
                && editor.code_action_lines.contains(&line.line_idx)
                && editor.lightbulb_glyph != '\0'
            {
                set_cell(
                    buf,
                    area.x,
                    screen_y,
                    editor.lightbulb_glyph,
                    qc(theme.lightbulb),
                    line_bg,
                );
            }
        }

        // ── Text (narrowed by 1 when scrollbar is shown) ────────
        let text_area_x = area.x + gutter_w;
        let text_width = area
            .width
            .saturating_sub(gutter_w)
            .saturating_sub(if has_scrollbar { 1 } else { 0 });
        render_text_line(
            buf,
            text_area_x,
            screen_y,
            text_width,
            line,
            editor.scroll_left,
            theme,
            line_bg,
            editor.tabstop,
        );

        // ── Indent guides ───────────────────────────────────────
        if !line.indent_guides.is_empty() {
            let guide_fg = qc(theme.indent_guide_fg);
            let active_fg = qc(theme.indent_guide_active_fg);
            for &guide_col in &line.indent_guides {
                if guide_col < editor.scroll_left {
                    continue;
                }
                let vis_col = (guide_col - editor.scroll_left) as u16;
                if vis_col >= text_width {
                    break;
                }
                let cx = text_area_x + vis_col;
                if cx < area.x + area.width && screen_y < area.y + area.height {
                    let cell = &mut buf[(cx, screen_y)];
                    if cell.symbol() == " " {
                        let is_active = editor.active_indent_col == Some(guide_col);
                        let fg = if is_active { active_fg } else { guide_fg };
                        cell.set_char('│');
                        cell.set_fg(fg);
                    }
                }
            }
        }

        // ── Color columns ───────────────────────────────────────
        if !line.colorcolumns.is_empty() {
            let cc_bg = qc(theme.colorcolumn_bg);
            for &cc_col in &line.colorcolumns {
                if cc_col < editor.scroll_left {
                    continue;
                }
                let vis_col = (cc_col - editor.scroll_left) as u16;
                if vis_col >= text_width {
                    break;
                }
                let cx = text_area_x + vis_col;
                if cx < area.x + area.width && screen_y < area.y + area.height {
                    let cell = &mut buf[(cx, screen_y)];
                    cell.set_bg(cc_bg);
                }
            }
        }

        // ── Ghost continuation lines ────────────────────────────
        if line.is_ghost_continuation {
            if let Some(ghost) = &line.ghost_suffix {
                let ghost_fg = qc(theme.ghost_text_fg);
                for (i, ch) in ghost.chars().enumerate() {
                    let gx = text_area_x + i as u16;
                    if gx >= text_area_x + text_width {
                        break;
                    }
                    set_cell(buf, gx, screen_y, ch, ghost_fg, line_bg);
                }
            }
        }

        // ── Diagnostic underlines ───────────────────────────────
        for dm in &line.diagnostics {
            let diag_fg = qc(match dm.severity {
                DiagnosticSeverity::Error => theme.diagnostic_error,
                DiagnosticSeverity::Warning => theme.diagnostic_warning,
                DiagnosticSeverity::Information => theme.diagnostic_info,
                DiagnosticSeverity::Hint => theme.diagnostic_hint,
            });
            let vis_start = char_col_to_visual(&line.raw_text, dm.start_col, editor.tabstop);
            let vis_end = char_col_to_visual(&line.raw_text, dm.end_col, editor.tabstop);
            for vcol in vis_start..vis_end {
                if vcol < editor.scroll_left {
                    continue;
                }
                let vis_col = (vcol - editor.scroll_left) as u16;
                if vis_col >= text_width {
                    break;
                }
                let cx = text_area_x + vis_col;
                if cx < area.x + area.width && screen_y < area.y + area.height {
                    let cell = &mut buf[(cx, screen_y)];
                    cell.set_fg(diag_fg);
                    cell.modifier |= Modifier::UNDERLINED;
                    cell.underline_color = diag_fg;
                }
            }
        }

        // ── Spell error underlines ──────────────────────────────
        let spell_fg = qc(theme.spell_error);
        for sm in &line.spell_errors {
            let vis_start = char_col_to_visual(&line.raw_text, sm.start_col, editor.tabstop);
            let vis_end = char_col_to_visual(&line.raw_text, sm.end_col, editor.tabstop);
            for vcol in vis_start..vis_end {
                if vcol < editor.scroll_left {
                    continue;
                }
                let vis_col = (vcol - editor.scroll_left) as u16;
                if vis_col >= text_width {
                    break;
                }
                let cx = text_area_x + vis_col;
                if cx < area.x + area.width && screen_y < area.y + area.height {
                    let cell = &mut buf[(cx, screen_y)];
                    cell.set_fg(spell_fg);
                    cell.modifier |= Modifier::UNDERLINED;
                    cell.underline_color = spell_fg;
                }
            }
        }

        // ── Bracket match highlighting ──────────────────────────
        let bracket_bg = qc(theme.bracket_match_bg);
        for &(view_line, col) in &editor.bracket_match_positions {
            if view_line == row_idx {
                let vis = char_col_to_visual(&line.raw_text, col, editor.tabstop);
                if vis < editor.scroll_left {
                    continue;
                }
                let vis_col = (vis - editor.scroll_left) as u16;
                if vis_col >= text_width {
                    continue;
                }
                let cx = text_area_x + vis_col;
                if cx < area.x + area.width && screen_y < area.y + area.height {
                    let cell = &mut buf[(cx, screen_y)];
                    cell.set_bg(bracket_bg);
                }
            }
        }
    }

    // ── Selection overlays ──────────────────────────────────────
    if let Some(sel) = &editor.selection {
        render_selection(
            buf,
            area,
            editor,
            sel,
            window_bg,
            theme.selection,
            default_fg,
        );
    }
    for esel in &editor.extra_selections {
        render_selection(
            buf,
            area,
            editor,
            esel,
            window_bg,
            theme.selection,
            default_fg,
        );
    }
    if let Some(yh) = &editor.yank_highlight {
        render_selection(
            buf,
            area,
            editor,
            yh,
            window_bg,
            theme.yank_highlight_bg,
            default_fg,
        );
    }

    // ── Vertical scrollbar ──────────────────────────────────────
    if has_scrollbar {
        let track_h = if has_h_scrollbar {
            area.height.saturating_sub(1)
        } else {
            area.height
        };
        if track_h > 0 {
            let track = crate::event::Rect::new(
                (area.x + area.width - 1) as f32,
                area.y as f32,
                1.0,
                track_h as f32,
            );
            let scrollbar = Scrollbar::vertical(
                "tui:editor:v_scrollbar",
                track,
                editor.scroll_top as f32,
                editor.total_lines as f32,
                viewport_lines as f32,
                1.0,
            );
            super::draw_scrollbar(buf, &scrollbar, theme, theme.background);
        }
    }

    // ── Horizontal scrollbar ────────────────────────────────────
    if has_h_scrollbar && editor.max_col > 0 && viewport_cols > 0 {
        let cell_bg = if editor.show_active_bg {
            theme.editor_active_background
        } else {
            theme.background
        };
        let sb_y = area.y + area.height - 1;
        let track_x = area.x + gutter_w;
        let track_w = area
            .width
            .saturating_sub(gutter_w + if has_scrollbar { 1 } else { 0 });
        if track_w > 0 {
            let track = crate::event::Rect::new(track_x as f32, sb_y as f32, track_w as f32, 1.0);
            let scrollbar = Scrollbar::horizontal(
                "tui:editor:h_scrollbar",
                track,
                editor.scroll_left as f32,
                editor.max_col as f32,
                viewport_cols as f32,
                1.0,
            );
            super::draw_scrollbar(buf, &scrollbar, theme, cell_bg);

            // Corner cell (intersection of v-scrollbar column and h-scrollbar row).
            if has_scrollbar {
                set_cell(
                    buf,
                    area.x + area.width - 1,
                    sb_y,
                    '┘',
                    qc(theme.separator),
                    qc(cell_bg),
                );
            }
        }
    }

    // ── Cursor ──────────────────────────────────────────────────
    if let Some(cursor) = &editor.cursor {
        let cursor_screen_y = area.y + cursor.pos.view_line as u16;
        let raw = editor
            .lines
            .get(cursor.pos.view_line)
            .map(|l| l.raw_text.as_str())
            .unwrap_or("");
        let vis_col = char_col_to_visual(raw, cursor.pos.col, editor.tabstop)
            .saturating_sub(editor.scroll_left) as u16;
        let cursor_screen_x = area.x + gutter_w + vis_col;

        let buf_area = buf.area;
        match cursor.shape {
            CursorShape::Block => {
                if cursor_screen_x < buf_area.x + buf_area.width
                    && cursor_screen_y < buf_area.y + buf_area.height
                {
                    let cell = &mut buf[(cursor_screen_x, cursor_screen_y)];
                    let old_fg = cell.fg;
                    let old_bg = cell.bg;
                    cell.set_fg(old_bg).set_bg(old_fg);
                }
            }
            CursorShape::Bar | CursorShape::Underline => {
                result.cursor_position = Some((cursor_screen_x, cursor_screen_y));
            }
        }
    }

    // ── AI ghost text (after cursor on cursor line) ─────────────
    if let Some(cursor) = &editor.cursor {
        if let Some(rl) = editor.lines.get(cursor.pos.view_line) {
            if let Some(ghost) = &rl.ghost_suffix {
                let ghost_screen_y = area.y + cursor.pos.view_line as u16;
                let vis_col = char_col_to_visual(&rl.raw_text, cursor.pos.col, editor.tabstop)
                    .saturating_sub(editor.scroll_left) as u16;
                let ghost_start_x = area.x + gutter_w + vis_col;
                let ghost_fg = qc(theme.ghost_text_fg);
                for (i, ch) in ghost.chars().enumerate() {
                    let gx = ghost_start_x + i as u16;
                    if gx >= area.x + area.width {
                        break;
                    }
                    set_cell(buf, gx, ghost_screen_y, ch, ghost_fg, window_bg);
                }
            }
        }
    }

    // ── Secondary cursors (multi-cursor) ────────────────────────
    let cursor_color = qc(theme.cursor);
    let bg_color = qc(theme.background);
    let has_extra_sels = !editor.extra_selections.is_empty();
    for extra_pos in &editor.extra_cursors {
        let sy = area.y + extra_pos.view_line as u16;
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
        let sx = area.x + gutter_w + vis_col;
        let buf_area = buf.area;
        if sx < buf_area.x + buf_area.width && sy < buf_area.y + buf_area.height {
            let cell = &mut buf[(sx, sy)];
            cell.set_bg(cursor_color).set_fg(bg_color);
        }
    }

    result
}

// ─── Private helpers ────────────────────────────────────────────────────

/// Convert a UTF-8 byte offset into a character index. Returns the
/// total char count if the offset is past the end. Walks back from
/// `clamped` to find a char boundary so non-boundary inputs don't
/// panic. Mirrors `vimcode::tui_main::byte_to_char_idx`.
fn byte_to_char_idx(text: &str, byte_offset: usize) -> usize {
    let clamped = byte_offset.min(text.len());
    let mut safe = clamped;
    while safe > 0 && !text.is_char_boundary(safe) {
        safe -= 1;
    }
    text[..safe].chars().count()
}

/// Convert a character-index column into a visual column, expanding
/// tabs to the next tab stop. Mirrors
/// `vimcode::tui_main::render_impl::char_col_to_visual`.
fn char_col_to_visual(raw_text: &str, char_col: usize, tabstop: usize) -> usize {
    let tabstop = tabstop.max(1);
    let mut vis = 0usize;
    for (i, ch) in raw_text.chars().enumerate() {
        if ch == '\n' || ch == '\r' {
            break;
        }
        if i >= char_col {
            break;
        }
        if ch == '\t' {
            vis = ((vis / tabstop) + 1) * tabstop;
        } else {
            vis += 1;
        }
    }
    vis
}

/// Paint one [`EditorLine`]'s text spans into `buf`, scrolled by
/// `scroll_left`, expanding tabs to `tabstop` and applying syntax
/// foreground / background / bold / italic from each [`StyledSpan`].
/// Inline annotations (virtual text) paint in `theme.annotation_fg`
/// after the line content. Mirrors
/// `vimcode::tui_main::render_impl::render_text_line`.
#[allow(clippy::too_many_arguments)]
fn render_text_line(
    buf: &mut Buffer,
    x_start: u16,
    y: u16,
    max_width: u16,
    line: &EditorLine,
    scroll_left: usize,
    theme: &Theme,
    window_bg: RColor,
    tabstop: usize,
) {
    let raw = &line.raw_text;
    let chars: Vec<char> = raw.chars().filter(|&c| c != '\n' && c != '\r').collect();

    let mut char_fgs: Vec<Color> = vec![theme.foreground; chars.len()];
    let mut char_bgs: Vec<Option<Color>> = vec![None; chars.len()];
    let mut char_mods: Vec<Modifier> = vec![Modifier::empty(); chars.len()];

    for span in &line.spans {
        let start = byte_to_char_idx(raw, span.start_byte);
        let end = byte_to_char_idx(raw, span.end_byte).min(chars.len());
        for i in start..end {
            char_fgs[i] = span.style.fg;
            char_bgs[i] = span.style.bg;
            let mut m = Modifier::empty();
            if span.style.bold {
                m |= Modifier::BOLD;
            }
            if span.style.italic {
                m |= Modifier::ITALIC;
            }
            char_mods[i] = m;
        }
    }

    let tabstop = tabstop.max(1);
    let mut vis_col: usize = 0;
    let mut cells: Vec<(usize, char, usize)> = Vec::with_capacity(chars.len());
    for (i, &ch) in chars.iter().enumerate() {
        if ch == '\t' {
            let next_stop = ((vis_col / tabstop) + 1) * tabstop;
            while vis_col < next_stop {
                cells.push((vis_col, ' ', i));
                vis_col += 1;
            }
        } else {
            cells.push((vis_col, ch, i));
            vis_col += 1;
        }
    }
    let total_vis_cols = vis_col;

    for &(vcol, ch, ci) in &cells {
        if vcol < scroll_left {
            continue;
        }
        let col = (vcol - scroll_left) as u16;
        if col >= max_width {
            break;
        }
        let fg = qc(char_fgs[ci]);
        let bg = char_bgs[ci].map(qc).unwrap_or(window_bg);
        if char_mods[ci].is_empty() {
            set_cell(buf, x_start + col, y, ch, fg, bg);
        } else {
            set_cell_styled(buf, x_start + col, y, ch, fg, bg, char_mods[ci], None);
        }
    }

    // Inline annotation / virtual text (e.g. git blame).
    if let Some(ann) = &line.annotation {
        let visible_cols = total_vis_cols.saturating_sub(scroll_left);
        let ann_start = x_start + visible_cols.min(max_width as usize) as u16;
        let ann_fg = qc(theme.annotation_fg);
        for (i, ch) in ann.chars().enumerate() {
            let col = ann_start + i as u16;
            if col >= x_start + max_width {
                break;
            }
            set_cell(buf, col, y, ch, ann_fg, window_bg);
        }
    }
}

/// Paint one selection range overlay onto `buf`. Walks the editor's
/// visible lines, mapping buffer-coordinate selection rows to
/// segment-local visual columns (so wrapped lines highlight only the
/// segment that overlaps the selection range). Sets each touched cell's
/// bg to `color`, preserving the cell's existing fg unless that fg
/// equals `window_bg` (in which case the cell would be invisible
/// against the new bg, so it's swapped to `default_fg`). Mirrors
/// `vimcode::tui_main::render_impl::render_selection`.
fn render_selection(
    buf: &mut Buffer,
    area: Rect,
    editor: &Editor,
    sel: &EditorSelection,
    window_bg: RColor,
    color: Color,
    default_fg: RColor,
) {
    let sel_bg = qc(color);
    let gutter_w = editor.gutter_char_width as u16;
    let text_area_x = area.x + gutter_w;
    let text_width = area.width.saturating_sub(gutter_w) as usize;

    for (row_idx, line) in editor.lines.iter().enumerate() {
        let buffer_line = line.line_idx;
        if buffer_line < sel.start_line || buffer_line > sel.end_line {
            continue;
        }
        let screen_y = area.y + row_idx as u16;
        let seg_offset = line.segment_col_offset;

        let (buf_col_start, buf_col_end) = match sel.kind {
            SelectionKind::Line => (0usize, usize::MAX),
            SelectionKind::Char => {
                let cs = if buffer_line == sel.start_line {
                    sel.start_col
                } else {
                    0
                };
                let ce = if buffer_line == sel.end_line {
                    sel.end_col + 1
                } else {
                    usize::MAX
                };
                (cs, ce)
            }
            SelectionKind::Block => (sel.start_col, sel.end_col + 1),
        };

        let char_count = line.raw_text.chars().filter(|&c| c != '\n').count().max(1);
        let seg_end = seg_offset + char_count;

        if buf_col_start >= seg_end && buf_col_end != usize::MAX {
            continue;
        }
        if buf_col_end <= seg_offset {
            continue;
        }

        let col_start = buf_col_start.saturating_sub(seg_offset);
        let col_end = if buf_col_end == usize::MAX {
            usize::MAX
        } else {
            buf_col_end.saturating_sub(seg_offset)
        };
        let effective_end = col_end.min(char_count);

        let vis_start = char_col_to_visual(&line.raw_text, col_start, editor.tabstop);
        let vis_end = char_col_to_visual(&line.raw_text, effective_end, editor.tabstop);

        for vis in vis_start..vis_end {
            if vis < editor.scroll_left {
                continue;
            }
            let screen_col = (vis - editor.scroll_left) as u16;
            if screen_col >= text_width as u16 {
                break;
            }
            let sx = text_area_x + screen_col;
            let buf_area = buf.area;
            if sx < buf_area.x + buf_area.width && screen_y < buf_area.y + buf_area.height {
                let cell = &mut buf[(sx, screen_y)];
                let old_fg = cell.fg;
                cell.set_bg(sel_bg);
                if old_fg == window_bg {
                    cell.set_fg(default_fg);
                }
            }
        }
    }
}
