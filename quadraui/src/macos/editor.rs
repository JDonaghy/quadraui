//! macOS rasteriser for [`crate::primitives::editor::Editor`].
//!
//! Port of `crate::gtk::editor::draw_editor`: background (incl. DAP
//! stopped-line / diff / cursorline priority), per-line text via Core
//! Text, selection overlays (`selection`, `extra_selections`,
//! `yank_highlight`), line-number gutter, and the primary cursor
//! (Block / Bar / Underline). Returns a default [`EditorPaintResult`]
//! — macOS, like GTK, paints its own caret rather than delegating to a
//! terminal cursor.
//!
//! ## Scope omissions (follow-up)
//!
//! Deferred to subsequent tickets (#943); each ships when a
//! kubeui-class consumer exercises it on macOS:
//!
//! - **Diagnostics + spell underlines** — wavy/dotted underlines via
//!   `kCTUnderlineStyleAttributeName` (deferred with attrs).
//! - **Indent guides, color columns, bracket-match alpha rects**.
//! - **AI ghost text + multi-cursor secondary carets**.
//! - **Gutter chrome** beyond line numbers — breakpoint glyph, git
//!   column, diagnostic dot, lightbulb glyph.

use core_graphics::geometry::CGRect;
use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::text::{draw_text, measure_text};
use crate::backend::EditorPaintResult;
use crate::primitives::editor::{
    CursorShape, DiffLine, Editor, EditorLine, EditorSelection, SelectionKind,
};
use crate::text_util::snap_to_char_boundary;
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

    // Cursorline / diff / DAP stopped-line backgrounds — painted before
    // text. Priority mirrors GTK: DAP-stopped > diff status >
    // cursorline (crate::gtk::editor::draw_editor).
    for (view_idx, line) in editor.lines.iter().enumerate() {
        let row_bg = if line.is_dap_current {
            Some(theme.dap_stopped_bg)
        } else if let Some(diff_status) = line.diff_status {
            match diff_status {
                DiffLine::Added => Some(theme.diff_added_bg),
                DiffLine::Removed => Some(theme.diff_removed_bg),
                DiffLine::Padding => Some(theme.diff_padding_bg),
                DiffLine::Same => None,
            }
        } else if line.is_current_line && editor.is_active && editor.cursorline {
            Some(theme.cursorline_bg)
        } else {
            None
        };
        if let Some(color) = row_bg {
            let line_y = y + view_idx as f64 * line_height;
            fill_rect(ctx, x, line_y, w, line_height, color);
        }
    }

    // Selection overlays — painted before text so text reads on top,
    // matching GTK's ordering (see module doc on crate::gtk::editor).
    if let Some(sel) = &editor.selection {
        draw_visual_selection(
            ctx,
            font,
            sel,
            &editor.lines,
            x,
            y,
            w,
            line_height,
            text_x_offset,
            theme.selection,
            theme.selection_alpha as f64,
        );
    }
    for esel in &editor.extra_selections {
        draw_visual_selection(
            ctx,
            font,
            esel,
            &editor.lines,
            x,
            y,
            w,
            line_height,
            text_x_offset,
            theme.selection,
            theme.selection_alpha as f64,
        );
    }
    if let Some(yh) = &editor.yank_highlight {
        draw_visual_selection(
            ctx,
            font,
            yh,
            &editor.lines,
            x,
            y,
            w,
            line_height,
            text_x_offset,
            theme.yank_highlight_bg,
            theme.yank_highlight_alpha as f64,
        );
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
            let start = snap_to_char_boundary(&line.raw_text, span.start_byte);
            let end = snap_to_char_boundary(&line.raw_text, span.end_byte);
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

/// Like [`fill_rect`], but the alpha channel is `alpha` instead of the
/// colour's own — mirrors GTK's `cr.set_source_rgba(r, g, b, alpha)`
/// for selection overlays, where `theme.selection_alpha` /
/// `theme.yank_highlight_alpha` carry the opacity independent of the
/// colour's own (always-opaque) `a` channel.
unsafe fn fill_rect_alpha(ctx: CGContextRef, x: f64, y: f64, w: f64, h: f64, c: Color, alpha: f64) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let (r, g, b, _) = color_to_cg(c);
    CGContextSetRGBFillColor(ctx, r, g, b, alpha);
    CGContextFillRect(ctx, CGRect::new_xywh(x, y, w, h));
}

/// Paint one visual-selection overlay (`editor.selection`,
/// `extra_selections`, or `yank_highlight` — they share a shape) across
/// `lines`. Port of `crate::gtk::editor::draw_visual_selection`, using
/// Core Text glyph-run measurement in place of Pango's
/// `index_to_pos`/`pixel_size`.
#[allow(clippy::too_many_arguments)]
unsafe fn draw_visual_selection(
    ctx: CGContextRef,
    font: &CTFont,
    sel: &EditorSelection,
    lines: &[EditorLine],
    x: f64,
    y: f64,
    w: f64,
    line_height: f64,
    text_x_offset: f64,
    color: Color,
    alpha: f64,
) {
    for (view_idx, rl) in lines.iter().enumerate() {
        if rl.is_ghost_continuation || rl.diff_status == Some(DiffLine::Padding) {
            continue;
        }
        let line_idx = rl.line_idx;
        if line_idx < sel.start_line || line_idx > sel.end_line {
            continue;
        }
        let line_y = y + view_idx as f64 * line_height;

        if sel.kind == SelectionKind::Line {
            let highlight_width = w - (text_x_offset - x);
            fill_rect_alpha(
                ctx,
                text_x_offset,
                line_y,
                highlight_width,
                line_height,
                color,
                alpha,
            );
            continue;
        }

        // Char / Block: both resolve to a per-line [start_col, end_col]
        // range — Char narrows it to the selection's own start/end line,
        // Block applies the same column range to every covered line.
        let sco = rl.segment_col_offset;
        let seg_chars = rl.raw_text.chars().count();
        let (sel_start, sel_end) = if sel.kind == SelectionKind::Char {
            let s = if line_idx == sel.start_line {
                sel.start_col
            } else {
                0
            };
            let e = if line_idx == sel.end_line {
                sel.end_col + 1
            } else {
                usize::MAX
            };
            (s, e)
        } else {
            (sel.start_col, sel.end_col + 1)
        };

        let hi_start = sel_start.max(sco).saturating_sub(sco);
        let hi_end = sel_end.min(sco + seg_chars).saturating_sub(sco);
        if hi_start >= hi_end {
            continue;
        }

        let start_byte = char_byte_offset(&rl.raw_text, hi_start);
        let (prefix_w, _) = measure_text(font, &rl.raw_text[..start_byte]);
        let start_x = text_x_offset + prefix_w;

        let width = if hi_end >= seg_chars && sel_end > sco + seg_chars {
            // Selection runs past this visual segment's text — extend
            // the highlight to the end of the rendered line content
            // (matches GTK's use of the Pango layout's full pixel
            // width in this branch).
            let (line_w, _) = measure_text(font, &rl.raw_text);
            (text_x_offset + line_w - start_x).max(0.0)
        } else {
            let end_byte = char_byte_offset(&rl.raw_text, hi_end);
            let (end_w, _) = measure_text(font, &rl.raw_text[..end_byte]);
            (text_x_offset + end_w - start_x).max(0.0)
        };
        if width > 0.0 {
            fill_rect_alpha(ctx, start_x, line_y, width, line_height, color, alpha);
        }
    }
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
        CursorPos, EditorCursor, EditorLine, EditorStyledSpan as ESpan, Style,
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

    /// Regression for issue #503: syntax-highlighting `StyledSpan`
    /// byte offsets were only `.min(len)`-clamped, not snapped to a
    /// char boundary — a span landing mid-multibyte-character (é / CJK
    /// / emoji) used to panic `&line.raw_text[..start]` /
    /// `[start..end]`. Also exercises a multibyte cursor column via
    /// `char_byte_offset`.
    #[test]
    fn multibyte_span_and_cursor_do_not_panic() {
        // "café🎉 beta" — byte 4 sits inside the 2-byte 'é' (starts at
        // byte 3); byte 8 sits inside the 4-byte emoji (starts at byte 6).
        let text = "café🎉 beta";
        assert!(!text.is_char_boundary(4));
        assert!(!text.is_char_boundary(8));

        let mut line = one_line(text);
        line.spans = vec![ESpan {
            start_byte: 4,
            end_byte: 8,
            style: Style {
                fg: Color::rgb(255, 255, 255),
                bg: Some(Color::rgb(50, 100, 150)),
                bold: false,
                italic: false,
                font_scale: 1.0,
            },
        }];
        // Cursor column 3 lands on 'é' (a char index, not a byte
        // offset) — `char_byte_offset` must resolve it to a boundary.
        let mut editor = editor_with_cursor(text, CursorShape::Block, 3);
        editor.lines = vec![line];

        // Must not panic.
        let _surface = paint_via_backend(&editor);
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

    /// `a` and `b` agree within `tol` — used for alpha-blended overlay
    /// pixels, where the exact composited value depends on CG's
    /// blending internals and only the *direction* (blended toward the
    /// overlay colour) is being asserted.
    fn approx(a: u8, b: u8, tol: u8) -> bool {
        a.abs_diff(b) <= tol
    }

    /// Issue #943: `editor.selection` was never read by the macOS
    /// rasteriser — visual-mode / mouse selection painted nothing.
    /// Probe a pixel inside a `Line`-kind selection and assert it
    /// reads as `theme.selection` alpha-blended over the background,
    /// not the plain background.
    #[test]
    fn line_selection_paints_over_background() {
        let mut editor = editor_with_cursor("let x = 1;", CursorShape::Bar, 99);
        editor.selection = Some(EditorSelection {
            kind: SelectionKind::Line,
            start_line: 0,
            start_col: 0,
            end_line: 0,
            end_col: 0,
        });
        let surface = paint_via_backend(&editor);
        let theme = Theme::default();

        // Far right of any glyph ink, same probe spot as the cursorline
        // test — inside the full-width Line-selection highlight.
        let (r, g, b, _) = surface.pixel(W - 4, 4);
        assert_ne!(
            (r, g, b),
            (theme.background.r, theme.background.g, theme.background.b),
            "selected row should not read as plain background"
        );
        let expected_r = (theme.selection.r as f32 * theme.selection_alpha
            + theme.background.r as f32 * (1.0 - theme.selection_alpha))
            as u8;
        let expected_g = (theme.selection.g as f32 * theme.selection_alpha
            + theme.background.g as f32 * (1.0 - theme.selection_alpha))
            as u8;
        let expected_b = (theme.selection.b as f32 * theme.selection_alpha
            + theme.background.b as f32 * (1.0 - theme.selection_alpha))
            as u8;
        assert!(
            approx(r, expected_r, 4) && approx(g, expected_g, 4) && approx(b, expected_b, 4),
            "expected ~({expected_r}, {expected_g}, {expected_b}), got ({r}, {g}, {b})"
        );
    }

    /// Issue #943: `yank_highlight` was set on `Editor` but never read
    /// — `yy` flashed on TUI and did nothing on macOS. Assert the
    /// yank overlay paints its own themed colour (distinct from a
    /// plain `editor.selection` overlay).
    #[test]
    fn yank_highlight_paints_distinct_color() {
        let mut editor = editor_with_cursor("let x = 1;", CursorShape::Bar, 99);
        editor.yank_highlight = Some(EditorSelection {
            kind: SelectionKind::Line,
            start_line: 0,
            start_col: 0,
            end_line: 0,
            end_col: 0,
        });
        let surface = paint_via_backend(&editor);
        let theme = Theme::default();

        let (r, g, b, _) = surface.pixel(W - 4, 4);
        assert_ne!(
            (r, g, b),
            (theme.background.r, theme.background.g, theme.background.b),
            "yanked row should not read as plain background"
        );
        let expected_r = (theme.yank_highlight_bg.r as f32 * theme.yank_highlight_alpha
            + theme.background.r as f32 * (1.0 - theme.yank_highlight_alpha))
            as u8;
        let expected_g = (theme.yank_highlight_bg.g as f32 * theme.yank_highlight_alpha
            + theme.background.g as f32 * (1.0 - theme.yank_highlight_alpha))
            as u8;
        let expected_b = (theme.yank_highlight_bg.b as f32 * theme.yank_highlight_alpha
            + theme.background.b as f32 * (1.0 - theme.yank_highlight_alpha))
            as u8;
        assert!(
            approx(r, expected_r, 4) && approx(g, expected_g, 4) && approx(b, expected_b, 4),
            "expected ~({expected_r}, {expected_g}, {expected_b}), got ({r}, {g}, {b})"
        );
    }

    /// A `Char`-kind selection must only highlight the selected column
    /// range — columns outside it stay plain background.
    #[test]
    fn char_selection_highlights_only_selected_columns() {
        let mut editor = editor_with_cursor("alpha beta", CursorShape::Bar, 99);
        editor.selection = Some(EditorSelection {
            kind: SelectionKind::Char,
            start_line: 0,
            start_col: 0,
            end_line: 0,
            end_col: 2, // inclusive -> covers chars 0..=2, "alp"
        });
        let surface = paint_via_backend(&editor);
        let theme = Theme::default();
        let metrics = font_metrics(&font());
        let text_start = 3.0 * metrics.char_width; // gutter width

        // Inside the selected range (char index 1, "l").
        let in_px = (text_start + metrics.char_width * 1.5) as u32;
        let (r, g, b, _) = surface.pixel(in_px, 0);
        assert_ne!(
            (r, g, b),
            (theme.background.r, theme.background.g, theme.background.b),
            "column inside selection should be highlighted"
        );

        // Outside the selected range (char index 6, "e" of "beta").
        let out_px = (text_start + metrics.char_width * 6.5) as u32;
        let (r2, g2, b2, _) = surface.pixel(out_px, 0);
        assert_eq!(
            (r2, g2, b2),
            (theme.background.r, theme.background.g, theme.background.b),
            "column outside selection should stay plain background"
        );
    }

    /// Issue #943: diff backgrounds (`DiffLine::Added` /
    /// `Removed` / `Padding`) were never painted on macOS. Also
    /// verifies diff status takes priority over cursorline, matching
    /// `crate::gtk::editor::draw_editor`'s priority order.
    #[test]
    fn diff_added_background_overrides_cursorline() {
        let mut line = one_line("added line");
        line.diff_status = Some(DiffLine::Added);
        let mut editor = editor_with_cursor("added line", CursorShape::Bar, 99);
        editor.lines = vec![line];
        editor.cursorline = true; // would paint cursorline_bg if diff didn't win
        let surface = paint_via_backend(&editor);
        let theme = Theme::default();

        let (r, g, b, _) = surface.pixel(W - 4, 4);
        assert_eq!(
            (r, g, b),
            (
                theme.diff_added_bg.r,
                theme.diff_added_bg.g,
                theme.diff_added_bg.b
            ),
            "diff-added row should paint theme.diff_added_bg, not cursorline_bg"
        );
    }
}
