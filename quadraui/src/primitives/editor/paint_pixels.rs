//! The pixel painter for [`Editor`], written against [`PaintSurface`]
//! only: text is shaped through [`PaintSurface::surface_draw_text_line`]
//! in [`FontRole::Editor`], and glyph positions come from
//! [`PaintSurface::surface_text_x_at_index`].
//!
//! Passes, back to front:
//!
//! 1. the window background, then cursorline / diff / DAP-stopped row
//!    backgrounds (stopping short of the reserved scrollbar column);
//! 2. selection overlays (primary, multi-cursor, yank flash), translucent
//!    and under the text;
//! 3. the gutter: breakpoint and git markers, the right-aligned line
//!    number, then the diagnostic dot or code-action lightbulb;
//! 4. clipped to the text area: each row's span backgrounds and text,
//!    ghost continuation, inline annotation, indent guides, colour
//!    columns, bracket match, wavy diagnostic and dotted spell
//!    underlines;
//! 5. both scrollbars, into the column the text clip kept clear;
//! 6. the primary caret (always shown, in the descriptor's
//!    [`CursorShape`]), AI ghost text after it, and secondary carets.
//!
//! Every text paint in a row is centred in the row pitch: it sits half
//! the gap between [`EditorLayout::line_height`] and the font's natural
//! line height below the row top, so the gutter, body text, ghost text
//! and annotation share one baseline.
//!
//! The per-window status line is not painted here: the host reserves a
//! row below the editor for it.

use super::EditorPaintOptions;
use super::{CursorShape, DiagnosticSeverity, DiffLine, Editor, EditorLayout, EditorLine};
use super::{EditorSelection, GitLineStatus, SelectionKind};
use crate::paint_surface::{PaintSurface, PathVerb, TextRunStyle};
use crate::primitives::scrollbar::{native_surface_paint as scrollbar_paint, Scrollbar};
use crate::text_util::{char_to_byte_idx, next_char_boundary, snap_to_char_boundary};
use crate::theme::Theme;
use crate::types::Color;
use crate::{FontRole, Point, Rect};

const ROLE: FontRole = FontRole::Editor;

/// Paint `editor` into the rects `layout` resolved, in the surface's
/// editor font.
pub(crate) fn paint(
    editor: &Editor,
    layout: &EditorLayout,
    surface: &mut dyn PaintSurface,
    theme: &Theme,
) {
    paint_with_options(editor, layout, surface, theme, editor.paint_options);
}

/// [`paint`], reading paint decisions from `options` instead of
/// [`Editor::paint_options`]: with `suppress_v_scrollbar` set, the
/// vertical scrollbar is not painted even where `layout` reserves its
/// column, for a host that paints that scrollbar itself.
pub(crate) fn paint_with_options(
    editor: &Editor,
    layout: &EditorLayout,
    surface: &mut dyn PaintSurface,
    theme: &Theme,
    options: EditorPaintOptions,
) {
    let mut paint_in = |s: &mut dyn PaintSurface| paint_body(editor, layout, s, theme, options);
    if !surface.surface_with_editor_font(&mut paint_in) {
        paint_body(editor, layout, surface, theme, options);
    }
}

/// Geometry every pass shares.
struct Ctx<'a> {
    editor: &'a Editor,
    theme: &'a Theme,
    geom: &'a EditorLayout,
    rect: Rect,
    cw: f32,
    lh: f32,
    gutter_w: f32,
    /// Where column 0 of a row's text sits, after horizontal scroll.
    text_x: f32,
    /// Row top to text-box top, centring the font in the row pitch.
    v_off: f32,
    /// The font's natural line height.
    natural_h: f32,
    /// Whether to paint the vertical scrollbar `geom` reserves.
    paint_v_scrollbar: bool,
}

impl Ctx<'_> {
    fn row_y(&self, view_idx: usize) -> f32 {
        self.rect.y + view_idx as f32 * self.lh
    }

    fn window_bg(&self) -> Color {
        if self.editor.show_active_bg {
            self.theme.editor_active_background
        } else {
            self.theme.background
        }
    }

    /// Absolute x of byte `byte` of `text` in this row's text.
    fn x_at(&self, s: &mut dyn PaintSurface, text: &str, byte: usize) -> f32 {
        self.text_x + s.surface_text_x_at_index(text, ROLE, byte)
    }

    /// Absolute x of character column `col` of `text`.
    fn x_at_col(&self, s: &mut dyn PaintSurface, text: &str, col: usize) -> f32 {
        self.x_at(s, text, char_to_byte_idx(text, col))
    }

    /// The advance of the glyph at `byte`, or one cell past the end.
    fn glyph_w(&self, s: &mut dyn PaintSurface, text: &str, byte: usize) -> f32 {
        let next = next_char_boundary(text, byte);
        let w = if next > byte {
            s.surface_text_x_at_index(text, ROLE, next)
                - s.surface_text_x_at_index(text, ROLE, byte)
        } else {
            0.0
        };
        if w > 0.0 {
            w
        } else {
            self.cw
        }
    }
}

/// Draw `text` in one colour with its box's top-left at `(x, y)`.
fn draw_plain(s: &mut dyn PaintSurface, x: f32, y: f32, text: &str, fg: Color) {
    if text.is_empty() {
        return;
    }
    s.surface_draw_text_line(
        Point::new(x, y),
        text,
        &[TextRunStyle::new(0..text.len(), fg)],
        ROLE,
    );
}

fn paint_body(
    editor: &Editor,
    geom: &EditorLayout,
    surface: &mut dyn PaintSurface,
    theme: &Theme,
    options: EditorPaintOptions,
) {
    let rect = geom.bounds;
    let cw = geom.cell_width;
    let lh = geom.line_height;
    let gutter_w = editor.gutter_char_width as f32 * cw;
    let natural_h = match surface.surface_measure_text("").1 {
        h if h > 0.0 => h,
        _ => surface.surface_measure_text("M").1,
    };
    let ctx = Ctx {
        editor,
        theme,
        geom,
        rect,
        cw,
        lh,
        gutter_w,
        text_x: rect.x + gutter_w - editor.scroll_left as f32 * cw,
        v_off: (lh - natural_h) / 2.0,
        natural_h,
        paint_v_scrollbar: !options.suppress_v_scrollbar,
    };

    surface.surface_fill_rect(rect, ctx.window_bg());
    paint_line_backgrounds(&ctx, surface);
    paint_selection_overlays(&ctx, surface);
    if editor.gutter_char_width > 0 {
        for (view_idx, line) in editor.lines.iter().enumerate() {
            paint_gutter_row(&ctx, surface, line, ctx.row_y(view_idx));
        }
    }

    // The text clip excludes the gutter and the reserved vertical
    // scrollbar column, so no glyph reaches pixels the scrollbar paints.
    surface.surface_push_clip(Rect::new(
        rect.x + gutter_w,
        rect.y,
        geom.text_bounds.width,
        rect.height,
    ));
    for (view_idx, line) in editor.lines.iter().enumerate() {
        let y = ctx.row_y(view_idx);
        paint_line_text(&ctx, surface, line, y);
        paint_line_guides_and_columns(&ctx, surface, line, y);
        paint_line_bracket_match(&ctx, surface, view_idx, y);
        paint_line_underlines(&ctx, surface, line, y);
    }
    surface.surface_pop_clip();

    paint_scrollbars(&ctx, surface);
    paint_primary_cursor(&ctx, surface);
    paint_ai_ghost_text(&ctx, surface);
    paint_secondary_cursors(&ctx, surface);
}

// ── Row backgrounds ─────────────────────────────────────────────────────

fn paint_line_backgrounds(ctx: &Ctx, s: &mut dyn PaintSurface) {
    let (editor, theme) = (ctx.editor, ctx.theme);
    let bg_right = ctx.geom.text_bounds.x + ctx.geom.text_bounds.width;
    let bg_width = (bg_right - ctx.rect.x).max(0.0);
    for (view_idx, rl) in editor.lines.iter().enumerate() {
        let color = if rl.is_dap_current {
            Some(theme.dap_stopped_bg)
        } else if let Some(diff_status) = rl.diff_status {
            match diff_status {
                DiffLine::Added => Some(theme.diff_added_bg),
                DiffLine::Removed => Some(theme.diff_removed_bg),
                DiffLine::Padding => Some(theme.diff_padding_bg),
                DiffLine::Same => None,
            }
        } else if rl.is_current_line && editor.is_active && editor.cursorline {
            Some(theme.cursorline_bg)
        } else {
            None
        };
        if let Some(color) = color {
            s.surface_fill_rect(
                Rect::new(ctx.rect.x, ctx.row_y(view_idx), bg_width, ctx.lh),
                color,
            );
        }
    }
}

// ── Selections ──────────────────────────────────────────────────────────

fn paint_selection_overlays(ctx: &Ctx, s: &mut dyn PaintSurface) {
    let (editor, theme) = (ctx.editor, ctx.theme);
    let sel_color = theme.selection.with_alpha(theme.selection_alpha as f64);
    for sel in editor.selection.iter().chain(&editor.extra_selections) {
        paint_selection(ctx, s, sel, sel_color);
    }
    if let Some(yh) = &editor.yank_highlight {
        let color = theme
            .yank_highlight_bg
            .with_alpha(theme.yank_highlight_alpha as f64);
        paint_selection(ctx, s, yh, color);
    }
}

/// One selection, one rect per visible row it covers. Column math —
/// wrapped segments, ghost-continuation and diff-padding rows — comes
/// from [`EditorSelection::cols_on`].
fn paint_selection(ctx: &Ctx, s: &mut dyn PaintSurface, sel: &EditorSelection, color: Color) {
    for (view_idx, rl) in ctx.editor.lines.iter().enumerate() {
        let Some(cols) = sel.cols_on(rl) else {
            continue;
        };
        let y = ctx.row_y(view_idx);
        if sel.kind == SelectionKind::Line {
            let w = ctx.rect.width - (ctx.text_x - ctx.rect.x);
            s.surface_fill_rect(Rect::new(ctx.text_x, y, w, ctx.lh), color);
            continue;
        }
        let text = rl.raw_text.as_str();
        let start_x = ctx.x_at_col(s, text, cols.start);
        let end_x = if cols.extends_beyond {
            ctx.text_x + s.surface_measure_text(text).0
        } else {
            ctx.x_at_col(s, text, cols.end)
        };
        s.surface_fill_rect(
            Rect::new(start_x, y, (end_x - start_x).max(0.0), ctx.lh),
            color,
        );
    }
}

// ── Gutter ──────────────────────────────────────────────────────────────

/// Breakpoint marker, git marker and right-aligned line number for one
/// row, then the diagnostic dot (which wins over the lightbulb) or the
/// code-action lightbulb glyph.
fn paint_gutter_row(ctx: &Ctx, s: &mut dyn PaintSurface, rl: &EditorLine, y: f32) {
    let (editor, theme) = (ctx.editor, ctx.theme);
    let x0 = ctx.rect.x;
    let text_y = y + ctx.v_off;
    let mut char_offset = 0usize;

    if editor.has_breakpoints {
        let bp: String = rl.gutter_text.chars().take(1).collect();
        let color = if rl.is_dap_current || rl.is_breakpoint {
            theme.diagnostic_error
        } else {
            theme.line_number_fg
        };
        draw_plain(s, x0 + 3.0, text_y, &bp, color);
        char_offset += 1;
    }
    if editor.has_git_diff {
        let git: String = rl.gutter_text.chars().skip(char_offset).take(1).collect();
        let color = match rl.git_diff {
            Some(GitLineStatus::Added) => theme.git_added,
            Some(GitLineStatus::Modified) => theme.git_modified,
            Some(GitLineStatus::Deleted) => theme.git_deleted,
            None => theme.line_number_fg,
        };
        draw_plain(
            s,
            x0 + char_offset as f32 * ctx.cw + 3.0,
            text_y,
            &git,
            color,
        );
        char_offset += 1;
    }

    let rest: String = rl.gutter_text.chars().skip(char_offset).collect();
    let num_w = s.surface_measure_text(&rest).0;
    let num_x = x0 + ctx.gutter_w - num_w - ctx.cw + 3.0;
    let num_color = if editor.is_active && rl.is_current_line {
        theme.line_number_active_fg
    } else {
        theme.line_number_fg
    };
    draw_plain(s, num_x, text_y, &rest, num_color);

    if let Some(severity) = editor.diagnostic_gutter.get(&rl.line_idx) {
        let r = ctx.lh * 0.2;
        let cx = x0 + 3.0 + r;
        let cy = y + ctx.lh * 0.5;
        s.surface_fill_rounded_rect(
            Rect::new(cx - r, cy - r, 2.0 * r, 2.0 * r),
            r,
            severity_color(*severity, theme),
        );
    } else if !rl.is_wrap_continuation
        && editor.code_action_lines.contains(&rl.line_idx)
        && editor.lightbulb_glyph != '\0'
    {
        draw_plain(
            s,
            x0 + 1.0,
            text_y,
            &editor.lightbulb_glyph.to_string(),
            theme.lightbulb,
        );
    }
}

fn severity_color(severity: DiagnosticSeverity, theme: &Theme) -> Color {
    match severity {
        DiagnosticSeverity::Error => theme.diagnostic_error,
        DiagnosticSeverity::Warning => theme.diagnostic_warning,
        DiagnosticSeverity::Information => theme.diagnostic_info,
        DiagnosticSeverity::Hint => theme.diagnostic_hint,
    }
}

// ── Row text ────────────────────────────────────────────────────────────

/// `rl.raw_text` partitioned into runs: each byte takes the last span
/// covering it, gaps take `default_fg`.
fn line_runs(rl: &EditorLine, default_fg: Color) -> Vec<TextRunStyle> {
    let text = rl.raw_text.as_str();
    let mut byte_style: Vec<(Color, bool, bool)> = vec![(default_fg, false, false); text.len()];
    for span in &rl.spans {
        let start = snap_to_char_boundary(text, span.start_byte);
        let end = snap_to_char_boundary(text, span.end_byte);
        for st in byte_style.iter_mut().take(end).skip(start) {
            *st = (span.style.fg, span.style.bold, span.style.italic);
        }
    }
    let mut runs: Vec<TextRunStyle> = Vec::new();
    let mut start = 0usize;
    for i in 1..=text.len() {
        if i == text.len() || (text.is_char_boundary(i) && byte_style[i] != byte_style[start]) {
            let (fg, bold, italic) = byte_style[start];
            runs.push(
                TextRunStyle::new(start..i, fg)
                    .with_bold(bold)
                    .with_italic(italic),
            );
            start = i;
        }
    }
    runs
}

/// Span backgrounds, the syntax-highlighted text, a ghost continuation
/// row's suggestion, and the inline annotation after the text.
fn paint_line_text(ctx: &Ctx, s: &mut dyn PaintSurface, rl: &EditorLine, y: f32) {
    let theme = ctx.theme;
    let text = rl.raw_text.as_str();
    let text_y = y + ctx.v_off;

    for span in &rl.spans {
        let Some(bg) = span.style.bg else {
            continue;
        };
        let x0 = ctx.x_at(s, text, span.start_byte);
        let x1 = ctx.x_at(s, text, span.end_byte);
        if x1 > x0 {
            s.surface_fill_rect(Rect::new(x0, text_y, x1 - x0, ctx.natural_h), bg);
        }
    }
    s.surface_draw_text_line(
        Point::new(ctx.text_x, text_y),
        text,
        &line_runs(rl, theme.foreground),
        ROLE,
    );

    let mut last_text = text;
    if rl.is_ghost_continuation {
        if let Some(ghost) = &rl.ghost_suffix {
            draw_plain(s, ctx.text_x, text_y, ghost, theme.ghost_text_fg);
            last_text = ghost;
        }
    }
    if let Some(ann) = &rl.annotation {
        let ann_x = ctx.text_x + s.surface_measure_text(last_text).0 + ctx.cw * 2.0;
        draw_plain(s, ann_x, text_y, ann, theme.annotation_fg);
    }
}

/// Indent guides (one-pixel vertical lines) and colour columns (tinted
/// cells) for one row.
fn paint_line_guides_and_columns(ctx: &Ctx, s: &mut dyn PaintSurface, rl: &EditorLine, y: f32) {
    let (editor, theme) = (ctx.editor, ctx.theme);
    for &guide_col in &rl.indent_guides {
        let color = if editor.active_indent_col == Some(guide_col) {
            theme.indent_guide_active_fg
        } else {
            theme.indent_guide_fg
        };
        let gx = ctx.text_x + guide_col as f32 * ctx.cw;
        s.surface_draw_line(Point::new(gx, y), Point::new(gx, y + ctx.lh), color, 1.0);
    }
    for &cc_col in &rl.colorcolumns {
        let cx = ctx.text_x + cc_col as f32 * ctx.cw;
        s.surface_fill_rect(Rect::new(cx, y, ctx.cw, ctx.lh), theme.colorcolumn_bg);
    }
}

/// A translucent cell over each matched bracket on this row.
fn paint_line_bracket_match(ctx: &Ctx, s: &mut dyn PaintSurface, view_idx: usize, y: f32) {
    let color = ctx.theme.bracket_match_bg.with_alpha(0.6);
    for &(bm_view_line, bm_col) in &ctx.editor.bracket_match_positions {
        if bm_view_line == view_idx {
            let bx = ctx.text_x + bm_col as f32 * ctx.cw;
            s.surface_fill_rect(Rect::new(bx, y, ctx.cw, ctx.lh), color);
        }
    }
}

/// Wavy diagnostic underlines and dotted spell underlines, two pixels
/// above the row's bottom edge.
fn paint_line_underlines(ctx: &Ctx, s: &mut dyn PaintSurface, rl: &EditorLine, y: f32) {
    let text = rl.raw_text.as_str();
    let underline_y = y + ctx.lh - 2.0;
    for dm in &rl.diagnostics {
        let x0 = ctx.x_at_col(s, text, dm.start_col);
        let x1 = ctx.x_at_col(s, text, dm.end_col);
        let verbs = wave_path(x0, x1, underline_y);
        s.surface_draw_path(&verbs, severity_color(dm.severity, ctx.theme), 1.0);
    }
    for sm in &rl.spell_errors {
        let x0 = ctx.x_at_col(s, text, sm.start_col);
        let x1 = ctx.x_at_col(s, text, sm.end_col);
        let mut dx = x0;
        while dx < x1 {
            s.surface_fill_rect(Rect::new(dx, underline_y, 1.0, 1.0), ctx.theme.spell_error);
            dx += 3.0;
        }
    }
}

/// A squiggle from `x0` to `x1` along `y`: half-waves 4px long, peaking
/// 1.5px either side alternately, each traced as a cubic whose two control
/// points meet at the half-wave's midpoint, sampled into line segments.
fn wave_path(x0: f32, x1: f32, y: f32) -> Vec<PathVerb> {
    const WAVE_H: f32 = 1.5;
    const WAVE_LEN: f32 = 4.0;
    const STEPS: usize = 4;
    let mut verbs = vec![PathVerb::MoveTo(Point::new(x0, y))];
    let mut wx = x0;
    let mut up = true;
    while wx < x1 {
        let next_x = (wx + WAVE_LEN).min(x1);
        let mid = wx + (next_x - wx) * 0.5;
        let peak = if up { -WAVE_H } else { WAVE_H };
        for step in 1..=STEPS {
            let t = step as f32 / STEPS as f32;
            let u = 1.0 - t;
            let ctrl = 3.0 * u * t;
            let x = u * u * u * wx + ctrl * mid + t * t * t * next_x;
            verbs.push(PathVerb::LineTo(Point::new(x, y + ctrl * peak)));
        }
        wx = next_x;
        up = !up;
    }
    verbs
}

// ── Scrollbars ──────────────────────────────────────────────────────────

fn paint_scrollbars(ctx: &Ctx, s: &mut dyn PaintSurface) {
    let (editor, geom) = (ctx.editor, ctx.geom);
    if let Some(v_track) = geom.v_scrollbar_bounds.filter(|_| ctx.paint_v_scrollbar) {
        let sb = Scrollbar::vertical(
            "editor:v_scrollbar",
            v_track,
            editor.scroll_top as f32,
            editor.total_lines as f32,
            geom.visible_lines as f32,
            ctx.lh,
        );
        scrollbar_paint::paint(&sb, s, ctx.theme);
    }
    paint_h_scrollbar(ctx, s);
}

fn paint_h_scrollbar(ctx: &Ctx, s: &mut dyn PaintSurface) {
    let (editor, geom) = (ctx.editor, ctx.geom);
    if let Some(h_track) = geom.h_scrollbar_bounds {
        let sb = Scrollbar::horizontal(
            "editor:h_scrollbar",
            h_track,
            editor.scroll_left as f32,
            editor.max_col as f32,
            geom.visible_cols as f32,
            ctx.lh,
        );
        scrollbar_paint::paint(&sb, s, ctx.theme);
    }
}

// ── Carets ──────────────────────────────────────────────────────────────

/// One caret of `shape` at `(x, y)`, `w` wide when it spans a glyph.
fn paint_caret(ctx: &Ctx, s: &mut dyn PaintSurface, shape: CursorShape, x: f32, y: f32, w: f32) {
    let theme = ctx.theme;
    match shape {
        CursorShape::Block => s.surface_fill_rect(
            Rect::new(x, y, w, ctx.lh),
            theme.cursor.with_alpha(theme.cursor_normal_alpha as f64),
        ),
        CursorShape::Bar => s.surface_fill_rect(Rect::new(x, y, 2.0, ctx.lh), theme.cursor),
        CursorShape::Underline => {
            let bar_h = (ctx.lh * 0.12).max(2.0);
            s.surface_fill_rect(Rect::new(x, y + ctx.lh - bar_h, w, bar_h), theme.cursor);
        }
    }
}

/// The primary caret, always visible. A bar caret with word
/// multi-selections active sits one column right, after the selected
/// word.
fn paint_primary_cursor(ctx: &Ctx, s: &mut dyn PaintSurface) {
    let editor = ctx.editor;
    let Some(cursor) = &editor.cursor else {
        return;
    };
    let Some(rl) = editor.lines.get(cursor.pos.view_line) else {
        return;
    };
    let col = if !editor.extra_selections.is_empty() && cursor.shape == CursorShape::Bar {
        cursor.pos.col + 1
    } else {
        cursor.pos.col
    };
    let text = rl.raw_text.as_str();
    let byte = char_to_byte_idx(text, col);
    let x = ctx.x_at(s, text, byte);
    let w = ctx.glyph_w(s, text, byte);
    paint_caret(ctx, s, cursor.shape, x, ctx.row_y(cursor.pos.view_line), w);
}

/// The AI suggestion after the caret, on the caret's row.
fn paint_ai_ghost_text(ctx: &Ctx, s: &mut dyn PaintSurface) {
    let editor = ctx.editor;
    let Some(cursor) = &editor.cursor else {
        return;
    };
    let Some(rl) = editor.lines.get(cursor.pos.view_line) else {
        return;
    };
    let Some(ghost) = &rl.ghost_suffix else {
        return;
    };
    let x = ctx.x_at_col(s, &rl.raw_text, cursor.pos.col);
    let y = ctx.row_y(cursor.pos.view_line) + ctx.v_off;
    draw_plain(s, x, y, ghost, ctx.theme.ghost_text_fg);
}

/// Multi-cursor carets, in the primary caret's shape (a bar when there is
/// no primary caret).
fn paint_secondary_cursors(ctx: &Ctx, s: &mut dyn PaintSurface) {
    let editor = ctx.editor;
    let shape = editor
        .cursor
        .as_ref()
        .map(|c| c.shape)
        .unwrap_or(CursorShape::Bar);
    let has_extra_sels = !editor.extra_selections.is_empty();
    for pos in &editor.extra_cursors {
        let Some(rl) = editor.lines.get(pos.view_line) else {
            continue;
        };
        let col = if has_extra_sels && shape == CursorShape::Bar {
            pos.col + 1
        } else {
            pos.col
        };
        let text = rl.raw_text.as_str();
        let byte = char_to_byte_idx(text, col);
        let x = ctx.x_at(s, text, byte);
        let w = ctx.glyph_w(s, text, byte);
        paint_caret(ctx, s, shape, x, ctx.row_y(pos.view_line), w);
    }
}

/// The GTK editor pixel tests, run against this painter through the
/// [`crate::gtk::surface::CairoSurface`] the GTK delegate paints with.
#[cfg(all(test, feature = "gtk"))]
mod gtk_pixel_tests {
    use super::super::{
        DiffLine, Editor, EditorLine, EditorPaintOptions, EditorSelection, SelectionKind,
    };
    use crate::event::Rect;
    use crate::theme::Theme;
    use crate::types::Color;
    use gtk4::pango;
    use pangocairo::cairo::{Context, Format, ImageSurface};
    use std::collections::{HashMap, HashSet};

    fn cairo_rgb(c: Color) -> (f64, f64, f64) {
        (c.r as f64 / 255.0, c.g as f64 / 255.0, c.b as f64 / 255.0)
    }

    /// What the GTK delegate does: lay out at `editor.rect`, paint through
    /// a `CairoSurface` over `cr` and the editor-font `layout`.
    #[allow(clippy::too_many_arguments)]
    fn draw_editor_with_options_and_v_scrollbar_w(
        cr: &Context,
        layout: &pango::Layout,
        _font_metrics: &pango::FontMetrics,
        editor: &Editor,
        theme: &Theme,
        char_width: f64,
        line_height: f64,
        options: EditorPaintOptions,
        v_scrollbar_w: Option<f32>,
    ) {
        let geom = editor.layout_with_options_and_v_scrollbar_w(
            editor.rect,
            char_width as f32,
            line_height as f32,
            options,
            v_scrollbar_w,
        );
        let mut surface = crate::gtk::surface::CairoSurface {
            cr,
            layout: Some(layout),
            translucent_fill: true,
        };
        super::paint_with_options(editor, &geom, &mut surface, theme, options);
    }

    fn draw_editor(
        cr: &Context,
        layout: &pango::Layout,
        font_metrics: &pango::FontMetrics,
        editor: &Editor,
        theme: &Theme,
        char_width: f64,
        line_height: f64,
    ) {
        draw_editor_with_options_and_v_scrollbar_w(
            cr,
            layout,
            font_metrics,
            editor,
            theme,
            char_width,
            line_height,
            EditorPaintOptions::default(),
            None,
        );
    }

    // ── Scrollbar paint ─────────────────────────────────────────────────
    //
    // "State-derived paint geometry" (quadraui/docs/TESTING.md coverage
    // taxonomy row 3): set `total_lines`/`max_col` to a known overflow (or
    // non-overflow) state, paint into a headless Cairo `ImageSurface`, and
    // assert the scrollbar track's translucent tint is present (or absent)
    // at the exact pixel `Editor::layout`'s own geometry predicts — the
    // same geometry `EditorLayout::hit_test` uses, so paint and click can't
    // drift apart. Mirrors `gtk/data_table.rs`'s pixel-readback harness.

    const SCROLL_TEST_W: i32 = 200;
    const SCROLL_TEST_H: i32 = 80;
    const SCROLL_TEST_CHAR_W: f64 = 8.0;
    const SCROLL_TEST_LINE_H: f64 = 16.0;

    fn rgb(c: Color) -> (u8, u8, u8) {
        (c.r, c.g, c.b)
    }

    /// Read an RGB triple from an ARgb32 surface at pixel (x, y). Same
    /// byte layout as `gtk/data_table.rs`'s test helper and `GtkDriver::pixel`.
    fn scroll_test_pixel(data: &[u8], stride: usize, x: i32, y: i32) -> (u8, u8, u8) {
        let off = y as usize * stride + x as usize * 4;
        (data[off + 2], data[off + 1], data[off])
    }

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

    /// Editor fixture at `SCROLL_TEST_W`x`SCROLL_TEST_H` (matching the
    /// surface size the tests below paint into) with no gutter, so the
    /// only geometry in play is the scrollbar reservation itself.
    fn scroll_test_editor(total_lines: usize, max_col: usize, num_lines: usize) -> Editor {
        Editor {
            id: "ed".into(),
            rect: Rect::new(0.0, 0.0, SCROLL_TEST_W as f32, SCROLL_TEST_H as f32),
            lines: (0..num_lines).map(blank_line).collect(),
            cursor: None,
            extra_cursors: Vec::new(),
            selection: None,
            extra_selections: Vec::new(),
            yank_highlight: None,
            scroll_top: 0,
            scroll_left: 0,
            total_lines,
            max_col,
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
            lightbulb_glyph: '\0',
            paint_options: Default::default(),
        }
    }

    /// Paint `editor` into a fresh `theme.background`-filled surface and
    /// return the raw pixel buffer + stride. Mirrors `gtk/data_table.rs`'s
    /// identical test harness.
    fn scroll_test_paint(editor: &Editor, theme: &Theme) -> (Vec<u8>, i32) {
        let mut surface = ImageSurface::create(Format::ARgb32, SCROLL_TEST_W, SCROLL_TEST_H)
            .expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("Context::new");
            let (br, bg, bb) = cairo_rgb(theme.background);
            cr.set_source_rgb(br, bg, bb);
            cr.paint().ok();

            let pango_layout = pangocairo::functions::create_layout(&cr);
            let pango_ctx = pango_layout.context();
            pango_ctx.set_font_description(&pango::FontDescription::from_string("Monospace 12"));
            let metrics = pango_ctx.metrics(None, None);

            draw_editor(
                &cr,
                &pango_layout,
                &metrics,
                editor,
                theme,
                SCROLL_TEST_CHAR_W,
                SCROLL_TEST_LINE_H,
            );
        }
        surface.flush();
        let stride = surface.stride();
        let data = surface.data().expect("surface data").to_vec();
        (data, stride)
    }

    /// [`scroll_test_paint`], but through [`draw_editor_with_options`] so
    /// tests can exercise [`EditorPaintOptions`].
    fn scroll_test_paint_with_options(
        editor: &Editor,
        theme: &Theme,
        options: EditorPaintOptions,
    ) -> (Vec<u8>, i32) {
        scroll_test_paint_with_v_scrollbar_w(editor, theme, options, None)
    }

    /// [`scroll_test_paint_with_options`], plus a scrollbar-width
    /// override via [`draw_editor_with_options_and_v_scrollbar_w`].
    fn scroll_test_paint_with_v_scrollbar_w(
        editor: &Editor,
        theme: &Theme,
        options: EditorPaintOptions,
        v_scrollbar_w: Option<f32>,
    ) -> (Vec<u8>, i32) {
        let mut surface = ImageSurface::create(Format::ARgb32, SCROLL_TEST_W, SCROLL_TEST_H)
            .expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("Context::new");
            let (br, bg, bb) = cairo_rgb(theme.background);
            cr.set_source_rgb(br, bg, bb);
            cr.paint().ok();

            let pango_layout = pangocairo::functions::create_layout(&cr);
            let pango_ctx = pango_layout.context();
            pango_ctx.set_font_description(&pango::FontDescription::from_string("Monospace 12"));
            let metrics = pango_ctx.metrics(None, None);

            draw_editor_with_options_and_v_scrollbar_w(
                &cr,
                &pango_layout,
                &metrics,
                editor,
                theme,
                SCROLL_TEST_CHAR_W,
                SCROLL_TEST_LINE_H,
                options,
                v_scrollbar_w,
            );
        }
        surface.flush();
        let stride = surface.stride();
        let data = surface.data().expect("surface data").to_vec();
        (data, stride)
    }

    /// `gtk::draw_editor` must paint scrollbars when a buffer is taller
    /// than the viewport — a vertical scrollbar track in the reserved
    /// rightmost `char_width`-wide column; a buffer that fits must not
    /// tint that column at all.
    #[test]
    fn draw_editor_paints_vertical_scrollbar_when_buffer_overflows() {
        let theme = Theme::default();

        let overflowing = scroll_test_editor(50, 0, 5);
        let (data, stride) = scroll_test_paint(&overflowing, &theme);
        // Solidly inside the reserved v-scrollbar column (x in [192,200))
        // and mid-track vertically — see `Editor::layout`'s
        // `v_scrollbar_bounds` formula for why this pixel is in-track.
        let px = scroll_test_pixel(&data, stride as usize, 198, 40);
        assert_ne!(
            px,
            rgb(theme.background),
            "vertical scrollbar track should tint this pixel when total_lines overflows the viewport"
        );

        let fits = scroll_test_editor(3, 0, 3);
        let (data2, stride2) = scroll_test_paint(&fits, &theme);
        let px2 = scroll_test_pixel(&data2, stride2 as usize, 198, 40);
        assert_eq!(
            px2,
            rgb(theme.background),
            "no vertical scrollbar should paint when the buffer fits the viewport"
        );
    }

    /// The content clip must be **narrowed** by the reserved v-scrollbar
    /// column width, not merely overlaid afterwards — otherwise a
    /// translucent scrollbar track paints on top of glyphs the
    /// (unnarrowed) text loop already rendered underneath it, which is
    /// visible wherever the track/thumb isn't fully opaque. Fill every
    /// visible line with far more text than fits the viewport (so glyphs
    /// would reach all the way to the true right edge without the fix)
    /// and assert the reserved column's pixel is identical whether or
    /// not that text is present — the only way that can hold is if the
    /// text never reaches that column at all (narrowed out), since the
    /// scrollbar itself paints identically in both cases.
    #[test]
    fn draw_editor_scrollbar_column_unaffected_by_overflowing_text() {
        let theme = Theme::default();

        let mut with_text = scroll_test_editor(50, 0, 5);
        for line in with_text.lines.iter_mut() {
            // Comfortably wider than SCROLL_TEST_W under any reasonable
            // font metrics — text that would overflow past the reserved
            // scrollbar column if the clip were not narrowed.
            line.raw_text = "M".repeat(60);
        }
        let blank = scroll_test_editor(50, 0, 5);

        let (data_text, stride) = scroll_test_paint(&with_text, &theme);
        let (data_blank, stride2) = scroll_test_paint(&blank, &theme);
        assert_eq!(stride, stride2);

        // Same pixel the tests above assert is scrollbar-tinted.
        let px_text = scroll_test_pixel(&data_text, stride as usize, 198, 40);
        let px_blank = scroll_test_pixel(&data_blank, stride as usize, 198, 40);
        assert_eq!(
            px_text, px_blank,
            "scrollbar column pixel must be identical regardless of overflowing text — \
             the text loop should narrow around the reserved column instead of painting \
             glyphs the scrollbar then overlays (#968)"
        );
    }

    /// `paint_line_backgrounds` must narrow the cursorline/diff/DAP row
    /// fill to `text_bounds`, the same way the text clip is narrowed —
    /// otherwise the fill reaches the reserved v-scrollbar column and
    /// bleeds through the translucent scrollbar track. Paint a buffer
    /// with the cursorline active on the row under the scrollbar's
    /// mid-track pixel and assert that pixel is identical to the same
    /// pixel with no cursorline active at all: the only way a translucent
    /// track composited over two different row colours produces the same
    /// result is if neither row colour actually reaches that pixel.
    #[test]
    fn draw_editor_cursorline_background_does_not_bleed_into_scrollbar_column() {
        let theme = Theme::default();
        assert_ne!(
            theme.cursorline_bg, theme.background,
            "fixture assumption: cursorline_bg must differ from background for this test to \
             be meaningful"
        );

        let mut with_cursorline = scroll_test_editor(50, 0, 5);
        // Line at view index 2 covers y in [32, 48) — pixel y=40 below is
        // the same mid-track pixel the other scrollbar tests sample.
        with_cursorline.cursorline = true;
        with_cursorline.lines[2].is_current_line = true;

        let blank = scroll_test_editor(50, 0, 5);

        let (data_cursorline, stride) = scroll_test_paint(&with_cursorline, &theme);
        let (data_blank, stride2) = scroll_test_paint(&blank, &theme);
        assert_eq!(stride, stride2);

        let px_cursorline = scroll_test_pixel(&data_cursorline, stride as usize, 198, 40);
        let px_blank = scroll_test_pixel(&data_blank, stride as usize, 198, 40);
        assert_eq!(
            px_cursorline, px_blank,
            "scrollbar column pixel must be identical regardless of the cursorline background \
             on the row underneath it — the row fill should narrow around the reserved column \
             instead of bleeding into it"
        );
    }

    /// Passing `EditorPaintOptions { suppress_v_scrollbar: true, .. }`
    /// through `draw_editor_with_options` must stop the vertical
    /// scrollbar from painting at all, even though the buffer still
    /// overflows the viewport — this is the opt-out a host running a
    /// `Minimap`-as-scrollbar passes to avoid a redundant second
    /// affordance beside it. `draw_editor` (no options) must remain
    /// unaffected.
    #[test]
    fn draw_editor_with_options_suppress_v_scrollbar_paints_no_column() {
        let theme = Theme::default();
        let overflowing = scroll_test_editor(50, 0, 5);

        // Baseline: plain `draw_editor` still paints the column.
        let (data0, stride0) = scroll_test_paint(&overflowing, &theme);
        assert_ne!(
            scroll_test_pixel(&data0, stride0 as usize, 198, 40),
            rgb(theme.background)
        );

        let (data, stride) = scroll_test_paint_with_options(
            &overflowing,
            &theme,
            EditorPaintOptions {
                suppress_v_scrollbar: true,
                ..Default::default()
            },
        );
        let px = scroll_test_pixel(&data, stride as usize, 198, 40);
        assert_eq!(
            px,
            rgb(theme.background),
            "suppress_v_scrollbar should stop the vertical scrollbar from painting even \
             though total_lines overflows the viewport"
        );
    }

    /// The GTK rasteriser's vertical scrollbar column honours a
    /// `v_scrollbar_w` override end-to-end — a wider override paints
    /// further out, matching the layout-level assertions in
    /// `primitives::editor`'s own tests.
    ///
    /// Viewport is `SCROLL_TEST_W` = 200px, `SCROLL_TEST_CHAR_W` = 8.0.
    /// The default column spans `[192, 200)`; a 14px override spans
    /// `[186, 200)`. `x = 188` falls in the gap between those two spans
    /// — plain text-area background at baseline, inside the track once
    /// widened.
    #[test]
    fn draw_editor_v_scrollbar_w_widens_painted_column() {
        let theme = Theme::default();
        let overflowing = scroll_test_editor(50, 0, 5);

        let (data0, stride0) = scroll_test_paint(&overflowing, &theme);
        let baseline_px = scroll_test_pixel(&data0, stride0 as usize, 188, 40);
        assert_eq!(
            baseline_px,
            rgb(theme.background),
            "x=188 sits outside the default 8px-wide column, [192, 200)"
        );

        let (data, stride) = scroll_test_paint_with_v_scrollbar_w(
            &overflowing,
            &theme,
            EditorPaintOptions::default(),
            Some(14.0),
        );
        let widened_px = scroll_test_pixel(&data, stride as usize, 188, 40);
        assert_ne!(
            widened_px,
            rgb(theme.background),
            "a Some(14.0) v_scrollbar_w should widen the painted track to cover x=188 ([186, 200))"
        );
    }

    /// Horizontal-overflow counterpart of the vertical scrollbar test:
    /// a line wider than the viewport must paint a horizontal scrollbar
    /// track in the reserved bottom `line_height`-tall row; a buffer
    /// whose longest line fits must not tint that row at all.
    #[test]
    fn draw_editor_paints_horizontal_scrollbar_when_line_overflows() {
        let theme = Theme::default();

        let overflowing = scroll_test_editor(3, 40, 3);
        let (data, stride) = scroll_test_paint(&overflowing, &theme);
        // Solidly inside the reserved h-scrollbar row (y in [64,80)) —
        // no v-scrollbar column here (total_lines fits), so the full
        // width is available for the horizontal track.
        let px = scroll_test_pixel(&data, stride as usize, 100, 70);
        assert_ne!(
            px,
            rgb(theme.background),
            "horizontal scrollbar track should tint this pixel when max_col overflows the viewport"
        );

        let fits = scroll_test_editor(3, 5, 3);
        let (data2, stride2) = scroll_test_paint(&fits, &theme);
        let px2 = scroll_test_pixel(&data2, stride2 as usize, 100, 70);
        assert_eq!(
            px2,
            rgb(theme.background),
            "no horizontal scrollbar should paint when every line fits the viewport"
        );
    }

    // ── Selection column math via `EditorSelection::cols_on` ────────────

    fn text_line(line_idx: usize, raw_text: &str) -> EditorLine {
        EditorLine {
            raw_text: raw_text.into(),
            ..blank_line(line_idx)
        }
    }

    /// One `Char` selection spanning buffer lines `[0, 2]` painted
    /// across a plain row, a wrapped-segment row, a ghost-continuation
    /// row, and a diff-padding row — the exact matrix
    /// `tests/conformance/editor.rs` proves against
    /// `EditorSelection::cols_on` directly, proven again here through a
    /// real Cairo paint so the backend's own call site is covered too.
    #[test]
    fn draw_editor_selection_respects_wrapped_ghost_and_diff_padding_rows() {
        let theme = Theme::default();

        let lines = vec![
            // Row 0: buffer line 0, plain, selection's own start line.
            text_line(0, "0123456789"),
            // Row 1: a wrap-continuation segment of buffer line 1,
            // starting at buffer column 5 — its own text is only 5
            // chars, entirely inside the selection.
            EditorLine {
                segment_col_offset: 5,
                is_wrap_continuation: true,
                ..text_line(1, "abcde")
            },
            // Row 2: the AI-ghost continuation of buffer line 1 — same
            // `line_idx` as row 1, but virtual, must never be selected.
            EditorLine {
                ghost_suffix: Some("ai suggestion".into()),
                ..text_line(1, "")
            },
            // Row 3: buffer line 2 (the selection's own end line), but a
            // diff-padding filler row — must never be selected either.
            EditorLine {
                diff_status: Some(DiffLine::Padding),
                ..text_line(2, "")
            },
        ];

        let mut editor = scroll_test_editor(4, 0, 4);
        editor.lines = lines;
        editor.selection = Some(EditorSelection {
            kind: SelectionKind::Char,
            start_line: 0,
            start_col: 0,
            end_line: 2,
            end_col: 10,
        });

        let (data, stride) = scroll_test_paint(&editor, &theme);
        let stride = stride as usize;
        let sample_x = 2; // just past the left text edge on every row
        let row_y = |row: i32| row * SCROLL_TEST_LINE_H as i32 + (SCROLL_TEST_LINE_H as i32 / 2);

        assert_ne!(
            scroll_test_pixel(&data, stride, sample_x, row_y(0)),
            rgb(theme.background),
            "row 0 (plain, selection's start line) should be tinted"
        );
        assert_ne!(
            scroll_test_pixel(&data, stride, sample_x, row_y(1)),
            rgb(theme.background),
            "row 1 (wrapped segment of a fully-selected line) should be tinted"
        );
        assert_eq!(
            scroll_test_pixel(&data, stride, sample_x, row_y(2)),
            rgb(theme.background),
            "row 2 (ghost continuation) must never be tinted by a selection"
        );
        assert_eq!(
            scroll_test_pixel(&data, stride, sample_x, row_y(3)),
            rgb(theme.diff_padding_bg),
            "row 3 (diff padding) must show only its diff background, not a selection tint"
        );
    }

    // ── Text/gutter vertical centring within a host-set row pitch ──────
    //
    // Mirrors `macos::editor::tests::body_text_centred_in_row_and_shares_baseline_with_gutter`:
    // paints the same digit glyph as both the gutter number and the
    // body text at two row pitches, and checks the ink shifts down by
    // half the pitch's slack over the font's own natural height, with
    // gutter and body sharing a baseline once the offset is applied.

    const CENTRE_TEST_W: i32 = 200;
    const CENTRE_TEST_ROW_PITCH: f64 = 24.0;

    /// `(ascent + descent, measured "7" width)` for "Monospace 12" —
    /// the real font metrics every `centre_test_paint` call below also
    /// resolves (same family/size), used here only to size the fixture.
    fn centre_test_metrics() -> (f64, f64) {
        let surface =
            ImageSurface::create(Format::ARgb32, CENTRE_TEST_W, 40).expect("create ImageSurface");
        let cr = Context::new(&surface).expect("Context::new");
        let pango_layout = pangocairo::functions::create_layout(&cr);
        let pango_ctx = pango_layout.context();
        pango_ctx.set_font_description(&pango::FontDescription::from_string("Monospace 12"));
        let metrics = pango_ctx.metrics(None, None);
        let natural_pitch = (metrics.ascent() + metrics.descent()) as f64 / pango::SCALE as f64;
        pango_layout.set_text("7");
        let char_w = pango_layout.pixel_size().0 as f64;
        (natural_pitch, char_w)
    }

    /// Paint a one-line editor (raw + gutter text both `"7"`) at row
    /// pitch `pitch`, returning the painted ARGB32 buffer + stride.
    fn centre_test_paint(pitch: f64, gutter_chars: usize) -> (Vec<u8>, i32) {
        let theme = Theme::default();
        let mut surface = ImageSurface::create(Format::ARgb32, CENTRE_TEST_W, pitch.ceil() as i32)
            .expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("Context::new");
            let (br, bg, bb) = cairo_rgb(theme.background);
            cr.set_source_rgb(br, bg, bb);
            cr.paint().ok();

            let pango_layout = pangocairo::functions::create_layout(&cr);
            let pango_ctx = pango_layout.context();
            pango_ctx.set_font_description(&pango::FontDescription::from_string("Monospace 12"));
            let metrics = pango_ctx.metrics(None, None);
            pango_layout.set_text("7");
            let char_w = pango_layout.pixel_size().0 as f64;

            let mut line = blank_line(0);
            line.raw_text = "7".into();
            line.gutter_text = "7".into();
            let mut editor = scroll_test_editor(1, 1, 1);
            editor.lines = vec![line];
            editor.gutter_char_width = gutter_chars;
            editor.rect = Rect::new(0.0, 0.0, CENTRE_TEST_W as f32, pitch as f32);

            draw_editor_with_options_and_v_scrollbar_w(
                &cr,
                &pango_layout,
                &metrics,
                &editor,
                &theme,
                char_w,
                pitch,
                EditorPaintOptions::default(),
                None,
            );
        }
        surface.flush();
        let stride = surface.stride();
        let data = surface.data().expect("surface data").to_vec();
        (data, stride)
    }

    /// Scan `data` for the vertical extent of non-background ink inside
    /// `[x_start, x_end) × [y_start, y_end)` — GTK's counterpart to
    /// `macos::editor::tests::ink_vertical_extent`. `None` if the window
    /// contains no ink.
    fn centre_test_ink_extent(
        data: &[u8],
        stride: i32,
        x_start: i32,
        x_end: i32,
        y_start: i32,
        y_end: i32,
        bg: Color,
    ) -> Option<(i32, i32)> {
        const TOL: i32 = 24;
        let mut extent: Option<(i32, i32)> = None;
        for y in y_start..y_end {
            let has_ink = (x_start..x_end).any(|x| {
                let (r, g, b) = scroll_test_pixel(data, stride as usize, x, y);
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

    /// The AI ghost suggestion painted after the cursor sits on the same
    /// centred baseline as the typed text before it on that row.
    #[test]
    fn ai_ghost_text_after_cursor_shares_body_text_baseline() {
        use crate::primitives::editor::{CursorPos, CursorShape, EditorCursor};

        let mut theme = Theme::default();
        theme.ghost_text_fg = theme.foreground;
        let pitch = CENTRE_TEST_ROW_PITCH;
        let mut surface = ImageSurface::create(Format::ARgb32, CENTRE_TEST_W, pitch.ceil() as i32)
            .expect("create ImageSurface");
        let char_w;
        {
            let cr = Context::new(&surface).expect("Context::new");
            let (br, bg, bb) = cairo_rgb(theme.background);
            cr.set_source_rgb(br, bg, bb);
            cr.paint().ok();

            let pango_layout = pangocairo::functions::create_layout(&cr);
            let pango_ctx = pango_layout.context();
            pango_ctx.set_font_description(&pango::FontDescription::from_string("Monospace 12"));
            let metrics = pango_ctx.metrics(None, None);
            pango_layout.set_text("7");
            char_w = pango_layout.pixel_size().0 as f64;

            let mut line = blank_line(0);
            line.raw_text = "7".into();
            line.ghost_suffix = Some("7".into());
            let mut editor = scroll_test_editor(1, 1, 1);
            editor.lines = vec![line];
            editor.gutter_char_width = 0;
            editor.cursor = Some(EditorCursor {
                pos: CursorPos {
                    view_line: 0,
                    col: 1,
                },
                shape: CursorShape::Bar,
            });
            editor.rect = Rect::new(0.0, 0.0, CENTRE_TEST_W as f32, pitch as f32);

            draw_editor_with_options_and_v_scrollbar_w(
                &cr,
                &pango_layout,
                &metrics,
                &editor,
                &theme,
                char_w,
                pitch,
                EditorPaintOptions::default(),
                None,
            );
        }
        surface.flush();
        let stride = surface.stride();
        let data = surface.data().expect("surface data").to_vec();

        let body = centre_test_ink_extent(
            &data,
            stride,
            0,
            char_w.round() as i32 - 1,
            0,
            pitch as i32,
            theme.background,
        )
        .expect("typed glyph should paint ink");
        // Skip the 2px bar cursor at the start of the ghost cell.
        let ghost = centre_test_ink_extent(
            &data,
            stride,
            char_w.round() as i32 + 3,
            (2.0 * char_w).round() as i32,
            0,
            pitch as i32,
            theme.background,
        )
        .expect("ghost glyph should paint ink");
        assert!(
            (ghost.0 - body.0).abs() <= 1 && (ghost.1 - body.1).abs() <= 1,
            "ghost suggestion ink {ghost:?} should share the typed glyph's vertical extent {body:?}"
        );
    }

    #[test]
    fn draw_editor_body_text_centred_in_row_and_shares_baseline_with_gutter() {
        let theme = Theme::default();
        let (natural_pitch, char_w) = centre_test_metrics();
        assert!(
            natural_pitch < CENTRE_TEST_ROW_PITCH,
            "fixture needs a font natural line height ({natural_pitch}) shorter than the \
             row pitch ({CENTRE_TEST_ROW_PITCH}) to leave any slack to centre in",
        );

        let gutter_chars = 2usize;
        let gutter_w = gutter_chars as f64 * char_w;

        let (natural_data, natural_stride) = centre_test_paint(natural_pitch, gutter_chars);
        let natural = centre_test_ink_extent(
            &natural_data,
            natural_stride,
            gutter_w.round() as i32,
            (gutter_w + char_w).round() as i32,
            0,
            natural_pitch.ceil() as i32,
            theme.background,
        )
        .expect("body glyph should paint some ink at the natural pitch");

        let (wide_data, wide_stride) = centre_test_paint(CENTRE_TEST_ROW_PITCH, gutter_chars);
        let wide = centre_test_ink_extent(
            &wide_data,
            wide_stride,
            gutter_w.round() as i32,
            (gutter_w + char_w).round() as i32,
            0,
            CENTRE_TEST_ROW_PITCH as i32,
            theme.background,
        )
        .expect("body glyph should paint some ink at the wide pitch");

        let expected_shift = (CENTRE_TEST_ROW_PITCH - natural_pitch) / 2.0;
        let top_shift = wide.0 as f64 - natural.0 as f64;
        let bottom_shift = wide.1 as f64 - natural.1 as f64;
        assert!(
            (top_shift - expected_shift).abs() <= 2.0
                && (bottom_shift - expected_shift).abs() <= 2.0,
            "widening the row pitch from {natural_pitch} to {CENTRE_TEST_ROW_PITCH} should \
             move the body glyph down by half the added slack ({expected_shift}px), got top \
             {top_shift}px / bottom {bottom_shift}px (ink {natural:?} -> {wide:?})"
        );

        let gutter = centre_test_ink_extent(
            &wide_data,
            wide_stride,
            0,
            gutter_w.round() as i32,
            0,
            CENTRE_TEST_ROW_PITCH as i32,
            theme.background,
        )
        .expect("gutter glyph should paint some ink");
        assert_eq!(
            gutter, wide,
            "gutter number and body text (same glyph, \"7\") should share a baseline, got \
             gutter={gutter:?}, body={wide:?}"
        );
    }
}
