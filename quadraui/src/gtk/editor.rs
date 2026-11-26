//! GTK entry points for the [`Editor`] primitive — thin delegates onto
//! the widget's own pixel painter, `crate::primitives::editor::paint_pixels`,
//! which documents every paint pass. These functions only adapt a bare
//! Cairo context and the frame's editor-font Pango layout into the
//! [`super::surface::CairoSurface`] that painter takes, laying the editor
//! out at [`Editor::rect`].
//!
//! [`editor_col_at_x`] resolves a click against the per-span-attributed
//! Pango layout of one line, for [`crate::Backend::editor_col_at_x`].

use crate::primitives::editor::{
    paint_pixels, Editor, EditorLayout, EditorLine, EditorPaintOptions, EditorStyledSpan,
};
use crate::theme::Theme;
use crate::types::Color;
use gtk4::cairo::Context;
use gtk4::pango::{self, AttrColor, AttrList};

/// Paint an [`Editor`] primitive into the supplied Cairo context.
///
/// `char_width` and `line_height` are surface-native pixel
/// measurements obtained from the host's font metrics — passed in so
/// the host's font/scale settings stay authoritative.
///
/// Equivalent to [`draw_editor_with_options`] with
/// `EditorPaintOptions::default()`.
#[allow(clippy::too_many_arguments)]
pub fn draw_editor(
    cr: &Context,
    layout: &pango::Layout,
    font_metrics: &pango::FontMetrics,
    editor: &Editor,
    theme: &Theme,
    char_width: f64,
    line_height: f64,
) {
    draw_editor_with_options(
        cr,
        layout,
        font_metrics,
        editor,
        theme,
        char_width,
        line_height,
        EditorPaintOptions::default(),
    )
}

/// [`draw_editor`], plus [`EditorPaintOptions`] — `suppress_v_scrollbar`
/// leaves no scrollbar column for a host whose minimap acts as the
/// scrollbar.
///
/// Equivalent to [`draw_editor_with_options_and_v_scrollbar_w`] with
/// `v_scrollbar_w: None` (a `char_width`-wide scrollbar column).
#[allow(clippy::too_many_arguments)]
pub fn draw_editor_with_options(
    cr: &Context,
    layout: &pango::Layout,
    font_metrics: &pango::FontMetrics,
    editor: &Editor,
    theme: &Theme,
    char_width: f64,
    line_height: f64,
    options: EditorPaintOptions,
) {
    draw_editor_with_options_and_v_scrollbar_w(
        cr,
        layout,
        font_metrics,
        editor,
        theme,
        char_width,
        line_height,
        options,
        None,
    )
}

/// [`draw_editor_with_options`], plus a host-settable vertical scrollbar
/// width in pixels — see [`Editor::layout_with_options_and_v_scrollbar_w`]
/// for the geometry. `None` sizes the column at `char_width`.
/// `GtkBackend::draw_editor` calls this with the width set through
/// [`crate::Backend::set_editor_v_scrollbar_width`].
///
/// `font_metrics` is unused: the painter measures the font through
/// `layout`.
#[allow(clippy::too_many_arguments)]
pub fn draw_editor_with_options_and_v_scrollbar_w(
    cr: &Context,
    layout: &pango::Layout,
    font_metrics: &pango::FontMetrics,
    editor: &Editor,
    theme: &Theme,
    char_width: f64,
    line_height: f64,
    options: EditorPaintOptions,
    v_scrollbar_w: Option<f32>,
) {
    let _ = font_metrics;
    let geom = editor.layout_with_options_and_v_scrollbar_w(
        editor.rect,
        char_width as f32,
        line_height as f32,
        options,
        v_scrollbar_w,
    );
    let mut surface = super::surface::CairoSurface {
        cr,
        layout: Some(layout),
        translucent_fill: true,
    };
    paint_pixels::paint_with_options(editor, &geom, &mut surface, theme, options);
}

/// Convert a `quadraui::Color` into the 16-bit (0..65535) values
/// expected by Pango's `AttrColor` constructors.
fn pango_u16(c: Color) -> (u16, u16, u16) {
    (c.r as u16 * 257, c.g as u16 * 257, c.b as u16 * 257)
}

/// Build a Pango `AttrList` from the editor primitive's byte-range
/// spans. Mirrors `vimcode::gtk::draw::build_pango_attrs`.
fn build_pango_attrs(spans: &[EditorStyledSpan]) -> AttrList {
    let attrs = AttrList::new();
    for span in spans {
        let (fr, fg_g, fb) = pango_u16(span.style.fg);
        let mut fg_attr = AttrColor::new_foreground(fr, fg_g, fb);
        fg_attr.set_start_index(span.start_byte as u32);
        fg_attr.set_end_index(span.end_byte as u32);
        attrs.insert(fg_attr);

        if let Some(bg) = span.style.bg {
            let (br, bg_g, bb) = pango_u16(bg);
            let mut bg_attr = AttrColor::new_background(br, bg_g, bb);
            bg_attr.set_start_index(span.start_byte as u32);
            bg_attr.set_end_index(span.end_byte as u32);
            attrs.insert(bg_attr);
        }
        if span.style.bold {
            let mut w = pango::AttrInt::new_weight(pango::Weight::Bold);
            w.set_start_index(span.start_byte as u32);
            w.set_end_index(span.end_byte as u32);
            attrs.insert(w);
        }
        if span.style.italic {
            let mut s = pango::AttrInt::new_style(pango::Style::Italic);
            s.set_start_index(span.start_byte as u32);
            s.set_end_index(span.end_byte as u32);
            attrs.insert(s);
        }
        if (span.style.font_scale - 1.0).abs() > f32::EPSILON {
            let mut sc = pango::AttrFloat::new_scale(span.style.font_scale as f64);
            sc.set_start_index(span.start_byte as u32);
            sc.set_end_index(span.end_byte as u32);
            attrs.insert(sc);
        }
    }
    attrs
}

/// Resolve `x` (absolute/surface-space) to a character column within
/// `line.raw_text`, inverting the **exact** glyph geometry
/// [`draw_editor`] painted the line with.
///
/// `pango_layout`'s text and attributes are overwritten by this call
/// (`set_text` + [`build_pango_attrs`], mirroring `draw_editor`'s own
/// per-line setup) — pass a layout carrying the same font family/size
/// `draw_editor` painted with (the frame-scoped layout while still
/// inside `enter_frame_scope`, or the cached last-painted clone when
/// called at click-time outside any frame scope).
///
/// `editor_layout` supplies `text_bounds.x`, `scroll_left`, and
/// `cell_width` — the same values `draw_editor` used to compute
/// `text_x_offset`. Horizontal scroll is resolved in the *layout's*
/// local coordinate space (the Pango layout holds the line's full,
/// unscrolled `raw_text`; scrolling only changes where `draw_editor`
/// positions that layout on screen), so the returned column already
/// reflects `scroll_left` — do not add it again.
pub fn editor_col_at_x(
    pango_layout: &pango::Layout,
    line: &EditorLine,
    editor_layout: &EditorLayout,
    x: f32,
) -> usize {
    pango_layout.set_text(&line.raw_text);
    let attrs = build_pango_attrs(&line.spans);
    pango_layout.set_attributes(Some(&attrs));

    let cell_width = editor_layout.cell_width as f64;
    let local_x = (x as f64 - editor_layout.text_bounds.x as f64)
        + editor_layout.scroll_left as f64 * cell_width;
    let x_pango = (local_x.max(0.0) * pango::SCALE as f64).round() as i32;
    let (_inside, byte_index, _trailing) = pango_layout.xy_to_index(x_pango, 0);
    // Pango should only ever return a UTF-8 char-boundary-aligned index for
    // valid input, but we don't fully control its internals across
    // versions/locales — snap defensively rather than trust it blindly and
    // risk a panic on a non-boundary `byte_index`.
    let clamped =
        crate::text_util::snap_to_char_boundary(&line.raw_text, byte_index.max(0) as usize);
    line.raw_text[..clamped].chars().count() + line.segment_col_offset
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Rect;
    use crate::primitives::editor::{Editor, Style};
    use crate::types::Color;
    use pangocairo::cairo::{Context, Format, ImageSurface};
    use std::collections::{HashMap, HashSet};

    /// A line with three differently-styled spans (bold / italic /
    /// `font_scale`) so its painted glyph advances are *not* uniform —
    /// the exact case where the monospace `EditorLayout::col_at_x`
    /// fallback drifts from what `draw_editor` actually painted.
    fn styled_line() -> EditorLine {
        let plain = Style {
            fg: Color::rgb(220, 220, 220),
            bg: None,
            bold: false,
            italic: false,
            font_scale: 1.0,
        };
        EditorLine {
            raw_text: "let x = width; // note".into(),
            gutter_text: "  1".into(),
            spans: vec![
                EditorStyledSpan {
                    start_byte: 0,
                    end_byte: 3,
                    style: Style {
                        bold: true,
                        ..plain
                    },
                }, // "let"
                EditorStyledSpan {
                    start_byte: 8,
                    end_byte: 13,
                    style: Style {
                        italic: true,
                        ..plain
                    },
                }, // "width"
                EditorStyledSpan {
                    start_byte: 15,
                    end_byte: 22,
                    style: Style {
                        font_scale: 1.8,
                        ..plain
                    },
                }, // "// note"
            ],
            line_idx: 0,
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

    fn headless_pango_layout() -> pango::Layout {
        let surface = ImageSurface::create(Format::ARgb32, 900, 60).expect("create ImageSurface");
        let cr = Context::new(&surface).expect("Context::new");
        let ctx = pangocairo::functions::create_context(&cr);
        ctx.set_font_description(&pango::FontDescription::from_string("Monospace 12"));
        pango::Layout::new(&ctx)
    }

    /// Paint a styled, horizontally-scrolled line, then assert
    /// `editor_col_at_x` resolves every visible glyph's own painted x
    /// position back to its own column. A naive uniform-monospace
    /// resolver fails this for any glyph painted after the bold/italic/
    /// `font_scale` spans, since their advances differ from a
    /// plain-glyph cell width.
    #[test]
    fn editor_col_at_x_round_trips_styled_line_with_scroll() {
        // Two independent `pango::Layout`s from the same font context —
        // mirrors production, where the "ground truth" paint layout
        // (`draw_editor`'s frame-scoped layout) and the click-time probe
        // layout (`editor_col_at_x`'s argument, possibly the cached
        // clone) are distinct GObjects. Using one shared, mutable layout
        // for both would let `editor_col_at_x`'s internal `set_text` /
        // `set_attributes` calls silently overwrite the "ground truth"
        // measurements taken later in the loop — which is exactly what
        // happened when this test was first written with one shared
        // layout: it kept passing even with `set_attributes(None)`
        // hard-coded into `editor_col_at_x`, because the corrupted
        // shared state made "expected" and "actual" agree for the wrong
        // reason. Two layouts closes that hole.
        let truth_layout = headless_pango_layout();
        let probe_layout = headless_pango_layout();
        let line = styled_line();
        let editor = Editor {
            id: "ed".into(),
            rect: Rect::new(0.0, 0.0, 900.0, 60.0),
            lines: vec![line.clone()],
            cursor: None,
            extra_cursors: Vec::new(),
            selection: None,
            extra_selections: Vec::new(),
            yank_highlight: None,
            scroll_top: 0,
            scroll_left: 3,
            total_lines: 1,
            max_col: 30,
            gutter_char_width: 4,
            is_active: true,
            show_active_bg: false,
            has_git_diff: false,
            has_breakpoints: false,
            diagnostic_gutter: HashMap::new(),
            code_action_lines: HashSet::new(),
            bracket_match_positions: Vec::new(),
            active_indent_col: None,
            tabstop: 4,
            cursorline: true,
            lightbulb_glyph: '!',
            paint_options: Default::default(),
        };

        let cell_width = 9.0_f32;
        let line_height = 18.0_f32;
        let viewport = Rect::new(0.0, 0.0, 900.0, 60.0);
        let editor_layout = editor.layout(viewport, cell_width, line_height);

        // Pass 1 — paint with attrs (identical setup to `draw_editor`'s
        // per-line loop) into `truth_layout` and record every glyph's
        // real painted x position *before* touching `probe_layout` at
        // all, so nothing later can contaminate these measurements.
        truth_layout.set_text(&line.raw_text);
        let attrs = build_pango_attrs(&line.spans);
        truth_layout.set_attributes(Some(&attrs));

        let text_x_offset =
            editor_layout.text_bounds.x as f64 - editor.scroll_left as f64 * cell_width as f64;

        let glyph_click_xs: Vec<(usize, char, f64)> = line
            .raw_text
            .char_indices()
            .enumerate()
            .map(|(char_idx, (byte_idx, ch))| {
                let pos = truth_layout.index_to_pos(byte_idx as i32);
                let glyph_left_x = text_x_offset + pos.x() as f64 / pango::SCALE as f64;
                let glyph_width = (pos.width() as f64 / pango::SCALE as f64).max(2.0);
                // Click a quarter-glyph past the leading edge — solidly
                // inside the glyph, clear of boundary-rounding ambiguity.
                (char_idx, ch, glyph_left_x + glyph_width * 0.25)
            })
            .collect();

        // Pass 2 — resolve each recorded click through the function
        // under test, which owns (and mutates) `probe_layout`.
        for (char_idx, ch, click_x) in glyph_click_xs {
            let resolved = editor_col_at_x(&probe_layout, &line, &editor_layout, click_x as f32);
            assert_eq!(
                resolved, char_idx,
                "glyph '{ch}' (char {char_idx}) at x={click_x} resolved to col {resolved}"
            );
        }
    }

    #[test]
    fn editor_col_at_x_adds_segment_col_offset_for_wrap_continuation() {
        let pango_layout = headless_pango_layout();
        let mut line = styled_line();
        line.is_wrap_continuation = true;
        line.segment_col_offset = 12;

        let editor = Editor {
            id: "ed".into(),
            rect: Rect::new(0.0, 0.0, 900.0, 60.0),
            lines: vec![line.clone()],
            cursor: None,
            extra_cursors: Vec::new(),
            selection: None,
            extra_selections: Vec::new(),
            yank_highlight: None,
            scroll_top: 0,
            scroll_left: 0,
            total_lines: 1,
            max_col: 30,
            gutter_char_width: 4,
            is_active: true,
            show_active_bg: false,
            has_git_diff: false,
            has_breakpoints: false,
            diagnostic_gutter: HashMap::new(),
            code_action_lines: HashSet::new(),
            bracket_match_positions: Vec::new(),
            active_indent_col: None,
            tabstop: 4,
            cursorline: true,
            lightbulb_glyph: '!',
            paint_options: Default::default(),
        };

        let cell_width = 9.0_f32;
        let viewport = Rect::new(0.0, 0.0, 900.0, 60.0);
        let editor_layout = editor.layout(viewport, cell_width, 18.0);

        // Click exactly at the text origin (col 0 of the visual segment)
        // — the resolved buffer column must be the segment offset.
        let resolved = editor_col_at_x(
            &pango_layout,
            &line,
            &editor_layout,
            editor_layout.text_bounds.x,
        );
        assert_eq!(resolved, 12);
    }

    /// When `editor_col_at_x`'s clamped byte index lands mid-character
    /// in a multibyte line (é / CJK / emoji), it must be snapped to a
    /// char boundary. Sweep the full width of a multibyte line and
    /// confirm every probe resolves without panicking.
    #[test]
    fn editor_col_at_x_multibyte_line_does_not_panic() {
        let pango_layout = headless_pango_layout();
        let mut line = styled_line();
        line.raw_text = "café 🎉 中文 done".into();
        line.spans = Vec::new();

        let editor = Editor {
            id: "ed".into(),
            rect: Rect::new(0.0, 0.0, 900.0, 60.0),
            lines: vec![line.clone()],
            cursor: None,
            extra_cursors: Vec::new(),
            selection: None,
            extra_selections: Vec::new(),
            yank_highlight: None,
            scroll_top: 0,
            scroll_left: 0,
            total_lines: 1,
            max_col: 30,
            gutter_char_width: 4,
            is_active: true,
            show_active_bg: false,
            has_git_diff: false,
            has_breakpoints: false,
            diagnostic_gutter: HashMap::new(),
            code_action_lines: HashSet::new(),
            bracket_match_positions: Vec::new(),
            active_indent_col: None,
            tabstop: 4,
            cursorline: true,
            lightbulb_glyph: '!',
            paint_options: Default::default(),
        };

        let cell_width = 9.0_f32;
        let viewport = Rect::new(0.0, 0.0, 900.0, 60.0);
        let editor_layout = editor.layout(viewport, cell_width, 18.0);

        // Sweep x from before the line start to well past its end —
        // every probe must resolve without panicking.
        let mut x = editor_layout.text_bounds.x - 5.0;
        while x < editor_layout.text_bounds.x + 300.0 {
            let _ = editor_col_at_x(&pango_layout, &line, &editor_layout, x);
            x += 1.0;
        }
    }
}
