//! macOS rasteriser for [`crate::primitives::editor::Editor`].
//!
//! Port of `crate::gtk::editor::draw_editor`: background (incl. DAP
//! stopped-line / diff / cursorline priority), per-line text via Core
//! Text, selection overlays (`selection`, `extra_selections`,
//! `yank_highlight`), line-number gutter, vertical scrollbar, and the
//! primary cursor (Block / Bar / Underline). Returns a default
//! [`EditorPaintResult`] — macOS, like GTK, paints its own caret rather
//! than delegating to a terminal cursor.
//!
//! ## Scrollbar
//!
//! `Editor::layout`'s `v_scrollbar_bounds` reserves the column (so
//! `EditorLayout::hit_test` and the content clip below already know
//! about it) and [`draw_editor_with_options_and_v_scrollbar_w`] paints
//! into it, mirroring `gtk::editor::draw_editor_with_options`'s "Scrollbars"
//! section: geometry comes from
//! [`Editor::layout_with_options_and_v_scrollbar_w`] (the same layout
//! hit-testing uses), the text clip is narrowed to
//! `text_bounds` so no glyph paints under the reserved column, and the
//! column itself is painted afterwards through the shared
//! [`crate::primitives::scrollbar::native_surface_paint::paint`] via the
//! [`super::surface::CgSurface`] adapter — the same pattern
//! `macos::data_table` uses to paint an embedded scrollbar from a bare
//! `ctx: CGContextRef`. The `v_scrollbar_w` argument lets a host
//! override the column's width (e.g. VS Code's fixed 14px, set via
//! [`crate::Backend::set_editor_v_scrollbar_width`]) instead of the
//! `cell_width` default; [`EditorPaintOptions::suppress_v_scrollbar`]
//! continues to opt out entirely for a `Minimap`-as-scrollbar host, same
//! as GTK.
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

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::cg::*;
use super::surface::CgSurface;
use super::text::{draw_text, measure_text};
use crate::backend::EditorPaintResult;
use crate::primitives::editor::{
    CursorShape, DiffLine, Editor, EditorLine, EditorPaintOptions, EditorSelection, SelectionKind,
};
use crate::primitives::scrollbar::{native_surface_paint, Scrollbar};
use crate::text_util::snap_to_char_boundary;
use crate::theme::Theme;
use crate::types::Color;

/// Paint `editor` onto `ctx`. Returns the default
/// [`EditorPaintResult`] — macOS paints its own caret.
///
/// Equivalent to [`draw_editor_with_options`] with
/// `EditorPaintOptions::default()` — kept as a separate, unchanged
/// function (rather than growing this one's argument list) so every
/// existing caller keeps compiling untouched.
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
    draw_editor_with_options(
        ctx,
        font,
        editor,
        theme,
        char_width,
        line_height,
        EditorPaintOptions::default(),
    )
}

/// [`draw_editor`], plus [`EditorPaintOptions`] a host can set to
/// override otherwise-automatic paint decisions — `suppress_v_scrollbar`.
/// See the module doc's "Scrollbar" section.
///
/// Equivalent to [`draw_editor_with_options_and_v_scrollbar_w`] with
/// `v_scrollbar_w: None` (a `char_width`-wide scrollbar column).
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_editor_with_options(
    ctx: CGContextRef,
    font: &CTFont,
    editor: &Editor,
    theme: &Theme,
    char_width: f64,
    line_height: f64,
    options: EditorPaintOptions,
) -> EditorPaintResult {
    draw_editor_with_options_and_v_scrollbar_w(
        ctx,
        font,
        editor,
        theme,
        char_width,
        line_height,
        options,
        None,
    )
}

/// [`draw_editor_with_options`], plus a host-settable vertical scrollbar
/// width in points — see
/// [`Editor::layout_with_options_and_v_scrollbar_w`] for the geometry
/// and the module doc's "Scrollbar" section for the paint. `None` sizes
/// the column at `char_width`. `MacBackend::draw_editor` calls this with
/// the width set through [`crate::Backend::set_editor_v_scrollbar_width`].
///
/// # Safety
///
/// `ctx` must be a valid `CGContextRef` borrowed for the duration of
/// the call.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_editor_with_options_and_v_scrollbar_w(
    ctx: CGContextRef,
    font: &CTFont,
    editor: &Editor,
    theme: &Theme,
    char_width: f64,
    line_height: f64,
    options: EditorPaintOptions,
    v_scrollbar_w: Option<f32>,
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
    CGContextClipToRect(ctx, super::cg::rect(x, y, w, h));

    let bg = if editor.show_active_bg {
        theme.editor_active_background
    } else {
        theme.background
    };
    fill_rect(ctx, x, y, w, h, bg);

    let gutter_width = editor.gutter_char_width as f64 * char_width;
    let h_scroll_offset = editor.scroll_left as f64 * char_width;
    let text_x_offset = x + gutter_width - h_scroll_offset;

    // Computed once up front, before any text paints, so the content
    // clip below can be narrowed by the same reserved scrollbar column
    // the scrollbar paint and `EditorLayout::hit_test` already agree on
    // — see the module doc's "Scrollbar" section.
    let editor_geom = editor.layout_with_options_and_v_scrollbar_w(
        *rect,
        char_width as f32,
        line_height as f32,
        options,
        v_scrollbar_w,
    );

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

    // Gutter — right-aligned line number text. Painted unclipped (it
    // sits entirely left of the text-area clip established below, so a
    // scrollbar-column clip there would never touch it anyway).
    for (view_idx, line) in editor.lines.iter().enumerate() {
        let line_y = y + view_idx as f64 * line_height;
        if line_y >= y + h {
            break;
        }
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
    }

    // ── Clip to text area (excludes gutter AND the reserved vertical
    // scrollbar column, when present) ───────────────────────────────────
    //
    // Narrowed to `editor_geom.text_bounds.width` rather than the full
    // `w - gutter_width`, mirroring `gtk::editor::draw_editor_with_options`
    // — text stops short of the scrollbar column instead of painting
    // glyphs the scrollbar then overlays.
    CGContextSaveGState(ctx);
    CGContextClipToRect(
        ctx,
        super::cg::rect(x + gutter_width, y, editor_geom.text_bounds.width as f64, h),
    );
    for (view_idx, line) in editor.lines.iter().enumerate() {
        let line_y = y + view_idx as f64 * line_height;
        if line_y >= y + h {
            break;
        }

        // Lay the line out as contiguous, non-overlapping runs (gaps in
        // `theme.foreground`, spans in their own colour) so every glyph
        // is painted exactly once — see `paint_line_text`'s doc for the
        // invariant this maintains.
        let raw_x = text_x_offset;
        paint_line_text(
            ctx,
            font,
            line,
            raw_x,
            line_y,
            line_height,
            theme.foreground,
        );
    }
    CGContextRestoreGState(ctx);

    // Vertical scrollbar — painted after text (into the reserved column
    // the clip above left untouched), before the cursor, mirroring
    // `gtk::editor::draw_editor_with_options`'s z-order (see module
    // doc's "Scrollbar" section).
    if let Some(v_track) = editor_geom.v_scrollbar_bounds {
        let sb = Scrollbar::vertical(
            "macos:editor:v_scrollbar",
            v_track,
            editor.scroll_top as f32,
            editor.total_lines as f32,
            editor_geom.visible_lines as f32,
            line_height as f32,
        );
        let mut surface = CgSurface {
            ctx,
            font: Some(font),
        };
        native_surface_paint::paint(&sb, &mut surface, theme);
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

/// Paint `line.raw_text` as a sequence of contiguous, non-overlapping
/// runs: gaps between (and around) `line.spans` in `default_fg`, each
/// span in its own `style.fg` (with `style.bg` filled first, if set).
///
/// Every glyph in the line is painted by exactly one [`draw_text`] call,
/// because the runs partition `text` with no overlapping coverage. Core
/// Text anti-aliases glyph edges with partial coverage; compositing two
/// glyph draws at the same position would turn edge alpha `a` into
/// `1-(1-a)²`, saturating the soft edge pixels and making strokes look
/// thicker and stair-stepped. Mirrors [`crate::win::editor`]'s
/// `paint_line_text`, which coalesces spans into non-overlapping runs
/// for the same reason; GTK's rasteriser holds the invariant by painting
/// the whole line in one pass via a single Pango layout with a
/// `PangoAttrList`.
///
/// Spans are expected to be sorted by `start_byte` and non-overlapping
/// (the shape every span producer — syntax highlighting, search
/// matches — emits in practice, mirroring `crate::win::editor`'s same
/// assumption). An out-of-order or overlapping span is tolerated
/// defensively: any portion already covered by an earlier run is
/// skipped rather than re-painted.
unsafe fn paint_line_text(
    ctx: CGContextRef,
    font: &CTFont,
    line: &EditorLine,
    raw_x: f64,
    line_y: f64,
    line_height: f64,
    default_fg: Color,
) {
    let text = &line.raw_text;
    if text.is_empty() {
        return;
    }

    let mut ordered_spans: Vec<(usize, usize, Color, Option<Color>)> = line
        .spans
        .iter()
        .filter_map(|span| {
            let start = snap_to_char_boundary(text, span.start_byte);
            let end = snap_to_char_boundary(text, span.end_byte);
            (start < end).then_some((start, end, span.style.fg, span.style.bg))
        })
        .collect();
    ordered_spans.sort_by_key(|&(start, _, _, _)| start);

    // Paint the run `text[from..to]` at its own x position (derived
    // from measuring the prefix `text[..from]`, same baseline the
    // original per-span code used) in `fg`, filling `bg` first when
    // present.
    let paint_run = |from: usize, to: usize, fg: Color, bg: Option<Color>| {
        if from >= to {
            return;
        }
        let (prefix_w, _) = measure_text(font, &text[..from]);
        let run_x = raw_x + prefix_w;
        let slice = &text[from..to];
        if let Some(bg) = bg {
            let (run_w, _) = measure_text(font, slice);
            fill_rect(ctx, run_x, line_y, run_w, line_height, bg);
        }
        draw_text(ctx, font, slice, run_x, line_y, color_to_cg(fg));
    };

    let mut cursor = 0usize;
    for (start, end, fg, bg) in ordered_spans {
        let start = start.max(cursor);
        if start >= end {
            // Fully consumed by an earlier (overlapping) run.
            continue;
        }
        if start > cursor {
            paint_run(cursor, start, default_fg, None);
        }
        paint_run(start, end, fg, bg);
        cursor = end;
    }
    if cursor < text.len() {
        paint_run(cursor, text.len(), default_fg, None);
    }
}

/// Byte offset corresponding to the `col`-th character of `s`. Saturates
/// at `s.len()` for out-of-range columns.
fn char_byte_offset(s: &str, col: usize) -> usize {
    s.char_indices().nth(col).map(|(b, _)| b).unwrap_or(s.len())
}

/// Paint one visual-selection overlay (`editor.selection`,
/// `extra_selections`, or `yank_highlight` — they share a shape) across
/// `lines`. Port of `crate::gtk::editor::draw_visual_selection`, using
/// Core Text glyph-run measurement in place of Pango's
/// `index_to_pos`/`pixel_size`. Column math — including
/// ghost-continuation / diff-padding row skipping and wrapped-segment
/// offsetting — comes from [`EditorSelection::cols_on`] (#1082); this
/// function only converts the returned columns to pixel positions.
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
        let Some(cols) = sel.cols_on(rl) else {
            continue;
        };
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

        let start_byte = char_byte_offset(&rl.raw_text, cols.start);
        let (prefix_w, _) = measure_text(font, &rl.raw_text[..start_byte]);
        let start_x = text_x_offset + prefix_w;

        let width = if cols.extends_beyond {
            // Selection runs past this visual segment's text — extend
            // the highlight to the end of the rendered line content
            // (matches GTK's use of the Pango layout's full pixel
            // width in this branch).
            let (line_w, _) = measure_text(font, &rl.raw_text);
            (text_x_offset + line_w - start_x).max(0.0)
        } else {
            let end_byte = char_byte_offset(&rl.raw_text, cols.end);
            let (end_w, _) = measure_text(font, &rl.raw_text[..end_byte]);
            (text_x_offset + end_w - start_x).max(0.0)
        };
        if width > 0.0 {
            fill_rect_alpha(ctx, start_x, line_y, width, line_height, color, alpha);
        }
    }
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

    /// A span covering the entire line must still paint the line's text
    /// exactly once, not as an overlapping pair of runs. Core Text's
    /// recorded [`TextRun`]s (via `start_recording_text`/
    /// `MacBackend::text_runs`) let this be asserted directly: exactly
    /// one run for the line's text, regardless of how many spans cover
    /// it.
    #[test]
    fn span_covering_whole_line_paints_exactly_one_text_run() {
        let mut line = one_line("alpha beta");
        line.spans = vec![ESpan {
            start_byte: 0,
            end_byte: line.raw_text.len(),
            style: Style {
                fg: Color::rgb(200, 100, 50),
                bg: None,
                bold: false,
                italic: false,
                font_scale: 1.0,
            },
        }];
        // `Bar` cursor at an out-of-range column: the cursor overlay
        // never draws a glyph of its own (only `Block` repaints one),
        // so the only `draw_text` calls for "alpha beta" come from the
        // line-text painter under test.
        let mut editor = editor_with_cursor("alpha beta", CursorShape::Bar, 99);
        editor.lines = vec![line];

        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.set_painted_text_recording(true);
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_editor(editor.rect, &editor);
        });
        backend.end_frame();

        let whole_line_runs: Vec<_> = backend
            .text_runs()
            .iter()
            .filter(|run| run.text == "alpha beta")
            .collect();
        assert_eq!(
            whole_line_runs.len(),
            1,
            "\"alpha beta\" should be painted exactly once (overlapping raw-run + \
             span-overlay would record it twice at the same position), got: {:?}",
            whole_line_runs,
        );
    }

    /// A span covering only part of the line must leave the uncovered
    /// gaps painted too, and each run — gap or span — must appear
    /// exactly once with no overlap. Pins both: the recorded
    /// [`TextRun`]s for `"alpha beta"` with `spans = [0..5]` must be
    /// exactly the gap `" beta"` and the span `"alpha"`, each starting
    /// where the previous one ends.
    #[test]
    fn span_covering_part_of_line_leaves_gap_runs_contiguous() {
        let mut line = one_line("alpha beta");
        line.spans = vec![ESpan {
            start_byte: 0,
            end_byte: 5,
            style: Style {
                fg: Color::rgb(200, 100, 50),
                bg: None,
                bold: false,
                italic: false,
                font_scale: 1.0,
            },
        }];
        let mut editor = editor_with_cursor("alpha beta", CursorShape::Bar, 99);
        editor.lines = vec![line];

        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.set_painted_text_recording(true);
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_editor(editor.rect, &editor);
        });
        backend.end_frame();

        let metrics = font_metrics(&font());
        let text_start = 3.0 * metrics.char_width; // gutter_char_width = 3

        let span_runs: Vec<_> = backend
            .text_runs()
            .iter()
            .filter(|run| run.text == "alpha")
            .collect();
        assert_eq!(
            span_runs.len(),
            1,
            "the span \"alpha\" should be painted exactly once, got: {:?}",
            span_runs,
        );
        assert!(
            (span_runs[0].bounds.x as f64 - text_start).abs() < 0.5,
            "the span run should start at the line's text origin, got x={}",
            span_runs[0].bounds.x,
        );

        let gap_runs: Vec<_> = backend
            .text_runs()
            .iter()
            .filter(|run| run.text == " beta")
            .collect();
        assert_eq!(
            gap_runs.len(),
            1,
            "the uncovered gap \" beta\" should still be painted exactly \
             once, got: {:?}",
            gap_runs,
        );
        let gap_x_expected = text_start + 5.0 * metrics.char_width;
        assert!(
            (gap_runs[0].bounds.x as f64 - gap_x_expected).abs() < 0.5,
            "the gap run should start right after the span ends, got x={}, expected={}",
            gap_runs[0].bounds.x,
            gap_x_expected,
        );
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

    // ── Vertical scrollbar ────────────────────────────────────────────────
    //
    // Mirrors `gtk::editor::draw_editor_with_options`'s own scroll-test
    // harness: blank lines, no gutter, explicit `char_width`/`line_height`
    // passed directly to `draw_editor_with_options` (independent of the
    // real font's own metrics, same as `font` is only used for glyph
    // measurement/paint here, not for the geometry grid) — so the only
    // thing in play is the scrollbar-column geometry itself.

    const SCROLL_TEST_W: u32 = 200;
    const SCROLL_TEST_H: u32 = 80;
    const SCROLL_TEST_CHAR_W: f64 = 8.0;
    const SCROLL_TEST_LINE_H: f64 = 16.0;

    fn blank_line(line_idx: usize) -> EditorLine {
        EditorLine {
            raw_text: String::new(),
            gutter_text: String::new(),
            spans: Vec::new(),
            line_idx,
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

    /// Editor fixture at `SCROLL_TEST_W`x`SCROLL_TEST_H` with no gutter,
    /// so the only geometry in play is the scrollbar reservation itself.
    fn scroll_test_editor(total_lines: usize, num_lines: usize) -> Editor {
        Editor {
            id: "editor:scroll".into(),
            rect: QRect::new(0.0, 0.0, SCROLL_TEST_W as f32, SCROLL_TEST_H as f32),
            lines: (0..num_lines).map(blank_line).collect(),
            cursor: None,
            extra_cursors: Vec::new(),
            selection: None,
            extra_selections: Vec::new(),
            yank_highlight: None,
            scroll_top: 0,
            scroll_left: 0,
            total_lines,
            max_col: 0,
            gutter_char_width: 0,
            is_active: true,
            show_active_bg: false,
            has_git_diff: false,
            has_breakpoints: false,
            diagnostic_gutter: HashMap::new(),
            code_action_lines: HashSet::new(),
            bracket_match_positions: Vec::new(),
            active_indent_col: None,
            tabstop: 4,
            cursorline: false,
            lightbulb_glyph: '!',
        }
    }

    /// Paint `editor` with `options` via [`draw_editor_with_options`]
    /// directly on a fresh headless surface — bypassing `MacBackend`
    /// entirely, the same way `gtk::editor`'s own scroll tests bypass
    /// `GtkBackend` (its trait-level `draw_editor` has no options
    /// parameter; see `EditorPaintOptions`'s doc for why).
    fn scroll_test_paint(editor: &Editor, options: EditorPaintOptions) -> BitmapSurface {
        scroll_test_paint_with_v_scrollbar_w(editor, options, None)
    }

    /// [`scroll_test_paint`], plus a scrollbar-width override via
    /// [`draw_editor_with_options_and_v_scrollbar_w`].
    fn scroll_test_paint_with_v_scrollbar_w(
        editor: &Editor,
        options: EditorPaintOptions,
        v_scrollbar_w: Option<f32>,
    ) -> BitmapSurface {
        let surface = BitmapSurface::new(SCROLL_TEST_W, SCROLL_TEST_H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let f = font();
        let theme = Theme::default();
        unsafe {
            draw_editor_with_options_and_v_scrollbar_w(
                surface.context_ptr(),
                &f,
                editor,
                &theme,
                SCROLL_TEST_CHAR_W,
                SCROLL_TEST_LINE_H,
                options,
                v_scrollbar_w,
            );
        }
        surface
    }

    /// A buffer taller than the viewport must tint the reserved
    /// rightmost `cell_width`-wide column; a buffer that fits must not.
    #[test]
    fn draw_editor_paints_vertical_scrollbar_when_buffer_overflows() {
        let bg = Theme::default().background;
        let overflowing = scroll_test_editor(50, 5);
        let surface = scroll_test_paint(&overflowing, EditorPaintOptions::default());
        // Solidly inside the reserved v-scrollbar column (x in [192, 200))
        // and mid-track vertically.
        let (r, g, b, _) = surface.pixel(198, 40);
        assert_ne!(
            (r, g, b),
            (bg.r, bg.g, bg.b),
            "vertical scrollbar track should tint this pixel when total_lines overflows the viewport"
        );

        let fits = scroll_test_editor(3, 3);
        let surface2 = scroll_test_paint(&fits, EditorPaintOptions::default());
        let (r2, g2, b2, _) = surface2.pixel(198, 40);
        assert_eq!(
            (r2, g2, b2),
            (bg.r, bg.g, bg.b),
            "no vertical scrollbar should paint when the buffer fits the viewport"
        );
    }

    /// `EditorPaintOptions::suppress_v_scrollbar` stops the macOS
    /// rasteriser from painting the column too, mirroring GTK's opt-out
    /// for a `Minimap`-as-scrollbar host.
    #[test]
    fn draw_editor_with_options_suppress_v_scrollbar_paints_no_column() {
        let bg = Theme::default().background;
        let overflowing = scroll_test_editor(50, 5);
        let surface = scroll_test_paint(
            &overflowing,
            EditorPaintOptions {
                suppress_v_scrollbar: true,
                ..Default::default()
            },
        );
        let (r, g, b, _) = surface.pixel(198, 40);
        assert_eq!(
            (r, g, b),
            (bg.r, bg.g, bg.b),
            "suppress_v_scrollbar should stop the vertical scrollbar from painting even though \
             total_lines overflows the viewport"
        );
    }

    /// A `v_scrollbar_w` override widens the painted column to a
    /// host-chosen pixel width (e.g. VS Code's fixed 14px) instead of
    /// `cell_width`. Viewport is `SCROLL_TEST_W`
    /// = 200, `SCROLL_TEST_CHAR_W` = 8.0: the default column spans
    /// `[192, 200)`; a 14px override spans `[186, 200)`. `x = 188` sits
    /// in the gap between those two spans — plain background at
    /// baseline, inside the track once widened.
    #[test]
    fn draw_editor_v_scrollbar_w_widens_painted_column() {
        let bg = Theme::default().background;
        let overflowing = scroll_test_editor(50, 5);

        let baseline = scroll_test_paint(&overflowing, EditorPaintOptions::default());
        let (r, g, b, _) = baseline.pixel(188, 40);
        assert_eq!(
            (r, g, b),
            (bg.r, bg.g, bg.b),
            "x=188 sits outside the default 8px-wide column, [192, 200)"
        );

        let widened = scroll_test_paint_with_v_scrollbar_w(
            &overflowing,
            EditorPaintOptions::default(),
            Some(14.0),
        );
        let (r2, g2, b2, _) = widened.pixel(188, 40);
        assert_ne!(
            (r2, g2, b2),
            (bg.r, bg.g, bg.b),
            "a Some(14.0) v_scrollbar_w should widen the painted track to cover x=188 ([186, 200))"
        );
    }

    /// Companion to the widened-track test: the same `v_scrollbar_w`
    /// override that widens the painted scrollbar also narrows the text
    /// clip by the same amount, so a full-line background span never
    /// bleeds into the wider reserved column. Uses a real line with a
    /// full-line background span rather than
    /// `scroll_test_editor`'s blank fixture, since clip narrowing is
    /// otherwise invisible with no content to clip.
    #[test]
    fn draw_editor_v_scrollbar_w_narrows_text_clip() {
        let mut editor = scroll_test_editor(50, 5);
        // Spaces, not glyphs: the background fill is what's under test,
        // not anti-aliased ink from a probe landing mid-stroke.
        let text = " ".repeat(40);
        editor.lines[2] = EditorLine {
            spans: vec![ESpan {
                start_byte: 0,
                end_byte: text.len(),
                style: Style {
                    fg: Color::rgb(255, 255, 255),
                    bg: Some(Color::rgb(10, 200, 10)),
                    bold: false,
                    italic: false,
                    font_scale: 1.0,
                },
            }],
            raw_text: text,
            ..blank_line(2)
        };

        // Baseline: the span's background fills right up to the default
        // 8px-wide column's left edge (x=192), covering x=188.
        let baseline = scroll_test_paint(&editor, EditorPaintOptions::default());
        let (r, g, b, _) = baseline.pixel(188, 40);
        assert_eq!(
            (r, g, b),
            (10, 200, 10),
            "baseline text clip should reach x=188 (span bg), just short of the 8px column"
        );

        // Widened: the text clip now stops 14px short of the right
        // edge, so the same pixel (188) falls outside the (shrunk) text
        // area — it reads as the scrollbar track tint instead.
        let widened = scroll_test_paint_with_v_scrollbar_w(
            &editor,
            EditorPaintOptions::default(),
            Some(14.0),
        );
        let (r2, g2, b2, _) = widened.pixel(188, 40);
        assert_ne!(
            (r2, g2, b2),
            (10, 200, 10),
            "a Some(14.0) v_scrollbar_w should narrow the text clip so x=188 no longer shows span bg"
        );
    }

    /// Paint `editor` through `MacBackend::draw_editor` after
    /// `set_editor_v_scrollbar_width(v_scrollbar_w)` — the path a host
    /// actually reaches, as opposed to calling the rasteriser directly.
    fn scroll_test_paint_via_backend(editor: &Editor, v_scrollbar_w: Option<f32>) -> BitmapSurface {
        let surface = BitmapSurface::new(SCROLL_TEST_W, SCROLL_TEST_H);
        surface.fill(0.0, 0.0, 0.0, 0.0);
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.set_editor_v_scrollbar_width(v_scrollbar_w);
        backend.begin_frame(Viewport::new(
            SCROLL_TEST_W as f32,
            SCROLL_TEST_H as f32,
            1.0,
        ));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_editor(editor.rect, editor);
        });
        backend.end_frame();
        surface
    }

    /// `Backend::set_editor_v_scrollbar_width(Some(14.0))` makes
    /// `MacBackend::draw_editor` paint a scrollbar column exactly 14px
    /// wide — `[186, 200)` in a 200px viewport — regardless of the
    /// font's own char width, and `Backend::editor_layout` reports the
    /// same column for hit-testing.
    #[test]
    fn backend_set_editor_v_scrollbar_width_paints_14px_column() {
        let bg = Theme::default().background;
        let overflowing = scroll_test_editor(50, 5);
        let surface = scroll_test_paint_via_backend(&overflowing, Some(14.0));
        let y = SCROLL_TEST_H - 4;
        for x in 186..SCROLL_TEST_W {
            let (r, g, b, _) = surface.pixel(x, y);
            assert_ne!(
                (r, g, b),
                (bg.r, bg.g, bg.b),
                "x={x} lies inside the 14px scrollbar column [186, 200) and should be tinted"
            );
        }
        let (r, g, b, _) = surface.pixel(185, y);
        assert_eq!(
            (r, g, b),
            (bg.r, bg.g, bg.b),
            "x=185 lies just left of the 14px scrollbar column and should be plain background"
        );

        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.set_editor_v_scrollbar_width(Some(14.0));
        assert_eq!(backend.editor_v_scrollbar_width(), Some(14.0));
        let layout = backend.editor_layout(overflowing.rect, &overflowing);
        let vsb = layout.v_scrollbar_bounds.expect("buffer overflows");
        assert_eq!(vsb.width, 14.0);
        assert_eq!(vsb.x, SCROLL_TEST_W as f32 - 14.0);
    }

    /// Without `set_editor_v_scrollbar_width`, the backend path keeps
    /// the one-char-wide column: x=186 stays background.
    #[test]
    fn backend_default_editor_v_scrollbar_width_is_one_char() {
        let bg = Theme::default().background;
        let overflowing = scroll_test_editor(50, 5);
        let surface = scroll_test_paint_via_backend(&overflowing, None);
        let (r, g, b, _) = surface.pixel(186, SCROLL_TEST_H - 4);
        assert_eq!(
            (r, g, b),
            (bg.r, bg.g, bg.b),
            "the default column is one char (~8px) wide, so x=186 is text-area background"
        );
        let (r, g, b, _) = surface.pixel(SCROLL_TEST_W - 2, SCROLL_TEST_H - 4);
        assert_ne!(
            (r, g, b),
            (bg.r, bg.g, bg.b),
            "the default column still paints at the right edge"
        );
    }
}
