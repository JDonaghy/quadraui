//! Tab drag-and-drop: the pure, position-based resolution logic behind
//! [`crate::compose::TabGroupController`]'s tab drag gesture, plus the
//! controller's own in-progress-drag bookkeeping.
//!
//! [`TabDragSource`]/[`TabDropInstruction`]/[`resolve_tab_drop`] take only
//! caller-supplied geometry and drag-source identity — no
//! `TabGroupController`, no owned `Vec<Pane>` — so a host with its own
//! pane/tab model (vimcode's `Engine::editor_groups`) can drive tab
//! drag-and-drop without mirroring that model into a second `Vec<Pane>`
//! just to compute it. [`TabDragState`] is `TabGroupController`'s own
//! private in-progress-drag record; it stays `pub(crate)` because nothing
//! outside this crate constructs or reads it directly.

use crate::primitives::drop_zone::{
    drop_zone_hit_test, DropEdge, DropGroupRect, DropZone, DropZoneHit, DropZoneKind,
};
use crate::primitives::split::SplitDirection;

// ── Host-owned-model adoption ───────────────────────────────────────────

/// Identifies the tab being dragged, from the host's *own* pane/tab model.
///
/// Input to [`resolve_tab_drop`]. Unlike
/// [`TabGroupController::handle_tab_drop`](crate::compose::TabGroupController::handle_tab_drop),
/// which resolves the dragged tab's current position by searching its own
/// owned `Vec<Pane>`, `resolve_tab_drop` takes the position as given — the
/// host already knows it, because the host owns the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabDragSource {
    /// Index of the pane the drag started in, in the host's own pane order.
    pub pane_idx: usize,
    /// Index of the tab within that pane, in the host's own tab order.
    pub tab_idx: usize,
}

/// A position-based drop instruction: the result of resolving a tab
/// drag-and-drop gesture against pane geometry, expressed purely as indices
/// and a split direction — never as a mutation applied to an owned model.
///
/// Returned by [`resolve_tab_drop`]. The host translates each variant into
/// its own mutation (e.g. `Engine::editor_groups[pane_idx].tabs.remove(...)`)
/// instead of `TabGroupController` performing the mutation on a `Vec<Pane>`
/// it would otherwise have to own.
///
/// Mirrors the shape of
/// [`TabGroupEvent`](crate::compose::TabGroupEvent)'s cross-group-drag
/// variants (`TabReordered`, `TabMovedToPane`, `TabSplitToNewPane`,
/// `PaneCollapsed`) deliberately — those were already index/id-based, not
/// `Pane`-object-based; `resolve_tab_drop` just makes it possible to
/// compute them without a `TabGroupController` owning the panes at all.
#[derive(Debug, Clone, PartialEq)]
pub enum TabDropInstruction {
    /// The drop was a no-op: outside every pane, dropped back onto the
    /// source tab's own position, or (for `Split`) the only tab of the
    /// only pane dropped on its own edge.
    NoOp,
    /// Reorder the tab within its own pane.
    Reorder {
        pane_idx: usize,
        /// Position before the move.
        from_idx: usize,
        /// Position after the move.
        to_idx: usize,
    },
    /// Move the tab into a different pane (merge).
    MoveToPane {
        from_pane_idx: usize,
        /// Position the tab held in the source pane before the move.
        from_idx: usize,
        to_pane_idx: usize,
        /// Position in the target pane's tab list where the tab should land.
        insert_idx: usize,
        /// `true` when removing the tab leaves the source pane with zero
        /// tabs — the host should collapse it (or not; `resolve_tab_drop`
        /// never decides that, since collapsing means renumbering the
        /// host's own pane indices, which only the host can do correctly).
        source_now_empty: bool,
    },
    /// Split the target pane, creating a new pane adjacent to it that holds
    /// the dragged tab.
    SplitToNewPane {
        from_pane_idx: usize,
        /// Position the tab held in the source pane before the move.
        from_idx: usize,
        /// The pane whose edge was targeted.
        target_pane_idx: usize,
        /// Which edge the tab was dropped onto.
        edge: DropEdge,
        /// `Left`/`Right` edges split horizontally; `Top`/`Bottom` split
        /// vertically — pre-resolved here so the host doesn't have to
        /// duplicate this mapping.
        split_direction: SplitDirection,
        /// Same meaning as [`TabDropInstruction::MoveToPane::source_now_empty`].
        source_now_empty: bool,
    },
}

/// Resolve a tab drag-and-drop gesture into a [`TabDropInstruction`], using
/// only caller-supplied geometry and drag-source identity — no
/// `TabGroupController`, no owned `Vec<Pane>`.
///
/// This is the adoption path for a host that already owns its tab/pane
/// model (e.g. vimcode's `Engine::editor_groups`, keyed by `GroupId`):
/// build `groups` from your own layout each frame (or drag), and translate
/// the returned instruction into your own mutations directly, without
/// mirroring that model into a second `Vec<Pane>` just to drive drag/drop.
///
/// # Arguments
///
/// * `source` — which pane/tab is being dragged, per the host's own model.
/// * `pane_tab_counts` — tab count of *every* pane, indexed the same way as
///   `groups` (i.e. `pane_tab_counts[i]` is the tab count of the pane
///   described by `groups[i]`). Used to detect an empty-after-removal
///   source pane and to clamp reorder/insert indices — deliberately **not**
///   derived from `groups[i].tab_slots.len()`, since tabs scrolled off a
///   tab strip are omitted from `tab_slots` and would undercount.
/// * `cursor_x`, `cursor_y`, `groups`, `tab_bar_height` — forwarded to
///   [`drop_zone_hit_test`]; see that function's docs.
///
/// [`TabGroupController::handle_tab_drop`](crate::compose::TabGroupController::handle_tab_drop)
/// is itself implemented in terms of this function — it is the reference
/// implementation, not a parallel reimplementation that could drift.
pub fn resolve_tab_drop(
    source: TabDragSource,
    pane_tab_counts: &[usize],
    cursor_x: f32,
    cursor_y: f32,
    groups: &[DropGroupRect],
    tab_bar_height: f32,
) -> TabDropInstruction {
    let DropZoneHit::Zone(zone) = drop_zone_hit_test(cursor_x, cursor_y, groups, tab_bar_height)
    else {
        return TabDropInstruction::NoOp;
    };

    let from = source.pane_idx;
    let to = zone.group_idx;
    let cur_idx = source.tab_idx;
    let from_count = pane_tab_counts.get(from).copied().unwrap_or(0);

    // Extracted up-front so the merge arm doesn't need to re-match on
    // `zone.kind` after the outer match has consumed it.
    let reorder_insert_idx = if let DropZoneKind::TabReorder(idx) = zone.kind {
        Some(idx)
    } else {
        None
    };

    match zone.kind {
        // ── Reorder within same pane ────────────────────────────────
        DropZoneKind::TabReorder(insert_idx) if to == from => {
            // Drop at the same position: no-op.
            if cur_idx == insert_idx || cur_idx + 1 == insert_idx {
                return TabDropInstruction::NoOp;
            }
            // After removal, indices > cur_idx shift down by 1.
            let adj = if insert_idx > cur_idx {
                insert_idx - 1
            } else {
                insert_idx
            };
            let adj = adj.min(from_count.saturating_sub(1));
            TabDropInstruction::Reorder {
                pane_idx: from,
                from_idx: cur_idx,
                to_idx: adj,
            }
        }

        // ── No-op: dropped on own content area ──────────────────────
        DropZoneKind::Center if to == from => TabDropInstruction::NoOp,

        // ── Merge: move tab to another pane ─────────────────────────
        DropZoneKind::Center | DropZoneKind::TabReorder(_) => {
            let to_count = pane_tab_counts.get(to).copied().unwrap_or(0);
            let insert_idx = reorder_insert_idx
                .map(|idx| idx.min(to_count))
                .unwrap_or(to_count);
            TabDropInstruction::MoveToPane {
                from_pane_idx: from,
                from_idx: cur_idx,
                to_pane_idx: to,
                insert_idx,
                source_now_empty: from_count <= 1,
            }
        }

        // ── Split: create a new adjacent pane ───────────────────────
        DropZoneKind::Split(edge) => {
            // Guard: splitting the only tab of the only pane is a no-op.
            if from_count == 1 && pane_tab_counts.len() == 1 {
                return TabDropInstruction::NoOp;
            }
            let split_direction = match edge {
                DropEdge::Left | DropEdge::Right => SplitDirection::Horizontal,
                DropEdge::Top | DropEdge::Bottom => SplitDirection::Vertical,
            };
            TabDropInstruction::SplitToNewPane {
                from_pane_idx: from,
                from_idx: cur_idx,
                target_pane_idx: to,
                edge,
                split_direction,
                source_now_empty: from_count <= 1,
            }
        }
    }
}

// ── Controller-owned in-progress drag ───────────────────────────────────

/// `TabGroupController`'s own in-progress tab drag record — mutually
/// exclusive with its `dragging_divider` field (a
/// [`crate::compose::divider_drag::DividerDrag`]).
pub(crate) struct TabDragState {
    /// Index of the pane the drag started in.
    pub(crate) source_pane_idx: usize,
    /// Id of the tab being dragged.
    pub(crate) tab_id: String,
    /// Most recently computed drop zone, updated on every drag-move.
    pub(crate) current_zone: Option<DropZone>,
    /// Last known cursor position, used to paint the overlay on render.
    pub(crate) cursor_x: f32,
    pub(crate) cursor_y: f32,
}
