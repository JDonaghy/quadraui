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
//! Both algorithms that make this possible — [`sample_blocks`] (row
//! down-sampling) and [`aggregate_spans`] (colour down-sampling) — live
//! here, not in either backend, so the two rasterisers never re-derive
//! or re-reduce data the primitive already resolved.
//!
//! [`sample_blocks`] (issue #1012) partitions the buffer into
//! [`block_bounds`] blocks and aggregates *every* line in each block's
//! read budget ([`BLOCK_LINE_SAMPLE_CAP`]) into one output row via a
//! per-column coverage [`dither_threshold_met`] — not [`sample_lines`]'s
//! older point-sample, which kept exactly one line per block and
//! discarded the rest outright (at a 647-line file through a ~33-row
//! strip, 80% of the buffer). [`sample_lines`] is now a deprecated shim
//! over [`sample_blocks`], the same way [`Minimap::layout`] is a shim
//! over [`Minimap::layout_with_sizing`].
//!
//! [`sample_blocks`] alone always compresses `total_lines` down to (at
//! most) `target_rows` output rows — the right behaviour for a rasteriser
//! that shows the *whole* buffer squeezed into the strip, but it means
//! `Minimap::lines.len()` never exceeds the display's own row budget, so
//! [`Minimap::layout_with_sizing`]'s `FixedPitch` slide never has more
//! rows than fit and its window never moves (issue #1044). A host that
//! instead wants a VS-Code-proportional **window** onto the buffer — a
//! fixed scale that slides as the editor scrolls, rather than the whole
//! file always squeezed in — calls [`sample_window`] (built from
//! [`window_start_line`] plus `sample_blocks`) instead of pre-slicing the
//! buffer itself: [`window_start_line`] is the same fraction-of-buffer
//! slide arithmetic `layout_with_sizing`'s own post-sample slide uses,
//! run one step earlier, over real buffer lines instead of already-sampled
//! rows, so the window's position is the primitive's decision rather than
//! logic every host duplicates by hand.
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
//!   fill it. No live rasteriser uses this any more (see `FixedPitch`
//!   below) — it survives only as the deprecated [`Minimap::layout`]
//!   shim's default, for source compatibility with pre-#667 out-of-tree
//!   callers. **Do not size a new rasteriser with `Fill`**: any backend
//!   that paints one glyph/cell per row (TUI's braille rows are cell-native
//!   with no font to scale, so there's never a reason for it to stretch
//!   pitch) reads a stretched pitch as *gaps between painted rows*, not a
//!   taller row — that was #992, where TUI's `Fill` usage left up to
//!   `MAX_ROW_PITCH - 1` blank cell rows between every painted row on a
//!   short file.
//! - [`MinimapSizing::FixedPitch`] — rows tile top-down at exactly the
//!   given pitch, regardless of `row_count`. When more rows exist than the
//!   strip can hold at that pitch, [`Minimap::layout_with_sizing`] doesn't
//!   shrink the pitch to compress everything in — it slides: only a window of
//!   `bounds.height / pitch` rows is shown at once, and that window's
//!   position tracks [`Minimap::visible_row_start`] against
//!   [`Minimap::total_buffer_lines`] (VS Code's `minimap.size:
//!   proportional`). Every backend uses this — GTK and Win-GUI at
//!   [`crate::primitives::minimap::ROW_PITCH_PX`] (a font-scaling
//!   pitch), TUI at exactly `1.0` cell row per row (#992) — and this is
//!   what makes a minimap row's on-screen size independent of the file's
//!   length: the same buffer, painted into the same strip, always
//!   resolves to the same `vline.bounds.height` no matter how many lines
//!   it has.
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
    /// Pre-aggregated lines (app does the sampling, e.g. via [`sample_blocks`]).
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

/// Pre-#822 name for the raw syntax-highlight span passed into
/// [`aggregate_spans`]. `SyntaxSpan` and [`MinimapSpan`] were
/// byte-identical four-field structs — one named for the *input* to
/// `aggregate_spans`, the other for its *output* — with nothing in the
/// type system actually preventing either from being used as the other
/// (all fields `pub`, no invariant enforced by construction). Merged
/// into a single type per `PRIMITIVE_RULES.md` rule 8; this alias keeps
/// old call sites (and the `quadraui::SyntaxSpan` crate-root re-export)
/// source-compatible.
#[deprecated(since = "0.0.1", note = "merged into `MinimapSpan` (#822)")]
pub type SyntaxSpan = MinimapSpan;

/// The region [`aggregate_spans`] folds raw [`MinimapSpan`]s into.
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
#[derive(Debug, Clone, PartialEq)]
pub struct MinimapLayout {
    pub bounds: Rect,
    pub visible_lines: Vec<VisibleMinimapLine>,
    /// Where the editor's current viewport appears, in the same absolute
    /// coordinates as `bounds`.
    pub viewport_highlight: Rect,
    pub scrollbar: Option<Scrollbar>,
    /// Buffer columns folded into one output cell along the horizontal
    /// axis — the actual scale the rasteriser that produced this layout
    /// used (or will use, for a no-paint `*_layout` call), not a
    /// caller-side guess at it.
    ///
    /// `1` (the [`Default`] impl below, and what [`Minimap::layout_with_sizing`]
    /// itself always sets) means one buffer column per output cell — GTK's
    /// and Win-GUI's own [`MinimapGrid::cols_per_cell`], since their strips
    /// are wide enough in pixels that [`COLUMN_CAPACITY`] columns fit
    /// without folding. TUI's braille rasteriser overrides this to a wider,
    /// buffer-width-adaptive value (`tui::minimap::resolve_cols_per_cell`,
    /// issue #1032) after calling `layout_with_sizing`, since its strip is
    /// cell-narrow enough that a fixed 1:1 scale only ever showed the
    /// buffer's first `width_cells * 2` columns.
    ///
    /// A host that colour-aggregates its own [`Minimap::syntax_spans`] via
    /// [`aggregate_spans`] must build its [`MinimapGrid::cols_per_cell`]
    /// from **this** field — read back from a `minimap_layout`/`draw_minimap`
    /// call against the same [`Minimap::lines`] — rather than hardcoding a
    /// constant. Before #1032 this was the only way TUI's scale could be
    /// known: it was a compile-time constant a host could copy, but once
    /// the scale became buffer-width-adaptive a copied constant silently
    /// drifts from what the rasteriser actually paints, desyncing colour
    /// from dot content (the #1000 review concern this field closes).
    pub cols_per_cell: usize,
}

impl Default for MinimapLayout {
    fn default() -> Self {
        Self {
            bounds: Rect::default(),
            visible_lines: Vec::new(),
            viewport_highlight: Rect::default(),
            scrollbar: None,
            cols_per_cell: 1,
        }
    }
}

/// Ceiling on a minimap row's pitch under [`MinimapSizing::Fill`], in
/// `bounds`'s own coordinate units — issue #663.
///
/// Without a ceiling, `Fill`'s `row_h` is `bounds.height / row_count`,
/// which grows without bound as a file gets shorter than the strip.
/// `MAX_ROW_PITCH` keeps a `Fill`-sized minimap always minimap-sized.
/// Below the ceiling, `row_count * row_h` rows top-align inside `bounds`
/// and the remainder of the strip stays unpainted — mirroring
/// [`sample_blocks`]'s own never-upscale rule. Long files are unaffected:
/// `bounds.height / row_count` is already below the ceiling once the
/// caller's sampling has downsampled them to roughly fit the strip.
///
/// No rasteriser sizes with `Fill` any more (#992 moved TUI, the last
/// user, to [`MinimapSizing::FixedPitch`]) — this constant now backs only
/// the deprecated [`Minimap::layout`] shim's default, kept for source
/// compatibility with pre-#667 out-of-tree callers.
pub const MAX_ROW_PITCH: f32 = 8.0;

/// How [`Minimap::layout`] sizes rows across `bounds.height` — issue #667.
///
/// Every backend's row pitch drives some paint cost per row — GTK's and
/// Win-GUI's drive a Pango/DirectWrite font size (or, below the
/// legibility floor, a colour block), and TUI's drives how many cell rows
/// `draw_minimap` paints per row (exactly one — braille rows are
/// cell-native, with no font to scale, but that still means a
/// file-length-dependent pitch turns into file-length-dependent *gaps*
/// rather than a file-length-dependent glyph size, which was #992). A
/// file-length-dependent pitch is the defect #667 (GTK) and #992 (TUI)
/// both remove: every backend now sizes with [`Self::FixedPitch`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MinimapSizing {
    /// Stretch rows to fill `bounds.height`, capped at [`MAX_ROW_PITCH`]
    /// (#663). A file shorter than the strip at that cap top-aligns
    /// rather than stretching further; there is no sliding window — every
    /// row is always visible.
    ///
    /// No live rasteriser uses this (see [`Self::FixedPitch`]) — kept only
    /// for the deprecated [`Minimap::layout`] shim's pre-#667 behaviour.
    Fill,
    /// Tile rows top-down at exactly this pitch (in `bounds`'s own
    /// coordinate units), regardless of `row_count`. When the file needs
    /// more rows than `bounds.height / pitch` holds, [`Minimap::layout`]
    /// shows a sliding window onto the map — see the module docs — instead
    /// of shrinking the pitch to compress everything in.
    FixedPitch(f32),
    /// VS Code-parity minimap **width** policy (issue #776) — orthogonal
    /// to `Fill` / `FixedPitch`, which size row *pitch* (vertical). This
    /// variant sizes the strip's overall width (horizontal) and is
    /// resolved with [`Self::resolve_width`], not consumed here.
    ///
    /// Before #776, a caller wanting VS Code parity kept two separate
    /// constant sets — one in raw pixels for a pixel backend, one in
    /// character columns for a cell-native backend — and picked between
    /// them with a backend sniff (`if char_width > 1.0`). This variant
    /// states the bounds once, in character columns, and
    /// [`Self::resolve_width`] converts them into whichever native unit
    /// the caller's own [`crate::backend::Backend::char_width`] implies,
    /// so no call site needs to know which backend it's talking to.
    ///
    /// - `target_cols` — the width the minimap holds steady at once the
    ///   pane can afford it (VS Code parity: the strip does not grow
    ///   with the window).
    /// - `fraction` — cap on how much of the pane's own width the
    ///   minimap may claim before `target_cols` is affordable, so a
    ///   narrow or split pane still narrows the strip instead of
    ///   clipping it.
    /// - `min` / `max` — floor and ceiling the resolved width clamps to,
    ///   in the same column unit as `target_cols`.
    VsCodeParity {
        target_cols: f32,
        fraction: f32,
        min: f32,
        max: f32,
    },
}

impl MinimapSizing {
    /// Resolve a [`Self::VsCodeParity`] width policy against the pane's
    /// own width and the caller's character metric, in one unit system
    /// (character columns) instead of two backend-specific constant sets
    /// (issue #776).
    ///
    /// `pane_width` and `char_width` are expected to be the same pair a
    /// [`crate::backend::Backend`] impl already exposes:
    /// `backend.char_width()` and a pane width in that backend's own
    /// native unit. A cell-native backend's `char_width() == 1.0` makes
    /// `pane_width` a column count outright, so `target_cols`/`min`/`max`
    /// apply unconverted; a pixel backend's `char_width()` is the
    /// resolved pixel width of one character, which turns the same
    /// column-denominated bounds into pixels. Callers never branch on
    /// which backend they're talking to — they just forward its
    /// `char_width()`.
    ///
    /// Returns `None` for [`Self::Fill`] / [`Self::FixedPitch`], which
    /// size row pitch, not width.
    pub fn resolve_width(&self, pane_width: f32, char_width: f32) -> Option<f32> {
        let MinimapSizing::VsCodeParity {
            target_cols,
            fraction,
            min,
            max,
        } = *self
        else {
            return None;
        };
        let cw = if char_width > 0.0 { char_width } else { 1.0 };
        let pane_width_cols = pane_width / cw;
        let want_cols = target_cols.min(pane_width_cols * fraction).clamp(min, max);
        Some(want_cols * cw)
    }
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
                cols_per_cell: 1,
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
            // `VsCodeParity` sizes the strip's *width*, an orthogonal axis
            // to row pitch — see its doc and `resolve_width`. No caller
            // should reach row-pitch layout with it; this arm exists only
            // to keep the match exhaustive, and falls back to `Fill`'s
            // behaviour rather than panicking on a value that carries no
            // meaningful pitch of its own.
            MinimapSizing::VsCodeParity { .. } => {
                let row_h = (bounds.height / row_count as f32).min(MAX_ROW_PITCH);
                (row_h, 0usize, row_count)
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
            cols_per_cell: 1,
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
    ///
    /// This is the **post-sample** slide: it only ever sees `row_count ==
    /// self.lines.len() / lines_per_row`, i.e. whatever
    /// [`sample_blocks`]/[`sample_window`] already handed it. A host that
    /// compresses `self.lines` down to (at most) the display's own row
    /// budget before constructing this [`Minimap`] — every real caller
    /// does, via `sample_blocks`'s own `target_rows` — always satisfies
    /// `row_count <= rows_shown` here, so this branch never engages for
    /// them; see [`window_start_line`]'s doc for the pre-sample slide that
    /// closes that gap (issue #1044).
    fn slide_window_start_row(&self, row_count: usize, rows_shown: usize) -> usize {
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
        slide_start(
            row_count,
            rows_shown,
            start_buffer_line,
            self.total_buffer_lines,
        )
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

/// Compress `buffer_lines` into at most `target_rows` [`MinimapLine`]s.
///
/// Deprecated (issue #1012): this signature forces a caller to
/// pre-resolve every buffer line into `buffer_lines` before sampling can
/// even begin, and — before #1012 — picked exactly one line per
/// [`block_bounds`] block and discarded the other `stride - 1` outright,
/// no matter what they contained (at a 647-line file through a ~33-row
/// strip, 80% of the buffer was invisible to the minimap). This shim now
/// forwards to [`sample_blocks`], so an existing caller that already has
/// the whole buffer materialised still gets the real down-sampling fix
/// for free — `buffer_lines[i].to_string()` as the accessor costs nothing
/// extra a materialised slice wasn't already paying. A caller that can
/// avoid materialising the whole buffer up front (e.g. one backed by a
/// rope) should call [`sample_blocks`] directly instead, and pull only
/// the lines its own accessor is asked for.
#[deprecated(
    since = "0.0.1",
    note = "point-sampler over a pre-materialised slice; use `sample_blocks` (line accessor, real block aggregation) instead (#1012)"
)]
pub fn sample_lines(buffer_lines: &[&str], target_rows: usize) -> Vec<MinimapLine> {
    sample_blocks(buffer_lines.len(), target_rows, |i| {
        buffer_lines[i].to_string()
    })
}

/// Ceiling on how many real buffer lines [`sample_blocks`] reads for one
/// output block's [`block_sample_indices`], regardless of how large the
/// block itself is (issue #1012).
///
/// A block's size grows with `total_lines / target_rows`, so reading
/// every line in every block would cost `O(total_lines)` per call again —
/// exactly the point-sampler's own complexity, just with more work spent
/// per sample rather than none. Capping the read at a small constant,
/// evenly spread across the block, keeps the cost
/// `O(target_rows * BLOCK_LINE_SAMPLE_CAP)` — independent of
/// `total_lines` — while still reading *every* line in any block small
/// enough to fit under the cap. `8` is the value vimcode's own
/// `MINIMAP_BLOCK_LINE_SAMPLE_CAP` (vimcode#1085) picked before this
/// budget moved here; #1012's point is that it's the primitive's own
/// decision now, not a constant each consumer reinvents.
pub const BLOCK_LINE_SAMPLE_CAP: usize = 8;

/// Partition `0..total_lines` into `target_rows.min(total_lines)`
/// contiguous, non-overlapping blocks — one per output [`MinimapLine`]
/// [`sample_blocks`] produces (issue #1012).
///
/// Returns `bounds` such that block `r` covers `bounds[r]..bounds[r + 1]`;
/// `bounds.len()` is always the block count plus one. Never upscales: one
/// line per block when `total_lines <= target_rows`, otherwise stride
/// `total_lines as f64 / target_rows as f64` between block starts —
/// exactly [`sample_lines`]'s own pre-#1012 stride formula, so a block's
/// *boundary* lands exactly where the old point-sampler's single pick
/// used to, but [`sample_blocks`] now reads (a capped sample of) every
/// line inside it rather than just that one boundary line.
pub fn block_bounds(total_lines: usize, target_rows: usize) -> Vec<usize> {
    if total_lines == 0 || target_rows == 0 {
        return vec![0];
    }
    if total_lines <= target_rows {
        return (0..=total_lines).collect();
    }
    let stride = total_lines as f64 / target_rows as f64;
    let mut bounds = Vec::with_capacity(target_rows + 1);
    for r in 0..target_rows {
        bounds.push(((r as f64 * stride) as usize).min(total_lines));
    }
    bounds.push(total_lines);
    bounds
}

/// Buffer line indices [`sample_blocks`] will actually read for one block
/// spanning `[start, end)` — every line when the block fits under `cap`,
/// otherwise `cap` lines evenly spaced across the block (issue #1012).
pub fn block_sample_indices(start: usize, end: usize, cap: usize) -> Vec<usize> {
    let len = end.saturating_sub(start);
    if len == 0 {
        return Vec::new();
    }
    let cap = cap.max(1);
    if len <= cap {
        return (start..end).collect();
    }
    let step = len as f64 / cap as f64;
    (0..cap)
        .map(|i| (start + (i as f64 * step) as usize).min(end - 1))
        .collect()
}

/// 4x4 ordered-dither (Bayer) threshold matrix, values `0..16`, shared by
/// every coverage-to-boolean decision this crate makes across a minimap
/// strip (issue #1012 pt. 2). [`sample_blocks`] uses it (via
/// [`dither_threshold_met`]) to decide whether a *block*'s column reads
/// back non-blank; [`crate::tui::braille::dither_threshold_met`] (issue
/// #1007) re-exports this exact matrix to make the same decision one
/// granularity finer, per *dot*. A single shared matrix means the two
/// compositions are provably the same dither policy applied twice, not
/// two independently-tuned ones that happen to agree today — before
/// #1012 they were exactly that: this matrix lived only in
/// `tui::braille`, and vimcode#1085 had already grown its own copy for
/// the block-level decision.
pub const BAYER4: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// Threshold `covered` of `total` sampled units (a block's sampled lines,
/// a dot's source columns, ...) against [`BAYER4`], indexed by the
/// caller's own absolute `(row, col)` position so the dither pattern
/// tiles across a whole strip rather than repeating identically inside
/// every block/cell. Pure integer arithmetic — one multiply, one
/// compare, no allocation.
///
/// `total == 0` always returns `false` (nothing sampled, nothing to
/// threshold). A fully-covered bucket (`covered == total`) always
/// returns `true`, since `total * 16 > total * 15` (`15` is [`BAYER4`]'s
/// largest entry) for any `total > 0` — a solid run still paints solid —
/// and `covered == 0` always returns `false`, since `0` is never greater
/// than a non-negative product. Dithering only has any effect strictly
/// *between* those two extremes.
pub fn dither_threshold_met(covered: usize, total: usize, row: usize, col: usize) -> bool {
    if total == 0 {
        return false;
    }
    let threshold = BAYER4[row & 3][col & 3] as usize;
    covered * 16 > total * threshold
}

/// Down-sample real buffer lines `0..total_lines` into at most
/// `target_rows` [`MinimapLine`]s — the primitive-owned row down-sampler
/// (issue #1012) that [`sample_lines`] used to only promise, not deliver.
///
/// `line_at` is a **line accessor**, not a materialised slice: this
/// function calls it only for the lines its own read budget
/// ([`BLOCK_LINE_SAMPLE_CAP`]) says it needs — at most
/// `BLOCK_LINE_SAMPLE_CAP` times per output row, evenly spread across
/// that row's [`block_bounds`] block via [`block_sample_indices`] — so a
/// host backed by a rope, a gap buffer, or anything else expensive to
/// fully materialise never has to resolve the whole buffer just to build
/// a minimap. That is the seam [`sample_lines`]'s `&[&str]` shape could
/// not offer: it forced the host to pre-resolve every line before
/// sampling could even begin.
///
/// A block whose read budget is exactly one line (true for every block
/// once `total_lines <= target_rows` — the never-upscale case) returns
/// that line's own text verbatim, truncated to [`COLUMN_CAPACITY`]
/// columns, same as [`sample_lines`] always did. Otherwise every sampled
/// line in the block votes on every column: a column reads back non-blank
/// (`'x'`) when its per-column coverage fraction — how many of the
/// block's sampled lines are non-whitespace there — clears
/// [`dither_threshold_met`]'s ordered-dither threshold, and blank (`' '`)
/// otherwise. A hard majority cutoff would erase a rare long line's tail
/// (surrounded by short ones, its reach past their length is well under
/// 50% of the block); dithering instead gives that low-but-nonzero
/// coverage fraction a proportionally small, evenly spread chance of
/// registering — enough for the tail to still show as a sparse trace
/// rather than nothing, while a densely-covered column still reads
/// solid. Every live [`MinimapRenderMode::ColumnBlocks`] paint walk (and
/// TUI's own dot-level fold on top of it, issue #1007) only asks "is this
/// column blank", so this synthesised text carries exactly the signal
/// those rasterisers consume — real per-block density instead of one
/// line's worth of gaps.
///
/// `MinimapLine::line_idx` is each block's own **start** line — the
/// buffer-line bridge `Minimap`'s scroll-thumb and `FixedPitch` slide
/// window both need stays meaningful even though a block folds several
/// real lines together.
pub fn sample_blocks<F: FnMut(usize) -> String>(
    total_lines: usize,
    target_rows: usize,
    mut line_at: F,
) -> Vec<MinimapLine> {
    if total_lines == 0 || target_rows == 0 {
        return Vec::new();
    }
    let bounds = block_bounds(total_lines, target_rows);
    (0..bounds.len() - 1)
        .map(|r| {
            let indices = block_sample_indices(bounds[r], bounds[r + 1], BLOCK_LINE_SAMPLE_CAP);
            MinimapLine {
                text: aggregate_block_text(&indices, r, &mut line_at),
                line_idx: bounds[r],
            }
        })
        .collect()
}

/// Slide-window fraction math shared by [`Minimap`]'s own post-sample
/// `FixedPitch` slide (`slide_window_start_row`) and [`window_start_line`]'s
/// pre-sample equivalent (issue #1044): given `total` items and a
/// `window`-sized slice of them the caller can actually show/read at once,
/// pick where that slice should start so it tracks `position` (out of
/// `total_at_position` possible positions) — both ends of the range are
/// always reachable (`position == 0` puts the window at the start;
/// `position == total_at_position - 1` puts the window's last item at the
/// very end).
fn slide_start(total: usize, window: usize, position: usize, total_at_position: usize) -> usize {
    if window == 0 || total <= window {
        return 0;
    }
    let max_start = total - window;
    if total_at_position <= 1 {
        return 0;
    }
    let denom = (total_at_position - 1) as f32;
    let fraction = (position.min(total_at_position - 1) as f32 / denom).clamp(0.0, 1.0);
    ((fraction * max_start as f32).round() as usize).min(max_start)
}

/// Where a fixed-scale minimap **window** onto real buffer lines should
/// start, so a host wanting VS Code's `minimap.size: "proportional"`
/// behaviour — a window that slides as the editor scrolls, reaching both
/// ends of the file — doesn't have to duplicate this fraction-of-buffer
/// arithmetic itself (issue #1044).
///
/// [`Minimap`]'s own `FixedPitch` slide (`slide_window_start_row`) only
/// ever sees `Minimap::lines` *after* [`sample_blocks`] has compressed it
/// down to (at most) the display's own row budget — every real caller
/// sizes `sample_blocks`'s `target_rows` to fit the strip, so
/// `row_count <= rows_shown` always holds by the time
/// [`Minimap::layout_with_sizing`] runs, and that slide branch never
/// engages. This function runs the same math one step earlier — over real
/// buffer lines, before sampling — so the window itself can be smaller
/// than the buffer and still slide; [`sample_window`] is the composition
/// of this with [`sample_blocks`] a host would normally reach for instead
/// of calling this directly.
///
/// `total_lines` — the whole buffer's line count.
/// `window_lines` — how many buffer lines the window should span, clamped
/// to `total_lines`.
/// `visible_row_start` — the editor's own current viewport top, in real
/// buffer-line units (not the sampled `Minimap::lines` index space
/// `Minimap::visible_row_start` uses post-sampling).
/// `total_buffer_lines` — normally the same value as `total_lines`; kept
/// as a separate parameter so a caller whose viewport tracks a different
/// (e.g. pre-folded) line count than the raw buffer can still anchor the
/// slide correctly, matching `slide_window_start_row`'s own two-parameter
/// shape (`self.lines` vs `self.total_buffer_lines`).
///
/// Returns the window's first real buffer line. `0` whenever the window
/// already covers the whole buffer (`window_lines >= total_lines`) —
/// nothing to slide.
pub fn window_start_line(
    total_lines: usize,
    window_lines: usize,
    visible_row_start: usize,
    total_buffer_lines: usize,
) -> usize {
    let window_lines = window_lines.min(total_lines);
    slide_start(
        total_lines,
        window_lines,
        visible_row_start,
        total_buffer_lines,
    )
}

/// Down-sample a **window** of `total_lines` real buffer lines into at
/// most `target_rows` [`MinimapLine`]s, sliding the window as
/// `visible_row_start` advances through the buffer — the composition of
/// [`window_start_line`] and [`sample_blocks`] issue #1044 asks for, so a
/// host no longer duplicates the window-then-sample arithmetic itself
/// (`vimcode`'s pre-#1044 `build_minimap_data` did exactly this by hand,
/// which is what left `slide_window_start_row` permanently defeated —
/// see [`window_start_line`]'s doc).
///
/// `window_lines` is how many real buffer lines the window spans *before*
/// `sample_blocks` compresses them down to `target_rows` — a host picks
/// this to trade off compression: `window_lines == target_rows` is the
/// crispest, uncompressed 1:1 scale; a larger `window_lines` folds more
/// real lines into each output row (`sample_blocks`'s own block
/// aggregation).
///
/// `line_at` is called with **real buffer line indices**, same accessor
/// contract as [`sample_blocks`] — this function applies the window's own
/// start-line shift internally, so a caller's accessor never needs to
/// know the window slid at all. Each returned [`MinimapLine::line_idx`]
/// is likewise a real buffer line number, with no caller-side shift
/// needed (unlike calling `sample_blocks` directly against a pre-sliced
/// window).
pub fn sample_window<F: FnMut(usize) -> String>(
    total_lines: usize,
    window_lines: usize,
    target_rows: usize,
    visible_row_start: usize,
    mut line_at: F,
) -> Vec<MinimapLine> {
    if total_lines == 0 || target_rows == 0 {
        return Vec::new();
    }
    let window_lines = window_lines.clamp(1, total_lines);
    let start = window_start_line(total_lines, window_lines, visible_row_start, total_lines);
    let mut lines = sample_blocks(window_lines, target_rows, |i| line_at(start + i));
    for line in &mut lines {
        line.line_idx += start;
    }
    lines
}

/// One output row's text for [`sample_blocks`] — see that function's doc
/// for the verbatim-vs-dithered split.
fn aggregate_block_text<F: FnMut(usize) -> String>(
    indices: &[usize],
    block_row: usize,
    line_at: &mut F,
) -> String {
    if indices.is_empty() {
        return String::new();
    }
    if let [only] = indices {
        return truncate_to_columns(&line_at(*only), COLUMN_CAPACITY).to_string();
    }
    let char_rows: Vec<Vec<char>> = indices
        .iter()
        .map(|&i| {
            truncate_to_columns(&line_at(i), COLUMN_CAPACITY)
                .chars()
                .collect()
        })
        .collect();
    let max_len = char_rows.iter().map(Vec::len).max().unwrap_or(0);
    let total = char_rows.len();
    (0..max_len)
        .map(|c| {
            let covered = char_rows
                .iter()
                .filter(|row| row.get(c).is_some_and(|ch| !ch.is_whitespace()))
                .count();
            if dither_threshold_met(covered, total, block_row, c) {
                'x'
            } else {
                ' '
            }
        })
        .collect()
}

/// Fold raw [`MinimapSpan`]s into a per-cell dominant colour, at whatever
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
pub fn aggregate_spans(spans: &[MinimapSpan], grid: MinimapGrid) -> Vec<MinimapSpan> {
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

// ── Character glyph atlas (#1035) ─────────────────────────────────────
//
// `MinimapRenderMode::Characters` was unreachable: `ROW_PITCH_PX` (2.0)
// never clears `LEGIBILITY_FLOOR_PX` (4.0), so every GUI rasteriser's
// `render_mode` call always resolved to `ColumnBlocks`, and the floor is
// *correct* about what it measures — a text shaper asked for a ~2px
// absolute font produces mush, not glyphs. What it gets wrong is the
// premise that shaping is the only way to put a recognisable glyph shape
// into a 2px-pitch row. VS Code's `MinimapCharRenderer` doesn't shape at
// the target size at all: it shapes once, *large*, into a sample sheet,
// then area-downsamples each cell to a tiny alpha tile and blits that —
// no shaping, no layout, on the paint path at all. `MinimapCharAtlas` is
// that facility, lifted here (not duplicated per backend) for the same
// reason `render_mode`/`minimap_font_px`/`ROW_PITCH_PX` were by #738: one
// shared interpretation instead of three independently-tuned ones.
//
// The pipeline a backend runs, once per `(font family, scale)` via
// [`MinimapAtlasCache`] rather than per frame:
// 1. Render [`ATLAS_CHAR_COUNT`] ASCII cells, large, into one sample
//    sheet (a backend-native paint call — Cairo/Pango, DirectWrite,
//    Core Text — outside this module's reach).
// 2. [`MinimapCharAtlas::from_alpha_sheet`] box-filter-downsamples each
//    cell to a `tile_w x tile_h` alpha tile (pure, portable, tested
//    below with no live surface).
// 3. [`MinimapCharAtlas::tile`] hands a backend its per-character alpha
//    tile at paint time; the backend blits it (alpha-blend, tinted by
//    the span colour) — still no shaping.

/// First and last ASCII code point [`MinimapCharAtlas`] samples — the
/// printable range `0x20` (space) through `0x7E` (`~`), inclusive:
/// [`ATLAS_CHAR_COUNT`] characters. VS Code's own `createSampleData`
/// describes this range as "96 chars"; the inclusive count is actually
/// 95 (`0x7E - 0x20 + 1`), so [`ATLAS_CHAR_COUNT`] is derived from the
/// bounds rather than hardcoded, to avoid carrying that off-by-one here.
pub const ATLAS_FIRST_CHAR: u32 = 0x20;
pub const ATLAS_LAST_CHAR: u32 = 0x7E;
/// Number of sampled ASCII code points — see [`ATLAS_FIRST_CHAR`].
pub const ATLAS_CHAR_COUNT: usize = (ATLAS_LAST_CHAR - ATLAS_FIRST_CHAR + 1) as usize;

/// An owned, backend-agnostic alpha atlas: one downsampled glyph tile per
/// ASCII code point in `ATLAS_FIRST_CHAR..=ATLAS_LAST_CHAR`, built once
/// from a backend-rendered sample sheet by [`Self::from_alpha_sheet`].
/// See the section docs above for the pipeline this is one step of.
#[derive(Debug, Clone, PartialEq)]
pub struct MinimapCharAtlas {
    tile_w: usize,
    tile_h: usize,
    /// `ATLAS_CHAR_COUNT * tile_w * tile_h` normalised alpha bytes, one
    /// `tile_w * tile_h` tile per code point, in `ATLAS_FIRST_CHAR..=
    /// ATLAS_LAST_CHAR` order.
    data: Vec<u8>,
    /// Returned by [`Self::tile`] for any code point outside the sampled
    /// range — VS Code falls back to a filled block for non-Latin text
    /// rather than painting nothing; matched here.
    fallback: Vec<u8>,
}

impl MinimapCharAtlas {
    /// Tile width/height in device pixels — whatever [`Self::from_alpha_sheet`]
    /// (or [`Self::filled`]) was built with.
    pub fn tile_w(&self) -> usize {
        self.tile_w
    }
    pub fn tile_h(&self) -> usize {
        self.tile_h
    }

    /// A fully-solid atlas: every tile, including the fallback, reads
    /// back all-`255`. This is the safe degraded result a backend falls
    /// back to when its sample-sheet render fails (e.g. surface
    /// allocation) instead of threading `Option<MinimapCharAtlas>`
    /// through every paint call site — blitting a filled block per
    /// non-blank character is exactly [`MinimapRenderMode::ColumnBlocks`]'s
    /// own look, so the degradation is visually inert, not a visible
    /// regression.
    pub fn filled(tile_w: usize, tile_h: usize) -> Self {
        let tile_w = tile_w.max(1);
        let tile_h = tile_h.max(1);
        let tile_len = tile_w * tile_h;
        Self {
            tile_w,
            tile_h,
            data: vec![255u8; tile_len * ATLAS_CHAR_COUNT],
            fallback: vec![255u8; tile_len],
        }
    }

    /// Build an atlas from a backend-rendered sample sheet.
    ///
    /// `alpha` is a tightly-packed, row-major coverage buffer (`0`
    /// transparent … `255` opaque), `sheet_w * sheet_h` bytes, holding
    /// [`ATLAS_CHAR_COUNT`] glyph cells side by side in `ATLAS_FIRST_CHAR..=
    /// ATLAS_LAST_CHAR` order — each `cell_w` pixels wide, `sheet_h`
    /// pixels tall (mirroring VS Code's `createSampleData`: 96 cells, a
    /// 10px advance, 16px tall, bold 16px font — this function doesn't
    /// care what size the backend actually shaped at, only that every
    /// cell shares one `cell_w`/`sheet_h`).
    ///
    /// Each cell is box-filter downsampled — fractional-edge area
    /// weighting, matching VS Code's `_downsampleChar` — to `tile_w x
    /// tile_h`. The whole sheet is then rescaled so its brightest sampled
    /// pixel reads back `255` (`_downsample`'s `255 / max` contrast
    /// normalisation) — without this step a lightly-anti-aliased glyph
    /// never reaches full opacity and the whole minimap reads as
    /// washed-out grey; this is most of why the result reads as text at
    /// all rather than a faint smear (see the section docs above).
    ///
    /// Returns [`Self::filled`] — a safe, visually-inert fallback —
    /// rather than panicking, if `alpha`'s length doesn't match `sheet_w
    /// * sheet_h` or the sheet isn't wide enough to hold
    /// [`ATLAS_CHAR_COUNT`] full `cell_w`-wide cells.
    pub fn from_alpha_sheet(
        alpha: &[u8],
        sheet_w: usize,
        sheet_h: usize,
        cell_w: usize,
        tile_w: usize,
        tile_h: usize,
    ) -> Self {
        let tile_w = tile_w.max(1);
        let tile_h = tile_h.max(1);
        if cell_w == 0
            || sheet_h == 0
            || alpha.len() != sheet_w * sheet_h
            || sheet_w < cell_w * ATLAS_CHAR_COUNT
        {
            return Self::filled(tile_w, tile_h);
        }

        let tile_len = tile_w * tile_h;
        let mut raw = vec![0f32; tile_len * ATLAS_CHAR_COUNT];
        for i in 0..ATLAS_CHAR_COUNT {
            let cell_x0 = i * cell_w;
            let cell =
                box_downsample_cell(alpha, sheet_w, sheet_h, cell_x0, cell_w, tile_w, tile_h);
            raw[i * tile_len..(i + 1) * tile_len].copy_from_slice(&cell);
        }

        Self {
            tile_w,
            tile_h,
            data: normalise_max_to_255(&raw),
            fallback: vec![255u8; tile_len],
        }
    }

    /// The `tile_w() * tile_h()` alpha tile for `ch` — the sampled ASCII
    /// tile when `ch` is in `ATLAS_FIRST_CHAR..=ATLAS_LAST_CHAR`,
    /// otherwise the filled-block fallback (see the struct docs).
    pub fn tile(&self, ch: char) -> &[u8] {
        match ascii_tile_index(ch) {
            Some(i) => {
                let tile_len = self.tile_w * self.tile_h;
                &self.data[i * tile_len..(i + 1) * tile_len]
            }
            None => &self.fallback,
        }
    }
}

fn ascii_tile_index(ch: char) -> Option<usize> {
    let c = ch as u32;
    if (ATLAS_FIRST_CHAR..=ATLAS_LAST_CHAR).contains(&c) {
        Some((c - ATLAS_FIRST_CHAR) as usize)
    } else {
        None
    }
}

/// Box-filter-downsample one `cell_w x sheet_h` cell — starting at
/// column `cell_x0` of the `sheet_w`-wide `alpha` buffer — to `tile_w x
/// tile_h`, using fractional-edge area weighting. See
/// [`MinimapCharAtlas::from_alpha_sheet`].
fn box_downsample_cell(
    alpha: &[u8],
    sheet_w: usize,
    sheet_h: usize,
    cell_x0: usize,
    cell_w: usize,
    tile_w: usize,
    tile_h: usize,
) -> Vec<f32> {
    let scale_x = cell_w as f64 / tile_w as f64;
    let scale_y = sheet_h as f64 / tile_h as f64;
    let mut out = vec![0f32; tile_w * tile_h];
    for ty in 0..tile_h {
        let y0 = ty as f64 * scale_y;
        let y1 = (((ty + 1) as f64) * scale_y).min(sheet_h as f64);
        let iy0 = y0.floor() as usize;
        let iy1 = (y1.ceil() as usize).min(sheet_h);
        for tx in 0..tile_w {
            let x0 = tx as f64 * scale_x;
            let x1 = (((tx + 1) as f64) * scale_x).min(cell_w as f64);
            let ix0 = x0.floor() as usize;
            let ix1 = (x1.ceil() as usize).min(cell_w);

            let mut sum = 0f64;
            for sy in iy0..iy1 {
                let wy = overlap(sy as f64, sy as f64 + 1.0, y0, y1);
                if wy <= 0.0 {
                    continue;
                }
                for local_sx in ix0..ix1 {
                    let wx = overlap(local_sx as f64, local_sx as f64 + 1.0, x0, x1);
                    if wx <= 0.0 {
                        continue;
                    }
                    let sx = cell_x0 + local_sx;
                    if sx >= sheet_w {
                        continue;
                    }
                    sum += alpha[sy * sheet_w + sx] as f64 * wx * wy;
                }
            }
            let area = scale_x * scale_y;
            out[ty * tile_w + tx] = if area > 0.0 { (sum / area) as f32 } else { 0.0 };
        }
    }
    out
}

/// Overlap length of intervals `[a0, a1)` and `[b0, b1)`, never negative.
fn overlap(a0: f64, a1: f64, b0: f64, b1: f64) -> f64 {
    (a1.min(b1) - a0.max(b0)).max(0.0)
}

/// Rescale `raw` so its maximum value reads back `255`, rounding and
/// clamping each element to `u8` — VS Code `_downsample`'s `255 / max`
/// contrast normalisation (see [`MinimapCharAtlas::from_alpha_sheet`]).
/// An all-zero `raw` (a totally blank sample sheet) stays all-zero
/// rather than dividing by zero.
fn normalise_max_to_255(raw: &[f32]) -> Vec<u8> {
    let max = raw.iter().cloned().fold(0f32, f32::max);
    if max <= 0.0 {
        return vec![0u8; raw.len()];
    }
    let scale = 255.0 / max;
    raw.iter()
        .map(|&v| (v * scale).round().clamp(0.0, 255.0) as u8)
        .collect()
}

/// Small single-entry cache mapping `(font family, scale)` to a built
/// [`MinimapCharAtlas`], so a font-scaling GUI backend builds its atlas
/// once per family/scale pair instead of re-shaping-and-downsampling on
/// every paint (#1035's "no per-frame shaping" requirement). Unlike
/// [`crate::image_cache::ImageCache`] this holds a single entry rather
/// than an LRU set: an app doesn't switch its editor font or display
/// scale mid-frame, so only one `(family, scale)` is ever live for a
/// given backend instance at a time in practice.
#[derive(Debug, Clone)]
pub struct MinimapAtlasCache {
    key: Option<(String, u32)>,
    atlas: Option<MinimapCharAtlas>,
}

impl MinimapAtlasCache {
    pub fn new() -> Self {
        Self {
            key: None,
            atlas: None,
        }
    }

    /// Look up the cached atlas for `(family, scale)`. On a miss (first
    /// call ever, or `family`/`scale` changed since the last call), runs
    /// `build` and caches its result before returning it.
    pub fn get_or_build(
        &mut self,
        family: &str,
        scale: f32,
        build: impl FnOnce() -> MinimapCharAtlas,
    ) -> &MinimapCharAtlas {
        // `f32` isn't `Hash`/`Eq`; a few decimal digits of scale
        // precision is plenty (real scale factors are values like `1.0`,
        // `1.25`, `1.5`, `2.0`) — same convention as `image_cache`'s own
        // `scale_millis`.
        let scale_key = (scale.max(0.0) * 1000.0).round() as u32;
        let hit = self
            .key
            .as_ref()
            .is_some_and(|(f, s)| f == family && *s == scale_key);
        if !hit {
            self.atlas = Some(build());
            self.key = Some((family.to_string(), scale_key));
        }
        self.atlas
            .as_ref()
            .expect("populated unconditionally above")
    }
}

impl Default for MinimapAtlasCache {
    fn default() -> Self {
        Self::new()
    }
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
            ..Default::default()
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
            ..Default::default()
        };
        assert_eq!(layout.hit_test(0.0, 0.0), MinimapHit::None);
        assert_eq!(layout.hit_test(25.0, 15.0), MinimapHit::None);
    }

    // ── sample_lines (deprecated shim, #1012) ───────────────────────────

    #[test]
    #[allow(deprecated)] // exercising the deprecated shim itself (#1012)
    fn sample_lines_empty_buffer_is_empty() {
        assert!(sample_lines(&[], 5).is_empty());
    }

    #[test]
    #[allow(deprecated)]
    fn sample_lines_zero_target_rows_is_empty_no_div_by_zero() {
        assert!(sample_lines(&["a", "b", "c"], 0).is_empty());
    }

    #[test]
    #[allow(deprecated)]
    fn sample_lines_never_upscales_small_files() {
        // 2 buffer lines, target 10 rows: keep exactly 2, not 10, and
        // (since neither block needs aggregating) the real text survives
        // verbatim.
        let out = sample_lines(&["a", "b"], 10);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].line_idx, 0);
        assert_eq!(out[1].line_idx, 1);
        assert_eq!(out[0].text, "a");
        assert_eq!(out[1].text, "b");
    }

    #[test]
    #[allow(deprecated)]
    fn sample_lines_downsamples_large_files_to_target_rows() {
        let owned: Vec<String> = (0..100).map(|i| format!("l{i}")).collect();
        let borrowed: Vec<&str> = owned.iter().map(String::as_str).collect();
        let out = sample_lines(&borrowed, 10);
        assert_eq!(out.len(), 10);
        assert_eq!(out[0].line_idx, 0);
        // Monotonically increasing source line indices.
        assert!(out.windows(2).all(|w| w[0].line_idx < w[1].line_idx));
    }

    #[test]
    #[allow(deprecated)]
    fn sample_lines_forwards_to_sample_blocks_byte_for_byte() {
        // The whole point of the shim: an existing `&[&str]` caller must
        // see exactly what `sample_blocks` would produce for the same
        // buffer, not some separately-maintained behaviour.
        let owned: Vec<String> = (0..50).map(|i| format!("line {i}")).collect();
        let borrowed: Vec<&str> = owned.iter().map(String::as_str).collect();
        let shim = sample_lines(&borrowed, 7);
        let direct = sample_blocks(borrowed.len(), 7, |i| borrowed[i].to_string());
        assert_eq!(shim, direct);
    }

    // ── block_bounds / block_sample_indices (#1012) ─────────────────────

    #[test]
    fn block_bounds_never_upscales() {
        assert_eq!(block_bounds(3, 10), vec![0, 1, 2, 3]);
    }

    #[test]
    fn block_bounds_downsamples_with_the_stride_formula() {
        assert_eq!(
            block_bounds(100, 10),
            vec![0, 10, 20, 30, 40, 50, 60, 70, 80, 90, 100]
        );
    }

    #[test]
    fn block_bounds_degenerate_inputs_do_not_panic() {
        assert_eq!(block_bounds(0, 10), vec![0]);
        assert_eq!(block_bounds(10, 0), vec![0]);
    }

    #[test]
    fn block_sample_indices_reads_every_line_under_the_cap() {
        assert_eq!(block_sample_indices(10, 15, 8), vec![10, 11, 12, 13, 14]);
    }

    #[test]
    fn block_sample_indices_spreads_evenly_when_capped() {
        let idx = block_sample_indices(0, 1000, 8);
        assert_eq!(idx.len(), 8);
        assert!(idx.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(idx[0], 0);
        assert!(*idx.last().unwrap() < 1000);
    }

    #[test]
    fn block_sample_indices_empty_span_is_empty() {
        assert!(block_sample_indices(5, 5, 8).is_empty());
    }

    // ── dither_threshold_met (owned here since #1012; TUI's per-dot use
    //    in `tui::braille` re-exports this exact function/matrix) ───────

    #[test]
    fn dither_threshold_met_zero_total_never_fires() {
        assert!(!dither_threshold_met(0, 0, 0, 0));
    }

    #[test]
    fn dither_threshold_met_full_coverage_always_fires() {
        for row in 0..4 {
            for col in 0..4 {
                assert!(dither_threshold_met(6, 6, row, col));
            }
        }
    }

    #[test]
    fn dither_threshold_met_zero_coverage_never_fires() {
        for row in 0..4 {
            for col in 0..4 {
                assert!(!dither_threshold_met(0, 6, row, col));
            }
        }
    }

    // ── sample_blocks (#1012) ────────────────────────────────────────

    #[test]
    fn sample_blocks_empty_or_zero_target_is_empty() {
        assert!(sample_blocks(0, 5, |_| String::new()).is_empty());
        assert!(sample_blocks(5, 0, |_| String::new()).is_empty());
    }

    #[test]
    fn sample_blocks_never_upscales_and_keeps_real_text_verbatim() {
        let lines = ["fn main() {", "    body();", "}"];
        let out = sample_blocks(lines.len(), 10, |i| lines[i].to_string());
        assert_eq!(out.len(), 3);
        for (i, line) in out.iter().enumerate() {
            assert_eq!(line.line_idx, i);
            assert_eq!(
                line.text, lines[i],
                "single-line block must not be dithered"
            );
        }
    }

    #[test]
    fn sample_blocks_no_line_in_a_block_is_ever_fully_discarded() {
        // The #1012 defect, reproduced directly: the old point sampler
        // for a single 1000-line block (target_rows=1) always picked
        // buffer line `0` (stride * r == 0 for r == 0) and discarded the
        // other 999 outright -- if line 0 happened to be blank, the
        // whole block painted as empty no matter what the other 999
        // lines contained. Line 0 here *is* blank; every other line
        // `block_sample_indices` samples (125, 250, ..., 875) carries an
        // `x` at a column that's a multiple of 4, which BAYER4's own
        // diagonal always dithers "on" for any nonzero coverage (row 0,
        // `col & 3 == 0` -> threshold 0) -- so the aggregated block must
        // show real content the old point sampler would have missed
        // entirely.
        let total = 1000;
        let lines: Vec<String> = (0..total)
            .map(|i| match i {
                0 => String::new(),
                125 => " ".repeat(4) + "x",
                250 => " ".repeat(8) + "x",
                375 => " ".repeat(12) + "x",
                500 => " ".repeat(16) + "x",
                625 => " ".repeat(20) + "x",
                750 => " ".repeat(24) + "x",
                875 => " ".repeat(28) + "x",
                _ => String::new(),
            })
            .collect();
        let out = sample_blocks(total, 1, |i| lines[i].clone());
        assert_eq!(out.len(), 1);
        assert!(
            out[0].text.contains('x'),
            "aggregated block text must retain real content from non-boundary lines, not just \
             the single line a point sampler would have picked"
        );
    }

    #[test]
    fn sample_blocks_respects_the_read_cap_per_block() {
        // A single huge block (target_rows=1) must call the accessor at
        // most BLOCK_LINE_SAMPLE_CAP times, not once per real line --
        // the whole point of capping the read (#1012's perf concern,
        // vimcode#1096).
        use std::cell::Cell;
        let calls = Cell::new(0usize);
        let total = 10_000;
        let out = sample_blocks(total, 1, |i| {
            calls.set(calls.get() + 1);
            format!("line{i}")
        });
        assert_eq!(out.len(), 1);
        assert!(
            calls.get() <= BLOCK_LINE_SAMPLE_CAP,
            "expected at most {BLOCK_LINE_SAMPLE_CAP} accessor calls, got {}",
            calls.get()
        );
    }

    #[test]
    fn sample_blocks_line_idx_is_each_blocks_start_line() {
        let total = 100;
        let out = sample_blocks(total, 10, |i| format!("l{i}"));
        assert_eq!(out.len(), 10);
        let expected_bounds = block_bounds(total, 10);
        for (row, line) in out.iter().enumerate() {
            assert_eq!(line.line_idx, expected_bounds[row]);
        }
    }

    // ── window_start_line / sample_window (#1044) ──────────────────

    #[test]
    fn window_start_line_whole_buffer_fits_needs_no_slide() {
        // window_lines >= total_lines: nothing to slide, regardless of
        // visible_row_start.
        assert_eq!(window_start_line(100, 100, 50, 100), 0);
        assert_eq!(window_start_line(100, 200, 99, 100), 0);
    }

    #[test]
    fn window_start_line_slides_monotonically_and_reaches_both_ends() {
        let total = 1000;
        let window = 100;
        let mut starts = Vec::new();
        for visible_row_start in [0, 100, 300, 500, 700, 900, 999] {
            starts.push(window_start_line(total, window, visible_row_start, total));
        }
        assert!(
            starts.windows(2).all(|w| w[0] <= w[1]),
            "window start must advance monotonically: {starts:?}"
        );
        assert_eq!(starts[0], 0, "scrolled to the top, window starts at 0");
        assert_eq!(
            *starts.last().unwrap(),
            total - window,
            "scrolled to the bottom, the window's last line must reach the file's end"
        );
    }

    #[test]
    fn window_start_line_zero_or_one_line_buffer_does_not_panic() {
        assert_eq!(window_start_line(0, 10, 0, 0), 0);
        assert_eq!(window_start_line(1, 10, 0, 1), 0);
    }

    #[test]
    fn sample_window_reads_only_the_windows_own_lines() {
        let total = 1000;
        let expected_start = window_start_line(total, 100, 900, total);
        let calls = std::cell::RefCell::new(Vec::new());
        let out = sample_window(total, 100, 10, 900, |i| {
            calls.borrow_mut().push(i);
            format!("l{i}")
        });
        assert_eq!(out.len(), 10);
        let seen = calls.borrow();
        // Every read must land inside the slid window, never before it
        // and never past the buffer's end.
        assert!(seen
            .iter()
            .all(|&i| i >= expected_start && i < expected_start + 100));
    }

    #[test]
    fn sample_window_line_idx_is_a_real_buffer_line_no_caller_shift_needed() {
        let total = 1000;
        let out = sample_window(total, 100, 10, 900, |i| format!("l{i}"));
        let expected_start = window_start_line(total, 100, 900, total);
        assert_eq!(out.first().unwrap().line_idx, expected_start);
        // Sliding to the very bottom of the file must let the last
        // sampled row's line_idx approach the buffer's own last line —
        // the same "both ends reachable" property `window_start_line`
        // itself guarantees.
        assert!(out.last().unwrap().line_idx < total);
    }

    #[test]
    fn sample_window_uncompressed_scale_matches_sample_blocks_over_the_window() {
        // window_lines == target_rows: the crispest, uncompressed 1:1
        // scale -- every output row is exactly one real buffer line, so
        // this must match calling sample_blocks directly over the same
        // (already-windowed) slice, shifted back to real line numbers.
        let total = 1000;
        let start = window_start_line(total, 50, 700, total);
        let direct = sample_blocks(50, 50, |i| format!("l{}", i + start));
        let via_window = sample_window(total, 50, 50, 700, |i| format!("l{i}"));
        assert_eq!(direct.len(), via_window.len());
        for (d, w) in direct.iter().zip(via_window.iter()) {
            assert_eq!(d.text, w.text);
            assert_eq!(d.line_idx + start, w.line_idx);
        }
    }

    #[test]
    fn sample_window_empty_inputs_are_empty() {
        assert!(sample_window(0, 10, 10, 0, |i| format!("l{i}")).is_empty());
        assert!(sample_window(100, 10, 0, 0, |i| format!("l{i}")).is_empty());
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
    fn sample_spans() -> Vec<MinimapSpan> {
        vec![
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
                line_idx: 1,
                start_col: 0,
                end_col: 4,
                color: red(),
            },
            MinimapSpan {
                line_idx: 2,
                start_col: 0,
                end_col: 4,
                color: blue(),
            },
            MinimapSpan {
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

    // ── MinimapSizing::VsCodeParity / resolve_width (#776) ─────────────

    #[test]
    fn resolve_width_returns_none_for_fill_and_fixed_pitch() {
        assert_eq!(MinimapSizing::Fill.resolve_width(800.0, 8.0), None);
        assert_eq!(
            MinimapSizing::FixedPitch(2.0).resolve_width(800.0, 8.0),
            None
        );
    }

    #[test]
    fn resolve_width_holds_steady_at_target_cols_for_a_wide_pane() {
        // Wide pane, pixel ("GTK-shaped") metric: `fraction` of the pane
        // comfortably affords `target_cols`, so `want` is the fixed
        // target — VS Code parity, the strip doesn't grow with the
        // window.
        let sizing = MinimapSizing::VsCodeParity {
            target_cols: 15.0,
            fraction: 0.15,
            min: 6.0,
            max: 30.0,
        };
        // char_width = 8.0px, pane = 1600px -> 200 cols; 0.15 * 200 = 30
        // cols affordable, target 15 wins the min().
        let got = sizing.resolve_width(1600.0, 8.0).unwrap();
        assert_eq!(got, 15.0 * 8.0);
    }

    #[test]
    fn resolve_width_shrinks_with_a_narrow_pane_down_to_the_floor() {
        let sizing = MinimapSizing::VsCodeParity {
            target_cols: 15.0,
            fraction: 0.15,
            min: 6.0,
            max: 30.0,
        };
        // char_width = 1.0 (cell-native), pane = 20 cols -> 0.15 * 20 = 3
        // cols wanted, floored to `min` (6.0).
        let got = sizing.resolve_width(20.0, 1.0).unwrap();
        assert_eq!(got, 6.0);
    }

    #[test]
    fn resolve_width_is_the_same_formula_across_native_units() {
        // The whole point of #776: one formula, no `if char_width > 1.0`
        // backend sniff. A cell-native call (char_width == 1.0, pane in
        // columns) and a pixel call (char_width == 8.0, pane scaled up by
        // the same factor) must agree once converted back to columns.
        let sizing = MinimapSizing::VsCodeParity {
            target_cols: 15.0,
            fraction: 0.2,
            min: 6.0,
            max: 30.0,
        };
        let cols_native = sizing.resolve_width(50.0, 1.0).unwrap();
        let px_native = sizing.resolve_width(50.0 * 8.0, 8.0).unwrap();
        assert_eq!(cols_native * 8.0, px_native);
    }

    #[test]
    fn resolve_width_treats_non_positive_char_width_as_one() {
        let sizing = MinimapSizing::VsCodeParity {
            target_cols: 15.0,
            fraction: 0.15,
            min: 6.0,
            max: 30.0,
        };
        assert_eq!(
            sizing.resolve_width(200.0, 0.0),
            sizing.resolve_width(200.0, 1.0)
        );
    }

    #[test]
    fn vscode_parity_in_row_layout_falls_back_to_fill_behavior() {
        // Not a real usage pattern (VsCodeParity governs width, not row
        // pitch) but must not panic — the match arm exists purely to
        // stay exhaustive, and should behave exactly like `Fill`.
        let mm = Minimap {
            id: WidgetId::new("mm"),
            lines: (0..8)
                .map(|i| MinimapLine {
                    text: String::new(),
                    line_idx: i,
                })
                .collect(),
            syntax_spans: Vec::new(),
            visible_row_start: 0,
            visible_row_count: 8,
            total_buffer_lines: 8,
        };
        let bounds = Rect::new(0.0, 0.0, 10.0, 40.0);
        let via_fill = mm.layout_with_sizing(bounds, 2, MinimapSizing::Fill);
        let via_vscode_parity = mm.layout_with_sizing(
            bounds,
            2,
            MinimapSizing::VsCodeParity {
                target_cols: 15.0,
                fraction: 0.15,
                min: 6.0,
                max: 30.0,
            },
        );
        assert_eq!(via_fill, via_vscode_parity);
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

    // ── MinimapCharAtlas (#1035) ────────────────────────────────────────

    /// Build a sample sheet of `ATLAS_CHAR_COUNT` cells, each `cell_w *
    /// sheet_h`, where every cell is blank (`0`) except the one for
    /// `target`, which is `pattern` (left-aligned, zero-padded to
    /// `cell_w * sheet_h`).
    fn sheet_with_one_cell(target: char, cell_w: usize, sheet_h: usize, pattern: &[u8]) -> Vec<u8> {
        let sheet_w = cell_w * ATLAS_CHAR_COUNT;
        let mut sheet = vec![0u8; sheet_w * sheet_h];
        let idx = ascii_tile_index(target).expect("target must be in the sampled ASCII range");
        let cell_x0 = idx * cell_w;
        for (i, &v) in pattern.iter().enumerate().take(cell_w * sheet_h) {
            let row = i / cell_w;
            let col = i % cell_w;
            sheet[row * sheet_w + cell_x0 + col] = v;
        }
        sheet
    }

    #[test]
    fn atlas_blank_cell_downsamples_to_all_zero() {
        // A space glyph has no ink anywhere in its cell -- box-filtering
        // an all-zero region must stay all-zero, and normalisation (a
        // pure rescale) must not turn zero into anything else.
        let cell_w = 10;
        let sheet_h = 16;
        // Give some other cell real ink so `max > 0` and normalisation
        // actually runs (a fully blank sheet is covered by
        // `atlas_all_blank_sheet_stays_all_zero`, below).
        let sheet = sheet_with_one_cell('!', cell_w, sheet_h, &vec![200u8; cell_w * sheet_h]);
        let atlas = MinimapCharAtlas::from_alpha_sheet(
            &sheet,
            cell_w * ATLAS_CHAR_COUNT,
            sheet_h,
            cell_w,
            2,
            4,
        );
        assert!(
            atlas.tile(' ').iter().all(|&b| b == 0),
            "a blank glyph cell must downsample to an all-zero tile"
        );
    }

    #[test]
    fn atlas_all_blank_sheet_stays_all_zero() {
        let cell_w = 10;
        let sheet_h = 16;
        let sheet = vec![0u8; cell_w * ATLAS_CHAR_COUNT * sheet_h];
        let atlas = MinimapCharAtlas::from_alpha_sheet(
            &sheet,
            cell_w * ATLAS_CHAR_COUNT,
            sheet_h,
            cell_w,
            2,
            4,
        );
        assert!(atlas.tile('a').iter().all(|&b| b == 0));
    }

    #[test]
    fn atlas_normalisation_drives_the_brightest_tile_to_255() {
        // Every sampled cell reads back the same partial coverage (100,
        // well under 255) -- without the `255 / max` rescale the whole
        // atlas would stay a washed-out ~100 and never read as legible
        // text (see the section docs on `from_alpha_sheet`).
        let cell_w = 10;
        let sheet_h = 16;
        let sheet = sheet_with_one_cell('x', cell_w, sheet_h, &vec![100u8; cell_w * sheet_h]);
        let atlas = MinimapCharAtlas::from_alpha_sheet(
            &sheet,
            cell_w * ATLAS_CHAR_COUNT,
            sheet_h,
            cell_w,
            2,
            4,
        );
        assert!(
            atlas.tile('x').iter().any(|&b| b == 255),
            "the brightest sampled tile must be rescaled to full opacity, got {:?}",
            atlas.tile('x')
        );
    }

    #[test]
    fn atlas_tile_count_and_stride_are_exact() {
        let cell_w = 10;
        let sheet_h = 16;
        let (tile_w, tile_h) = (2, 4);
        // Distinct, non-overlapping content per character (a uniform
        // value equal to that character's own ASCII code) so a stride
        // bug -- reading into a neighbouring tile -- shows up as a wrong
        // *value*, not just a wrong length.
        let sheet_w = cell_w * ATLAS_CHAR_COUNT;
        let mut sheet = vec![0u8; sheet_w * sheet_h];
        for i in 0..ATLAS_CHAR_COUNT {
            let v = ((i + 1) % 256) as u8;
            for row in 0..sheet_h {
                for col in 0..cell_w {
                    sheet[row * sheet_w + i * cell_w + col] = v;
                }
            }
        }
        let atlas =
            MinimapCharAtlas::from_alpha_sheet(&sheet, sheet_w, sheet_h, cell_w, tile_w, tile_h);

        for ch in ATLAS_FIRST_CHAR..=ATLAS_LAST_CHAR {
            let c = char::from_u32(ch).unwrap();
            let tile = atlas.tile(c);
            assert_eq!(
                tile.len(),
                tile_w * tile_h,
                "tile for {c:?} must be exactly tile_w * tile_h bytes"
            );
        }
        // Two distinct, uniformly-filled cells must not have bled into
        // each other: a solid-value cell downsamples to a uniform tile,
        // and different characters used different fill values above, so
        // their tiles must differ (unless the ASCII code happened to
        // wrap to the same value mod 256, which `(i+1)%256` avoids for
        // this 95-character range).
        assert_ne!(
            atlas.tile(' '),
            atlas.tile('!'),
            "adjacent cells must not alias each other's tile data"
        );
    }

    #[test]
    fn atlas_fractional_edge_box_filter_matches_hand_computed_values() {
        // cell_w=3 -> tile_w=2 downsample, scale_x=1.5: tile 0 covers
        // source pixel 0 fully (weight 1.0) and pixel 1 half (weight
        // 0.5); tile 1 covers pixel 1 half (weight 0.5) and pixel 2
        // fully (weight 1.0). sheet_h=1 -> tile_h=1 is a 1:1 no-op on
        // the vertical axis. Source (coverage, so `0..=255`) row
        // `[50, 90, 255]`:
        //   tile0 = (50*1.0 + 90*0.5) / 1.5 = 95 / 1.5 = 63.33333...
        //   tile1 = (90*0.5 + 255*1.0) / 1.5 = 300 / 1.5 = 200.0
        // Every other sampled cell is blank (0), so 200.0 is the whole
        // atlas's maximum -- normalisation rescales by 255 / 200.0 = 1.275:
        //   tile1' = 200.0 * 1.275 = 255 (exact, the max itself)
        //   tile0' = 63.33333 * 1.275 = 80.75 -> rounds to 81
        // Deliberately not a value ending in .5 in the *scaled* input
        // (a float-imprecision trap for `round()`), so this pins the box
        // filter's fractional weighting and the normalisation rescale
        // together against a value with clear rounding margin either
        // side.
        let cell_w = 3;
        let sheet_h = 1;
        let sheet = sheet_with_one_cell('Q', cell_w, sheet_h, &[50, 90, 255]);
        let atlas = MinimapCharAtlas::from_alpha_sheet(
            &sheet,
            cell_w * ATLAS_CHAR_COUNT,
            sheet_h,
            cell_w,
            2,
            1,
        );
        assert_eq!(atlas.tile('Q'), &[81, 255]);
    }

    #[test]
    fn atlas_malformed_sheet_falls_back_to_filled() {
        // A sheet too narrow to hold ATLAS_CHAR_COUNT cells must not
        // panic -- it degrades to the safe filled-block fallback.
        let atlas = MinimapCharAtlas::from_alpha_sheet(&[0u8; 4], 4, 1, 10, 2, 4);
        assert_eq!(atlas.tile(' '), &[255, 255, 255, 255, 255, 255, 255, 255]);
        assert_eq!(atlas.tile('~'), &[255, 255, 255, 255, 255, 255, 255, 255]);
    }

    #[test]
    fn atlas_tile_falls_back_to_filled_block_outside_ascii_range() {
        let cell_w = 10;
        let sheet_h = 16;
        let sheet = sheet_with_one_cell('x', cell_w, sheet_h, &vec![10u8; cell_w * sheet_h]);
        let atlas = MinimapCharAtlas::from_alpha_sheet(
            &sheet,
            cell_w * ATLAS_CHAR_COUNT,
            sheet_h,
            cell_w,
            2,
            4,
        );
        assert_eq!(atlas.tile('字'), &[255; 8]);
    }

    // ── MinimapAtlasCache (#1035) ───────────────────────────────────────

    #[test]
    fn atlas_cache_builds_once_for_repeated_same_key_lookups() {
        let mut cache = MinimapAtlasCache::new();
        let calls = std::cell::Cell::new(0);
        for _ in 0..5 {
            cache.get_or_build("Monospace", 1.0, || {
                calls.set(calls.get() + 1);
                MinimapCharAtlas::filled(1, 2)
            });
        }
        assert_eq!(
            calls.get(),
            1,
            "the build closure must run once across N paints at the same (family, scale) -- no per-frame shaping"
        );
    }

    #[test]
    fn atlas_cache_rebuilds_when_family_changes() {
        let mut cache = MinimapAtlasCache::new();
        let calls = std::cell::Cell::new(0);
        let build = |calls: &std::cell::Cell<i32>| {
            calls.set(calls.get() + 1);
            MinimapCharAtlas::filled(1, 2)
        };
        cache.get_or_build("Monospace", 1.0, || build(&calls));
        cache.get_or_build("Fira Code", 1.0, || build(&calls));
        assert_eq!(calls.get(), 2, "a different font family must rebuild");
    }

    #[test]
    fn atlas_cache_rebuilds_when_scale_changes() {
        let mut cache = MinimapAtlasCache::new();
        let calls = std::cell::Cell::new(0);
        let build = |calls: &std::cell::Cell<i32>| {
            calls.set(calls.get() + 1);
            MinimapCharAtlas::filled(1, 2)
        };
        cache.get_or_build("Monospace", 1.0, || build(&calls));
        cache.get_or_build("Monospace", 2.0, || build(&calls));
        assert_eq!(calls.get(), 2, "a different scale must rebuild");
    }
}
