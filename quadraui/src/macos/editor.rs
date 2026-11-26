//! macOS entry points for [`crate::primitives::editor::Editor`] — thin
//! delegates onto the widget's own pixel painter,
//! `crate::primitives::editor::paint_pixels`, which documents every paint
//! pass. These functions only adapt a bare `CGContextRef` and the editor
//! `CTFont` into the [`super::surface::CgSurface`] that painter takes,
//! laying the editor out at [`Editor::rect`] and clipping to it. They
//! return a default [`EditorPaintResult`]: macOS paints its own caret.

use core_graphics::sys::CGContextRef;
use core_text::font::CTFont;

use super::surface::CgSurface;
use crate::backend::EditorPaintResult;
use crate::paint_surface::PaintSurface;
use crate::primitives::editor::{paint_pixels, Editor, EditorPaintOptions};
use crate::theme::Theme;

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
/// width in points — see [`Editor::layout_with_options_and_v_scrollbar_w`]
/// for the geometry. `None` sizes the column at `char_width`.
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
    let rect = editor.rect;
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return EditorPaintResult::default();
    }
    let geom = editor.layout_with_options_and_v_scrollbar_w(
        rect,
        char_width as f32,
        line_height as f32,
        options,
        v_scrollbar_w,
    );
    let mut surface = CgSurface {
        ctx,
        font: Some(font),
    };
    surface.surface_push_clip(rect);
    paint_pixels::paint_with_options(editor, &geom, &mut surface, theme, options);
    surface.surface_pop_clip();
    EditorPaintResult::default()
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
    use crate::primitives::editor::{CursorShape, DiffLine, EditorSelection, SelectionKind};
    use crate::theme::Theme;
    use crate::types::Color;
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
            paint_options: Default::default(),
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

    /// The block caret is a translucent `theme.cursor` cell at
    /// `cursor_normal_alpha` over the glyph — probed on a space so no ink
    /// shifts the colour.
    #[test]
    fn block_cursor_paints_translucent_theme_cursor_cell() {
        let editor = editor_with_cursor("let x = 1;", CursorShape::Block, 3);
        let surface = paint_via_backend(&editor);
        let theme = Theme::default();
        let metrics = font_metrics(&font());
        // Gutter is 3 char widths; column 3 is the space after "let".
        let cx = (6.0 * metrics.char_width) as u32 + 1;
        let cy = (metrics.line_height / 2.0) as u32;
        let (r, g, b, _) = surface.pixel(cx, cy);
        let expect = theme
            .background
            .blend(theme.cursor, theme.cursor_normal_alpha as f64);
        assert!(
            approx(r, expect.r, 3) && approx(g, expect.g, 3) && approx(b, expect.b, 3),
            "block cursor pixel at ({cx}, {cy}) should be theme.cursor at cursor_normal_alpha \
             over the background: got {:?}, expected {:?}",
            (r, g, b),
            (expect.r, expect.g, expect.b)
        );
    }

    /// The shared painter brings the GTK-only features to macOS: a
    /// secondary caret and an indent guide now paint.
    #[test]
    fn secondary_caret_and_indent_guide_paint() {
        let mut editor = editor_with_cursor("let x = 1;", CursorShape::Bar, 0);
        editor.extra_cursors = vec![CursorPos {
            view_line: 0,
            col: 3,
        }];
        editor.lines[0].indent_guides = vec![8];
        let surface = paint_via_backend(&editor);
        let theme = Theme::default();
        let metrics = font_metrics(&font());
        let cy = (metrics.line_height / 2.0) as u32;
        // One pixel in, so the 2px bar fully covers the probe.
        let caret_x = (6.0 * metrics.char_width) as u32 + 1;
        let (r, g, b, _) = surface.pixel(caret_x, cy);
        assert_eq!(
            (r, g, b),
            (theme.cursor.r, theme.cursor.g, theme.cursor.b),
            "secondary bar caret at column 3"
        );
        let guide_x = ((3.0 + 8.0) * metrics.char_width) as u32;
        let painted = (guide_x.saturating_sub(1)..=guide_x + 1).any(|x| {
            let (r, g, b, _) = surface.pixel(x, cy);
            (r, g, b) != (theme.background.r, theme.background.g, theme.background.b)
        });
        assert!(painted, "indent guide at column 8");
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

    /// `StyledSpan` byte offsets must be snapped to char boundaries.
    /// A span landing mid-multibyte-character (é / CJK / emoji)
    /// could panic `&line.raw_text[..start]` / `[start..end]`.
    /// This test exercises multibyte spans and a multibyte cursor
    /// column via `char_byte_offset`.
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
            paint_options: Default::default(),
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

    /// `editor.selection` must be read by the rasteriser.
    /// A `Line`-kind selection should paint `theme.selection`
    /// alpha-blended over the background, not the plain background.
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

    /// `yank_highlight` on `Editor` must be read by the rasteriser.
    /// The yank overlay paints its own themed colour,
    /// distinct from a plain `editor.selection` overlay.
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

    /// Diff backgrounds (`DiffLine::Added` / `Removed` / `Padding`)
    /// must be painted by the rasteriser. Diff status takes priority
    /// over cursorline, matching `crate::gtk::editor::draw_editor`'s
    /// priority order.
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

    /// Scan `surface` for the vertical extent of non-background ink
    /// inside `[x_start, x_end) × [y_start, y_end)` — the smallest `top`
    /// and largest `bottom` row (inclusive) where any pixel in the
    /// column range differs from `bg` by more than a small
    /// anti-aliasing tolerance. `None` if the window contains no ink.
    fn ink_vertical_extent(
        surface: &BitmapSurface,
        x_start: u32,
        x_end: u32,
        y_start: u32,
        y_end: u32,
        bg: Color,
    ) -> Option<(u32, u32)> {
        const TOL: i32 = 8;
        let mut extent: Option<(u32, u32)> = None;
        for y in y_start..y_end {
            let has_ink = (x_start..x_end).any(|x| {
                let (r, g, b, _) = surface.pixel(x, y);
                (r as i32 - bg.r as i32).abs() > TOL
                    || (g as i32 - bg.g as i32).abs() > TOL
                    || (b as i32 - bg.b as i32).abs() > TOL
            });
            if has_ink {
                extent = Some(match extent {
                    Some((top, _)) => (top, y),
                    None => (y, y),
                });
            }
        }
        extent
    }

    /// The editor's row pitch ([`Backend::set_current_line_height`]) is
    /// a host decision, independent of the font's own natural line
    /// height: a 12pt Menlo's ~14px of ascent + descent + leading inside
    /// a VS Code-style 18px row leaves ~4px of slack. Both halves of the
    /// contract that slack is split *evenly* are pinned here:
    ///
    /// - Widening the pitch from the font's natural height to 18px moves
    ///   the body glyph's ink down by half the slack that added — ink
    ///   anchored to the row's top edge instead wouldn't move at all.
    /// - At 18px the gutter number and the body text — the identical
    ///   glyph, `"7"`, hence identical ink — occupy exactly the same
    ///   rows, i.e. they share a baseline.
    ///
    /// Asserting on the *shift* rather than on the ink's absolute
    /// midpoint is deliberate: a digit has no descender, so its ink sits
    /// in the upper part of its glyph cell and its own midpoint is
    /// legitimately above the row's, by a glyph-shape-dependent amount
    /// no centring rule should be read as promising.
    #[test]
    fn body_text_centred_in_row_and_shares_baseline_with_gutter() {
        const ROW_PITCH: f64 = 18.0;
        let metrics = font_metrics(&make_font("Menlo", 12.0).expect("Menlo installed"));
        let natural_pitch = metrics.line_height;
        assert!(
            natural_pitch < ROW_PITCH,
            "fixture needs a font natural line height ({natural_pitch}) shorter \
             than the row pitch ({ROW_PITCH}) to leave any slack to centre in",
        );

        let theme = Theme::default();
        let gutter_chars = 2usize;
        let paint_at_pitch = |pitch: f64| {
            let mut line = one_line("7");
            line.gutter_text = "7".into();
            let mut editor = editor_with_cursor("7", CursorShape::Bar, 99);
            editor.cursor = None; // isolate glyph ink from any cursor paint
            editor.lines = vec![line];
            editor.gutter_char_width = gutter_chars;

            let surface = BitmapSurface::new(W, H);
            surface.fill(
                theme.background.r as f64 / 255.0,
                theme.background.g as f64 / 255.0,
                theme.background.b as f64 / 255.0,
                1.0,
            );
            let mut backend = MacBackend::new();
            backend.set_current_font(make_font("Menlo", 12.0).expect("Menlo installed"));
            backend.set_current_line_height(pitch);
            backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
            backend.enter_frame_scope(surface.context_ptr(), |b| {
                b.draw_editor(editor.rect, &editor);
            });
            backend.end_frame();
            surface
        };

        let char_w = metrics.char_width;
        let gutter_w = gutter_chars as f64 * char_w;
        let body_ink = |surface: &BitmapSurface, pitch: f64| {
            ink_vertical_extent(
                surface,
                gutter_w.round() as u32,
                (gutter_w + char_w).round() as u32,
                0,
                pitch.ceil() as u32,
                theme.background,
            )
            .expect("body glyph should paint some ink")
        };

        let natural = body_ink(&paint_at_pitch(natural_pitch), natural_pitch);
        let wide_surface = paint_at_pitch(ROW_PITCH);
        let wide = body_ink(&wide_surface, ROW_PITCH);

        let expected_shift = (ROW_PITCH - natural_pitch) / 2.0;
        let top_shift = wide.0 as f64 - natural.0 as f64;
        let bottom_shift = wide.1 as f64 - natural.1 as f64;
        assert!(
            (top_shift - expected_shift).abs() <= 1.0
                && (bottom_shift - expected_shift).abs() <= 1.0,
            "widening the row pitch from {natural_pitch} to {ROW_PITCH} should move \
             the body glyph down by half the added slack ({expected_shift}px), got \
             top {top_shift}px / bottom {bottom_shift}px (ink {natural:?} -> {wide:?})"
        );

        let gutter = ink_vertical_extent(
            &wide_surface,
            0,
            gutter_w.round() as u32,
            0,
            ROW_PITCH as u32,
            theme.background,
        )
        .expect("gutter glyph should paint some ink");
        assert_eq!(
            gutter, wide,
            "gutter number and body text (same glyph, \"7\") should share \
             a baseline, got gutter={gutter:?}, body={wide:?}"
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
            paint_options: Default::default(),
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

    /// Core Graphics anti-aliases a fill's boundary, bleeding a level or
    /// two of track colour into the neighbouring text-area pixel, so an
    /// exact `== background` on a pixel adjacent to the column edge is
    /// not a stable assertion across hosts. The scrollbar track sits
    /// ~20 levels per channel clear of the background in every shipped
    /// theme, far outside this tolerance, so "background" and "track"
    /// stay unambiguous.
    const SCROLL_TEST_AA_TOLERANCE: i32 = 4;

    /// Whether the pixel at `(x, y)` is the editor background rather
    /// than scrollbar track, tolerant of edge anti-aliasing.
    fn scroll_test_pixel_is_bg(surface: &BitmapSurface, x: u32, y: u32) -> bool {
        let bg = Theme::default().background;
        let (r, g, b, _) = surface.pixel(x, y);
        (r as i32 - bg.r as i32).abs() <= SCROLL_TEST_AA_TOLERANCE
            && (g as i32 - bg.g as i32).abs() <= SCROLL_TEST_AA_TOLERANCE
            && (b as i32 - bg.b as i32).abs() <= SCROLL_TEST_AA_TOLERANCE
    }

    /// `Backend::set_editor_v_scrollbar_width(Some(14.0))` makes
    /// `MacBackend::draw_editor` paint a scrollbar column exactly 14px
    /// wide — `[186, 200)` in a 200px viewport — regardless of the
    /// font's own char width, and `Backend::editor_layout` reports the
    /// same column for hit-testing.
    #[test]
    fn backend_set_editor_v_scrollbar_width_paints_14px_column() {
        let overflowing = scroll_test_editor(50, 5);
        let surface = scroll_test_paint_via_backend(&overflowing, Some(14.0));
        let y = SCROLL_TEST_H - 4;
        for x in 186..SCROLL_TEST_W {
            assert!(
                !scroll_test_pixel_is_bg(&surface, x, y),
                "x={x} lies inside the 14px scrollbar column [186, 200) and should be tinted"
            );
        }
        // Three pixels clear of the column edge at x=186, so edge
        // anti-aliasing cannot reach the probe.
        assert!(
            scroll_test_pixel_is_bg(&surface, 183, y),
            "x=183 lies left of the 14px scrollbar column and should be plain background"
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

    /// Without `set_editor_v_scrollbar_width`, the backend path keeps a
    /// column one `char_width` wide: narrower than the 14px override, so
    /// the pixels the override would have tinted stay background.
    #[test]
    fn backend_default_editor_v_scrollbar_width_is_one_char() {
        let overflowing = scroll_test_editor(50, 5);

        // The default column is `char_width` wide, which the real font
        // supplies — derive the boundary from the backend rather than
        // assuming a glyph advance.
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        assert_eq!(backend.editor_v_scrollbar_width(), None);
        let char_w = backend.char_width();
        assert!(
            char_w > 0.0 && char_w < 14.0,
            "fixture needs a default column narrower than the 14px override, got {char_w}"
        );

        let surface = scroll_test_paint_via_backend(&overflowing, None);
        let y = SCROLL_TEST_H - 4;
        // Three pixels clear of the column's left edge, inside what the
        // 14px override would have tinted.
        let text_area_x = (SCROLL_TEST_W as f32 - char_w).floor() as u32 - 3;
        assert!(
            scroll_test_pixel_is_bg(&surface, text_area_x, y),
            "default column is {char_w}px wide, so x={text_area_x} is text-area background"
        );
        assert!(
            !scroll_test_pixel_is_bg(&surface, SCROLL_TEST_W - 2, y),
            "the default column still paints at the right edge"
        );
    }

    /// A host can hand `draw_editor` an `editor.rect` wider than the
    /// real window (e.g. a rounding/DPI mismatch upstream of this
    /// backend). `draw_editor` must still paint the scrollbar track fully
    /// inside the real viewport rather than laying it out past the right
    /// edge, where only a sliver (or nothing at all) would be visible.
    ///
    /// `editor.rect` here is 20px wider than the real 200x80 viewport
    /// `scroll_test_paint_via_backend` paints into. Without the clamp,
    /// the 14px column would land at `x = 0 + 220 - 14 = 206` —
    /// entirely outside the painted surface's `[0, 200)`. The clamp in
    /// `draw_editor` shrinks `editor.rect` back inside `self.viewport`,
    /// so the column lands at `[186, 200)` — the same span the in-bounds
    /// `_paints_14px_column` test above asserts.
    ///
    /// This only covers the paint path. `Backend::editor_layout` (the
    /// no-paint hit-test twin, `src/backend.rs`) is shared, trait-default
    /// code and is not overridden here — its own doc documents that
    /// passing it a `rect` which disagrees with what was actually
    /// painted is an uncovered caller invariant.
    #[test]
    fn backend_clamps_oversized_editor_rect_to_viewport() {
        let mut oversized = scroll_test_editor(50, 5);
        oversized.rect.width = SCROLL_TEST_W as f32 + 20.0;

        let surface = scroll_test_paint_via_backend(&oversized, Some(14.0));
        let y = SCROLL_TEST_H - 4;
        for x in 186..SCROLL_TEST_W {
            assert!(
                !scroll_test_pixel_is_bg(&surface, x, y),
                "x={x} lies inside the 14px scrollbar column [186, 200) and should be \
                 tinted — editor.rect ({}px) is wider than the real 200px viewport, so an \
                 unclamped draw_editor would have laid this column out past the window edge",
                oversized.rect.width
            );
        }
        assert!(
            scroll_test_pixel_is_bg(&surface, 183, y),
            "x=183 lies left of the 14px scrollbar column and should be plain background"
        );
    }
}
