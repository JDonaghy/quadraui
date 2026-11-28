//! Container drag state, plus the small hit-map lookups
//! ([`find_split_layout`], [`find_split_tree_layout`], [`scroll_view_at`],
//! [`scroll_scrollbar`], [`enclosing_scroll_child`]) that arm and drive
//! them. A binding arms [`ActiveDrag`]/[`ScrollDrag`] on the matching
//! `MouseDown` and clears it on the matching `MouseUp`; while armed, every
//! `MouseMoved` becomes a synthesized `UiEvent::SplitDividerDragged`/
//! `ScrollOffsetChanged` instead of the raw move. A widget's own drags (a
//! table's column divider, a list's or tree's scrollbar thumb) are the
//! widget's: it captures the pointer through its `Interactive::handle`.
//! [`GestureDrag`] is a different shape entirely: a generic draggable/
//! drop-target gesture over ANY leaf `InteractionEngine::drag_roles`
//! marks, rather than a fixed-geometry divider/thumb — see its own doc,
//! and [`DragGestureWireEvent`] for what it reports.

use std::collections::HashMap;

use quadraui::primitives::scrollbar::ThumbDrag;
use quadraui::{
    Point, ScrollAxis, ScrollView, Scrollbar, SplitHit, SplitLayout,
    SplitTreeLayout, WidgetId,
};

use crate::hit::{hit_test_painted, HitDetail, PaintedHit};
use crate::wire::DragRole;

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

    /// The ratio that puts the dragged divider's leading edge at `point`
    /// (same absolute frame as the snapshotted layout), clamped to
    /// `[0.0, 1.0]` — [`SplitLayout::ratio_at`] /
    /// [`quadraui::SplitTreeDivider::ratio_at`], the exact inverse of the
    /// layout the divider was painted from. A two-pane split with no room
    /// to drag in keeps its resolved ratio; a tree divider with none (or
    /// one the snapshot no longer has) answers `None`, and the caller
    /// leaves that move a no-op.
    pub fn ratio_at(&self, point: Point) -> Option<f32> {
        match self {
            ActiveDrag::Split { layout, .. } => Some(layout.ratio_at(point)),
            ActiveDrag::SplitTree {
                layout,
                split_index,
                ..
            } => layout
                .dividers
                .iter()
                .find(|d| d.split_index == *split_index)?
                .ratio_at(point),
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

/// A `Scroll` scrollbar thumb drag in progress: armed by the binding's own
/// event handler on a `MouseDown` landing on `HitDetail::ScrollScrollbarThumb`,
/// and cleared on the matching `MouseUp`. `axis` is the dragged
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
/// current hit map. The construction itself is
/// [`crate::render::scroll_view_at`]'s own — shared rather than
/// re-implemented, so the two never drift apart on how a `ScrollView` is
/// rebuilt from a `Scroll`'s painted fields.
pub fn scroll_view_at(hit_map: &[PaintedHit], id: WidgetId) -> Option<ScrollView> {
    hit_map.iter().find_map(|hit| match hit {
        PaintedHit::Scroll {
            id: hit_id,
            viewport,
            axis,
            content_len,
            offset,
            ..
        } if *hit_id == id => Some(crate::render::scroll_view_at(
            id,
            *axis,
            *viewport,
            *content_len,
            *offset,
        )),
        _ => None,
    })
}

/// The [`Scrollbar`] [`crate::render::paint_scroll`] painted this frame for
/// the `Scroll` hit under id `id`, or `None` when that scroll isn't in the
/// current hit map or has nothing to scroll (no scrollbar painted) — used
/// both to arm a thumb drag
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
            .find(|c| c.id == child)
            .map(|c| (*id, c.start, c.len)),
        _ => None,
    })
}

/// A draggable-leaf gesture in progress: armed by
/// [`crate::engine::InteractionEngine::handle`] on a `MouseDown` landing on
/// a leaf whose [`DragRole::draggable`] is set (per
/// [`crate::engine::InteractionEngine::drag_roles`]), continued on every
/// `MouseMoved` via [`gesture_target_at`], and cleared on the matching
/// `MouseUp` — mirrors [`ActiveDrag`]'s own arm/continue/clear lifecycle,
/// but tracks an opaque drag source/target pair (any leaf, keyed by
/// whatever [`crate::wire::DecodeDiagnostics::drag_roles`] recorded for it)
/// rather than a fixed divider/thumb geometry. Unlike every other drag in
/// this module, arming one never rewrites the raw `MouseMoved`/`MouseUp`
/// into a replacement `UiEvent`: a draggable leaf's own click/selection
/// semantics (e.g. a `List` row's `ListViewEvent::SelectionChanged`) keep
/// firing exactly as without a gesture armed, and
/// `InteractionEngine::handle` dispatches the
/// [`crate::semantic::DragGestureWireEvent::DragStarted`]/`DragOver`/
/// `Dropped` wire events alongside them, through
/// [`crate::engine::EventSink::dispatch_drag_gesture`].
pub struct GestureDrag {
    /// The leaf the gesture started on.
    pub source: WidgetId,
    /// The [`HitDetail`] the originating `MouseDown` landed on within
    /// [`Self::source`] — e.g. [`HitDetail::ListItem`] for a drag starting
    /// on one row of a `List` — so a host reordering a list by drag knows
    /// WHICH row was picked up, not just that `source` (the whole `List`)
    /// was.
    pub source_detail: HitDetail,
    /// The leaf (and its own [`HitDetail`]) currently under the pointer,
    /// when that leaf's own [`DragRole::drop_target`] is set — `None`
    /// while the pointer hovers nothing accepting a drop, including right
    /// after arming (before the first `MouseMoved`). Updated by every
    /// `MouseMoved` while this gesture is armed, via [`gesture_target_at`];
    /// read back on the terminating `MouseUp` to resolve `Dropped`'s own
    /// target — a `MouseUp` with this still `None` ends the gesture with
    /// no `Dropped` dispatched at all.
    pub target: Option<(WidgetId, HitDetail)>,
}

/// The drop-target `(id, HitDetail)` the pointer at `point` currently
/// resolves to, if any — `None` when `point` lands on nothing painted this
/// frame, or on a leaf whose `drag_roles` entry has
/// [`DragRole::drop_target`] `false` (including a leaf absent from
/// `drag_roles` altogether, the common case for a binding that never calls
/// [`crate::engine::InteractionEngine::set_drag_roles`]). The `MouseMoved`
/// continuation of an armed [`GestureDrag`] calls this on every move to
/// decide whether its own [`GestureDrag::target`] changed — a target hit
/// on the gesture's own [`GestureDrag::source`] counts the same as any
/// other leaf: nothing here special-cases "dropping back onto where the
/// drag started" as a no-op, since that's just as meaningful a drop as any
/// other for a leaf marked both `draggable` and `drop_target` (e.g.
/// reordering a `List` by dragging one row onto another row of the same
/// list).
pub fn gesture_target_at(
    hit_map: &[PaintedHit],
    drag_roles: &HashMap<WidgetId, DragRole>,
    point: Point,
) -> Option<(WidgetId, HitDetail)> {
    let (id, detail) = hit_test_painted(hit_map, point)?;
    if drag_roles.get(&id).is_some_and(|role| role.drop_target) {
        Some((id, detail))
    } else {
        None
    }
}

/// Wire-shaped drag-gesture event [`crate::engine::InteractionEngine::handle`]
/// dispatches through [`crate::engine::EventSink::dispatch_drag_gesture`],
/// a default method on that trait: it defaults to ignoring the event and
/// reporting [`quadraui::Reaction::Continue`], so every existing
/// [`crate::engine::EventSink`] impl keeps compiling unchanged until a
/// binding deliberately overrides it to thread this to its own callback.
/// Derives `Serialize` only, not `Deserialize`: [`HitDetail`] — the shape
/// every `*_detail` field below reuses rather than inventing a second
/// drag-specific hit description — has no `Deserialize` of its own either,
/// since neither type has a reverse (host -> engine) decode path.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DragGestureWireEvent {
    /// A `MouseDown` landed on a leaf
    /// [`crate::engine::InteractionEngine::drag_roles`] marks
    /// [`crate::wire::DragRole::draggable`] — the gesture's own arming
    /// moment. Dispatched before the leaf's own ordinary click/selection
    /// semantics (if any) also fire through [`crate::engine::EventSink::dispatch`]
    /// for the same `MouseDown` — the two are independent effects of one
    /// press, not alternatives.
    DragStarted {
        source: WidgetId,
        source_detail: HitDetail,
    },
    /// The pointer moved onto a NEW leaf `drag_roles` marks
    /// [`crate::wire::DragRole::drop_target`] while the gesture above is
    /// still armed — dispatched once per target change (including back to
    /// the same leaf under a different [`HitDetail`], e.g. a different
    /// `List` row), never on every `MouseMoved` pixel that lands on the
    /// unchanged target.
    DragOver {
        source: WidgetId,
        source_detail: HitDetail,
        target: WidgetId,
        target_detail: HitDetail,
    },
    /// The gesture ended (`MouseUp`) while hovering a valid drop target —
    /// the host applies whatever mutation `source`/`source_detail` dropped
    /// onto `target`/`target_detail` stands for (e.g. reordering a
    /// `List`'s own backing collection from `source_detail`'s row index to
    /// `target_detail`'s). A `MouseUp` with no drop target under the
    /// pointer ends the gesture silently instead — no `Dropped` is
    /// dispatched at all.
    Dropped {
        source: WidgetId,
        source_detail: HitDetail,
        target: WidgetId,
        target_detail: HitDetail,
    },
}
