//! Hit testing against a frame's painted primitives: [`PaintedHit`] records
//! what was painted where, [`hit_test_painted`] turns a pointer position
//! into the id and fine-grained [`HitDetail`] it landed on, and
//! [`event_position`]/[`set_event_widget`] read and write the one `widget`
//! field a [`quadraui::UiEvent`] this module enriches carries.

use std::rc::Rc;

use quadraui::{
    ButtonHit, ButtonLayout, DataTableHit, DataTableLayout, ListViewHit, ListViewLayout, Point,
    PressZone, Rect, Scrollbar, SplitHit, SplitLayout, TabBarHit, TabBarLayout, TreePath,
    TreeViewHit, TreeViewLayout, UiEvent, WidgetId,
};

use crate::extra_paint::ExtraPaint;

/// One primitive this app painted during the current frame: its id and the
/// hit-testable layout its backend `draw_*` call returned (or, for
/// `Backend::draw_tree`/`draw_list`, which paint without returning one, the
/// layout its no-paint twin `tree_layout`/`list_layout` produced for the
/// same `area`). Recorded into the binding's own hit-map cache by `draw_bands` /
/// `paint_layout` / `paint_leaf`, in paint order, and consulted by
/// [`hit_test_painted`] to translate an incoming pointer event's screen
/// position into the primitive (and sub-region) under it.
pub enum PaintedHit {
    /// `Button`'s layout is in the backend's ABSOLUTE coordinate frame, so
    /// no `area` is stored alongside it — same as [`Self::Split`].
    Button { id: WidgetId, layout: ButtonLayout },
    /// `area` is the absolute rect this tree was painted into. `Tree`'s
    /// layout (like `List`/`DataTable`/`TabBar`'s) is in the LOCAL
    /// coordinate frame, relative to `area`'s origin, so a hit-test
    /// against it must first translate the click into that frame.
    /// `row_paths[i]` is the `TreePath` of the row at visual index `i`
    /// (i.e. `TreeView::rows[i].path`), kept alongside the layout so a
    /// `TreeViewHit::Row(i)` can be turned into a semantic
    /// `UiEvent::Tree(RowClicked)` without re-borrowing the `TreeView`
    /// itself. `area` is already narrowed to exclude `scrollbar`'s own
    /// gutter (see `draw_tree_with_scrollbar`), so a row hit-test never
    /// sees a point that landed on the scrollbar. `scrollbar` is `None`
    /// when the tree's rows fit without scrolling. `scroll_offset` is the
    /// tree's own offset as painted; `viewport_rows` is how many rows fit
    /// in `area` at a zero offset (snapshotted at paint time, since a
    /// `Scrollbar`'s `thumb_len` alone can't be inverted back to it once
    /// `min_thumb_len` clamping has applied). Both feed the scrollbar
    /// drag/page/wheel math in the binding's own event handler without needing to
    /// re-borrow the `TreeView` or re-touch the backend.
    Tree {
        id: WidgetId,
        area: Rect,
        layout: TreeViewLayout,
        row_paths: Vec<TreePath>,
        scrollbar: Option<Scrollbar>,
        scroll_offset: usize,
        viewport_rows: usize,
    },
    List {
        id: WidgetId,
        area: Rect,
        layout: ListViewLayout,
    },
    DataTable {
        id: WidgetId,
        area: Rect,
        layout: DataTableLayout,
        scroll_offset: usize,
        total_rows: usize,
    },
    TabBar {
        id: WidgetId,
        area: Rect,
        layout: TabBarLayout,
    },
    /// `Split`'s layout is already in the ABSOLUTE frame, and its own
    /// `WidgetId` only matters once a hit is found — carried by
    /// `SplitHit`'s own variants, not stored here.
    Split { layout: SplitLayout },
    /// One additive primitive (anything registered in [`EXTRA_PRIMITIVES`]),
    /// painted by `draw_bands`'s extras loop or by a [`LeafDescriptor::Extra`]
    /// leaf. `area` is the ABSOLUTE rect it was painted into — the same frame
    /// [`ExtraPaint::hit_test`] expects, so no translation is needed before
    /// calling it (matching `Button`/`Switch`'s own layout convention: no
    /// local-frame remap). `extra` is an [`Rc`] clone of the slot's/leaf's
    /// descriptor, kept alive past this frame's render so a later `handle`
    /// call can still hit-test and tab-stop against it.
    Extra {
        id: WidgetId,
        area: Rect,
        extra: Rc<dyn ExtraPaint>,
    },
}

/// The sub-region of a primitive an incoming pointer event landed on,
/// attached to the enriched event's `"hit"` key (alongside `widget`, the
/// primitive's own id) so a Python handler can react to "row 3" or "the
/// divider" without re-deriving hit geometry itself.
#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HitDetail {
    ButtonBody,
    TreeRow {
        row: usize,
    },
    TreeChevron {
        row: usize,
    },
    ListTitle,
    ListItem {
        index: usize,
    },
    DataTableHeader {
        col: usize,
    },
    DataTableHeaderDivider {
        col: usize,
    },
    DataTableRow {
        row: usize,
    },
    DataTableFooter,
    TabBarTab {
        tab: usize,
    },
    TabBarTabClose {
        tab: usize,
    },
    TabBarScrollLeft,
    TabBarScrollRight,
    TabBarRightSegment,
    SplitFirstPane,
    SplitSecondPane,
    SplitDivider,
    /// Landed before the thumb on a tree's vertical scrollbar track
    /// (page-up zone). Handled directly in the binding's own event handler, which
    /// replaces the raw click with `UiEvent::ScrollOffsetChanged` — never
    /// reaches `semantic_event_for_hit`.
    TreeScrollbarTrackBefore,
    /// Landed on a tree's vertical scrollbar thumb. Arms a drag in
    /// the binding's own event handler (mirroring `SplitDivider`'s `ActiveDrag`); never
    /// reaches `semantic_event_for_hit`.
    TreeScrollbarThumb,
    /// Landed after the thumb on a tree's vertical scrollbar track
    /// (page-down zone). Handled directly in the binding's own event handler, same as
    /// `TreeScrollbarTrackBefore`.
    TreeScrollbarTrackAfter,
    /// Landed on an additive primitive's interactive region (currently
    /// [`Switch`] and [`TextInput`]). Carries no sub-region detail yet —
    /// just enough for [the binding's own event handler] to set `widget` and attach
    /// this as `hit`, so a Python handler sees the click reached the
    /// primitive. Per-kind sub-region detail (e.g. which combo-box row) is
    /// a later, separate extension of this variant.
    ExtraBody,
}

/// Hit-tests `point` (in the backend's native absolute coordinate space,
/// same as `UiEvent::MouseDown`'s `position`) against every primitive
/// painted this frame, returning the id and fine-grained detail of
/// whichever one claims it.
///
/// Checked in reverse paint order — later-painted primitives sit on top of
/// earlier ones — and a miss against one candidate (e.g. outside its own
/// paint `area`, or `TreeViewHit::Empty` inside it) falls through to the
/// next rather than stopping, so a click that misses the topmost primitive
/// can still land on one painted earlier.
pub fn hit_test_painted(hit_map: &[PaintedHit], point: Point) -> Option<(WidgetId, HitDetail)> {
    for hit in hit_map.iter().rev() {
        match hit {
            PaintedHit::Button { id, layout } => {
                if matches!(layout.hit_test(point), ButtonHit::Body) {
                    return Some((*id, HitDetail::ButtonBody));
                }
            }
            PaintedHit::Tree {
                id,
                area,
                layout,
                scrollbar,
                ..
            } => {
                if let Some(scrollbar) = scrollbar {
                    if scrollbar.track.contains(point) {
                        return Some((
                            *id,
                            match scrollbar.press_zone(point.y) {
                                PressZone::Before => HitDetail::TreeScrollbarTrackBefore,
                                PressZone::Thumb => HitDetail::TreeScrollbarThumb,
                                PressZone::After => HitDetail::TreeScrollbarTrackAfter,
                            },
                        ));
                    }
                }
                if !area.contains(point) {
                    continue;
                }
                match layout.hit_test(point.x - area.x, point.y - area.y) {
                    TreeViewHit::Row(row) => return Some((*id, HitDetail::TreeRow { row })),
                    TreeViewHit::Chevron(row) => {
                        return Some((*id, HitDetail::TreeChevron { row }))
                    }
                    TreeViewHit::Empty => {}
                }
            }
            PaintedHit::List { id, area, layout } => {
                if !area.contains(point) {
                    continue;
                }
                match layout.hit_test(point.x - area.x, point.y - area.y) {
                    ListViewHit::Title => return Some((*id, HitDetail::ListTitle)),
                    ListViewHit::Item(index) => return Some((*id, HitDetail::ListItem { index })),
                    ListViewHit::Empty => {}
                }
            }
            PaintedHit::DataTable {
                id,
                area,
                layout,
                scroll_offset,
                total_rows,
            } => {
                if !area.contains(point) {
                    continue;
                }
                match layout.hit_test(
                    point.x - area.x,
                    point.y - area.y,
                    *scroll_offset,
                    *total_rows,
                ) {
                    DataTableHit::Header { col } => {
                        return Some((*id, HitDetail::DataTableHeader { col }))
                    }
                    DataTableHit::HeaderDivider { col } => {
                        return Some((*id, HitDetail::DataTableHeaderDivider { col }))
                    }
                    DataTableHit::Row { idx } => {
                        return Some((*id, HitDetail::DataTableRow { row: idx }))
                    }
                    DataTableHit::Footer => return Some((*id, HitDetail::DataTableFooter)),
                    DataTableHit::Empty => {}
                }
            }
            PaintedHit::TabBar { id, area, layout } => {
                if !area.contains(point) {
                    continue;
                }
                match layout.hit_test(point.x - area.x, point.y - area.y) {
                    TabBarHit::Tab(tab) => return Some((*id, HitDetail::TabBarTab { tab })),
                    TabBarHit::TabClose(tab) => {
                        return Some((*id, HitDetail::TabBarTabClose { tab }))
                    }
                    TabBarHit::ScrollLeft => return Some((*id, HitDetail::TabBarScrollLeft)),
                    TabBarHit::ScrollRight => return Some((*id, HitDetail::TabBarScrollRight)),
                    TabBarHit::RightSegment(right_id) => {
                        return Some((right_id, HitDetail::TabBarRightSegment))
                    }
                    TabBarHit::Empty => {}
                }
            }
            PaintedHit::Split { layout } => match layout.hit_test(point.x, point.y) {
                SplitHit::FirstPane(id) => return Some((id, HitDetail::SplitFirstPane)),
                SplitHit::SecondPane(id) => return Some((id, HitDetail::SplitSecondPane)),
                SplitHit::Divider(id) => return Some((id, HitDetail::SplitDivider)),
                SplitHit::Outside => {}
            },
            PaintedHit::Extra { id, area, extra } => {
                if extra.hit_test(*area, point) {
                    return Some((*id, HitDetail::ExtraBody));
                }
            }
        }
    }
    None
}

/// The screen position to hit-test against [`PaintedHit`]s, for the one
/// `event` variants whose own `widget` field this module enriches. `None`
/// for every other variant (e.g. `MouseMoved`, `MouseEntered`, key events),
/// which [the binding's own event handler] forwards to Python exactly as received.
pub fn event_position(event: &UiEvent) -> Option<Point> {
    match event {
        UiEvent::MouseDown { position, .. }
        | UiEvent::MouseUp { position, .. }
        | UiEvent::DoubleClick { position, .. }
        | UiEvent::Scroll { position, .. } => Some(*position),
        _ => None,
    }
}

/// Sets `event`'s own `widget` field to `id`, for whichever of the four
/// variants [`event_position`] matched. A no-op for every other variant.
pub fn set_event_widget(event: &mut UiEvent, id: WidgetId) {
    match event {
        UiEvent::MouseDown { widget, .. }
        | UiEvent::MouseUp { widget, .. }
        | UiEvent::DoubleClick { widget, .. }
        | UiEvent::Scroll { widget, .. } => *widget = Some(id),
        _ => {}
    }
}
