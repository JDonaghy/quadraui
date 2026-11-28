//! `TextDisplay` primitive: a scrollable, append-only viewer for streamed
//! text. Distinct from `Terminal` (which is VT100-aware with cursor and
//! attributes) and from `TextEditor` (full editor with cursor, selection,
//! undo). `TextDisplay` is the right primitive for log tails, command
//! output, debug console, kubectl logs streams.
//!
//! The primitive itself is a `Vec<TextDisplayLine>` plus scroll and
//! auto-scroll state. Backends are expected to render efficiently — for
//! high-volume streams, backends may diff only the appended slice rather
//! than re-rasterising the whole viewport. The primitive's append-only API
//! (`append_line`, no mid-buffer mutation) is what makes that diff cheap.
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
//! affected rows.

use crate::event::Rect;
use crate::types::{Decoration, Modifiers, StyledSpan, StyledText, WidgetId};
use crate::Handled;
use serde::{Deserialize, Serialize};

/// Declarative description of a `TextDisplay`.
///
/// Lines are rendered top-to-bottom in insertion order. `scroll_offset`
/// is the index of the first visible line (0 = top). When `auto_scroll`
/// is true the backend should clamp `scroll_offset` to keep the most
/// recent line visible after each `append_line` — paused only when the
/// user explicitly scrolls upward.
///
/// # Examples
///
/// ```
/// use quadraui::{StyledSpan, TextDisplay, TextDisplayLine, TextDisplayLineMeasure, WidgetId};
///
/// let mut display = TextDisplay::new(WidgetId::new("display:logs"));
/// display.append_line(TextDisplayLine {
///     spans: vec![StyledSpan::plain("server started")],
///     decoration: Default::default(),
///     timestamp: Some("12:00:00".to_string()),
/// });
///
/// let layout = display.layout(80.0, 10.0, |_| TextDisplayLineMeasure::new(1.0));
/// assert_eq!(layout.visible_lines.len(), 1);
/// // auto_scroll defaults to true, so the resolved offset keeps the
/// // newest (only) line visible.
/// assert_eq!(layout.resolved_scroll_offset, 0);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TextDisplay {
    pub id: WidgetId,
    pub lines: Vec<TextDisplayLine>,
    /// Index of the first visible line. `0` = top.
    #[serde(default)]
    pub scroll_offset: usize,
    /// The loaded slice's place in the real collection — see
    /// [`crate::behaviour::row_window`]. `None`: the payload is the whole
    /// collection. When set, `scroll_offset`, the selection and every row
    /// index in this widget's events are absolute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items_window: Option<crate::behaviour::row_window::ItemsWindow>,
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

/// Backend-shared line-wrapping math for [`TextDisplay`].
///
/// Every rasteriser — `tui`, and the shared [`native_surface_paint::paint`]
/// behind `gtk`/`macos`/`win` — computes its wrap decisions here rather
/// than per-backend, so paint and the pure hit-testing layout helpers can
/// never disagree about how many rows a line occupies (see
/// [`wrap::wrap_row_count`]'s doc).
///
/// **Why the whole module is `cfg`-gated to the rasteriser features.**
/// These helpers exist *only* to serve rasterisers. A default-feature
/// `quadraui` build (no `tui`/`gtk`/`win`/`macos`) compiles the primitive
/// types but no backend, so nothing calls them — and `cargo check`
/// with warnings-as-errors turns that into hard `dead_code` errors.
/// Gating the module prevents those errors. The gate must stay a superset
/// of every backend that calls into here: adding a new rasteriser feature
/// means adding it to this list.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
mod wrap {
    use super::{StyledSpan, TextDisplayLine};

    /// Glyph prefixed to every continuation row of a wrapped
    /// [`super::TextDisplay`] line. Without it, a wrapped
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
    /// locally. A single function ensures all callers use the same formula.
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
    /// `TextDisplay` always wraps over-long lines — this is
    /// not a caller-toggleable option (see the module doc: the primitive is
    /// documented exclusively for prose/log content, which has no fixed-
    /// column use case; that's what `DataTable` is for). Shared by every
    /// backend's `measure_line` closure (paint) *and* its pure layout
    /// counterpart (hit-testing), so the two can never disagree about row
    /// counts.
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
    /// backends (GTK/macOS/Windows) use this directly, and
    /// [`super::pixel_layout`] uses it through the same path, so their
    /// wrap column budget is computed identically whether or not a live
    /// rendering surface is on hand, and a degenerate `char_width` (`<=
    /// 0.0`) resolves to a `0` column budget everywhere rather than a
    /// locally re-derived division — see [`wrap_row_count`]'s doc on why
    /// paint and layout must agree.
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
        fn px_to_cols_floors_and_handles_degenerate_input() {
            assert_eq!(px_to_cols(100.0, 8.0), 12);
            assert_eq!(px_to_cols(100.0, 0.0), 0);
            assert_eq!(px_to_cols(0.0, 8.0), 0);
        }
    }
}

// Re-exported at the old `primitives::text_display::*` paths so every
// backend call site keeps its existing import.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub(crate) use wrap::{
    content_budget_cols, px_to_cols, wrap_display_line, wrap_row_count, WRAP_CONTINUATION_MARKER,
};

/// One line in a `TextDisplay`. Carries styled spans plus an optional
/// decoration tag (Error/Warning/Muted/Header) for log-level styling and
/// an optional left-aligned timestamp string the backend renders in a
/// dim colour.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
    /// display line is one text row. Wrap-enabled
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
    /// Re-size the scrollbar thumb (and its track hit regions) for a
    /// collection of `total` lines scrolled to line `offset` — a windowed
    /// display's real extent, which its band layout cannot see.
    pub fn set_scroll_extent(&mut self, total: usize, offset: usize, min_thumb: f32) {
        let Some(gutter) = self.scrollbar_bounds else {
            return;
        };
        self.hit_regions.retain(|(_, hit)| {
            !matches!(
                hit,
                TextDisplayHit::ScrollbarThumb
                    | TextDisplayHit::ScrollbarTrackBefore
                    | TextDisplayHit::ScrollbarTrackAfter
            )
        });
        let (thumb_start, thumb_len) = crate::primitives::scrollbar::fit_thumb(
            offset as f32,
            total as f32,
            self.visible_lines.len() as f32,
            gutter.height,
            min_thumb,
        );
        if thumb_len <= 0.0 {
            self.thumb_bounds = None;
            return;
        }
        let x = gutter.x;
        let w = gutter.width;
        let thumb = Rect::new(x, gutter.y + thumb_start, w, thumb_len);
        let before = Rect::new(x, gutter.y, w, thumb_start);
        let after = Rect::new(
            x,
            thumb.y + thumb_len,
            w,
            (gutter.height - thumb_start - thumb_len).max(0.0),
        );
        let mut regions = vec![(thumb, TextDisplayHit::ScrollbarThumb)];
        if before.height > 0.0 {
            regions.push((before, TextDisplayHit::ScrollbarTrackBefore));
        }
        if after.height > 0.0 {
            regions.push((after, TextDisplayHit::ScrollbarTrackAfter));
        }
        regions.append(&mut self.hit_regions);
        self.hit_regions = regions;
        self.thumb_bounds = Some(thumb);
    }

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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
            items_window: None,
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

// ── PaintSurface paint (unified pixel backend implementation) ────
//
// Before this, `gtk::text_display::draw_text_display`,
// `macos::text_display::draw_text_display` and
// `win::text_display::draw_text_display` each independently painted the
// optional title row, per-line spans/timestamp, and scrollbar gutter +
// thumb with their own Cairo / CoreGraphics / Direct2D calls — the three
// were already near-identical. `paint` below is the one shared
// implementation, written against
// [`crate::paint_surface::PaintSurface`].
//
// Each backend keeps its own `*_text_display_layout` free function
// (`gtk_text_display_layout`/`mac_text_display_layout`/
// `win_text_display_layout`) — unlike `paint`, that helper is pure
// `TextDisplay::layout`/`layout_with_scrollbar` math with zero
// Cairo/CoreGraphics/Direct2D dependency, so it remains backend-agnostic
// but is left as three near-identical copies to keep their scope bounded.
//
// **Behavioural difference:** Bold span text is not painted specially on
// any pixel backend. The `PaintSurface` trait carries no bold parameter,
// so `StyledSpan::bold` cannot be preserved through this trait.
// TUI's own `tui::text_display` is untouched.
//
// `#[allow(dead_code)]`: see `primitives::form`'s identical note
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
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;
    use crate::types::Decoration;
    use crate::Rect;

    /// Paint a [`TextDisplay`] into `rect` on `surface`.
    ///
    /// Fills `rect` with [`Theme::background`], paints an optional title
    /// row (shrinking the body by one `line_height`), then each visible
    /// line's optional timestamp + spans (span `bg` filled before its
    /// text, per-line `Decoration` resolving a fallback `fg`), then an
    /// optional scrollbar gutter + thumb at the trailing edge.
    ///
    /// `line_height` and the resulting layout math are identical to
    /// what the per-backend `*_text_display_layout` functions compute.
    ///
    /// `char_width` is the backend's approximate average character width
    /// (`Backend::char_width`), used to convert `rect`'s pixel width into
    /// the same "display cells" unit the wrap decision budgets in —
    /// see [`px_to_cols`]'s doc for why an approximation (rather than a
    /// real per-glyph measurement, which `surface` could give here) is
    /// the correct choice: the pure `*_text_display_layout` hit-testing
    /// helpers have no live surface to measure with, so paint must use
    /// the same approximation they do or a click could resolve to a
    /// different row than what's on screen.
    pub(crate) fn paint(
        display: &TextDisplay,
        rect: Rect,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        line_height: f32,
        char_width: f32,
    ) {
        paint_with_thumb(display, rect, surface, theme, line_height, char_width, None);
    }

    /// [`paint`], with the scrollbar thumb taken from `thumb` when it is
    /// `Some` — a windowed display's thumb, sized from the real extent
    /// rather than the band `display` holds.
    pub(crate) fn paint_with_thumb(
        display: &TextDisplay,
        rect: Rect,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        line_height: f32,
        char_width: f32,
        thumb: Option<Option<Rect>>,
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

            // Word-wrap onto continuation rows — every
            // `TextDisplay` line wraps rather than being cut off. Same
            // gutter math as `wrap_row_count` (and TUI's `line_rows`) via
            // `content_budget_cols` — see its doc for why that sharing
            // matters.
            let rows = wrap_display_line(line, content_budget_cols(line, col_budget));

            for (row_i, row_spans) in rows.iter().enumerate() {
                let row_y = row0_y + row_i as f32 * line_height;
                // `break` here only exits *this line's* wrapped-row loop,
                // not the outer `for vis in &layout.visible_lines` loop.
                // A single visible line can span multiple rows, so a line
                // hitting the bottom does not mean every later line is also
                // below it. `TextDisplay::layout`'s visible-line selection
                // already stops handing back lines that start past the
                // bottom via the `row0_y` check above.
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
            if let Some(thumb) = thumb.unwrap_or(layout.thumb_bounds) {
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
        /// `RecordingSurface`.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            text_runs: Vec<(Rect, String, Color)>,
        }

        impl PaintSurface for RecordingSurface {
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
            /// Test-only recorder — `paint` never calls this verb
            /// (see this module's own doc for why no primitive here has been
            /// migrated onto it yet); this exists only so `RecordingSurface`
            /// satisfies the trait. Records into the same `fills` list as
            /// `surface_fill_rect` (radius dropped) — no test asserts on it
            /// today.
            fn surface_fill_rounded_rect(&mut self, rect: Rect, _radius: f32, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_stroke_rounded_rect(
                &mut self,
                _rect: Rect,
                _radius: f32,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
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
                items_window: None,
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

        // wrap paint behaviour

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

// ── Layout + cell paint for the widget contract ────────────────────────

/// Scrollbar gutter width on a pixel surface, in surface-native units —
/// the 12-unit gutter every pixel backend's own
/// `*_text_display_layout` reserves.
const PIXEL_GUTTER: f32 = 12.0;

/// Minimum scrollbar thumb length on a pixel surface.
const PIXEL_MIN_THUMB: f32 = 8.0;

/// How many rows line `line` occupies at a `cols` column budget.
///
/// A rasteriser feature compiled in gets [`wrap_row_count`]'s real
/// wrapped row count. A build with no rasteriser at all has no `wrap`
/// module (see that module's own gate) and also nothing that could paint
/// a wrapped row, so it counts one row per line — the layout is still
/// resolvable, just never painted. Keeping the fork inside this one
/// helper is what lets [`crate::primitive::Primitive::layout`] stay a
/// single shared path across every feature set.
fn layout_row_count(line: &TextDisplayLine, cols: usize) -> usize {
    #[cfg(any(
        feature = "tui",
        feature = "gtk",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    {
        wrap_row_count(line, cols)
    }
    #[cfg(not(any(
        feature = "tui",
        feature = "gtk",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    )))]
    {
        let _ = (line, cols);
        1
    }
}

/// Resolve this display's layout in **whole cell** units (one row per
/// line, a 1-cell scrollbar gutter) — the one cell-grid entry point
/// [`crate::primitive::Primitive::layout`] and
/// [`crate::tui::tui_text_display_layout`] share, so paint and hit-test
/// agree on every row.
pub fn cell_layout(display: &TextDisplay, area: Rect) -> TextDisplayPrimitiveLayout {
    let body = body_rect(display, area, 1.0);
    if body.height < 1.0 {
        return TextDisplayPrimitiveLayout {
            area,
            body,
            layout: display.layout(0.0, 0.0, |_| TextDisplayLineMeasure::new(1.0)),
            window: None,
        };
    }
    let line_end = if display.show_scrollbar {
        (body.width - 1.0).max(0.0)
    } else {
        body.width
    };
    let col_budget = line_end as usize;
    let measure = |i: usize| {
        TextDisplayLineMeasure::new(layout_row_count(&display.lines[i], col_budget) as f32)
    };
    let layout = if display.show_scrollbar {
        display.layout_with_scrollbar(body.width, body.height, 1.0, 1.0, measure)
    } else {
        display.layout(body.width, body.height, measure)
    };
    TextDisplayPrimitiveLayout {
        area,
        body,
        layout,
        window: None,
    }
}

/// Resolve this display's layout in **pixel** units, with the calling
/// backend's own line height and character advance — the shared twin of
/// [`cell_layout`], and the same math every pixel backend's own
/// `*_text_display_layout` helper performs.
pub fn pixel_layout(
    display: &TextDisplay,
    area: Rect,
    line_height: f32,
    char_width: f32,
) -> TextDisplayPrimitiveLayout {
    let row_h = line_height.max(1.0);
    let body = body_rect(display, area, row_h);
    if body.height <= 0.0 {
        return TextDisplayPrimitiveLayout {
            area,
            body,
            layout: display.layout(0.0, 0.0, |_| TextDisplayLineMeasure::new(row_h)),
            window: None,
        };
    }
    let body_width = if display.show_scrollbar {
        (area.width - PIXEL_GUTTER).max(0.0)
    } else {
        area.width
    };
    let col_budget = pixel_col_budget(body_width, char_width);
    let measure = |i: usize| {
        TextDisplayLineMeasure::new(layout_row_count(&display.lines[i], col_budget) as f32 * row_h)
    };
    let layout = if display.show_scrollbar {
        display.layout_with_scrollbar(
            area.width,
            body.height,
            PIXEL_GUTTER,
            PIXEL_MIN_THUMB,
            measure,
        )
    } else {
        display.layout(area.width, body.height, measure)
    };
    TextDisplayPrimitiveLayout {
        area,
        body,
        layout,
        window: None,
    }
}

/// The display-cell column budget [`pixel_layout`] wraps against for a
/// `body_width_px`-wide body at `char_width`.
///
/// A rasteriser feature compiled in gets the real [`wrap::px_to_cols`]
/// (same mirror as [`layout_row_count`]'s own fork), so every pixel
/// backend's wrap decision and [`pixel_layout`]'s hit-testing agree on a
/// degenerate `char_width` the same way they agree on a normal one. A
/// build with no rasteriser at all has no [`wrap`] module to call into
/// and also no caller that ever observes the result, so it keeps the
/// plain floored division as a self-contained fallback.
fn pixel_col_budget(body_width_px: f32, char_width: f32) -> usize {
    #[cfg(any(
        feature = "tui",
        feature = "gtk",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    {
        px_to_cols(body_width_px, char_width)
    }
    #[cfg(not(any(
        feature = "tui",
        feature = "gtk",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    )))]
    {
        (body_width_px / char_width.max(1.0)).floor().max(0.0) as usize
    }
}

/// The body strip inside `area`: the whole of it, or everything below
/// the one-`row_h` title row when [`TextDisplay::title`] is set.
fn body_rect(display: &TextDisplay, area: Rect, row_h: f32) -> Rect {
    if display.title.is_some() {
        Rect::new(
            area.x,
            area.y + row_h,
            area.width,
            (area.height - row_h).max(0.0),
        )
    } else {
        area
    }
}

/// `TextDisplay`'s cell-grid rasteriser, written against
/// [`crate::CellSurface`]'s verbs — the cell-grid twin of
/// [`native_surface_paint::paint`].
///
/// # Visual contract
///
/// - Background: filled with [`crate::Theme::background`].
/// - An optional title row at the top; the body shrinks by one row when
///   present.
/// - Per-line decoration → default fg: `Error → error_fg`,
///   `Warning → warning_fg`, `Muted → muted_fg`, others →
///   [`crate::Theme::foreground`]; per-span `fg`/`bg` win over it.
/// - Timestamp prefix (when present) in [`crate::Theme::muted_fg`] on a
///   line's first row only; a wrapped continuation row carries
///   [`WRAP_CONTINUATION_MARKER`] instead, so it never reads as an
///   unrelated new line.
/// - A scrollbar gutter (`░`) plus thumb (`█`) in the trailing column
///   when [`TextDisplay::show_scrollbar`].
///
/// One terminal cell is exactly one [`crate::text_util::display_width`]
/// column, so the column-budget wrap decision is exact here, unlike the
/// pixel backends' `char_width` approximation.
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::{
        content_budget_cols, wrap_display_line, TextDisplay, TextDisplayLine,
        TextDisplayPrimitiveLayout, WRAP_CONTINUATION_MARKER,
    };
    use crate::cell_surface::{CellStyle, CellSurface};
    use crate::theme::Theme;
    use crate::types::{Decoration, StyledSpan};

    /// Word-wrap one [`TextDisplayLine`] to `line_end` columns, reserving
    /// gutter width for its timestamp prefix / the continuation marker
    /// via [`content_budget_cols`] — the single shared formula also used
    /// by [`super::wrap_row_count`] and the pixel backends' paint path,
    /// so none of the three can drift out of step.
    fn line_rows(line: &TextDisplayLine, line_end: u16) -> Vec<Vec<StyledSpan>> {
        wrap_display_line(line, content_budget_cols(line, line_end as usize))
    }

    /// Paint `display` at the already-resolved `resolved` layout onto
    /// `surface`.
    pub(crate) fn paint(
        display: &TextDisplay,
        resolved: &TextDisplayPrimitiveLayout,
        surface: &mut dyn CellSurface,
        theme: &Theme,
    ) {
        let area = resolved.area;
        if area.width < 1.0 || area.height < 1.0 {
            return;
        }
        let plain = CellStyle {
            fg: Some(theme.foreground),
            bg: Some(theme.background),
            ..CellStyle::default()
        };
        let muted = CellStyle {
            fg: Some(theme.muted_fg),
            ..plain
        };
        let bg = theme.background;

        surface.fill(area, plain);

        // Optional title row at the top.
        if let Some(title) = &display.title {
            let row_y = area.y as u16;
            let mut col = 0u16;
            for span in &title.spans {
                let style = CellStyle {
                    fg: Some(span.fg.unwrap_or(theme.foreground)),
                    bg: Some(span.bg.unwrap_or(bg)),
                    ..CellStyle::default()
                };
                for ch in span.text.chars() {
                    if col >= area.width as u16 {
                        break;
                    }
                    surface.put_char(area.x as u16 + col, row_y, ch, style);
                    col += 1;
                }
            }
        }

        let body = resolved.body;
        if body.height < 1.0 {
            return;
        }
        let body_x = body.x as u16;
        let body_y = body.y as u16;
        let body_h = body.height as u16;
        let line_end = if display.show_scrollbar {
            (body.width as u16).saturating_sub(1)
        } else {
            body.width as u16
        };

        for vis in &resolved.layout.visible_lines {
            let line = &display.lines[vis.line_idx];
            let row0_y = body_y + vis.bounds.y.round() as u16;
            if row0_y >= body_y + body_h {
                break;
            }

            let line_fg = match line.decoration {
                Decoration::Error => theme.error_fg,
                Decoration::Warning => theme.warning_fg,
                Decoration::Muted => theme.muted_fg,
                _ => theme.foreground,
            };

            for (row_i, row_spans) in line_rows(line, line_end).iter().enumerate() {
                let row_y = row0_y + row_i as u16;
                if row_y >= body_y + body_h {
                    break;
                }

                let mut col: u16 = 0;

                if row_i == 0 {
                    // Timestamp prefix (if present) — first row only.
                    if let Some(ts) = &line.timestamp {
                        for ch in ts.chars() {
                            if col >= line_end {
                                break;
                            }
                            surface.put_char(body_x + col, row_y, ch, muted);
                            col += 1;
                        }
                        if col < line_end {
                            surface.put_char(body_x + col, row_y, ' ', muted);
                            col += 1;
                        }
                    }
                } else {
                    // Continuation-row marker — a wrapped row must not
                    // read as an unrelated new line.
                    for ch in WRAP_CONTINUATION_MARKER.chars() {
                        if col >= line_end {
                            break;
                        }
                        surface.put_char(body_x + col, row_y, ch, muted);
                        col += 1;
                    }
                }

                for span in row_spans {
                    let style = CellStyle {
                        fg: Some(span.fg.unwrap_or(line_fg)),
                        bg: Some(span.bg.unwrap_or(bg)),
                        ..CellStyle::default()
                    };
                    for ch in span.text.chars() {
                        if col >= line_end {
                            break;
                        }
                        surface.put_char(body_x + col, row_y, ch, style);
                        col += 1;
                    }
                }
            }
        }

        // Scrollbar gutter.
        if display.show_scrollbar {
            let gutter_x = body_x + body.width as u16 - 1;
            let track = CellStyle {
                fg: Some(theme.scrollbar_track),
                bg: Some(bg),
                ..CellStyle::default()
            };
            for dy in 0..body_h {
                surface.put_char(gutter_x, body_y + dy, '░', track);
            }
            if let Some(thumb) = resolved.layout.thumb_bounds {
                let thumb_y = body_y + thumb.y.round() as u16;
                let thumb_h = thumb.height.round() as u16;
                let thumb_style = CellStyle {
                    fg: Some(theme.scrollbar_thumb),
                    bg: Some(bg),
                    ..CellStyle::default()
                };
                for dy in 0..thumb_h {
                    let y = thumb_y + dy;
                    if y < body_y + body_h {
                        surface.put_char(gutter_x, y, '█', thumb_style);
                    }
                }
            }
        }
    }
}

// ── Widget contract ────────────────────────────────────────────────────

/// [`crate::primitive::Primitive::Layout`] for [`TextDisplay`]: the
/// absolute area the display was laid out into, the body strip inside it
/// (the area minus a title row), and the resolved
/// [`TextDisplayLayout`].
///
/// [`TextDisplayLayout`]'s own coordinates are **body-local** (`y = 0` at
/// the top of the body), which is why the body strip is carried
/// alongside: the paint path and [`Self::hit_test`] both need the origin
/// those coordinates are relative to.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct TextDisplayPrimitiveLayout {
    /// The absolute rect the display was laid out into.
    pub area: Rect,
    /// The body strip inside [`Self::area`], below any title row.
    pub body: Rect,
    /// The resolved display layout, in body-local coordinates. For a
    /// windowed display its line indices are band-relative — see
    /// [`Self::window`].
    pub layout: TextDisplayLayout,
    /// The band a windowed display ([`TextDisplay::items_window`]) was
    /// laid out through, `None` for an unwindowed one. [`Self::layout`]'s
    /// line indices are relative to its band; [`Self::abs_line`] maps them
    /// back.
    pub window: Option<Box<crate::behaviour::row_window::WindowedLayout<TextDisplay>>>,
}

impl TextDisplayPrimitiveLayout {
    /// Classify an **absolute** point against this display, translating
    /// it into the body-local frame [`TextDisplayLayout::hit_test`]
    /// answers in.
    pub fn hit_test(&self, x: f32, y: f32) -> TextDisplayHit {
        self.layout.hit_test(x - self.body.x, y - self.body.y)
    }

    /// How many whole lines one page of scrolling moves — the visible
    /// line count, floored at one so a viewport too short for a single
    /// line still scrolls.
    pub fn page(&self) -> usize {
        match &self.window {
            Some(w) => w.viewport_rows.max(1),
            None => self.layout.visible_lines.len().max(1),
        }
    }

    /// The absolute line index of [`Self::layout`]'s line `idx`.
    pub fn abs_line(&self, idx: usize) -> usize {
        match &self.window {
            Some(w) => w.to_abs(idx),
            None => idx,
        }
    }

    /// The absolute scroll offset on screen.
    pub fn resolved_offset(&self) -> usize {
        self.abs_line(self.layout.resolved_scroll_offset)
    }
}

impl crate::primitive::Primitive for TextDisplay {
    type Layout = TextDisplayPrimitiveLayout;
    type Event = TextDisplayHit;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::TextDisplay(self.clone())
    }

    /// A text display shows the user's own content (a log, a console
    /// buffer), so it is one of [`crate::EditorClassPrimitive::ALL`] and
    /// measures/paints in the editor font rather than the trait's
    /// chrome-by-default.
    fn font_role(&self) -> crate::FontRole {
        crate::FontRole::Editor
    }

    /// One row per buffer line (unwrapped) plus a title row when one is
    /// set — what a container sizing this display to its content would
    /// give it.
    fn measure(
        &self,
        ctx: &crate::primitive::LayoutCtx,
        avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        let row_h = ctx.line_height.max(1.0);
        let rows = self.lines.len() as f32 + f32::from(u8::from(self.title.is_some()));
        crate::NaturalSize::new(None, Some((rows * row_h).min(avail.height)))
    }

    /// `rect` is the display's own area, title row included. On a cell
    /// grid the layout comes from [`cell_layout`]; on a pixel surface
    /// from [`pixel_layout`] with the backend's own character advance and
    /// — because this is an editor-class primitive — the *editor* row
    /// pitch ([`crate::primitive::LayoutCtx::editor_line_height`]), the
    /// same one the editor-font surface this paints through reports. Row
    /// hit-testing and the painted rows therefore stay on one grid even
    /// when the chrome and editor fonts have different heights.
    ///
    /// The display resolves through its own state first ([`Self::live`]),
    /// and a windowed one ([`TextDisplay::items_window`]) lays out only the
    /// band its viewport shows — see [`crate::behaviour::row_window`].
    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> TextDisplayPrimitiveLayout {
        let scroll = crate::widget_state::with_widget_state_ref(Some(&self.id), |state| {
            state
                .and_then(|s| s.downcast_ref::<TextDisplayState>())
                .map(|s| s.scroll.get(&self.scroll_offset))
        });
        let live = self.live_at(scroll);
        let base = |td: &TextDisplay| match ctx.units() {
            crate::primitive::Units::Cells => {
                cell_layout(td, crate::primitive::cell_grid_rect(rect))
            }
            _ => pixel_layout(td, rect, ctx.editor_line_height(), ctx.char_width),
        };
        let Some(window) = live.items_window else {
            return base(&live);
        };
        use crate::behaviour::row_window;
        let (row_h, min_thumb) = match ctx.units() {
            crate::primitive::Units::Cells => (1.0, 1.0),
            _ => (ctx.editor_line_height().max(1.0), PIXEL_MIN_THUMB),
        };
        let body_h = body_rect(&live, rect, row_h).height;
        let band_rows = row_window::band_rows_for(body_h, row_h);
        let loaded = live.lines.len();
        let total = window.row_count(loaded);
        let viewport_rows = (body_h / row_h).floor().max(0.0) as usize;
        // Auto-scroll anchors the band at the tail, so the display's own
        // "last line visible" resolution lands on the real last line.
        let band_start = if live.auto_scroll {
            total.saturating_sub(band_rows.max(1))
        } else {
            live.scroll_offset
        };
        let band = row_window::band(window, loaded, band_start, band_rows);
        let band_td = TextDisplay {
            lines: row_window::materialise(&live.lines, window, band, |_| TextDisplayLine {
                spans: Vec::new(),
                decoration: Decoration::default(),
                timestamp: None,
            }),
            scroll_offset: 0,
            items_window: None,
            ..live.as_ref().clone()
        };
        let mut layout = base(&band_td);
        let offset = band.absolute(layout.layout.resolved_scroll_offset);
        layout
            .layout
            .set_scroll_extent(total, offset, min_thumb);
        layout.window = Some(Box::new(row_window::WindowedLayout {
            band,
            total,
            loaded: (window.start, loaded),
            scroll: offset,
            viewport_rows,
            scroll_px: None,
            live: Box::new(band_td),
        }));
        layout
    }

    /// The display's body text, minus the scrollbar gutter, as a
    /// selectable region, so drag-select and copy work against it. Its
    /// `lines` stay empty: a wrapped line occupies a different number of
    /// rows under each backend's wrap width, so no backend-portable
    /// flattening of `lines` matches what every backend painted — a TUI
    /// copy reads the painted cells instead, and a pixel backend copies
    /// nothing rather than the wrong text.
    fn text_region(&self, layout: &TextDisplayPrimitiveLayout) -> Option<crate::dispatch::TextRegion> {
        let gutter_width = layout
            .layout
            .scrollbar_bounds
            .map(|g| g.width)
            .unwrap_or(0.0);
        Some(crate::dispatch::TextRegion {
            id: self.id,
            bounds: Rect::new(
                layout.body.x,
                layout.body.y,
                (layout.body.width - gutter_width).max(0.0),
                layout.body.height,
            ),
            lines: Vec::new(),
        })
    }

    /// The shared pixel painter — the same one every pixel backend's
    /// `draw_text_display` routes through.
    fn paint(
        &self,
        layout: &TextDisplayPrimitiveLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            let line_height = surface.surface_line_height();
            let char_width = surface.surface_char_width();
            let state = env.widget_state_as::<TextDisplayState>();
            let live = self.live(state);
            let (painted, thumb) = match &layout.window {
                Some(w) => (w.live.as_ref(), Some(layout.layout.thumb_bounds)),
                None => (live.as_ref(), None),
            };
            native_surface_paint::paint_with_thumb(
                painted,
                layout.area,
                surface,
                theme,
                line_height,
                char_width,
                thumb,
            );
        }
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        {
            let _ = (layout, surface, theme);
        }
    }

    /// [`cell_paint::paint`] over `buf`, reached through
    /// [`crate::tui::draw_text_display`] because the `Buffer` →
    /// [`crate::CellSurface`] adapter is private to `crate::tui`. That
    /// entry point re-resolves the layout from the same area through the
    /// same [`cell_layout`] this `Primitive` laid out with, so the glyphs
    /// land exactly where `layout` says they do.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &TextDisplayPrimitiveLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        let live = self.live(env.widget_state_as::<TextDisplayState>());
        let painted = match &layout.window {
            Some(w) => w.live.as_ref(),
            None => live.as_ref(),
        };
        cell_paint::paint(painted, layout, cells, theme);
    }
}

/// [`crate::interactive::Interactive::State`] for [`TextDisplay`]:
/// whether a scrollbar-thumb drag is in progress, and the scroll offset
/// the display resolves to when the app does not echo
/// [`TextDisplayEvent::Scrolled`] back into [`TextDisplay::scroll_offset`].
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextDisplayState {
    /// `true` between a press on the scrollbar thumb and its release.
    pub dragging_thumb: bool,
    /// The scroll offset the last scroll landed on, against the
    /// descriptor's [`TextDisplay::scroll_offset`] — see
    /// [`crate::behaviour::row_window::HostLatch`].
    pub scroll: crate::behaviour::row_window::HostLatch<usize>,
    /// Repeat suppression for a windowed display's
    /// [`crate::UiEvent::RowsRequested`].
    pub rows_requested: crate::behaviour::row_window::RowsRequestLatch,
}

impl crate::interactive::WidgetAction for TextDisplayEvent {
    /// Every outcome rides [`crate::UiEvent::TextDisplay`] under this
    /// display's own id — the channel a host already routes a
    /// `Backend::draw_text_display` consumer's scroll and copy through.
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        crate::UiEvent::TextDisplay(id, self)
    }
}

impl TextDisplay {
    /// How many lines the real collection has: the window's total when
    /// [`Self::items_window`] is set, else [`Self::lines`]' length.
    pub fn row_count(&self) -> usize {
        match self.items_window {
            Some(w) => w.row_count(self.lines.len()),
            None => self.lines.len(),
        }
    }

    /// This display with its state's latched scroll offset applied — the
    /// descriptor it lays out and paints as.
    pub fn live(&self, state: Option<&TextDisplayState>) -> std::borrow::Cow<'_, Self> {
        self.live_at(state.map(|s| s.scroll.get(&self.scroll_offset)))
    }

    fn live_at(&self, scroll: Option<usize>) -> std::borrow::Cow<'_, Self> {
        match scroll {
            Some(scroll) if scroll != self.scroll_offset => std::borrow::Cow::Owned(Self {
                scroll_offset: scroll,
                ..self.clone()
            }),
            _ => std::borrow::Cow::Borrowed(self),
        }
    }

    /// The scroll offset currently on screen — what the layout resolved,
    /// which auto-scroll may have moved past the descriptor's own
    /// [`Self::scroll_offset`].
    fn resolved_offset(&self, layout: &TextDisplayPrimitiveLayout) -> usize {
        layout.resolved_offset()
    }

    /// The largest offset that still shows content — one page back from
    /// the end of the buffer.
    fn max_offset(&self, layout: &TextDisplayPrimitiveLayout) -> usize {
        self.row_count().saturating_sub(layout.page())
    }

    /// Report a scroll to `target`, clamped, as
    /// [`TextDisplayEvent::Scrolled`] — or [`Handled::Consumed`] when it
    /// wouldn't move — and latch it as this display's own offset.
    fn scroll_to(
        &self,
        state: &mut TextDisplayState,
        layout: &TextDisplayPrimitiveLayout,
        target: usize,
    ) -> Handled<TextDisplayEvent> {
        let new_offset = target.min(self.max_offset(layout));
        if new_offset == self.resolved_offset(layout) {
            Handled::Consumed
        } else {
            state.scroll.set(&self.scroll_offset, new_offset);
            Handled::Event(TextDisplayEvent::Scrolled { new_offset })
        }
    }

    /// Scroll `delta` lines from the offset currently on screen.
    fn scroll_by(
        &self,
        state: &mut TextDisplayState,
        layout: &TextDisplayPrimitiveLayout,
        delta: isize,
    ) -> Handled<TextDisplayEvent> {
        let current = self.resolved_offset(layout);
        let target = if delta < 0 {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            current.saturating_add(delta as usize)
        };
        self.scroll_to(state, layout, target)
    }

    /// The offset a drag or track click at absolute `y` asks for: the
    /// fraction of the track the point sits at, mapped onto the
    /// scrollable range.
    fn offset_for_track_y(&self, layout: &TextDisplayPrimitiveLayout, y: f32) -> usize {
        let track = match layout.layout.scrollbar_bounds {
            Some(t) if t.height > 0.0 => t,
            _ => return self.resolved_offset(layout),
        };
        let local = (y - layout.body.y - track.y).clamp(0.0, track.height);
        let fraction = local / track.height;
        let max = self.max_offset(layout);
        (fraction * max as f32).round().max(0.0) as usize
    }
}

impl crate::interactive::Interactive for TextDisplay {
    type State = TextDisplayState;
    type Action = TextDisplayEvent;

    /// One focus stop as a whole: a read-only log pane takes focus so
    /// the keyboard can scroll it, but has no focusable sub-parts.
    /// [`crate::interactive::FocusSpec::None`] for an empty buffer —
    /// there is nothing to scroll.
    fn focus(&self) -> crate::interactive::FocusSpec {
        if self.row_count() == 0 {
            crate::interactive::FocusSpec::None
        } else {
            crate::interactive::FocusSpec::Stop
        }
    }

    /// The scrollbar gutter, then each visible line — most specific
    /// first, translated out of the layout's body-local frame into the
    /// widget-local space this method answers in.
    fn sub_regions(
        &self,
        layout: &TextDisplayPrimitiveLayout,
    ) -> Vec<(Rect, crate::interactive::SubTarget)> {
        let (ox, oy) = (layout.area.x, layout.area.y);
        let local = |r: Rect| {
            Rect::new(
                layout.body.x + r.x - ox,
                layout.body.y + r.y - oy,
                r.width,
                r.height,
            )
        };
        let mut out = Vec::new();
        if let Some(gutter) = layout.layout.scrollbar_bounds {
            out.push((local(gutter), crate::interactive::SubTarget(SUB_SCROLLBAR)));
        }
        out.extend(layout.layout.visible_lines.iter().map(|vis| {
            (
                local(vis.bounds),
                crate::interactive::SubTarget(SUB_LINE_BASE + layout.abs_line(vis.line_idx) as u64),
            )
        }));
        out
    }

    /// A screen reader can focus the pane and scroll it a step either
    /// way.
    fn a11y_actions(&self, _state: &TextDisplayState) -> Vec<crate::a11y::A11yAction> {
        if self.row_count() == 0 {
            Vec::new()
        } else {
            vec![
                crate::a11y::A11yAction::Focus,
                crate::a11y::A11yAction::ScrollUp,
                crate::a11y::A11yAction::ScrollDown,
            ]
        }
    }

    /// Read-only and scrollable: the arrow keys move one line, PageUp/
    /// PageDown one viewport, Home/End the ends (through
    /// [`crate::behaviour::scroll::ensure_visible`], so this pane and
    /// every other scroller agree on where an edge lands), the wheel
    /// scrolls, a click on the scrollbar track pages toward it, and a
    /// drag on the thumb scrubs. Every other key is handed back to the
    /// app as [`TextDisplayEvent::KeyPressed`].
    fn handle(
        &self,
        state: &mut TextDisplayState,
        event: &crate::interactive::RoutedEvent,
        layout: &TextDisplayPrimitiveLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> Handled<TextDisplayEvent> {
        use crate::behaviour::scroll::ensure_visible;
        use crate::event::{Key, NamedKey};
        use crate::interactive::{PointerKind, RoutedEvent};

        let page = layout.page();
        match event {
            RoutedEvent::Key(key) => match key.key {
                Key::Named(NamedKey::Up) => self.scroll_by(state, layout, -1),
                Key::Named(NamedKey::Down) => self.scroll_by(state, layout, 1),
                Key::Named(NamedKey::PageUp) => self.scroll_by(state, layout, -(page as isize)),
                Key::Named(NamedKey::PageDown) => self.scroll_by(state, layout, page as isize),
                Key::Named(NamedKey::Home) => self.scroll_to(
                    state,
                    layout,
                    ensure_visible(self.resolved_offset(layout), page, 0),
                ),
                Key::Named(NamedKey::End) => self.scroll_to(
                    state,
                    layout,
                    ensure_visible(
                        self.resolved_offset(layout),
                        page,
                        self.row_count().saturating_sub(1),
                    ),
                ),
                _ => Handled::Event(TextDisplayEvent::KeyPressed {
                    key: format!("{:?}", key.key),
                    modifiers: key.modifiers,
                }),
            },
            RoutedEvent::Wheel { dy, .. } => {
                match crate::primitives::scrollbar::wheel_row_delta(*dy, 1.0) {
                    0 => Handled::Ignored,
                    rows => self.scroll_by(state, layout, rows),
                }
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::event::MouseButton::Left),
                local,
                ..
            } => {
                let (x, y) = (layout.area.x + local.x, layout.area.y + local.y);
                cx.request_focus(None);
                match layout.hit_test(x, y) {
                    TextDisplayHit::ScrollbarThumb => {
                        state.dragging_thumb = true;
                        cx.capture_pointer();
                        Handled::Consumed
                    }
                    TextDisplayHit::ScrollbarTrackBefore => {
                        self.scroll_by(state, layout, -(page as isize))
                    }
                    TextDisplayHit::ScrollbarTrackAfter => self.scroll_by(state, layout, page as isize),
                    _ => Handled::Consumed,
                }
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Move,
                local,
                ..
            } if state.dragging_thumb => {
                let y = layout.area.y + local.y;
                self.scroll_to(state, layout, self.offset_for_track_y(layout, y))
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Up(crate::event::MouseButton::Left),
                ..
            } if state.dragging_thumb => {
                state.dragging_thumb = false;
                cx.release_pointer();
                Handled::Consumed
            }
            RoutedEvent::Action(crate::a11y::A11yAction::Focus) => {
                cx.request_focus(None);
                Handled::Consumed
            }
            RoutedEvent::Action(crate::a11y::A11yAction::ScrollUp) => self.scroll_by(state, layout, -1),
            RoutedEvent::Action(crate::a11y::A11yAction::ScrollDown) => self.scroll_by(state, layout, 1),
            _ => Handled::Ignored,
        }
    }

    /// A windowed display asks for the lines around its viewport once it
    /// scrolls outside the loaded slice — see
    /// [`crate::behaviour::row_window::RowsRequestLatch`].
    fn data_requested(
        &self,
        state: &mut TextDisplayState,
        layout: &TextDisplayPrimitiveLayout,
    ) -> Option<crate::UiEvent> {
        let window = layout.window.as_ref()?;
        state.rows_requested.request(
            self.id,
            window.window(),
            window.loaded.1,
            window.scroll,
            window.viewport_rows,
        )
    }

    fn state_summary(&self, state: &TextDisplayState) -> crate::interactive::StateSummary {
        crate::interactive::StateSummary::scroll(state.scroll.get(&self.scroll_offset))
    }
}
/// [`crate::interactive::SubTarget`] for the scrollbar gutter — see
/// [`crate::interactive::Interactive::sub_regions`].
const SUB_SCROLLBAR: u64 = 0;
/// First [`crate::interactive::SubTarget`] a visible line claims; line
/// `i` is `SUB_LINE_BASE + i`.
const SUB_LINE_BASE: u64 = 1;

/// Routes a headless `TuiDriver` test can't reach: the
/// assistive-technology scroll actions, the widget-local `sub_regions`
/// translation out of the layout's body-local frame, and the
/// scrollbar-thumb drag.
#[cfg(test)]
mod interactive_tests {
    use super::*;
    use crate::a11y::A11yAction;
    use crate::event::{Key, MouseButton, NamedKey, Point};
    use crate::interactive::{
        EventCx, EventHost, FocusSpec, Interactive, KeyEvent, PointerKind, RoutedEvent, SubTarget,
    };
    use crate::primitive::{LayoutCtx, Primitive, Units};
    use crate::types::Modifiers;
    use crate::PointerShape;

    /// Minimal [`EventHost`] mock — the same shape every other widget
    /// module's own `Interactive` tests use.
    #[derive(Debug, Default)]
    struct MockHost {
        focus_requests: Vec<Option<SubTarget>>,
        captures: usize,
        releases: usize,
    }

    impl EventHost for MockHost {
        fn is_focused(&self, _id: &WidgetId, _sub: Option<SubTarget>) -> bool {
            false
        }
        fn request_focus(&mut self, _id: &WidgetId, sub: Option<SubTarget>) {
            self.focus_requests.push(sub);
        }
        fn capture_pointer(&mut self, _id: &WidgetId) {
            self.captures += 1;
        }
        fn release_pointer(&mut self) {
            self.releases += 1;
        }
        fn set_cursor(&mut self, _cursor: PointerShape) {}
        fn request_redraw(&mut self) {}
        fn emit(&mut self, _event: crate::UiEvent) {}
        fn units(&self) -> Units {
            Units::Cells
        }
    }

    fn plain_line(text: &str) -> TextDisplayLine {
        TextDisplayLine {
            spans: vec![StyledSpan::plain(text)],
            decoration: Decoration::Normal,
            timestamp: None,
        }
    }

    fn display(count: usize, show_scrollbar: bool) -> TextDisplay {
        TextDisplay {
            id: WidgetId::new("td"),
            lines: (0..count)
                .map(|i| plain_line(&format!("L{i:02}")))
                .collect(),
            scroll_offset: 0,
            items_window: None,
            auto_scroll: false,
            max_lines: 0,
            has_focus: true,
            title: None,
            show_scrollbar,
        }
    }

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 20.0, 5.0)
    }

    fn layout_of(td: &TextDisplay) -> TextDisplayPrimitiveLayout {
        Primitive::layout(td, &LayoutCtx::new(1.0, 1.0, false), area())
    }

    fn key(named: NamedKey) -> RoutedEvent {
        RoutedEvent::Key(KeyEvent {
            key: Key::Named(named),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn handle(
        td: &TextDisplay,
        state: &mut TextDisplayState,
        event: &RoutedEvent,
        layout: &TextDisplayPrimitiveLayout,
        host: &mut MockHost,
    ) -> Handled<TextDisplayEvent> {
        let mut cx = EventCx::new(td.id, host);
        Interactive::handle(td, state, event, layout, &mut cx)
    }

    /// A fractional pane rect (a ratio-split pane) paints only its whole
    /// cells, so the cell-grid layout must not claim the column past them —
    /// that column belongs to whatever sits beside the pane.
    #[test]
    fn a_fractional_cell_rect_hit_tests_only_its_painted_cells() {
        let td = display(3, false);
        let layout = Primitive::layout(
            &td,
            &LayoutCtx::new(1.0, 1.0, false),
            Rect::new(0.0, 0.0, 23.4, 5.0),
        );
        assert_eq!(layout.area, Rect::new(0.0, 0.0, 23.0, 5.0));
        assert_eq!(layout.hit_test(22.0, 0.0), TextDisplayHit::Line(0));
        assert_eq!(layout.hit_test(23.0, 0.0), TextDisplayHit::Empty);
    }

    #[test]
    fn an_empty_display_is_not_a_focus_stop() {
        assert_eq!(Interactive::focus(&display(0, false)), FocusSpec::None);
        assert_eq!(Interactive::focus(&display(3, false)), FocusSpec::Stop);
    }

    #[test]
    fn page_down_then_page_up_walks_a_viewport_at_a_time() {
        let td = display(20, false);
        let layout = layout_of(&td);
        let mut host = MockHost::default();
        let mut state = TextDisplayState::default();
        assert_eq!(
            handle(
                &td,
                &mut state,
                &key(NamedKey::PageDown),
                &layout,
                &mut host
            ),
            Handled::Event(TextDisplayEvent::Scrolled { new_offset: 5 })
        );
        // The descriptor still reports offset 0 (the app owns it), so a
        // page up from the painted offset lands back at the top.
        assert_eq!(
            handle(&td, &mut state, &key(NamedKey::PageUp), &layout, &mut host),
            Handled::Consumed
        );
    }

    #[test]
    fn end_scrolls_to_the_last_page_and_home_back_to_the_top() {
        let mut td = display(20, false);
        let layout = layout_of(&td);
        let mut host = MockHost::default();
        let mut state = TextDisplayState::default();
        assert_eq!(
            handle(&td, &mut state, &key(NamedKey::End), &layout, &mut host),
            Handled::Event(TextDisplayEvent::Scrolled { new_offset: 15 })
        );
        td.scroll_offset = 15;
        let layout = layout_of(&td);
        assert_eq!(
            handle(&td, &mut state, &key(NamedKey::Home), &layout, &mut host),
            Handled::Event(TextDisplayEvent::Scrolled { new_offset: 0 })
        );
    }

    #[test]
    fn an_unhandled_key_is_handed_back_to_the_app() {
        let td = display(3, false);
        let layout = layout_of(&td);
        let mut host = MockHost::default();
        let mut state = TextDisplayState::default();
        let handled = handle(
            &td,
            &mut state,
            &RoutedEvent::Key(KeyEvent {
                key: Key::Char('?'),
                modifiers: Modifiers::default(),
                repeat: false,
            }),
            &layout,
            &mut host,
        );
        assert!(matches!(
            handled,
            Handled::Event(TextDisplayEvent::KeyPressed { .. })
        ));
    }

    #[test]
    fn the_wheel_scrolls_one_line_per_notch() {
        let td = display(20, false);
        let layout = layout_of(&td);
        let mut host = MockHost::default();
        let mut state = TextDisplayState::default();
        // A negative `dy` is a notch toward later lines — the sign every
        // wheel-scrolling widget reads through `wheel_row_delta`.
        let handled = handle(
            &td,
            &mut state,
            &RoutedEvent::Wheel {
                dx: 0.0,
                dy: -1.0,
                local: Point::new(1.0, 1.0),
                mods: Modifiers::default(),
            },
            &layout,
            &mut host,
        );
        assert_eq!(
            handled,
            Handled::Event(TextDisplayEvent::Scrolled { new_offset: 1 })
        );
    }

    #[test]
    fn a_thumb_drag_scrubs_the_offset_and_releases_the_capture() {
        let td = display(20, true);
        let layout = layout_of(&td);
        let thumb = layout.layout.thumb_bounds.expect("thumb");
        let track = layout.layout.scrollbar_bounds.expect("track");
        let mut host = MockHost::default();
        let mut state = TextDisplayState::default();

        let press = RoutedEvent::Pointer {
            kind: PointerKind::Down(MouseButton::Left),
            sub: None,
            local: Point::new(
                layout.body.x + thumb.x + 0.5 - layout.area.x,
                layout.body.y + thumb.y + 0.5 - layout.area.y,
            ),
            mods: Modifiers::default(),
        };
        assert_eq!(
            handle(&td, &mut state, &press, &layout, &mut host),
            Handled::Consumed
        );
        assert!(state.dragging_thumb);
        assert_eq!(host.captures, 1);

        // Drag to the bottom of the track: the full scrollable range.
        let drag = RoutedEvent::Pointer {
            kind: PointerKind::Move,
            sub: None,
            local: Point::new(
                layout.body.x + track.x - layout.area.x,
                layout.body.y + track.y + track.height - layout.area.y,
            ),
            mods: Modifiers::default(),
        };
        assert_eq!(
            handle(&td, &mut state, &drag, &layout, &mut host),
            Handled::Event(TextDisplayEvent::Scrolled { new_offset: 15 })
        );

        let release = RoutedEvent::Pointer {
            kind: PointerKind::Up(MouseButton::Left),
            sub: None,
            local: Point::new(0.0, 0.0),
            mods: Modifiers::default(),
        };
        assert_eq!(
            handle(&td, &mut state, &release, &layout, &mut host),
            Handled::Consumed
        );
        assert!(!state.dragging_thumb);
        assert_eq!(host.releases, 1);
    }

    #[test]
    fn a_click_on_the_track_pages_toward_it() {
        let td = display(20, true);
        let layout = layout_of(&td);
        let track = layout.layout.scrollbar_bounds.expect("track");
        let mut host = MockHost::default();
        let mut state = TextDisplayState::default();
        let press = RoutedEvent::Pointer {
            kind: PointerKind::Down(MouseButton::Left),
            sub: None,
            local: Point::new(
                layout.body.x + track.x + 0.5 - layout.area.x,
                layout.body.y + track.y + track.height - 0.5 - layout.area.y,
            ),
            mods: Modifiers::default(),
        };
        assert_eq!(
            handle(&td, &mut state, &press, &layout, &mut host),
            Handled::Event(TextDisplayEvent::Scrolled { new_offset: 5 })
        );
        assert_eq!(host.focus_requests, vec![None]);
    }

    #[test]
    fn a11y_scroll_actions_move_one_line_either_way() {
        let mut td = display(20, false);
        td.scroll_offset = 4;
        let layout = layout_of(&td);
        let mut host = MockHost::default();
        let mut state = TextDisplayState::default();
        assert_eq!(
            handle(
                &td,
                &mut state,
                &RoutedEvent::Action(A11yAction::ScrollDown),
                &layout,
                &mut host
            ),
            Handled::Event(TextDisplayEvent::Scrolled { new_offset: 5 })
        );
        assert_eq!(
            handle(
                &td,
                &mut state,
                &RoutedEvent::Action(A11yAction::ScrollUp),
                &layout,
                &mut host
            ),
            Handled::Event(TextDisplayEvent::Scrolled { new_offset: 3 })
        );
    }

    #[test]
    fn sub_regions_name_the_gutter_then_each_visible_line_in_widget_local_space() {
        let mut td = display(20, true);
        td.title = Some(crate::types::StyledText {
            spans: vec![StyledSpan::plain("Output")],
        });
        let layout = layout_of(&td);
        let regions = Interactive::sub_regions(&td, &layout);
        assert_eq!(regions[0].1, SubTarget(0), "the gutter comes first");
        assert_eq!(regions[1].1, SubTarget(1), "then line 0");
        // The title row shifts every body region down one row in
        // widget-local space.
        assert_eq!(regions[1].0.y, 1.0);
    }

    #[test]
    fn a_pixel_layout_reserves_the_shared_twelve_unit_gutter() {
        let td = display(40, true);
        let layout = Primitive::layout(
            &td,
            &LayoutCtx::new(16.0, 8.0, false),
            Rect::new(0.0, 0.0, 200.0, 160.0),
        );
        let track = layout.layout.scrollbar_bounds.expect("track");
        assert_eq!(track.width, 12.0);
        assert_eq!(track.x, 200.0 - 12.0);
    }

    #[test]
    fn measure_reports_a_row_per_line_plus_the_title_row() {
        let mut td = display(3, false);
        let ctx = LayoutCtx::new(1.0, 1.0, false);
        let avail = crate::primitive::Size::new(40.0, 40.0);
        assert_eq!(Primitive::measure(&td, &ctx, avail).height, Some(3.0));
        td.title = Some(crate::types::StyledText {
            spans: vec![StyledSpan::plain("Output")],
        });
        assert_eq!(Primitive::measure(&td, &ctx, avail).height, Some(4.0));
    }
}

/// Cell-grid paint tests for [`cell_paint`], driven through
/// [`crate::tui::draw_text_display`] — the delegate that owns the
/// `Buffer` → [`crate::CellSurface`] adapter the painter needs.
#[cfg(all(test, feature = "tui"))]
mod cell_paint_tests {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    use super::cell_test_paint::{draw_text_display, tui_text_display_layout};
    use super::{TextDisplay, TextDisplayHit, TextDisplayLine, TextDisplayLineMeasure};
    use crate::theme::Theme;
    use crate::types::{Color, Decoration, StyledSpan, WidgetId};

    fn line(text: &str) -> TextDisplayLine {
        TextDisplayLine {
            spans: vec![StyledSpan::plain(text)],
            decoration: Decoration::Normal,
            timestamp: None,
        }
    }

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    #[test]
    fn paints_lines_top_to_bottom() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        let display = TextDisplay {
            id: WidgetId::new("td"),
            lines: vec![line("alpha"), line("beta"), line("gamma")],
            scroll_offset: 0,
            items_window: None,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: false,
        };
        draw_text_display(
            &mut buf,
            Rect::new(0, 0, 20, 5),
            &display,
            &Theme::default(),
        );
        let row0: String = (0..5).map(|x| cell_char(&buf, x, 0)).collect();
        let row1: String = (0..4).map(|x| cell_char(&buf, x, 1)).collect();
        let row2: String = (0..5).map(|x| cell_char(&buf, x, 2)).collect();
        assert_eq!(row0, "alpha");
        assert_eq!(row1, "beta");
        assert_eq!(row2, "gamma");
    }

    #[test]
    fn span_fg_override_wins() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        let display = TextDisplay {
            id: WidgetId::new("td"),
            lines: vec![TextDisplayLine {
                spans: vec![
                    StyledSpan {
                        text: "key:".into(),
                        fg: Some(Color::rgb(99, 0, 0)),
                        bg: None,
                        bold: false,
                        italic: false,
                        underline: false,
                    },
                    StyledSpan::plain(" value"),
                ],
                decoration: Decoration::Normal,
                timestamp: None,
            }],
            scroll_offset: 0,
            items_window: None,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: false,
        };
        draw_text_display(
            &mut buf,
            Rect::new(0, 0, 20, 5),
            &display,
            &Theme::default(),
        );
        // 'k' at col 0 should be in (99, 0, 0).
        let fg = buf[(0u16, 0u16)].fg;
        assert_eq!(fg, ratatui::style::Color::Rgb(99, 0, 0));
    }

    #[test]
    fn auto_scroll_pins_to_bottom() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 3));
        let display = TextDisplay {
            id: WidgetId::new("td"),
            lines: (0..10).map(|i| line(&format!("line{i}"))).collect(),
            scroll_offset: 0,
            items_window: None,
            auto_scroll: true,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: false,
        };
        draw_text_display(
            &mut buf,
            Rect::new(0, 0, 20, 3),
            &display,
            &Theme::default(),
        );
        // Last 3 lines visible (line7, line8, line9).
        let row0: String = (0..5).map(|x| cell_char(&buf, x, 0)).collect();
        let row1: String = (0..5).map(|x| cell_char(&buf, x, 1)).collect();
        let row2: String = (0..5).map(|x| cell_char(&buf, x, 2)).collect();
        assert_eq!(row0, "line7");
        assert_eq!(row1, "line8");
        assert_eq!(row2, "line9");
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 5));
        let display = TextDisplay {
            id: WidgetId::new("td"),
            lines: vec![line("x")],
            scroll_offset: 0,
            items_window: None,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: false,
        };
        draw_text_display(&mut buf, Rect::new(0, 0, 0, 5), &display, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    // Parametrized over the area's origin — LESSONS.md "Layout helpers
    // must return coords in the same frame across backends".
    // `tui_text_display_layout` is local-frame (it only takes `body`
    // width/height, never `area.x`/`area.y`), but `draw_text_display`
    // paints each row at `body.y + vis.bounds.y` (and the scrollbar gutter
    // at `body.x`/`body.y`) — untested at a non-zero `area.y`/`area.x`
    // until now.
    fn scrollbar_thumb_round_trip_at(origin_x: u16, origin_y: u16) {
        let mut buf = Buffer::empty(Rect::new(0, 0, origin_x + 20, origin_y + 5));
        let display = TextDisplay {
            id: WidgetId::new("td"),
            lines: (0..20).map(|i| line(&format!("line{i}"))).collect(),
            scroll_offset: 0,
            items_window: None,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: true,
        };
        draw_text_display(
            &mut buf,
            Rect::new(origin_x, origin_y, 20, 5),
            &display,
            &Theme::default(),
        );

        // Scrollbar gutter at the last column of the area. Thumb should be
        // at top (scroll_offset=0).
        assert_eq!(cell_char(&buf, origin_x + 19, origin_y), '█');

        // Use layout for hit-test — layout is local-frame (0,0-based), so
        // the hit_test coordinates are NOT shifted by origin_x/origin_y.
        let layout = display
            .layout_with_scrollbar(20.0, 5.0, 1.0, 1.0, |_| TextDisplayLineMeasure::new(1.0));
        let thumb = layout.thumb_bounds.expect("thumb bounds present");
        let hit = layout.hit_test(thumb.x + 0.5, thumb.y + 0.5);
        assert_eq!(hit, TextDisplayHit::ScrollbarThumb);
    }

    #[test]
    fn scrollbar_thumb_paint_and_click_round_trip() {
        scrollbar_thumb_round_trip_at(0, 0);
    }

    /// Regression guard: the same scrollbar paint + hit_test round trip must
    /// hold when the display isn't painted at the screen origin.
    #[test]
    fn scrollbar_thumb_paint_and_click_round_trip_at_nonzero_origin() {
        scrollbar_thumb_round_trip_at(7, 13);
    }

    #[test]
    fn scrollbar_track_after_hit() {
        let display = TextDisplay {
            id: WidgetId::new("td"),
            lines: (0..20).map(|i| line(&format!("line{i}"))).collect(),
            scroll_offset: 0,
            items_window: None,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: true,
        };
        let layout = display
            .layout_with_scrollbar(20.0, 5.0, 1.0, 1.0, |_| TextDisplayLineMeasure::new(1.0));
        // Click below the thumb → TrackAfter.
        let hit = layout.hit_test(19.5, 4.5);
        assert_eq!(hit, TextDisplayHit::ScrollbarTrackAfter);
    }

    #[test]
    fn no_scrollbar_when_disabled() {
        let display = TextDisplay {
            id: WidgetId::new("td"),
            lines: (0..20).map(|i| line(&format!("line{i}"))).collect(),
            scroll_offset: 0,
            items_window: None,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: false,
        };
        let layout = display.layout(20.0, 5.0, |_| TextDisplayLineMeasure::new(1.0));
        assert!(layout.scrollbar_bounds.is_none());
        assert!(layout.thumb_bounds.is_none());
    }

    #[test]
    fn scrollbar_body_width_reduced() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        // 26-char line in a 5-cell-tall viewport: at a 19-col (20 - 1
        // scrollbar) budget it wraps onto a second row
        // rather than being cut mid-word — this test now asserts the
        // scrollbar gutter still lands in the same column either way.
        let display = TextDisplay {
            id: WidgetId::new("td"),
            lines: vec![line("abcdefghijklmnopqrstuvwxyz")],
            scroll_offset: 0,
            items_window: None,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: true,
        };
        draw_text_display(
            &mut buf,
            Rect::new(0, 0, 20, 5),
            &display,
            &Theme::default(),
        );
        // Row 0 holds the first wrapped chunk, stopping before the
        // scrollbar column (col 19); row 1 is the continuation, marked
        // so it isn't mistaken for an unrelated new line.
        let row0: String = (0..19).map(|x| cell_char(&buf, x, 0)).collect();
        let row1: String = (0..19).map(|x| cell_char(&buf, x, 1)).collect();
        assert!(
            !row0.contains('…'),
            "wrap mode must not truncate, got {row0:?}"
        );
        assert!(
            row1.starts_with('↳'),
            "continuation row should carry the wrap marker, got {row1:?}"
        );
        assert_ne!(cell_char(&buf, 19, 0), 't');
    }

    #[test]
    fn track_before_page_up_reaches_line_zero() {
        // Scenario: 40 lines, 30-row viewport, scrolled to bottom
        // (offset=10, showing lines 10..39). Consumer clicks TrackBefore
        // and pages up by visible_lines.len(). After paging, offset
        // should be 0 and lines 0..29 should be visible.
        let viewport_h = 30.0_f32;
        let total_lines = 40;

        // Step 1: layout at the bottom (scroll_offset=10).
        let display_bottom = TextDisplay {
            id: WidgetId::new("td"),
            lines: (0..total_lines)
                .map(|i| line(&format!("L{i:02}")))
                .collect(),
            scroll_offset: 10,
            items_window: None,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: true,
        };
        let layout_bottom =
            display_bottom.layout_with_scrollbar(40.0, viewport_h, 1.0, 1.0, |_| {
                TextDisplayLineMeasure::new(1.0)
            });
        assert_eq!(layout_bottom.resolved_scroll_offset, 10);
        assert_eq!(layout_bottom.visible_lines.len(), 30);
        assert_eq!(layout_bottom.visible_lines[0].line_idx, 10);
        assert_eq!(layout_bottom.visible_lines[29].line_idx, 39);

        // Thumb should be at the bottom of the track.
        let thumb = layout_bottom.thumb_bounds.expect("thumb");
        assert!(
            thumb.y > 0.0,
            "thumb should not be at top when scrolled to bottom"
        );

        // TrackBefore should exist above the thumb.
        let hit_top = layout_bottom.hit_test(39.5, 0.5);
        assert_eq!(hit_top, TextDisplayHit::ScrollbarTrackBefore);

        // Step 2: consumer pages up by visible_lines.len().
        let page_size = layout_bottom.visible_lines.len();
        let new_offset = 10_usize.saturating_sub(page_size);
        assert_eq!(new_offset, 0, "page-up from 10 by 30 should reach 0");

        // Step 3: re-layout at offset 0.
        let display_top = TextDisplay {
            id: WidgetId::new("td"),
            lines: (0..total_lines)
                .map(|i| line(&format!("L{i:02}")))
                .collect(),
            scroll_offset: new_offset,
            items_window: None,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: true,
        };
        let layout_top = display_top.layout_with_scrollbar(40.0, viewport_h, 1.0, 1.0, |_| {
            TextDisplayLineMeasure::new(1.0)
        });
        assert_eq!(layout_top.resolved_scroll_offset, 0);
        assert_eq!(
            layout_top.visible_lines[0].line_idx, 0,
            "first visible line should be 0 after paging to top"
        );
        assert_eq!(layout_top.visible_lines.len(), 30);

        // Thumb should now be at y=0.
        let thumb_top = layout_top.thumb_bounds.expect("thumb at top");
        assert_eq!(
            thumb_top.y, 0.0,
            "thumb should be at top when scroll_offset=0"
        );
    }

    #[test]
    fn wrap_splits_long_line_and_shows_continuation_marker() {
        // 12-col viewport, no scrollbar, no timestamp: gutter reserved
        // for the continuation marker is 2 cols (see
        // `wrap_continuation_marker_width`), leaving a 10-col content
        // budget — the same budget `text_util`'s own
        // `word_wrap_packs_multiple_words_per_row` test wraps "the quick
        // brown fox" at, into exactly ["the quick", "brown fox"].
        let mut buf = Buffer::empty(Rect::new(0, 0, 12, 5));
        let display = TextDisplay {
            id: WidgetId::new("td"),
            lines: vec![line("the quick brown fox")],
            scroll_offset: 0,
            items_window: None,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: false,
        };
        draw_text_display(
            &mut buf,
            Rect::new(0, 0, 12, 5),
            &display,
            &Theme::default(),
        );
        let row0: String = (0..12).map(|x| cell_char(&buf, x, 0)).collect();
        let row1: String = (0..12).map(|x| cell_char(&buf, x, 1)).collect();
        assert_eq!(row0.trim_end(), "the quick");
        assert!(
            row1.starts_with('↳'),
            "continuation row should start with the wrap marker, got {row1:?}"
        );
        assert!(
            row1.contains("brown fox"),
            "continuation row should carry the rest of the line, got {row1:?}"
        );
    }

    #[test]
    fn tui_text_display_layout_row_count_matches_paint_for_wrapped_line() {
        // The pure hit-testing layout must agree with what `draw_text_display`
        // actually paints — a wrapped line occupies more
        // than one visible row in both.
        let display = TextDisplay {
            id: WidgetId::new("td"),
            lines: vec![
                line("the quick brown fox jumps over the lazy dog"),
                line("short"),
            ],
            scroll_offset: 0,
            items_window: None,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: None,
            show_scrollbar: false,
        };
        let layout = tui_text_display_layout(&display, Rect::new(0, 0, 10, 10));
        // One `VisibleTextDisplayLine` per source line, with `bounds.height`
        // equal to its wrapped row count (see `text_display_layout_wrap_heights`
        // in `primitives::text_display`). Line 0 must wrap to >1 row, and
        // line 1 ("short") must start immediately after all of line 0's
        // wrapped rows — never overlapping them.
        let line0 = layout
            .visible_lines
            .iter()
            .find(|v| v.line_idx == 0)
            .expect("line 0 visible");
        assert!(
            line0.bounds.height > 1.0,
            "expected line 0 to occupy multiple visual rows, layout: {:?}",
            layout.visible_lines
        );
        let line1 = layout
            .visible_lines
            .iter()
            .find(|v| v.line_idx == 1)
            .expect("line 1 visible");
        assert!(
            line1.bounds.y >= line0.bounds.height,
            "line 1 must start after all of line 0's wrapped rows"
        );
    }

    #[test]
    fn title_row_paints_above_a_body_that_shrinks_by_one_row() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 4));
        let display = TextDisplay {
            id: WidgetId::new("td"),
            lines: vec![line("alpha"), line("beta")],
            scroll_offset: 0,
            items_window: None,
            auto_scroll: false,
            max_lines: 0,
            has_focus: false,
            title: Some(crate::types::StyledText {
                spans: vec![StyledSpan::plain("Output")],
            }),
            show_scrollbar: false,
        };
        draw_text_display(
            &mut buf,
            Rect::new(0, 0, 20, 4),
            &display,
            &Theme::default(),
        );
        let row0: String = (0..6).map(|x| cell_char(&buf, x, 0)).collect();
        let row1: String = (0..5).map(|x| cell_char(&buf, x, 1)).collect();
        let row2: String = (0..4).map(|x| cell_char(&buf, x, 2)).collect();
        assert_eq!(row0, "Output");
        assert_eq!(row1, "alpha");
        assert_eq!(row2, "beta");
    }
}

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
            items_window: None,
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
            items_window: None,
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
            items_window: None,
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

    // wrap-helper unit tests live in `wrap::tests`, next to the helpers
    // themselves — both are gated on the rasteriser features (see the
    // `wrap` module doc), so they can't sit in this ungated `mod tests`.
}

/// Cell-painting entry points used by this module's own `#[cfg(test)]`
/// rasteriser tests: they build the `Buffer` -> [`crate::CellSurface`]
/// adapter (private to `crate::tui`) the same way
/// [`crate::primitive::Primitive::paint_cells`] does, so a test can
/// paint into a bare ratatui buffer without a backend.
#[cfg(all(test, feature = "tui"))]
mod cell_test_paint {
    use super::*;
    use crate::theme::Theme;
    use ratatui::buffer::Buffer;

    fn surface<'a>(
        buf: &'a mut Buffer,
        sink: &'a mut Option<(u16, u16, crate::cell_surface::CursorShape)>,
    ) -> impl crate::CellSurface + 'a {
        crate::tui::cell_surface::TuiCellSurface::over(
            buf,
            crate::backend::ColorDepth::TrueColor,
            sink,
        )
    }

    pub fn tui_text_display_layout(
        display: &TextDisplay,
        area: ratatui::layout::Rect,
    ) -> TextDisplayLayout {
        cell_layout(display, q_rect(area)).layout
    }

    pub fn draw_text_display(
        buf: &mut Buffer,
        area: ratatui::layout::Rect,
        display: &TextDisplay,
        theme: &Theme,
    ) {
        let resolved = cell_layout(display, q_rect(area));
        let mut sink = None;
        cell_paint::paint(display, &resolved, &mut surface(buf, &mut sink), theme);
    }

    fn q_rect(area: ratatui::layout::Rect) -> Rect {
        Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        )
    }
}
