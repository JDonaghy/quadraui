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
//! segments on narrow widths. A purely declarative paint
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
//! **`left_segments` is the mirror image: most-important first,
//! least-important last.** Layout walks it in index order, giving each
//! segment its natural width until the space the right group leaves runs
//! out; the segment that crosses that edge is clamped to the cells that
//! remain (backends ellipsis-truncate its text to `bounds.width`) and a
//! segment left with no cells at all is dropped whole. So a left segment
//! that may grow without bound — a path, a message — belongs last, where
//! it is squeezed instead of squeezing the mode indicator ahead of it.
//!
//! Skipping step 1 + 2 makes narrow bars look like `BARMODE filenameSpaces:`
//! (touching, no gap) or worse (right segments overdrawing left in TUI).
//!
//! Skipping step 3 means clicking blank space at the left of the right
//! group can trigger random toggles — confusing and undebuggable.

use crate::compose::Handled;
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
    /// Left-aligned segments, **most-important first**: a narrow bar
    /// squeezes and then drops from the end. See the module header.
    pub left_segments: Vec<StatusBarSegment>,
    /// Right-aligned segments, **most-important last**: a narrow bar
    /// drops from the front. See the module header.
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
    fn hit_regions_impl(&self, bar_width: usize) -> Vec<StatusBarHitRegion> {
        let mut regions = Vec::new();
        let mut col: u16 = 0;
        for seg in &self.left_segments {
            let w = seg.text.chars().count() as u16;
            if let Some(id) = &seg.action_id {
                regions.push(StatusBarHitRegion {
                    col,
                    width: w,
                    id: *id,
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
                    id: *id,
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
/// segments, VS Code-like. Pixel backends (GTK/Win/macOS)
/// pass this to [`StatusBar::layout_padded`]. TUI's char-cell bar has no
/// use for a sub-cell inset and keeps calling plain [`StatusBar::layout`]
/// (equivalent to `layout_padded` with `edge_inset = 0.0`).
pub const PIXEL_EDGE_INSET: f32 = 10.0;

/// Per-segment horizontal padding (surface-native units — px on every
/// pixel backend), added to *both* sides of each segment's measured text
/// width, VS Code-like. See [`PIXEL_EDGE_INSET`]'s doc.
pub const PIXEL_SEGMENT_PADDING: f32 = 5.0;

/// Minimum gap (surface-native units — px on every pixel backend)
/// reserved between the left and right segment groups. See
/// [`PIXEL_EDGE_INSET`]'s doc; [`MIN_GAP_CELLS`] is the cell-grid
/// counterpart.
pub const PIXEL_MIN_GAP: f32 = 16.0;

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
    /// This is what TUI's char-cell bar wants: a monospace
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
    /// per-segment horizontal padding — what pixel backends
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

        // ── Left segments: measure first, resolve geometry after the
        // right side is known ──────────────────────────────────────────
        //
        // `left_w` (the *natural*, unsqueezed total) feeds the right
        // side's own budget below. Left's own geometry is resolved
        // afterward, once `total_visible` (the right side's resolved
        // width) is known, so a huge left segment stays within its own
        // reserved space rather than painting into the right group.
        let left_widths: Vec<f32> = self
            .left_segments
            .iter()
            .map(|s| measure(s).width + pad)
            .collect();
        let total_left: f32 = left_widths.iter().sum();
        let left_w = edge_inset + total_left;

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

        let visible_right_widths = &right_widths[resolved_right_start..];
        let total_visible: f32 = visible_right_widths.iter().sum();

        // ── Left segments: squeeze, then drop, inside the space the
        // (now-resolved) right side leaves ─────────────────────────────
        //
        // Convention, mirroring `right_segments`: `left_segments` is
        // ordered most-important first (e.g. the mode indicator), least-
        // important last. Each segment is laid out at its natural width
        // in that order until the budget runs low; the segment that
        // crosses the budget edge keeps whatever cells remain (paint time
        // ellipsis-truncates its text to that clamped `bounds.width`) and
        // anything after it, having no room at all, is dropped whole.
        //
        // Squeezing before dropping is deliberate: unlike the right
        // side's short atomic indicators, a left segment is typically a
        // message whose head is the informative part, so a truncated head
        // beats an absent segment. Index 0 therefore always survives, and
        // a long trailing message is only dropped once the segments ahead
        // of it have eaten the whole budget.
        //
        // `min_gap` separates the two *groups*, so it is only reserved
        // when the right group actually has something in it — otherwise
        // the left side owns the bar right up to the edge inset.
        let right_reserved = if total_visible > 0.0 {
            total_visible + min_gap
        } else {
            0.0
        };
        let left_budget = (bar_width - 2.0 * edge_inset - right_reserved).max(0.0);

        // ── Left segments, left-to-right from the inset edge ───────────
        let mut cursor = edge_inset;
        for (i, seg) in self.left_segments.iter().enumerate() {
            let remaining = (left_budget - (cursor - edge_inset)).max(0.0);
            // No cells left at all: this segment and every lower-priority
            // one after it drop out entirely.
            if remaining <= 0.0 && i > 0 {
                break;
            }
            // Only clamp when the natural width genuinely overflows —
            // `f32::EPSILON` of float drift must not trip paint-time
            // ellipsis truncation on a segment that in fact fits.
            let w = if left_widths[i] <= remaining + f32::EPSILON {
                left_widths[i]
            } else {
                remaining
            };
            let bounds = Rect::new(cursor, 0.0, w, bar_height);
            let clickable = seg.action_id.is_some();
            visible_segments.push(VisibleStatusSegment {
                segment_idx: i,
                side: StatusSegmentSide::Left,
                bounds,
                clickable,
            });
            if let Some(id) = &seg.action_id {
                hit_regions.push((bounds, StatusBarHit::Segment(*id)));
            }
            cursor += w;
        }

        // Right segments right-aligned inside `bar_width`, inset from the
        // bar's own right edge by `edge_inset`. Rendered in the natural
        // `right_segments[start..]` order; first visible segment is
        // leftmost of the right group.
        let visible_right = &self.right_segments[resolved_right_start..];
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
                hit_regions.push((bounds, StatusBarHit::Segment(*id)));
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

/// Minimum gap, in character cells, reserved between the left and right
/// segment groups on a cell grid — the cell-unit counterpart of
/// `native_surface_paint`'s own pixel `MIN_GAP`. Both exist because
/// [`StatusBar::layout`]'s `min_gap` is expressed in the caller's own
/// unit, so each unit system names its own value.
pub const MIN_GAP_CELLS: f32 = 2.0;

/// The cell-units [`StatusBarLayout`] every cell-grid caller shares: one
/// cell per character, [`MIN_GAP_CELLS`] between the two groups, and a
/// single-row bar.
///
/// No edge inset and no per-segment padding — a monospace grid has no
/// sub-cell space to spend on either, and cell-grid callers already
/// reserve visual breathing room with literal space characters inside
/// segment text (see [`StatusBar::layout`]).
pub fn cell_layout(bar: &StatusBar, area: Rect) -> StatusBarLayout {
    bar.layout(area.width, 1.0, MIN_GAP_CELLS, |seg| {
        StatusSegmentMeasure::new(seg.text.chars().count() as f32)
    })
}

/// Cell-grid rasteriser for [`StatusBar`], written against
/// [`crate::CellSurface`]'s verbs.
///
/// Consumes a pre-resolved [`StatusBarLayout`] (built by
/// [`cell_layout`], or by any caller with its own cell-width measurer)
/// and paints the resolved `visible_segments`. No layout decisions live
/// here; any policy change (priority drop, gap rules, …) belongs in
/// [`StatusBar::layout_padded`]. The one adjustment made at paint time is
/// ellipsis-truncating a segment's text to the `bounds.width` cells the
/// layout already resolved for it — a rendering detail, not a layout
/// decision, since the width itself came from the layout.
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::{StatusBar, StatusBarLayout, StatusSegmentSide};
    use crate::cell_surface::{CellStyle, CellSurface};
    use crate::theme::Theme;
    use crate::types::{Color, WidgetId};
    use crate::Rect;

    /// A foreground/background pair plus the one modifier bit a segment
    /// can carry — the only [`CellStyle`] shape this painter builds.
    fn ink(fg: Color, bg: Color, bold: bool) -> CellStyle {
        CellStyle {
            fg: Some(fg),
            bg: Some(bg),
            bold,
            ..CellStyle::default()
        }
    }

    /// Paint `bar` at `layout` into `area`'s first row on `surface`, and
    /// hand back the layout the caller's click dispatch should use — the
    /// same **bar-local** hit regions `layout` already carries (relative
    /// to `area.x`/`area.y`).
    ///
    /// The bar is washed with the first segment's `bg` (so it looks
    /// continuous even where a gap separates the two halves), falling
    /// back to [`Theme::background`] for a bar with no segments at all.
    /// Each visible segment's text is painted at the `bounds.x` the
    /// layout assigned, in the segment's own `fg`/`bg`, with `bold`
    /// honoured.
    ///
    /// `hovered_id`/`pressed_id` tint the matching *clickable* segment's
    /// background (lighten on hover, darken on press) — the descriptor
    /// itself carries no mouse state.
    ///
    /// A degenerate `area` short-circuits to the no-paint layout without
    /// touching `surface`.
    pub(crate) fn paint(
        bar: &StatusBar,
        layout: &StatusBarLayout,
        area: Rect,
        surface: &mut dyn CellSurface,
        theme: &Theme,
        hovered_id: Option<&WidgetId>,
        pressed_id: Option<&WidgetId>,
    ) -> StatusBarLayout {
        if area.width < 1.0 || area.height < 1.0 {
            return StatusBarLayout {
                bar_width: 0.0,
                bar_height: 0.0,
                visible_segments: Vec::new(),
                hit_regions: Vec::new(),
                resolved_right_start: 0,
            };
        }

        let x0 = area.x.max(0.0).round() as u16;
        let y = area.y.max(0.0).round() as u16;
        let width = area.width.round() as u16;
        let bar_end = x0.saturating_add(width);

        let fill = bar
            .left_segments
            .first()
            .or(bar.right_segments.first())
            .map(|s| s.bg)
            .unwrap_or(theme.background);
        surface.fill(
            Rect::new(x0 as f32, y as f32, width as f32, 1.0),
            ink(fill, fill, false),
        );

        for vs in &layout.visible_segments {
            let seg = match vs.side {
                StatusSegmentSide::Left => &bar.left_segments[vs.segment_idx],
                StatusSegmentSide::Right => &bar.right_segments[vs.segment_idx],
            };
            // Hover/press tint — applied only to interactive segments,
            // press winning over hover.
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
            let start_x = x0.saturating_add(vs.bounds.x.max(0.0).round() as u16);
            if start_x >= bar_end {
                continue;
            }
            // `vs.bounds.width` is already the priority-drop-resolved
            // budget (see `StatusBar::layout_padded`) in the same
            // char-count unit this painter measures in — ellipsis-truncate
            // to it so an over-long segment (most often the always-kept
            // first left segment) never paints past its own reserved cells
            // into a neighbouring segment. A segment that overflows only by
            // the trailing space a consumer padded it with is clipped
            // instead, keeping the character the `…` would have eaten.
            let paint_text =
                crate::text_util::truncate_padded_with_ellipsis(&seg.text, vs.bounds.width, |s| {
                    s.chars().count() as f32
                });
            surface.put_str_clipped(
                start_x,
                y,
                &paint_text,
                bar_end - start_x,
                ink(seg.fg, effective_bg, seg.bold),
            );
        }

        layout.clone()
    }
}

// Shared implementation for painting status bars across GTK, Windows,
// and macOS. The widget's own `Primitive::paint` calls [`paint`] with each
// backend's native [`PaintSurface`], measuring and rendering text with backend-
// specific overrides as needed.
//
// Bold-aware measurement: GTK and Windows measure segment width against
// the segment's `bold` flag; macOS measures all segments at non-bold
// weight. The shared `paint` uses [`PaintSurface::surface_measure_text_styled`]
// which defaults to non-bold (macOS behaviour); GTK and Windows override
// it to preserve their bold measurement.
//
// Zero-size guard: a non-positive `width` or `line_height` short-
// circuits without touching the surface.
//
// `#[allow(dead_code)]`: `paint` is only called when a pixel backend
// (GTK, Windows, or macOS) is compiled in.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{
        StatusBar, StatusBarLayout, StatusSegmentMeasure, StatusSegmentSide, PIXEL_EDGE_INSET,
        PIXEL_MIN_GAP as MIN_GAP, PIXEL_SEGMENT_PADDING,
    };
    use crate::event::Rect;
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;
    use crate::types::WidgetId;

    /// Sub-pixel tolerance on the text-fits check: far below anything
    /// visible, far above f32 round-off on a status-bar-sized width.
    const TEXT_FIT_SLACK_PX: f32 = 0.5;

    /// Paint a [`StatusBar`] into `(x, y, width, line_height)` on
    /// `surface`, returning the resolved [`StatusBarLayout`] for the
    /// caller's click dispatch: hit regions are **bar-local** (relative to
    /// `x`/`y`).
    ///
    /// `hovered_id` / `pressed_id` tint the matching clickable segment's
    /// background (lighten on hover, darken on press) — the primitive
    /// itself carries no mouse state.
    ///
    /// A non-positive `width`/`line_height` short-circuits to the
    /// no-paint layout without touching `surface` at all.
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

        // Background fill: first segment's bg, else theme bg.
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
        // `layout_padded` reserves `PIXEL_EDGE_INSET` at
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

            // Hover/press tint — applied only to interactive segments.
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
            // Text sits inset within the padded segment box
            // rather than flush against `seg_rect`'s left edge — the fill
            // above still covers the full padded bounds, matching the hit
            // region VS Code-style.
            //
            // Vertically centred within `seg_rect.height`, which can be
            // much taller than the text's own measured height on a pixel
            // backend whose bar is sized off `Style::status_bar_height`
            // rather than the chrome font's line height (mirrors the
            // `DataTable` header title centring).
            let (_, th) = surface.surface_measure_text_styled(&seg.text, seg.bold);
            let text_avail = (seg_rect.width - 2.0 * PIXEL_SEGMENT_PADDING).max(0.0);
            // `text_avail` is `(measured + 2 * padding) - 2 * padding`,
            // which f32 can round a hair below `measured` for fractional
            // pixel widths. The slack keeps a segment the layout gave its
            // natural width from being ellipsized by that round-off.
            let fit_avail = text_avail + TEXT_FIT_SLACK_PX;
            // The layout clamps a segment's `bounds.width` below its
            // natural width when it crosses the left-side budget edge
            // (see `layout_padded`) — ellipsis-truncate the text to fit
            // that clamp rather than painting it at full width over the
            // neighbouring segment. A segment that overflows only by the
            // trailing space a consumer padded it with is clipped
            // instead, keeping the character the `…` would have eaten.
            let bold = seg.bold;
            let paint_text =
                crate::text_util::truncate_padded_with_ellipsis(&seg.text, fit_avail, |s| {
                    surface.surface_measure_text_styled(s, bold).0
                });
            let text_y = seg_rect.y + (seg_rect.height - th).max(0.0) / 2.0;
            let text_rect = Rect::new(
                seg_rect.x + PIXEL_SEGMENT_PADDING,
                text_y,
                text_avail,
                th.max(1.0),
            );
            surface.surface_draw_text_run_styled(
                text_rect,
                &paint_text,
                seg.fg,
                seg.bold,
                false,
                false,
                1.0,
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
            /// Per-char width override; `0.0` means the default `8.0`.
            char_px: f32,
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
                let char_px = if self.char_px > 0.0 {
                    self.char_px
                } else {
                    8.0
                };
                (text.chars().count() as f32 * char_px, 16.0)
            }
            fn surface_measure_text_styled(&self, text: &str, bold: bool) -> (f32, f32) {
                let (w, h) = self.surface_measure_text(text);
                (w + if bold { BOLD_BONUS_PX } else { 0.0 }, h)
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

        /// A degenerate rect must not touch `surface` at all.
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
            // Wide enough that neither weight hits the left-side budget
            // clamp — this test is about the bold/plain width delta, not
            // about overflow handling.
            let mut plain_surface = RecordingSurface::default();
            let plain_layout = paint(
                &bar,
                &mut plain_surface,
                &theme,
                0.0,
                0.0,
                1000.0,
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
                1000.0,
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
            // Wide enough that `RecordingSurface`'s exaggerated
            // `BOLD_BONUS_PX` test fixture doesn't trip the left-side
            // overflow clamp this test isn't about.
            paint(
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

        /// A fractional measured width (`NORMAL` at 30.03px) round-trips
        /// through `+ 2 * padding` / `- 2 * padding` to just under itself
        /// in f32; a segment with ample room must still paint unclipped.
        #[test]
        fn fractional_width_segment_that_fits_paints_its_full_text() {
            let mut bar = sample_bar();
            bar.left_segments[0].text = "NORMAL".into();
            bar.left_segments[0].bold = false;
            bar.right_segments.clear();
            let theme = Theme::default();
            let mut surface = RecordingSurface {
                char_px: 5.005,
                ..Default::default()
            };
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
            assert!(
                surface.text_runs.iter().any(|(_, t, _, _)| t == "NORMAL"),
                "a segment with room to spare must paint whole, got {:?}",
                surface.text_runs
            );
        }

        /// Covers the gallery's dirty status-bar scenario: a
        /// single left segment too wide for the bar paints an ellipsis-
        /// truncated run, not its full text, so it never bleeds into the
        /// right group's painted segment.
        #[test]
        fn overlong_left_segment_paints_an_ellipsis_not_its_full_text() {
            let mut bar = sample_bar();
            bar.left_segments[0].text = "x".repeat(60);
            // Plain weight — `RecordingSurface`'s `BOLD_BONUS_PX` test
            // fixture is tuned for the bold-measurement test above, not
            // this one; on top of a 60-char overflow it would eat the
            // entire remaining budget before even a bare ellipsis fits.
            bar.left_segments[0].bold = false;
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

            let left_run = surface
                .text_runs
                .iter()
                .find(|(_, t, _, _)| t.starts_with('x'))
                .expect("left segment text must still paint");
            assert!(
                left_run.1.ends_with('…') && left_run.1.len() < 60,
                "overlong left segment must end in an ellipsis, got {:?}",
                left_run.1
            );

            // The right-side cursor segment must still paint, undisturbed,
            // past where the clamped left run ends.
            let right_run = surface
                .text_runs
                .iter()
                .find(|(_, t, _, _)| t == "Ln 1, Col 1")
                .expect("right segment must still paint");
            assert!(right_run.0.x >= left_run.0.x + left_run.0.width - 0.01);
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

        /// [`crate::primitive::Primitive::paint`] reads the hovered and
        /// pressed segment from its [`crate::PaintEnv`], addressed by the
        /// segment's index in `visible_segments`.
        #[test]
        fn primitive_paint_tints_the_segment_its_env_names() {
            use crate::interactive::SubTarget;
            use crate::primitive::{LayoutCtx, Primitive};

            let bar = sample_bar();
            let theme = Theme::default();
            let style = crate::Style::default();
            let base = bar.left_segments[0].bg;
            let layout = Primitive::layout(
                &bar,
                &LayoutCtx::new(16.0, 8.0, false),
                Rect::new(0.0, 0.0, 400.0, 20.0),
            );
            let mode = layout
                .layout
                .visible_segments
                .iter()
                .position(|vs| vs.side == StatusSegmentSide::Left && vs.segment_idx == 0)
                .expect("the mode segment is visible");
            let target = Some((bar.id, Some(SubTarget(mode as u64))));
            let mode_fill = |interaction: &crate::InteractionState| {
                let mut surface = RecordingSurface::default();
                let env = crate::PaintEnv::new(&theme, &style).with_interaction(interaction);
                Primitive::paint(&bar, &layout, &mut surface, &env);
                surface.fills[1].1
            };

            assert_eq!(mode_fill(&crate::InteractionState::new()), base);
            assert_eq!(
                mode_fill(&crate::InteractionState::from_targets(target, None)),
                base.lighten(0.05)
            );
            assert_eq!(
                mode_fill(&crate::InteractionState::from_targets(target, target)),
                base.darken(0.05)
            );
            // A press the pointer has dragged off paints at rest.
            assert_eq!(
                mode_fill(&crate::InteractionState::from_targets(None, target)),
                base
            );
        }

        #[test]
        fn paint_and_click_round_trip() {
            let bar = sample_bar();
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            // Wide enough that the bold-inflated left segment
            // (`BOLD_BONUS_PX`) and the right segment don't overlap —
            // this test is about hit-test dispatch, not overflow policy.
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

        /// On every pixel backend, the right-most segment must not touch the
        /// bar's own right edge, and the left-most segment must not touch the
        /// left edge — both get `PIXEL_EDGE_INSET`'s worth of outer margin.
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

        /// The segment's background fill covers the full padded box (so
        /// hover/press tint and hit region agree), but the text itself is
        /// drawn inset by `PIXEL_SEGMENT_PADDING` rather than flush against
        /// the fill's left edge.
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

        /// Regression: a bar taller than the segment text's own measured
        /// height (e.g. `Style::status_bar_height`'s 22px or the sidebar
        /// header's `tab_height`-driven 35px against ~16px text) must
        /// centre the text vertically within the bar, not anchor it to
        /// the bar's own top edge.
        #[test]
        fn text_is_vertically_centred_within_a_bar_taller_than_the_text() {
            let bar = sample_bar();
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let bar_height = 35.0;
            paint(
                &bar,
                &mut surface,
                &theme,
                0.0,
                0.0,
                400.0,
                bar_height,
                None,
                None,
            );

            // `RecordingSurface::surface_measure_text` always reports a
            // text height of 16.0 regardless of content or weight.
            let expected_text_h = 16.0;
            let expected_y = (bar_height - expected_text_h) / 2.0;

            let (text_rect, _, _, _) = surface
                .text_runs
                .iter()
                .find(|(_, t, _, _)| t == "NORMAL")
                .expect("left segment text drawn");
            assert_eq!(
                text_rect.y, expected_y,
                "left segment text should be centred within the {bar_height}px bar, \
                 not flush against its top edge"
            );

            let (text_rect, _, _, _) = surface
                .text_runs
                .iter()
                .find(|(_, t, _, _)| t == "Ln 1, Col 1")
                .expect("right segment text drawn");
            assert_eq!(
                text_rect.y, expected_y,
                "right segment text should be centred within the {bar_height}px bar too"
            );
        }
    }
}

// ── Widget contract ─────────────────────────────────────────────────────────

/// [`StatusBar`]'s fully-resolved widget-contract layout: the absolute
/// area the bar was laid out into, plus the [`StatusBarLayout`] resolved
/// inside it.
///
/// `layout`'s rects are **bar-local** — [`StatusBarLayout::hit_test`] has
/// always taken coordinates relative to the bar's own origin — so
/// [`Self::hit_test`] takes bar-local coordinates too, which is already
/// the space [`crate::interactive::RoutedEvent::Pointer`]'s `local` field
/// and [`crate::interactive::Interactive::sub_regions`] answer in.
///
/// `#[non_exhaustive]`: produced by
/// [`Primitive::layout`](crate::primitive::Primitive::layout), never built
/// by a consumer via struct literal.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct StatusBarPrimitiveLayout {
    /// The rect [`Primitive::layout`](crate::primitive::Primitive::layout)
    /// received — the bar's own absolute area.
    pub area: Rect,
    /// The bar's resolved layout, in bar-local coordinates.
    pub layout: StatusBarLayout,
}

impl StatusBarPrimitiveLayout {
    /// Classify a bar-local point against this bar — see
    /// [`StatusBarLayout::hit_test`].
    pub fn hit_test(&self, x: f32, y: f32) -> StatusBarHit {
        self.layout.hit_test(x, y)
    }
}

/// [`crate::interactive::Interactive::State`] for [`StatusBar`]: which
/// clickable segment the pointer is over, and which one it is pressing.
///
/// Keyed by [`StatusBarSegment::action_id`] rather than by segment index
/// because that id is what the bar's own hit test resolves and what
/// [`StatusBarEvent::SegmentClicked`] carries — an index would have to be
/// re-resolved against a layout that may have dropped segments since.
/// That is also why this type implements no
/// [`crate::primitive::PaintState`]: its hover/press targets are ids, not
/// the row indices that trait's `hovered_idx`/`pressed_idx` describe.
///
/// `#[non_exhaustive]`: a future field add here is additive.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatusBarState {
    /// The clickable segment under the pointer, if any.
    pub hovered: Option<WidgetId>,
    /// The clickable segment a left button is held down on, if any.
    pub pressed: Option<WidgetId>,
}

/// What releasing the pointer resolved to — see
/// [`StatusBarState::release`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusBarRelease {
    /// The release landed on the same segment the press armed: a click on
    /// that segment.
    Clicked(WidgetId),
    /// A press was armed, but the release landed somewhere else — the
    /// press is forgotten and nothing is activated, matching every
    /// desktop button.
    Dropped,
    /// No press was armed, so the release resolved nothing at all.
    Idle,
}

impl StatusBarState {
    /// Record which clickable segment the pointer is over, reporting
    /// whether that changed — callers turn `true` into a repaint.
    pub fn hover(&mut self, hovered: Option<WidgetId>) -> bool {
        if self.hovered == hovered {
            return false;
        }
        self.hovered = hovered;
        true
    }

    /// Arm `hit` as the pressed segment, reporting whether the press
    /// landed on a clickable segment at all. A press also pulls hover
    /// onto the same segment, so the pressed tint doesn't depend on a
    /// hover event having arrived first.
    pub fn press(&mut self, hit: Option<WidgetId>) -> bool {
        self.pressed = hit;
        if hit.is_some() {
            self.hovered = hit;
            true
        } else {
            false
        }
    }

    /// Resolve a release over `hit` against whatever [`Self::press`]
    /// armed, clearing the press either way.
    pub fn release(&mut self, hit: Option<WidgetId>) -> StatusBarRelease {
        match self.pressed.take() {
            Some(pressed) if hit == Some(pressed) => StatusBarRelease::Clicked(pressed),
            Some(_) => StatusBarRelease::Dropped,
            None => StatusBarRelease::Idle,
        }
    }
}

/// What a [`StatusBar`]'s own [`crate::interactive::Interactive::handle`]
/// resolves one gesture to.
///
/// Named for the segment it carries rather than for the bar, so it reads
/// unambiguously alongside [`crate::compose::StatusBarAction`] — the
/// pre-widget-contract hover/press tracker's own outcome enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusBarSegmentAction {
    /// A clickable segment was activated: the pointer went down and came
    /// back up on the same segment. Carries that segment's
    /// [`StatusBarSegment::action_id`].
    SegmentClicked(WidgetId),
}

impl crate::interactive::WidgetAction for StatusBarSegmentAction {
    /// Rides [`crate::UiEvent::StatusBar`] under the bar's own id,
    /// carrying [`StatusBarEvent::SegmentClicked`] — the one channel every
    /// existing status-bar host already routes a segment click through.
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        match self {
            StatusBarSegmentAction::SegmentClicked(segment) => {
                crate::UiEvent::StatusBar(id, StatusBarEvent::SegmentClicked { id: segment })
            }
        }
    }
}

impl crate::primitive::Primitive for StatusBar {
    type Layout = StatusBarPrimitiveLayout;
    type Event = StatusBarHit;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::StatusBar(self.clone())
    }

    /// A status bar stretches to its container's width and is one row
    /// tall: one cell on a cell grid, [`crate::Style::status_bar_height`]
    /// on a pixel surface.
    fn measure(
        &self,
        ctx: &crate::primitive::LayoutCtx,
        _avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        let height = match ctx.units() {
            crate::primitive::Units::Cells => 1.0,
            _ => ctx.style().status_bar_height,
        };
        crate::NaturalSize::new(None, Some(height))
    }

    /// On a cell grid the bar is measured one cell per character by
    /// [`cell_layout`]; on a pixel surface segment widths come from the
    /// calling backend's own [`crate::Backend::measure_text`] (through
    /// [`crate::primitive::LayoutCtx::text_width`], in
    /// [`crate::FontRole::ChromeBold`] for a bold segment so its box is as
    /// wide as the bold text the painter draws) and the bar reserves
    /// [`PIXEL_EDGE_INSET`]/[`PIXEL_SEGMENT_PADDING`], the same insets
    /// every pixel rasteriser paints with.
    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> StatusBarPrimitiveLayout {
        let layout = match ctx.units() {
            crate::primitive::Units::Cells => cell_layout(self, rect),
            _ => self.layout_padded(
                rect.width,
                rect.height,
                PIXEL_MIN_GAP,
                PIXEL_EDGE_INSET,
                PIXEL_SEGMENT_PADDING,
                |seg| {
                    let role = if seg.bold {
                        crate::FontRole::ChromeBold
                    } else {
                        crate::FontRole::Chrome
                    };
                    StatusSegmentMeasure::new(ctx.text_width(&seg.text, role))
                },
            ),
        };
        StatusBarPrimitiveLayout { area: rect, layout }
    }

    /// The shared pixel painter, at the area [`Self::layout`] resolved,
    /// tinting the clickable segment `env` reports hovered or pressed.
    fn paint(
        &self,
        layout: &StatusBarPrimitiveLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        let style = env.style();
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            let _ = style;
            let (hovered, pressed) = self.interaction_ids(&layout.layout, env);
            native_surface_paint::paint(
                self,
                surface,
                theme,
                layout.area.x,
                layout.area.y,
                layout.area.width,
                layout.area.height,
                hovered.as_ref(),
                pressed.as_ref(),
            );
        }
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        {
            let _ = (layout, surface, theme, style);
        }
    }

    /// [`cell_paint::paint`] over `cells`, tinting the clickable segment
    /// `env` reports hovered or pressed.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &StatusBarPrimitiveLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        let area = Rect::new(
            layout.area.x.max(0.0).round(),
            layout.area.y.max(0.0).round(),
            layout.area.width.max(0.0).round(),
            layout.area.height.max(0.0).round(),
        );
        let (hovered, pressed) = self.interaction_ids(&layout.layout, env);
        cell_paint::paint(
            self,
            &layout.layout,
            area,
            cells,
            theme,
            hovered.as_ref(),
            pressed.as_ref(),
        );
    }
}

impl StatusBar {
    /// The `action_id`s of the segments `env` reports hovered and pressed,
    /// decoding each [`crate::interactive::SubTarget`] as an index into
    /// `layout.visible_segments` — the index space
    /// [`crate::interactive::Interactive::sub_regions`] publishes.
    #[cfg_attr(
        not(any(
            feature = "tui",
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )),
        allow(dead_code)
    )]
    fn interaction_ids(
        &self,
        layout: &StatusBarLayout,
        env: &crate::PaintEnv,
    ) -> (Option<WidgetId>, Option<WidgetId>) {
        let action = |sub: Option<crate::interactive::SubTarget>| {
            let vs = layout.visible_segments.get(usize::try_from(sub?.0).ok()?)?;
            let seg = match vs.side {
                StatusSegmentSide::Left => self.left_segments.get(vs.segment_idx),
                StatusSegmentSide::Right => self.right_segments.get(vs.segment_idx),
            }?;
            seg.action_id
        };
        (
            action(env.hovered_sub(&self.id)),
            action(env.pressed_sub(&self.id)),
        )
    }

    /// The clickable segment at bar-local `(x, y)`, or `None` for a
    /// non-clickable segment or the gap between the groups — the one
    /// pointer-to-segment resolution both this widget's
    /// [`crate::interactive::Interactive`] impl and
    /// [`crate::compose::StatusBarInteraction`] route through.
    pub fn segment_at(layout: &StatusBarLayout, x: f32, y: f32) -> Option<WidgetId> {
        match layout.hit_test(x, y) {
            StatusBarHit::Segment(id) => Some(id),
            StatusBarHit::Empty => None,
        }
    }
}

impl crate::interactive::Interactive for StatusBar {
    type State = StatusBarState;
    type Action = StatusBarSegmentAction;

    /// A status bar never takes keyboard focus: every segment is reached
    /// by pointer, and a host that wants a keyboard route to one of its
    /// actions binds that action's own accelerator instead.
    fn focus(&self) -> crate::interactive::FocusSpec {
        crate::interactive::FocusSpec::None
    }

    /// Every visible segment's own rect, in bar-local coordinates and in
    /// the layout's own order (left group, then the visible right group) —
    /// the segments never overlap, so there is no most-specific-first
    /// ordering to impose.
    fn sub_regions(
        &self,
        layout: &StatusBarPrimitiveLayout,
    ) -> Vec<(Rect, crate::interactive::SubTarget)> {
        layout
            .layout
            .visible_segments
            .iter()
            .enumerate()
            .map(|(i, vs)| (vs.bounds, crate::interactive::SubTarget(i as u64)))
            .collect()
    }

    /// Pointer-only: hover tracks which clickable segment the pointer is
    /// over, a left press arms that segment, and a release over the same
    /// segment activates it — a press that drifts off its segment before
    /// release activates nothing, matching every desktop button.
    ///
    /// The press captures the pointer so the release reaches this bar even
    /// when it lands outside the bar's own rect, and so intra-bar moves
    /// keep the hover tint current while the button is held.
    fn handle(
        &self,
        state: &mut StatusBarState,
        event: &crate::interactive::RoutedEvent,
        layout: &StatusBarPrimitiveLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> Handled<StatusBarSegmentAction> {
        use crate::interactive::{PointerKind, RoutedEvent};

        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Enter | PointerKind::Move,
                local,
                ..
            } => {
                let hit = Self::segment_at(&layout.layout, local.x, local.y);
                if state.hover(hit) {
                    cx.request_redraw();
                    Handled::Consumed
                } else {
                    Handled::Ignored
                }
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Leave,
                ..
            } => {
                if state.hover(None) {
                    cx.request_redraw();
                    Handled::Consumed
                } else {
                    Handled::Ignored
                }
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::event::MouseButton::Left),
                local,
                ..
            } => {
                let hit = Self::segment_at(&layout.layout, local.x, local.y);
                if state.press(hit) {
                    cx.capture_pointer();
                    cx.request_redraw();
                    Handled::Consumed
                } else {
                    Handled::Ignored
                }
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Up(crate::event::MouseButton::Left),
                local,
                ..
            } => {
                let hit = Self::segment_at(&layout.layout, local.x, local.y);
                match state.release(hit) {
                    StatusBarRelease::Clicked(id) => {
                        cx.release_pointer();
                        cx.request_redraw();
                        Handled::Event(StatusBarSegmentAction::SegmentClicked(id))
                    }
                    StatusBarRelease::Dropped => {
                        cx.release_pointer();
                        cx.request_redraw();
                        Handled::Consumed
                    }
                    StatusBarRelease::Idle => Handled::Ignored,
                }
            }
            _ => Handled::Ignored,
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

    #[test]
    fn status_bar_layout_left_drops_segments_with_no_cells_left() {
        // Left segments ordered most-important first. When the segments
        // ahead of it have eaten the whole budget, a trailing low-priority
        // segment drops out whole and the mode indicator is preserved.
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![
                make_status_seg("NORMAL", None, true),   // 6, highest priority
                make_status_seg("main.rs", None, false), // 7
                make_status_seg("UTF-8", None, false),   // 5
            ],
            right_segments: vec![make_status_seg("Ln 1,Col 1", Some("cursor"), false)], // 10
        };
        // bar=25, right=10, gap=2 → left_budget = 25-2-10=13.
        // "NORMAL" (6) + "main.rs" (7) exactly fills 13, so "UTF-8" has
        // zero cells to squeeze into and drops.
        let layout = bar.layout(25.0, 1.0, 2.0, |seg| {
            StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        });
        let left_segments: Vec<_> = layout
            .visible_segments
            .iter()
            .filter(|v| v.side == StatusSegmentSide::Left)
            .collect();
        assert_eq!(
            left_segments.len(),
            2,
            "a trailing left segment with no cells left should be dropped whole"
        );
        assert_eq!(left_segments[0].segment_idx, 0);
        assert_eq!(left_segments[1].segment_idx, 1);
        // Neither surviving segment is clamped — their sum fits the budget.
        assert_eq!(left_segments[0].bounds.width, 6.0);
        assert_eq!(left_segments[1].bounds.width, 7.0);
    }

    #[test]
    fn status_bar_layout_left_squeezes_a_long_trailing_segment_before_dropping_it() {
        // A long message in a trailing left segment is squeezed into the
        // cells that remain (paint time ellipsis-truncates it) rather than
        // dropped: its head carries the information a reader wants, so a
        // truncated message beats an absent one.
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![
                make_status_seg(" demo ", None, true), // 6, highest priority
                make_status_seg(&format!(" Opened: {} ", "p".repeat(120)), None, false),
            ],
            right_segments: vec![make_status_seg(" ✓ leaf.rs ", Some("leaf"), false)], // 11
        };
        // bar=40, right=11, gap=2 → left_budget = 40-2-11=27.
        let layout = bar.layout(40.0, 1.0, 2.0, |seg| {
            StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        });
        let left_segments: Vec<_> = layout
            .visible_segments
            .iter()
            .filter(|v| v.side == StatusSegmentSide::Left)
            .collect();
        assert_eq!(
            left_segments.len(),
            2,
            "the overlong trailing segment must be squeezed, not dropped"
        );
        assert_eq!(left_segments[0].bounds.width, 6.0, "head keeps its width");
        assert_eq!(
            left_segments[1].bounds.width, 21.0,
            "the trailing segment gets exactly the cells left in the budget"
        );
        let right = layout
            .visible_segments
            .iter()
            .find(|v| v.side == StatusSegmentSide::Right)
            .expect("the right segment must survive");
        assert!(
            left_segments[1].bounds.x + left_segments[1].bounds.width <= right.bounds.x + 0.01,
            "the squeezed segment must end at or before the right group starts"
        );
    }

    #[test]
    fn status_bar_layout_left_group_owns_the_bar_when_there_is_no_right_group() {
        // `min_gap` separates the two groups, so with `right_segments`
        // empty the left side's budget is the whole bar (minus the edge
        // insets) — reserving a gap against nothing would truncate a
        // single-segment bar two cells early.
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![make_status_seg(" (sidebar content here) ", None, false)], // 24
            right_segments: vec![],
        };
        let layout = bar.layout(20.0, 1.0, 2.0, |seg| {
            StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        });
        let left = layout
            .visible_segments
            .iter()
            .find(|v| v.side == StatusSegmentSide::Left)
            .expect("the only segment must be visible");
        assert_eq!(
            left.bounds.width, 20.0,
            "the left group should fill the whole bar when nothing is right-aligned"
        );
    }

    #[test]
    fn status_bar_layout_left_single_segment_clamped_not_overflowing_into_right() {
        // A single left segment too wide even by itself (the gallery
        // "dirty" demo's scenario) must never paint at its full natural
        // width over the right group — its bounds are clamped to the
        // left budget so paint time ellipsis-truncates it instead.
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![make_status_seg(&"x".repeat(60), None, true)],
            right_segments: vec![make_status_seg(" Ln 1,Col 1", Some("cursor"), false)],
        };
        let layout = bar.layout(30.0, 1.0, 2.0, |seg| {
            StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        });
        let left = layout
            .visible_segments
            .iter()
            .find(|v| v.side == StatusSegmentSide::Left)
            .expect("the always-kept first left segment must still be visible");
        assert!(
            left.bounds.width < 60.0,
            "an overflowing lone left segment must be clamped below its natural \
             width, got {}",
            left.bounds.width
        );
        // The right-side cursor segment must still be fully visible and
        // positioned past where the clamped left segment ends.
        let right = layout
            .visible_segments
            .iter()
            .find(|v| v.side == StatusSegmentSide::Right)
            .expect("the cursor segment must survive");
        assert!(left.bounds.x + left.bounds.width <= right.bounds.x + 0.01);
    }

    // Outer edge inset + per-segment padding.

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

/// The cell painter's own paint coverage, driven through
/// [`cell_paint::paint`] over a ratatui `Buffer`.
#[cfg(all(test, feature = "tui"))]
mod cell_paint_tests {
    use super::{
        cell_layout, StatusBar, StatusBarHit, StatusBarLayout, StatusBarSegment,
        StatusSegmentMeasure, StatusSegmentSide,
    };
    use crate::backend::ColorDepth;
    use crate::theme::Theme;
    use crate::tui::cell_surface::TuiCellSurface;
    use crate::tui::ratatui_color;
    use crate::types::{Color, WidgetId};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Modifier;

    /// [`super::cell_paint::paint`] over `buf` at the ratatui `area`.
    fn draw_status_bar(
        buf: &mut Buffer,
        area: Rect,
        bar: &StatusBar,
        layout: &StatusBarLayout,
        theme: &Theme,
        hovered_id: Option<&WidgetId>,
        pressed_id: Option<&WidgetId>,
    ) -> StatusBarLayout {
        let mut cursor_sink = None;
        let mut surface = TuiCellSurface::over(buf, ColorDepth::TrueColor, &mut cursor_sink);
        super::cell_paint::paint(
            bar,
            layout,
            crate::event::Rect::new(
                area.x as f32,
                area.y as f32,
                area.width as f32,
                area.height as f32,
            ),
            &mut surface,
            theme,
            hovered_id,
            pressed_id,
        )
    }

    fn make_bar() -> StatusBar {
        StatusBar {
            id: WidgetId::new("test-bar"),
            left_segments: vec![
                StatusBarSegment {
                    text: "NORMAL".into(),
                    fg: Color::rgb(255, 255, 255),
                    bg: Color::rgb(40, 80, 120),
                    bold: true,
                    action_id: None,
                },
                StatusBarSegment {
                    text: " main.rs ".into(),
                    fg: Color::rgb(220, 220, 220),
                    bg: Color::rgb(40, 80, 120),
                    bold: false,
                    action_id: None,
                },
            ],
            right_segments: vec![StatusBarSegment {
                text: " 1:1 ".into(),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    /// `cell_layout`'s own measurer, spelled out so a test can lay out a
    /// bar at a width that differs from the painted area.
    fn layout_at(bar: &StatusBar, width: f32, min_gap: f32) -> StatusBarLayout {
        bar.layout(width, 1.0, min_gap, |seg| {
            StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        })
    }

    #[test]
    fn paints_left_then_right_segments() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 1));
        let bar = make_bar();
        let layout = layout_at(&bar, 40.0, 2.0);
        draw_status_bar(
            &mut buf,
            Rect::new(0, 0, 40, 1),
            &bar,
            &layout,
            &Theme::default(),
            None,
            None,
        );

        // Left side starts at column 0: "NORMAL main.rs "
        let row: String = (0..15).map(|x| cell_char(&buf, x, 0)).collect();
        assert_eq!(row, "NORMAL main.rs ");

        // Right side ends at column 40: last cell is the space after "1:1 ".
        let right: String = (35..40).map(|x| cell_char(&buf, x, 0)).collect();
        assert_eq!(right, " 1:1 ");
    }

    #[test]
    fn empty_bar_falls_back_to_theme_background() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        let bar = StatusBar {
            id: WidgetId::new("empty"),
            left_segments: vec![],
            right_segments: vec![],
        };
        let layout = layout_at(&bar, 10.0, 2.0);
        let theme = Theme {
            background: Color::rgb(1, 2, 3),
            foreground: Color::rgb(255, 255, 255),
            ..Theme::default()
        };
        draw_status_bar(
            &mut buf,
            Rect::new(0, 0, 10, 1),
            &bar,
            &layout,
            &theme,
            None,
            None,
        );

        // Whole bar painted with theme.background as bg.
        for x in 0..10 {
            let bg = buf[(x, 0)].bg;
            assert_eq!(
                bg,
                ratatui::style::Color::Rgb(1, 2, 3),
                "expected theme.background at column {x}, got {bg:?}"
            );
        }
    }

    #[test]
    fn bold_segment_sets_bold_modifier() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        let bar = make_bar();
        let layout = layout_at(&bar, 20.0, 2.0);
        draw_status_bar(
            &mut buf,
            Rect::new(0, 0, 20, 1),
            &bar,
            &layout,
            &Theme::default(),
            None,
            None,
        );

        // "NORMAL" was the bold segment — first 6 cells should carry BOLD.
        for x in 0..6 {
            assert!(
                buf[(x, 0)].modifier.contains(Modifier::BOLD),
                "expected BOLD at column {x}",
            );
        }
        // " main.rs " (non-bold) should not carry BOLD.
        for x in 6..15 {
            assert!(
                !buf[(x, 0)].modifier.contains(Modifier::BOLD),
                "did not expect BOLD at column {x}",
            );
        }
    }

    #[test]
    fn zero_size_area_is_a_no_op() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        let bar = make_bar();
        let layout = layout_at(&bar, 10.0, 2.0);
        // Zero-width area: the painter must return without panicking,
        // without touching the buffer, and with an empty layout.
        let result = draw_status_bar(
            &mut buf,
            Rect::new(0, 0, 0, 1),
            &bar,
            &layout,
            &Theme::default(),
            None,
            None,
        );
        assert_eq!(cell_char(&buf, 0, 0), ' ');
        assert!(result.hit_regions.is_empty());
    }

    // Parametrized over the area's origin — LESSONS.md "Layout helpers
    // must return coords in the same frame across backends".
    // `hit_regions` are bar-local by construction (`cell_layout` only
    // reads the rect's width), but the painter puts each segment's text at
    // `area.x + vs.bounds.x` — so the only way to prove painting is
    // correct at a non-zero origin is a full round trip: lay out, paint,
    // and check the glyph lands at the *absolute* screen position while
    // the returned hit region stays bar-local.
    fn round_trip_at(origin_x: u16, origin_y: u16) {
        // Bar with an action_id'd segment in the middle of the left half.
        let bar = StatusBar {
            id: WidgetId::new("test-bar"),
            left_segments: vec![
                StatusBarSegment {
                    text: "AAA".into(),
                    fg: Color::rgb(255, 255, 255),
                    bg: Color::rgb(0, 0, 0),
                    bold: false,
                    action_id: None,
                },
                StatusBarSegment {
                    text: "BBBB".into(),
                    fg: Color::rgb(255, 255, 255),
                    bg: Color::rgb(0, 0, 0),
                    bold: false,
                    action_id: Some(WidgetId::new("middle-action")),
                },
            ],
            right_segments: vec![],
        };
        let layout = layout_at(&bar, 20.0, 0.0);
        let mut buf = Buffer::empty(Rect::new(0, 0, origin_x + 20, origin_y + 1));
        let result = draw_status_bar(
            &mut buf,
            Rect::new(origin_x, origin_y, 20, 1),
            &bar,
            &layout,
            &Theme::default(),
            None,
            None,
        );

        // One hit region for the segment with action_id.
        assert_eq!(result.hit_regions.len(), 1);
        match &result.hit_regions[0].1 {
            StatusBarHit::Segment(id) => assert_eq!(id.as_str(), "middle-action"),
            other => panic!("expected Segment, got {:?}", other),
        }
        // Region starts after "AAA" (x=3.0) and is 4 cells wide ("BBBB") —
        // bar-local, independent of `origin_x`/`origin_y`.
        let r = &result.hit_regions[0].0;
        assert_eq!(r.x, 3.0);
        assert_eq!(r.width, 4.0);

        // Painting must land at the *absolute* screen position
        // (area.x + bounds.x, area.y), not at the bar-local coordinates.
        let painted_x = origin_x + r.x as u16;
        assert_eq!(
            cell_char(&buf, painted_x, origin_y),
            'B',
            "segment text should paint at area.x ({origin_x}) + bounds.x ({}), area.y ({origin_y})",
            r.x
        );
    }

    #[test]
    fn returns_hit_regions_for_action_segments() {
        round_trip_at(0, 0);
    }

    /// The same hit-region + paint round trip must hold when the bar
    /// isn't painted at the screen origin.
    #[test]
    fn returns_hit_regions_for_action_segments_at_nonzero_origin() {
        round_trip_at(7, 13);
    }

    fn make_clickable_bar() -> StatusBar {
        StatusBar {
            id: WidgetId::new("click-bar"),
            left_segments: vec![StatusBarSegment {
                text: "BTN".into(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: Some(WidgetId::new("btn")),
            }],
            right_segments: vec![],
        }
    }

    #[test]
    fn hovered_segment_uses_lightened_bg() {
        let bar = make_clickable_bar();
        let base_bg = bar.left_segments[0].bg;
        let hover_id = WidgetId::new("btn");
        let layout = layout_at(&bar, 20.0, 0.0);

        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        draw_status_bar(
            &mut buf,
            Rect::new(0, 0, 20, 1),
            &bar,
            &layout,
            &Theme::default(),
            Some(&hover_id),
            None,
        );

        let expected = ratatui_color(base_bg.lighten(0.05));
        assert_eq!(
            buf[(0, 0)].bg,
            expected,
            "hovered segment bg should be lightened"
        );
    }

    /// [`crate::primitive::Primitive::paint_cells`] reads the hovered and
    /// pressed segment from its [`crate::PaintEnv`].
    #[test]
    fn primitive_paint_cells_tints_the_segment_its_env_names() {
        use crate::interactive::SubTarget;
        use crate::primitive::{LayoutCtx, Primitive};

        let bar = make_clickable_bar();
        let base_bg = bar.left_segments[0].bg;
        let theme = Theme::default();
        let style = crate::Style::default();
        let layout = Primitive::layout(
            &bar,
            &LayoutCtx::new(1.0, 1.0, false),
            crate::event::Rect::new(0.0, 0.0, 20.0, 1.0),
        );
        let target = Some((bar.id, Some(SubTarget(0))));
        let first_cell_bg = |interaction: &crate::InteractionState| {
            let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
            let env = crate::PaintEnv::new(&theme, &style).with_interaction(interaction);
            Primitive::paint_cells(
                &bar,
                &layout,
                &mut crate::tui::BufferCellSurface::new(&mut buf),
                &env,
            );
            buf[(0, 0)].bg
        };

        assert_eq!(
            first_cell_bg(&crate::InteractionState::new()),
            ratatui_color(base_bg)
        );
        assert_eq!(
            first_cell_bg(&crate::InteractionState::from_targets(target, None)),
            ratatui_color(base_bg.lighten(0.05))
        );
        assert_eq!(
            first_cell_bg(&crate::InteractionState::from_targets(target, target)),
            ratatui_color(base_bg.darken(0.05))
        );
        // Hover on another widget leaves this bar at rest.
        let elsewhere = Some((WidgetId::new("other"), Some(SubTarget(0))));
        assert_eq!(
            first_cell_bg(&crate::InteractionState::from_targets(elsewhere, None)),
            ratatui_color(base_bg)
        );
    }

    #[test]
    fn pressed_segment_uses_darkened_bg() {
        let bar = make_clickable_bar();
        let base_bg = bar.left_segments[0].bg;
        let press_id = WidgetId::new("btn");
        let layout = layout_at(&bar, 20.0, 0.0);

        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        draw_status_bar(
            &mut buf,
            Rect::new(0, 0, 20, 1),
            &bar,
            &layout,
            &Theme::default(),
            None,
            Some(&press_id),
        );

        let expected = ratatui_color(base_bg.darken(0.05));
        assert_eq!(
            buf[(0, 0)].bg,
            expected,
            "pressed segment bg should be darkened"
        );
    }

    #[test]
    fn pressed_takes_precedence_over_hovered() {
        let bar = make_clickable_bar();
        let base_bg = bar.left_segments[0].bg;
        let id = WidgetId::new("btn");
        let layout = layout_at(&bar, 20.0, 0.0);

        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        draw_status_bar(
            &mut buf,
            Rect::new(0, 0, 20, 1),
            &bar,
            &layout,
            &Theme::default(),
            Some(&id),
            Some(&id),
        );

        let expected = ratatui_color(base_bg.darken(0.05));
        assert_eq!(buf[(0, 0)].bg, expected, "pressed should win over hovered");
    }

    #[test]
    fn non_clickable_segment_ignores_hover() {
        let bar = make_bar();
        let base_bg = bar.left_segments[0].bg;
        let fake_id = WidgetId::new("no-match");
        let layout = layout_at(&bar, 40.0, 2.0);

        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 1));
        draw_status_bar(
            &mut buf,
            Rect::new(0, 0, 40, 1),
            &bar,
            &layout,
            &Theme::default(),
            Some(&fake_id),
            None,
        );

        let expected = ratatui_color(base_bg);
        assert_eq!(
            buf[(0, 0)].bg,
            expected,
            "non-clickable segment bg should be unchanged"
        );
    }

    fn plain_right_seg(text: &str) -> StatusBarSegment {
        StatusBarSegment {
            text: text.to_string(),
            fg: Color::rgb(220, 220, 220),
            bg: Color::rgb(40, 80, 120),
            bold: false,
            action_id: None,
        }
    }

    fn row_text(buf: &Buffer, width: u16) -> String {
        (0..width).map(|x| cell_char(buf, x, 0)).collect()
    }

    /// Paints through [`cell_layout`] itself — the measurement every
    /// cell-grid caller shares — rather than a hand-rolled measurer.
    fn paint_at_width(bar: &StatusBar, width: u16) -> (Buffer, StatusBarLayout) {
        let area = crate::event::Rect::new(0.0, 0.0, width as f32, 1.0);
        let layout = cell_layout(bar, area);
        let mut buf = Buffer::empty(Rect::new(0, 0, width, 1));
        draw_status_bar(
            &mut buf,
            Rect::new(0, 0, width, 1),
            bar,
            &layout,
            &Theme::default(),
            None,
            None,
        );
        (buf, layout)
    }

    /// Reproduces the bugbash shape exactly — a left segment growing by 4
    /// characters (the "[+]" dirty badge) tips a 37-column bar from
    /// "everything fits" to "one right segment must drop" — and proves
    /// that when `right_segments` follows the documented convention
    /// (cursor-position segment *last*), only the lowest-priority segment
    /// ("Spaces: 4") is dropped. "Ln 1, Col 1" keeps painting through the
    /// dirty transition.
    #[test]
    fn priority_drop_keeps_painting_the_last_segment_once_a_left_segment_grows() {
        let mut bar = StatusBar {
            id: WidgetId::new("editor-status"),
            left_segments: vec![StatusBarSegment {
                text: "NORMAL".into(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: true,
                action_id: None,
            }],
            right_segments: vec![
                plain_right_seg("Spaces: 4"),
                plain_right_seg("UTF-8"),
                plain_right_seg("LF"),
                plain_right_seg("Ln 1, Col 1"), // cursor position: last = highest priority
            ],
        };

        // Clean: "NORMAL" (6) + gap(2) + right total(27) = 35 <= 37. Nothing dropped.
        let (clean_buf, clean_layout) = paint_at_width(&bar, 37);
        assert_eq!(clean_layout.resolved_right_start, 0);
        let clean_row = row_text(&clean_buf, 37);
        assert!(clean_row.contains("Ln 1, Col 1"));
        assert!(clean_row.contains("Spaces: 4"));

        // Dirty: "[+]" badge grows the left segment by exactly 4 chars,
        // same as the bugbash repro ("i X Esc"). 10 + gap(2) = 12,
        // max_right = 25 < 27, so the front (lowest-priority) segment —
        // "Spaces: 4" — must drop. The cursor segment must NOT.
        bar.left_segments[0].text = "NORMAL [+]".into();
        let (dirty_buf, dirty_layout) = paint_at_width(&bar, 37);
        assert_eq!(
            dirty_layout.resolved_right_start, 1,
            "only the lowest-priority segment should be dropped for a 4-char deficit"
        );
        let dirty_row = row_text(&dirty_buf, 37);
        assert!(
            dirty_row.contains("Ln 1, Col 1"),
            "cursor-position segment must keep painting once dirtied: {dirty_row:?}"
        );
        assert!(
            !dirty_row.contains("Spaces: 4"),
            "the dropped segment must not still be painted: {dirty_row:?}"
        );
        assert!(dirty_row.contains("UTF-8"));
        assert!(dirty_row.contains("LF"));
    }

    /// Companion to the test above: if the call site puts the
    /// cursor-position segment *first* in `right_segments` instead of
    /// last — e.g. because that reads naturally next to
    /// "Spaces"/"UTF-8"/"LF" — the shared drop algorithm (correctly, per
    /// its documented contract) treats it as the lowest-priority segment
    /// and drops it first, under the exact same 4-character deficit that
    /// the test above shows is recoverable by dropping a *different*
    /// segment. This demonstrates the call-site ordering convention, not
    /// a `fit_right_start`/`layout` defect — swapping this segment to the
    /// end of the vector (as above) is the available fix, with no
    /// primitive change. This test only asserts which *index* is dropped;
    /// it intentionally does not assert which segment's *text* must or
    /// must not survive, so it stays agnostic to any future primitive-
    /// level affordance (e.g. an always-visible flag) that might change
    /// which segment that index names.
    #[test]
    fn ordering_the_cursor_segment_first_instead_of_last_makes_it_the_one_dropped() {
        let mut bar = StatusBar {
            id: WidgetId::new("editor-status"),
            left_segments: vec![StatusBarSegment {
                text: "NORMAL".into(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: true,
                action_id: None,
            }],
            right_segments: vec![
                plain_right_seg("Ln 1, Col 1"), // cursor position placed first — the anti-pattern
                plain_right_seg("Spaces: 4"),
                plain_right_seg("UTF-8"),
                plain_right_seg("LF"),
            ],
        };

        let (clean_buf, clean_layout) = paint_at_width(&bar, 37);
        assert_eq!(clean_layout.resolved_right_start, 0);
        assert!(row_text(&clean_buf, 37).contains("Ln 1, Col 1"));

        bar.left_segments[0].text = "NORMAL [+]".into();
        let (_dirty_buf, dirty_layout) = paint_at_width(&bar, 37);
        assert_eq!(
            dirty_layout.resolved_right_start, 1,
            "the front segment (cursor position, in this mis-ordered bar) is dropped"
        );
    }

    /// Reproduces the gallery Status Bar demo's "Dirty (priority drop)"
    /// variant: the left segment grows far past anything a priority-drop
    /// of the *right* side alone can accommodate. The left segment must
    /// still paint — ellipsis-truncated to its own reserved cells — and
    /// the cursor-position segment must keep painting undisturbed,
    /// rather than the left segment's overflow bleeding into (or past)
    /// it.
    #[test]
    fn dirty_variant_ellipsizes_the_left_segment_without_corrupting_the_right_side() {
        let bar = StatusBar {
            id: WidgetId::new("gallery:status-bar"),
            left_segments: vec![StatusBarSegment {
                text: format!("NORMAL{}", " [+]".repeat(60)),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: true,
                action_id: None,
            }],
            right_segments: vec![
                plain_right_seg("Spaces: 4"),
                plain_right_seg("UTF-8"),
                plain_right_seg("LF"),
                plain_right_seg("Ln 1, Col 1"),
            ],
        };
        let (buf, layout) = paint_at_width(&bar, 60);
        let row = row_text(&buf, 60);

        assert!(
            row.contains('…'),
            "the massively overflowing left segment must ellipsize: {row:?}"
        );
        assert!(
            row.contains("Ln 1, Col 1"),
            "the cursor-position segment must survive, undisturbed: {row:?}"
        );

        let left = layout
            .visible_segments
            .iter()
            .find(|v| v.side == StatusSegmentSide::Left)
            .expect("the always-kept left segment must still be visible");
        let right = layout
            .visible_segments
            .iter()
            .find(|v| v.side == StatusSegmentSide::Right)
            .expect("at least the cursor segment must survive on the right");
        assert!(
            left.bounds.x + left.bounds.width <= right.bounds.x,
            "the left segment's reserved cells must end at or before the right \
             group starts: left ends at {}, right starts at {}",
            left.bounds.x + left.bounds.width,
            right.bounds.x
        );
    }

    /// A consumer that pads a segment as `format!(" {label} ")` — the
    /// conventional shape — must not lose the last character of `label`
    /// to an ellipsis when the only thing that overflows is the padding
    /// space itself. `quadraui-gallery`'s Code/Data/log panes paint one
    /// source line per bar this way, so a single-cell overflow there is
    /// the difference between a readable line and a mangled one.
    #[test]
    fn a_segment_overflowing_only_by_its_trailing_pad_clips_instead_of_ellipsizing() {
        let bar = StatusBar {
            id: WidgetId::new("padded"),
            // 14 cells: a 12-char label between two pad spaces.
            left_segments: vec![plain_right_seg(" padded label ")],
            right_segments: vec![],
        };
        // 13 cells of budget: one short of the padded text, exactly the
        // width of its trimmed form.
        let (buf, _) = paint_at_width(&bar, 13);
        let row = row_text(&buf, 13);
        assert_eq!(
            row, " padded label",
            "the trailing pad should be clipped, keeping the whole label"
        );
    }

    #[test]
    fn a_segment_overflowing_past_its_trailing_pad_still_ellipsizes() {
        let bar = StatusBar {
            id: WidgetId::new("padded"),
            left_segments: vec![plain_right_seg(" padded label ")],
            right_segments: vec![],
        };
        // 12 cells: too narrow for even the trimmed text, so a real
        // character has to go and the run must say so with an ellipsis.
        let (buf, _) = paint_at_width(&bar, 12);
        let row = row_text(&buf, 12);
        assert_eq!(row, " padded lab…");
    }
}

/// [`StatusBar`]'s own `Primitive`/`Interactive` impls, exercised
/// directly: layout in both unit regimes, and every pointer gesture the
/// widget resolves.
#[cfg(test)]
mod widget_contract_tests {
    use super::{
        StatusBar, StatusBarSegment, StatusBarSegmentAction, StatusBarState, PIXEL_EDGE_INSET,
    };
    use crate::compose::Handled;
    use crate::event::{MouseButton, Point};
    use crate::interactive::{
        EventCx, EventHost, FocusSpec, Interactive, PointerKind, RoutedEvent, SubTarget,
    };
    use crate::primitive::{LayoutCtx, Primitive, Units};
    use crate::types::{Color, WidgetId};
    use crate::Rect;

    /// Records every [`EventHost`] intent a `handle` call makes, so a
    /// test can assert the capture/redraw side effects the runtime would
    /// have applied.
    #[derive(Default)]
    struct RecordingHost {
        captured: Option<WidgetId>,
        released: usize,
        redraws: usize,
    }

    impl EventHost for RecordingHost {
        fn is_focused(&self, _id: &WidgetId, _sub: Option<SubTarget>) -> bool {
            false
        }
        fn request_focus(&mut self, _id: &WidgetId, _sub: Option<SubTarget>) {}
        fn capture_pointer(&mut self, id: &WidgetId) {
            self.captured = Some(*id);
        }
        fn release_pointer(&mut self) {
            self.released += 1;
        }
        fn set_cursor(&mut self, _cursor: crate::PointerShape) {}
        fn request_redraw(&mut self) {
            self.redraws += 1;
        }
        fn emit(&mut self, _event: crate::UiEvent) {}
        fn units(&self) -> Units {
            Units::Cells
        }
    }

    fn seg(text: &str, action: Option<&str>) -> StatusBarSegment {
        StatusBarSegment {
            text: text.to_string(),
            fg: Color::rgb(255, 255, 255),
            bg: Color::rgb(40, 80, 120),
            bold: false,
            action_id: action.map(WidgetId::new),
        }
    }

    fn bar() -> StatusBar {
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![seg("MODE", Some("mode")), seg("plain", None)],
            right_segments: vec![seg("1:1", Some("cursor"))],
        }
    }

    const AREA: Rect = Rect::new(0.0, 0.0, 40.0, 1.0);

    fn cell_ctx() -> LayoutCtx<'static> {
        LayoutCtx::new(1.0, 1.0, false)
    }

    fn pointer(kind: PointerKind, x: f32, y: f32) -> RoutedEvent {
        RoutedEvent::Pointer {
            kind,
            sub: None,
            local: Point::new(x, y),
            mods: Default::default(),
        }
    }

    #[test]
    fn cell_layout_resolves_one_row_and_bar_local_hit_regions() {
        let bar = bar();
        let layout = Primitive::layout(&bar, &cell_ctx(), Rect::new(5.0, 9.0, 40.0, 1.0));

        assert_eq!(layout.area, Rect::new(5.0, 9.0, 40.0, 1.0));
        assert_eq!(layout.layout.bar_height, 1.0);
        // "MODE" is four cells from the bar's own left edge — bar-local,
        // not offset by the area's origin.
        let mode = layout.layout.visible_segments[0];
        assert_eq!(mode.bounds.x, 0.0);
        assert_eq!(mode.bounds.width, 4.0);
        assert_eq!(
            layout.hit_test(1.0, 0.5),
            super::StatusBarHit::Segment(WidgetId::new("mode"))
        );
    }

    /// A pixel ctx lays out through `layout_padded`, so the left-most
    /// segment starts `PIXEL_EDGE_INSET` in from the bar's own edge
    /// rather than flush against it.
    #[test]
    fn pixel_layout_reserves_the_pixel_edge_inset() {
        let bar = bar();
        let ctx = LayoutCtx::new(16.0, 8.0, false);
        assert_eq!(ctx.units(), Units::Pixels);
        let layout = Primitive::layout(&bar, &ctx, Rect::new(0.0, 0.0, 600.0, 22.0));

        assert_eq!(layout.layout.visible_segments[0].bounds.x, PIXEL_EDGE_INSET);
        assert_eq!(layout.layout.bar_height, 22.0);
    }

    /// A bold segment is measured in `FontRole::ChromeBold`, so a backend
    /// that measures bold text wider gives the segment a wider box and
    /// the next segment's hit region starts where the painter draws it.
    #[test]
    fn pixel_layout_measures_a_bold_segment_in_the_bold_chrome_role() {
        let measurer = |text: &str, role: crate::FontRole| {
            let base = 8.0 * text.chars().count() as f32;
            if role == crate::FontRole::ChromeBold {
                base + 10.0
            } else {
                base
            }
        };
        let ctx = LayoutCtx::new(16.0, 8.0, false).with_measurer(&measurer);
        let first_and_next = |bold: bool| {
            let mut bar = bar();
            bar.left_segments[0].bold = bold;
            let layout = Primitive::layout(&bar, &ctx, Rect::new(0.0, 0.0, 600.0, 22.0));
            (
                layout.layout.visible_segments[0].bounds.width,
                layout.layout.visible_segments[1].bounds.x,
            )
        };
        let (regular_w, regular_next) = first_and_next(false);
        let (bold_w, bold_next) = first_and_next(true);
        assert_eq!(bold_w, regular_w + 10.0);
        assert_eq!(bold_next, regular_next + 10.0);
    }

    #[test]
    fn measure_reports_one_row_on_a_cell_grid() {
        let bar = bar();
        let natural =
            Primitive::measure(&bar, &cell_ctx(), crate::primitive::Size::new(40.0, 10.0));
        assert_eq!(natural.width, None, "a status bar stretches horizontally");
        assert_eq!(natural.height, Some(1.0));
    }

    #[test]
    fn a_status_bar_is_not_a_keyboard_focus_stop() {
        assert_eq!(Interactive::focus(&bar()), FocusSpec::None);
    }

    #[test]
    fn sub_regions_name_every_visible_segment_in_bar_local_space() {
        let bar = bar();
        let layout = Primitive::layout(&bar, &cell_ctx(), AREA);
        let regions = Interactive::sub_regions(&bar, &layout);

        assert_eq!(regions.len(), layout.layout.visible_segments.len());
        assert_eq!(regions[0].0, layout.layout.visible_segments[0].bounds);
        assert_eq!(regions[0].1, SubTarget(0));
    }

    #[test]
    fn hover_tracks_the_clickable_segment_under_the_pointer() {
        let bar = bar();
        let layout = Primitive::layout(&bar, &cell_ctx(), AREA);
        let mut state = StatusBarState::default();
        let mut host = RecordingHost::default();
        let mut cx = EventCx::new(bar.id, &mut host);

        let handled = Interactive::handle(
            &bar,
            &mut state,
            &pointer(PointerKind::Enter, 1.0, 0.5),
            &layout,
            &mut cx,
        );
        assert_eq!(handled, Handled::Consumed);
        assert_eq!(state.hovered, Some(WidgetId::new("mode")));

        // Moving onto the non-clickable segment clears the hover.
        let handled = Interactive::handle(
            &bar,
            &mut state,
            &pointer(PointerKind::Move, 5.0, 0.5),
            &layout,
            &mut cx,
        );
        assert_eq!(handled, Handled::Consumed);
        assert_eq!(state.hovered, None);

        // A second move with nothing changing asks for no redraw.
        let handled = Interactive::handle(
            &bar,
            &mut state,
            &pointer(PointerKind::Move, 6.0, 0.5),
            &layout,
            &mut cx,
        );
        assert_eq!(handled, Handled::Ignored);
    }

    #[test]
    fn leaving_the_bar_clears_the_hover() {
        let bar = bar();
        let layout = Primitive::layout(&bar, &cell_ctx(), AREA);
        let mut state = StatusBarState {
            hovered: Some(WidgetId::new("mode")),
            ..StatusBarState::default()
        };
        let mut host = RecordingHost::default();
        let mut cx = EventCx::new(bar.id, &mut host);

        let handled = Interactive::handle(
            &bar,
            &mut state,
            &pointer(PointerKind::Leave, 1.0, 0.5),
            &layout,
            &mut cx,
        );
        assert_eq!(handled, Handled::Consumed);
        assert_eq!(state.hovered, None);
        assert_eq!(host.redraws, 1);
    }

    #[test]
    fn press_then_release_on_the_same_segment_clicks_it() {
        let bar = bar();
        let layout = Primitive::layout(&bar, &cell_ctx(), AREA);
        let mut state = StatusBarState::default();
        let mut host = RecordingHost::default();
        let mut cx = EventCx::new(bar.id, &mut host);

        let down = Interactive::handle(
            &bar,
            &mut state,
            &pointer(PointerKind::Down(MouseButton::Left), 2.0, 0.5),
            &layout,
            &mut cx,
        );
        assert_eq!(down, Handled::Consumed);
        assert_eq!(state.pressed, Some(WidgetId::new("mode")));

        let up = Interactive::handle(
            &bar,
            &mut state,
            &pointer(PointerKind::Up(MouseButton::Left), 2.0, 0.5),
            &layout,
            &mut cx,
        );
        assert_eq!(
            up,
            Handled::Event(StatusBarSegmentAction::SegmentClicked(WidgetId::new(
                "mode"
            )))
        );
        assert_eq!(state.pressed, None);
        assert_eq!(
            host.captured,
            Some(bar.id),
            "the press must capture the pointer so the release reaches the bar"
        );
        assert_eq!(host.released, 1);
    }

    #[test]
    fn a_press_released_off_its_segment_clicks_nothing() {
        let bar = bar();
        let layout = Primitive::layout(&bar, &cell_ctx(), AREA);
        let mut state = StatusBarState::default();
        let mut host = RecordingHost::default();
        let mut cx = EventCx::new(bar.id, &mut host);

        let _ = Interactive::handle(
            &bar,
            &mut state,
            &pointer(PointerKind::Down(MouseButton::Left), 2.0, 0.5),
            &layout,
            &mut cx,
        );
        let up = Interactive::handle(
            &bar,
            &mut state,
            &pointer(PointerKind::Up(MouseButton::Left), 6.0, 0.5),
            &layout,
            &mut cx,
        );
        assert_eq!(up, Handled::Consumed);
        assert_eq!(state.pressed, None);
    }

    #[test]
    fn a_press_on_a_non_clickable_segment_is_ignored() {
        let bar = bar();
        let layout = Primitive::layout(&bar, &cell_ctx(), AREA);
        let mut state = StatusBarState::default();
        let mut host = RecordingHost::default();
        let mut cx = EventCx::new(bar.id, &mut host);

        let down = Interactive::handle(
            &bar,
            &mut state,
            &pointer(PointerKind::Down(MouseButton::Left), 6.0, 0.5),
            &layout,
            &mut cx,
        );
        assert_eq!(down, Handled::Ignored);
        assert_eq!(state.pressed, None);
        assert_eq!(host.captured, None);
    }

    #[test]
    fn the_segment_action_rides_the_status_bar_ui_event() {
        use crate::interactive::WidgetAction;
        let event = StatusBarSegmentAction::SegmentClicked(WidgetId::new("mode"))
            .into_ui_event(WidgetId::new("status"));
        assert_eq!(
            event,
            crate::UiEvent::StatusBar(
                WidgetId::new("status"),
                super::StatusBarEvent::SegmentClicked {
                    id: WidgetId::new("mode")
                }
            )
        );
    }
}

/// The macOS pixel coverage for the bar, painted through the widget
/// contract ([`crate::DrawsPrimitives::draw_dyn`]) onto a headless
/// Core Graphics bitmap.
#[cfg(all(test, feature = "macos", target_os = "macos"))]
mod mac_paint_tests {
    use super::{
        StatusBar, StatusBarHit, StatusBarLayout, StatusBarPrimitiveLayout, StatusBarSegment,
        StatusSegmentSide,
    };
    use crate::event::{Rect as QRect, Viewport};
    use crate::macos::headless::BitmapSurface;
    use crate::macos::text::make_font;
    use crate::macos::MacBackend;
    use crate::theme::Theme;
    use crate::types::{Color, WidgetId};
    use crate::{Backend, DrawsPrimitives};
    use core_text::font::CTFont;

    const W: u32 = 320;
    const H: u32 = 24;
    const FONT_SIZE: f64 = 14.0;

    fn font() -> CTFont {
        make_font("Menlo", FONT_SIZE).expect("Menlo installed on every macOS host")
    }

    /// A backend with **both** font slots set to the harness font.
    ///
    /// The status bar paints and measures through `MacBackend::chrome_font` /
    /// `chrome_line_height`, not the editor `current_font`. `MacBackend::new`
    /// seeds chrome with the ~11pt system UI font, whose line height is under
    /// half of this module's `H = 24` surface. Setting both keeps every
    /// assertion about status-bar geometry at a known `FONT_SIZE`.
    fn backend_with_test_fonts() -> MacBackend {
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.set_chrome_font(font());
        backend
    }

    /// Three-segment bar:
    /// - Left[0]: "(L) " sentinel with bg `(10, 20, 30)` — sets the
    ///   bar fill (first segment's bg) to a colour distinct from the
    ///   clickable segment, so paint-shift mutations can't hide
    ///   behind a matching bar fill.
    /// - Left[1]: " Save " clickable, bg `(40, 80, 120)`.
    /// - Right[0]: " 1:1 " non-clickable, bg `(40, 80, 120)`.
    ///
    /// Leading/trailing spaces give glyph-free padding pixels for
    /// bg-colour probing — same trick the TUI reference tests use.
    fn sample_bar() -> StatusBar {
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![
                StatusBarSegment {
                    text: "(L) ".into(),
                    fg: Color::rgb(255, 255, 255),
                    bg: Color::rgb(10, 20, 30),
                    bold: false,
                    action_id: None,
                },
                StatusBarSegment {
                    text: " Save ".into(),
                    fg: Color::rgb(255, 255, 255),
                    bg: Color::rgb(40, 80, 120),
                    bold: false,
                    action_id: Some(WidgetId::new("status:save")),
                },
            ],
            right_segments: vec![StatusBarSegment {
                text: " 1:1 ".into(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    /// Paint a bar through the widget contract and return both the
    /// surface (for pixel inspection) and the layout (for hit_test).
    fn paint_via_backend(bar: &StatusBar) -> (BitmapSurface, StatusBarLayout) {
        paint_via_backend_at(bar, 0.0, 0.0)
    }

    /// Like [`paint_via_backend`] but paints at an arbitrary `(x, y)`
    /// origin instead of always `(0, 0)`. Lets tests exercise the
    /// paint-time `x`/`y` shift independently of the bar-local
    /// `StatusBarLayout` it returns.
    fn paint_via_backend_at(bar: &StatusBar, x: f32, y: f32) -> (BitmapSurface, StatusBarLayout) {
        let surface = BitmapSurface::new(W, H);
        // Clear so we can distinguish "painted by status_bar" from
        // "untouched memory" cleanly.
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let mut backend = backend_with_test_fonts();
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));

        let layout = std::cell::RefCell::new(None);
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            let l = b
                .draw_dyn(QRect::new(x, y, W as f32 - x, H as f32 - y), bar, &())
                .downcast_ref::<StatusBarPrimitiveLayout>()
                .expect("a StatusBar lays out as StatusBarPrimitiveLayout")
                .layout
                .clone();
            *layout.borrow_mut() = Some(l);
        });
        backend.end_frame();
        (surface, layout.into_inner().unwrap())
    }

    /// Probe a column near the leading space of the clickable "Save"
    /// segment — a glyph-free padding region that exposes the
    /// segment's bg fill without interference from rasterised text.
    /// Sample near the top edge where line_height is guaranteed
    /// painted and glyphs (anchored at the ascent baseline) don't
    /// reach.
    fn probe_save_segment_bg(surface: &BitmapSurface, layout: &StatusBarLayout) -> (u8, u8, u8) {
        let save = layout
            .visible_segments
            .iter()
            .filter(|vs| vs.side == StatusSegmentSide::Left)
            .nth(1)
            .expect("save segment visible");
        let probe_x = (save.bounds.x as u32) + 1;
        let probe_y = 2;
        let (r, g, b, _) = surface.pixel(probe_x, probe_y);
        (r, g, b)
    }

    #[test]
    fn paints_segment_backgrounds() {
        let bar = sample_bar();
        let (surface, layout) = paint_via_backend(&bar);
        assert_eq!(
            probe_save_segment_bg(&surface, &layout),
            (40, 80, 120),
            "left-segment bg should match StatusBarSegment.bg",
        );
    }

    /// Shared body for `round_trip_click_hits_clickable_segment` and its
    /// `_at_nonzero_origin` sibling. Paints `sample_bar()` at
    /// `(origin_x, origin_y)`, then round-trips an absolute click through
    /// localisation the way a real host does before calling `hit_test`, and
    /// checks the *painted* segment lands at the origin-shifted absolute
    /// position. Guards against a paint/layout frame mismatch.
    fn round_trip_click_hits_clickable_segment_at(origin_x: f32, origin_y: f32) {
        let bar = sample_bar();
        let (surface, layout) = paint_via_backend_at(&bar, origin_x, origin_y);

        let save = layout
            .visible_segments
            .iter()
            .filter(|vs| vs.side == StatusSegmentSide::Left)
            .nth(1)
            .expect("save segment visible");

        // Paint-vs-layout frame check: the segment's bg must be painted
        // at the origin-shifted absolute x, not at the bar-local x.
        let probe_x = (origin_x + save.bounds.x) as u32 + 1;
        let probe_y = origin_y as u32 + 2;
        let (r, g, b, _) = surface.pixel(probe_x, probe_y);
        assert_eq!(
            (r, g, b),
            (40, 80, 120),
            "painted Save segment bg should be at absolute x = origin_x + local bounds.x",
        );

        // Round trip: absolute click position -> localise like a real
        // host -> hit_test against the bar-local layout.
        let abs_x = origin_x + save.bounds.x + save.bounds.width * 0.5;
        let abs_y = origin_y + save.bounds.y + save.bounds.height * 0.5;
        let local_x = abs_x - origin_x;
        let local_y = abs_y - origin_y;
        let hit = layout.hit_test(local_x, local_y);
        assert_eq!(
            hit,
            StatusBarHit::Segment(WidgetId::new("status:save")),
            "expected clickable Save segment hit at local ({}, {})",
            local_x,
            local_y,
        );

        // Sanity check: the right segment is non-clickable, so a hit
        // inside its bounds must return Empty.
        let right = layout
            .visible_segments
            .iter()
            .find(|vs| vs.side == StatusSegmentSide::Right)
            .expect("right segment visible");
        let right_hit = layout.hit_test(
            right.bounds.x + right.bounds.width * 0.5,
            right.bounds.y + right.bounds.height * 0.5,
        );
        assert_eq!(
            right_hit,
            StatusBarHit::Empty,
            "non-clickable right segment must hit Empty",
        );
    }

    #[test]
    fn round_trip_click_hits_clickable_segment() {
        round_trip_click_hits_clickable_segment_at(0.0, 0.0);
    }

    #[test]
    fn round_trip_click_hits_clickable_segment_at_nonzero_origin() {
        // The origin-(0,0) variant above can't exercise localisation —
        // `abs_x - x` is a no-op when `x == 0`. Paint at a non-zero
        // origin so the round trip only passes if paint and layout agree
        // on which frame `x`/`y` apply in.
        round_trip_click_hits_clickable_segment_at(7.0, 4.0);
    }

    #[test]
    fn empty_bar_falls_back_to_theme_background() {
        // No segments → fill colour comes from theme.background.
        let bar = StatusBar {
            id: WidgetId::new("empty"),
            left_segments: vec![],
            right_segments: vec![],
        };
        let surface = BitmapSurface::new(W, H);
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let mut backend = backend_with_test_fonts();
        backend.set_current_theme(Theme {
            background: Color::rgb(1, 2, 3),
            ..Theme::default()
        });
        backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
        backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_dyn(QRect::new(0.0, 0.0, W as f32, H as f32), &bar, &());
        });
        backend.end_frame();

        // Every pixel should carry the theme bg.
        let (r, g, b, _) = surface.pixel(W / 2, H / 2);
        assert_eq!(
            (r, g, b),
            (1, 2, 3),
            "empty bar should be filled with theme.background",
        );
    }

    /// [`crate::DrawsPrimitives::layout_primitive`] must return exactly
    /// the layout the widget painted.
    #[test]
    fn layout_twin_matches_the_painted_layout() {
        let bar = sample_bar();
        let (_surface, painted) = paint_via_backend(&bar);

        let backend = backend_with_test_fonts();
        let computed = backend
            .layout_primitive(QRect::new(0.0, 0.0, W as f32, H as f32), &bar)
            .layout;

        assert_eq!(
            painted.visible_segments.len(),
            computed.visible_segments.len(),
        );
        for (p, c) in painted
            .visible_segments
            .iter()
            .zip(computed.visible_segments.iter())
        {
            assert_eq!(p.segment_idx, c.segment_idx);
            assert_eq!(p.side, c.side);
            assert!((p.bounds.x - c.bounds.x).abs() < 0.001);
            assert!((p.bounds.width - c.bounds.width).abs() < 0.001);
        }
    }

    /// The no-paint twin is also origin-independent: hit regions are
    /// bar-local, so painting at a non-zero origin does not move them.
    #[test]
    fn layout_twin_is_bar_local_at_a_nonzero_origin() {
        let bar = sample_bar();
        let (_surface, painted) = paint_via_backend_at(&bar, 0.0, 0.0);
        let backend = backend_with_test_fonts();
        let computed = backend
            .layout_primitive(QRect::new(17.0, 3.0, W as f32, H as f32), &bar)
            .layout;
        for (p, c) in painted
            .visible_segments
            .iter()
            .zip(computed.visible_segments.iter())
        {
            assert!(
                (p.bounds.x - c.bounds.x).abs() < 0.001,
                "segment x moved with the origin: {} vs {}",
                p.bounds.x,
                c.bounds.x,
            );
        }
    }
}

/// The GTK Cairo coverage for the shared pixel painter.
#[cfg(all(test, feature = "gtk"))]
mod gtk_paint_tests {
    use super::{StatusBar, StatusBarHit, StatusBarSegment};
    use crate::theme::Theme;
    use crate::types::{Color, WidgetId};
    use gtk4::pango;
    use pangocairo::cairo::{Context as CairoContext, Format, ImageSurface};

    fn headless_cairo_and_pango() -> (ImageSurface, pango::Layout) {
        let surface = ImageSurface::create(Format::ARgb32, 200, 200).expect("create ImageSurface");
        let cr = CairoContext::new(&surface).expect("Context::new");
        let layout = pangocairo::functions::create_layout(&cr);
        (surface, layout)
    }

    fn test_bar() -> StatusBar {
        StatusBar {
            id: WidgetId::new("sb"),
            left_segments: vec![StatusBarSegment {
                text: "Ready".into(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(0, 0, 0),
                bold: false,
                action_id: Some(WidgetId::new("sb:action")),
            }],
            right_segments: vec![],
        }
    }

    /// The painted layout is **bar-local**: `StatusBar::layout` (called by
    /// `paint` internally) only ever receives `width`/`line_height` — the
    /// `x`/`y` the painter paints at only offset the drawing calls, never
    /// fold into the returned `StatusBarLayout`. Painting at a non-zero
    /// origin must therefore produce byte-identical segment bounds / hit
    /// regions to painting at the origin.
    fn round_trip_at(x: f64, y: f64) {
        let (surface, pango_layout) = headless_cairo_and_pango();
        let cr = CairoContext::new(&surface).expect("Context::new");
        let theme = Theme::default();
        let bar = test_bar();

        let mut backend = crate::gtk::GtkBackend::new();
        let (at_origin, shifted) = backend.enter_frame_scope(&cr, &pango_layout, |b| {
            let at_origin = super::native_surface_paint::paint(
                &bar, b, &theme, 0.0, 0.0, 100.0, 20.0, None, None,
            );
            let shifted = super::native_surface_paint::paint(
                &bar, b, &theme, x as f32, y as f32, 100.0, 20.0, None, None,
            );
            (at_origin, shifted)
        });

        assert_eq!(
            at_origin, shifted,
            "the bar-local layout must not shift segment bounds by x/y"
        );

        let seg = shifted.visible_segments[0];
        let cx = seg.bounds.x + 1.0;
        let cy = seg.bounds.y + 1.0;
        assert_eq!(
            shifted.hit_test(cx, cy),
            StatusBarHit::Segment(bar.left_segments[0].action_id.clone().unwrap())
        );
    }

    #[test]
    fn paint_and_click_round_trip() {
        round_trip_at(0.0, 0.0);
    }

    /// Non-zero-origin regression guard.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        round_trip_at(7.0, 13.0);
    }
}
