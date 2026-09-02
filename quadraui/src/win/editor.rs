//! Direct2D / DirectWrite rasteriser for [`crate::primitives::editor::Editor`]
//! (issue #26).
//!
//! Positions come from [`Editor::layout`] (viewport / gutter / text
//! bounds, all in the caller's `cell_width`/`line_height` — uniform
//! monospace, same convention as TUI). Text and every other paint
//! category are done in this module via [`DWrite`] + [`fill_rect`] /
//! [`blend`].
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod editor;` and `backend.rs`'s
//! module docs. See `win::status_bar`'s module doc for why colours come
//! from `Theme::default()` rather than a live `WinBackend` theme field.
//!
//! # Scope for #26
//!
//! This issue's acceptance bar is "line numbers, syntax-highlighted
//! text, cursor, selections, diagnostics" — implemented below. Not yet
//! painted (all pre-existing GTK/TUI features out of scope for this
//! pass, not a compile-error gap): git-diff gutter column, breakpoint
//! gutter column, DAP-current-line background, fold headers, AI ghost
//! text, inline annotations, bracket-match highlight, indent guides,
//! colorcolumns, spell-check underlines, and the code-action lightbulb
//! glyph. `EditorPaintResult::cursor_position` is always `None` — this
//! backend paints its own caret directly (via [`fill_rect`]), the same
//! posture `GtkBackend::draw_editor`'s doc documents for GTK.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{blend, fill_rect, pop_clip, push_clip, DWrite};
use crate::backend::EditorPaintResult;
use crate::event::Rect;
use crate::primitives::editor::{
    CursorShape, DiagnosticSeverity, Editor, EditorLine, EditorSelection, SelectionKind,
};
use crate::theme::Theme;
use crate::types::Color;

/// Draw an [`Editor`] viewport (`editor.rect`) on `target`, at uniform
/// `cell_width` / `line_height` (DIPs).
///
/// # Visual contract
///
/// - **Background:** `Theme::editor_active_background` when
///   `editor.show_active_bg`, else `Theme::background`.
/// - **Cursorline:** `Theme::cursorline_bg` on `is_current_line` rows
///   when `editor.is_active && editor.cursorline`.
/// - **Selections** (primary + extra + yank highlight): `Theme::selection`
///   blended over the row background at `Theme::selection_alpha` (yank
///   uses `yank_highlight_bg` / `yank_highlight_alpha`). Painted before
///   text, same ordering as `gtk::editor`.
/// - **Gutter:** right-aligned `gutter_text`, `Theme::line_number_active_fg`
///   on the current line, else `Theme::line_number_fg`.
/// - **Text:** per-[`crate::primitives::editor::StyledSpan`] colour
///   (`Theme::foreground` where unset), honouring `bold`.
/// - **Diagnostics:** a 2-DIP underline under `[start_col, end_col)` in
///   `Theme::diagnostic_error` / `_warning` / `_info` / `_hint`.
/// - **Cursor:** `Theme::cursor` blended at `Theme::cursor_normal_alpha`
///   for `Block`; a solid 2-DIP bar for `Bar`; a solid 2-DIP underline
///   for `Underline`.
pub fn draw_editor(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    editor: &Editor,
    cell_width: f32,
    line_height: f32,
) -> EditorPaintResult {
    let theme = Theme::default();
    let rect = editor.rect;
    let layout = editor.layout(rect, cell_width, line_height);

    let bg = if editor.show_active_bg {
        theme.editor_active_background
    } else {
        theme.background
    };
    let _ = fill_rect(target, rect, bg);

    // ── Cursorline (spans the full width, including the gutter — same
    //    as `gtk::editor`, so it's painted before any clip is pushed) ──
    for (view_idx, line) in editor.lines.iter().enumerate() {
        if line.is_current_line && editor.is_active && editor.cursorline {
            let y = rect.y + view_idx as f32 * line_height;
            let _ = fill_rect(
                target,
                Rect::new(rect.x, y, rect.width, line_height),
                theme.cursorline_bg,
            );
        }
    }

    // ── Selections (painted before text) ────────────────────────────
    // Clipped to `text_bounds`: a `SelectionKind::Line` row (or any
    // selection reaching the buffer line's raw length) can be wider
    // than the visible viewport, and must not bleed past its right
    // edge — the gutter is never touched since every selection rect's
    // left edge already starts at `text_bounds.x`.
    push_clip(target, layout.text_bounds);
    if let Some(sel) = &editor.selection {
        paint_selection(
            target,
            sel,
            &editor.lines,
            rect,
            line_height,
            layout.text_bounds.x,
            cell_width,
            theme.selection,
            theme.selection_alpha,
        );
    }
    for sel in &editor.extra_selections {
        paint_selection(
            target,
            sel,
            &editor.lines,
            rect,
            line_height,
            layout.text_bounds.x,
            cell_width,
            theme.selection,
            theme.selection_alpha,
        );
    }
    if let Some(sel) = &editor.yank_highlight {
        paint_selection(
            target,
            sel,
            &editor.lines,
            rect,
            line_height,
            layout.text_bounds.x,
            cell_width,
            theme.yank_highlight_bg,
            theme.yank_highlight_alpha,
        );
    }
    pop_clip(target);

    // ── Gutter + text + diagnostics ──────────────────────────────────
    // The gutter number paints *outside* the `text_bounds` clip below
    // (it lives at `x < text_bounds.x`); text and diagnostics paint
    // *inside* it per line, so a long line's text can't bleed into the
    // gutter or past the viewport's right edge.
    for (view_idx, line) in editor.lines.iter().enumerate() {
        let y = rect.y + view_idx as f32 * line_height;

        if editor.gutter_char_width > 0 {
            let gutter_fg = if line.is_current_line {
                theme.line_number_active_fg
            } else {
                theme.line_number_fg
            };
            let gutter_w = editor.gutter_char_width as f32 * cell_width;
            let (gw, gh) = dwrite.measure_text(&line.gutter_text).unwrap_or((0.0, 0.0));
            let gx = rect.x + (gutter_w - gw).max(0.0);
            let gy = y + (line_height - gh) / 2.0;
            let _ = dwrite.draw_text(
                target,
                &line.gutter_text,
                Rect::new(gx, gy, gw, gh),
                gutter_fg,
            );
        }

        push_clip(target, layout.text_bounds);

        paint_line_text(
            target,
            dwrite,
            line,
            layout.text_bounds.x,
            y,
            line_height,
            cell_width,
            editor.scroll_left,
            layout.visible_cols,
            theme.foreground,
        );

        for diag in &line.diagnostics {
            let start = diag.start_col.saturating_sub(editor.scroll_left);
            let end = diag.end_col.saturating_sub(editor.scroll_left);
            if end <= start {
                continue;
            }
            let color = match diag.severity {
                DiagnosticSeverity::Error => theme.diagnostic_error,
                DiagnosticSeverity::Warning => theme.diagnostic_warning,
                DiagnosticSeverity::Information => theme.diagnostic_info,
                DiagnosticSeverity::Hint => theme.diagnostic_hint,
            };
            let ux = layout.text_bounds.x + start as f32 * cell_width;
            let uw = (end - start) as f32 * cell_width;
            let _ = fill_rect(target, Rect::new(ux, y + line_height - 2.0, uw, 2.0), color);
        }

        pop_clip(target);
    }

    // ── Cursor ───────────────────────────────────────────────────────
    if let Some(cursor) = &editor.cursor {
        let y = rect.y + cursor.pos.view_line as f32 * line_height;
        let col = cursor.pos.col.saturating_sub(editor.scroll_left);
        let x = layout.text_bounds.x + col as f32 * cell_width;
        match cursor.shape {
            CursorShape::Block => {
                let color = blend(bg, theme.cursor, theme.cursor_normal_alpha);
                let _ = fill_rect(target, Rect::new(x, y, cell_width, line_height), color);
            }
            CursorShape::Bar => {
                let _ = fill_rect(target, Rect::new(x, y, 2.0, line_height), theme.cursor);
            }
            CursorShape::Underline => {
                let _ = fill_rect(
                    target,
                    Rect::new(x, y + line_height - 2.0, cell_width, 2.0),
                    theme.cursor,
                );
            }
        }
    }

    // GTK-style posture: this backend paints its own caret directly
    // (above), so there's no terminal-cursor position for the host to
    // reposition — see this module's doc.
    EditorPaintResult::default()
}

/// Paint one selection range as a translucent overlay across the
/// visible `lines`, before text is painted. `SelectionKind::Block` is
/// treated the same as `Char` (a per-row `[start_col, end_col)` span) —
/// a column-rectangle selection needs the same column on every row,
/// which this approximation already gives when `start_col`/`end_col`
/// are equal across rows; a genuinely ragged block selection paints
/// slightly wider than the exact column rectangle. Follow-up scope, not
/// a correctness gap for the common case.
#[allow(clippy::too_many_arguments)]
fn paint_selection(
    target: &ID2D1RenderTarget,
    sel: &EditorSelection,
    lines: &[EditorLine],
    rect: Rect,
    line_height: f32,
    text_x: f32,
    cell_width: f32,
    color: Color,
    alpha: f32,
) {
    for (view_idx, line) in lines.iter().enumerate() {
        if line.line_idx < sel.start_line || line.line_idx > sel.end_line {
            continue;
        }
        let line_len = line.raw_text.trim_end_matches('\n').chars().count();
        let (start_col, end_col) = match sel.kind {
            SelectionKind::Line => (0, line_len),
            SelectionKind::Char | SelectionKind::Block => {
                let start = if line.line_idx == sel.start_line {
                    sel.start_col
                } else {
                    0
                };
                let end = if line.line_idx == sel.end_line {
                    sel.end_col
                } else {
                    line_len
                };
                (start, end)
            }
        };
        if end_col <= start_col {
            continue;
        }
        let y = rect.y + view_idx as f32 * line_height;
        let x = text_x + start_col as f32 * cell_width;
        let w = (end_col - start_col) as f32 * cell_width;
        let blended = blend(Theme::default().background, color, alpha);
        let _ = fill_rect(target, Rect::new(x, y, w, line_height), blended);
    }
}

/// Paint `line`'s visible text window (`[scroll_left, scroll_left +
/// visible_cols)` characters) at `y`, splitting into contiguous runs by
/// [`crate::primitives::editor::StyledSpan`] colour.
#[allow(clippy::too_many_arguments)]
fn paint_line_text(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    line: &EditorLine,
    text_x: f32,
    y: f32,
    line_height: f32,
    cell_width: f32,
    scroll_left: usize,
    visible_cols: usize,
    default_fg: Color,
) {
    let raw = line.raw_text.trim_end_matches('\n');
    let chars: Vec<(usize, char)> = raw.char_indices().collect();
    if chars.len() <= scroll_left {
        return;
    }
    let end = (scroll_left + visible_cols).min(chars.len());
    if end <= scroll_left {
        return;
    }

    // Resolve a colour + bold flag per visible character from
    // byte-range spans, then coalesce into runs so each DirectWrite
    // call paints a contiguous same-style substring.
    let style_at = |byte_off: usize| -> (Color, bool) {
        for span in &line.spans {
            if byte_off >= span.start_byte && byte_off < span.end_byte {
                return (span.style.fg, span.style.bold);
            }
        }
        (default_fg, false)
    };

    let mut run_start = scroll_left;
    let mut run_style = style_at(chars[scroll_left].0);
    let mut cursor_x = text_x;

    let flush = |from: usize, to: usize, style: (Color, bool), cursor_x: &mut f32| {
        if to <= from {
            return;
        }
        let start_byte = chars[from].0;
        let end_byte = chars.get(to).map(|(b, _)| *b).unwrap_or(raw.len());
        let text = &raw[start_byte..end_byte];
        let w = (to - from) as f32 * cell_width;
        let (_, h) = dwrite.measure_text(text).unwrap_or((0.0, line_height));
        let _ = dwrite.draw_text_styled(
            target,
            text,
            Rect::new(*cursor_x, y, w, h.max(line_height)),
            style.0,
            style.1,
        );
        *cursor_x += w;
    };

    // Iterate the slice rather than the index range (`clippy::
    // needless_range_loop`, an error under CI's `-D warnings` and only
    // visible on the windows-latest leg): `skip`/`take` reproduce the
    // half-open `(scroll_left + 1)..end` window, and `enumerate` still
    // yields the absolute index the run bookkeeping needs.
    for (i, (byte_off, _)) in chars.iter().enumerate().take(end).skip(scroll_left + 1) {
        let s = style_at(*byte_off);
        if s != run_style {
            flush(run_start, i, run_style, &mut cursor_x);
            run_start = i;
            run_style = s;
        }
    }
    flush(run_start, end, run_style, &mut cursor_x);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Rect as QRect;
    use crate::primitives::editor::{CursorPos, DiagnosticMark, EditorCursor, Style, StyledSpan};
    use crate::types::WidgetId;
    use crate::win::testing::HeadlessSurface;

    const CELL_W: f32 = 8.0;
    const LINE_H: f32 = 16.0;

    fn plain_line(idx: usize, text: &str) -> EditorLine {
        EditorLine {
            raw_text: text.to_string(),
            gutter_text: format!("{:>3}", idx + 1),
            spans: vec![StyledSpan {
                start_byte: 0,
                end_byte: text.len(),
                style: Style {
                    fg: Color::rgb(200, 200, 200),
                    bg: None,
                    bold: false,
                    italic: false,
                    font_scale: 1.0,
                },
            }],
            line_idx: idx,
            is_current_line: false,
            is_fold_header: false,
            folded_line_count: 0,
            git_diff: None,
            diff_status: None,
            diagnostics: Vec::new(),
            spell_errors: Vec::new(),
            is_breakpoint: false,
            is_conditional_bp: false,
            is_dap_current: false,
            is_wrap_continuation: false,
            segment_col_offset: 0,
            annotation: None,
            ghost_suffix: None,
            is_ghost_continuation: false,
            indent_guides: Vec::new(),
            colorcolumns: Vec::new(),
        }
    }

    fn editor(lines: Vec<EditorLine>) -> Editor {
        Editor {
            id: WidgetId::new("editor"),
            rect: QRect::new(0.0, 0.0, 200.0, 100.0),
            lines,
            cursor: None,
            extra_cursors: Vec::new(),
            selection: None,
            extra_selections: Vec::new(),
            yank_highlight: None,
            scroll_top: 0,
            scroll_left: 0,
            total_lines: 3,
            max_col: 20,
            gutter_char_width: 4,
            is_active: true,
            show_active_bg: false,
            has_git_diff: false,
            has_breakpoints: false,
            diagnostic_gutter: Default::default(),
            code_action_lines: Default::default(),
            bracket_match_positions: Vec::new(),
            active_indent_col: None,
            tabstop: 4,
            cursorline: true,
            lightbulb_glyph: '!',
        }
    }

    /// Painting must not panic across a mix of plain lines, a cursor, a
    /// selection, and a diagnostic underline, and the gutter's line
    /// number must be visible pixels distinct from the background.
    #[test]
    fn paints_without_panicking_and_gutter_is_visible() {
        let surface = HeadlessSurface::new(200, 100).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Consolas", 10.0).expect("create DWrite");
        let mut lines = vec![
            plain_line(0, "fn main() {"),
            plain_line(1, "    ok();"),
            plain_line(2, "}"),
        ];
        lines[0].is_current_line = true;
        lines[1].diagnostics.push(DiagnosticMark {
            start_col: 4,
            end_col: 6,
            severity: DiagnosticSeverity::Error,
            message: "unused".into(),
        });
        let mut e = editor(lines);
        e.cursor = Some(EditorCursor {
            pos: CursorPos {
                view_line: 0,
                col: 3,
            },
            shape: CursorShape::Bar,
        });
        e.selection = Some(EditorSelection {
            kind: SelectionKind::Char,
            start_line: 1,
            start_col: 0,
            end_line: 1,
            end_col: 4,
        });

        surface
            .paint(|target| {
                draw_editor(target, &dwrite, &e, CELL_W, LINE_H);
            })
            .expect("paint editor");

        let theme = Theme::default();
        // Current-line gutter number is painted in `line_number_active_fg`,
        // distinct from the plain background. Scan the whole gutter
        // column (0..gutter width, the full first row's height) rather
        // than a guessed glyph position — real DirectWrite glyph
        // placement within that box isn't known ahead of a real
        // Windows font metrics pass.
        let gutter_w = (4.0 * CELL_W) as u32;
        let found = (0..gutter_w).any(|x| {
            (0..LINE_H as u32).any(|y| {
                let px = surface.pixel_at(x, y);
                (px.r, px.g, px.b)
                    == (
                        theme.line_number_active_fg.r,
                        theme.line_number_active_fg.g,
                        theme.line_number_active_fg.b,
                    )
            })
        });
        assert!(
            found,
            "current line's gutter number should paint line_number_active_fg somewhere in the gutter column"
        );
    }

    /// A `Line`-kind selection covers the full row width regardless of
    /// `start_col`/`end_col`.
    #[test]
    fn line_selection_spans_full_row() {
        let surface = HeadlessSurface::new(200, 100).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Consolas", 10.0).expect("create DWrite");
        // 2 visible characters + 23 trailing spaces: `line_len` (25,
        // matching the selection's full-row width) is unaffected by the
        // trailing spaces, but space glyphs paint no ink — so the
        // sample point below lands on pure selection-tint background,
        // not real DirectWrite glyph rendering this test has no way to
        // predict pixel-for-pixel without a live Windows font pass.
        let text = format!("ab{}", " ".repeat(23));
        let lines = vec![plain_line(0, &text)];
        let mut e = editor(lines);
        e.selection = Some(EditorSelection {
            kind: SelectionKind::Line,
            start_line: 0,
            start_col: 2,
            end_line: 0,
            end_col: 3,
        });

        surface
            .paint(|target| {
                draw_editor(target, &dwrite, &e, CELL_W, LINE_H);
            })
            .expect("paint editor");

        let theme = Theme::default();
        let text_x = (e.gutter_char_width as f32 * CELL_W) as u32;
        // Sample near the right edge of the viewport, well past
        // `end_col`'s column — a `Line` selection should still cover it.
        let sample_x = 180u32.max(text_x + 5);
        let px = surface.pixel_at(sample_x, 4);
        let expected = blend(theme.background, theme.selection, theme.selection_alpha);
        assert_eq!((px.r, px.g, px.b), (expected.r, expected.g, expected.b));
    }
}
