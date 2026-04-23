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
//! 3. **Skip dropped segments in click handlers.** Use
//!    [`StatusBar::resolve_click_fit_chars`] (TUI) or compute hit
//!    regions only for visible segments (GTK / Win-GUI, where draw_func
//!    populates per-segment hit zones inline). Otherwise clicks on
//!    columns where dropped segments *used to be* will trigger their
//!    actions even though the user can't see them.
//!
//! Convention for app-side priority: **`right_segments` is built
//! least-important first, most-important (e.g. cursor position) last.**
//! `fit_right_start` drops from the front, so the rightmost (highest-
//! priority) segments stay visible at the right edge of the bar.
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
/// width in cells. Computed by [`StatusBar::hit_regions`].
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
    pub fn hit_regions(&self, bar_width: usize) -> Vec<StatusBarHitRegion> {
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
        for region in self.hit_regions(bar_width) {
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

    /// Like `hit_regions` but skips segments dropped by `fit_right_start_chars`.
    /// Use when the visible right half may have been narrowed.
    pub fn hit_regions_fit_chars(
        &self,
        bar_width: usize,
        min_gap: usize,
    ) -> Vec<StatusBarHitRegion> {
        let start = self.fit_right_start_chars(bar_width, min_gap);
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
        let visible_right = &self.right_segments[start..];
        let right_width: usize = visible_right.iter().map(|s| s.text.chars().count()).sum();
        let mut col = bar_width.saturating_sub(right_width) as u16;
        for seg in visible_right {
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

    /// Like `resolve_click` but uses `hit_regions_fit_chars` so clicks on
    /// dropped (invisible) segments don't trigger spurious actions.
    pub fn resolve_click_fit_chars(
        &self,
        click_col: u16,
        bar_width: usize,
        min_gap: usize,
    ) -> Option<WidgetId> {
        for region in self.hit_regions_fit_chars(bar_width, min_gap) {
            if click_col >= region.col && click_col < region.col + region.width {
                return Some(region.id);
            }
        }
        None
    }
}

// ── D6 Layout API ───────────────────────────────────────────────────────────
//
// Per Decision D6 in `docs/BACKEND_TRAIT_PROPOSAL.md` §9: primitives return
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
        let mut visible_segments: Vec<VisibleStatusSegment> = Vec::new();
        let mut hit_regions: Vec<(Rect, StatusBarHit)> = Vec::new();

        // ── Left segments, left-to-right from column 0 ─────────────────
        let mut cursor = 0.0_f32;
        for (i, seg) in self.left_segments.iter().enumerate() {
            let w = measure(seg).width;
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
            .map(|s| measure(s).width)
            .collect();
        let total_right: f32 = right_widths.iter().sum();
        let max_right = (bar_width - left_w - min_gap).max(0.0);

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

        // Right segments right-aligned inside `bar_width`. Rendered in the
        // natural `right_segments[start..]` order; first visible segment
        // is leftmost of the right group.
        let visible_right = &self.right_segments[resolved_right_start..];
        let visible_right_widths = &right_widths[resolved_right_start..];
        let total_visible: f32 = visible_right_widths.iter().sum();
        let mut cursor = (bar_width - total_visible).max(0.0);
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
