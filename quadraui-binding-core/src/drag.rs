//! Divider and tree-scrollbar drag state, plus the small hit-map lookups
//! ([`find_split_layout`], [`tree_row_path`], [`tree_scroll_state`],
//! [`tab_bar_for_right_segment`]) that arm and drive them. A binding arms
//! [`ActiveDrag`]/[`ScrollbarDrag`] on the matching `MouseDown` and clears
//! it on the matching `MouseUp`; while armed, every `MouseMoved` becomes a
//! synthesized `UiEvent::SplitDividerDragged`/`ScrollOffsetChanged` instead
//! of the raw move.

use quadraui::primitives::scrollbar::ThumbDrag;
use quadraui::{Point, Scrollbar, SplitHit, SplitLayout, TabBarHit, TreePath, WidgetId};

use crate::hit::PaintedHit;

/// A divider drag in progress: armed by the binding's own event handler on a
/// `MouseDown` landing on `HitDetail::SplitDivider`, and cleared on the
/// matching `MouseUp`. `layout` is the `Split`'s own layout as painted the
/// frame the drag started, snapshotted rather than re-read each move so a
/// drag stays anchored to the geometry the user actually grabbed even if a
/// handler-driven re-render shifts the divider under the cursor mid-drag.
pub struct ActiveDrag {
    pub tree: WidgetId,
    pub layout: SplitLayout,
}

impl ActiveDrag {
    /// The ratio `point` (in the same absolute frame as `layout`) implies
    /// along this split's axis, clamped to `[0.0, 1.0]`. Falls back to the
    /// layout's own already-resolved ratio when the split has no room to
    /// drag in (e.g. a pane collapsed to zero by its own `first_min`/
    /// `second_min`), rather than dividing by zero.
    pub fn ratio_at(&self, point: Point) -> f32 {
        let divider = self.layout.divider_bounds;
        let bounds = self.layout.bounds;
        let vertical_divider = divider.width < divider.height;
        let (total, offset, divider_thickness) = if vertical_divider {
            (bounds.width, point.x - bounds.x, divider.width)
        } else {
            (bounds.height, point.y - bounds.y, divider.height)
        };
        let available = (total - divider_thickness).max(0.0);
        if available <= 0.0 {
            return self.layout.resolved_ratio;
        }
        (offset / available).clamp(0.0, 1.0)
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
