//! `StatusBar` primitive: a horizontal row of styled, optionally
//! clickable segments, with left-aligned and right-aligned halves.
//!
//! Used for editor status bars (mode / filename / cursor position /
//! LSP status / etc.), footer bars in data-explorer apps, and any
//! horizontal summary strip. Segments carry their own colours so the
//! bar can mix mode badges, dim hints, and warning accents freely.
//!
//! Segments that declare an `action_id` become click targets. The
//! backend resolves a click column to a segment and emits
//! `StatusBarEvent::SegmentClicked { id }`. Apps map the `WidgetId`
//! back to their own action dispatch (see vimcode's
//! `render::status_action_id` / `StatusAction::from_id`).
//!
//! # Backend contract
//!
//! **`StatusBar` has narrow-bar handling that backends MUST implement
//! correctly** or the right segments overlap / touch / overflow the left
//! segments on narrow widths (issue #159). A purely declarative paint
//! that just renders all segments left-aligned and all segments right-
//! aligned looks fine on wide bars and ugly-to-broken on narrow ones.
//!
//! Per paint, the backend MUST:
//!
//! 1. **Decide which right segments fit** by calling
//!    [`StatusBar::fit_right_start`] with the bar's available width,
//!    a minimum gap (e.g. 2 cells / 16 px), and a measurement closure
//!    in the backend's native unit. Returns the index where rendering
//!    of right segments should *start* — segments at indices below it
//!    are dropped to fit.
//!
//! 2. **Render only the visible slice** — `&right_segments[start..]` —
//!    right-aligned. Segments before `start` must NOT be drawn.
//!
//! 3. **Skip dropped segments in click handlers.** Call [`StatusBar::layout`]
//!    and resolve clicks with [`StatusBarLayout::hit_test`] — its
//!    `hit_regions` only ever cover the segments `resolved_right_start`
//!    kept visible. (The pre-D6 `StatusBar::resolve_click_fit_chars` did
//!    the same thing by hand for char-cell backends; it was removed in
//!    issue #1109 — use `layout` + `hit_test` instead.) Otherwise
//!    clicks on columns where dropped segments *used to be* will trigger
//!    their actions even though the user can't see them.
//!
//! Convention for app-side priority: **`right_segments` is built
//! least-important first, most-important (e.g. cursor position) last.**
//! `fit_right_start` drops from the front, so the rightmost (highest-
//! priority) segments stay visible at the right edge of the bar. Note
//! that this is `right_segments` *index* order, not the visual
//! left-to-right order a human would pick by eye — a segment that reads
//! naturally next to its neighbours may still need to move to the end of
//! the vector to be protected from the drop.
//!
//! Skipping step 1 + 2 makes narrow bars look like `BARMODE filenameSpaces:`
//! (touching, no gap) or worse (right segments overdrawing left in TUI).
//!
//! Skipping step 3 means clicking blank space at the left of the right
//! group can trigger random toggles — confusing and undebuggable.
//!
//! See vimcode's `src/gtk/quadraui_gtk.rs::draw_status_bar` and
//! `src/tui_main/quadraui_tui.rs::draw_status_bar` for reference
//! implementations.

use crate::event::Rect;
use crate::types::{Color, Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a status bar.
///
/// # Examples
///
/// ```
/// use quadraui::{
///     Color, StatusBar, StatusBarHit, StatusBarSegment, StatusSegmentMeasure, WidgetId,
/// };
///
/// let bar = StatusBar {
///     id: WidgetId::new("status:editor"),
///     left_segments: vec![StatusBarSegment {
///         text: "NORMAL".to_string(),
///         fg: Color::rgb(0, 0, 0),
///         bg: Color::rgb(100, 200, 100),
///         bold: true,
///         action_id: Some(WidgetId::new("status:mode")),
///     }],
///     right_segments: vec![StatusBarSegment {
///         text: "Ln 1, Col 1".to_string(),
///         fg: Color::rgb(200, 200, 200),
///         bg: Color::rgb(30, 30, 30),
///         bold: false,
///         action_id: None,
///     }],
/// };
///
/// let measure = |seg: &StatusBarSegment| StatusSegmentMeasure::new(seg.text.chars().count() as f32);
/// let layout = bar.layout(80.0, 1.0, 1.0, measure);
///
/// assert_eq!(
///     layout.hit_test(0.0, 0.0),
///     StatusBarHit::Segment(WidgetId::new("status:mode"))
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusBar {
    pub id: WidgetId,
    pub left_segments: Vec<StatusBarSegment>,
    pub right_segments: Vec<StatusBarSegment>,
}

/// One styled segment in a `StatusBar`.
///
/// The `action_id` is an opaque app-defined string. The primitive does
/// not interpret it beyond echoing it back in `StatusBarEvent`. Apps
/// typically namespace (e.g. `"status:goto_line"`) per plugin invariant #4.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusBarSegment {
    pub text: String,
    pub fg: Color,
    pub bg: Color,
    #[serde(default)]
    pub bold: bool,
    /// `None` = non-interactive. `Some(id)` = clickable; backend emits
    /// `SegmentClicked { id }` when resolving a hit on this segment.
    #[serde(default)]
    pub action_id: Option<WidgetId>,
}

/// One pre-computed hit region used for click resolution. `(col, width, id)`
/// where `col` is the starting character column and `width` is the segment
/// width in cells. Computed internally by [`StatusBar::resolve_click`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusBarHitRegion {
    pub col: u16,
    pub width: u16,
    pub id: WidgetId,
}

/// Events a `StatusBar` emits back to the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatusBarEvent {
    /// A clickable segment was activated (mouse click, or future enter-on-focus).
    SegmentClicked { id: WidgetId },
    /// A key was pressed while the bar had focus and the primitive didn't
    /// consume it. Currently unused by vimcode (status bars don't take
    /// keyboard focus) but part of the primitive shape for parity.
    KeyPressed { key: String, modifiers: Modifiers },
}

impl StatusBar {
    /// Compute clickable hit regions given the bar's pixel/char width.
    /// Left segments accumulate from column 0; right segments are right-
    /// aligned inside `bar_width`.
    ///
    /// Pre-D6 shape (character-column `u16` pairs); kept private, as the
    /// shared body [`Self::resolve_click`] calls. The public `hit_regions`
    /// this once backed was removed in issue #1109 (zero uses in
    /// coord-tui's `main` and vimcode's `develop`) — use [`Self::layout`]
    /// and [`StatusBarLayout::hit_test`] instead for the crate's `Rect` +
    /// `Hit`-enum convention.
    fn hit_regions_impl(&self, bar_width: usize) -> Vec<StatusBarHitRegion> {
        let mut regions = Vec::new();
        let mut col: u16 = 0;
        for seg in &self.left_segments {
            let w = seg.text.chars().count() as u16;
            if let Some(id) = &seg.action_id {
                regions.push(StatusBarHitRegion {
                    col,
                    width: w,
                    id: id.clone(),
                });
            }
            col += w;
        }
        let right_width: usize = self
            .right_segments
            .iter()
            .map(|s| s.text.chars().count())
            .sum();
        let mut col = bar_width.saturating_sub(right_width) as u16;
        for seg in &self.right_segments {
            let w = seg.text.chars().count() as u16;
            if let Some(id) = &seg.action_id {
                regions.push(StatusBarHitRegion {
                    col,
                    width: w,
                    id: id.clone(),
                });
            }
            col += w;
        }
        regions
    }

    /// Resolve a column position to the `WidgetId` of the clicked segment,
    /// or `None` if the column falls outside any interactive segment.
    pub fn resolve_click(&self, click_col: u16, bar_width: usize) -> Option<WidgetId> {
        for region in self.hit_regions_impl(bar_width) {
            if click_col >= region.col && click_col < region.col + region.width {
                return Some(region.id);
            }
        }
        None
    }

    /// Compute how many leading right segments to drop so the visible right
    /// half fits in `bar_width` after reserving the left segments and a
    /// `min_gap` between the two halves. Returns the start index into
    /// `right_segments` — render `&right_segments[start..]`.
    ///
    /// Convention: `right_segments` is ordered least-important first,
    /// most-important last. Backends drop from the front (low priority) so
    /// the rightmost (highest-priority) segment, e.g. cursor position, is
    /// always preserved.
    ///
    /// Generic over the unit system: `measure` returns the width of a
    /// segment, `bar_width` and `min_gap` use the same unit. Each backend
    /// supplies its native measurer:
    ///
    /// - TUI passes `|seg| seg.text.chars().count()` (cells).
    /// - GTK passes a Pango closure that handles bold (pixels).
    /// - Win-GUI / macOS pass DirectWrite / Core Text measurers (pixels).
    ///
    /// The closure receives a full [`StatusBarSegment`] (not just the text)
    /// so backends can vary measurement based on `bold` and any future
    /// styling fields without API churn.
    ///
    /// The drop *policy* is shared across all backends so a fix or tweak
    /// here applies uniformly. Per-unit backends pick `min_gap` to suit
    /// their measurement (e.g. 2 cells / 16 px).
    pub fn fit_right_start<F>(&self, bar_width: usize, min_gap: usize, measure: F) -> usize
    where
        F: Fn(&StatusBarSegment) -> usize,
    {
        if self.right_segments.is_empty() {
            return 0;
        }
        let left_w: usize = self.left_segments.iter().map(&measure).sum();
        let widths: Vec<usize> = self.right_segments.iter().map(&measure).collect();
        let total: usize = widths.iter().sum();
        if left_w + min_gap + total <= bar_width {
            return 0;
        }
        let max_right = bar_width.saturating_sub(left_w + min_gap);
        let mut remaining = total;
        let last = widths.len() - 1;
        for (i, w) in widths.iter().enumerate() {
            if remaining <= max_right {
                return i;
            }
            // Always preserve the last (highest-priority) segment, even if
            // it alone overflows — better to clip one segment than to render
            // an empty right half.
            if i == last {
                return i;
            }
            remaining -= w;
        }
        last
    }

    /// Convenience wrapper around [`fit_right_start`] for char-cell backends
    /// (TUI). Same algorithm, with `measure = |seg| seg.text.chars().count()`.
    pub fn fit_right_start_chars(&self, bar_width: usize, min_gap: usize) -> usize {
        self.fit_right_start(bar_width, min_gap, |seg| seg.text.chars().count())
    }
}

// ── D6 Layout API ───────────────────────────────────────────────────────────
//
// Per Decision D6 in `docs/decisions/BACKEND_TRAIT_PROPOSAL.md` §9: primitives return
// fully-resolved `Layout` structs; backends rasterise verbatim. Second
// primitive to gain the new shape after `TabBar` — see that file for the
// established template.

/// Per-segment measurement supplied by the backend's layout caller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatusSegmentMeasure {
    pub width: f32,
}

impl StatusSegmentMeasure {
    pub fn new(width: f32) -> Self {
        Self { width }
    }
}

/// Which side of the bar a resolved segment belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusSegmentSide {
    Left,
    Right,
}

/// Resolved position of one visible status-bar segment after layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleStatusSegment {
    /// Index into `left_segments` (when `side == Left`) or
    /// `right_segments` (when `side == Right`).
    pub segment_idx: usize,
    pub side: StatusSegmentSide,
    pub bounds: Rect,
    /// `true` iff the segment has an `action_id`.
    pub clickable: bool,
}

/// Classification of a hit-test result on a status bar. Unlike
/// [`TabBarHit`](super::tab_bar::TabBarHit) the status bar has a single
/// interactive variant: a segment was clicked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusBarHit {
    /// Click landed on a clickable segment — carries its `action_id`.
    Segment(WidgetId),
    /// Click landed on a non-clickable segment or in the gap.
    Empty,
}

/// Fully-resolved status-bar layout. Backends iterate `visible_segments`
/// for painting and call [`Self::hit_test`] for clicks.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusBarLayout {
    /// Total bar width in the measurer's unit.
    pub bar_width: f32,
    /// Total bar height in the measurer's unit.
    pub bar_height: f32,
    /// All visible segments, left-side first (in their natural order),
    /// then the visible right-side segments (in their natural order).
    pub visible_segments: Vec<VisibleStatusSegment>,
    /// Ordered hit-region list. Non-clickable segments don't appear here;
    /// use [`Self::hit_test`] rather than walking this directly.
    pub hit_regions: Vec<(Rect, StatusBarHit)>,
    /// Index into `right_segments` at which rendering actually started —
    /// everything before this index was dropped by priority-drop. `0`
    /// means all right segments survived.
    pub resolved_right_start: usize,
}

impl StatusBarLayout {
    /// Test which clickable segment (if any) contains point `(x, y)`.
    /// Returns `StatusBarHit::Empty` when no region matches.
    pub fn hit_test(&self, x: f32, y: f32) -> StatusBarHit {
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        StatusBarHit::Empty
    }
}

/// Outer edge inset (surface-native units — px on every pixel backend)
/// reserved between the bar's own left/right edges and its outermost
/// segments, VS Code-like (issue #1155). Pixel backends (GTK/Win/macOS)
/// pass this to [`StatusBar::layout_padded`]. TUI's char-cell bar has no
/// use for a sub-cell inset and keeps calling plain [`StatusBar::layout`]
/// (equivalent to `layout_padded` with `edge_inset = 0.0`).
pub const PIXEL_EDGE_INSET: f32 = 10.0;

/// Per-segment horizontal padding (surface-native units — px on every
/// pixel backend), added to *both* sides of each segment's measured text
/// width, VS Code-like (issue #1155). See [`PIXEL_EDGE_INSET`]'s doc.
pub const PIXEL_SEGMENT_PADDING: f32 = 5.0;

impl StatusBar {
    /// Compute the full rendering + hit-test layout for this status bar.
    ///
    /// Per D6: layout decisions live here; backends consume the returned
    /// `StatusBarLayout` verbatim. The priority-drop policy for
    /// overflowing right segments is the same one as
    /// [`Self::fit_right_start`] — this method calls it internally.
    ///
    /// # Arguments
    ///
    /// - `bar_width`, `bar_height` — bar dimensions in the measurer's unit.
    /// - `min_gap` — minimum gap between the left group and the right
    ///   group. Right segments are dropped from the front (least important)
    ///   until they fit, preserving the gap. Typical values: `2` cells
    ///   (TUI), `16` pixels (native).
    /// - `measure(seg)` — returns a `StatusSegmentMeasure` for the segment.
    ///   Receives the full `StatusBarSegment` so measurers can vary by
    ///   `bold` or other style flags.
    ///
    /// All numeric arguments share the same unit; the primitive itself is
    /// unit-agnostic. See [`quadraui::TabBar::layout`] for TUI/pixel
    /// examples.
    ///
    /// Equivalent to [`Self::layout_padded`] with `edge_inset = 0.0` and
    /// `segment_padding = 0.0` — no outer inset, no per-segment padding.
    /// This is what TUI's char-cell bar wants (issue #1155): a monospace
    /// cell grid has no sub-cell pixels to spend on padding, and its
    /// callers already reserve visual breathing room with literal space
    /// characters inside segment text.
    pub fn layout<F>(
        &self,
        bar_width: f32,
        bar_height: f32,
        min_gap: f32,
        measure: F,
    ) -> StatusBarLayout
    where
        F: Fn(&StatusBarSegment) -> StatusSegmentMeasure,
    {
        self.layout_padded(bar_width, bar_height, min_gap, 0.0, 0.0, measure)
    }

    /// [`Self::layout`] plus a pixel-space outer edge inset and
    /// per-segment horizontal padding (issue #1155) — what pixel backends
    /// (GTK/Win/macOS) should call instead of `layout` so the right-most
    /// segment doesn't touch the window edge and adjacent segments get a
    /// visible gap without relying on the consumer's literal space
    /// characters (font-dependent, and often invisible on proportional
    /// fonts — quadraui#963).
    ///
    /// # Arguments
    ///
    /// - `edge_inset` — space reserved between the bar's own left/right
    ///   edges and its outermost segments. [`PIXEL_EDGE_INSET`] is the
    ///   recommended value for pixel backends (`10.0`, VS Code-like).
    /// - `segment_padding` — extra width added to *both* sides of every
    ///   segment's measured text width — i.e. each segment's `bounds`
    ///   grows by `2 * segment_padding` versus its raw text measurement.
    ///   [`PIXEL_SEGMENT_PADDING`] is the recommended value (`5.0`).
    ///   Hit-test regions cover the padded bounds, matching VS Code's
    ///   item hit boxes. Painting the segment's *text* inset within that
    ///   wider box (rather than flush to its left edge) is the paint
    ///   caller's job — see
    ///   [`native_surface_paint::paint`]
    ///   for the pixel-backend reference implementation.
    ///
    /// `min_gap` still reserves a gap between the left and right groups
    /// on top of both insets — unchanged from `layout`.
    pub fn layout_padded<F>(
        &self,
        bar_width: f32,
        bar_height: f32,
        min_gap: f32,
        edge_inset: f32,
        segment_padding: f32,
        measure: F,
    ) -> StatusBarLayout
    where
        F: Fn(&StatusBarSegment) -> StatusSegmentMeasure,
    {
        let pad = 2.0 * segment_padding;
        let mut visible_segments: Vec<VisibleStatusSegment> = Vec::new();
        let mut hit_regions: Vec<(Rect, StatusBarHit)> = Vec::new();

        // ── Left segments, left-to-right from the inset edge ───────────
        let mut cursor = edge_inset;
        for (i, seg) in self.left_segments.iter().enumerate() {
            let w = measure(seg).width + pad;
            let bounds = Rect::new(cursor, 0.0, w, bar_height);
            let clickable = seg.action_id.is_some();
            visible_segments.push(VisibleStatusSegment {
                segment_idx: i,
                side: StatusSegmentSide::Left,
                bounds,
                clickable,
            });
            if let Some(id) = &seg.action_id {
                hit_regions.push((bounds, StatusBarHit::Segment(id.clone())));
            }
            cursor += w;
        }
        let left_w = cursor;

        // ── Right segments: priority-drop so they fit ─────────────────
        //
        // Mirrors `fit_right_start` but stays in f32 to avoid rounding
        // artefacts when widths are fractional (proportional fonts).
        let right_widths: Vec<f32> = self
            .right_segments
            .iter()
            .map(|s| measure(s).width + pad)
            .collect();
        let total_right: f32 = right_widths.iter().sum();
        let max_right = (bar_width - left_w - min_gap - edge_inset).max(0.0);

        let resolved_right_start =
            if self.right_segments.is_empty() || total_right <= max_right + f32::EPSILON {
                0
            } else {
                let last = right_widths.len() - 1;
                let mut remaining = total_right;
                let mut found = last;
                for (i, w) in right_widths.iter().enumerate() {
                    if remaining <= max_right + f32::EPSILON {
                        found = i;
                        break;
                    }
                    // Always keep the last (highest-priority) segment, even if
                    // it alone overflows — better to clip one segment than to
                    // render an empty right half.
                    if i == last {
                        found = i;
                        break;
                    }
                    remaining -= w;
                }
                found
            };

        // Right segments right-aligned inside `bar_width`, inset from the
        // bar's own right edge by `edge_inset`. Rendered in the natural
        // `right_segments[start..]` order; first visible segment is
        // leftmost of the right group.
        let visible_right = &self.right_segments[resolved_right_start..];
        let visible_right_widths = &right_widths[resolved_right_start..];
        let total_visible: f32 = visible_right_widths.iter().sum();
        let mut cursor = (bar_width - edge_inset - total_visible).max(0.0);
        for (offset, seg) in visible_right.iter().enumerate() {
            let seg_idx = resolved_right_start + offset;
            let w = visible_right_widths[offset];
            let bounds = Rect::new(cursor, 0.0, w, bar_height);
            let clickable = seg.action_id.is_some();
            visible_segments.push(VisibleStatusSegment {
                segment_idx: seg_idx,
                side: StatusSegmentSide::Right,
                bounds,
                clickable,
            });
            if let Some(id) = &seg.action_id {
                hit_regions.push((bounds, StatusBarHit::Segment(id.clone())));
            }
            cursor += w;
        }

        StatusBarLayout {
            bar_width,
            bar_height,
            visible_segments,
            hit_regions,
            resolved_right_start,
        }
    }
}

// ── PaintSurface paint (#860, Phase 2d slice 3/9 of the PaintSurface
// milestone) ─────────────────────────────────────────────────────────────
//
// Before this, `gtk::status_bar::draw_status_bar` (Cairo/Pango),
// `macos::status_bar::draw_status_bar` (Core Graphics/Core Text) and
// `win::status_bar::draw_status_bar` (Direct2D/DirectWrite) each
// independently painted the same bar-fill + per-segment chrome with
// their own drawing API (quadraui#785 child #811, `docs/SMELL_AUDIT_2026-07.md`
// §5). `paint` below is the one shared implementation, written against
// [`crate::paint_surface::PaintSurface`] (#807, Phase 1) instead of any
// one backend's drawing API.
//
// # Divergences found — reported, not silently resolved
//
// 1. **Bold-aware measurement.** `gtk::status_bar::draw_status_bar` and
//    `win::status_bar::draw_status_bar` both measured a segment's width
//    against its own `bold` flag (a Pango `AttrList` weight / DirectWrite
//    `measure_text_styled`); `macos::status_bar::draw_status_bar` measured
//    (and rendered) every segment at the same non-bold weight — its own
//    module doc names this explicitly ("Bold segments — Tracked
//    separately... bold is currently ignored"). This migration adds
//    [`PaintSurface::surface_measure_text_styled`] (mirroring
//    [`PaintSurface::surface_draw_text_run_styled`]'s #810 shape), whose
//    default drops `bold` — exactly macOS's existing behaviour, so
//    `MacBackend` needs no override — while `GtkBackend` and `WinBackend`
//    override it to measure the real bold weight, preserving what they
//    already painted. No behaviour changes on any of the three backends;
//    the divergence itself is left unresolved and reported here, per this
//    issue's instructions.
//
// 2. **Zero-size guard + clip.** `macos::status_bar::draw_status_bar` and
//    `win::status_bar::draw_status_bar` already short-circuited to the
//    no-paint layout on a non-positive `width`/`line_height`
//    (quadraui#791 — re-verified while migrating, per this issue's
//    "re-verify before you implement": both already carried the fix,
//    contrary to this issue's own text, which claimed only `macos` had
//    it — reported here rather than assumed).
//    `gtk::status_bar::draw_status_bar` had no such guard: it still
//    clipped (so the fill/segments stayed invisible) but continued to
//    measure every segment's real text width regardless. `paint` below
//    applies the #791-fixed shape (early return, matching mac/win)
//    uniformly, which changes GTK's zero-size behaviour from "measure
//    real widths, paint nothing visible" to "measure nothing, paint
//    nothing" — invisible either way, so no observable regression, and
//    it's what #791 already established as the correct shape for the
//    other two backends.
//
// 3. **`find_bounds` semantics now genuinely differ per backend (#1155).**
//    Each backend's testing driver locates a painted segment's label by
//    recording where text actually got drawn — but "where text got
//    drawn" means different rects on different backends, and this PR is
//    the first change that makes the difference numerically observable.
//    `GtkBackend::draw_status_bar_interactive` explicitly re-records
//    `seg.bounds` (the full padded fill rect this `paint` computes) over
//    whatever Pango recorded, per that call site's own comment — so
//    `GtkDriver::find_bounds` returns the *segment's padded box*,
//    starting at `PIXEL_EDGE_INSET` for the lone left segment. macOS and
//    Windows have no such override: `MacDriver`/`WinDriver::find_bounds`
//    is backed purely by the glyph draw position this `paint` computes
//    below (`text_rect`, inset a further `PIXEL_SEGMENT_PADDING` past the
//    fill rect) — so it returns `PIXEL_EDGE_INSET + PIXEL_SEGMENT_PADDING`
//    for the same segment. Before this change both conventions happened
//    to agree (`0.0` either way, no inset or padding existed to diverge
//    on). Not introduced by this PR — the asymmetry is pre-existing
//    backend-testing-helper divergence — but worth flagging so a future
//    change doesn't assume `find_bounds` means the same rect across
//    `GtkDriver`/`MacDriver`/`WinDriver`.
//
// `#[allow(dead_code)]`: see `primitives::form`'s identical note (#808)
// — only *called* once a real pixel backend is compiled in, exercised by
// each backend's own `Backend::draw_status_bar` call site plus this
// module's own `RecordingSurface` tests on every leg that enables one of
// the three cfg'd features.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{
        StatusBar, StatusBarLayout, StatusSegmentMeasure, StatusSegmentSide, PIXEL_EDGE_INSET,
        PIXEL_SEGMENT_PADDING,
    };
    use crate::event::Rect;
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;
    use crate::types::WidgetId;

    /// Minimum gap (surface-native units — px on every pixel backend)
    /// reserved between the left and right segment groups. All three
    /// pre-#860 per-backend copies agreed on `16.0`
    /// (`gtk::status_bar::MIN_GAP_PX`, `macos::status_bar::MIN_GAP_PX`,
    /// `win::status_bar::MIN_GAP_DIP`) — those stay put (each backend's
    /// own no-paint `*_status_bar_layout` twin still uses its own copy
    /// for `Backend::status_bar_layout`); this is `paint`'s independent
    /// copy of the same value.
    const MIN_GAP: f32 = 16.0;

    /// Paint a [`StatusBar`] into `(x, y, width, line_height)` on
    /// `surface`, returning the resolved [`StatusBarLayout`] for the
    /// caller's click dispatch — same contract as
    /// [`crate::Backend::draw_status_bar`]: hit regions are **bar-local**
    /// (relative to `x`/`y`), matching every backend's pre-#860
    /// rasteriser.
    ///
    /// `hovered_id` / `pressed_id` tint the matching clickable segment's
    /// background (lighten on hover, darken on press) — the primitive
    /// itself carries no mouse state.
    ///
    /// A non-positive `width`/`line_height` short-circuits to the
    /// no-paint layout without touching `surface` at all — see this
    /// module's doc, divergence 2, for why this is applied uniformly
    /// (including to GTK, which never had this guard pre-#860).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint(
        bar: &StatusBar,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        x: f32,
        y: f32,
        width: f32,
        line_height: f32,
        hovered_id: Option<&WidgetId>,
        pressed_id: Option<&WidgetId>,
    ) -> StatusBarLayout {
        if width <= 0.0 || line_height <= 0.0 {
            return bar.layout_padded(
                width.max(0.0),
                line_height.max(0.0),
                MIN_GAP,
                PIXEL_EDGE_INSET,
                PIXEL_SEGMENT_PADDING,
                |_| StatusSegmentMeasure::new(0.0),
            );
        }

        let rect = Rect::new(x, y, width, line_height);
        surface.surface_push_clip(rect);

        // Background fill: first segment's bg, else theme bg — matches
        // every pre-#860 per-backend copy.
        let fill = bar
            .left_segments
            .first()
            .or(bar.right_segments.first())
            .map(|s| s.bg)
            .unwrap_or(theme.background);
        surface.surface_fill_rect(rect, fill);

        // See this module's doc, divergence 1: bold-aware measurement is
        // per-backend (`surface_measure_text_styled`'s default/override
        // split), not decided here.
        //
        // `layout_padded` (issue #1155) reserves `PIXEL_EDGE_INSET` at
        // both bar edges and `PIXEL_SEGMENT_PADDING` on both sides of
        // every segment's measured text — VS Code-like outer/per-item
        // padding so the right-most segment doesn't touch the window
        // edge. The text draw below insets by the same padding so the
        // label sits centred in its (wider) segment box rather than
        // flush against its left edge.
        let bar_layout = bar.layout_padded(
            width,
            line_height,
            MIN_GAP,
            PIXEL_EDGE_INSET,
            PIXEL_SEGMENT_PADDING,
            |seg| {
                let (w, _) = surface.surface_measure_text_styled(&seg.text, seg.bold);
                StatusSegmentMeasure::new(w)
            },
        );

        for vs in &bar_layout.visible_segments {
            let seg = match vs.side {
                StatusSegmentSide::Left => &bar.left_segments[vs.segment_idx],
                StatusSegmentSide::Right => &bar.right_segments[vs.segment_idx],
            };
            let seg_rect = Rect::new(
                x + vs.bounds.x,
                y + vs.bounds.y,
                vs.bounds.width,
                vs.bounds.height,
            );

            // Hover/press tint — applied only to interactive segments,
            // matching every pre-#860 per-backend copy.
            // `action_id.is_some_and(...)` is false for both
            // non-clickable segments and segments whose id doesn't match
            // `hovered_id` / `pressed_id`.
            let effective_bg = if seg
                .action_id
                .as_ref()
                .is_some_and(|id| Some(id) == pressed_id)
            {
                seg.bg.darken(0.05)
            } else if seg
                .action_id
                .as_ref()
                .is_some_and(|id| Some(id) == hovered_id)
            {
                seg.bg.lighten(0.05)
            } else {
                seg.bg
            };
            surface.surface_fill_rect(seg_rect, effective_bg);
            // Text sits inset within the padded segment box (issue #1155)
            // rather than flush against `seg_rect`'s left edge — the fill
            // above still covers the full padded bounds, matching the hit
            // region VS Code-style.
            let text_rect = Rect::new(
                seg_rect.x + PIXEL_SEGMENT_PADDING,
                seg_rect.y,
                (seg_rect.width - 2.0 * PIXEL_SEGMENT_PADDING).max(0.0),
                seg_rect.height,
            );
            surface.surface_draw_text_run_styled(
                text_rect, &seg.text, seg.fg, seg.bold, false, false, 1.0,
            );
        }

        surface.surface_pop_clip();
        bar_layout
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::primitives::status_bar::{StatusBarHit, StatusBarSegment};
        use crate::types::Color;
        use crate::Image;

        /// Records every surface verb this primitive's paint uses —
        /// mirrors `primitives::panel`'s `RecordingSurface` test double,
        /// so this test runs on any host without Cairo/Core
        /// Graphics/Direct2D. `surface_measure_text_styled` adds a fixed
        /// bonus width for `bold` text so tests can assert `paint`
        /// actually threads a segment's `bold` flag through to
        /// measurement (divergence 1 above), not just to painting.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            /// `(rect, text, color, bold)` — every text run this test
            /// double sees goes through `surface_draw_text_run_styled`
            /// (`paint` never calls the unstyled `surface_draw_text_run`).
            text_runs: Vec<(Rect, String, Color, bool)>,
            clip_pushes: Vec<Rect>,
            clip_pops: usize,
        }

        const BOLD_BONUS_PX: f32 = 100.0;

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(200.0, 100.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                16.0
            }
            fn surface_char_width(&self) -> f32 {
                8.0
            }
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 8.0, 16.0)
            }
            fn surface_measure_text_styled(&self, text: &str, bold: bool) -> (f32, f32) {
                let (w, h) = self.surface_measure_text(text);
                (w + if bold { BOLD_BONUS_PX } else { 0.0 }, h)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            /// #1073: test-only recorder — `paint` never calls this verb
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
                self.text_runs.push((rect, text.to_string(), color, false));
            }
            #[allow(clippy::too_many_arguments)]
            fn surface_draw_text_run_styled(
                &mut self,
                rect: Rect,
                text: &str,
                color: Color,
                bold: bool,
                _italic: bool,
                _underline: bool,
                _scale_x: f32,
            ) {
                self.text_runs.push((rect, text.to_string(), color, bold));
            }
            fn surface_draw_line(
                &mut self,
                _from: crate::Point,
                _to: crate::Point,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_push_clip(&mut self, rect: Rect) {
                self.clip_pushes.push(rect);
            }
            fn surface_pop_clip(&mut self) {
                self.clip_pops += 1;
            }
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        fn sample_bar() -> StatusBar {
            StatusBar {
                id: WidgetId::new("sb"),
                left_segments: vec![StatusBarSegment {
                    text: "NORMAL".into(),
                    fg: Color::rgb(255, 255, 255),
                    bg: Color::rgb(10, 20, 30),
                    bold: true,
                    action_id: Some(WidgetId::new("sb:mode")),
                }],
                right_segments: vec![StatusBarSegment {
                    text: "Ln 1, Col 1".into(),
                    fg: Color::rgb(255, 255, 255),
                    bg: Color::rgb(40, 50, 60),
                    bold: false,
                    action_id: Some(WidgetId::new("sb:cursor")),
                }],
            }
        }

        #[test]
        fn bar_background_falls_back_to_theme_when_no_segments() {
            let bar = StatusBar {
                id: WidgetId::new("empty"),
                left_segments: vec![],
                right_segments: vec![],
            };
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(
                &bar,
                &mut surface,
                &theme,
                0.0,
                0.0,
                200.0,
                20.0,
                None,
                None,
            );
            assert_eq!(surface.fills[0].1, theme.background);
        }

        #[test]
        fn bar_background_uses_first_segment_bg_when_present() {
            let bar = sample_bar();
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(
                &bar,
                &mut surface,
                &theme,
                0.0,
                0.0,
                200.0,
                20.0,
                None,
                None,
            );
            assert_eq!(surface.fills[0].1, bar.left_segments[0].bg);
        }

        /// Regression for the already-fixed quadraui#791 shape (divergence
        /// 2 above): a degenerate rect must not touch `surface` at all,
        /// on every backend — including GTK, which had no such guard
        /// pre-#860.
        #[test]
        fn zero_width_short_circuits_without_touching_surface() {
            let bar = sample_bar();
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let layout = paint(&bar, &mut surface, &theme, 0.0, 0.0, 0.0, 20.0, None, None);
            assert!(surface.fills.is_empty());
            assert!(surface.text_runs.is_empty());
            assert!(surface.clip_pushes.is_empty());
            assert_eq!(surface.clip_pops, 0);
            // The primitive's own "always keep the last segment" rule
            // still resolves *some* layout even at zero width — it's just
            // never painted.
            assert!(!layout.visible_segments.is_empty());
        }

        #[test]
        fn clip_is_pushed_and_popped_around_the_bar_rect() {
            let bar = sample_bar();
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(
                &bar,
                &mut surface,
                &theme,
                5.0,
                3.0,
                200.0,
                20.0,
                None,
                None,
            );
            assert_eq!(surface.clip_pushes, vec![Rect::new(5.0, 3.0, 200.0, 20.0)]);
            assert_eq!(surface.clip_pops, 1);
        }

        /// Regression for divergence 1 above: `paint` must measure (and
        /// therefore lay out) a bold segment wider than the identical
        /// text at regular weight, proving `seg.bold` actually reaches
        /// `surface_measure_text_styled` rather than the plain
        /// `surface_measure_text`.
        #[test]
        fn bold_segment_measured_wider_than_the_same_text_plain() {
            let mut bar = sample_bar();
            bar.left_segments[0].bold = false;
            let theme = Theme::default();
            let mut plain_surface = RecordingSurface::default();
            let plain_layout = paint(
                &bar,
                &mut plain_surface,
                &theme,
                0.0,
                0.0,
                200.0,
                20.0,
                None,
                None,
            );

            bar.left_segments[0].bold = true;
            let mut bold_surface = RecordingSurface::default();
            let bold_layout = paint(
                &bar,
                &mut bold_surface,
                &theme,
                0.0,
                0.0,
                200.0,
                20.0,
                None,
                None,
            );

            let plain_w = plain_layout.visible_segments[0].bounds.width;
            let bold_w = bold_layout.visible_segments[0].bounds.width;
            assert!(
                (bold_w - plain_w - BOLD_BONUS_PX).abs() < 0.01,
                "bold width {bold_w} should exceed plain width {plain_w} by exactly the bold bonus"
            );
        }

        #[test]
        fn text_drawn_via_styled_run_with_segment_bold_flag() {
            let bar = sample_bar();
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(
                &bar,
                &mut surface,
                &theme,
                0.0,
                0.0,
                200.0,
                20.0,
                None,
                None,
            );

            let (_, _, _, left_bold) = surface
                .text_runs
                .iter()
                .find(|(_, t, _, _)| t == "NORMAL")
                .expect("left segment text drawn");
            assert!(*left_bold, "left segment is bold=true");

            let (_, _, _, right_bold) = surface
                .text_runs
                .iter()
                .find(|(_, t, _, _)| t == "Ln 1, Col 1")
                .expect("right segment text drawn");
            assert!(!*right_bold, "right segment is bold=false");
        }

        #[test]
        fn hover_lightens_and_press_darkens_clickable_segment_bg() {
            let bar = sample_bar();
            let theme = Theme::default();
            let base = bar.left_segments[0].bg;

            let hovered = WidgetId::new("sb:mode");
            let mut hovered_surface = RecordingSurface::default();
            paint(
                &bar,
                &mut hovered_surface,
                &theme,
                0.0,
                0.0,
                200.0,
                20.0,
                Some(&hovered),
                None,
            );
            // fills[0] = bar bg, fills[1] = left "mode" segment.
            assert_eq!(hovered_surface.fills[1].1, base.lighten(0.05));

            let pressed = WidgetId::new("sb:mode");
            let mut pressed_surface = RecordingSurface::default();
            paint(
                &bar,
                &mut pressed_surface,
                &theme,
                0.0,
                0.0,
                200.0,
                20.0,
                None,
                Some(&pressed),
            );
            assert_eq!(pressed_surface.fills[1].1, base.darken(0.05));
        }

        #[test]
        fn paint_and_click_round_trip() {
            let bar = sample_bar();
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            // Wide enough that the bold-inflated left segment
            // (`BOLD_BONUS_PX`) and the right segment don't overlap —
            // this test is about hit-test dispatch, not priority-drop.
            let layout = paint(
                &bar,
                &mut surface,
                &theme,
                0.0,
                0.0,
                400.0,
                20.0,
                None,
                None,
            );

            let mode = layout
                .visible_segments
                .iter()
                .find(|vs| vs.side == StatusSegmentSide::Left)
                .expect("mode segment visible");
            let hit = layout.hit_test(mode.bounds.x + 1.0, mode.bounds.y + 1.0);
            assert_eq!(hit, StatusBarHit::Segment(WidgetId::new("sb:mode")));

            let cursor = layout
                .visible_segments
                .iter()
                .find(|vs| vs.side == StatusSegmentSide::Right)
                .expect("cursor segment visible");
            let hit = layout.hit_test(cursor.bounds.x + 1.0, cursor.bounds.y + 1.0);
            assert_eq!(hit, StatusBarHit::Segment(WidgetId::new("sb:cursor")));
        }

        /// Issue #1155: on every pixel backend (this shared `paint` is
        /// GTK/Win/macOS's rasteriser) the right-most segment must not
        /// touch the bar's own right edge, and the left-most segment
        /// must not touch the left edge — both get `PIXEL_EDGE_INSET`'s
        /// worth of outer margin.
        #[test]
        fn paint_reserves_pixel_edge_inset_at_both_bar_edges() {
            let bar = sample_bar();
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let layout = paint(
                &bar,
                &mut surface,
                &theme,
                0.0,
                0.0,
                400.0,
                20.0,
                None,
                None,
            );

            let left = layout
                .visible_segments
                .iter()
                .find(|vs| vs.side == StatusSegmentSide::Left)
                .expect("left segment visible");
            assert_eq!(
                left.bounds.x, PIXEL_EDGE_INSET,
                "left-most segment should start `PIXEL_EDGE_INSET` in from x=0, \
                 not flush at the bar's own left edge"
            );

            let right = layout
                .visible_segments
                .iter()
                .find(|vs| vs.side == StatusSegmentSide::Right)
                .expect("right segment visible");
            assert_eq!(
                right.bounds.x + right.bounds.width,
                400.0 - PIXEL_EDGE_INSET,
                "right-most segment should end `PIXEL_EDGE_INSET` short of the \
                 bar's own right edge, not touching it"
            );
        }

        /// Issue #1155: the segment's background fill covers the full
        /// padded box (so hover/press tint and hit region agree), but the
        /// text itself is drawn inset by `PIXEL_SEGMENT_PADDING` rather
        /// than flush against the fill's left edge.
        #[test]
        fn paint_insets_text_within_the_padded_segment_box_but_fills_the_whole_box() {
            let bar = sample_bar();
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let layout = paint(
                &bar,
                &mut surface,
                &theme,
                0.0,
                0.0,
                400.0,
                20.0,
                None,
                None,
            );

            let left = layout
                .visible_segments
                .iter()
                .find(|vs| vs.side == StatusSegmentSide::Left)
                .expect("left segment visible");
            let (fill_rect, _) = surface
                .fills
                .iter()
                .find(|(r, _)| *r == left.bounds)
                .expect("segment background filled across its full padded bounds");
            let (text_rect, _, _, _) = surface
                .text_runs
                .iter()
                .find(|(_, t, _, _)| t == "NORMAL")
                .expect("left segment text drawn");
            assert_eq!(
                text_rect.x,
                fill_rect.x + PIXEL_SEGMENT_PADDING,
                "text should be inset by `PIXEL_SEGMENT_PADDING` from the \
                 filled segment box's own left edge"
            );
            assert!(
                text_rect.width < fill_rect.width,
                "text draw rect ({}) should be narrower than the filled \
                 segment box ({}) it sits inside",
                text_rect.width,
                fill_rect.width
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_bar_roundtrip_serde() {
        let bar = StatusBar {
            id: WidgetId::new("editor-status"),
            left_segments: vec![
                StatusBarSegment {
                    text: " NORMAL ".to_string(),
                    fg: Color::rgb(255, 255, 255),
                    bg: Color::rgb(30, 30, 30),
                    bold: true,
                    action_id: None,
                },
                StatusBarSegment {
                    text: " main.rs".to_string(),
                    fg: Color::rgb(200, 200, 200),
                    bg: Color::rgb(30, 30, 30),
                    bold: true,
                    action_id: None,
                },
            ],
            right_segments: vec![
                StatusBarSegment {
                    text: " rust ".to_string(),
                    fg: Color::rgb(200, 200, 200),
                    bg: Color::rgb(30, 30, 30),
                    bold: false,
                    action_id: Some(WidgetId::new("status:change_language")),
                },
                StatusBarSegment {
                    text: " Ln 12, Col 4 ".to_string(),
                    fg: Color::rgb(200, 200, 200),
                    bg: Color::rgb(30, 30, 30),
                    bold: false,
                    action_id: Some(WidgetId::new("status:goto_line")),
                },
            ],
        };
        let json = serde_json::to_string(&bar).unwrap();
        let back: StatusBar = serde_json::from_str(&json).unwrap();
        assert_eq!(bar, back);
    }

    #[test]
    fn status_bar_resolve_click() {
        // Bar width 30: left " LEFT " (6 chars, clickable "left") +
        // right " R " (3 chars, clickable "right") right-aligned at col 27.
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![StatusBarSegment {
                text: " LEFT ".to_string(),
                fg: Color::rgb(0, 0, 0),
                bg: Color::rgb(0, 0, 0),
                bold: false,
                action_id: Some(WidgetId::new("left")),
            }],
            right_segments: vec![StatusBarSegment {
                text: " R ".to_string(),
                fg: Color::rgb(0, 0, 0),
                bg: Color::rgb(0, 0, 0),
                bold: false,
                action_id: Some(WidgetId::new("right")),
            }],
        };
        // Click resolution
        assert_eq!(
            bar.resolve_click(3, 30).as_ref().map(|w| w.as_str()),
            Some("left")
        );
        assert_eq!(
            bar.resolve_click(28, 30).as_ref().map(|w| w.as_str()),
            Some("right")
        );
        assert_eq!(bar.resolve_click(15, 30), None); // gap between segments
    }

    #[test]
    fn status_bar_fit_right_start_chars() {
        let mk = |text: &str, id: &str| StatusBarSegment {
            text: text.to_string(),
            fg: Color::rgb(0, 0, 0),
            bg: Color::rgb(0, 0, 0),
            bold: false,
            action_id: Some(WidgetId::new(id)),
        };
        // Left 5 chars, right = 4 low-priority (lo0..lo3) + cursor (always kept).
        // Right segments total: 3+3+3+3+11 = 23 chars
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![mk(" LEFT", "left")],
            right_segments: vec![
                mk(" a ", "lo0"),
                mk(" b ", "lo1"),
                mk(" c ", "lo2"),
                mk(" d ", "lo3"),
                mk(" Ln 1,Col 1", "cursor"),
            ],
        };

        // Plenty of room (left 5 + gap 2 + right 23 = 30 <= 40) → nothing dropped.
        assert_eq!(bar.fit_right_start_chars(40, 2), 0);

        // Exact fit (30) → still 0 dropped.
        assert_eq!(bar.fit_right_start_chars(30, 2), 0);

        // bar_width 29: need max_right = 29 - 5 - 2 = 22. Total 23 > 22, drop lo0 (3).
        // After dropping lo0, remaining = 20 <= 22, keep rest.
        assert_eq!(bar.fit_right_start_chars(29, 2), 1);

        // bar_width 20: max_right = 13. Must drop lo0(3), lo1(3), lo2(3), lo3(3)
        // → remaining = 11 <= 13. Keep only cursor.
        assert_eq!(bar.fit_right_start_chars(20, 2), 4);

        // Tiny bar: left(5)+gap(2)=7 already >= bar. max_right=0. Even cursor
        // (11) doesn't fit — but we always keep the last segment.
        assert_eq!(bar.fit_right_start_chars(5, 2), 4);

        // Empty right side.
        let empty_right = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![mk(" X", "x")],
            right_segments: vec![],
        };
        assert_eq!(empty_right.fit_right_start_chars(10, 2), 0);
    }

    #[test]
    fn status_bar_fit_right_start_generic_pixel_measurer() {
        // Proves the fit algorithm is unit-agnostic: a backend can supply
        // its own measurer (e.g. Pango pixel widths for GTK) and the same
        // drop-by-priority logic applies. Each char here = 10 "px".
        let mk = |text: &str, id: &str| StatusBarSegment {
            text: text.to_string(),
            fg: Color::rgb(0, 0, 0),
            bg: Color::rgb(0, 0, 0),
            bold: false,
            action_id: Some(WidgetId::new(id)),
        };
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![mk("LL", "left")], // 20 px
            right_segments: vec![
                mk("aaa", "lo"),    // 30 px (lowest priority)
                mk("bbbb", "mid"),  // 40 px
                mk("cursor", "hi"), // 60 px (highest priority)
            ],
        };
        let measure_px = |seg: &StatusBarSegment| seg.text.chars().count() * 10;

        // 200 px: 20 + 16 (gap) + 130 = 166 <= 200, no drop.
        assert_eq!(bar.fit_right_start(200, 16, measure_px), 0);

        // 150 px: 20 + 16 + 130 = 166 > 150. Drop "aaa" (30): 20+16+100=136 <= 150.
        assert_eq!(bar.fit_right_start(150, 16, measure_px), 1);

        // 100 px: drop "aaa" (30) + "bbbb" (40), keep "cursor": 20+16+60=96 <= 100.
        assert_eq!(bar.fit_right_start(100, 16, measure_px), 2);

        // 30 px: even cursor doesn't fit alone, but algorithm always keeps last.
        assert_eq!(bar.fit_right_start(30, 16, measure_px), 2);

        // Bold-aware: a measurer that adds 5 px for bold segments yields a
        // different fit. Verifies the closure can vary by segment style.
        let bold = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![StatusBarSegment {
                text: "BOLD".to_string(),
                fg: Color::rgb(0, 0, 0),
                bg: Color::rgb(0, 0, 0),
                bold: true,
                action_id: None,
            }],
            right_segments: vec![mk("xx", "a"), mk("yy", "b")],
        };
        let measure_with_bold =
            |seg: &StatusBarSegment| seg.text.chars().count() * 10 + if seg.bold { 5 } else { 0 };
        // Left: 4*10 + 5 (bold) = 45. Right total: 20 + 20 = 40. Gap 5.
        // 45 + 5 + 40 = 90 <= 90 → no drop.
        assert_eq!(bold.fit_right_start(90, 5, measure_with_bold), 0);
        // 89: drop one — first ("xx").
        assert_eq!(bold.fit_right_start(89, 5, measure_with_bold), 1);
    }

    // ── D6 StatusBar layout API tests ─────────────────────────────────

    fn make_status_seg(text: &str, id: Option<&str>, bold: bool) -> StatusBarSegment {
        StatusBarSegment {
            text: text.to_string(),
            fg: Color::rgb(255, 255, 255),
            bg: Color::rgb(30, 30, 30),
            bold,
            action_id: id.map(WidgetId::new),
        }
    }

    #[test]
    fn status_bar_layout_empty() {
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![],
            right_segments: vec![],
        };
        let layout = bar.layout(30.0, 1.0, 2.0, |_| StatusSegmentMeasure::new(0.0));
        assert_eq!(layout.visible_segments.len(), 0);
        assert_eq!(layout.hit_regions.len(), 0);
        assert_eq!(layout.resolved_right_start, 0);
        assert_eq!(layout.hit_test(5.0, 0.5), StatusBarHit::Empty);
    }

    #[test]
    fn status_bar_layout_left_only() {
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![
                make_status_seg(" NORMAL ", None, true),
                make_status_seg(" main.rs", Some("filename"), false),
            ],
            right_segments: vec![],
        };
        let layout = bar.layout(50.0, 1.0, 2.0, |seg| {
            StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        });
        assert_eq!(layout.visible_segments.len(), 2);
        assert_eq!(layout.visible_segments[0].bounds.x, 0.0);
        assert_eq!(layout.visible_segments[0].bounds.width, 8.0); // " NORMAL "
        assert_eq!(layout.visible_segments[0].side, StatusSegmentSide::Left);
        assert!(!layout.visible_segments[0].clickable);
        assert_eq!(layout.visible_segments[1].bounds.x, 8.0);
        assert_eq!(layout.visible_segments[1].side, StatusSegmentSide::Left);
        assert!(layout.visible_segments[1].clickable);

        // Click on non-clickable → Empty. Click on clickable → the id.
        assert_eq!(layout.hit_test(3.0, 0.5), StatusBarHit::Empty);
        match layout.hit_test(10.0, 0.5) {
            StatusBarHit::Segment(id) => assert_eq!(id.as_str(), "filename"),
            other => panic!("expected Segment(filename), got {other:?}"),
        }
    }

    #[test]
    fn status_bar_layout_right_aligned() {
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![make_status_seg(" NORMAL", None, true)],
            right_segments: vec![
                make_status_seg(" rust ", Some("lang"), false),
                make_status_seg(" Ln 1,Col 1 ", Some("cursor"), false),
            ],
        };
        // Bar 40 chars. Right segs total 18; left 7; gap min 2. 7+2+18=27<=40.
        // No drop. Right starts at 40 - 18 = 22.
        let layout = bar.layout(40.0, 1.0, 2.0, |seg| {
            StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        });
        assert_eq!(layout.resolved_right_start, 0);
        assert_eq!(layout.visible_segments.len(), 3);
        // Right side starts at bar_width - total_visible_right = 40 - 18 = 22
        let lang = &layout.visible_segments[1];
        assert_eq!(lang.side, StatusSegmentSide::Right);
        assert_eq!(lang.bounds.x, 22.0);
        assert_eq!(lang.bounds.width, 6.0);
        let cursor = &layout.visible_segments[2];
        assert_eq!(cursor.bounds.x, 28.0);

        // Hit-test the right-side cursor segment.
        match layout.hit_test(30.0, 0.5) {
            StatusBarHit::Segment(id) => assert_eq!(id.as_str(), "cursor"),
            other => panic!("expected Segment(cursor), got {other:?}"),
        }
    }

    #[test]
    fn status_bar_layout_priority_drop() {
        // Right segments ordered least-important first. A narrow bar should
        // drop the low-priority ones and preserve the cursor segment.
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![make_status_seg(" LEFT", None, false)],
            right_segments: vec![
                make_status_seg(" a ", Some("lo0"), false),            // 3
                make_status_seg(" b ", Some("lo1"), false),            // 3
                make_status_seg(" c ", Some("lo2"), false),            // 3
                make_status_seg(" Ln 1,Col 1", Some("cursor"), false), // 11
            ],
        };
        // bar=20, left=5, gap=2 → max_right=13. Sum=20 > 13. Drop lo0 (3).
        // Remaining 17 > 13. Drop lo1. Remaining 14 > 13. Drop lo2. Remaining 11 ≤ 13.
        // resolved_right_start = 3 (cursor only).
        let layout = bar.layout(20.0, 1.0, 2.0, |seg| {
            StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        });
        assert_eq!(layout.resolved_right_start, 3);
        // Visible: 1 left + 1 right = 2
        assert_eq!(layout.visible_segments.len(), 2);
        let surviving_right = layout
            .visible_segments
            .iter()
            .find(|v| v.side == StatusSegmentSide::Right)
            .unwrap();
        assert_eq!(surviving_right.segment_idx, 3);
        assert_eq!(surviving_right.bounds.width, 11.0);

        // Hit-test the dropped-segment columns: no action fires.
        assert_eq!(layout.hit_test(7.0, 0.5), StatusBarHit::Empty);
    }

    #[test]
    fn status_bar_layout_pixel_units_fractional() {
        // Native-style measurement: fractional pixel widths, proportional
        // font. Proves the unit-agnostic contract (north-star goal).
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![make_status_seg("NORMAL", None, true)],
            right_segments: vec![make_status_seg("Ln 1,Col 1", Some("cursor"), false)],
        };
        // Non-uniform widths — pretend each char is ~7.3 px average, bold +5.
        let measure = |seg: &StatusBarSegment| {
            let w = seg.text.chars().count() as f32 * 7.3 + if seg.bold { 5.0 } else { 0.0 };
            StatusSegmentMeasure::new(w)
        };
        let layout = bar.layout(400.0, 22.0, 16.0, measure);
        assert_eq!(layout.resolved_right_start, 0);
        assert_eq!(layout.visible_segments.len(), 2);
        assert_eq!(layout.visible_segments[0].side, StatusSegmentSide::Left);
        assert_eq!(layout.visible_segments[0].bounds.x, 0.0);
        assert!((layout.visible_segments[0].bounds.width - (6.0 * 7.3 + 5.0)).abs() < 0.01);
        // Right segment right-aligned.
        let right = &layout.visible_segments[1];
        let right_w = 10.0 * 7.3;
        assert!((right.bounds.x - (400.0 - right_w)).abs() < 0.01);
    }

    #[test]
    fn status_bar_layout_always_keeps_last_right_segment() {
        // Even if the last (highest-priority) segment alone doesn't fit,
        // the layout keeps it rather than rendering an empty right half.
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![make_status_seg("LEFT_MORE", None, false)],
            right_segments: vec![make_status_seg("cursor_info", Some("cursor"), false)],
        };
        // bar=10, left=9, gap=2 → max_right=0 (well, negative → clamped to 0).
        // Single segment, alone overflow → keep it anyway.
        let layout = bar.layout(10.0, 1.0, 2.0, |seg| {
            StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        });
        assert_eq!(layout.resolved_right_start, 0);
        let r = layout
            .visible_segments
            .iter()
            .find(|v| v.side == StatusSegmentSide::Right);
        assert!(
            r.is_some(),
            "last segment should survive even when too wide"
        );
    }

    // ── issue #1155: outer edge inset + per-segment padding ────────────

    #[test]
    fn layout_padded_insets_left_segment_from_the_bar_edge() {
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![make_status_seg("NORMAL", Some("mode"), false)],
            right_segments: vec![],
        };
        let layout = bar.layout_padded(200.0, 20.0, 16.0, 10.0, 5.0, |seg| {
            StatusSegmentMeasure::new(seg.text.chars().count() as f32 * 8.0)
        });
        let mode = &layout.visible_segments[0];
        // Starts `edge_inset` in from the bar's own left edge, not at 0.0.
        assert_eq!(mode.bounds.x, 10.0);
        // Width grows by `2 * segment_padding` over the raw measured width.
        assert_eq!(mode.bounds.width, 6.0 * 8.0 + 2.0 * 5.0);
    }

    #[test]
    fn layout_padded_insets_right_segment_from_the_bar_edge() {
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![],
            right_segments: vec![make_status_seg("Ln 1, Col 1", Some("cursor"), false)],
        };
        let measured_w = "Ln 1, Col 1".chars().count() as f32 * 8.0;
        let layout = bar.layout_padded(200.0, 20.0, 16.0, 10.0, 5.0, |seg| {
            StatusSegmentMeasure::new(seg.text.chars().count() as f32 * 8.0)
        });
        let cursor = &layout.visible_segments[0];
        let padded_w = measured_w + 2.0 * 5.0;
        // Right-aligned `edge_inset` in from the bar's own right edge —
        // the segment's right edge must land at `bar_width - edge_inset`,
        // not flush at `bar_width` (the vimcode "Ln 59, Col 2 touches the
        // window edge" regression this issue reports).
        assert_eq!(cursor.bounds.x + cursor.bounds.width, 200.0 - 10.0);
        assert_eq!(cursor.bounds.width, padded_w);
    }

    #[test]
    fn layout_padded_hit_region_covers_the_padded_bounds_not_just_the_text() {
        // "Hit-test regions must cover the padded item" per this issue's
        // fix description — a click in the padding (not on the glyphs
        // themselves) must still resolve to the segment.
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![make_status_seg("X", Some("mode"), false)],
            right_segments: vec![],
        };
        let layout = bar.layout_padded(200.0, 20.0, 16.0, 10.0, 5.0, |seg| {
            StatusSegmentMeasure::new(seg.text.chars().count() as f32 * 8.0)
        });
        // bounds.x = 10.0 (edge_inset), width = 8.0 + 10.0 (padding) = 18.0.
        // Click right at the padded left edge (still inside the padding,
        // left of where the glyph itself starts).
        match layout.hit_test(11.0, 5.0) {
            StatusBarHit::Segment(id) => assert_eq!(id.as_str(), "mode"),
            other => panic!("expected the padding to be part of the hit region, got {other:?}"),
        }
    }

    #[test]
    fn layout_with_zero_inset_and_padding_matches_plain_layout() {
        // `layout` is documented as `layout_padded` with both new
        // parameters at `0.0` — pin that equivalence directly so a future
        // change to one can't silently diverge from the other.
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![make_status_seg(" NORMAL ", Some("mode"), true)],
            right_segments: vec![make_status_seg(" Ln 1, Col 1 ", Some("cursor"), false)],
        };
        let measure = |seg: &StatusBarSegment| StatusSegmentMeasure::new(seg.text.len() as f32);
        let via_layout = bar.layout(80.0, 1.0, 2.0, measure);
        let via_padded = bar.layout_padded(80.0, 1.0, 2.0, 0.0, 0.0, measure);
        assert_eq!(via_layout, via_padded);
    }
}
