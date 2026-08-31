//! `Minimap` primitive: a code-overview minimap sitting alongside an
//! editor viewport (vimcode#35).
//!
//! Two backends paint the same [`Minimap`] with different *techniques*,
//! not different *behaviour* (issue #382):
//!
//! | backend | technique | density |
//! |---|---|---|
//! | GTK | font scaling — real glyphs at a scaled-down absolute Pango size | 1 buffer line per row |
//! | TUI | braille — `U+2800`-block dot cells | 4 buffer lines per row, 2 columns per cell |
//!
//! Both algorithms that make this possible — [`sample_lines`] (row
//! down-sampling) and [`aggregate_spans`] (colour down-sampling) — live
//! here, not in either backend, so the two rasterisers never re-derive
//! or re-reduce data the primitive already resolved.
//!
//! # Coordinate model
//!
//! [`Minimap::lines`] is the *fine-grained* list the app chose to show —
//! for GTK, typically one entry per rendered row; for TUI, four
//! consecutive entries are packed into one braille row. [`Minimap::layout`]
//! takes `lines_per_row` (the backend's own grouping factor: `1` for GTK,
//! `4` for TUI) and groups `lines` into that many rows, tiling
//! `bounds.height` evenly across them — this is what lets
//! [`MinimapLayout::visible_lines`].len() serve directly as the row count
//! the GTK rasteriser divides by for font-pitch (`line_px = bounds.height
//! / layout.visible_lines.len()`, see the rasteriser spec in #382).
//!
//! `visible_row_start` / `visible_row_count` describe the *editor's*
//! current viewport as an index range into `lines` (not a separate scroll
//! position for the minimap itself) — that's what [`MinimapLayout::viewport_highlight`]
//! outlines, and what [`Minimap::scroll_thumb`] converts into a
//! [`Scrollbar`] against [`Minimap::total_buffer_lines`] using each
//! [`MinimapLine::line_idx`] as the bridge back to real buffer line
//! numbers (sampling may skip lines, so a `lines`-relative fraction and a
//! `total_buffer_lines`-relative fraction are not the same thing whenever
//! sampling is non-uniform).

use crate::event::Rect;
use crate::primitives::scrollbar::Scrollbar;
use crate::types::{Color, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a `Minimap` widget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Minimap {
    pub id: WidgetId,
    /// Pre-aggregated lines (app does the sampling, e.g. via [`sample_lines`]).
    pub lines: Vec<MinimapLine>,
    /// Syntax colour spans, already aggregated to a dominant colour per
    /// *cell* by [`aggregate_spans`] — one backend-sized cell, not one
    /// buffer line.
    pub syntax_spans: Vec<MinimapSpan>,
    /// Index into `lines` at the top of the editor's current viewport.
    pub visible_row_start: usize,
    /// Number of entries in `lines` the editor's viewport currently shows.
    pub visible_row_count: usize,
    /// Total buffer lines (used to compute the scroll thumb).
    pub total_buffer_lines: usize,
}

/// One sampled line in a [`Minimap`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MinimapLine {
    pub text: String,
    /// Original buffer line number this entry stands in for.
    pub line_idx: usize,
}

/// One aggregated colour span — the *output* of [`aggregate_spans`], at
/// whatever cell granularity the target backend paints (one entry per
/// coloured cell, not per raw syntax token).
///
/// `line_idx` indexes into [`Minimap::lines`] (the same index space as
/// [`VisibleMinimapLine::start_line_idx`]), **not** the original buffer
/// line number — a rasteriser looking up the span for a painted row
/// matches directly against the row's `start_line_idx` with no reverse
/// mapping back through [`MinimapLine::line_idx`] needed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MinimapSpan {
    pub line_idx: usize,
    pub start_col: usize,
    pub end_col: usize,
    pub color: Color,
}

/// A raw syntax-highlight span at buffer granularity — the *input* to
/// [`aggregate_spans`]. Distinct from [`MinimapSpan`], the aggregated
/// output: many `SyntaxSpan`s can fold into one `MinimapSpan`.
///
/// `line_idx` uses the same [`Minimap::lines`]-index space as
/// [`MinimapSpan::line_idx`] (see that field's doc) — callers building
/// spans from raw buffer syntax highlighting map buffer line numbers to
/// `lines` indices themselves (mirroring how [`sample_lines`] already
/// requires the caller to pre-select which buffer lines are sampled).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SyntaxSpan {
    pub line_idx: usize,
    pub start_col: usize,
    pub end_col: usize,
    pub color: Color,
}

/// The region [`aggregate_spans`] folds raw [`SyntaxSpan`]s into.
///
/// `rows` / `cols` bound the output grid (spans outside it are dropped);
/// `lines_per_row` / `cols_per_cell` say how many buffer lines / columns
/// fold into one output cell. GTK uses `lines_per_row: 1, cols_per_cell:
/// 1` (no reduction — one cell per source character); TUI uses `4` / `2`
/// (one braille cell covers 4 lines × 2 columns, and can carry only one
/// foreground colour, so the reduction is load-bearing there).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MinimapGrid {
    pub rows: usize,
    pub cols: usize,
    pub lines_per_row: usize,
    pub cols_per_cell: usize,
}

/// One visible row after [`Minimap::layout`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleMinimapLine {
    /// Index into [`Minimap::lines`] where this row's line(s) begin.
    /// Backends read `lines_per_row` consecutive entries starting here.
    pub start_line_idx: usize,
    /// This row's bounds within [`MinimapLayout::bounds`].
    pub bounds: Rect,
}

/// Fully-resolved minimap layout. Both rasterisers consume this verbatim.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MinimapLayout {
    pub bounds: Rect,
    pub visible_lines: Vec<VisibleMinimapLine>,
    /// Where the editor's current viewport appears, in the same absolute
    /// coordinates as `bounds`.
    pub viewport_highlight: Rect,
    pub scrollbar: Option<Scrollbar>,
}

/// Classification of a minimap hit-test result.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MinimapHit {
    /// Click/drag landed on the track: seek to this fraction of the file.
    Seek { fraction: f32 },
    /// Click landed outside the minimap's bounds.
    None,
}

impl MinimapLayout {
    /// Hit-test a click/drag against the minimap track. Any point inside
    /// `bounds` resolves to [`MinimapHit::Seek`] with `fraction` in
    /// `[0.0, 1.0]` — the app seeks its scroll engine to that fraction of
    /// the file. Outside `bounds` resolves to [`MinimapHit::None`].
    pub fn hit_test(&self, x: f32, y: f32) -> MinimapHit {
        let b = &self.bounds;
        if b.height <= 0.0 || x < b.x || x >= b.x + b.width || y < b.y || y >= b.y + b.height {
            return MinimapHit::None;
        }
        let fraction = ((y - b.y) / b.height).clamp(0.0, 1.0);
        MinimapHit::Seek { fraction }
    }
}

impl Minimap {
    /// Compute layout + hit regions. `lines_per_row` is the backend's
    /// grouping factor (`1` for GTK, `4` for TUI) — see the module docs
    /// for why layout needs it but painting-only metrics (font size,
    /// dot density) don't.
    pub fn layout(&self, bounds: Rect, lines_per_row: usize) -> MinimapLayout {
        let lines_per_row = lines_per_row.max(1);

        if self.lines.is_empty() || bounds.width <= 0.0 || bounds.height <= 0.0 {
            return MinimapLayout {
                bounds,
                visible_lines: Vec::new(),
                viewport_highlight: Rect::new(bounds.x, bounds.y, 0.0, 0.0),
                scrollbar: None,
            };
        }

        let row_count = self.lines.len().div_ceil(lines_per_row);
        let row_h = bounds.height / row_count as f32;

        let visible_lines: Vec<VisibleMinimapLine> = (0..row_count)
            .map(|r| VisibleMinimapLine {
                start_line_idx: r * lines_per_row,
                bounds: Rect::new(bounds.x, bounds.y + row_h * r as f32, bounds.width, row_h),
            })
            .collect();

        let start_row = (self.visible_row_start / lines_per_row) as f32;
        let end_line = (self.visible_row_start + self.visible_row_count).min(self.lines.len());
        let end_row = if end_line == 0 {
            0.0
        } else {
            ((end_line - 1) / lines_per_row) as f32 + 1.0
        };
        let highlight_h = (end_row - start_row).max(0.0) * row_h;
        let viewport_highlight = Rect::new(
            bounds.x,
            bounds.y + start_row * row_h,
            bounds.width,
            highlight_h,
        );

        let scrollbar = self.scroll_thumb();

        MinimapLayout {
            bounds,
            visible_lines,
            viewport_highlight,
            scrollbar,
        }
    }

    /// Scroll-thumb geometry for the editor's viewport within the whole
    /// file, bridging `lines`-relative indices back to real buffer line
    /// numbers via each [`MinimapLine::line_idx`]. `None` when the whole
    /// file already fits (nothing to scroll).
    fn scroll_thumb(&self) -> Option<Scrollbar> {
        if self.total_buffer_lines == 0 || self.lines.is_empty() {
            return None;
        }
        let start_buffer_line = self
            .lines
            .get(self.visible_row_start)
            .map(|l| l.line_idx)
            .unwrap_or(0);
        let end_idx = (self.visible_row_start + self.visible_row_count).min(self.lines.len());
        let end_buffer_line = if end_idx == 0 {
            start_buffer_line
        } else {
            self.lines
                .get(end_idx - 1)
                .map(|l| l.line_idx + 1)
                .unwrap_or(self.total_buffer_lines)
        };
        let visible = end_buffer_line.saturating_sub(start_buffer_line).max(1) as f32;
        let total = self.total_buffer_lines as f32;
        if total <= visible {
            return None;
        }
        // Track geometry is resolved by the caller (paint time) via
        // `Scrollbar`'s own fields; here we only need `fit_thumb`'s
        // scroll/total/visible inputs, so an empty zero-sized track is
        // fine — callers that want on-screen thumb pixels recompute
        // `track` from their own bounds if they choose to paint this.
        Some(Scrollbar::vertical(
            format!("{}-scrollbar", self.id.0),
            Rect::new(0.0, 0.0, 0.0, 0.0),
            start_buffer_line as f32,
            total,
            visible,
            1.0,
        ))
    }
}

/// Compress `buffer_lines` into at most `target_rows` [`MinimapLine`]s
/// with a configurable stride. Never upscales: when `buffer_lines.len()
/// <= target_rows`, every line is kept as-is (one [`MinimapLine`] per
/// buffer line) rather than manufacturing extra rows. `target_rows == 0`
/// or an empty buffer returns an empty `Vec` (no divide-by-zero).
pub fn sample_lines(buffer_lines: &[&str], target_rows: usize) -> Vec<MinimapLine> {
    if buffer_lines.is_empty() || target_rows == 0 {
        return Vec::new();
    }
    if buffer_lines.len() <= target_rows {
        return buffer_lines
            .iter()
            .enumerate()
            .map(|(i, &text)| MinimapLine {
                text: text.to_string(),
                line_idx: i,
            })
            .collect();
    }
    let stride = buffer_lines.len() as f64 / target_rows as f64;
    (0..target_rows)
        .map(|r| {
            let idx = ((r as f64 * stride) as usize).min(buffer_lines.len() - 1);
            MinimapLine {
                text: buffer_lines[idx].to_string(),
                line_idx: idx,
            }
        })
        .collect()
}

/// Fold raw [`SyntaxSpan`]s into a per-cell dominant colour, at whatever
/// granularity `grid` describes (see [`MinimapGrid`]).
///
/// Each input span contributes its column length as weight to every
/// output cell it touches; the surviving colour per cell is whichever
/// accumulated the most weight, ties broken by *last* span seen (so the
/// result is a pure function of `spans`' order, not of hashing — no
/// dependency on iteration order of an internal map).
///
/// Returns one [`MinimapSpan`] per non-empty cell, sorted by
/// `(line_idx, start_col)`. Cells with no overlapping span are omitted —
/// callers fall back to a default colour when painting.
pub fn aggregate_spans(spans: &[SyntaxSpan], grid: MinimapGrid) -> Vec<MinimapSpan> {
    if grid.rows == 0 || grid.cols == 0 || grid.lines_per_row == 0 || grid.cols_per_cell == 0 {
        return Vec::new();
    }

    // `(row, col) -> [(color, weight), ...]` in first-seen order, so
    // `max_by_key` ties resolve deterministically (it returns the *last*
    // maximal element) rather than depending on hash iteration order.
    let mut hist: std::collections::HashMap<(usize, usize), Vec<(Color, usize)>> =
        std::collections::HashMap::new();

    for span in spans {
        if span.end_col <= span.start_col {
            continue;
        }
        let row = span.line_idx / grid.lines_per_row;
        if row >= grid.rows {
            continue;
        }
        let weight = span.end_col - span.start_col;
        let col_start = span.start_col / grid.cols_per_cell;
        let col_end = (span.end_col - 1) / grid.cols_per_cell;
        for col in col_start..=col_end {
            if col >= grid.cols {
                break;
            }
            let entry = hist.entry((row, col)).or_default();
            match entry.iter_mut().find(|(c, _)| *c == span.color) {
                Some((_, w)) => *w += weight,
                None => entry.push((span.color, weight)),
            }
        }
    }

    let mut out: Vec<MinimapSpan> = hist
        .into_iter()
        .filter_map(|((row, col), colors)| {
            colors
                .into_iter()
                .max_by_key(|(_, w)| *w)
                .map(|(color, _)| MinimapSpan {
                    line_idx: row * grid.lines_per_row,
                    start_col: col * grid.cols_per_cell,
                    end_col: (col + 1) * grid.cols_per_cell,
                    color,
                })
        })
        .collect();
    out.sort_by_key(|s| (s.line_idx, s.start_col));
    out
}

/// Width the minimap reserves alongside the editor. `0.0` when there is
/// no minimap to draw — the app's on/off setting (vimcode#35) toggles
/// the *presence* of the [`Minimap`], and both backends reclaim the
/// editor width by calling this with `has_minimap: false`.
pub fn reserved_width(cols_or_px: f32, has_minimap: bool) -> f32 {
    if has_minimap {
        cols_or_px
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(n: usize) -> Vec<MinimapLine> {
        (0..n)
            .map(|i| MinimapLine {
                text: format!("line{i}"),
                line_idx: i,
            })
            .collect()
    }

    fn minimap(n: usize, visible_row_start: usize, visible_row_count: usize) -> Minimap {
        Minimap {
            id: WidgetId::new("mm"),
            lines: lines(n),
            syntax_spans: Vec::new(),
            visible_row_start,
            visible_row_count,
            total_buffer_lines: n,
        }
    }

    // ── layout geometry ─────────────────────────────────────────────

    #[test]
    fn layout_tiles_rows_evenly_across_bounds() {
        let mm = minimap(8, 2, 3);
        let bounds = Rect::new(0.0, 0.0, 10.0, 40.0);
        let layout = mm.layout(bounds, 2); // 8 lines / 2 per row = 4 rows
        assert_eq!(layout.visible_lines.len(), 4);
        assert_eq!(
            layout.visible_lines[0].bounds,
            Rect::new(0.0, 0.0, 10.0, 10.0)
        );
        assert_eq!(
            layout.visible_lines[1].bounds,
            Rect::new(0.0, 10.0, 10.0, 10.0)
        );
        assert_eq!(
            layout.visible_lines[3].bounds,
            Rect::new(0.0, 30.0, 10.0, 10.0)
        );
        assert_eq!(layout.visible_lines[3].start_line_idx, 6);
    }

    #[test]
    fn layout_viewport_highlight_spans_the_editor_viewport() {
        // visible_row_start=2, visible_row_count=3 -> lines[2..5), rows
        // grouped 2-per-row -> row 1 (start) through row 3 (exclusive).
        let mm = minimap(8, 2, 3);
        let bounds = Rect::new(0.0, 0.0, 10.0, 40.0);
        let layout = mm.layout(bounds, 2);
        assert_eq!(layout.viewport_highlight, Rect::new(0.0, 10.0, 10.0, 20.0));
    }

    #[test]
    fn layout_empty_lines_is_a_no_op() {
        let mm = minimap(0, 0, 0);
        let layout = mm.layout(Rect::new(0.0, 0.0, 10.0, 40.0), 4);
        assert!(layout.visible_lines.is_empty());
        assert_eq!(layout.viewport_highlight.height, 0.0);
    }

    #[test]
    fn layout_zero_size_bounds_is_a_no_op() {
        let mm = minimap(8, 0, 4);
        let layout = mm.layout(Rect::new(0.0, 0.0, 0.0, 0.0), 4);
        assert!(layout.visible_lines.is_empty());
    }

    // ── hit_test ─────────────────────────────────────────────────────

    #[test]
    fn hit_test_top_middle_bottom_of_track() {
        let layout = MinimapLayout {
            bounds: Rect::new(0.0, 0.0, 10.0, 10.0),
            visible_lines: Vec::new(),
            viewport_highlight: Rect::default(),
            scrollbar: None,
        };
        assert_eq!(
            layout.hit_test(5.0, 0.0),
            MinimapHit::Seek { fraction: 0.0 }
        );
        assert_eq!(
            layout.hit_test(5.0, 5.0),
            MinimapHit::Seek { fraction: 0.5 }
        );
        assert_eq!(
            layout.hit_test(5.0, 9.0),
            MinimapHit::Seek { fraction: 0.9 }
        );
    }

    #[test]
    fn hit_test_outside_bounds_is_none() {
        let layout = MinimapLayout {
            bounds: Rect::new(10.0, 10.0, 10.0, 10.0),
            visible_lines: Vec::new(),
            viewport_highlight: Rect::default(),
            scrollbar: None,
        };
        assert_eq!(layout.hit_test(0.0, 0.0), MinimapHit::None);
        assert_eq!(layout.hit_test(25.0, 15.0), MinimapHit::None);
    }

    // ── sample_lines ─────────────────────────────────────────────────

    #[test]
    fn sample_lines_empty_buffer_is_empty() {
        assert!(sample_lines(&[], 5).is_empty());
    }

    #[test]
    fn sample_lines_zero_target_rows_is_empty_no_div_by_zero() {
        assert!(sample_lines(&["a", "b", "c"], 0).is_empty());
    }

    #[test]
    fn sample_lines_never_upscales_small_files() {
        // 2 buffer lines, target 10 rows: keep exactly 2, not 10.
        let out = sample_lines(&["a", "b"], 10);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].line_idx, 0);
        assert_eq!(out[1].line_idx, 1);
    }

    #[test]
    fn sample_lines_downsamples_large_files_to_target_rows() {
        let owned: Vec<String> = (0..100).map(|i| format!("l{i}")).collect();
        let borrowed: Vec<&str> = owned.iter().map(String::as_str).collect();
        let out = sample_lines(&borrowed, 10);
        assert_eq!(out.len(), 10);
        assert_eq!(out[0].line_idx, 0);
        // Monotonically increasing source line indices.
        assert!(out.windows(2).all(|w| w[0].line_idx < w[1].line_idx));
    }

    // ── aggregate_spans ────────────────────────────────────────────

    fn red() -> Color {
        Color::rgb(255, 0, 0)
    }
    fn blue() -> Color {
        Color::rgb(0, 0, 255)
    }

    /// 4 lines x 4 columns: line0 is half red / half blue; lines 1-3 are
    /// solid red, blue, blue. Shared across the GTK (1x1) and TUI (4x2)
    /// grid tests so both exercise the exact same input.
    fn sample_spans() -> Vec<SyntaxSpan> {
        vec![
            SyntaxSpan {
                line_idx: 0,
                start_col: 0,
                end_col: 2,
                color: red(),
            },
            SyntaxSpan {
                line_idx: 0,
                start_col: 2,
                end_col: 4,
                color: blue(),
            },
            SyntaxSpan {
                line_idx: 1,
                start_col: 0,
                end_col: 4,
                color: red(),
            },
            SyntaxSpan {
                line_idx: 2,
                start_col: 0,
                end_col: 4,
                color: blue(),
            },
            SyntaxSpan {
                line_idx: 3,
                start_col: 0,
                end_col: 4,
                color: blue(),
            },
        ]
    }

    #[test]
    fn aggregate_spans_gtk_grid_is_per_line_no_reduction() {
        let grid = MinimapGrid {
            rows: 4,
            cols: 4,
            lines_per_row: 1,
            cols_per_cell: 1,
        };
        let out = aggregate_spans(&sample_spans(), grid);
        // Row 0 keeps both colours side by side: no cross-line mixing.
        assert!(out.contains(&MinimapSpan {
            line_idx: 0,
            start_col: 0,
            end_col: 1,
            color: red()
        }));
        assert!(out.contains(&MinimapSpan {
            line_idx: 0,
            start_col: 2,
            end_col: 3,
            color: blue()
        }));
        assert!(out.contains(&MinimapSpan {
            line_idx: 1,
            start_col: 0,
            end_col: 1,
            color: red()
        }));
    }

    #[test]
    fn aggregate_spans_tui_grid_folds_four_lines_into_one_cell() {
        let grid = MinimapGrid {
            rows: 1,
            cols: 2,
            lines_per_row: 4,
            cols_per_cell: 2,
        };
        let out = aggregate_spans(&sample_spans(), grid);
        // Cell (row 0, col 0) covers lines 0-3, cols 0-1: red contributes
        // weight 2 (line0) + 4 (line1) = 6; blue contributes weight
        // 4 (line2) + 4 (line3) = 8 -> blue wins, unlike the GTK grid
        // above where the same source pixels were red.
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0],
            MinimapSpan {
                line_idx: 0,
                start_col: 0,
                end_col: 2,
                color: blue()
            }
        );
        assert_eq!(
            out[1],
            MinimapSpan {
                line_idx: 0,
                start_col: 2,
                end_col: 4,
                color: blue()
            }
        );
    }

    #[test]
    fn aggregate_spans_empty_grid_dims_return_empty() {
        let grid = MinimapGrid {
            rows: 0,
            cols: 4,
            lines_per_row: 1,
            cols_per_cell: 1,
        };
        assert!(aggregate_spans(&sample_spans(), grid).is_empty());
    }

    // ── reserved_width ───────────────────────────────────────────────

    #[test]
    fn reserved_width_is_zero_without_a_minimap() {
        assert_eq!(reserved_width(20.0, false), 0.0);
    }

    #[test]
    fn reserved_width_returns_the_width_with_a_minimap() {
        assert_eq!(reserved_width(20.0, true), 20.0);
    }
}
