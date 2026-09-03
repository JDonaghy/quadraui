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
    fn status_bar_hit_regions() {
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
        let regions = bar.hit_regions(30);
        assert_eq!(regions.len(), 2);
        // Left starts at col 0, width 6
        assert_eq!(regions[0].col, 0);
        assert_eq!(regions[0].width, 6);
        assert_eq!(regions[0].id.as_str(), "left");
        // Right starts at col 27, width 3
        assert_eq!(regions[1].col, 27);
        assert_eq!(regions[1].width, 3);
        assert_eq!(regions[1].id.as_str(), "right");

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

    #[test]
    fn status_bar_resolve_click_fit_chars_skips_dropped() {
        let mk = |text: &str, id: &str| StatusBarSegment {
            text: text.to_string(),
            fg: Color::rgb(0, 0, 0),
            bg: Color::rgb(0, 0, 0),
            bold: false,
            action_id: Some(WidgetId::new(id)),
        };
        let bar = StatusBar {
            id: WidgetId::new("t"),
            left_segments: vec![mk(" L ", "left")],
            right_segments: vec![mk(" drop ", "drop"), mk(" keep ", "keep")],
        };

        // bar_width 20 fits both on the right (3+12=15 <= 20-0=20 with gap 2): left_w=3, gap=2, total_r=12, 3+2+12=17 <= 20.
        // No drop; keep starts at col 14 (20-6), drop at col 8 (20-12).
        assert_eq!(
            bar.resolve_click_fit_chars(10, 20, 2)
                .as_ref()
                .map(|w| w.as_str()),
            Some("drop")
        );

        // Narrow bar: 3 + 2 + 12 = 17 > 15. Drop " drop " (6). Remaining " keep " (6) fits (3+2+6=11<=15).
        // Now visible right: just "keep" at col 15-6=9.
        // Click at col 10 → hits "keep".
        assert_eq!(
            bar.resolve_click_fit_chars(10, 15, 2)
                .as_ref()
                .map(|w| w.as_str()),
            Some("keep")
        );
        // Click at col 3 (where "drop" used to be) → no segment.
        assert_eq!(bar.resolve_click_fit_chars(3, 15, 2), None);
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
}
