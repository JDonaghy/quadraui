//! macOS rasteriser for [`crate::primitives::editor::Editor`].
//!
//! Minimum viable port of `crate::gtk::editor::draw_editor`: background,
//! per-line text via Core Text, line-number gutter, and the primary
//! cursor (Block / Bar / Underline). Returns a default
//! [`EditorPaintResult`] — macOS, like GTK, paints its own caret rather
//! than delegating to a terminal cursor.
//!
//! ## Scope omissions (follow-up)
//!
//! Deferred to subsequent tickets to keep #39 manageable; each ships
//! when a kubeui-class consumer exercises it on macOS:
//!
//! - **Selection overlays** — `editor.selection`, `extra_selections`,
//!   `yank_highlight`. Need the unified text-attribute path that also
//!   carries bold/italic and selection-bg.
//! - **Diagnostics + spell underlines** — wavy/dotted underlines via
//!   `kCTUnderlineStyleAttributeName` (deferred with attrs).
//! - **Indent guides, color columns, bracket-match alpha rects**.
//! - **AI ghost text + multi-cursor secondary carets**.
//! - **Diff backgrounds** (`DiffLine::Added`/`Removed`/`Padding`) and
//!   **cursorline highlight** — straightforward pixel fills, will land
//!   alongside the consumer that needs them.
//! - **Gutter chrome** beyond line numbers — breakpoint glyph, git
//!   column, diagnostic dot, lightbulb glyph.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::backend::EditorPaintResult;
use crate::primitives::editor::{CursorShape, Editor};
use crate::theme::Theme;
use crate::types::Color;

/// Paint `editor` onto `ctx`. Returns the default
/// [`EditorPaintResult`] — macOS paints its own caret.
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
pub unsafe fn draw_editor(
    ctx: CGContextRef,
    font: &CTFont,
    editor: &Editor,
    theme: &Theme,
    char_width: f64,
    line_height: f64,
) -> EditorPaintResult {
    let rect = &editor.rect;
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return EditorPaintResult::default();
    }

    let x = rect.x as f64;
    let y = rect.y as f64;
    let w = rect.width as f64;
    let h = rect.height as f64;

    CGContextSaveGState(ctx);
    CGContextClipToRect(ctx, CGRect::new_xywh(x, y, w, h));

    let bg = if editor.show_active_bg {
        theme.editor_active_background
    } else {
        theme.background
    };
    fill_rect(ctx, x, y, w, h, bg);

    let gutter_width = editor.gutter_char_width as f64 * char_width;
    let h_scroll_offset = editor.scroll_left as f64 * char_width;
    let text_x_offset = x + gutter_width - h_scroll_offset;

    // Cursorline highlight (active editor only) — painted before text.
    if editor.is_active && editor.cursorline {
        for (view_idx, line) in editor.lines.iter().enumerate() {
            if line.is_current_line {
                let line_y = y + view_idx as f64 * line_height;
                fill_rect(ctx, x, line_y, w, line_height, theme.cursorline_bg);
            }
        }
    }

    // Lines + gutter.
    for (view_idx, line) in editor.lines.iter().enumerate() {
        let line_y = y + view_idx as f64 * line_height;
        if line_y >= y + h {
            break;
        }

        // Gutter — right-aligned line number text.
        if gutter_width > 0.0 && !line.gutter_text.is_empty() {
            let (gtw, _) = measure_text(font, &line.gutter_text);
            let gx = x + gutter_width - gtw - 4.0;
            draw_text(
                ctx,
                font,
                &line.gutter_text,
                gx.max(x + 2.0),
                line_y + (line_height - measure_text(font, &line.gutter_text).1) / 2.0,
                color_to_cg(theme.line_number_fg),
            );
        }

        // Raw text painted as a single fg run; per-span colouring is
        // applied on top below.
        let raw_x = text_x_offset;
        if !line.raw_text.is_empty() {
            draw_text(
                ctx,
                font,
                &line.raw_text,
                raw_x,
                line_y,
                color_to_cg(theme.foreground),
            );
        }

        // Per-span colouring — re-render coloured slices on top of the
        // raw run. Crude but produces correct colour at character
        // boundaries given the monospace baseline.
        for span in &line.spans {
            let start = span.start_byte.min(line.raw_text.len());
            let end = span.end_byte.min(line.raw_text.len());
            if start >= end {
                continue;
            }
            let prefix = &line.raw_text[..start];
            let slice = &line.raw_text[start..end];
            let (px, _) = measure_text(font, prefix);
            // Repaint the slice's bg first (if set) so the raw run
            // beneath doesn't bleed through.
            if let Some(sbg) = span.style.bg {
                let (sw, _) = measure_text(font, slice);
                fill_rect(ctx, raw_x + px, line_y, sw, line_height, sbg);
            }
            draw_text(
                ctx,
                font,
                slice,
                raw_x + px,
                line_y,
                color_to_cg(span.style.fg),
            );
        }
    }

    // Primary cursor.
    if let Some(cursor) = editor.cursor {
        if cursor.pos.view_line < editor.lines.len() {
            let line = &editor.lines[cursor.pos.view_line];
            let prefix_end = char_byte_offset(&line.raw_text, cursor.pos.col);
            let prefix = &line.raw_text[..prefix_end];
            let (prefix_w, _) = measure_text(font, prefix);
            let cur_x = text_x_offset + prefix_w;
            let cur_y = y + cursor.pos.view_line as f64 * line_height;
            match cursor.shape {
                CursorShape::Block => {
                    fill_rect(ctx, cur_x, cur_y, char_width, line_height, theme.cursor);
                    // Re-paint the glyph under the cursor in background
                    // colour so it reads against the cursor fill.
                    let ch = line.raw_text[prefix_end..]
                        .chars()
                        .next()
                        .map(|c| c.to_string())
                        .unwrap_or_default();
                    if !ch.is_empty() {
                        draw_text(ctx, font, &ch, cur_x, cur_y, color_to_cg(theme.background));
                    }
                }
                CursorShape::Bar => {
                    fill_rect(ctx, cur_x, cur_y, 2.0, line_height, theme.cursor);
                }
                CursorShape::Underline => {
                    let underline_h = (line_height * 0.12).max(1.0);
                    fill_rect(
                        ctx,
                        cur_x,
                        cur_y + line_height - underline_h,
                        char_width,
                        underline_h,
                        theme.cursor,
                    );
                }
            }
        }
    }

    CGContextRestoreGState(ctx);
    EditorPaintResult::default()
}

/// Byte offset corresponding to the `col`-th character of `s`. Saturates
/// at `s.len()` for out-of-range columns.
fn char_byte_offset(s: &str, col: usize) -> usize {
    s.char_indices().nth(col).map(|(b, _)| b).unwrap_or(s.len())
}

fn color_to_cg(c: Color) -> (f64, f64, f64, f64) {
    (
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    )
}

unsafe fn fill_rect(ctx: CGContextRef, x: f64, y: f64, w: f64, h: f64, c: Color) {
    let (r, g, b, a) = color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r, g, b, a);
    CGContextFillRect(ctx, CGRect::new_xywh(x, y, w, h));
}

trait CGRectExt {
    fn new_xywh(x: f64, y: f64, w: f64, h: f64) -> Self;
}
impl CGRectExt for CGRect {
    fn new_xywh(x: f64, y: f64, w: f64, h: f64) -> Self {
        use core_graphics::geometry::{CGPoint, CGSize};
        CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h))
    }
}

extern "C" {
    fn CGContextSaveGState(c: CGContextRef);
    fn CGContextRestoreGState(c: CGContextRef);
    fn CGContextClipToRect(c: CGContextRef, rect: CGRect);
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
}

#[cfg(test)]
mod tests {
    use super::super::headless::BitmapSurface;
    use super::super::text::{font_metrics, make_font};
    use super::super::MacBackend;
    use super::*;
    use crate::event::{Rect as QRect, Viewport};
    use crate::primitives::editor::{
        CursorPos, EditorCursor, EditorLine, Style, StyledSpan as ESpan,
    };
    use crate::theme::Theme;
    use crate::Backend;
    use std::collections::{HashMap, HashSet};

    const W: u32 = 400;
    const H: u32 = 160;

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn one_line(text: &str) -> EditorLine {
        EditorLine {
            raw_text: text.into(),
            gutter_text: "1".into(),
            spans: vec![],
            line_idx: 0,
            is_current_line: true,
            is_fold_header: false,
            folded_line_count: 0,
            git_diff: None,
            diff_status: None,
            diagnostics: vec![],
            spell_errors: vec![],
            is_breakpoint: false,
            is_conditional_bp: false,
            is_dap_current: false,
            is_wrap_continuation: false,
            segment_col_offset: 0,
            annotation: None,
            ghost_suffix: None,
            is_ghost_continuation: false,
            indent_guides: vec![],
            colorcolumns: vec![],
        }
    }

    fn editor_with_cursor(text: &str, shape: CursorShape, col: usize) -> Editor {
        Editor {
            id: "editor:0".into(),
            rect: QRect::new(0.0, 0.0, W as f32, H as f32),
            lines: vec![one_line(text)],
            cursor: Some(EditorCursor {
                pos: CursorPos { view_line: 0, col },
                shape,
            }),
            extra_cursors: vec![],
            selection: None,
            extra_selections: vec![],
            yank_highlight: None,
            scroll_top: 0,
            scroll_left: 0,
            total_lines: 1,
            max_col: text.chars().count(),
            gutter_char_width: 3,
            is_active: true,
            show_active_bg: false,
            has_git_diff: false,
            has_breakpoints: false,
            diagnostic_gutter: HashMap::new(),
            code_action_lines: HashSet::new(),
            bracket_match_positions: vec![],
            active_indent_col: None,
            tabstop: 4,
            cursorline: false,
            lightbulb_glyph: '!',
        }
    }

    fn paint_via_backend(editor: &Editor) -> BitmapSurface {
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_editor(editor.rect, editor);
        });
        backend.end_frame();
        surface
    }

    #[test]
    fn block_cursor_paints_theme_cursor_color() {
        let editor = editor_with_cursor("let x = 1;", CursorShape::Block, 0);
        let surface = paint_via_backend(&editor);
        let theme = Theme::default();
        let metrics = font_metrics(&font());
        // Cursor lands at column 0 in gutter-offset coords. Gutter is
        // 3 char_widths, so cursor starts at 3*char_width on x.
        let cx = (3.0 * metrics.char_width) as u32 + 1;
        let cy = (metrics.line_height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(cx, cy);
        assert_eq!(
            (r, g, b),
            (theme.cursor.r, theme.cursor.g, theme.cursor.b),
            "block cursor pixel at ({}, {}) should be theme.cursor",
            cx,
            cy,
        );
    }

    #[test]
    fn cursorline_highlight_painted_when_active() {
        let mut editor = editor_with_cursor("let x = 1;", CursorShape::Bar, 0);
        editor.cursorline = true;
        let surface = paint_via_backend(&editor);
        let theme = Theme::default();
        // Probe far to the right of any glyph or cursor.
        let px = W - 4;
        let py = 4_u32;
        let (r, g, b, _) = surface.pixel(px, py);
        assert_eq!(
            (r, g, b),
            (
                theme.cursorline_bg.r,
                theme.cursorline_bg.g,
                theme.cursorline_bg.b
            ),
        );
    }

    #[test]
    fn span_bg_paints_over_raw_run() {
        // A line whose first 3 bytes carry a coloured background — the
        // probe inside that range should read the span bg, not the
        // editor bg.
        let mut line = one_line("alpha beta");
        line.spans = vec![ESpan {
            start_byte: 0,
            end_byte: 3,
            style: Style {
                fg: Color::rgb(255, 255, 255),
                bg: Some(Color::rgb(50, 100, 150)),
                bold: false,
                italic: false,
                font_scale: 1.0,
            },
        }];
        let mut editor = editor_with_cursor("alpha beta", CursorShape::Bar, 99);
        editor.lines = vec![line];
        let surface = paint_via_backend(&editor);
        let metrics = font_metrics(&font());
        // The span covers raw_text[0..3] ("alp"). Probe at the very
        // top scanline (y=0) of the line — above any glyph ink and
        // its anti-aliased boundary, fully inside the span bg.
        let text_start = 3.0 * metrics.char_width;
        let px = (text_start + metrics.char_width * 0.5) as u32;
        let py = 0_u32;
        let (r, g, b, _) = surface.pixel(px, py);
        // Span bg painted over editor bg.
        assert_eq!((r, g, b), (50, 100, 150));
    }

    #[test]
    fn empty_editor_returns_default_paint_result() {
        let editor = Editor {
            id: "editor:empty".into(),
            rect: QRect::new(0.0, 0.0, 0.0, 0.0),
            lines: vec![],
            cursor: None,
            extra_cursors: vec![],
            selection: None,
            extra_selections: vec![],
            yank_highlight: None,
            scroll_top: 0,
            scroll_left: 0,
            total_lines: 0,
            max_col: 0,
            gutter_char_width: 0,
            is_active: false,
            show_active_bg: false,
            has_git_diff: false,
            has_breakpoints: false,
            diagnostic_gutter: HashMap::new(),
            code_action_lines: HashSet::new(),
            bracket_match_positions: vec![],
            active_indent_col: None,
            tabstop: 4,
            cursorline: false,
            lightbulb_glyph: '!',
        };
        let surface = BitmapSurface::new(10, 10);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(10.0, 10.0, 1.0));
        let result = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            *result.borrow_mut() = Some(b.draw_editor(editor.rect, &editor));
        });
        backend.end_frame();
        assert_eq!(result.into_inner().unwrap(), EditorPaintResult::default(),);
    }
}
