//! Shared foundation for the layout()/hit_test() convergence (issue #816).
//!
//! Today every primitive invents its own `layout()` argument shape (four
//! families: `(bounds, measure)`, `(origin_x, origin_y, measure)`,
//! `(viewport, line_height)`, `(bounds, lines_per_row)`, ...), its own
//! hit-test shape (`(x, y) -> Hit`, `(x, y, id) -> Hit`, `(point,
//! tolerance) -> Option<usize>`, or nothing at all — seven primitives
//! have no `hit_test` today), and its own coordinate frame (LOCAL or
//! ABSOLUTE, tracked in a table in `docs/PRIMITIVE_RULES.md` because the
//! two are not interchangeable at a call site). On top of that, 19
//! primitives each define a near-identical `Visible*{idx, bounds}`
//! struct and re-walk "which items are visible from this scroll offset"
//! by hand.
//!
//! This module is the shared *infrastructure* that per-primitive
//! convergence PRs adopt one at a time — it does not, by itself, change
//! any existing primitive's public API. Converting a primitive to return
//! [`VisibleItem`] instead of its bespoke `Visible*` struct, or to
//! position itself via [`Anchor`] instead of its own placement enum, is
//! a rule-8 breaking change (see `docs/PRIMITIVE_RULES.md`) and lands as
//! its own PR with a `#[deprecated]` shim so no consumer breaks in one
//! commit. See issue #816 for the full per-primitive split and its
//! coordination with quadraui#785's Phase 2.
//!
//! ## Target convention (for primitives converging onto this module)
//!
//! - `fn layout(&self, bounds: Rect, m: &Self::Measure) -> Self::Layout`
//!   — one argument shape, replacing the four in use today.
//! - `fn hit_test(&self, p: Point) -> Self::Hit`, with a `Miss` (or
//!   equivalent empty) variant on every `Hit` enum — no primitive
//!   returns `Option<T>` or omits `hit_test` entirely once converged.
//! - Every returned bound is in **ABSOLUTE** (target-surface)
//!   coordinates. Once every primitive has converged, the LOCAL/ABSOLUTE
//!   table in `docs/PRIMITIVE_RULES.md` is deleted — a coordinate frame
//!   that is always the same frame needs no table.
//!
//! This module does not force those two method shapes into a Rust
//! trait: `Measure`/`Layout`/`Hit` differ per primitive by design, and a
//! trait bound gains nothing that a doc-comment convention plus review
//! doesn't already give the per-primitive PRs landing this. What *is*
//! shared and worth centralising is the geometry math underneath both
//! methods — anchoring an overlay against a viewport, and walking a
//! scrollable range of variable-size items — which is what the rest of
//! this module provides.

use crate::event::{Point, Rect};

// ── Anchor: shared overlay positioning ──────────────────────────────────

/// Side of an [`Anchor`]'s `rect` that an overlay prefers to open on.
///
/// Unifies four near-identical enums that each primitive invented for
/// the same "which side, with overflow fallback" concept:
/// `TooltipPlacement` (`Top`/`Bottom`/`Left`/`Right`), `PopupPlacement`
/// (`Above`/`Below`), `CompletionsPlacement` (`Below`/`Above`), and
/// `ContextMenuPlacement` (`AnchorPoint`/`Below`/`Above`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Side {
    Top,
    #[default]
    Bottom,
    Left,
    Right,
    /// No preferred side — the anchor rect already *is* the desired
    /// point (e.g. a right-click cursor position). [`Anchor::resolve`]
    /// never flips an `AtPoint` anchor; it only clamps to the viewport.
    AtPoint,
}

/// Which [`Side`] an overlay actually resolved to, after
/// [`Anchor::resolve`] applied the overflow-flip fallback. Same
/// variants as [`Side`] minus the "preference" framing — this is the
/// resolved fact, not the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedSide {
    Top,
    Bottom,
    Left,
    Right,
    AtPoint,
}

/// Common shape for every overlay primitive that positions a box
/// relative to an anchor rect (or point) inside a containing viewport:
/// `Tooltip`, `Completions`, `ContextMenu`, `RichTextPopup`, `Dialog`.
///
/// Each of those primitives re-implements the same "try the preferred
/// side; if the box would overflow the viewport, try the opposite side;
/// if both overflow, pin to the viewport edge on the preferred side"
/// resolution today (see e.g. `TooltipView::layout`'s "Placement
/// fallback" doc). `Anchor::resolve` is that logic, written once.
///
/// Converting an existing primitive onto `Anchor` is a rule-8 breaking
/// change (its own PR, `#[deprecated]` shim on the old placement enum) —
/// see issue #816.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    /// Bounds of the element the overlay is positioned against, in
    /// ABSOLUTE (target-surface) coordinates. A cursor-position anchor
    /// (right-click context menu) is a zero-size `Rect` at that point.
    pub rect: Rect,
    /// Side of `rect` the overlay prefers to open on.
    pub preferred: Side,
    /// Gap between `rect` and the overlay along the placement axis.
    /// Ignored when `preferred` is [`Side::AtPoint`].
    pub margin: f32,
}

impl Anchor {
    /// An anchor at a rect, preferring `preferred`, with no margin.
    pub const fn new(rect: Rect, preferred: Side) -> Self {
        Self {
            rect,
            preferred,
            margin: 0.0,
        }
    }

    /// An anchor at a single point (e.g. the mouse cursor on
    /// right-click) that never flips.
    pub const fn at_point(p: Point) -> Self {
        Self {
            rect: Rect::new(p.x, p.y, 0.0, 0.0),
            preferred: Side::AtPoint,
            margin: 0.0,
        }
    }

    /// Builder: set the gap between the anchor and the resolved overlay.
    pub const fn with_margin(mut self, margin: f32) -> Self {
        self.margin = margin;
        self
    }

    /// Resolve the top-left position for an overlay box of `size`
    /// (width, height), clamped to stay inside `viewport`.
    ///
    /// # Placement fallback
    ///
    /// The preferred side is tried first. If the box would extend past
    /// a `viewport` edge, the opposite side is tried. If both overflow
    /// (an anchor near the middle of a viewport too small for the box),
    /// the box is pinned to the viewport edge on the preferred axis
    /// while still centering on the anchor along the cross axis.
    /// [`Side::AtPoint`] never flips: the box opens at `rect`'s origin,
    /// clamped to stay inside `viewport`.
    pub fn resolve(&self, size: (f32, f32), viewport: Rect) -> (Point, ResolvedSide) {
        let (w, h) = size;
        let r = self.rect;

        if self.preferred == Side::AtPoint {
            let x = r.x.clamp(
                viewport.x,
                (viewport.x + viewport.width - w).max(viewport.x),
            );
            let y = r.y.clamp(
                viewport.y,
                (viewport.y + viewport.height - h).max(viewport.y),
            );
            return (Point::new(x, y), ResolvedSide::AtPoint);
        }

        let candidate = |side: Side| -> (f32, f32) {
            match side {
                Side::Top => (r.x + (r.width - w) * 0.5, r.y - self.margin - h),
                Side::Bottom => (r.x + (r.width - w) * 0.5, r.y + r.height + self.margin),
                Side::Left => (r.x - self.margin - w, r.y + (r.height - h) * 0.5),
                Side::Right => (r.x + r.width + self.margin, r.y + (r.height - h) * 0.5),
                Side::AtPoint => (r.x, r.y),
            }
        };

        let fits = |x: f32, y: f32| -> bool {
            x >= viewport.x
                && y >= viewport.y
                && x + w <= viewport.x + viewport.width
                && y + h <= viewport.y + viewport.height
        };

        let opposite = |side: Side| -> Side {
            match side {
                Side::Top => Side::Bottom,
                Side::Bottom => Side::Top,
                Side::Left => Side::Right,
                Side::Right => Side::Left,
                Side::AtPoint => Side::AtPoint,
            }
        };
        let resolved_of = |side: Side| -> ResolvedSide {
            match side {
                Side::Top => ResolvedSide::Top,
                Side::Bottom => ResolvedSide::Bottom,
                Side::Left => ResolvedSide::Left,
                Side::Right => ResolvedSide::Right,
                Side::AtPoint => ResolvedSide::AtPoint,
            }
        };

        let (px, py) = candidate(self.preferred);
        if fits(px, py) {
            return (Point::new(px, py), resolved_of(self.preferred));
        }

        let flipped = opposite(self.preferred);
        let (fx, fy) = candidate(flipped);
        if fits(fx, fy) {
            return (Point::new(fx, fy), resolved_of(flipped));
        }

        // Neither side fits: pin to the viewport edge on the preferred
        // side, clamping the cross axis so the box stays fully visible.
        let (mut x, mut y) = (px, py);
        x = x.clamp(
            viewport.x,
            (viewport.x + viewport.width - w).max(viewport.x),
        );
        y = y.clamp(
            viewport.y,
            (viewport.y + viewport.height - h).max(viewport.y),
        );
        (Point::new(x, y), resolved_of(self.preferred))
    }
}

// ── Shared visible-range walk ────────────────────────────────────────────

/// Axis a [`visible_range_walk`] lays items out along.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Items stack top-to-bottom; `measure` returns each item's height.
    Vertical,
    /// Items stack left-to-right; `measure` returns each item's width.
    Horizontal,
}

/// One entry produced by [`visible_range_walk`]: the absolute index of a
/// visible item (into the caller's backing `Vec`, *not* a position
/// within the visible slice) and its painted bounds in ABSOLUTE
/// coordinates.
///
/// Replaces the shape every one of the 19 `Visible*{idx, bounds}`
/// structs shares today (`VisibleListItem`, `VisibleTreeRow`,
/// `VisibleTextDisplayLine`, `VisibleTab`, `VisibleMenuBarItem`, ...).
/// A primitive whose visible-item type carries extra per-item data
/// (`VisibleTab::close_bounds`, `VisibleMenuBarItem::clickable`) still
/// benefits from `visible_range_walk` for the index/bounds walk itself,
/// zipping its extra fields onto the result — see issue #816's
/// per-primitive conversion notes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleItem {
    pub idx: usize,
    pub bounds: Rect,
}

/// Walk `first_idx..count`, laying each item out along `axis` starting
/// at `origin`, until `available` (the viewport's remaining extent
/// along the walk axis) is exhausted. `cross` is the fixed extent
/// perpendicular to the walk axis (item width for a vertical list, item
/// height for a horizontal one). `measure(idx)` returns that item's
/// extent along the walk axis.
///
/// The last visible item is clipped to the remaining space, matching
/// every existing hand-written walk (`ListView::layout`,
/// `TreeView::layout`, ...): its `bounds` extent is `measure(idx).min(remaining)`,
/// but the walk still advances by the item's *full* (unclipped) extent,
/// so a zero-or-negative remainder on the next item naturally ends the
/// walk. An item whose clipped extent is `<= 0.0` is not emitted and
/// ends the walk (there is no space left to paint it).
///
/// `scroll_offset`/clamping is the caller's responsibility — pass the
/// already-clamped `first_idx` (see
/// `primitives::scrollbar::clamp_scroll_offset`).
pub fn visible_range_walk(
    first_idx: usize,
    count: usize,
    origin: Point,
    axis: Axis,
    cross: f32,
    available: f32,
    mut measure: impl FnMut(usize) -> f32,
) -> Vec<VisibleItem> {
    let mut out = Vec::new();
    let mut pos = 0.0f32;

    for idx in first_idx..count {
        if pos >= available {
            break;
        }
        let extent = measure(idx);
        let remaining = available - pos;
        let clipped = extent.min(remaining).max(0.0);
        if clipped <= 0.0 {
            break;
        }
        let bounds = match axis {
            Axis::Vertical => Rect::new(origin.x, origin.y + pos, cross, clipped),
            Axis::Horizontal => Rect::new(origin.x + pos, origin.y, clipped, cross),
        };
        out.push(VisibleItem { idx, bounds });
        pos += extent;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Anchor ───────────────────────────────────────────────────────

    const VIEWPORT: Rect = Rect::new(0.0, 0.0, 100.0, 100.0);

    #[test]
    fn resolve_preferred_side_when_it_fits() {
        let anchor = Anchor::new(Rect::new(40.0, 40.0, 10.0, 10.0), Side::Bottom).with_margin(2.0);
        let (p, side) = anchor.resolve((20.0, 10.0), VIEWPORT);
        assert_eq!(side, ResolvedSide::Bottom);
        assert_eq!(p, Point::new(35.0, 52.0));
    }

    #[test]
    fn resolve_flips_to_opposite_side_on_overflow() {
        // Anchor near the bottom edge: preferred Bottom would overflow,
        // so it should flip to Top.
        let anchor = Anchor::new(Rect::new(40.0, 95.0, 10.0, 5.0), Side::Bottom).with_margin(1.0);
        let (p, side) = anchor.resolve((20.0, 10.0), VIEWPORT);
        assert_eq!(side, ResolvedSide::Top);
        assert_eq!(p, Point::new(35.0, 84.0));
    }

    #[test]
    fn resolve_pins_to_edge_when_both_sides_overflow() {
        // Viewport too short for the box on either Top or Bottom.
        let short_viewport = Rect::new(0.0, 0.0, 100.0, 12.0);
        let anchor = Anchor::new(Rect::new(40.0, 4.0, 10.0, 4.0), Side::Bottom).with_margin(1.0);
        let (p, side) = anchor.resolve((20.0, 10.0), short_viewport);
        // Preferred side reported even though it had to be clamped.
        assert_eq!(side, ResolvedSide::Bottom);
        assert!(p.y >= short_viewport.y);
        assert!(p.y + 10.0 <= short_viewport.y + short_viewport.height);
    }

    #[test]
    fn resolve_at_point_never_flips_and_clamps() {
        let anchor = Anchor::at_point(Point::new(98.0, 98.0));
        let (p, side) = anchor.resolve((20.0, 10.0), VIEWPORT);
        assert_eq!(side, ResolvedSide::AtPoint);
        // Clamped so the whole box stays inside the viewport.
        assert_eq!(p, Point::new(80.0, 90.0));
    }

    // ── visible_range_walk ──────────────────────────────────────────

    #[test]
    fn vertical_walk_stops_when_viewport_is_full() {
        // Ten items of height 10 in a viewport that only fits 5.
        let items = visible_range_walk(
            0,
            10,
            Point::new(0.0, 0.0),
            Axis::Vertical,
            30.0,
            50.0,
            |_| 10.0,
        );
        assert_eq!(items.len(), 5);
        assert_eq!(items[0].idx, 0);
        assert_eq!(items[0].bounds, Rect::new(0.0, 0.0, 30.0, 10.0));
        assert_eq!(items[4].idx, 4);
        assert_eq!(items[4].bounds, Rect::new(0.0, 40.0, 30.0, 10.0));
    }

    #[test]
    fn vertical_walk_starts_at_first_idx() {
        // Scrolled down by 3: only items 3.. are visible.
        let items = visible_range_walk(
            3,
            10,
            Point::new(0.0, 0.0),
            Axis::Vertical,
            30.0,
            25.0,
            |_| 10.0,
        );
        let idxs: Vec<usize> = items.iter().map(|v| v.idx).collect();
        // idx 5 starts at pos 20 with remaining 5 < its own height (10),
        // so it's clipped to height 5 rather than dropped — same
        // "clip, don't drop, the last partially-visible item" behaviour
        // as `ListView::layout`.
        assert_eq!(idxs, vec![3, 4, 5]);
        assert_eq!(items.last().unwrap().bounds.height, 5.0);
    }

    #[test]
    fn vertical_walk_stops_at_count_not_just_available_space() {
        // Only 3 items exist even though the viewport could fit more.
        let items = visible_range_walk(
            0,
            3,
            Point::new(0.0, 0.0),
            Axis::Vertical,
            30.0,
            100.0,
            |_| 10.0,
        );
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn horizontal_walk_lays_out_along_x() {
        let items = visible_range_walk(
            0,
            5,
            Point::new(10.0, 0.0),
            Axis::Horizontal,
            8.0,
            25.0,
            |_| 10.0,
        );
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].bounds, Rect::new(10.0, 0.0, 10.0, 8.0));
        assert_eq!(items[1].bounds, Rect::new(20.0, 0.0, 10.0, 8.0));
        // Third item clipped to the remaining 5.0.
        assert_eq!(items[2].bounds, Rect::new(30.0, 0.0, 5.0, 8.0));
    }

    #[test]
    fn walk_at_nonzero_origin_is_absolute_not_local() {
        let items = visible_range_walk(
            0,
            1,
            Point::new(7.0, 9.0),
            Axis::Vertical,
            30.0,
            10.0,
            |_| 10.0,
        );
        assert_eq!(items[0].bounds, Rect::new(7.0, 9.0, 30.0, 10.0));
    }

    #[test]
    fn walk_skips_zero_extent_items_and_stops() {
        let items = visible_range_walk(
            0,
            3,
            Point::new(0.0, 0.0),
            Axis::Vertical,
            10.0,
            50.0,
            |_| 0.0,
        );
        assert!(items.is_empty());
    }

    #[test]
    fn walk_variable_height_items_via_closure() {
        let heights = [5.0, 15.0, 8.0];
        let items = visible_range_walk(
            0,
            heights.len(),
            Point::new(0.0, 0.0),
            Axis::Vertical,
            30.0,
            100.0,
            |idx| heights[idx],
        );
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].bounds.y, 0.0);
        assert_eq!(items[1].bounds.y, 5.0);
        assert_eq!(items[2].bounds.y, 20.0);
        assert_eq!(items[2].bounds.height, 8.0);
    }
}
