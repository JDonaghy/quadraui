//! Win-GUI entry points for [`crate::primitives::editor::Editor`] — thin
//! delegates onto the widget's own pixel painter,
//! `crate::primitives::editor::paint_pixels`, which documents every paint
//! pass. These functions only adapt a raw `ID2D1RenderTarget` and the
//! editor [`DWrite`] handle into the [`super::surface::D2dSurface`] that
//! painter takes, laying the editor out at [`Editor::rect`].
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod editor;`.
//!
//! `EditorPaintResult::cursor_position_native` is always `None`: this
//! backend paints its own caret.
//!
//! ## Scrollbar
//!
//! [`draw_editor_with_options_and_v_scrollbar_w`] reserves the vertical
//! scrollbar column (the text clip stops short of it) but leaves it
//! unpainted: `WinBackend::draw_editor` paints that column itself, from
//! the same layout, through `Backend::draw_scrollbar`. The horizontal
//! scrollbar is painted here.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::surface::D2dSurface;
use super::text::DWrite;
use crate::backend::EditorPaintResult;
use crate::primitives::editor::{paint_pixels, Editor, EditorPaintOptions};
use crate::theme::Theme;

/// Draw an [`Editor`] viewport (`editor.rect`) on `target`, at
/// `cell_width` / `line_height` (DIPs).
///
/// Equivalent to [`draw_editor_with_options_and_v_scrollbar_w`] with
/// `EditorPaintOptions::default()` and no scrollbar-width override.
pub fn draw_editor(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    editor: &Editor,
    cell_width: f32,
    line_height: f32,
    theme: &Theme,
) -> EditorPaintResult {
    draw_editor_with_options_and_v_scrollbar_w(
        target,
        dwrite,
        editor,
        cell_width,
        line_height,
        theme,
        EditorPaintOptions::default(),
        None,
    )
}

/// [`draw_editor`], plus [`EditorPaintOptions`] and a host-settable
/// vertical scrollbar width in DIPs (`None` = one `cell_width`). Both
/// feed [`Editor::layout_with_options_and_v_scrollbar_w`]; see the module
/// doc's "Scrollbar" section for who paints the vertical column.
#[allow(clippy::too_many_arguments)]
pub fn draw_editor_with_options_and_v_scrollbar_w(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    editor: &Editor,
    cell_width: f32,
    line_height: f32,
    theme: &Theme,
    options: EditorPaintOptions,
    v_scrollbar_w: Option<f32>,
) -> EditorPaintResult {
    let geom = editor.layout_with_options_and_v_scrollbar_w(
        editor.rect,
        cell_width,
        line_height,
        options,
        v_scrollbar_w,
    );
    let mut surface = D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
    let paint_options = EditorPaintOptions {
        suppress_v_scrollbar: true,
        ..options
    };
    paint_pixels::paint_with_options(editor, &geom, &mut surface, theme, paint_options);
    EditorPaintResult::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Rect as QRect;
    use crate::primitives::editor::{
        CursorPos, CursorShape, DiagnosticMark, DiagnosticSeverity, DiffLine, EditorCursor,
        EditorLine, EditorSelection, EditorStyledSpan, SelectionKind, Style,
    };
    use crate::types::Color;
    use crate::types::WidgetId;
    use crate::win::testing::HeadlessSurface;

    const CELL_W: f32 = 8.0;
    const LINE_H: f32 = 16.0;

    /// Whether `px` is within 2 of `c` on every channel — a translucent
    /// fill's alpha is quantised to 8 bits before Direct2D blends it.
    fn near(px: Color, c: Color) -> bool {
        [(px.r, c.r), (px.g, c.g), (px.b, c.b)]
            .iter()
            .all(|&(a, b)| (a as i32 - b as i32).abs() <= 2)
    }

    fn plain_line(idx: usize, text: &str) -> EditorLine {
        EditorLine {
            raw_text: text.to_string(),
            gutter_text: format!("{:>3}", idx + 1),
            spans: vec![EditorStyledSpan {
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
            paint_options: Default::default(),
        }
    }

    /// The `v_scrollbar_w` override narrows `text_bounds` by exactly
    /// its delta from `cell_width` — the layout
    /// `draw_editor_with_options_and_v_scrollbar_w` clips text to. Pure
    /// layout, no `HeadlessSurface`/`DWrite` needed.
    #[test]
    fn v_scrollbar_w_option_narrows_text_bounds() {
        let mut e = editor(vec![plain_line(0, "line one")]);
        e.total_lines = 50; // overflow: far more than the viewport's rows
        let rect = e.rect;

        let baseline = e.layout(rect, CELL_W, LINE_H);
        let baseline_vsb = baseline
            .v_scrollbar_bounds
            .expect("overflowing buffer reserves a vertical scrollbar column");
        assert_eq!(baseline_vsb.width, CELL_W);

        let widened = e.layout_with_options_and_v_scrollbar_w(
            rect,
            CELL_W,
            LINE_H,
            EditorPaintOptions::default(),
            Some(14.0),
        );
        let widened_vsb = widened
            .v_scrollbar_bounds
            .expect("still overflowing with the override applied");
        assert_eq!(widened_vsb.width, 14.0);
        assert_eq!(
            widened.text_bounds.width,
            baseline.text_bounds.width - (14.0 - CELL_W)
        );
    }

    /// Painting must not panic across a mix of plain lines, a cursor, a
    /// selection, and a diagnostic underline, and the gutter's line
    /// number must be visible pixels distinct from the background.
    #[test]
    fn paints_without_panicking_and_gutter_is_visible() {
        let surface = HeadlessSurface::new(200, 100).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Consolas", 10.0, None).expect("create DWrite");
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
                draw_editor(target, &dwrite, &e, CELL_W, LINE_H, &Theme::default());
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
        let (dwrite, _, _) = DWrite::new("Consolas", 10.0, None).expect("create DWrite");
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
                draw_editor(target, &dwrite, &e, CELL_W, LINE_H, &Theme::default());
            })
            .expect("paint editor");

        let theme = Theme::default();
        let text_x = (e.gutter_char_width as f32 * CELL_W) as u32;
        // Sample near the right edge of the viewport, well past
        // `end_col`'s column — a `Line` selection should still cover it.
        let sample_x = 180u32.max(text_x + 5);
        let px = surface.pixel_at(sample_x, 4);
        let expected = theme
            .background
            .blend(theme.selection, theme.selection_alpha as f64);
        assert!(near(px, expected), "{px:?} vs {expected:?}");
    }

    /// `EditorSelection::cols_on` (shared with every other backend) correctly
    /// computes selection columns, accounting for wrapped ghosts, padding rows,
    /// and segment offsets. This proves it through a real Direct2D paint.
    #[test]
    fn selection_respects_wrapped_ghost_and_diff_padding_rows() {
        let surface = HeadlessSurface::new(200, 100).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Consolas", 10.0, None).expect("create DWrite");

        let lines = vec![
            // Row 0: buffer line 0, plain, selection's own start line.
            // Visible glyphs only in the first two columns, spaces after
            // — the sample column below lands on inkless space cells so
            // the pixel is pure selection tint, not DirectWrite glyph
            // antialiasing (same trick as `line_selection_spans_full_row`).
            plain_line(0, &format!("01{}", " ".repeat(8))),
            // Row 1: a wrap-continuation segment of buffer line 1,
            // starting at buffer column 5 — its own text (10 chars,
            // same glyph-then-spaces shape) is entirely inside the
            // selection.
            EditorLine {
                segment_col_offset: 5,
                is_wrap_continuation: true,
                ..plain_line(1, &format!("ab{}", " ".repeat(8)))
            },
            // Row 2: the AI-ghost continuation of buffer line 1 — same
            // `line_idx` as row 1, but virtual, must never be selected.
            EditorLine {
                is_ghost_continuation: true,
                // Two glyphs, so the sample column stays inkless.
                ghost_suffix: Some("ai".into()),
                ..plain_line(1, "")
            },
            // Row 3: buffer line 2 (the selection's own end line), but a
            // diff-padding filler row — must never be selected either.
            EditorLine {
                diff_status: Some(DiffLine::Padding),
                ..plain_line(2, "")
            },
        ];
        let mut e = editor(lines);
        e.total_lines = 4;
        e.selection = Some(EditorSelection {
            kind: SelectionKind::Char,
            start_line: 0,
            start_col: 0,
            end_line: 2,
            end_col: 10,
        });

        surface
            .paint(|target| {
                draw_editor(target, &dwrite, &e, CELL_W, LINE_H, &Theme::default());
            })
            .expect("paint editor");

        let theme = Theme::default();
        let blended = theme
            .background
            .blend(theme.selection, theme.selection_alpha as f64);
        let text_x = (e.gutter_char_width as f32 * CELL_W) as i32;
        // Middle of column 5: past the two visible glyphs, inside
        // every tinted row's selected range.
        let sample_x = (text_x as f32 + 5.0 * CELL_W + CELL_W / 2.0) as u32;
        let row_y = |row: i32| (row as f32 * LINE_H + LINE_H / 2.0) as u32;

        let row0 = surface.pixel_at(sample_x, row_y(0));
        assert!(
            near(row0, blended),
            "row 0 (plain, selection's start line) should be tinted: {row0:?}"
        );
        let row1 = surface.pixel_at(sample_x, row_y(1));
        assert!(
            near(row1, blended),
            "row 1 (wrapped segment of a fully-selected line) should be tinted: {row1:?}"
        );
        let row2 = surface.pixel_at(sample_x, row_y(2));
        assert_eq!(
            (row2.r, row2.g, row2.b),
            (theme.background.r, theme.background.g, theme.background.b),
            "row 2 (ghost continuation) must never be tinted by a selection"
        );
        // The diff-padding row shows its own diff background, untinted.
        let row3 = surface.pixel_at(sample_x, row_y(3));
        assert_eq!(
            (row3.r, row3.g, row3.b),
            (
                theme.diff_padding_bg.r,
                theme.diff_padding_bg.g,
                theme.diff_padding_bg.b
            ),
            "row 3 (diff padding) must never be tinted by a selection"
        );
    }

    /// A block caret is a translucent `theme.cursor` cell over the glyph:
    /// the cell's inkless pixels carry the tint, and the glyph's ink still
    /// shows through it.
    #[test]
    fn block_cursor_tints_its_cell_and_keeps_the_glyph_visible() {
        let surface = HeadlessSurface::new(200, 100).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Consolas", 10.0, None).expect("create DWrite");
        let lines = vec![plain_line(0, "1789518172522")];
        let mut e = editor(lines);
        e.cursor = Some(EditorCursor {
            pos: CursorPos {
                view_line: 0,
                col: 0,
            },
            shape: CursorShape::Block,
        });

        surface
            .paint(|target| {
                draw_editor(target, &dwrite, &e, CELL_W, LINE_H, &Theme::default());
            })
            .expect("paint editor");

        let theme = Theme::default();
        let tint = theme
            .background
            .blend(theme.cursor, theme.cursor_normal_alpha as f64);
        let text_x = (e.gutter_char_width as f32 * CELL_W) as u32;
        let cell: Vec<Color> = (text_x + 1..text_x + 6)
            .flat_map(|x| (1..LINE_H as u32 - 1).map(move |y| (x, y)))
            .map(|(x, y)| surface.pixel_at(x, y))
            .collect();
        assert!(
            cell.iter().any(|&px| near(px, tint)),
            "the caret cell should carry the translucent cursor tint {tint:?}"
        );
        assert!(
            cell.iter().any(|&px| !near(px, tint)),
            "the glyph's ink should still show through the caret"
        );
    }

    /// The editor's row pitch is a host decision independent of
    /// `dwrite`'s own natural line height — mirrors
    /// `macos::editor::tests::body_text_centred_in_row_and_shares_baseline_with_gutter`
    /// and `primitives::editor::paint_pixels`'s GTK `draw_editor_body_text_centred_in_row_and_shares_baseline_with_gutter`:
    /// widening the pitch from the font's natural height to a VS
    /// Code-style wider row should move the body glyph's ink down by
    /// half the added slack, and the gutter number (same glyph, `"7"`)
    /// should land on the same rows as the body text once widened.
    #[test]
    fn body_text_centred_in_row_and_shares_baseline_with_gutter() {
        let (dwrite, natural_line_height, _) =
            DWrite::new("Consolas", 10.0, None).expect("create DWrite");
        const ROW_PITCH: f32 = 24.0;
        assert!(
            natural_line_height < ROW_PITCH,
            "fixture needs a font natural line height ({natural_line_height}) shorter than \
             the row pitch ({ROW_PITCH}) to leave any slack to centre in",
        );

        let gutter_chars = 2usize;
        let gutter_w = gutter_chars as f32 * CELL_W;
        let theme = Theme::default();

        let paint_at_pitch = |pitch: f32| -> HeadlessSurface {
            let surface =
                HeadlessSurface::new(200, pitch.ceil() as u32 + 8).expect("create surface");
            let mut line = plain_line(0, "7");
            line.gutter_text = "7".into();
            let mut e = editor(vec![line]);
            e.gutter_char_width = gutter_chars;
            e.rect = QRect::new(0.0, 0.0, 200.0, pitch + 8.0);
            surface
                .paint(|target| {
                    draw_editor(target, &dwrite, &e, CELL_W, pitch, &theme);
                })
                .expect("paint editor");
            surface
        };

        // Scan `surface` for the vertical extent of non-background ink
        // inside `[x_start, x_end) × [0, y_end)` — Win-GUI's counterpart
        // to the macOS/GTK twins' identically-named test helpers.
        let ink_extent = |surface: &HeadlessSurface, x_start: u32, x_end: u32, y_end: u32| {
            const TOL: i32 = 24;
            let mut extent: Option<(u32, u32)> = None;
            for y in 0..y_end {
                let has_ink = (x_start..x_end).any(|x| {
                    let px = surface.pixel_at(x, y);
                    (px.r as i32 - theme.background.r as i32).abs() > TOL
                        || (px.g as i32 - theme.background.g as i32).abs() > TOL
                        || (px.b as i32 - theme.background.b as i32).abs() > TOL
                });
                if has_ink {
                    extent = Some(match extent {
                        Some((top, _)) => (top, y),
                        None => (y, y),
                    });
                }
            }
            extent
        };

        let natural_surface = paint_at_pitch(natural_line_height);
        let natural = ink_extent(
            &natural_surface,
            gutter_w.round() as u32,
            (gutter_w + CELL_W).round() as u32,
            natural_line_height.ceil() as u32 + 8,
        )
        .expect("body glyph should paint some ink at the natural pitch");

        let wide_surface = paint_at_pitch(ROW_PITCH);
        let wide = ink_extent(
            &wide_surface,
            gutter_w.round() as u32,
            (gutter_w + CELL_W).round() as u32,
            ROW_PITCH as u32 + 8,
        )
        .expect("body glyph should paint some ink at the wide pitch");

        let expected_shift = (ROW_PITCH - natural_line_height) / 2.0;
        let top_shift = wide.0 as f32 - natural.0 as f32;
        let bottom_shift = wide.1 as f32 - natural.1 as f32;
        assert!(
            (top_shift - expected_shift).abs() <= 2.0
                && (bottom_shift - expected_shift).abs() <= 2.0,
            "widening the row pitch from {natural_line_height} to {ROW_PITCH} should move \
             the body glyph down by half the added slack ({expected_shift}px), got top \
             {top_shift}px / bottom {bottom_shift}px (ink {natural:?} -> {wide:?})"
        );

        let gutter = ink_extent(
            &wide_surface,
            0,
            gutter_w.round() as u32,
            ROW_PITCH as u32 + 8,
        )
        .expect("gutter glyph should paint some ink");
        assert_eq!(
            gutter, wide,
            "gutter number and body text (same glyph, \"7\") should share a baseline, got \
             gutter={gutter:?}, body={wide:?}"
        );
    }
}
