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
//! consecutive entries are packed into one braille row.
//! [`Minimap::layout_with_sizing`] takes `lines_per_row` (the backend's own
//! grouping factor: `1` for GTK, `4` for TUI) and groups `lines` into that
//! many rows, then tiles those rows according to [`MinimapSizing`] (issue
//! #667). [`Minimap::layout`] is the pre-#667 two-argument shape, kept as a
//! deprecated shim over `layout_with_sizing(bounds, lines_per_row,
//! MinimapSizing::Fill)` for source compatibility (see the *Downstream
//! consumers* section of `CLAUDE.md`) — new call sites should use
//! `layout_with_sizing` directly:
//!
//! - [`MinimapSizing::Fill`] — stretch to fill `bounds.height`, at a pitch
//!   that is never allowed to exceed [`MAX_ROW_PITCH`] (issue #663): a file
//!   short enough that `bounds.height / row_count` would blow past that
//!   ceiling instead top-aligns and only occupies `row_count * row_h` of
//!   the strip, leaving the remainder unpainted, rather than stretching to
//!   fill it. TUI is the only user — it is cell-native (braille rows) and
//!   has no font to scale.
//! - [`MinimapSizing::FixedPitch`] — rows tile top-down at exactly the
//!   given pitch, regardless of `row_count`. When more rows exist than the
//!   strip can hold at that pitch, [`Minimap::layout_with_sizing`] doesn't
//!   shrink the pitch to compress everything in — it slides: only a window of
//!   `bounds.height / pitch` rows is shown at once, and that window's
//!   position tracks [`Minimap::visible_row_start`] against
//!   [`Minimap::total_buffer_lines`] (VS Code's `minimap.size:
//!   proportional`). GTK is the only user, and this is what makes a
//!   minimap row's on-screen size independent of the file's length: the
//!   same buffer, painted into the same strip, always resolves to the same
//!   `vline.bounds.height` no matter how many lines it has.
//!
//! Each [`VisibleMinimapLine::bounds`] carries the *resolved* row height
//! and position, so a rasteriser reads its pitch straight off the layout
//! (`vline.bounds.height`) instead of re-deriving it via `bounds.height /
//! layout.visible_lines.len()` — re-deriving it that way silently undoes
//! both the [`MAX_ROW_PITCH`] cap under `Fill` and the fixed pitch itself
//! under `FixedPitch` (see the rasteriser spec in #382, #663 for the
//! `Fill` bug this replaced, and #667 for `FixedPitch`).
//!
//! `visible_row_start` / `visible_row_count` describe the *editor's*
//! current viewport as an index range into `lines` (not a separate scroll
//! position for the minimap itself) — that's what [`MinimapLayout::viewport_highlight`]
//! outlines, and what [`Minimap::scroll_thumb`] converts into a
//! [`Scrollbar`] against [`Minimap::total_buffer_lines`] using each
//! [`MinimapLine::line_idx`] as the bridge back to real buffer line
//! numbers (sampling may skip lines, so a `lines`-relative fraction and a
//! `total_buffer_lines`-relative fraction are not the same thing whenever
//! sampling is non-uniform). [`MinimapSizing::FixedPitch`]'s own slide
//! offset uses that same buffer-line bridge, for the same reason.

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

/// Ceiling on a minimap row's pitch under [`MinimapSizing::Fill`], in
/// `bounds`'s own coordinate units (terminal cell rows for TUI, the only
/// remaining `Fill` user as of #667 — GTK moved to
/// [`MinimapSizing::FixedPitch`]) — issue #663.
///
/// Without a ceiling, `Fill`'s `row_h` is `bounds.height / row_count`,
/// which grows without bound as a file gets shorter than the strip.
/// `MAX_ROW_PITCH` keeps a `Fill`-sized minimap always minimap-sized.
/// Below the ceiling, `row_count * row_h` rows top-align inside `bounds`
/// and the remainder of the strip stays unpainted — mirroring
/// [`sample_lines`]'s own never-upscale rule. Long files are unaffected:
/// `bounds.height / row_count` is already below the ceiling once the
/// caller's sampling has downsampled them to roughly fit the strip.
pub const MAX_ROW_PITCH: f32 = 8.0;

/// How [`Minimap::layout`] sizes rows across `bounds.height` — issue #667.
///
/// The two backends want opposite things: TUI's braille rows are
/// cell-native and have no font to scale, so it always wants to fill the
/// strip ([`Self::Fill`]). GTK's row pitch drives a Pango font size (or,
/// below the legibility floor, a colour block), so a file-length-dependent
/// pitch means a file-length-dependent glyph size — exactly the defect
/// #667 removes. GTK always wants [`Self::FixedPitch`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MinimapSizing {
    /// Stretch rows to fill `bounds.height`, capped at [`MAX_ROW_PITCH`]
    /// (#663). A file shorter than the strip at that cap top-aligns
    /// rather than stretching further; there is no sliding window — every
    /// row is always visible.
    Fill,
    /// Tile rows top-down at exactly this pitch (in `bounds`'s own
    /// coordinate units), regardless of `row_count`. When the file needs
    /// more rows than `bounds.height / pitch` holds, [`Minimap::layout`]
    /// shows a sliding window onto the map — see the module docs — instead
    /// of shrinking the pitch to compress everything in.
    FixedPitch(f32),
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
    /// Pre-#667 two-argument shape of [`Self::layout_with_sizing`], kept as
    /// a deprecated shim for source compatibility with out-of-tree callers
    /// (per CLAUDE.md's rule 8 deprecate-then-remove protocol —
    /// `vimcode`'s `src/render.rs::minimap_click_line` calls this exact
    /// 2-arg shape with no version pin on this crate). Forwards to
    /// [`Self::layout_with_sizing`] with [`MinimapSizing::Fill`], which is
    /// this method's own pre-#667 behaviour byte-for-byte — this shim does
    /// not change what any existing caller sees.
    ///
    /// New call sites — everything in this crate, and any new downstream
    /// code — should call [`Self::layout_with_sizing`] directly and choose
    /// a `sizing` explicitly instead of relying on this default.
    #[deprecated(
        since = "0.0.1",
        note = "use `layout_with_sizing(bounds, lines_per_row, sizing)` instead — this shim defaults to `MinimapSizing::Fill` (#667)"
    )]
    pub fn layout(&self, bounds: Rect, lines_per_row: usize) -> MinimapLayout {
        self.layout_with_sizing(bounds, lines_per_row, MinimapSizing::Fill)
    }

    /// Compute layout + hit regions. `lines_per_row` is the backend's
    /// grouping factor (`1` for GTK, `4` for TUI) — see the module docs
    /// for why layout needs it but painting-only metrics (font size,
    /// dot density) don't. `sizing` picks the row-pitch strategy — see
    /// [`MinimapSizing`].
    pub fn layout_with_sizing(
        &self,
        bounds: Rect,
        lines_per_row: usize,
        sizing: MinimapSizing,
    ) -> MinimapLayout {
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

        let (row_h, window_start_row, rows_shown) = match sizing {
            MinimapSizing::Fill => {
                // Bounded, not stretch-to-fill: see MAX_ROW_PITCH's doc
                // for why a short file must not blow its pitch up to fill
                // the whole strip (#663). No sliding window: every row is
                // always visible.
                let row_h = (bounds.height / row_count as f32).min(MAX_ROW_PITCH);
                (row_h, 0usize, row_count)
            }
            MinimapSizing::FixedPitch(px) => {
                let row_h = px.max(f32::MIN_POSITIVE);
                let rows_that_fit = (bounds.height / row_h).floor() as usize;
                let rows_shown = rows_that_fit.min(row_count);
                let window_start_row = self.slide_window_start_row(row_count, rows_shown);
                (row_h, window_start_row, rows_shown)
            }
        };

        let visible_lines: Vec<VisibleMinimapLine> = (0..rows_shown)
            .map(|i| {
                let r = window_start_row + i;
                VisibleMinimapLine {
                    start_line_idx: r * lines_per_row,
                    bounds: Rect::new(bounds.x, bounds.y + row_h * i as f32, bounds.width, row_h),
                }
            })
            .collect();

        // Absolute row positions of the editor's viewport, then clipped
        // into the visible window (`FixedPitch` may be sliding, so the
        // editor's viewport band can be partly or wholly off-strip).
        let start_row_abs = (self.visible_row_start / lines_per_row) as f32;
        let end_line = (self.visible_row_start + self.visible_row_count).min(self.lines.len());
        let end_row_abs = if end_line == 0 {
            0.0
        } else {
            ((end_line - 1) / lines_per_row) as f32 + 1.0
        };
        let window_lo = window_start_row as f32;
        let window_hi = (window_start_row + rows_shown) as f32;
        let clipped_start = start_row_abs.clamp(window_lo, window_hi);
        let clipped_end = end_row_abs.clamp(window_lo, window_hi);
        let highlight_h = (clipped_end - clipped_start).max(0.0) * row_h;
        let viewport_highlight = Rect::new(
            bounds.x,
            bounds.y + (clipped_start - window_lo) * row_h,
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

    /// First row index shown by a [`MinimapSizing::FixedPitch`] window of
    /// `rows_shown` rows out of `row_count` total — the "slide" the module
    /// docs describe. `0` when the whole map already fits (`row_count <=
    /// rows_shown`, or `rows_shown == 0`). Otherwise tracks how far
    /// [`Self::visible_row_start`] (mapped to a real buffer line via
    /// [`MinimapLine::line_idx`], the same bridge [`Self::scroll_thumb`]
    /// uses) has advanced through [`Self::total_buffer_lines`], so both
    /// ends of the file are reachable: the window sits at the top when the
    /// editor viewport is at the top, and at the bottom when it's at the
    /// bottom.
    fn slide_window_start_row(&self, row_count: usize, rows_shown: usize) -> usize {
        if rows_shown == 0 || row_count <= rows_shown {
            return 0;
        }
        let max_start = row_count - rows_shown;
        if self.total_buffer_lines <= 1 {
            return 0;
        }
        // `visible_row_start` is caller-supplied and expected to stay in
        // `0..self.lines.len()`; if it's ever out of range, treat that as
        // "the viewport is past the end of the file" rather than "at the
        // top" — falling back to `0` would snap the slide window to the
        // top of the file on out-of-range input, which is the wrong end.
        let start_buffer_line = self
            .lines
            .get(self.visible_row_start)
            .map(|l| l.line_idx)
            .unwrap_or_else(|| self.total_buffer_lines.saturating_sub(1));
        let denom = (self.total_buffer_lines - 1) as f32;
        let fraction = (start_buffer_line as f32 / denom).clamp(0.0, 1.0);
        ((fraction * max_start as f32).round() as usize).min(max_start)
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

// ── Render technique: font-scaling vs colour-block (#738) ────────────────
//
// Only the "cell-native braille" technique (TUI) has no font to scale, so
// it never faces this choice. Every pixel/DIP-unit backend that paints a
// row of real text at a scaled-down size — GTK ([`crate::gtk::minimap`])
// and now Win-GUI ([`crate::win::minimap`]) — needs the exact same
// decision: below what pitch does shaping a font at that size stop
// reading as recognisable glyphs and start reading as mush? Before #738
// this threshold ([`is_legible`] / [`render_mode`] / [`minimap_font_px`])
// existed only in `gtk::minimap`, so Win-GUI (and, eventually, macOS) would
// each have had to invent their own answer rather than reuse GTK's
// already-tuned one. `ROW_PITCH_PX` and `COLUMN_CAPACITY` move here for the
// same reason `primitives::board`'s `BOARD_*_PX` constants did (#736): both
// values are backend-agnostic DIP/pixel geometry (a DIP is a pixel at 100%
// display scale, same convention as every other lifted `*_PX` constant in
// this crate), and GTK's and Win-GUI's rasterisers want byte-for-byte the
// same numbers rather than an independently-tuned copy each.

/// Below this absolute pixel size, real glyph shaping reads as indistinct
/// mush rather than recognisable code shapes — a rasteriser falls back to
/// per-column colour blocks instead of real glyphs below this floor (see
/// [`render_mode`]).
pub const LEGIBILITY_FLOOR_PX: f64 = 4.0;

/// The fixed row pitch a font-scaling-technique backend (GTK, Win-GUI)
/// tiles minimap rows at, in device pixels/DIPs — independent of the
/// file's length (#667). VS Code's default minimap pitch is in the same
/// ~2px ballpark; below [`LEGIBILITY_FLOOR_PX`], so the default rasteriser
/// always lands in [`MinimapRenderMode::ColumnBlocks`].
pub const ROW_PITCH_PX: f64 = 2.0;

/// How many character columns a row's paint walk covers before stopping —
/// bounds both [`MinimapRenderMode::ColumnBlocks`]'s per-column walk and
/// how much of a line [`MinimapRenderMode::Characters`] hands to its text
/// shaper, so a 10,000-character line costs no more to paint than a short
/// one (#667 pt. 2/3). Also doubles as the assumed "wide" line width a
/// minimap strip is sized for.
pub const COLUMN_CAPACITY: usize = 120;

/// Render technique a row's pitch selects — see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MinimapRenderMode {
    /// Real glyphs at a scaled-down absolute size.
    Characters,
    /// One narrow block per non-blank character column, coloured by
    /// whichever span covers it.
    ColumnBlocks,
}

/// Pure function of the row pitch: is `line_px` legible enough to shape
/// real text? Every backend's `draw_minimap` calls this exact function to
/// pick a branch, so it is the one source of truth for the threshold —
/// tests exercise it directly instead of needing a live paint surface.
pub fn is_legible(line_px: f64) -> bool {
    line_px >= LEGIBILITY_FLOOR_PX
}

/// [`MinimapRenderMode`] for a given row pitch. See [`is_legible`].
pub fn render_mode(line_px: f64) -> MinimapRenderMode {
    if is_legible(line_px) {
        MinimapRenderMode::Characters
    } else {
        MinimapRenderMode::ColumnBlocks
    }
}

/// The absolute font size (in device pixels/DIPs) for a row of pitch
/// `line_px` — clamped to a sane band so a pathologically short or tall
/// minimap doesn't request a zero or absurd font size. Only reachable when
/// a row's pitch clears [`LEGIBILITY_FLOOR_PX`] — not the case for
/// [`ROW_PITCH_PX`] today, but the function stays pitch-driven rather than
/// a fixed constant in case a future caller requests a taller fixed pitch.
pub fn minimap_font_px(line_px: f64) -> f64 {
    line_px.clamp(1.0, 64.0)
}

/// Truncate `text` to at most `n` characters, on a char boundary. Never
/// allocates — returns a borrowed slice. Shared by every backend's
/// `ColumnBlocks`/`Characters` row paint so a pathologically long line
/// costs no more to walk or shape than a short one (#667 pt. 2/3).
pub fn truncate_to_columns(text: &str, n: usize) -> &str {
    match text.char_indices().nth(n) {
        Some((byte_idx, _)) => &text[..byte_idx],
        None => text,
    }
}

/// The colour covering character column `col`, from a row's own span
/// slice (already narrowed to that row by [`SpanCursor`]). Falls back to
/// `default_fg` when no span covers it.
pub fn color_at_column(row_spans: &[MinimapSpan], col: usize, default_fg: Color) -> Color {
    row_spans
        .iter()
        .find(|s| col >= s.start_col && col < s.end_col)
        .map(|s| s.color)
        .unwrap_or(default_fg)
}

/// Walks a [`MinimapSpan`] slice — sorted by `(line_idx, start_col)`, per
/// [`aggregate_spans`]'s documented output order — once across a
/// caller-driven sequence of *non-decreasing* `line_idx` queries, hence one
/// merge-walk in O(rows + spans) total rather than one `filter` scan of the
/// whole slice per row (#667 pt. 4). Shared by every backend's
/// `draw_minimap` row loop.
pub struct SpanCursor<'a> {
    spans: &'a [MinimapSpan],
    pos: usize,
}

impl<'a> SpanCursor<'a> {
    pub fn new(spans: &'a [MinimapSpan]) -> Self {
        Self { spans, pos: 0 }
    }

    /// The contiguous run of spans covering `line_idx`. `line_idx` must be
    /// non-decreasing across successive calls (true for every backend's
    /// `draw_minimap` row loop) — an out-of-order query would silently
    /// miss spans the cursor already walked past.
    pub fn row_spans(&mut self, line_idx: usize) -> &'a [MinimapSpan] {
        while self.pos < self.spans.len() && self.spans[self.pos].line_idx < line_idx {
            self.pos += 1;
        }
        let start = self.pos;
        let mut end = start;
        while end < self.spans.len() && self.spans[end].line_idx == line_idx {
            end += 1;
        }
        &self.spans[start..end]
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
        // 4 rows over 16px: pitch 4px, comfortably under MAX_ROW_PITCH,
        // so this exercises even tiling rather than the pitch cap
        // (that's `layout_caps_row_pitch_and_top_aligns_short_files`,
        // below).
        let bounds = Rect::new(0.0, 0.0, 10.0, 16.0);
        let layout = mm.layout_with_sizing(bounds, 2, MinimapSizing::Fill); // 8 lines / 2 per row = 4 rows
        assert_eq!(layout.visible_lines.len(), 4);
        assert_eq!(
            layout.visible_lines[0].bounds,
            Rect::new(0.0, 0.0, 10.0, 4.0)
        );
        assert_eq!(
            layout.visible_lines[1].bounds,
            Rect::new(0.0, 4.0, 10.0, 4.0)
        );
        assert_eq!(
            layout.visible_lines[3].bounds,
            Rect::new(0.0, 12.0, 10.0, 4.0)
        );
        assert_eq!(layout.visible_lines[3].start_line_idx, 6);
    }

    #[test]
    #[allow(deprecated)] // exercising the deprecated shim itself (#667)
    fn deprecated_layout_shim_matches_layout_with_sizing_fill() {
        // The pre-#667 two-argument `layout()` must keep resolving exactly
        // like `layout_with_sizing(.., MinimapSizing::Fill)` -- that's the
        // whole point of the shim (CLAUDE.md rule 8: `vimcode`'s
        // `src/render.rs::minimap_click_line` still calls the 2-arg form
        // with no version pin on this crate, so its behaviour must not
        // change out from under it).
        let mm = minimap(8, 2, 3);
        let bounds = Rect::new(0.0, 0.0, 10.0, 16.0);
        let shim = mm.layout(bounds, 2);
        let direct = mm.layout_with_sizing(bounds, 2, MinimapSizing::Fill);
        assert_eq!(shim, direct);
    }

    #[test]
    fn layout_viewport_highlight_spans_the_editor_viewport() {
        // visible_row_start=2, visible_row_count=3 -> lines[2..5), rows
        // grouped 2-per-row -> row 1 (start) through row 3 (exclusive).
        let mm = minimap(8, 2, 3);
        let bounds = Rect::new(0.0, 0.0, 10.0, 16.0); // pitch 4px, under the cap
        let layout = mm.layout_with_sizing(bounds, 2, MinimapSizing::Fill);
        assert_eq!(layout.viewport_highlight, Rect::new(0.0, 4.0, 10.0, 8.0));
    }

    #[test]
    fn layout_caps_row_pitch_and_top_aligns_short_files() {
        // A 3-line file (lines_per_row=1 -> 3 rows) inside a tall 200px
        // strip. Uncapped, row_h would be 200/3 ~= 66.7px -- a 3-line
        // file rendering at near-editor scale (#663). Capped, no row's
        // pitch may exceed MAX_ROW_PITCH, and the rows top-align, only
        // occupying the strip's top `row_count * row_h` -- the rest of
        // the tall strip stays unpainted rather than stretching to fill.
        let mm = minimap(3, 0, 3);
        let bounds = Rect::new(0.0, 0.0, 40.0, 200.0);
        let layout = mm.layout_with_sizing(bounds, 1, MinimapSizing::Fill);
        assert_eq!(layout.visible_lines.len(), 3);
        for vline in &layout.visible_lines {
            assert!(
                vline.bounds.height <= MAX_ROW_PITCH,
                "row pitch {} exceeds the MAX_ROW_PITCH ceiling",
                vline.bounds.height
            );
        }
        assert_eq!(
            layout.visible_lines[0].bounds.y, bounds.y,
            "rows must top-align to the strip"
        );
        let last = layout.visible_lines.last().unwrap();
        let occupied = last.bounds.y + last.bounds.height;
        assert!(
            occupied < bounds.height,
            "a short file must not stretch its rows to fill the whole strip \
             (occupied {occupied}, strip height {})",
            bounds.height
        );
    }

    #[test]
    fn layout_empty_lines_is_a_no_op() {
        let mm = minimap(0, 0, 0);
        let layout = mm.layout_with_sizing(Rect::new(0.0, 0.0, 10.0, 40.0), 4, MinimapSizing::Fill);
        assert!(layout.visible_lines.is_empty());
        assert_eq!(layout.viewport_highlight.height, 0.0);
    }

    #[test]
    fn layout_zero_size_bounds_is_a_no_op() {
        let mm = minimap(8, 0, 4);
        let layout = mm.layout_with_sizing(Rect::new(0.0, 0.0, 0.0, 0.0), 4, MinimapSizing::Fill);
        assert!(layout.visible_lines.is_empty());
    }

    // ── MinimapSizing::FixedPitch (#667) ────────────────────────────

    #[test]
    fn fixed_pitch_row_height_is_independent_of_file_length() {
        // The core #667 acceptance test: the same strip, at the same
        // fixed pitch, yields an identical row height for a 3-line file
        // and a 10,000-line file -- unlike `Fill`, where a short file's
        // pitch balloons and a long file's pitch shrinks.
        let bounds = Rect::new(0.0, 0.0, 40.0, 400.0);
        let short = minimap(3, 0, 3);
        let long = minimap(10_000, 0, 10);

        let short_layout = short.layout_with_sizing(bounds, 1, MinimapSizing::FixedPitch(2.0));
        let long_layout = long.layout_with_sizing(bounds, 1, MinimapSizing::FixedPitch(2.0));

        assert_eq!(short_layout.visible_lines[0].bounds.height, 2.0);
        assert_eq!(long_layout.visible_lines[0].bounds.height, 2.0);
        assert_eq!(
            short_layout.visible_lines[0].bounds.height,
            long_layout.visible_lines[0].bounds.height
        );
    }

    #[test]
    fn fixed_pitch_short_file_top_aligns_without_stretching() {
        // 3 rows at a 2px pitch only occupy 6px of a 400px strip -- no
        // windowing needed, no stretching either.
        let mm = minimap(3, 0, 3);
        let bounds = Rect::new(0.0, 0.0, 40.0, 400.0);
        let layout = mm.layout_with_sizing(bounds, 1, MinimapSizing::FixedPitch(2.0));
        assert_eq!(layout.visible_lines.len(), 3);
        let last = layout.visible_lines.last().unwrap();
        assert!(last.bounds.y + last.bounds.height < bounds.height);
    }

    #[test]
    fn fixed_pitch_slides_monotonically_and_reaches_both_ends() {
        // 1000 rows at 2px pitch need 2000px; a 200px strip only shows
        // 100 at a time, so the window must slide as visible_row_start
        // advances -- and both ends of the file must be reachable.
        let bounds = Rect::new(0.0, 0.0, 40.0, 200.0);
        let n = 1000;

        let mut starts = Vec::new();
        for visible_row_start in [0, 100, 300, 500, 700, 900, 999] {
            let mm = minimap(n, visible_row_start, 1);
            let layout = mm.layout_with_sizing(bounds, 1, MinimapSizing::FixedPitch(2.0));
            let first_line_idx = layout.visible_lines.first().unwrap().start_line_idx;
            starts.push(first_line_idx);
        }

        assert!(
            starts.windows(2).all(|w| w[0] <= w[1]),
            "window start must advance monotonically as visible_row_start advances: {starts:?}"
        );
        assert_eq!(
            starts[0], 0,
            "scrolled to the top, the window starts at row 0"
        );
        assert_eq!(
            *starts.last().unwrap(),
            n - 100,
            "scrolled to the bottom, the window's last row must reach the file's end"
        );
    }

    #[test]
    fn fixed_pitch_out_of_range_visible_row_start_slides_to_the_end() {
        // `visible_row_start >= lines.len()` is out-of-range input (callers
        // are expected to keep it in bounds), but `slide_window_start_row`
        // must not silently snap to the top of the file for it -- that's
        // the wrong end for a viewport that's (nominally) past the end.
        let bounds = Rect::new(0.0, 0.0, 40.0, 200.0);
        let n = 1000;
        let mut mm = minimap(n, 0, 1);
        mm.visible_row_start = n; // one past the last valid `lines` index

        let layout = mm.layout_with_sizing(bounds, 1, MinimapSizing::FixedPitch(2.0));
        let first_line_idx = layout.visible_lines.first().unwrap().start_line_idx;
        assert_eq!(
            first_line_idx,
            n - 100,
            "out-of-range visible_row_start must slide to the bottom of the file, not the top"
        );
    }

    #[test]
    fn fixed_pitch_whole_file_fits_needs_no_slide() {
        // row_count (100) <= rows that fit (100 at 2px in 200px): every
        // row is visible regardless of visible_row_start.
        let mm = minimap(100, 50, 1);
        let bounds = Rect::new(0.0, 0.0, 40.0, 200.0);
        let layout = mm.layout_with_sizing(bounds, 1, MinimapSizing::FixedPitch(2.0));
        assert_eq!(layout.visible_lines.len(), 100);
        assert_eq!(layout.visible_lines[0].start_line_idx, 0);
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

    // ── render-mode threshold (#738, moved from gtk::minimap) ─────────

    #[test]
    fn legibility_floor_switches_render_mode_on_both_sides() {
        assert_eq!(
            render_mode(LEGIBILITY_FLOOR_PX - 0.1),
            MinimapRenderMode::ColumnBlocks
        );
        assert_eq!(
            render_mode(LEGIBILITY_FLOOR_PX),
            MinimapRenderMode::Characters
        );
        assert_eq!(
            render_mode(LEGIBILITY_FLOOR_PX + 4.0),
            MinimapRenderMode::Characters
        );
    }

    #[test]
    fn default_row_pitch_stays_below_the_legibility_floor() {
        // The whole point of #738's shared constants: both GTK and Win-GUI
        // key their default fixed pitch off `ROW_PITCH_PX`, and it must
        // stay below `LEGIBILITY_FLOOR_PX` so the default rasteriser always
        // lands in `ColumnBlocks` on either backend.
        assert!(ROW_PITCH_PX < LEGIBILITY_FLOOR_PX);
        assert_eq!(render_mode(ROW_PITCH_PX), MinimapRenderMode::ColumnBlocks);
    }

    #[test]
    fn minimap_font_px_clamps_to_a_sane_band() {
        assert_eq!(minimap_font_px(0.0), 1.0);
        assert_eq!(minimap_font_px(1000.0), 64.0);
        assert_eq!(minimap_font_px(10.0), 10.0);
    }

    // ── truncate_to_columns / color_at_column / SpanCursor ─────────────

    #[test]
    fn truncate_to_columns_cuts_on_a_char_boundary() {
        assert_eq!(truncate_to_columns("café", 3), "caf");
        assert_eq!(truncate_to_columns("ab", 10), "ab");
        assert_eq!(truncate_to_columns("", 5), "");
    }

    // `red()`/`blue()` helpers already defined above for the
    // `aggregate_spans` tests — reused here too.

    #[test]
    fn color_at_column_picks_the_span_covering_that_column() {
        let spans = vec![
            MinimapSpan {
                line_idx: 0,
                start_col: 0,
                end_col: 1,
                color: red(),
            },
            MinimapSpan {
                line_idx: 0,
                start_col: 1,
                end_col: 6,
                color: blue(),
            },
        ];
        assert_eq!(color_at_column(&spans, 0, Color::rgb(1, 1, 1)), red());
        assert_eq!(color_at_column(&spans, 3, Color::rgb(1, 1, 1)), blue());
    }

    #[test]
    fn color_at_column_falls_back_to_default_outside_any_span() {
        let fallback = Color::rgb(9, 9, 9);
        assert_eq!(color_at_column(&[], 2, fallback), fallback);
    }

    #[test]
    fn span_cursor_matches_a_full_linear_scan_per_row() {
        let green = Color::rgb(0, 255, 0);
        let spans = vec![
            MinimapSpan {
                line_idx: 0,
                start_col: 0,
                end_col: 2,
                color: red(),
            },
            MinimapSpan {
                line_idx: 0,
                start_col: 2,
                end_col: 4,
                color: blue(),
            },
            MinimapSpan {
                line_idx: 2,
                start_col: 0,
                end_col: 3,
                color: green,
            },
            MinimapSpan {
                line_idx: 5,
                start_col: 1,
                end_col: 2,
                color: red(),
            },
        ];
        let mut cursor = SpanCursor::new(&spans);
        for line_idx in 0..6 {
            let via_cursor = cursor.row_spans(line_idx).to_vec();
            let via_linear: Vec<MinimapSpan> = spans
                .iter()
                .filter(|s| s.line_idx == line_idx)
                .cloned()
                .collect();
            assert_eq!(
                via_cursor, via_linear,
                "row {line_idx} spans must match a full linear scan"
            );
        }
    }
}
