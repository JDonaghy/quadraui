//! `TextDisplay` primitive: a scrollable, append-only viewer for streamed
//! text. Distinct from `Terminal` (which is VT100-aware with cursor and
//! attributes) and from `TextEditor` (deferred to A.9 — full editor with
//! cursor, selection, undo). `TextDisplay` is the right primitive for
//! log tails, command output, debug console, kubectl logs streams.
//!
//! The primitive itself is a `Vec<TextDisplayLine>` plus scroll + auto-
//! scroll state. Backends are expected to render efficiently — for
//! high-volume streams (≥10k lines/sec target per #144) backends may
//! diff only the appended slice rather than re-rasterising the whole
//! viewport. The primitive's append-only API (`append_line`, no
//! mid-buffer mutation) is what makes that diff cheap.
//!
//! **Status:** A.8 ships the primitive types only. Backend draw
//! functions and optimised partial-repaint paths land when the first
//! consumer (kubectl logs viewer #145, LSP trace viewer, etc.) needs
//! them.
//!
//! # Backend contract
//!
//! **Declarative + auto-scroll convention.** Render
//! `lines[scroll_offset..]` from top to bottom of the viewport. Each
//! `TextDisplayLine` carries pre-styled spans + an optional `decoration`
//! (info / warn / error tint) + an optional `timestamp`. Backends paint
//! the spans, optionally tint the row by decoration, optionally render
//! the timestamp prefix in a dim style.
//!
//! **Auto-scroll handling**: when `auto_scroll == true`, the backend
//! ignores `scroll_offset` and pins the view to the bottom (newest
//! lines). When the user scrolls up, the backend (or the app on its
//! behalf) sets `auto_scroll = false` and respects `scroll_offset`
//! until the user scrolls back to the bottom.
//!
//! **Performance**: for high-volume streams, backends may diff only the
//! newly-appended lines (the primitive is append-only — `append_line` /
//! `clear` / cap-eviction are the only mutations) and repaint just the
//! affected rows. Reference implementations land with the first
//! consumer.

use crate::event::Rect;
use crate::types::{Decoration, Modifiers, StyledSpan, StyledText, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a `TextDisplay`.
///
/// Lines are rendered top-to-bottom in insertion order. `scroll_offset`
/// is the index of the first visible line (0 = top). When `auto_scroll`
/// is true the backend should clamp `scroll_offset` to keep the most
/// recent line visible after each `append_line` — paused only when the
/// user explicitly scrolls upward.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextDisplay {
    pub id: WidgetId,
    pub lines: Vec<TextDisplayLine>,
    /// Index of the first visible line. `0` = top.
    #[serde(default)]
    pub scroll_offset: usize,
    /// When true, backends auto-scroll to keep the latest line visible.
    /// Toggled off when the user scrolls upward, re-enabled when they
    /// scroll back to the bottom.
    #[serde(default = "default_auto_scroll")]
    pub auto_scroll: bool,
    /// Maximum lines to retain in the ring buffer. `0` = unbounded.
    /// Helpful for log tails where memory can grow without bound.
    #[serde(default)]
    pub max_lines: usize,
    #[serde(default)]
    pub has_focus: bool,
    /// Optional title row painted above the body. The body's visible
    /// height shrinks by one row when present; spans render as-is so
    /// callers control colour/bold/etc. Backends consume this directly
    /// — no bespoke title painter needed.
    #[serde(default)]
    pub title: Option<StyledText>,
    /// When true, a vertical scrollbar is rendered at the trailing edge
    /// of the viewport. The body's visible width shrinks by the scrollbar
    /// gutter width (1 cell on TUI, ~12px on GTK). The scrollbar's
    /// thumb + track hit regions are included in the layout's
    /// `hit_regions` for drag interaction.
    #[serde(default)]
    pub show_scrollbar: bool,
}

fn default_auto_scroll() -> bool {
    true
}

/// Backend-shared line-wrapping math for [`TextDisplay`] (quadraui#905).
///
/// Every rasteriser — `tui`, and the shared [`native_surface_paint::paint`]
/// behind `gtk`/`macos`/`win` — computes its wrap decisions here rather
/// than per-backend, so paint and the pure hit-testing layout helpers can
/// never disagree about how many rows a line occupies (see
/// [`wrap::wrap_row_count`]'s doc, and quadraui#494).
///
/// **Why the whole module is `cfg`-gated to the rasteriser features.**
/// These helpers exist *only* to serve rasterisers. A default-feature
/// `quadraui` build (no `tui`/`gtk`/`win`/`macos`) compiles the primitive
/// types but no backend, so nothing calls them — and `cargo check
/// -p quadraui --all-targets` under `RUSTFLAGS="-D warnings"` turns that
/// into five hard `dead_code` errors, not warnings. Gating the module is
/// the same fix (and the same reasoning) that [`wrap::px_to_cols`] already
/// carries one level narrower. The gate must stay a superset of every
/// backend that calls into here: adding a new rasteriser feature means
/// adding it to this list.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
mod wrap {
    use super::{StyledSpan, TextDisplayLine};

    /// Glyph prefixed to every continuation row of a wrapped
    /// [`super::TextDisplay`] line (quadraui#905). Without it, a wrapped
    /// row and a genuinely new line are visually identical, so a reader
    /// can't tell "this continues" from "this is unrelated."
    /// U+21B3 (DOWNWARDS ARROW WITH TIP RIGHTWARDS) plus a separating
    /// space: a single narrow glyph, so it costs a small, predictable,
    /// constant column budget on every row (see
    /// [`wrap_continuation_marker_width`]).
    pub(crate) const WRAP_CONTINUATION_MARKER: &str = "\u{21B3} ";

    /// Display-cell width of [`WRAP_CONTINUATION_MARKER`] (see
    /// [`crate::text_util::display_width`]).
    pub(crate) fn wrap_continuation_marker_width() -> usize {
        crate::text_util::display_width(WRAP_CONTINUATION_MARKER)
    }

    /// Column budget consumed by `line`'s timestamp prefix (its display
    /// width plus one separating space), or `0` when the line has none.
    /// Shared by every backend so the wrap column accounting always agrees
    /// with how much room the timestamp actually occupies.
    pub(crate) fn line_timestamp_cols(line: &TextDisplayLine) -> usize {
        line.timestamp
            .as_ref()
            .map(|ts| crate::text_util::display_width(ts) + 1)
            .unwrap_or(0)
    }

    /// Word-wrap one [`TextDisplayLine`]'s spans to `col_budget` display
    /// cells. Thin, backend-shared adapter over
    /// [`crate::text_util::wrap_spans`] (`WrapPolicy::Word`) — the crate's
    /// one line wrapper — so every rasteriser makes the same wrap
    /// decision. `col_budget` is the room left for content *after* the
    /// caller has already reserved timestamp/marker gutter width (see
    /// [`line_timestamp_cols`], [`wrap_continuation_marker_width`]) —
    /// this function only wraps what's left.
    pub(crate) fn wrap_display_line(
        line: &TextDisplayLine,
        col_budget: usize,
    ) -> Vec<Vec<StyledSpan>> {
        crate::text_util::wrap_spans(&line.spans, col_budget, crate::text_util::WrapPolicy::Word)
    }

    /// The column budget actually left for `line`'s span text after
    /// reserving gutter width for whichever is wider: `line`'s timestamp
    /// prefix (row 0 only) or the continuation marker (every other row).
    ///
    /// **The one place this subtraction is allowed to happen.** Every
    /// caller that needs "how much room is left for content" —
    /// [`wrap_row_count`] (row-count measurement), [`native_surface_paint::paint`]
    /// (actual pixel-backend painting), and TUI's `line_rows`
    /// (`crate::tui::text_display`, actual cell painting) — goes through
    /// this function instead of re-deriving `col_budget.saturating_sub(gutter)`
    /// locally. Before this helper existed the same three-line formula was
    /// duplicated verbatim at all three call sites, which is exactly the
    /// kind of drift that caused #494's paint/layout row-count mismatch:
    /// a future edit to the formula in only one copy would silently
    /// reintroduce it. A single function can't drift from itself.
    ///
    /// Reserving the wider of the two gutters for *every* row (including
    /// row 0, which never draws a marker, and continuation rows, which
    /// never draw a timestamp) is deliberately conservative rather than
    /// exact: it keeps this one function usable for both row-count
    /// measurement and per-row painting without either needing to know
    /// which row it's computing for, at the cost of a line with no
    /// timestamp wrapping very slightly earlier than the pixels available
    /// to it would strictly allow. That trade-off is what keeps
    /// [`wrap_row_count`] (measurement, no row index) and the paint loops
    /// (per-row, but budgeted once up front) unable to disagree.
    pub(crate) fn content_budget_cols(line: &TextDisplayLine, col_budget: usize) -> usize {
        let gutter = line_timestamp_cols(line).max(wrap_continuation_marker_width());
        col_budget.saturating_sub(gutter).max(1)
    }

    /// Number of visual rows `line` occupies at `col_budget` display cells.
    /// `TextDisplay` always wraps over-long lines (quadraui#905) — this is
    /// not a caller-toggleable option (see the module doc: the primitive is
    /// documented exclusively for prose/log content, which has no fixed-
    /// column use case; that's what `DataTable` is for). Shared by every
    /// backend's `measure_line` closure (paint) *and* its pure layout
    /// counterpart (hit-testing), so the two can never disagree about row
    /// counts — see quadraui#494 (layout/paint parity) and #905 (this
    /// function's reason for existing).
    pub(crate) fn wrap_row_count(line: &TextDisplayLine, col_budget: usize) -> usize {
        wrap_display_line(line, content_budget_cols(line, col_budget))
            .len()
            .max(1)
    }

    /// Convert a pixel width to a display-cell column budget using an
    /// approximate average character width — the same "no live measurement
    /// context available" approximation
    /// [`crate::backend::Backend::char_width`] exists for (used by
    /// hit-testing layout helpers that run outside a paint pass). Pixel
    /// backends (GTK/macOS/Windows) use this so their wrap column budget is
    /// computed identically whether or not a live rendering surface is on
    /// hand — see [`wrap_row_count`]'s doc on why paint and layout must
    /// agree.
    ///
    /// `cfg`-gated one level narrower than the enclosing module: TUI cells
    /// are already a 1:1 column unit (no conversion needed), so under a
    /// bare `--features tui` build this function has no caller — and CI's
    /// tui leg runs with `-D warnings`, so an ungated `pub(crate) fn` here
    /// would fail that leg on unused-function, not just warn.
    #[cfg(any(
        feature = "gtk",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    pub(crate) fn px_to_cols(width_px: f32, char_width: f32) -> usize {
        if char_width <= 0.0 || width_px <= 0.0 {
            0
        } else {
            (width_px / char_width).floor() as usize
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::types::Decoration;

        fn make_td_line(text: &str) -> TextDisplayLine {
            TextDisplayLine {
                spans: vec![StyledSpan::plain(text)],
                decoration: Decoration::Normal,
                timestamp: None,
            }
        }

        #[test]
        fn wrap_display_line_short_line_is_one_row() {
            let line = make_td_line("short");
            let rows = wrap_display_line(&line, 80);
            assert_eq!(rows.len(), 1);
        }

        #[test]
        fn wrap_display_line_long_line_produces_multiple_rows() {
            let line = make_td_line("the quick brown fox jumps over the lazy dog");
            let rows = wrap_display_line(&line, 10);
            assert!(rows.len() > 1, "expected wrapping, got {rows:?}");
            for row in &rows {
                let w: usize = row
                    .iter()
                    .map(|s| crate::text_util::display_width(&s.text))
                    .sum();
                assert!(w <= 10, "row {row:?} exceeds the 10-cell budget");
            }
        }

        #[test]
        fn wrap_row_count_matches_wrap_display_line_len() {
            let line = make_td_line("the quick brown fox jumps over the lazy dog");
            assert_eq!(
                wrap_row_count(&line, 10),
                wrap_display_line(
                    &line,
                    10_usize.saturating_sub(wrap_continuation_marker_width())
                )
                .len()
            );
        }

        #[test]
        fn line_timestamp_cols_accounts_for_separator_space() {
            let mut line = make_td_line("x");
            assert_eq!(line_timestamp_cols(&line), 0);
            line.timestamp = Some("12:00:00".to_string());
            assert_eq!(line_timestamp_cols(&line), 8 + 1);
        }

        #[test]
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        fn px_to_cols_floors_and_handles_degenerate_input() {
            assert_eq!(px_to_cols(100.0, 8.0), 12);
            assert_eq!(px_to_cols(100.0, 0.0), 0);
            assert_eq!(px_to_cols(0.0, 8.0), 0);
        }
    }
}

// Re-exported at the old `primitives::text_display::*` paths so every
// backend call site keeps its existing import. Two `use`s, not one: the
// pixel-only `px_to_cols` keeps the narrower gate it carries inside
// `wrap` (see its doc).
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub(crate) use wrap::{
    content_budget_cols, wrap_display_line, wrap_row_count, WRAP_CONTINUATION_MARKER,
};

#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub(crate) use wrap::px_to_cols;

/// One line in a `TextDisplay`. Carries styled spans plus an optional
/// decoration tag (Error/Warning/Muted/Header) for log-level styling and
/// an optional left-aligned timestamp string the backend renders in a
/// dim colour.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextDisplayLine {
    pub spans: Vec<StyledSpan>,
    #[serde(default)]
    pub decoration: Decoration,
    /// Optional timestamp prefix (e.g. `"12:34:56"`) rendered before spans.
    #[serde(default)]
    pub timestamp: Option<String>,
}

// ── D6 Layout API ───────────────────────────────────────────────────────────
//
// Per Decision D6: primitives return fully-resolved `Layout` structs.
// Eighth primitive on the new shape. TextDisplay is a vertical stack
// of log lines, with auto-scroll support: when `auto_scroll` is true,
// the layout pins to the bottom (newest lines visible) regardless of
// the input `scroll_offset`. The backend doesn't need to compute this
// itself — `resolved_scroll_offset` is correct either way.

/// Per-line measurement supplied by the backend. Single-line displays
/// usually have a uniform `height`, but wrap-enabled backends can vary
/// it (e.g. a long line that wraps onto 3 visual rows returns `3.0`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextDisplayLineMeasure {
    pub height: f32,
}

impl TextDisplayLineMeasure {
    pub fn new(height: f32) -> Self {
        Self { height }
    }

    /// Build from the backend's own [`crate::backend::Metrics`]
    /// (`backend.measure()`) for the common uniform-height case — one
    /// display line is one text row (quadraui#817). Wrap-enabled
    /// backends that vary height per line still use [`Self::new`]
    /// directly.
    pub fn from_metrics(m: &crate::backend::Metrics) -> Self {
        Self::new(m.line_height)
    }
}

/// Resolved position of one visible text-display line after layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleTextDisplayLine {
    /// Index into `TextDisplay.lines`.
    pub line_idx: usize,
    pub bounds: Rect,
}

/// Classification of a hit-test result. Clicks on log lines usually
/// start a text-selection or copy action; the primitive reports which
/// line was hit and the backend decides what to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextDisplayHit {
    Line(usize),
    ScrollbarThumb,
    ScrollbarTrackBefore,
    ScrollbarTrackAfter,
    Empty,
}

/// Fully-resolved text-display layout.
#[derive(Debug, Clone, PartialEq)]
pub struct TextDisplayLayout {
    pub viewport_width: f32,
    pub viewport_height: f32,
    pub visible_lines: Vec<VisibleTextDisplayLine>,
    pub hit_regions: Vec<(Rect, TextDisplayHit)>,
    /// Scroll offset actually used. When `auto_scroll` is true, this is
    /// chosen so the last line is visible; otherwise it's the input
    /// `scroll_offset` clamped to `[0, lines.len())`. Backends should
    /// write this back to the app's stored value so auto-scroll state
    /// is coherent across frames.
    pub resolved_scroll_offset: usize,
    /// Scrollbar gutter bounds (when `show_scrollbar` is true).
    pub scrollbar_bounds: Option<Rect>,
    /// Scrollbar thumb bounds within the gutter.
    pub thumb_bounds: Option<Rect>,
}

impl TextDisplayLayout {
    pub fn hit_test(&self, x: f32, y: f32) -> TextDisplayHit {
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        TextDisplayHit::Empty
    }
}

impl TextDisplay {
    /// Compute the full rendering + hit-test layout for this display.
    ///
    /// # Auto-scroll
    ///
    /// When `self.auto_scroll == true`, the layout chooses the
    /// smallest `resolved_scroll_offset` such that the last line is
    /// still visible — overriding the stored `scroll_offset`. When
    /// `auto_scroll == false`, `scroll_offset` is used as-is (clamped).
    ///
    /// # Arguments
    ///
    /// - `viewport_width`, `viewport_height` — display area.
    /// - `measure_line(i)` — height for line `i`. Most backends use a
    ///   uniform height; wrap-enabled renderers return the wrapped-line
    ///   row count × base height.
    pub fn layout<F>(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        measure_line: F,
    ) -> TextDisplayLayout
    where
        F: Fn(usize) -> TextDisplayLineMeasure,
    {
        self.layout_inner(viewport_width, viewport_height, 0.0, 0.0, measure_line)
    }

    /// Layout with scrollbar gutter reserved on the right.
    /// `scrollbar_gutter` is the width in native units (1.0 for TUI,
    /// ~12.0 for GTK). `min_thumb` is the minimum thumb length.
    pub fn layout_with_scrollbar<F>(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        scrollbar_gutter: f32,
        min_thumb: f32,
        measure_line: F,
    ) -> TextDisplayLayout
    where
        F: Fn(usize) -> TextDisplayLineMeasure,
    {
        self.layout_inner(
            viewport_width,
            viewport_height,
            scrollbar_gutter,
            min_thumb,
            measure_line,
        )
    }

    fn layout_inner<F>(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        scrollbar_gutter: f32,
        min_thumb: f32,
        measure_line: F,
    ) -> TextDisplayLayout
    where
        F: Fn(usize) -> TextDisplayLineMeasure,
    {
        let mut visible_lines: Vec<VisibleTextDisplayLine> = Vec::new();
        let mut hit_regions: Vec<(Rect, TextDisplayHit)> = Vec::new();

        let body_width = if self.show_scrollbar && scrollbar_gutter > 0.0 {
            (viewport_width - scrollbar_gutter).max(0.0)
        } else {
            viewport_width
        };

        if self.lines.is_empty() || viewport_height <= 0.0 {
            return TextDisplayLayout {
                viewport_width,
                viewport_height,
                visible_lines,
                hit_regions,
                resolved_scroll_offset: 0,
                scrollbar_bounds: None,
                thumb_bounds: None,
            };
        }

        // Decide the starting offset.
        let resolved_scroll_offset = if self.auto_scroll {
            let mut used = 0.0_f32;
            let mut offset = self.lines.len();
            while offset > 0 {
                let cand = offset - 1;
                let h = measure_line(cand).height;
                if used + h > viewport_height + f32::EPSILON {
                    break;
                }
                used += h;
                offset = cand;
            }
            offset
        } else {
            crate::primitives::scrollbar::clamp_scroll_offset(self.scroll_offset, self.lines.len())
        };

        let mut y = 0.0_f32;
        for i in resolved_scroll_offset..self.lines.len() {
            if y >= viewport_height {
                break;
            }
            let m = measure_line(i);
            let remaining = viewport_height - y;
            let height = m.height.min(remaining).max(0.0);
            if height <= 0.0 {
                break;
            }
            let bounds = Rect::new(0.0, y, body_width, height);
            visible_lines.push(VisibleTextDisplayLine {
                line_idx: i,
                bounds,
            });
            hit_regions.push((bounds, TextDisplayHit::Line(i)));
            y += m.height;
        }

        // Scrollbar.
        let (scrollbar_bounds, thumb_bounds) =
            if self.show_scrollbar && scrollbar_gutter > 0.0 && !self.lines.is_empty() {
                let gutter = Rect::new(body_width, 0.0, scrollbar_gutter, viewport_height);
                let visible_count = visible_lines.len() as f32;
                let total = self.lines.len() as f32;
                let (thumb_start, thumb_len) = crate::primitives::scrollbar::fit_thumb(
                    resolved_scroll_offset as f32,
                    total,
                    visible_count,
                    viewport_height,
                    min_thumb,
                );

                if thumb_len > 0.0 {
                    let thumb = Rect::new(body_width, thumb_start, scrollbar_gutter, thumb_len);
                    let track_before = Rect::new(body_width, 0.0, scrollbar_gutter, thumb_start);
                    let track_after = Rect::new(
                        body_width,
                        thumb_start + thumb_len,
                        scrollbar_gutter,
                        (viewport_height - thumb_start - thumb_len).max(0.0),
                    );

                    hit_regions.insert(0, (thumb, TextDisplayHit::ScrollbarThumb));
                    if track_before.height > 0.0 {
                        hit_regions.insert(1, (track_before, TextDisplayHit::ScrollbarTrackBefore));
                    }
                    if track_after.height > 0.0 {
                        hit_regions.insert(
                            if track_before.height > 0.0 { 2 } else { 1 },
                            (track_after, TextDisplayHit::ScrollbarTrackAfter),
                        );
                    }
                    (Some(gutter), Some(thumb))
                } else {
                    (Some(gutter), None)
                }
            } else {
                (None, None)
            };

        TextDisplayLayout {
            viewport_width,
            viewport_height,
            visible_lines,
            hit_regions,
            resolved_scroll_offset,
            scrollbar_bounds,
            thumb_bounds,
        }
    }
}

/// Events a `TextDisplay` emits back to the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextDisplayEvent {
    /// User scrolled the view (mouse wheel, PageUp/Down, etc.).
    /// `new_offset` is the post-scroll `scroll_offset`. Apps update
    /// `auto_scroll` based on whether the new offset reached the bottom.
    Scrolled { new_offset: usize },
    /// User toggled auto-scroll (typically via a keyboard shortcut or
    /// click on a "Follow" indicator).
    AutoScrollToggled { enabled: bool },
    /// User initiated a copy of selected lines.
    Copied { text: String },
    /// A key was pressed with the display focused and the primitive
    /// did not consume it.
    KeyPressed { key: String, modifiers: Modifiers },
}

impl TextDisplay {
    /// Construct an empty `TextDisplay` with the given id.
    pub fn new(id: WidgetId) -> Self {
        Self {
            id,
            lines: Vec::new(),
            scroll_offset: 0,
            auto_scroll: true,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: false,
        }
    }

    /// Append a line to the end of the buffer. Honours `max_lines` by
    /// dropping the oldest line(s) when the buffer would grow past the cap.
    pub fn append_line(&mut self, line: TextDisplayLine) {
        self.lines.push(line);
        if self.max_lines > 0 && self.lines.len() > self.max_lines {
            let drop = self.lines.len() - self.max_lines;
            self.lines.drain(..drop);
            // Adjust scroll offset so the visible region stays put when
            // we evict older lines.
            self.scroll_offset = self.scroll_offset.saturating_sub(drop);
        }
    }

    /// Drop all lines and reset scroll to top.
    pub fn clear(&mut self) {
        self.lines.clear();
        self.scroll_offset = 0;
    }

    /// Set the max retention; when set lower than the current line count,
    /// trims oldest lines immediately.
    pub fn set_max_lines(&mut self, max: usize) {
        self.max_lines = max;
        if max > 0 && self.lines.len() > max {
            let drop = self.lines.len() - max;
            self.lines.drain(..drop);
            self.scroll_offset = self.scroll_offset.saturating_sub(drop);
        }
    }
}

// ── NativeSurface paint (#810, Phase 2c of the NativeSurface milestone) ────
//
// Before this, `gtk::text_display::draw_text_display`,
// `macos::text_display::draw_text_display` and
// `win::text_display::draw_text_display` each independently painted the
// optional title row, per-line spans/timestamp, and scrollbar gutter +
// thumb with their own Cairo / CoreGraphics / Direct2D calls — the three
// were already near-identical (macOS's own module doc: "Mirror of
// `gtk::text_display::draw_text_display`"; Windows's: "Mirrors
// `macos::text_display::draw_text_display`"). `paint` below is the one
// shared implementation, written against
// [`crate::native_surface::NativeSurface`] (#807, Phase 1).
//
// Each backend keeps its own `*_text_display_layout` free function
// (`gtk_text_display_layout`/`mac_text_display_layout`/
// `win_text_display_layout`) — unlike `paint`, that helper is pure
// `TextDisplay::layout`/`layout_with_scrollbar` math with zero
// Cairo/CoreGraphics/Direct2D dependency, so it was already
// backend-agnostic; unifying it into a fourth shared function is a
// natural follow-on but outside this issue's stated scope ("move …
// paint"), so it's left as three near-identical copies for now, the
// same call made for `*_chart_layout` in #810's chart migration.
//
// Behavioural divergence found while unifying (not resolved silently,
// per this issue's acceptance bar): **bold span text, on Windows only.**
// `win::text_display::draw_text_display` measured/drew each span with
// `DWrite::measure_text_styled`/`draw_text_styled(.., span.bold)`; GTK
// and macOS never read `StyledSpan::bold` at all — Pango/Core Text
// bold-weight selection was never wired into either rasteriser.
// `NativeSurface::surface_measure_text`/`surface_draw_text_run` carry no
// bold parameter (see that trait's module doc's "~15 drawing verbs" —
// weight selection isn't one of them), so there is no way to preserve
// Windows's behaviour through this trait. `paint` adopts the
// two-out-of-three shape: `StyledSpan::bold` is not painted specially on
// any pixel backend. TUI's own `tui::text_display` is untouched.
//
// `#[allow(dead_code)]`: see `primitives::form`'s identical note (#808)
// — only *called* once a real pixel backend is compiled in, exercised by
// each backend's own `Backend::draw_text_display` call site plus this
// module's own `RecordingSurface` tests on every leg that enables one of
// the three cfg'd features.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
mod native_surface_paint {
    use super::{
        content_budget_cols, px_to_cols, wrap_display_line, wrap_row_count, TextDisplay,
        TextDisplayLineMeasure, WRAP_CONTINUATION_MARKER,
    };
    use crate::native_surface::NativeSurface;
    use crate::theme::Theme;
    use crate::types::Decoration;
    use crate::Rect;

    /// Paint a [`TextDisplay`] into `rect` on `surface`.
    ///
    /// Mirrors every deleted per-backend `draw_text_display`: fills
    /// `rect` with [`Theme::background`], paints an optional title row
    /// (shrinking the body by one `line_height`), then each visible
    /// line's optional timestamp + spans (span `bg` filled before its
    /// text, per-line `Decoration` resolving a fallback `fg`), then an
    /// optional scrollbar gutter + thumb at the trailing edge.
    ///
    /// `line_height` and the resulting layout math are identical to
    /// what `gtk_text_display_layout`/`mac_text_display_layout`/
    /// `win_text_display_layout` already compute — see this module's
    /// doc for why those three stay separate, pure-math copies rather
    /// than also being unified here.
    ///
    /// `char_width` is the backend's approximate average character width
    /// (`Backend::char_width`), used to convert `rect`'s pixel width into
    /// the same "display cells" unit the wrap decision budgets in —
    /// see [`px_to_cols`]'s doc for why an approximation (rather than a
    /// real per-glyph measurement, which `surface` could give here) is
    /// the correct choice: the pure `*_text_display_layout` hit-testing
    /// helpers have no live surface to measure with, so paint must use
    /// the same approximation they do or a click could resolve to a
    /// different row than what's on screen (quadraui#494/#905).
    pub(crate) fn paint(
        display: &TextDisplay,
        rect: Rect,
        surface: &mut dyn NativeSurface,
        theme: &Theme,
        line_height: f32,
        char_width: f32,
    ) {
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }

        surface.surface_fill_rect(rect, theme.background);

        // Optional title row at the top. Body shrinks by `line_height`
        // when present.
        let (body_y, body_h) = if let Some(ref title) = display.title {
            let mut cursor_x = rect.x;
            for span in &title.spans {
                let span_fg = span.fg.unwrap_or(theme.foreground);
                let (sw, _) = surface.surface_measure_text(&span.text);
                surface.surface_draw_text_run(
                    Rect::new(cursor_x, rect.y, sw.max(1.0), line_height),
                    &span.text,
                    span_fg,
                );
                cursor_x += sw;
            }
            (rect.y + line_height, (rect.height - line_height).max(0.0))
        } else {
            (rect.y, rect.height)
        };
        if body_h <= 0.0 {
            return;
        }

        let gutter = 12.0_f32;
        let min_thumb = 8.0_f32;
        let body_width_px = if display.show_scrollbar {
            (rect.width - gutter).max(0.0)
        } else {
            rect.width
        };
        let col_budget = px_to_cols(body_width_px, char_width);

        let measure = |i: usize| {
            let rows = wrap_row_count(&display.lines[i], col_budget);
            TextDisplayLineMeasure::new(rows as f32 * line_height)
        };
        let layout = if display.show_scrollbar {
            display.layout_with_scrollbar(rect.width, body_h, gutter, min_thumb, measure)
        } else {
            display.layout(rect.width, body_h, measure)
        };

        for vis in &layout.visible_lines {
            let line = &display.lines[vis.line_idx];
            let row0_y = body_y + vis.bounds.y;
            if row0_y >= body_y + body_h {
                break;
            }

            let line_fg = match line.decoration {
                Decoration::Error => theme.error_fg,
                Decoration::Warning => theme.warning_fg,
                Decoration::Muted => theme.muted_fg,
                _ => theme.foreground,
            };

            // Word-wrap onto continuation rows (quadraui#905) — every
            // `TextDisplay` line wraps rather than being cut off. Same
            // gutter math as `wrap_row_count` (and TUI's `line_rows`) via
            // `content_budget_cols` — see its doc for why that sharing
            // matters (#494).
            let rows = wrap_display_line(line, content_budget_cols(line, col_budget));

            for (row_i, row_spans) in rows.iter().enumerate() {
                let row_y = row0_y + row_i as f32 * line_height;
                // `break` here only exits *this line's* wrapped-row loop,
                // not the outer `for vis in &layout.visible_lines` loop —
                // deliberately, since #905 made a single visible line
                // capable of spanning multiple rows. The old single-row
                // code could `break` the outer loop directly because
                // hitting the bottom on one line meant every later line
                // was also below it; that's no longer true in the
                // abstract (though `TextDisplay::layout`'s own visible-line
                // selection already stops handing back lines that start
                // past the bottom, via the `row0_y` check above, so this
                // rarely does more than confirm there is no next line to
                // skip to).
                if row_y + line_height > body_y + body_h {
                    break;
                }

                let mut cursor_x = rect.x;

                if row_i == 0 {
                    if let Some(ref ts) = line.timestamp {
                        let (tw, _) = surface.surface_measure_text(ts);
                        surface.surface_draw_text_run(
                            Rect::new(cursor_x, row_y, tw.max(1.0), line_height),
                            ts,
                            theme.muted_fg,
                        );
                        cursor_x += tw + 6.0;
                    }
                } else {
                    let (mw, _) = surface.surface_measure_text(WRAP_CONTINUATION_MARKER);
                    surface.surface_draw_text_run(
                        Rect::new(cursor_x, row_y, mw.max(1.0), line_height),
                        WRAP_CONTINUATION_MARKER,
                        theme.muted_fg,
                    );
                    cursor_x += mw;
                }

                for span in row_spans {
                    let span_fg = span.fg.unwrap_or(line_fg);
                    let (sw, _) = surface.surface_measure_text(&span.text);
                    if let Some(span_bg) = span.bg {
                        surface.surface_fill_rect(
                            Rect::new(cursor_x, row_y, sw, line_height),
                            span_bg,
                        );
                    }
                    surface.surface_draw_text_run(
                        Rect::new(cursor_x, row_y, sw.max(1.0), line_height),
                        &span.text,
                        span_fg,
                    );
                    cursor_x += sw;
                }
            }
        }

        // Scrollbar gutter.
        if display.show_scrollbar {
            if let Some(gutter) = layout.scrollbar_bounds {
                surface.surface_fill_rect(
                    Rect::new(
                        rect.x + gutter.x,
                        body_y + gutter.y,
                        gutter.width,
                        gutter.height,
                    ),
                    theme.scrollbar_track,
                );
            }
            if let Some(thumb) = layout.thumb_bounds {
                let inset = 2.0;
                surface.surface_fill_rect(
                    Rect::new(
                        rect.x + thumb.x + inset,
                        body_y + thumb.y,
                        (thumb.width - inset * 2.0).max(2.0),
                        thumb.height,
                    ),
                    theme.scrollbar_thumb,
                );
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::primitives::text_display::TextDisplayLine;
        use crate::types::{Color, StyledSpan, StyledText, WidgetId};
        use crate::Image;

        /// Records every drawing verb `paint` issues — mirrors
        /// `primitives::chart`/`primitives::find_replace`'s own
        /// `RecordingSurface` (#810/#809).
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            text_runs: Vec<(Rect, String, Color)>,
        }

        impl NativeSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(240.0, 160.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                16.0
            }
            fn surface_char_width(&self) -> f32 {
                8.0
            }
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 8.0, 14.0)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.text_runs.push((rect, text.to_string(), color));
            }
            fn surface_draw_line(
                &mut self,
                _from: crate::Point,
                _to: crate::Point,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        fn td_line(text: &str) -> TextDisplayLine {
            TextDisplayLine {
                spans: vec![StyledSpan::plain(text)],
                decoration: Decoration::Normal,
                timestamp: None,
            }
        }

        fn make_td(lines: usize, show_scrollbar: bool) -> TextDisplay {
            TextDisplay {
                id: WidgetId::new("td"),
                lines: (0..lines).map(|i| td_line(&format!("ln{i}"))).collect(),
                scroll_offset: 0,
                auto_scroll: false,
                max_lines: 0,
                has_focus: false,
                title: None,
                show_scrollbar,
            }
        }

        #[test]
        fn background_fills_theme_background() {
            let td = make_td(0, false);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let rect = Rect::new(0.0, 0.0, 240.0, 160.0);

            paint(&td, rect, &mut surface, &theme, 16.0, 8.0);

            assert!(
                surface
                    .fills
                    .iter()
                    .any(|&(r, c)| r == rect && c == theme.background),
                "expected a full-rect background fill, fills were {:?}",
                surface.fills,
            );
        }

        #[test]
        fn zero_size_rect_paints_nothing() {
            let td = make_td(5, false);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(
                &td,
                Rect::new(0.0, 0.0, 0.0, 0.0),
                &mut surface,
                &theme,
                16.0,
                8.0,
            );

            assert!(surface.fills.is_empty());
            assert!(surface.text_runs.is_empty());
        }

        #[test]
        fn title_row_paints_before_the_body_and_shrinks_it() {
            let mut td = make_td(1, false);
            td.title = Some(StyledText::plain("Logs"));
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let rect = Rect::new(0.0, 0.0, 240.0, 32.0);

            paint(&td, rect, &mut surface, &theme, 16.0, 8.0);

            assert!(
                surface
                    .text_runs
                    .iter()
                    .any(|(r, t, _)| t == "Logs" && r.y == 0.0),
                "expected the title span painted at the top row, got {:?}",
                surface.text_runs,
            );
            assert!(
                surface
                    .text_runs
                    .iter()
                    .any(|(r, t, _)| t == "ln0" && r.y == 16.0),
                "expected the body's first line shifted down by one line_height, got {:?}",
                surface.text_runs,
            );
        }

        #[test]
        fn line_decoration_resolves_fallback_color() {
            let mut td = make_td(1, false);
            td.lines[0].decoration = Decoration::Error;
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(
                &td,
                Rect::new(0.0, 0.0, 240.0, 32.0),
                &mut surface,
                &theme,
                16.0,
                8.0,
            );

            assert!(
                surface
                    .text_runs
                    .iter()
                    .any(|(_, t, c)| t == "ln0" && *c == theme.error_fg),
                "expected the error-decorated line painted in theme.error_fg, got {:?}",
                surface.text_runs,
            );
        }

        #[test]
        fn scrollbar_gutter_and_thumb_paint_when_shown() {
            let td = make_td(100, true);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();

            paint(
                &td,
                Rect::new(0.0, 0.0, 240.0, 160.0),
                &mut surface,
                &theme,
                16.0,
                8.0,
            );

            assert!(
                surface
                    .fills
                    .iter()
                    .any(|&(_, c)| c == theme.scrollbar_track),
                "expected a scrollbar-track fill"
            );
            assert!(
                surface
                    .fills
                    .iter()
                    .any(|&(_, c)| c == theme.scrollbar_thumb),
                "expected a scrollbar-thumb fill"
            );
        }

        // ── quadraui#905: wrap paint behaviour ──────────────────────────

        #[test]
        fn wrap_splits_long_line_across_multiple_rows_with_continuation_marker() {
            let mut td = make_td(0, false);
            td.lines.push(TextDisplayLine {
                spans: vec![StyledSpan::plain(
                    "the quick brown fox jumps over the lazy dog",
                )],
                decoration: Decoration::Normal,
                timestamp: None,
            });
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            // RecordingSurface measures 8px/char; an 80px-wide viewport is
            // a 10-cell budget, far narrower than the 44-char line.
            paint(
                &td,
                Rect::new(0.0, 0.0, 80.0, 160.0),
                &mut surface,
                &theme,
                16.0,
                8.0,
            );

            let row_ys: std::collections::BTreeSet<i64> = surface
                .text_runs
                .iter()
                .map(|(r, _, _)| (r.y * 1000.0).round() as i64)
                .collect();
            assert!(
                row_ys.len() > 1,
                "expected the line to wrap onto multiple rows, got runs {:?}",
                surface.text_runs
            );
            assert!(
                surface
                    .text_runs
                    .iter()
                    .any(|(_, t, _)| t == WRAP_CONTINUATION_MARKER),
                "expected the continuation marker painted on a wrapped row, got {:?}",
                surface.text_runs,
            );
        }
    }
}

#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(unused_imports)]
pub(crate) use native_surface_paint::paint;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Color, StyledSpan};

    fn line(text: &str) -> TextDisplayLine {
        TextDisplayLine {
            spans: vec![StyledSpan::plain(text)],
            decoration: Decoration::Normal,
            timestamp: None,
        }
    }

    fn make_td(lines: usize, show_scrollbar: bool, scroll: usize, auto: bool) -> TextDisplay {
        TextDisplay {
            id: WidgetId::new("td"),
            lines: (0..lines).map(|i| line(&format!("line{i}"))).collect(),
            scroll_offset: scroll,
            auto_scroll: auto,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar,
        }
    }

    #[test]
    fn scrollbar_layout_reserves_gutter_width() {
        let td = make_td(20, true, 0, false);
        let layout =
            td.layout_with_scrollbar(40.0, 10.0, 1.0, 1.0, |_| TextDisplayLineMeasure::new(1.0));
        // Body width should be 39 (40 - 1 gutter).
        for vis in &layout.visible_lines {
            assert!(
                (vis.bounds.width - 39.0).abs() < 0.01,
                "line body width should be 39, got {}",
                vis.bounds.width
            );
        }
        assert!(layout.scrollbar_bounds.is_some());
        let gutter = layout.scrollbar_bounds.unwrap();
        assert!((gutter.x - 39.0).abs() < 0.01);
        assert!((gutter.width - 1.0).abs() < 0.01);
    }

    #[test]
    fn scrollbar_thumb_at_top_when_scroll_zero() {
        let td = make_td(20, true, 0, false);
        let layout =
            td.layout_with_scrollbar(40.0, 10.0, 1.0, 1.0, |_| TextDisplayLineMeasure::new(1.0));
        let thumb = layout.thumb_bounds.expect("thumb present");
        assert!(
            thumb.y.abs() < 0.01,
            "thumb should start at top, got y={}",
            thumb.y
        );
    }

    #[test]
    fn scrollbar_thumb_at_bottom_when_fully_scrolled() {
        let td = make_td(20, true, 10, false);
        let layout =
            td.layout_with_scrollbar(40.0, 10.0, 1.0, 1.0, |_| TextDisplayLineMeasure::new(1.0));
        let thumb = layout.thumb_bounds.expect("thumb present");
        assert!(
            (thumb.y + thumb.height - 10.0).abs() < 0.01,
            "thumb should touch bottom: y={}, h={}, viewport=10",
            thumb.y,
            thumb.height
        );
    }

    #[test]
    fn scrollbar_hit_test_regions_present() {
        let td = make_td(20, true, 5, false);
        let layout =
            td.layout_with_scrollbar(40.0, 10.0, 1.0, 1.0, |_| TextDisplayLineMeasure::new(1.0));
        // Scrollbar hit regions should be present (thumb + track before/after).
        let has_thumb = layout
            .hit_regions
            .iter()
            .any(|(_, h)| matches!(h, TextDisplayHit::ScrollbarThumb));
        let has_track_before = layout
            .hit_regions
            .iter()
            .any(|(_, h)| matches!(h, TextDisplayHit::ScrollbarTrackBefore));
        let has_track_after = layout
            .hit_regions
            .iter()
            .any(|(_, h)| matches!(h, TextDisplayHit::ScrollbarTrackAfter));
        assert!(has_thumb, "thumb hit region missing");
        assert!(has_track_before, "track-before hit region missing");
        assert!(has_track_after, "track-after hit region missing");
    }

    #[test]
    fn no_scrollbar_when_content_fits() {
        let td = make_td(5, true, 0, false);
        let layout =
            td.layout_with_scrollbar(40.0, 10.0, 1.0, 1.0, |_| TextDisplayLineMeasure::new(1.0));
        // Content fits in viewport; scrollbar should have no thumb.
        assert!(layout.thumb_bounds.is_none());
    }

    #[test]
    fn no_scrollbar_when_disabled() {
        let td = make_td(20, false, 0, false);
        let layout = td.layout(40.0, 10.0, |_| TextDisplayLineMeasure::new(1.0));
        assert!(layout.scrollbar_bounds.is_none());
        assert!(layout.thumb_bounds.is_none());
        // No scrollbar hit regions.
        let scrollbar_hits = layout.hit_regions.iter().any(|(_, h)| {
            matches!(
                h,
                TextDisplayHit::ScrollbarThumb
                    | TextDisplayHit::ScrollbarTrackBefore
                    | TextDisplayHit::ScrollbarTrackAfter
            )
        });
        assert!(!scrollbar_hits, "no scrollbar hits when disabled");
    }

    #[test]
    fn hit_test_line_in_body_area() {
        let td = make_td(20, true, 0, false);
        let layout =
            td.layout_with_scrollbar(40.0, 10.0, 1.0, 1.0, |_| TextDisplayLineMeasure::new(1.0));
        // Click in the body area (x=5, y=3.5) should hit line 3.
        match layout.hit_test(5.0, 3.5) {
            TextDisplayHit::Line(idx) => assert_eq!(idx, 3),
            other => panic!("expected Line(3), got {:?}", other),
        }
    }

    #[test]
    fn hit_test_scrollbar_thumb() {
        let td = make_td(20, true, 0, false);
        let layout =
            td.layout_with_scrollbar(40.0, 10.0, 1.0, 1.0, |_| TextDisplayLineMeasure::new(1.0));
        let thumb = layout.thumb_bounds.expect("thumb present");
        // Click inside the thumb.
        match layout.hit_test(thumb.x + 0.5, thumb.y + thumb.height / 2.0) {
            TextDisplayHit::ScrollbarThumb => {}
            other => panic!("expected ScrollbarThumb, got {:?}", other),
        }
    }

    #[test]
    fn auto_scroll_with_scrollbar() {
        let td = make_td(20, true, 0, true);
        let layout =
            td.layout_with_scrollbar(40.0, 10.0, 1.0, 1.0, |_| TextDisplayLineMeasure::new(1.0));
        // Auto-scroll should pin to bottom: resolved offset = 10.
        assert_eq!(layout.resolved_scroll_offset, 10);
        assert_eq!(layout.visible_lines.last().unwrap().line_idx, 19);
    }

    #[test]
    fn append_line_and_max_lines() {
        let mut td = TextDisplay::new(WidgetId::new("td"));
        td.set_max_lines(5);
        for i in 0..10 {
            td.append_line(line(&format!("l{i}")));
        }
        assert_eq!(td.lines.len(), 5);
        // Oldest lines should be trimmed; newest 5 remain.
        assert_eq!(td.lines[0].spans[0].text, "l5");
        assert_eq!(td.lines[4].spans[0].text, "l9");
    }

    #[test]
    fn text_display_append_and_cap() {
        let mut td = TextDisplay::new(WidgetId::new("logs"));
        td.set_max_lines(3);

        let mk = |text: &str| TextDisplayLine {
            spans: vec![StyledSpan::plain(text)],
            decoration: Decoration::Normal,
            timestamp: None,
        };

        td.append_line(mk("a"));
        td.append_line(mk("b"));
        td.append_line(mk("c"));
        assert_eq!(td.lines.len(), 3);

        // Fourth append evicts the oldest.
        td.append_line(mk("d"));
        assert_eq!(td.lines.len(), 3);
        assert_eq!(td.lines.first().unwrap().spans[0].text, "b");
        assert_eq!(td.lines.last().unwrap().spans[0].text, "d");

        // Lower the cap → trims oldest.
        td.set_max_lines(2);
        assert_eq!(td.lines.len(), 2);
        assert_eq!(td.lines.first().unwrap().spans[0].text, "c");

        td.clear();
        assert_eq!(td.lines.len(), 0);
        assert_eq!(td.scroll_offset, 0);
    }

    #[test]
    fn text_display_roundtrip_serde() {
        let td = TextDisplay {
            id: WidgetId::new("td"),
            lines: vec![
                TextDisplayLine {
                    spans: vec![StyledSpan::plain("hello")],
                    decoration: Decoration::Normal,
                    timestamp: Some("12:00:00".to_string()),
                },
                TextDisplayLine {
                    spans: vec![
                        StyledSpan::plain("error: "),
                        StyledSpan::with_fg("not found", Color::rgb(255, 80, 80)),
                    ],
                    decoration: Decoration::Error,
                    timestamp: None,
                },
            ],
            scroll_offset: 0,
            auto_scroll: false,
            max_lines: 1000,
            has_focus: true,
            title: None,
            show_scrollbar: false,
        };
        let json = serde_json::to_string(&td).unwrap();
        let back: TextDisplay = serde_json::from_str(&json).unwrap();
        assert_eq!(td, back);
    }

    #[test]
    fn text_display_event_roundtrip_serde() {
        let events = vec![
            TextDisplayEvent::Scrolled { new_offset: 42 },
            TextDisplayEvent::AutoScrollToggled { enabled: false },
            TextDisplayEvent::Copied {
                text: "selected line".to_string(),
            },
            TextDisplayEvent::KeyPressed {
                key: "G".to_string(),
                modifiers: Modifiers::default(),
            },
        ];
        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let back: TextDisplayEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(event, &back);
        }
    }

    // ── D6 TextDisplay layout API tests ───────────────────────────────

    fn make_td_line(text: &str) -> TextDisplayLine {
        TextDisplayLine {
            spans: vec![StyledSpan::plain(text)],
            decoration: Decoration::Normal,
            timestamp: None,
        }
    }

    fn make_td_from_lines(lines: Vec<TextDisplayLine>, scroll: usize, auto: bool) -> TextDisplay {
        TextDisplay {
            id: WidgetId::new("td"),
            lines,
            scroll_offset: scroll,
            auto_scroll: auto,
            max_lines: 0,
            has_focus: true,
            title: None,
            show_scrollbar: false,
        }
    }

    #[test]
    fn text_display_layout_empty() {
        let td = make_td_from_lines(vec![], 0, true);
        let layout = td.layout(40.0, 10.0, |_| TextDisplayLineMeasure::new(1.0));
        assert_eq!(layout.visible_lines.len(), 0);
        assert_eq!(layout.hit_test(5.0, 5.0), TextDisplayHit::Empty);
    }

    #[test]
    fn text_display_layout_manual_scroll() {
        let td = make_td_from_lines(
            (0..10).map(|i| make_td_line(&format!("l{i}"))).collect(),
            3,
            false,
        );
        let layout = td.layout(40.0, 5.0, |_| TextDisplayLineMeasure::new(1.0));
        // scroll_offset honoured verbatim; 5 lines visible from offset 3.
        assert_eq!(layout.resolved_scroll_offset, 3);
        assert_eq!(layout.visible_lines.len(), 5);
        assert_eq!(layout.visible_lines[0].line_idx, 3);
    }

    #[test]
    fn text_display_layout_auto_scroll_pins_bottom() {
        // 10 lines, viewport fits 5 lines, auto_scroll true. Layout
        // should pick offset 5 so lines 5..10 are visible — ignoring
        // whatever scroll_offset was in the primitive.
        let td = make_td_from_lines(
            (0..10).map(|i| make_td_line(&format!("l{i}"))).collect(),
            0, // stored scroll_offset overridden by auto-scroll
            true,
        );
        let layout = td.layout(40.0, 5.0, |_| TextDisplayLineMeasure::new(1.0));
        assert_eq!(layout.resolved_scroll_offset, 5);
        assert_eq!(layout.visible_lines.len(), 5);
        assert_eq!(layout.visible_lines[0].line_idx, 5);
        assert_eq!(layout.visible_lines[4].line_idx, 9);
    }

    #[test]
    fn text_display_layout_auto_scroll_short_stream() {
        // Only 3 lines, viewport fits 5. Auto-scroll pins bottom but
        // there's nothing to scroll past — offset should stay at 0.
        let td = make_td_from_lines(
            (0..3).map(|i| make_td_line(&format!("l{i}"))).collect(),
            0,
            true,
        );
        let layout = td.layout(40.0, 5.0, |_| TextDisplayLineMeasure::new(1.0));
        assert_eq!(layout.resolved_scroll_offset, 0);
        assert_eq!(layout.visible_lines.len(), 3);
    }

    #[test]
    fn text_display_layout_wrap_heights() {
        // Simulate wrap: line 0 wraps to 3 rows, line 1 fits in 1 row,
        // line 2 wraps to 2 rows. Viewport 5 rows. Lines 0 + 1 take
        // rows 0..4; line 2 starts at y=4 and clips to 1 row.
        let td = make_td_from_lines(
            (0..3).map(|i| make_td_line(&format!("l{i}"))).collect(),
            0,
            false,
        );
        let heights = [3.0, 1.0, 2.0];
        let layout = td.layout(40.0, 5.0, |i| TextDisplayLineMeasure::new(heights[i]));
        assert_eq!(layout.visible_lines.len(), 3);
        assert_eq!(layout.visible_lines[0].bounds.height, 3.0);
        assert_eq!(layout.visible_lines[1].bounds.y, 3.0);
        assert_eq!(layout.visible_lines[1].bounds.height, 1.0);
        // Third line clipped to the remaining 1 row of viewport.
        assert_eq!(layout.visible_lines[2].bounds.y, 4.0);
        assert_eq!(layout.visible_lines[2].bounds.height, 1.0);
    }

    // quadraui#905's wrap-helper unit tests live in `wrap::tests`, next to
    // the helpers themselves — both are gated on the rasteriser features
    // (see the `wrap` module doc), so they can't sit in this ungated
    // `mod tests`.
}
