//! Backend-free editor geometry: [`EditorLayout`], its hit-testing,
//! and the [`Editor::layout`] family that resolves it.

use super::{Editor, EditorLine};
use crate::event::Rect;
use crate::paint_surface::{PaintSurface, TextRunStyle};
use crate::text_util::{
    char_cell_width, char_col_to_visual, char_to_byte_idx, snap_to_char_boundary,
};
use crate::types::Color;
use crate::FontRole;
use serde::{Deserialize, Serialize};

// ─── EditorPaintOptions ─────────────────────────────────────────────────────

/// Geometry options an [`Editor`] carries on its descriptor
/// ([`Editor::paint_options`]): which scrollbar to reserve, how wide, and
/// what strip to leave free beside the text. [`crate::Primitive::layout`]
/// reads them, so the layout a host hit-tests is the one paint used.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default)]
pub struct EditorPaintOptions {
    /// Suppress this editor's own vertical scrollbar even though the
    /// buffer overflows the viewport.
    ///
    /// A host running a scrollbar-style minimap beside the editor has no
    /// other way to stop the editor from reserving and painting a
    /// `v_scrollbar_bounds` column immediately next to it. Passing
    /// `suppress_v_scrollbar: true` makes `has_v_scrollbar` return `false`,
    /// so `v_scrollbar_bounds` is `None` and neither `EditorLayout::hit_test`
    /// nor the painters reserve or paint that column. Leave at `false`
    /// (the default) for editors with no minimap, or one that isn't
    /// acting as a scrollbar.
    pub suppress_v_scrollbar: bool,
    /// Width of the vertical scrollbar column on a pixel surface, in the
    /// surface's own units — see
    /// [`Editor::layout_with_options_and_v_scrollbar_w`]. `None` sizes it
    /// at one character advance. A cell grid always uses one cell.
    pub v_scrollbar_w: Option<f32>,
    /// Width of the strip reserved between the text area and the vertical
    /// scrollbar, in the surface's own units — see
    /// [`Editor::layout_with_options_and_reserve`].
    pub right_reserve_w: f32,
}

// ─── EditorLayout + hit_test ──────────────────────────────────────────────────

/// Classification of a hit-test result within an editor viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum EditorHit {
    /// Click landed on a text position. `line` is the buffer line index
    /// (always accurate — row height is uniform on every backend).
    ///
    /// From [`EditorLayout::hit_test`], `col` is the cell column under
    /// the point (`(x - text_bounds.x) / cell_width`, plus `scroll_left`),
    /// which knows nothing of the row's text. The editor's own pointer
    /// events carry the text column instead, resolved against the painted
    /// glyphs by [`EditorLayout::col_at_x`].
    BufferPos { line: usize, col: usize },
    /// Click landed in the gutter region.
    Gutter { line_idx: usize, gutter_col: usize },
    /// Click landed on the vertical scrollbar track.
    VScrollbar,
    /// Click landed on the horizontal scrollbar track.
    HScrollbar,
    /// Click landed outside any meaningful region.
    Empty,
}

/// Fully-resolved editor geometry for a single viewport. Produced by
/// [`Editor::layout`] and used by both rasterisers and click handlers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditorLayout {
    /// Full editor bounds (the rect passed to layout).
    pub bounds: Rect,
    /// Gutter region (line numbers, git, diagnostics). `None` when
    /// `gutter_char_width == 0`.
    pub gutter_bounds: Option<Rect>,
    /// Code content region (text area).
    pub text_bounds: Rect,
    /// Vertical scrollbar track. `None` when content fits.
    pub v_scrollbar_bounds: Option<Rect>,
    /// Horizontal scrollbar track. `None` when content fits.
    pub h_scrollbar_bounds: Option<Rect>,
    /// Cell dimensions used for coordinate-to-position conversion.
    pub cell_width: f32,
    pub line_height: f32,
    /// Scroll offsets for buffer-position calculation.
    pub scroll_top: usize,
    pub scroll_left: usize,
    /// Number of visible text rows.
    pub visible_lines: usize,
    /// Number of visible text columns.
    pub visible_cols: usize,
}

impl EditorLayout {
    /// Resolve an absolute point to an editor zone with buffer-relative
    /// positions.
    ///
    /// Zone classification (gutter / scrollbar / text-area) and the
    /// resolved `line` are always accurate. The `col` on the returned
    /// [`EditorHit::BufferPos`] is the cell column under the point; the
    /// text column under it is [`Self::col_at_x`].
    pub fn hit_test(&self, x: f32, y: f32) -> EditorHit {
        if x < self.bounds.x
            || x >= self.bounds.x + self.bounds.width
            || y < self.bounds.y
            || y >= self.bounds.y + self.bounds.height
        {
            return EditorHit::Empty;
        }

        if let Some(vsb) = self.v_scrollbar_bounds {
            if x >= vsb.x && x < vsb.x + vsb.width && y >= vsb.y && y < vsb.y + vsb.height {
                return EditorHit::VScrollbar;
            }
        }

        if let Some(hsb) = self.h_scrollbar_bounds {
            if x >= hsb.x && x < hsb.x + hsb.width && y >= hsb.y && y < hsb.y + hsb.height {
                return EditorHit::HScrollbar;
            }
        }

        if let Some(g) = self.gutter_bounds {
            if x >= g.x && x < g.x + g.width && y >= g.y && y < g.y + g.height {
                let row = ((y - g.y) / self.line_height).floor() as usize;
                let line_idx = self.scroll_top + row;
                let gutter_col = ((x - g.x) / self.cell_width).floor() as usize;
                return EditorHit::Gutter {
                    line_idx,
                    gutter_col,
                };
            }
        }

        if x >= self.text_bounds.x
            && x < self.text_bounds.x + self.text_bounds.width
            && y >= self.text_bounds.y
            && y < self.text_bounds.y + self.text_bounds.height
        {
            let row = ((y - self.text_bounds.y) / self.line_height).floor() as usize;
            let col_offset = ((x - self.text_bounds.x) / self.cell_width).floor() as usize;
            let line = self.scroll_top + row;
            let col = self.scroll_left + col_offset;
            return EditorHit::BufferPos { line, col };
        }

        EditorHit::Empty
    }

    /// Resolve absolute `x` to a character column within
    /// `editor.lines[view_row]`: the column of the glyph whose advance box
    /// contains `x`, with columns past the end of the text continuing at
    /// [`Self::cell_width`] each.
    ///
    /// `text` measures the row the way the pixel painter shapes it: its
    /// editor-font text verbs ([`PaintSurface::surface_text_x_at_index`])
    /// with the row's span weight, slant and scale applied, so proportional
    /// fonts and wide glyphs (emoji, CJK) resolve to the glyph actually
    /// painted under `x`. A host passes [`crate::Backend::paint_surface`];
    /// the measurement runs only for this call, shaping nothing per frame.
    /// `None` (a cell grid, or no surface in reach) measures in display
    /// cells of `cell_width`: a wide glyph spans two cells and a tab runs
    /// to the next [`Editor::tabstop`] stop, as the cell painter lays them
    /// out.
    ///
    /// The returned column folds in the row's `segment_col_offset` (0 for
    /// non-wrapped lines and the first visual segment of a wrapped line),
    /// so it is directly usable as the buffer-line-relative column. A
    /// `view_row` past `editor.lines` measures as an empty row.
    pub fn col_at_x(
        &self,
        editor: &Editor,
        view_row: usize,
        x: f32,
        text: Option<&mut dyn PaintSurface>,
    ) -> usize {
        let row = RowText::new(editor, view_row);
        let x = x - self.text_origin_x();
        let col = self.measure_row(&row, text, |x_at| {
            col_containing_x(row.text, x, self.cell_width, x_at)
        });
        col + row.segment_col_offset
    }

    /// Resolve a character column within `editor.lines[view_row]` to the
    /// absolute `x` of its glyph's left edge — the inverse of
    /// [`Self::col_at_x`], measured the same way, so the caret the pixel
    /// painter places at `col` sits at this `x` and `col_at_x` of it
    /// returns `col`. Columns past the end of the text continue at
    /// [`Self::cell_width`] each.
    pub fn x_at_col(
        &self,
        editor: &Editor,
        view_row: usize,
        col: usize,
        text: Option<&mut dyn PaintSurface>,
    ) -> f32 {
        let row = RowText::new(editor, view_row);
        let local = col.saturating_sub(row.segment_col_offset);
        let in_text = local.min(row.text.chars().count());
        let byte = char_to_byte_idx(row.text, in_text);
        let x = self.measure_row(&row, text, |x_at| x_at(byte));
        self.text_origin_x() + x + (local - in_text) as f32 * self.cell_width
    }

    /// Where column 0 of a row's text sits: the text area's left edge,
    /// moved left by the horizontal scroll.
    fn text_origin_x(&self) -> f32 {
        self.text_bounds.x - self.scroll_left as f32 * self.cell_width
    }

    /// Run `f` with `row`'s advance at a byte index: shaped through
    /// `surface`'s editor font with the row's runs, or in display cells
    /// without one.
    fn measure_row<R>(
        &self,
        row: &RowText,
        surface: Option<&mut dyn PaintSurface>,
        f: impl FnOnce(&mut dyn FnMut(usize) -> f32) -> R,
    ) -> R {
        let Some(surface) = surface else {
            let (text, tabstop, cw) = (row.text, row.tabstop, self.cell_width);
            return f(&mut |byte| {
                let col = text[..snap_to_char_boundary(text, byte)].chars().count();
                char_col_to_visual(text, col, tabstop) as f32 * cw
            });
        };
        let mut f = Some(f);
        let mut out = None;
        crate::backend::paint_in_role(surface, FontRole::Editor, &mut |s| {
            if let Some(f) = f.take() {
                out = Some(f(&mut |byte| {
                    s.surface_text_x_at_index(row.text, &row.runs, FontRole::Editor, byte)
                }));
            }
        });
        out.expect("paint_in_role runs its callback exactly once")
    }
}

/// One editor row's text as the painters lay it out: its characters
/// without a line terminator, the runs its spans style, and its tab stop.
struct RowText<'a> {
    text: &'a str,
    runs: Vec<TextRunStyle>,
    tabstop: usize,
    segment_col_offset: usize,
}

impl<'a> RowText<'a> {
    /// `editor.lines[view_row]`, or an empty row when there is none.
    fn new(editor: &'a Editor, view_row: usize) -> Self {
        let line = editor.lines.get(view_row);
        Self {
            text: line.map_or("", |l| l.raw_text.trim_end_matches(['\n', '\r'])),
            runs: line.map_or_else(Vec::new, |l| line_runs(l, Color::rgb(0, 0, 0))),
            tabstop: editor.tabstop,
            segment_col_offset: line.map_or(0, |l| l.segment_col_offset),
        }
    }
}

/// `rl.raw_text` partitioned into runs: each byte takes the last span
/// covering it (colour, weight, slant and font scale), gaps take
/// `default_fg` unscaled.
pub(super) fn line_runs(rl: &EditorLine, default_fg: Color) -> Vec<TextRunStyle> {
    let text = rl.raw_text.as_str();
    let mut byte_style: Vec<(Color, bool, bool, f32)> =
        vec![(default_fg, false, false, 1.0); text.len()];
    for span in &rl.spans {
        let start = snap_to_char_boundary(text, span.start_byte);
        let end = snap_to_char_boundary(text, span.end_byte);
        let st = &span.style;
        for b in byte_style.iter_mut().take(end).skip(start) {
            *b = (st.fg, st.bold, st.italic, st.font_scale);
        }
    }
    let mut runs: Vec<TextRunStyle> = Vec::new();
    let mut start = 0usize;
    for i in 1..=text.len() {
        if i == text.len() || (text.is_char_boundary(i) && byte_style[i] != byte_style[start]) {
            let (fg, bold, italic, scale) = byte_style[start];
            runs.push(
                TextRunStyle::new(start..i, fg)
                    .with_bold(bold)
                    .with_italic(italic)
                    .with_scale(scale),
            );
            start = i;
        }
    }
    runs
}

/// The char column of `text` whose glyph spans `x` (measured from the
/// start of the text by `x_at`, a byte index's advance), or, past the
/// last glyph, the text's length plus whole `cell_width` steps beyond its
/// end. A zero-width char (a combining mark) never starts a column of its
/// own, so a click on a base character never lands between it and its
/// marks. Advances grow left to right, so the glyph is found by binary
/// search: a click shapes the row a logarithmic number of times.
fn col_containing_x(
    text: &str,
    x: f32,
    cell_width: f32,
    x_at: &mut dyn FnMut(usize) -> f32,
) -> usize {
    let end_x = x_at(text.len());
    if x >= end_x {
        let past = if cell_width > 0.0 {
            ((x - end_x) / cell_width).floor() as usize
        } else {
            0
        };
        return text.chars().count() + past;
    }
    let starts: Vec<usize> = text
        .char_indices()
        .filter(|&(i, ch)| i == 0 || char_cell_width(ch) != 0)
        .map(|(i, _)| i)
        .collect();
    let at_or_left = starts.partition_point(|&b| x_at(b) <= x);
    match at_or_left.checked_sub(1) {
        Some(i) => text[..starts[i]].chars().count(),
        None => 0,
    }
}

impl Editor {
    /// Compute the viewport geometry for hit-testing and rendering layout.
    ///
    /// # Arguments
    ///
    /// - `viewport` — the rectangle this editor occupies.
    /// - `cell_width` — character advance width (1.0 for TUI, font px for GTK).
    /// - `line_height` — row height (1.0 for TUI, font px for GTK).
    ///
    /// Equivalent to [`Self::layout_with_options`] with
    /// `EditorPaintOptions::default()` — kept as a separate, unchanged
    /// method (rather than growing this one's argument list) so every
    /// existing caller keeps compiling untouched. See
    /// [`EditorPaintOptions`] for why a new parameter here would be a
    /// breaking change for both known downstream consumers.
    pub fn layout(&self, viewport: Rect, cell_width: f32, line_height: f32) -> EditorLayout {
        self.layout_with_options(
            viewport,
            cell_width,
            line_height,
            EditorPaintOptions::default(),
        )
    }

    /// [`Self::layout`], plus [`EditorPaintOptions`] a host can set to
    /// override otherwise-automatic geometry decisions — currently just
    /// `suppress_v_scrollbar`.
    ///
    /// Equivalent to [`Self::layout_with_options_and_v_scrollbar_w`] with
    /// `v_scrollbar_w: None` (a `cell_width`-wide scrollbar column).
    pub fn layout_with_options(
        &self,
        viewport: Rect,
        cell_width: f32,
        line_height: f32,
        options: EditorPaintOptions,
    ) -> EditorLayout {
        self.layout_with_options_and_v_scrollbar_w(viewport, cell_width, line_height, options, None)
    }

    /// [`Self::layout_with_options`], plus a host-settable vertical
    /// scrollbar width in the same pixel units as `cell_width` /
    /// `line_height`.
    ///
    /// Without an override the vertical scrollbar column is exactly
    /// `cell_width` wide — right for TUI's fixed-cell grid, but on a GUI
    /// backend that ties the scrollbar's width to the font's glyph
    /// advance rather than to a fixed chrome dimension. VS Code's editor
    /// scrollbar is a constant 14px column independent of font size.
    /// `Some(px)` overrides just the scrollbar column's width (both
    /// `v_scrollbar_bounds.width` and the text-area narrowing it causes)
    /// while every other geometry calculation keeps using `cell_width`.
    /// `None` sizes the column at `cell_width`. Ignored (there is no
    /// column to size) when `options.suppress_v_scrollbar` is `true` or
    /// the buffer doesn't overflow the viewport.
    ///
    /// GUI hosts normally reach this through the editor's
    /// `paint_options.v_scrollbar_w` rather than calling it directly.
    ///
    /// Equivalent to [`Self::layout_with_options_and_reserve`] with
    /// `right_reserve_w: 0.0`.
    pub fn layout_with_options_and_v_scrollbar_w(
        &self,
        viewport: Rect,
        cell_width: f32,
        line_height: f32,
        options: EditorPaintOptions,
        v_scrollbar_w: Option<f32>,
    ) -> EditorLayout {
        self.layout_with_options_and_reserve(
            viewport,
            cell_width,
            line_height,
            options,
            v_scrollbar_w,
            0.0,
        )
    }

    /// [`Self::layout_with_options_and_v_scrollbar_w`], plus a fixed-width
    /// strip a caller can reserve between the text area and the vertical
    /// scrollbar.
    ///
    /// Every other tier of `layout*` derives `text_w`/`visible_cols` and
    /// the scrollbar bounds from `viewport.width` alone, so a host that
    /// wants to dock something between the buffer text and the scrollbar
    /// — a minimap strip, for example — has no way to shrink the text
    /// area without also shifting the vertical scrollbar off the
    /// viewport's true right edge. `right_reserve_w` (in the same pixel
    /// units as `cell_width`/`line_height`) is subtracted from the text
    /// area only: `text_bounds`/`h_scrollbar_bounds` narrow by it, while
    /// `v_scrollbar_bounds` keeps hugging `viewport.x + viewport.width`.
    /// The reserved strip itself isn't a layout field — a caller derives
    /// its bounds as the gap between `text_bounds`'s right edge and
    /// `v_scrollbar_bounds`'s left edge (or the viewport's right edge
    /// when there's no vertical scrollbar). `right_reserve_w: 0.0`
    /// reproduces [`Self::layout_with_options_and_v_scrollbar_w`] exactly.
    ///
    /// A negative `right_reserve_w` is clamped to `0.0`.
    pub fn layout_with_options_and_reserve(
        &self,
        viewport: Rect,
        cell_width: f32,
        line_height: f32,
        options: EditorPaintOptions,
        v_scrollbar_w: Option<f32>,
        right_reserve_w: f32,
    ) -> EditorLayout {
        let right_reserve_w = right_reserve_w.max(0.0);
        let gutter_w = self.gutter_char_width as f32 * cell_width;
        let visible_lines = if line_height > 0.0 {
            (viewport.height / line_height).floor() as usize
        } else {
            0
        };
        let has_v_scrollbar = !options.suppress_v_scrollbar
            && self.total_lines > visible_lines
            && viewport.width > gutter_w + cell_width + right_reserve_w;
        let v_scrollbar_w = if has_v_scrollbar {
            v_scrollbar_w.unwrap_or(cell_width)
        } else {
            0.0
        };

        let text_w = (viewport.width - gutter_w - v_scrollbar_w - right_reserve_w).max(0.0);
        let visible_cols = if cell_width > 0.0 {
            (text_w / cell_width).floor() as usize
        } else {
            0
        };
        let has_h_scrollbar = self.max_col > visible_cols && viewport.height > line_height;
        let h_scrollbar_h = if has_h_scrollbar { line_height } else { 0.0 };

        let text_h = (viewport.height - h_scrollbar_h).max(0.0);
        let visible_lines_final = if line_height > 0.0 {
            (text_h / line_height).floor() as usize
        } else {
            0
        };

        let gutter_bounds = if gutter_w > 0.0 {
            Some(Rect::new(viewport.x, viewport.y, gutter_w, text_h))
        } else {
            None
        };

        let text_x = viewport.x + gutter_w;
        let text_bounds = Rect::new(text_x, viewport.y, text_w, text_h);

        let v_scrollbar_bounds = if has_v_scrollbar {
            let v_track_h = text_h;
            Some(Rect::new(
                viewport.x + viewport.width - v_scrollbar_w,
                viewport.y,
                v_scrollbar_w,
                v_track_h,
            ))
        } else {
            None
        };

        let h_scrollbar_bounds = if has_h_scrollbar {
            let h_track_x = viewport.x + gutter_w;
            let h_track_w = text_w;
            Some(Rect::new(
                h_track_x,
                viewport.y + text_h,
                h_track_w,
                h_scrollbar_h,
            ))
        } else {
            None
        };

        EditorLayout {
            bounds: viewport,
            gutter_bounds,
            text_bounds,
            v_scrollbar_bounds,
            h_scrollbar_bounds,
            cell_width,
            line_height,
            scroll_top: self.scroll_top,
            scroll_left: self.scroll_left,
            visible_lines: visible_lines_final,
            visible_cols,
        }
    }
}
