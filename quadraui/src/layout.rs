//! Shared foundation for layout() and hit_test() primitives.
//!
//! This module provides shared infrastructure for primitives to converge on
//! common method shapes and geometry math: `fn layout(&self, bounds: Rect,
//! m: &Self::Measure)`, `fn hit_test(&self, p: Point)`, with bounds in
//! ABSOLUTE (target-surface) coordinates. All returned bounds use the same
//! coordinate frame, eliminating the LOCAL/ABSOLUTE ambiguity tracked in
//! `docs/PRIMITIVE_RULES.md`. A primitive may adopt [`VisibleItem`] and
//! [`Anchor`] internally with no public change at all (its own
//! placement enum and `layout()` signature stay put, mapping onto
//! `Side`/`Anchor` as an implementation detail — `Tooltip`,
//! `RichTextPopup`, `Completions`, and `ContextMenu` all do this).
//! Converting a primitive to the one-argument `fn layout(&self, bounds:
//! Rect, m: &Self::Measure)` method shape itself is the rule-8 breaking
//! change (see `docs/PRIMITIVE_RULES.md`), landing with a
//! `#[deprecated]` shim.
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

use std::collections::BTreeMap;

use crate::event::{Point, Rect};
use serde::{Deserialize, Serialize};

// ── Anchor: shared overlay positioning ──────────────────────────────────

/// Side of an [`Anchor`]'s `rect` that an overlay prefers to open on.
///
/// Unifies four near-identical enums that each primitive invented for
/// the same "which side, with overflow fallback" concept:
/// `TooltipPlacement` (`Top`/`Bottom`/`Left`/`Right`), `PopupPlacement`
/// (`Above`/`Below`), `CompletionsPlacement` (`Below`/`Above`), and
/// `ContextMenuPlacement` (`AnchorPoint`/`Below`/`Above`). Each of those
/// four keeps its own public enum and method signature — consumers never
/// see a `Side` — but `Tooltip::layout`, `RichTextPopup::layout`,
/// `Completions::layout`, and `ContextMenu::layout`/`layout_at` all map
/// their enum to a `Side` and delegate the actual flip/clamp geometry to
/// [`Anchor::resolve`] internally, same as [`Float`](crate::Float).
///
/// `Serialize`/`Deserialize`: [`Float`](crate::Float) is
/// the first primitive to store an [`Anchor`] directly on a
/// serde-describable descriptor (rather than taking one as a transient
/// `layout()` argument the way `Tooltip`/`ContextMenu` do), so this and
/// [`ResolvedSide`]/[`Anchor`] itself need to round-trip through JSON
/// for a Lua-extension-declared float to deserialise cleanly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
/// Provides the logic to try the preferred side; if the box would
/// overflow the viewport, try the opposite side; if both overflow, pin
/// to the viewport edge on the preferred side. See [`Anchor::resolve`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
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

// ── NaturalSize: best-effort intrinsic leaf size ────────────────────────

/// A leaf's best-effort intrinsic `(width, height)` in surface-native
/// units, each `None` when no cheap measurement exists for that axis.
///
/// A layout-tree container's non-stretch cross-axis alignment sizes a
/// child to its own content instead of always filling the cross-axis
/// extent; a `None` on either axis degrades that axis back to filling
/// it — the same "no cheap measurement, don't collapse to zero" rule a
/// consumer applies to every other `Option<f32>` this crate hands it for
/// sizing. A primitive with no single natural size (an `Editor`, a
/// `Terminal`, a calendar grid, a chrome bar meant to span its whole
/// axis) reports [`NaturalSize::NONE`] on the axis it has no answer for,
/// same as leaving a field unset; a primitive with a genuinely compact
/// or fixed-content shape (a checkbox's toggle box, a progress bar's one
/// row, an image's intrinsic pixel size) reports a real value instead.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct NaturalSize {
    pub width: Option<f32>,
    pub height: Option<f32>,
}

impl NaturalSize {
    /// No cheap measurement on either axis — the pre-existing "always
    /// stretch" behaviour for a primitive that never overrides this.
    pub const NONE: Self = Self {
        width: None,
        height: None,
    };

    pub const fn new(width: Option<f32>, height: Option<f32>) -> Self {
        Self { width, height }
    }

    /// The extent along whichever axis is the container's main axis:
    /// width for a horizontal (`Row`) container, height for a vertical
    /// (`Column`) one.
    pub fn main(&self, horizontal: bool) -> Option<f32> {
        if horizontal {
            self.width
        } else {
            self.height
        }
    }

    /// [`Self::main`]'s counterpart on the container's cross axis.
    pub fn cross(&self, horizontal: bool) -> Option<f32> {
        if horizontal {
            self.height
        } else {
            self.width
        }
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
/// Replaces the per-primitive `Visible*{idx, bounds}` structs
/// (`VisibleListItem`, `VisibleTreeRow`, `VisibleTextDisplayLine`,
/// `VisibleTab`, `VisibleMenuBarItem`, ...). A primitive whose
/// visible-item type carries extra per-item data (`VisibleTab::close_bounds`,
/// `VisibleMenuBarItem::clickable`) still benefits from [`visible_range_walk`]
/// for the index/bounds walk itself, zipping its extra fields onto the result.
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

// ── Grid track sizing ────────────────────────────────────────────────────

/// One column or row track's sizing strategy for [`resolve_grid_tracks`].
///
/// Generalises `quadraui::SectionSize`'s `Fixed`/`Percent`/`Weight`/`Content`
/// vocabulary (single-axis, one dimension of sections) to a 2D grid: columns
/// and rows each resolve independently, through the same
/// Fixed+Content → Percent → Weight pass order `resolve_main_axis_sizes`
/// uses for a `LayoutContainer`'s children. No `min`/`max` clamp and no
/// `EqualShare` variant yet — a grid track adds them if a consumer needs
/// them, same as `SectionSize` did.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum GridTrack {
    /// Exact size in the container's native units (cells / pixels).
    Fixed(f32),
    /// Share of the *original* container extent along this axis,
    /// `0.0..=1.0`. Allocated against the container's full extent, not
    /// the post-`Fixed` remainder. On total overflow, every `Percent`
    /// track on that axis scales down proportionally.
    Percent(f32),
    /// Proportional weight (CSS `fr` unit). Shares whatever remains
    /// after every `Fixed`/`Content`/`Percent` track on the same axis
    /// claimed its size. A weight `<= 0.0` is treated as zero.
    Weight(f32),
    /// Exact natural-content size the caller already measured (e.g. a
    /// column's widest cell). Resolved exactly, in the same pass as
    /// `Fixed`.
    Content(f32),
}

/// Three-pass single-axis track resolution shared by [`resolve_grid_tracks`]'s
/// column pass and row pass.
///
/// Pass 1 resolves every `Fixed`/`Content` track exactly. Pass 2 resolves
/// every `Percent` track against `container_extent` (the axis's full
/// extent, before any track consumed part of it); if the percentages
/// overflow what's left after pass 1, every `Percent` track scales down
/// proportionally. Pass 3 splits whatever remains across every `Weight`
/// track by weight ratio; with no `Weight` track, the remainder goes
/// unused. Same pass order as
/// `quadraui_binding_core::layout::resolve_main_axis_sizes`, without that
/// function's `min`/`max` clamp (no such concept on a `GridTrack` today)
/// or its separately-reserved `usable_main` (a grid track has no gap
/// parameter yet — a caller wanting a gutter shrinks the container or
/// inserts a `Fixed` spacer track).
fn resolve_track_sizes(tracks: &[GridTrack], container_extent: f32) -> Vec<f32> {
    let n = tracks.len();
    let mut out = vec![0.0_f32; n];
    let mut done = vec![false; n];
    let mut consumed = 0.0_f32;

    // Pass 1: Fixed + Content.
    for (i, track) in tracks.iter().enumerate() {
        let raw = match track {
            GridTrack::Fixed(v) | GridTrack::Content(v) => *v,
            GridTrack::Percent(_) | GridTrack::Weight(_) => continue,
        };
        let v = raw.max(0.0);
        out[i] = v;
        consumed += v;
        done[i] = true;
    }

    // Pass 2: Percent, against the original container_extent.
    let mut percent_indices = Vec::new();
    let mut percent_raw = Vec::new();
    for (i, track) in tracks.iter().enumerate() {
        if done[i] {
            continue;
        }
        if let GridTrack::Percent(p) = track {
            percent_indices.push(i);
            percent_raw.push(p.clamp(0.0, 1.0) * container_extent);
        }
    }
    let percent_total: f32 = percent_raw.iter().sum();
    let percent_budget = (container_extent - consumed).max(0.0);
    let percent_scale = if percent_total > percent_budget && percent_total > 0.0 {
        percent_budget / percent_total
    } else {
        1.0
    };
    for (k, &i) in percent_indices.iter().enumerate() {
        let v = (percent_raw[k] * percent_scale).max(0.0);
        out[i] = v;
        consumed += v;
        done[i] = true;
    }

    // Pass 3: Weight shares whatever remains.
    let remainder = (container_extent - consumed).max(0.0);
    let mut weight_indices = Vec::new();
    let mut weights = Vec::new();
    for (i, track) in tracks.iter().enumerate() {
        if done[i] {
            continue;
        }
        if let GridTrack::Weight(w) = track {
            weight_indices.push(i);
            weights.push(w.max(0.0));
        }
    }
    let weight_sum: f32 = weights.iter().sum();
    if !weight_indices.is_empty() && weight_sum > 0.0 {
        for (k, &i) in weight_indices.iter().enumerate() {
            out[i] = remainder * (weights[k] / weight_sum);
        }
    }

    out
}

/// Resolves `columns` and `rows` against `container` into a 2D grid of
/// cell [`Rect`]s, indexed `[row][col]` (row-major, matching how a
/// [`resolve_grid_areas`] template string is read).
///
/// Each axis resolves independently through [`resolve_track_sizes`]'s
/// three-pass order (columns against `container.width`, rows against
/// `container.height`); cells are then placed by walking column widths
/// left-to-right and row heights top-to-bottom from `container`'s
/// origin. No gap between tracks — a caller wanting a gutter shrinks
/// `container` or inserts a `Fixed` spacer track into `columns`/`rows`.
pub fn resolve_grid_tracks(
    columns: &[GridTrack],
    rows: &[GridTrack],
    container: Rect,
) -> Vec<Vec<Rect>> {
    let col_sizes = resolve_track_sizes(columns, container.width);
    let row_sizes = resolve_track_sizes(rows, container.height);

    let mut row_y = container.y;
    let mut grid = Vec::with_capacity(row_sizes.len());
    for &row_h in &row_sizes {
        let mut col_x = container.x;
        let mut row_cells = Vec::with_capacity(col_sizes.len());
        for &col_w in &col_sizes {
            row_cells.push(Rect::new(col_x, row_y, col_w, row_h));
            col_x += col_w;
        }
        grid.push(row_cells);
        row_y += row_h;
    }
    grid
}

/// Inclusive row/col index range an area name spans within a
/// `grid-template-areas`-style template, as resolved by
/// [`resolve_grid_areas`]. Indexes into the same `columns`/`rows` axes
/// (and the `[row][col]` grid) [`resolve_grid_tracks`] produces for the
/// matching track lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridAreaSpan {
    /// First row index the area covers.
    pub row_start: usize,
    /// Last row index the area covers, inclusive.
    pub row_end: usize,
    /// First column index the area covers.
    pub col_start: usize,
    /// Last column index the area covers, inclusive.
    pub col_end: usize,
}

/// Resolves a CSS-style `grid-template-areas` template — one `&str` per
/// row, each a whitespace-separated list of area names, with `"."`
/// opting a cell out of every area — into a map from area name to the
/// [`GridAreaSpan`] of cells it spans.
///
/// A name repeated across more than one row/column folds into a single
/// bounding span. CSS requires the cells a name occupies form a solid
/// rectangle; this helper doesn't validate that — a non-rectangular
/// template just resolves to its bounding box, same as if the gaps
/// inside it were filled.
pub fn resolve_grid_areas(template: &[&str]) -> BTreeMap<String, GridAreaSpan> {
    let mut spans: BTreeMap<String, GridAreaSpan> = BTreeMap::new();
    for (row, line) in template.iter().enumerate() {
        for (col, name) in line.split_whitespace().enumerate() {
            if name == "." {
                continue;
            }
            spans
                .entry(name.to_string())
                .and_modify(|s| {
                    s.row_start = s.row_start.min(row);
                    s.row_end = s.row_end.max(row);
                    s.col_start = s.col_start.min(col);
                    s.col_end = s.col_end.max(col);
                })
                .or_insert(GridAreaSpan {
                    row_start: row,
                    row_end: row,
                    col_start: col,
                    col_end: col,
                });
        }
    }
    spans
}

/// Unions the cell [`Rect`]s a [`GridAreaSpan`] covers in `cells` (the
/// `[row][col]` grid [`resolve_grid_tracks`] returned) into the single
/// `Rect` that area occupies. `None` if `span` indexes outside `cells`.
pub fn grid_area_rect(cells: &[Vec<Rect>], span: GridAreaSpan) -> Option<Rect> {
    let top_left = cells.get(span.row_start)?.get(span.col_start)?;
    let bottom_right = cells.get(span.row_end)?.get(span.col_end)?;
    let x = top_left.x;
    let y = top_left.y;
    let width = (bottom_right.x + bottom_right.width) - x;
    let height = (bottom_right.y + bottom_right.height) - y;
    Some(Rect::new(x, y, width, height))
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
    fn anchor_serde_round_trips() {
        // `Float` stores an `Anchor` directly on a serde-describable
        // descriptor, so a Lua-extension-declared float must be able to
        // round-trip one through JSON.
        let anchor = Anchor::new(Rect::new(1.0, 2.0, 3.0, 4.0), Side::Right).with_margin(5.0);
        let json = serde_json::to_string(&anchor).unwrap();
        let back: Anchor = serde_json::from_str(&json).unwrap();
        assert_eq!(anchor, back);
    }

    #[test]
    fn resolve_at_point_never_flips_and_clamps() {
        let anchor = Anchor::at_point(Point::new(98.0, 98.0));
        let (p, side) = anchor.resolve((20.0, 10.0), VIEWPORT);
        assert_eq!(side, ResolvedSide::AtPoint);
        // Clamped so the whole box stays inside the viewport.
        assert_eq!(p, Point::new(80.0, 90.0));
    }

    // ── NaturalSize ───────────────────────────────────────────────────

    #[test]
    fn natural_size_main_cross_pick_the_matching_axis() {
        let size = NaturalSize::new(Some(10.0), Some(20.0));
        assert_eq!(size.main(true), Some(10.0));
        assert_eq!(size.main(false), Some(20.0));
        assert_eq!(size.cross(true), Some(20.0));
        assert_eq!(size.cross(false), Some(10.0));
    }

    #[test]
    fn natural_size_none_has_no_measurement_on_either_axis() {
        assert_eq!(NaturalSize::NONE, NaturalSize::default());
        assert_eq!(NaturalSize::NONE.main(true), None);
        assert_eq!(NaturalSize::NONE.cross(true), None);
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

    // ── Grid track sizing ───────────────────────────────────────────

    const GRID_CONTAINER: Rect = Rect::new(0.0, 0.0, 100.0, 60.0);

    #[test]
    fn grid_tracks_fixed_and_content_resolve_exactly() {
        let cols = [GridTrack::Fixed(20.0), GridTrack::Content(30.0)];
        let rows = [GridTrack::Fixed(10.0), GridTrack::Content(50.0)];
        let grid = resolve_grid_tracks(&cols, &rows, GRID_CONTAINER);

        assert_eq!(grid.len(), 2);
        assert_eq!(grid[0].len(), 2);
        assert_eq!(grid[0][0], Rect::new(0.0, 0.0, 20.0, 10.0));
        assert_eq!(grid[0][1], Rect::new(20.0, 0.0, 30.0, 10.0));
        assert_eq!(grid[1][0], Rect::new(0.0, 10.0, 20.0, 50.0));
        assert_eq!(grid[1][1], Rect::new(20.0, 10.0, 30.0, 50.0));
    }

    #[test]
    fn grid_tracks_percent_is_against_the_original_container() {
        let cols = [GridTrack::Fixed(20.0), GridTrack::Percent(0.5)];
        let rows = [GridTrack::Percent(1.0)];
        let grid = resolve_grid_tracks(&cols, &rows, GRID_CONTAINER);

        // 0.5 * 100.0 (the full container width) == 50.0, independent of
        // the 20.0 the Fixed column already consumed.
        assert_eq!(grid[0][1].width, 50.0);
        assert_eq!(grid[0][0].height, 60.0);
    }

    #[test]
    fn grid_tracks_percent_overflow_scales_down_proportionally() {
        // Two 0.6 Percent columns overflow the container; both scale
        // down by the same factor rather than one starving the other.
        let cols = [GridTrack::Percent(0.6), GridTrack::Percent(0.6)];
        let rows = [GridTrack::Fixed(10.0)];
        let grid = resolve_grid_tracks(&cols, &rows, GRID_CONTAINER);

        assert_eq!(grid[0][0].width, grid[0][1].width);
        assert!((grid[0][0].width + grid[0][1].width - 100.0).abs() < 1e-4);
    }

    #[test]
    fn grid_tracks_weight_shares_the_remainder() {
        let cols = [
            GridTrack::Fixed(20.0),
            GridTrack::Weight(1.0),
            GridTrack::Weight(3.0),
        ];
        let rows = [GridTrack::Fixed(10.0)];
        let grid = resolve_grid_tracks(&cols, &rows, GRID_CONTAINER);

        // Remainder after the Fixed(20.0) column is 80.0, split 1:3.
        assert_eq!(grid[0][1].width, 20.0);
        assert_eq!(grid[0][2].width, 60.0);
    }

    #[test]
    fn grid_tracks_weight_with_no_weighted_track_leaves_remainder_unused() {
        let cols = [GridTrack::Fixed(20.0)];
        let rows = [GridTrack::Fixed(10.0)];
        let grid = resolve_grid_tracks(&cols, &rows, GRID_CONTAINER);

        assert_eq!(grid.len(), 1);
        assert_eq!(grid[0].len(), 1);
        assert_eq!(grid[0][0], Rect::new(0.0, 0.0, 20.0, 10.0));
    }

    #[test]
    fn grid_track_serde_round_trips() {
        for track in [
            GridTrack::Fixed(1.0),
            GridTrack::Percent(0.5),
            GridTrack::Weight(2.0),
            GridTrack::Content(3.0),
        ] {
            let json = serde_json::to_string(&track).unwrap();
            let back: GridTrack = serde_json::from_str(&json).unwrap();
            assert_eq!(track, back);
        }
    }

    #[test]
    fn grid_areas_resolve_named_spans_and_skip_dots() {
        let template = ["header header", "sidebar content", "sidebar footer"];
        let areas = resolve_grid_areas(&template);

        assert_eq!(
            areas["header"],
            GridAreaSpan {
                row_start: 0,
                row_end: 0,
                col_start: 0,
                col_end: 1,
            }
        );
        assert_eq!(
            areas["sidebar"],
            GridAreaSpan {
                row_start: 1,
                row_end: 2,
                col_start: 0,
                col_end: 0,
            }
        );
        assert_eq!(
            areas["content"],
            GridAreaSpan {
                row_start: 1,
                row_end: 1,
                col_start: 1,
                col_end: 1,
            }
        );
        assert_eq!(
            areas["footer"],
            GridAreaSpan {
                row_start: 2,
                row_end: 2,
                col_start: 1,
                col_end: 1,
            }
        );
    }

    #[test]
    fn grid_areas_dot_opts_a_cell_out_of_every_area() {
        let template = [". content", ". content"];
        let areas = resolve_grid_areas(&template);

        assert_eq!(areas.len(), 1);
        assert_eq!(
            areas["content"],
            GridAreaSpan {
                row_start: 0,
                row_end: 1,
                col_start: 1,
                col_end: 1,
            }
        );
    }

    #[test]
    fn grid_area_rect_unions_the_spanned_cells() {
        let cols = [GridTrack::Fixed(20.0), GridTrack::Fixed(30.0)];
        let rows = [GridTrack::Fixed(10.0), GridTrack::Fixed(15.0)];
        let grid = resolve_grid_tracks(&cols, &rows, GRID_CONTAINER);

        let span = GridAreaSpan {
            row_start: 0,
            row_end: 1,
            col_start: 0,
            col_end: 1,
        };
        let rect = grid_area_rect(&grid, span).unwrap();
        assert_eq!(rect, Rect::new(0.0, 0.0, 50.0, 25.0));
    }

    #[test]
    fn grid_area_rect_out_of_bounds_span_is_none() {
        let cols = [GridTrack::Fixed(20.0)];
        let rows = [GridTrack::Fixed(10.0)];
        let grid = resolve_grid_tracks(&cols, &rows, GRID_CONTAINER);

        let span = GridAreaSpan {
            row_start: 0,
            row_end: 5,
            col_start: 0,
            col_end: 0,
        };
        assert!(grid_area_rect(&grid, span).is_none());
    }
}
