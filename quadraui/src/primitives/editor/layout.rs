//! Backend-free editor geometry: [`EditorLayout`], its hit-testing,
//! and the [`Editor::layout`] family that resolves it.

use super::Editor;
use crate::event::Rect;
use serde::{Deserialize, Serialize};

// ─── EditorPaintOptions ─────────────────────────────────────────────────────

/// Geometry options an [`Editor`] carries on its descriptor
/// ([`Editor::paint_options`]): which scrollbar to reserve, how wide, and
/// what strip to leave free beside the text. [`crate::Primitive::layout`]
/// reads them, so the layout a host hit-tests is the one paint used.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
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
pub enum EditorHit {
    /// Click landed on a text position. `line` is the buffer line index
    /// (always accurate — row height is uniform on every backend).
    ///
    /// `col` is a **uniform-monospace approximation** of the character
    /// column (`(x - text_bounds.x) / cell_width`, plus `scroll_left`;
    /// no tab expansion). It matches the painted glyph exactly on TUI's
    /// fixed-cell grid, but can drift from the actual glyph on
    /// proportional-font or per-span-attributed backends (bold /
    /// italic / `font_scale` spans paint at a different advance than
    /// `cell_width`). Callers that need the exact column on those backends
    /// should discard this field and resolve via
    /// [`crate::Backend::editor_col_at_x`] instead (falls back to this
    /// same approximation on TUI).
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
    /// [`EditorHit::BufferPos`] is a uniform-monospace approximation —
    /// see that variant's doc and [`Self::col_at_x`] /
    /// [`crate::Backend::editor_col_at_x`] for the text-engine-accurate
    /// resolution proportional/attributed backends need.
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

    /// Resolve `x` to a character column within `editor.lines[view_row]`
    /// using uniform monospace division (`self.cell_width`).
    ///
    /// This is the default [`crate::Backend::editor_col_at_x`]
    /// implementation — correct for TUI's fixed-cell grid and any
    /// backend whose configured font renders every glyph at the same
    /// advance width. Backends that paint editor text with a real
    /// text-shaping engine and per-span attributes (bold / italic /
    /// `font_scale`) override `Backend::editor_col_at_x` with an exact
    /// glyph-position inverse of their `draw_editor` instead.
    ///
    /// The returned column already folds in `self.scroll_left` and the
    /// row's `segment_col_offset` (0 for non-wrapped lines and the
    /// first visual segment of a wrapped line), so it's directly usable
    /// as the buffer-line-relative column. Falls back to
    /// `self.scroll_left` alone (no segment offset) when `view_row` is
    /// out of range for `editor.lines`.
    pub fn col_at_x(&self, editor: &Editor, view_row: usize, x: f32) -> usize {
        let segment_col_offset = editor
            .lines
            .get(view_row)
            .map(|line| line.segment_col_offset)
            .unwrap_or(0);
        let col_offset = if self.cell_width > 0.0 {
            ((x - self.text_bounds.x) / self.cell_width)
                .floor()
                .max(0.0) as usize
        } else {
            0
        };
        self.scroll_left + col_offset + segment_col_offset
    }

    /// Resolve a character column within `editor.lines[view_row]` back to
    /// an absolute `x` — the exact inverse of [`Self::col_at_x`], so
    /// `col_at_x(x_at_col(editor, view_row, c)) == c` holds for any `c`
    /// reachable from a valid on-screen `x`.
    ///
    /// Like `col_at_x`, this is a uniform-monospace calculation
    /// (`self.cell_width`) with no tab expansion — correct for TUI's
    /// fixed-cell grid, and the default [`crate::Backend`] behaviour for
    /// any host that hasn't overridden the exact-glyph resolution. A host
    /// anchoring UI at a caret column (a popup, say) on a backend with
    /// per-span attributes or proportional fonts should resolve the exact
    /// pixel position through that backend's own text-shaping engine
    /// instead of this approximation.
    pub fn x_at_col(&self, editor: &Editor, view_row: usize, col: usize) -> f32 {
        let segment_col_offset = editor
            .lines
            .get(view_row)
            .map(|line| line.segment_col_offset)
            .unwrap_or(0);
        let col_offset = col.saturating_sub(self.scroll_left + segment_col_offset);
        self.text_bounds.x + col_offset as f32 * self.cell_width
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
    /// GUI hosts normally reach this through
    /// [`crate::Backend::set_editor_v_scrollbar_width`] rather than
    /// calling it directly.
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
