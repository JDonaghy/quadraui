//! Divider and tree-scrollbar drag state, plus the small hit-map lookups
//! ([`find_split_layout`], [`find_data_table_layout`], [`tree_row_path`],
//! [`tree_scroll_state`], [`tab_bar_for_right_segment`]) that arm and drive
//! them. A binding arms [`ActiveDrag`]/[`ScrollbarDrag`]/
//! [`DataTableColumnDrag`]/[`ScrollDrag`] on the matching `MouseDown` and
//! clears it on the matching `MouseUp`; while armed, every `MouseMoved`
//! becomes a synthesized `UiEvent::SplitDividerDragged`/
//! `ScrollOffsetChanged`/`UiEvent::DataTable(.., ColumnResized)` instead of
//! the raw move. [`scroll_view_at`]/[`scroll_scrollbar`]/
//! [`enclosing_scroll_child`] are the `LayoutNode::Scroll` counterpart of
//! [`tree_scroll_state`] — a generic container has no row-indexed geometry
//! of its own, so these rebuild a [`quadraui::ScrollView`] (for the
//! wheel/keyboard delta math) and hand back the already-painted
//! [`quadraui::Scrollbar`] separately, rather than one combined tuple.

use quadraui::primitives::scrollbar::ThumbDrag;
use quadraui::{
    DataTableLayout, Point, Rect, ScrollAxis, ScrollView, Scrollbar, SplitDirection, SplitHit,
    SplitLayout, SplitTreeLayout, TabBarHit, TreePath, WidgetId,
};

use crate::hit::PaintedHit;

/// A divider drag in progress: armed by the binding's own event handler on a
/// `MouseDown` landing on `HitDetail::SplitDivider` (a two-pane
/// [`quadraui::Split`]) or `HitDetail::SplitTreeDivider` (one divider of a
/// [`quadraui::SplitTree`]), and cleared on the matching `MouseUp`. Either
/// variant snapshots the layout as painted the frame the drag started,
/// rather than re-reading it each move, so a drag stays anchored to the
/// geometry the user actually grabbed even if a handler-driven re-render
/// shifts the divider under the cursor mid-drag.
pub enum ActiveDrag {
    Split {
        tree: WidgetId,
        layout: SplitLayout,
    },
    SplitTree {
        tree: WidgetId,
        split_index: usize,
        layout: SplitTreeLayout,
    },
}

impl ActiveDrag {
    /// The tree/split this drag is resizing — the `widget`
    /// `UiEvent::SplitDividerDragged::tree` should carry.
    pub fn tree(&self) -> WidgetId {
        match self {
            ActiveDrag::Split { tree, .. } | ActiveDrag::SplitTree { tree, .. } => *tree,
        }
    }

    /// The `UiEvent::SplitDividerDragged::split_index` this drag should
    /// carry — always `0` for a two-pane `Split` (it has exactly one
    /// divider, with no pre-order addressing of its own), or the dragged
    /// divider's own `SplitTreeDivider::split_index` for a `SplitTree`.
    pub fn split_index(&self) -> usize {
        match self {
            ActiveDrag::Split { .. } => 0,
            ActiveDrag::SplitTree { split_index, .. } => *split_index,
        }
    }

    /// The ratio `point` (in the same absolute frame as the snapshotted
    /// layout) implies along this divider's axis, clamped to `[0.0, 1.0]`.
    /// For a two-pane [`ActiveDrag::Split`], falls back to the divider's
    /// own already-resolved ratio when it has no room to drag in (e.g. a
    /// pane collapsed to zero by its own `first_min`/`second_min`), rather
    /// than dividing by zero. For an [`ActiveDrag::SplitTree`], returns
    /// `None` instead — a `SplitTreeDivider` carries no equivalent
    /// already-resolved ratio to fall back to, so the caller leaves the
    /// drag a no-op for that move rather than dividing by zero or
    /// snapping to a magic ratio. Also returns `None` when `split_index`
    /// names a divider the snapshotted layout no longer has — unreachable
    /// today, since the index came from a hit test on this same snapshot.
    pub fn ratio_at(&self, point: Point) -> Option<f32> {
        match self {
            ActiveDrag::Split { layout, .. } => {
                let divider = layout.divider_bounds;
                let bounds = layout.bounds;
                let vertical_divider = divider.width < divider.height;
                let (total, offset, divider_thickness) = if vertical_divider {
                    (bounds.width, point.x - bounds.x, divider.width)
                } else {
                    (bounds.height, point.y - bounds.y, divider.height)
                };
                let available = (total - divider_thickness).max(0.0);
                if available <= 0.0 {
                    return Some(layout.resolved_ratio);
                }
                Some((offset / available).clamp(0.0, 1.0))
            }
            ActiveDrag::SplitTree {
                layout,
                split_index,
                ..
            } => {
                let divider = layout
                    .dividers
                    .iter()
                    .find(|d| d.split_index == *split_index)?;
                if divider.axis_size <= 0.0 {
                    return None;
                }
                let axis_point = match divider.direction {
                    SplitDirection::Horizontal => point.x,
                    SplitDirection::Vertical => point.y,
                };
                // Matches `dispatch_mouse_moved`'s own `DragTarget::SplitDivider`
                // conversion (`quadraui/src/dispatch.rs`): divide the cursor's
                // offset from the divider's axis start by the FULL parent
                // span, not the span minus the divider's own thickness — so
                // the same physical drag yields the same ratio through this
                // binding as through a Rust app driving the primitive
                // directly.
                Some(((axis_point - divider.axis_start) / divider.axis_size).clamp(0.0, 1.0))
            }
        }
    }
}

/// Re-finds the [`SplitLayout`] whose divider `id` claims at `point`, so a
/// `MouseDown` landing on `HitDetail::SplitDivider` can snapshot it into an
/// [`ActiveDrag`]. Re-runs the hit-test (rather than trusting
/// [`crate::hit::hit_test_painted`]'s already-discarded match) because
/// `PaintedHit::Split` never stores its own id — only `SplitHit`'s variants
/// carry one — so the id has to be recovered the same way
/// [`crate::hit::hit_test_painted`] found it the first time.
pub fn find_split_layout(
    hit_map: &[PaintedHit],
    point: Point,
    id: WidgetId,
) -> Option<SplitLayout> {
    hit_map.iter().rev().find_map(|hit| match hit {
        PaintedHit::Split { layout } => {
            matches!(layout.hit_test(point.x, point.y), SplitHit::Divider(hit_id) if hit_id == id)
                .then(|| layout.clone())
        }
        _ => None,
    })
}

/// Re-finds the [`SplitTreeLayout`] painted under id `id`, so a `MouseDown`
/// landing on `HitDetail::SplitTreeDivider` can snapshot it into an
/// [`ActiveDrag::SplitTree`] — mirrors [`find_split_layout`], minus the
/// re-run hit-test: unlike [`PaintedHit::Split`], [`PaintedHit::SplitTree`]
/// already stores its own id directly.
pub fn find_split_tree_layout(hit_map: &[PaintedHit], id: WidgetId) -> Option<SplitTreeLayout> {
    hit_map.iter().rev().find_map(|hit| match hit {
        PaintedHit::SplitTree { id: hit_id, layout } if *hit_id == id => Some(layout.clone()),
        _ => None,
    })
}

/// A column-resize drag in progress: armed by the binding's own event
/// handler on a `MouseDown` landing on `HitDetail::DataTableHeaderDivider`,
/// and cleared on the matching `MouseUp` — mirrors [`ActiveDrag`]'s divider
/// bookkeeping. `area`/`layout` are the `DataTable`'s own paint geometry as
/// painted the frame the drag started, snapshotted (rather than re-read each
/// move) for the same reason `ActiveDrag::layout` is: a drag stays anchored
/// to the geometry the user actually grabbed even if a handler-driven
/// re-render shifts the divider under the cursor mid-drag. `col` is the
/// column to the LEFT of the dragged divider, per
/// [`quadraui::DataTableHit::HeaderDivider`].
pub struct DataTableColumnDrag {
    pub widget: WidgetId,
    pub col: usize,
    pub area: Rect,
    pub layout: DataTableLayout,
}

impl DataTableColumnDrag {
    /// Floor a dragged column is clamped to, so a drag can never collapse it
    /// (or, by underflow, the divider past it) to nothing.
    const MIN_WIDTH: f32 = 4.0;

    /// The width `col` should resize to for a pointer now at `point` (same
    /// absolute frame as `area`), per the "`App should update
    /// column_overrides[col]`" contract [`quadraui::DataTableEvent::ColumnResized`]
    /// documents — a single column's width, not the paired
    /// last-column-absorbs model [`DataTableLayout::drag_divider`] computes
    /// for a Rust app driving the primitive directly.
    pub fn width_at(&self, point: Point) -> f32 {
        let local_x = point.x - self.area.x;
        let content_x = local_x + self.layout.h_scroll;
        let col_x = self.layout.columns[self.col].x;
        (content_x - col_x).max(Self::MIN_WIDTH)
    }
}

/// Re-finds the `(area, layout)` of the `DataTable` hit painted under id
/// `id` this frame, so a `MouseDown` landing on
/// `HitDetail::DataTableHeaderDivider` can snapshot it into a
/// [`DataTableColumnDrag`].
pub fn find_data_table_layout(
    hit_map: &[PaintedHit],
    id: WidgetId,
) -> Option<(Rect, DataTableLayout)> {
    hit_map.iter().find_map(|hit| match hit {
        PaintedHit::DataTable {
            id: hit_id,
            area,
            layout,
            ..
        } if *hit_id == id => Some((*area, layout.clone())),
        _ => None,
    })
}

/// The [`TreePath`] of the row at visual index `row` within the `Tree` hit
/// painted under id `id` this frame, or `None` if that tree isn't in the
/// current hit map (e.g. the frame that produced this hit test is stale).
pub fn tree_row_path(hit_map: &[PaintedHit], id: WidgetId, row: usize) -> Option<TreePath> {
    hit_map.iter().find_map(|hit| match hit {
        PaintedHit::Tree {
            id: hit_id,
            row_paths,
            ..
        } if *hit_id == id => row_paths.get(row).cloned(),
        _ => None,
    })
}

/// A tree scrollbar thumb drag in progress: armed by the binding's own event handler
/// on a `MouseDown` landing on `HitDetail::TreeScrollbarThumb`, and cleared
/// on the matching `MouseUp` — mirrors [`ActiveDrag`]'s divider-drag
/// bookkeeping. `drag` is the [`ThumbDrag`] snapshotted the frame the drag
/// started, so a later re-render shifting the thumb under the cursor never
/// retargets a drag already in progress.
pub struct ScrollbarDrag {
    pub widget: WidgetId,
    pub drag: ThumbDrag,
}

/// The `(Scrollbar, total_rows, viewport_rows, scroll_offset)` of the
/// `Tree` hit painted under id `id` this frame, or `None` if that tree
/// isn't in the current hit map or has no scrollbar (its rows fit without
/// scrolling). Used both to arm a thumb drag (needs [`Scrollbar::travel`]
/// and the current `scroll_offset` as the drag's origin) and to compute a
/// track-click page or a wheel notch (needs `total_rows`/`viewport_rows`
/// for the `max_offset` clamp and `scroll_offset` as the value to move
/// from).
pub fn tree_scroll_state(
    hit_map: &[PaintedHit],
    id: WidgetId,
) -> Option<(Scrollbar, usize, usize, usize)> {
    hit_map.iter().find_map(|hit| match hit {
        PaintedHit::Tree {
            id: hit_id,
            scrollbar,
            row_paths,
            viewport_rows,
            scroll_offset,
            ..
        } if *hit_id == id => scrollbar
            .clone()
            .map(|sb| (sb, row_paths.len(), *viewport_rows, *scroll_offset)),
        _ => None,
    })
}

/// The `(total_lines, viewport_lines, scroll_offset)` of the `TextDisplay`
/// hit painted under id `id` this frame, or `None` if that display isn't in
/// the current hit map — mirrors [`tree_scroll_state`], minus the
/// `Scrollbar` (a wheel notch over a `TextDisplay` scrolls it whether or not
/// `show_scrollbar` painted one).
pub fn text_display_scroll_state(
    hit_map: &[PaintedHit],
    id: WidgetId,
) -> Option<(usize, usize, usize)> {
    hit_map.iter().find_map(|hit| match hit {
        PaintedHit::TextDisplay {
            id: hit_id,
            layout,
            total_lines,
            viewport_lines,
            ..
        } if *hit_id == id => Some((*total_lines, *viewport_lines, layout.resolved_scroll_offset)),
        _ => None,
    })
}

/// A `Scroll` scrollbar thumb drag in progress: armed by the binding's own
/// event handler on a `MouseDown` landing on `HitDetail::ScrollScrollbarThumb`,
/// and cleared on the matching `MouseUp` — mirrors [`ScrollbarDrag`]'s own
/// bookkeeping, kept as a separate field/type rather than reused outright
/// because a `Scroll` can scroll along either axis (a `Tree`'s own
/// scrollbar is always vertical, so [`ScrollbarDrag`]'s own `MouseMoved`
/// continuation only ever reads `position.y`). `axis` is the dragged
/// scrollbar's own [`ScrollAxis`] — [`Self`]'s own `MouseMoved`
/// continuation reads `position.y` for [`ScrollAxis::Vertical`],
/// `position.x` for [`ScrollAxis::Horizontal`].
pub struct ScrollDrag {
    pub widget: WidgetId,
    pub axis: ScrollAxis,
    pub drag: ThumbDrag,
}

/// Rebuilds the [`quadraui::ScrollView`] [`crate::render::paint_scroll`]
/// painted this frame for the `Scroll` hit under id `id`, so the engine's
/// own wheel/keyboard/scrollbar handling can resolve a new offset through
/// `ScrollView::handle_wheel`/`handle_key`/`scroll_by` — this crate never
/// re-derives that math itself. `None` if `id` isn't a `Scroll` in the
/// current hit map.
pub fn scroll_view_at(hit_map: &[PaintedHit], id: WidgetId) -> Option<ScrollView> {
    hit_map.iter().find_map(|hit| match hit {
        PaintedHit::Scroll {
            id: hit_id,
            viewport,
            axis,
            content_len,
            offset,
            ..
        } if *hit_id == id => {
            let mut view = match axis {
                ScrollAxis::Vertical => ScrollView::vertical(id, *viewport, *content_len),
                ScrollAxis::Horizontal => ScrollView::horizontal(id, *viewport, *content_len),
            };
            view.scroll_offset = *offset;
            Some(view)
        }
        _ => None,
    })
}

/// The [`Scrollbar`] [`crate::render::paint_scroll`] painted this frame for
/// the `Scroll` hit under id `id`, or `None` when that scroll isn't in the
/// current hit map or has nothing to scroll (no scrollbar painted, same as
/// [`tree_scroll_state`]'s own `None` case) — used both to arm a thumb drag
/// ([`Scrollbar::travel`], feeding [`ThumbDrag::begin`]'s own `travel`) and
/// to page the track ([`Scrollbar::press_zone`]).
pub fn scroll_scrollbar(hit_map: &[PaintedHit], id: WidgetId) -> Option<Scrollbar> {
    hit_map.iter().find_map(|hit| match hit {
        PaintedHit::Scroll {
            id: hit_id,
            scrollbar: Some(scrollbar),
            ..
        } if *hit_id == id => Some(scrollbar.clone()),
        _ => None,
    })
}

/// The enclosing `Scroll`'s own id and `child`'s own `(start, len)` within
/// it, for whichever `Scroll` hit (if any) lists `child` among its own
/// [`PaintedHit::Scroll::children`] — used to scroll a child that just
/// received keyboard focus fully into view, and to decide whether
/// PageUp/PageDown/Home/End should scroll the enclosing `Scroll` rather
/// than route to the focused child itself. There is no reverse index from
/// child to parent, so this scans every `Scroll` hit painted this frame;
/// `children` is only ever populated for a direct `Leaf` child (see that
/// field's own doc), so a child nested two `Scroll`s deep, or under a
/// `Row`/`Column`/`Split`/… inside one, never matches either — out of
/// scope for now, same as `children` itself.
pub fn enclosing_scroll_child(
    hit_map: &[PaintedHit],
    child: WidgetId,
) -> Option<(WidgetId, f32, f32)> {
    hit_map.iter().find_map(|hit| match hit {
        PaintedHit::Scroll { id, children, .. } => children
            .iter()
            .find(|(cid, ..)| *cid == child)
            .map(|(_, start, len)| (*id, *start, *len)),
        _ => None,
    })
}

/// The tab bar's own id for a `TabBarRightSegment` hit, given the `id` of
/// the clicked button — `hit_test_painted` returns the button's id (the one
/// [`TabBarHit::RightSegment`] carries), not the enclosing bar's, so this
/// walks the hit map's painted tab bars to find the one whose
/// `hit_regions` contains that button id.
pub fn tab_bar_for_right_segment(hit_map: &[PaintedHit], button_id: WidgetId) -> Option<WidgetId> {
    hit_map.iter().find_map(|hit| match hit {
        PaintedHit::TabBar { id, layout, .. }
            if layout.hit_regions.iter().any(
                |(_, hit)| matches!(hit, TabBarHit::RightSegment(rid) if *rid == button_id),
            ) =>
        {
            Some(*id)
        }
        _ => None,
    })
}
