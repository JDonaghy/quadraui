//! `SplitTree` primitive: an N-way recursive tree of `Split`-style
//! dividers. Generalises [`crate::Split`] (a single two-pane divider)
//! to arbitrary nesting — vimcode's editor-group splits and in-group
//! vim window splits (`GroupLayout` / `WindowLayout` in
//! `vimcode/src/core/window.rs`) are both instances of this shape
//! (issue #435).
//!
//! # Relationship to `Split`
//!
//! [`crate::Split`] stays the primitive for a single two-pane boundary
//! — most consumers (sidebar/editor, diff view) only ever need one.
//! `SplitTree` is for hosts that need arbitrary nesting with per-node
//! ratio state addressed by a stable pre-order index. Per
//! `docs/DECISIONS.md`'s "one primitive per UX concept, not per
//! algebraic reduction" principle this is a distinct primitive, not a
//! `Split` with a recursion flag — leaf identity, ratio mutation by
//! index, and parent lookup have no analogue in `Split`.
//!
//! # `SplitDirection` note for vimcode adopters
//!
//! `SplitTree` reuses [`crate::primitives::split::SplitDirection`] for
//! internal consistency with `Split` (same crate, same mental model).
//! vimcode's own `core::window::SplitDirection` has the **opposite**
//! meaning (`Horizontal` = split top/bottom there, vs. side-by-side
//! here) — mapping that on adoption is vimcode's job, not a bug here.
//!
//! # Backend contract
//!
//! **Descriptor + layout only — no default rasteriser opinion beyond
//! divider chrome.** Like `Split`, `SplitTree` describes leaf rects and
//! divider geometry; hosts paint leaf content into the returned leaf
//! rects. Backends draw only the divider lines, using the same visual
//! convention as `Split` (see `tui::draw_split_tree` / `gtk::draw_split_tree`).
//!
//! # One source of truth (avoiding the vimcode drift bug class)
//!
//! vimcode's `GroupLayout` computes leaf rects and divider geometry via
//! two *separate* recursive functions (`calculate_group_rects` and
//! `dividers`) that each re-derive the same split math independently —
//! a latent second-source-of-truth risk. [`SplitTree::layout`] computes
//! both in a single recursive pass so they structurally cannot diverge.
//!
//! Similarly, vimcode issue #582/#452 was a paint/click drift bug: the
//! renderer truncated a divider's float position with `as u16` while a
//! hand-rolled click handler used `.round()`, landing them on different
//! cells. [`SplitTreeDivider::cell_position`] is the single conversion
//! both a TUI rasteriser's paint call and [`SplitTreeLayout::hit_test_divider_cell`]
//! must use — see their doc comments.

use crate::event::{Point, Rect};
use crate::types::WidgetId;
use serde::{Deserialize, Serialize};

pub use crate::primitives::split::SplitDirection;

/// Ratio range enforced by [`SplitTree::set_ratio_at_index`],
/// [`SplitTree::adjust_ratio_at_index`], and
/// [`SplitTree::set_all_ratios`] — mirrors vimcode's `GroupLayout` /
/// `WindowLayout` convention of never letting a pane collapse to
/// zero-width. Hosts that need pixel-precise minimums (like
/// [`crate::Split::first_min`] / [`crate::Split::second_min`]) should
/// clamp further downstream using the resolved leaf rects from
/// [`SplitTree::layout`].
pub const MIN_RATIO: f32 = 0.1;
/// See [`MIN_RATIO`].
pub const MAX_RATIO: f32 = 0.9;

/// Declarative description of an N-way recursive split tree. A `Leaf`
/// carries the [`WidgetId`] of whatever the host is arranging (an
/// editor group, a vim window, a panel — anything). A `Split` node
/// divides its bounds into `first` / `second` sub-trees along
/// `direction` at `ratio` (fraction of the container's cross-axis
/// length given to `first`, `0.0..=1.0`), same convention as
/// [`crate::Split::ratio`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SplitTree {
    /// A single leaf — no further nesting at this position.
    Leaf(WidgetId),
    /// A split containing two sub-trees.
    Split {
        direction: SplitDirection,
        /// Ratio of space given to `first` (`0.0..=1.0`).
        ratio: f32,
        first: Box<SplitTree>,
        second: Box<SplitTree>,
    },
}

impl SplitTree {
    /// Create a new leaf tree with a single widget.
    pub fn leaf(id: WidgetId) -> Self {
        SplitTree::Leaf(id)
    }

    /// Create a new split tree from two sub-trees.
    pub fn split(
        direction: SplitDirection,
        ratio: f32,
        first: SplitTree,
        second: SplitTree,
    ) -> Self {
        SplitTree::Split {
            direction,
            ratio,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    /// True if this node is a leaf (no nesting).
    pub fn is_leaf(&self) -> bool {
        matches!(self, SplitTree::Leaf(_))
    }

    /// Leaf ids in pre-order (left-to-right / top-to-bottom).
    pub fn leaf_ids(&self) -> Vec<WidgetId> {
        let mut out = Vec::new();
        self.collect_leaf_ids(&mut out);
        out
    }

    fn collect_leaf_ids(&self, out: &mut Vec<WidgetId>) {
        match self {
            SplitTree::Leaf(id) => out.push(id.clone()),
            SplitTree::Split { first, second, .. } => {
                first.collect_leaf_ids(out);
                second.collect_leaf_ids(out);
            }
        }
    }

    /// Count the number of leaves in the tree.
    pub fn leaf_count(&self) -> usize {
        match self {
            SplitTree::Leaf(_) => 1,
            SplitTree::Split { first, second, .. } => first.leaf_count() + second.leaf_count(),
        }
    }

    /// Count the number of internal `Split` nodes in the tree (i.e. the
    /// number of dividers `layout` will produce).
    pub fn split_count(&self) -> usize {
        match self {
            SplitTree::Leaf(_) => 0,
            SplitTree::Split { first, second, .. } => {
                1 + first.split_count() + second.split_count()
            }
        }
    }

    /// Compute leaf rects + divider geometry in one pre-order pass —
    /// the single source of truth both rasterisers and hosts' hit-test
    /// calls consume, so paint and click can never read a leaf rect or
    /// divider position derived from two different formulas (see
    /// module docs).
    pub fn layout(&self, bounds: Rect, measure: SplitTreeMeasure) -> SplitTreeLayout {
        let mut leaves = Vec::new();
        let mut dividers = Vec::new();
        let mut counter = 0usize;
        self.layout_into(bounds, measure, &mut counter, &mut leaves, &mut dividers);
        SplitTreeLayout {
            bounds,
            leaves,
            dividers,
        }
    }

    fn layout_into(
        &self,
        bounds: Rect,
        measure: SplitTreeMeasure,
        counter: &mut usize,
        leaves: &mut Vec<(WidgetId, Rect)>,
        dividers: &mut Vec<SplitTreeDivider>,
    ) {
        match self {
            SplitTree::Leaf(id) => leaves.push((id.clone(), bounds)),
            SplitTree::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let idx = *counter;
                *counter += 1;
                let clamped = ratio.clamp(0.0, 1.0);
                let thickness = measure.divider_thickness;

                let (first_bounds, second_bounds, divider) = match direction {
                    SplitDirection::Horizontal => {
                        // Side-by-side (first = left, second = right) —
                        // matches Split::layout's Horizontal case.
                        let available = (bounds.width - thickness).max(0.0);
                        let first_w = available * clamped;
                        let position = bounds.x + first_w;
                        let divider = SplitTreeDivider {
                            split_index: idx,
                            direction: *direction,
                            position,
                            axis_start: bounds.x,
                            axis_size: bounds.width,
                            cross_start: bounds.y,
                            cross_size: bounds.height,
                            thickness,
                        };
                        let first_bounds = Rect::new(bounds.x, bounds.y, first_w, bounds.height);
                        let second_bounds = Rect::new(
                            position + thickness,
                            bounds.y,
                            (bounds.width - first_w - thickness).max(0.0),
                            bounds.height,
                        );
                        (first_bounds, second_bounds, divider)
                    }
                    SplitDirection::Vertical => {
                        // Stacked (first = top, second = bottom) —
                        // matches Split::layout's Vertical case.
                        let available = (bounds.height - thickness).max(0.0);
                        let first_h = available * clamped;
                        let position = bounds.y + first_h;
                        let divider = SplitTreeDivider {
                            split_index: idx,
                            direction: *direction,
                            position,
                            axis_start: bounds.y,
                            axis_size: bounds.height,
                            cross_start: bounds.x,
                            cross_size: bounds.width,
                            thickness,
                        };
                        let first_bounds = Rect::new(bounds.x, bounds.y, bounds.width, first_h);
                        let second_bounds = Rect::new(
                            bounds.x,
                            position + thickness,
                            bounds.width,
                            (bounds.height - first_h - thickness).max(0.0),
                        );
                        (first_bounds, second_bounds, divider)
                    }
                };

                dividers.push(divider);
                first.layout_into(first_bounds, measure, counter, leaves, dividers);
                second.layout_into(second_bounds, measure, counter, leaves, dividers);
            }
        }
    }

    /// Find the Nth split node in pre-order and set its ratio (clamped
    /// to [`MIN_RATIO`]..=[`MAX_RATIO`]). Returns `true` if found.
    pub fn set_ratio_at_index(&mut self, split_index: usize, ratio: f32) -> bool {
        self.set_ratio_at_index_impl(split_index, ratio, &mut 0)
    }

    fn set_ratio_at_index_impl(&mut self, target: usize, ratio: f32, counter: &mut usize) -> bool {
        match self {
            SplitTree::Leaf(_) => false,
            SplitTree::Split {
                ratio: r,
                first,
                second,
                ..
            } => {
                let idx = *counter;
                *counter += 1;
                if idx == target {
                    *r = ratio.clamp(MIN_RATIO, MAX_RATIO);
                    return true;
                }
                first.set_ratio_at_index_impl(target, ratio, counter)
                    || second.set_ratio_at_index_impl(target, ratio, counter)
            }
        }
    }

    /// Find the Nth split node in pre-order and adjust its ratio by
    /// `delta` (clamped to [`MIN_RATIO`]..=[`MAX_RATIO`]). Returns
    /// `true` if found.
    pub fn adjust_ratio_at_index(&mut self, split_index: usize, delta: f32) -> bool {
        self.adjust_ratio_at_index_impl(split_index, delta, &mut 0)
    }

    fn adjust_ratio_at_index_impl(
        &mut self,
        target: usize,
        delta: f32,
        counter: &mut usize,
    ) -> bool {
        match self {
            SplitTree::Leaf(_) => false,
            SplitTree::Split {
                ratio,
                first,
                second,
                ..
            } => {
                let idx = *counter;
                *counter += 1;
                if idx == target {
                    *ratio = (*ratio + delta).clamp(MIN_RATIO, MAX_RATIO);
                    return true;
                }
                first.adjust_ratio_at_index_impl(target, delta, counter)
                    || second.adjust_ratio_at_index_impl(target, delta, counter)
            }
        }
    }

    /// Set every split ratio in the tree to the given value (clamped),
    /// e.g. for an "equalize splits" command.
    pub fn set_all_ratios(&mut self, ratio: f32) {
        match self {
            SplitTree::Leaf(_) => {}
            SplitTree::Split {
                ratio: r,
                first,
                second,
                ..
            } => {
                *r = ratio.clamp(MIN_RATIO, MAX_RATIO);
                first.set_all_ratios(ratio);
                second.set_all_ratios(ratio);
            }
        }
    }

    /// Find the parent split of a leaf. Returns
    /// `(split_index, direction, is_first_child)`.
    pub fn parent_split_of(&self, target: &WidgetId) -> Option<(usize, SplitDirection, bool)> {
        self.parent_split_of_impl(target, &mut 0)
    }

    /// Find the Nth split node in pre-order and return its current
    /// ratio. Mainly useful for tests/introspection — hosts driving a
    /// UI normally consume ratios via [`SplitTree::layout`] instead.
    pub fn ratio_at_index(&self, split_index: usize) -> Option<f32> {
        self.ratio_at_index_impl(split_index, &mut 0)
    }

    fn ratio_at_index_impl(&self, target: usize, counter: &mut usize) -> Option<f32> {
        match self {
            SplitTree::Leaf(_) => None,
            SplitTree::Split {
                ratio,
                first,
                second,
                ..
            } => {
                let idx = *counter;
                *counter += 1;
                if idx == target {
                    return Some(*ratio);
                }
                first
                    .ratio_at_index_impl(target, counter)
                    .or_else(|| second.ratio_at_index_impl(target, counter))
            }
        }
    }

    fn parent_split_of_impl(
        &self,
        target: &WidgetId,
        counter: &mut usize,
    ) -> Option<(usize, SplitDirection, bool)> {
        match self {
            SplitTree::Leaf(_) => None,
            SplitTree::Split {
                direction,
                first,
                second,
                ..
            } => {
                let idx = *counter;
                *counter += 1;
                if let SplitTree::Leaf(id) = first.as_ref() {
                    if id == target {
                        return Some((idx, *direction, true));
                    }
                }
                if let SplitTree::Leaf(id) = second.as_ref() {
                    if id == target {
                        return Some((idx, *direction, false));
                    }
                }
                first
                    .parent_split_of_impl(target, counter)
                    .or_else(|| second.parent_split_of_impl(target, counter))
            }
        }
    }
}

/// Divider dimensions — mirrors [`crate::primitives::split::SplitMeasure`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SplitTreeMeasure {
    /// Thickness of each divider along its split axis (e.g. 1 cell in
    /// TUI, 4–6 px in GTK/macOS).
    pub divider_thickness: f32,
}

impl SplitTreeMeasure {
    pub fn new(divider_thickness: f32) -> Self {
        Self { divider_thickness }
    }
}

/// Geometry of one internal `Split` node's divider, resolved against a
/// concrete bounds rect. Pre-order `split_index` matches
/// [`SplitTree::set_ratio_at_index`] / [`SplitTree::adjust_ratio_at_index`]
/// / [`SplitTree::parent_split_of`] addressing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SplitTreeDivider {
    /// Pre-order index of the `Split` node in the tree.
    pub split_index: usize,
    /// Direction of this split.
    pub direction: SplitDirection,
    /// Divider position along the split axis (x for `Horizontal`, y for
    /// `Vertical`) — the resolved, unrounded position. Backends snap it
    /// to their native units when painting; see [`Self::cell_position`]
    /// for the TUI convention.
    pub position: f32,
    /// Start of the *parent* rect along the split axis. Combine with
    /// `axis_size` to convert a drag cursor position back to a ratio:
    /// `ratio = (cursor_axis - axis_start) / axis_size`. This is
    /// exactly what [`crate::DragTarget::SplitDivider`] carries so a
    /// drag can be resumed without re-running `layout()` on every
    /// mouse-move.
    pub axis_start: f32,
    /// Size of the *parent* rect along the split axis. See `axis_start`.
    pub axis_size: f32,
    /// Start of the divider line along the cross axis.
    pub cross_start: f32,
    /// Length of the divider line along the cross axis.
    pub cross_size: f32,
    /// Divider thickness along the split axis (native units). The
    /// paintable/hit-testable band is `[position, position + thickness)`.
    pub thickness: f32,
}

impl SplitTreeDivider {
    /// TUI-native cell coordinate this divider paints at. Truncates
    /// toward zero — the same conversion ratatui-backed rasterisers use
    /// for cell coordinates (`as u16`). Call this from BOTH paint and
    /// hit-test so they can never diverge: this is the structural fix
    /// for the bug class where an independently hand-rolled click
    /// handler used `.round()` while the renderer used `as u16`
    /// truncation, landing one cell apart (vimcode #582 / #452).
    pub fn cell_position(&self) -> u16 {
        self.position.max(0.0) as u16
    }

    /// True if `axis_point` (native units, same axis as `position`)
    /// falls within a symmetric `tolerance` band around this divider's
    /// paintable band `[position, position + thickness)` — the
    /// GTK/macOS pixel-backend convention (a few px of forgiveness
    /// either side of the exact divider line; exact-hit-only is
    /// unusable at pixel precision).
    pub fn hit_tolerant(&self, axis_point: f32, tolerance: f32) -> bool {
        axis_point >= self.position - tolerance
            && axis_point < self.position + self.thickness + tolerance
    }

    /// True if `cross_point` falls within this divider's cross-axis
    /// extent (`cross_start..cross_start + cross_size`).
    pub fn cross_contains(&self, cross_point: f32) -> bool {
        cross_point >= self.cross_start && cross_point < self.cross_start + self.cross_size
    }
}

/// Fully-resolved split-tree layout: every leaf's rect plus every
/// internal split's divider geometry, computed by [`SplitTree::layout`].
#[derive(Debug, Clone, PartialEq)]
pub struct SplitTreeLayout {
    pub bounds: Rect,
    /// Leaf rects, pre-order (left-to-right / top-to-bottom).
    pub leaves: Vec<(WidgetId, Rect)>,
    /// Divider geometry, pre-order. `split_index` is the stable
    /// address for ratio mutation — don't rely on this `Vec`'s
    /// positional index instead, since it's only stable for a given
    /// tree shape.
    pub dividers: Vec<SplitTreeDivider>,
}

impl SplitTreeLayout {
    /// Resolve a point against the divider list using a symmetric
    /// pixel tolerance — the GTK/macOS convention. Returns the
    /// `split_index` of the first divider (pre-order — outermost
    /// splits win ties) whose band contains `point`, checking
    /// cross-axis extent too.
    pub fn hit_test_divider(&self, point: Point, tolerance: f32) -> Option<usize> {
        self.dividers.iter().find_map(|d| {
            let (axis, cross) = match d.direction {
                SplitDirection::Horizontal => (point.x, point.y),
                SplitDirection::Vertical => (point.y, point.x),
            };
            (d.hit_tolerant(axis, tolerance) && d.cross_contains(cross)).then_some(d.split_index)
        })
    }

    /// Resolve a discrete TUI cell coordinate against the divider list
    /// using the cell-quantized convention — exact match against
    /// [`SplitTreeDivider::cell_position`], not a tolerance band. Use
    /// this (not [`Self::hit_test_divider`]) for TUI mouse events so
    /// hit-test agrees exactly with what `cell_position()`-driven
    /// painting drew.
    pub fn hit_test_divider_cell(&self, axis_cell: u16, cross_cell: u16) -> Option<usize> {
        self.dividers.iter().find_map(|d| {
            let cross_ok = d.cross_contains(cross_cell as f32);
            (d.cell_position() == axis_cell && cross_ok).then_some(d.split_index)
        })
    }

    /// Resolve a point against leaf rects. Returns the leaf `WidgetId`
    /// whose rect contains `point`, or `None` if it falls in a divider
    /// gap or outside `bounds` entirely.
    pub fn hit_test_leaf(&self, point: Point) -> Option<&WidgetId> {
        self.leaves.iter().find_map(|(id, rect)| {
            (point.x >= rect.x
                && point.x < rect.x + rect.width
                && point.y >= rect.y
                && point.y < rect.y + rect.height)
                .then_some(id)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wid(s: &str) -> WidgetId {
        WidgetId::new(s)
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::new(x, y, w, h)
    }

    fn measure(t: f32) -> SplitTreeMeasure {
        SplitTreeMeasure::new(t)
    }

    #[test]
    fn single_leaf() {
        let tree = SplitTree::leaf(wid("a"));
        assert!(tree.is_leaf());
        assert_eq!(tree.leaf_ids(), vec![wid("a")]);
        assert_eq!(tree.leaf_count(), 1);
        assert_eq!(tree.split_count(), 0);
    }

    #[test]
    fn single_split() {
        let tree = SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::leaf(wid("a")),
            SplitTree::leaf(wid("b")),
        );
        assert!(!tree.is_leaf());
        assert_eq!(tree.leaf_ids(), vec![wid("a"), wid("b")]);
        assert_eq!(tree.leaf_count(), 2);
        assert_eq!(tree.split_count(), 1);
    }

    #[test]
    fn nested_split_pre_order() {
        // Split(H, Split(V, a, c), b)
        let tree = SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::split(
                SplitDirection::Vertical,
                0.5,
                SplitTree::leaf(wid("a")),
                SplitTree::leaf(wid("c")),
            ),
            SplitTree::leaf(wid("b")),
        );
        assert_eq!(tree.leaf_ids(), vec![wid("a"), wid("c"), wid("b")]);
        assert_eq!(tree.leaf_count(), 3);
        assert_eq!(tree.split_count(), 2);
    }

    #[test]
    fn layout_leaf_gets_full_bounds() {
        let tree = SplitTree::leaf(wid("a"));
        let layout = tree.layout(rect(0.0, 0.0, 100.0, 50.0), measure(1.0));
        assert_eq!(layout.leaves, vec![(wid("a"), rect(0.0, 0.0, 100.0, 50.0))]);
        assert!(layout.dividers.is_empty());
    }

    #[test]
    fn layout_horizontal_split_side_by_side() {
        let tree = SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::leaf(wid("a")),
            SplitTree::leaf(wid("b")),
        );
        // 41 wide, 1-thick divider -> available 40, 0.5*40=20.
        let layout = tree.layout(rect(0.0, 0.0, 41.0, 10.0), measure(1.0));
        assert_eq!(layout.leaves.len(), 2);
        assert_eq!(layout.leaves[0], (wid("a"), rect(0.0, 0.0, 20.0, 10.0)));
        assert_eq!(layout.leaves[1], (wid("b"), rect(21.0, 0.0, 20.0, 10.0)));
        assert_eq!(layout.dividers.len(), 1);
        let d = &layout.dividers[0];
        assert_eq!(d.split_index, 0);
        assert_eq!(d.direction, SplitDirection::Horizontal);
        assert!((d.position - 20.0).abs() < 0.001);
        assert!((d.axis_start - 0.0).abs() < 0.001);
        assert!((d.axis_size - 41.0).abs() < 0.001);
        assert!((d.cross_start - 0.0).abs() < 0.001);
        assert!((d.cross_size - 10.0).abs() < 0.001);
    }

    #[test]
    fn layout_vertical_split_stacked() {
        let tree = SplitTree::split(
            SplitDirection::Vertical,
            0.25,
            SplitTree::leaf(wid("a")),
            SplitTree::leaf(wid("b")),
        );
        // 21 tall, 1-thick divider -> available 20, 0.25*20=5.
        let layout = tree.layout(rect(0.0, 0.0, 10.0, 21.0), measure(1.0));
        assert_eq!(layout.leaves[0], (wid("a"), rect(0.0, 0.0, 10.0, 5.0)));
        assert_eq!(layout.leaves[1], (wid("b"), rect(0.0, 6.0, 10.0, 15.0)));
        let d = &layout.dividers[0];
        assert_eq!(d.direction, SplitDirection::Vertical);
        assert!((d.position - 5.0).abs() < 0.001);
    }

    #[test]
    fn dividers_pre_order_nested() {
        // Split(H@root idx0, Split(V idx1, a, c), b)
        let tree = SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::split(
                SplitDirection::Vertical,
                0.5,
                SplitTree::leaf(wid("a")),
                SplitTree::leaf(wid("c")),
            ),
            SplitTree::leaf(wid("b")),
        );
        let layout = tree.layout(rect(0.0, 0.0, 800.0, 600.0), measure(0.0));
        assert_eq!(layout.dividers.len(), 2);
        assert_eq!(layout.dividers[0].split_index, 0);
        assert_eq!(layout.dividers[0].direction, SplitDirection::Horizontal);
        assert!((layout.dividers[0].position - 400.0).abs() < 0.001);
        assert_eq!(layout.dividers[1].split_index, 1);
        assert_eq!(layout.dividers[1].direction, SplitDirection::Vertical);
        assert!((layout.dividers[1].position - 300.0).abs() < 0.001); // 600*0.5
    }

    #[test]
    fn set_ratio_at_index_updates_layout_and_clamps() {
        let mut tree = SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::leaf(wid("a")),
            SplitTree::leaf(wid("b")),
        );
        assert!(tree.set_ratio_at_index(0, 0.7));
        let layout = tree.layout(rect(0.0, 0.0, 1000.0, 600.0), measure(0.0));
        assert!((layout.leaves[0].1.width - 700.0).abs() < 0.001);
        assert!((layout.leaves[1].1.width - 300.0).abs() < 0.001);

        // Clamped to MAX_RATIO.
        assert!(tree.set_ratio_at_index(0, 0.99));
        let layout = tree.layout(rect(0.0, 0.0, 1000.0, 600.0), measure(0.0));
        assert!((layout.leaves[0].1.width - 900.0).abs() < 0.001);

        // Clamped to MIN_RATIO.
        assert!(tree.set_ratio_at_index(0, 0.01));
        let layout = tree.layout(rect(0.0, 0.0, 1000.0, 600.0), measure(0.0));
        assert!((layout.leaves[0].1.width - 100.0).abs() < 0.001);

        // Not found.
        assert!(!tree.set_ratio_at_index(5, 0.5));
    }

    #[test]
    fn ratio_at_index_reads_back_current_value() {
        let mut tree = SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::split(
                SplitDirection::Vertical,
                0.25,
                SplitTree::leaf(wid("a")),
                SplitTree::leaf(wid("c")),
            ),
            SplitTree::leaf(wid("b")),
        );
        assert_eq!(tree.ratio_at_index(0), Some(0.5));
        assert_eq!(tree.ratio_at_index(1), Some(0.25));
        assert_eq!(tree.ratio_at_index(2), None); // not a split index
        tree.set_ratio_at_index(1, 0.6);
        assert_eq!(tree.ratio_at_index(1), Some(0.6));
    }

    #[test]
    fn adjust_ratio_at_index_deltas_and_clamps() {
        let mut tree = SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::leaf(wid("a")),
            SplitTree::leaf(wid("b")),
        );
        assert!(tree.adjust_ratio_at_index(0, 0.2));
        match &tree {
            SplitTree::Split { ratio, .. } => assert!((ratio - 0.7).abs() < 0.001),
            _ => panic!(),
        }
        // Push past MAX_RATIO — clamps.
        assert!(tree.adjust_ratio_at_index(0, 0.5));
        match &tree {
            SplitTree::Split { ratio, .. } => assert!((ratio - MAX_RATIO).abs() < 0.001),
            _ => panic!(),
        }
    }

    #[test]
    fn set_all_ratios_applies_to_every_split() {
        let mut tree = SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::split(
                SplitDirection::Vertical,
                0.5,
                SplitTree::leaf(wid("a")),
                SplitTree::leaf(wid("c")),
            ),
            SplitTree::leaf(wid("b")),
        );
        tree.set_all_ratios(0.3);
        let layout = tree.layout(rect(0.0, 0.0, 800.0, 600.0), measure(0.0));
        assert!((layout.dividers[0].position - 240.0).abs() < 0.001); // 800*0.3
        assert!((layout.dividers[1].position - 180.0).abs() < 0.001); // 600*0.3
    }

    #[test]
    fn parent_split_of_finds_direct_children() {
        // Split(H idx0, Split(V idx1, a, c), b)
        let tree = SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::split(
                SplitDirection::Vertical,
                0.5,
                SplitTree::leaf(wid("a")),
                SplitTree::leaf(wid("c")),
            ),
            SplitTree::leaf(wid("b")),
        );
        let pa = tree.parent_split_of(&wid("a")).unwrap();
        assert_eq!(pa.0, 1);
        assert_eq!(pa.1, SplitDirection::Vertical);
        assert!(pa.2);

        let pc = tree.parent_split_of(&wid("c")).unwrap();
        assert_eq!(pc.0, 1);
        assert!(!pc.2);

        let pb = tree.parent_split_of(&wid("b")).unwrap();
        assert_eq!(pb.0, 0);
        assert_eq!(pb.1, SplitDirection::Horizontal);
        assert!(!pb.2);

        assert!(tree.parent_split_of(&wid("nope")).is_none());
    }

    #[test]
    fn cell_position_truncates_toward_zero() {
        let d = SplitTreeDivider {
            split_index: 0,
            direction: SplitDirection::Horizontal,
            position: 20.9,
            axis_start: 0.0,
            axis_size: 41.0,
            cross_start: 0.0,
            cross_size: 10.0,
            thickness: 1.0,
        };
        // `as u16` truncates, doesn't round — 20.9 -> 20, NOT 21.
        assert_eq!(d.cell_position(), 20);
    }

    #[test]
    fn hit_test_divider_tolerant_matches_gtk_convention() {
        let tree = SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::leaf(wid("a")),
            SplitTree::leaf(wid("b")),
        );
        let layout = tree.layout(rect(0.0, 0.0, 100.0, 100.0), measure(4.0));
        let d = &layout.dividers[0];
        // Exactly inside the band.
        assert_eq!(
            layout.hit_test_divider(
                Point {
                    x: d.position + 1.0,
                    y: 50.0
                },
                3.0
            ),
            Some(0)
        );
        // Just outside the band but within tolerance.
        assert_eq!(
            layout.hit_test_divider(
                Point {
                    x: d.position - 2.0,
                    y: 50.0
                },
                3.0
            ),
            Some(0)
        );
        // Well outside tolerance.
        assert_eq!(
            layout.hit_test_divider(
                Point {
                    x: d.position - 20.0,
                    y: 50.0
                },
                3.0
            ),
            None
        );
        // Outside cross-axis extent.
        assert_eq!(
            layout.hit_test_divider(
                Point {
                    x: d.position,
                    y: 500.0
                },
                3.0
            ),
            None
        );
    }

    #[test]
    fn hit_test_divider_cell_requires_exact_cell_match() {
        let tree = SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::leaf(wid("a")),
            SplitTree::leaf(wid("b")),
        );
        let layout = tree.layout(rect(0.0, 0.0, 41.0, 10.0), measure(1.0));
        let d = &layout.dividers[0];
        let cell = d.cell_position();
        assert_eq!(layout.hit_test_divider_cell(cell, 5), Some(0));
        // One cell off — no match (unlike the tolerant variant).
        assert_eq!(layout.hit_test_divider_cell(cell + 1, 5), None);
        assert_eq!(
            layout.hit_test_divider_cell(cell.saturating_sub(1), 5),
            None
        );
        // Outside cross-axis cell range.
        assert_eq!(layout.hit_test_divider_cell(cell, 50), None);
    }

    #[test]
    fn hit_test_leaf_resolves_pane_and_ignores_divider_gap() {
        let tree = SplitTree::split(
            SplitDirection::Horizontal,
            0.5,
            SplitTree::leaf(wid("a")),
            SplitTree::leaf(wid("b")),
        );
        let layout = tree.layout(rect(0.0, 0.0, 41.0, 10.0), measure(1.0));
        assert_eq!(
            layout.hit_test_leaf(Point { x: 5.0, y: 5.0 }),
            Some(&wid("a"))
        );
        assert_eq!(
            layout.hit_test_leaf(Point { x: 30.0, y: 5.0 }),
            Some(&wid("b"))
        );
        // Inside the 1-wide divider gap at x=20 — no leaf.
        assert_eq!(layout.hit_test_leaf(Point { x: 20.0, y: 5.0 }), None);
        // Outside bounds entirely.
        assert_eq!(layout.hit_test_leaf(Point { x: 500.0, y: 5.0 }), None);
    }

    #[test]
    fn deep_nesting_leaf_and_divider_counts() {
        // 7-leaf balanced-ish tree.
        let mut tree = SplitTree::leaf(wid("0"));
        for i in 1..7 {
            tree = SplitTree::split(
                SplitDirection::Horizontal,
                0.5,
                tree,
                SplitTree::leaf(wid(&i.to_string())),
            );
        }
        assert_eq!(tree.leaf_count(), 7);
        assert_eq!(tree.split_count(), 6);
        let layout = tree.layout(rect(0.0, 0.0, 1400.0, 100.0), measure(0.0));
        assert_eq!(layout.leaves.len(), 7);
        assert_eq!(layout.dividers.len(), 6);
        // Every leaf still has positive width.
        for (_, r) in &layout.leaves {
            assert!(r.width > 0.0);
        }
    }
}
