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
