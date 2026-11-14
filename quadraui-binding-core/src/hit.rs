//! Hit testing against a frame's painted primitives: [`PaintedHit`] records
//! what was painted where, [`hit_test_painted`] turns a pointer position
//! into the id and fine-grained [`HitDetail`] it landed on, and
//! [`event_position`]/[`set_event_widget`] read and write the one `widget`
//! field a [`quadraui::UiEvent`] this module enriches carries.

use std::rc::Rc;

use quadraui::{
    ButtonHit, ButtonLayout, DataTableHit, DataTableLayout, ListViewHit, ListViewLayout, PanelHit,
    PanelLayout, Point, PressZone, Rect, ScrollAxis, Scrollbar, SidebarPanelHit,
    SidebarPanelLayout, SplitHit, SplitLayout, SplitTreeLayout, TabBarHit, TabBarLayout,
    TextDisplayHit, TextDisplayLayout, Toolbar, TreePath, TreeRowEditState, TreeViewHit,
    TreeViewLayout, UiEvent, WidgetId,
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
    /// `area` is the absolute rect this tree was painted into, and the
    /// frame `layout` is relative to (like `List`/`DataTable`/`TabBar`'s
    /// own layout) — a hit-test must translate a click into it via
    /// `area`'s origin before calling `layout.hit_test`, and `area` is
    /// never narrowed by [`clip_hits_to_viewport`] for exactly that
    /// reason. `clip` is the sub-rect of `area` actually inside every
    /// enclosing `Scroll`'s own viewport (equal to `area` until a `Scroll`
    /// narrows it) — checked for containment in `clip`'s place, so a point
    /// outside an enclosing scroll's viewport never resolves to this tree
    /// even though `area`'s own origin (hence the frame `layout.hit_test`
    /// translates into) is untouched. `row_paths[i]` is the `TreePath` of
    /// the row at visual index `i` (i.e. `TreeView::rows[i].path`), kept
    /// alongside the layout so a `TreeViewHit::Row(i)` can be turned into a
    /// semantic `UiEvent::Tree(RowClicked)` without re-borrowing the
    /// `TreeView` itself. `area` is already narrowed to exclude
    /// `scrollbar`'s own gutter (see `draw_tree_with_scrollbar`), so a row
    /// hit-test never sees a point that landed on the scrollbar.
    /// `scrollbar` is `None` when the tree's rows fit without scrolling —
    /// its own `track` is an absolute rect (see [`quadraui::Scrollbar`]'s
    /// own doc), so [`clip_hits_to_viewport`] narrows it directly, the same
    /// way it narrows [`Self::Button`]'s `layout.bounds`. `scroll_offset`
    /// is the tree's own offset as painted; `viewport_rows` is how many
    /// rows fit in `area` at a zero offset (snapshotted at paint time,
    /// since a `Scrollbar`'s `thumb_len` alone can't be inverted back to
    /// it once `min_thumb_len` clamping has applied). Both feed the
    /// scrollbar drag/page/wheel math in the binding's own event handler
    /// without needing to re-borrow the `TreeView` or re-touch the backend.
    /// `editing` is the path and edit state of whichever row currently has
    /// `TreeRow::edit` set (`None` when no row is mid-rename), read by
    /// [`crate::semantic::focus_scoped_key_event`] so a key pressed while
    /// that row is being renamed is resolved by
    /// [`crate::tree_row_rename_key_event`] instead of falling straight
    /// through to the tree's raw `TreeEvent::KeyPressed`.
    Tree {
        id: WidgetId,
        area: Rect,
        clip: Rect,
        layout: TreeViewLayout,
        row_paths: Vec<TreePath>,
        scrollbar: Option<Scrollbar>,
        scroll_offset: usize,
        viewport_rows: usize,
        editing: Option<(TreePath, TreeRowEditState)>,
    },
    List {
        id: WidgetId,
        area: Rect,
        /// See [`Self::Tree`]'s own doc: the sub-rect of `area` inside
        /// every enclosing `Scroll`'s viewport, checked for containment in
        /// `area`'s place so `area`'s origin (hence the frame
        /// `layout.hit_test` translates a click into) stays untouched.
        clip: Rect,
        layout: ListViewLayout,
        /// The list's current row selection and item count, as painted —
        /// read by [`crate::semantic::focus_scoped_key_event`] so Enter on
        /// a focused list can be resolved against what was actually
        /// painted, the same way [`Self::DataTable`]'s `selected_idx`/
        /// `total_rows` feed its own Enter handling. `selected_idx` is
        /// `ListView::selected_idx` verbatim (always a valid index into a
        /// non-empty list); `total_items` is `ListView::items.len()`, and
        /// `0` is what tells the key handler there is no row to activate.
        selected_idx: usize,
        total_items: usize,
        /// `ListView::scroll_offset` as painted, and how many rows fit in
        /// `area` at a zero offset (snapshotted at paint time, mirroring
        /// [`Self::Tree`]'s own `scroll_offset`/`viewport_rows`) — read by
        /// `crate::host::apply_list_nav_key`/`crate::host::resolve_list_cursor`
        /// to move the uncontrolled cursor and scroll it into view.
        scroll_offset: usize,
        viewport_rows: usize,
    },
    DataTable {
        id: WidgetId,
        area: Rect,
        /// See [`Self::Tree`]'s own doc.
        clip: Rect,
        layout: DataTableLayout,
        scroll_offset: usize,
        total_rows: usize,
        /// The table's current row selection, as painted — read by
        /// [`crate::semantic::focus_scoped_key_event`] to compute the next
        /// row an Up/Down arrow key should select, the same way
        /// [`Self::Tree`]'s `row_paths` lets a key event resolve against
        /// what was actually painted rather than re-deriving it.
        selected_idx: Option<usize>,
        /// How many rows fit in `area` at a zero scroll offset — mirrors
        /// [`Self::Tree`]'s own `viewport_rows`, read by
        /// `crate::host::apply_data_table_nav_key`/
        /// `crate::host::resolve_data_table_cursor` to scroll the
        /// uncontrolled cursor into view.
        viewport_rows: usize,
    },
    TabBar {
        id: WidgetId,
        area: Rect,
        /// See [`Self::Tree`]'s own doc.
        clip: Rect,
        layout: TabBarLayout,
        /// The bar's own tab count and keyboard-focused tab, as painted —
        /// read by [`crate::semantic::focus_scoped_key_event`] so an arrow
        /// key or Enter on a focused `TabBar` resolves through
        /// [`quadraui::TabBar::handle_focus_key`] the same way
        /// [`Self::List`]'s `selected_idx`/`total_items` feed
        /// [`crate::semantic::list_key_event`]. `focused_index` is `None`
        /// for a bare `TabBar` leaf (no [`quadraui::TabChrome`] involved)
        /// and for a `"tabs"` layout node whose `focused_index` wasn't set;
        /// `tab_count` is `TabBar::tabs.len()` regardless.
        focused_index: Option<usize>,
        tab_count: usize,
    },
    /// `area` is the absolute rect this display was painted into;
    /// `layout` is in a frame relative to `area`'s origin **shifted down by
    /// `body_offset`** — unlike [`Self::Tree`]/[`Self::List`]/[`Self::DataTable`],
    /// whose layouts include their own title row, every backend's
    /// `text_display_layout` lays the body out from `0.0` and leaves the
    /// title row (when `TextDisplay::title.is_some()`) to be painted
    /// separately above it. `body_offset` is that row's height (one
    /// `Backend::line_height`, or `0.0` when there's no title), so a
    /// hit-test must subtract `area`'s origin AND `body_offset` before
    /// calling `layout.hit_test`. `clip` is [`Self::Tree`]'s own
    /// enclosing-viewport sub-rect of `area`, checked for containment in
    /// `area`'s place for the same reason. `total_lines` and
    /// `viewport_lines` (the row capacity at a zero scroll offset,
    /// snapshotted the same way [`Self::Tree`]'s own `viewport_rows` is)
    /// feed the wheel-scroll `max_offset` clamp in
    /// `InteractionEngine::handle` without re-touching the backend.
    TextDisplay {
        id: WidgetId,
        area: Rect,
        clip: Rect,
        layout: TextDisplayLayout,
        body_offset: f32,
        total_lines: usize,
        viewport_lines: usize,
    },
    /// `Split`'s layout is already in the ABSOLUTE frame, and its own
    /// `WidgetId` only matters once a hit is found — carried by
    /// `SplitHit`'s own variants, not stored here.
    Split { layout: SplitLayout },
    /// A [`quadraui::SplitTree`] node's layout, in the ABSOLUTE frame
    /// (matching [`Self::Split`]). `id` is the tree's own host-supplied id
    /// — unlike `SplitHit`, [`SplitTreeLayout::hit_test_divider`] only
    /// resolves to a `split_index`, not an id, so it's carried alongside
    /// the layout here instead. Divider-only: a hit anywhere else in the
    /// tree's bounds (a leaf's content, or the gap between leaves) is left
    /// to whatever [`crate::layout::LayoutNode::SplitTree`]'s own
    /// per-leaf content paints underneath, which `crate::render::paint_layout`
    /// pushes AFTER this entry so it wins the reverse-paint-order check
    /// first.
    SplitTree {
        id: WidgetId,
        layout: SplitTreeLayout,
    },
    /// A [`quadraui::Panel`]'s chrome layout, in the ABSOLUTE frame
    /// (matching [`Self::Split`]). `id` is the panel's own id; a
    /// `PanelHit::Action`/`PanelHit::TitleBar` resolves to its OWN id
    /// (the action button's, or the panel's), same convention `SplitHit`'s
    /// variants use. `PanelHit::Content` is left to the panel's own
    /// content subtree, pushed after this entry by
    /// `crate::render::paint_layout`'s `LayoutNode::Panel` arm.
    Panel { id: WidgetId, layout: PanelLayout },
    /// A [`quadraui::SidebarPanel`]'s chrome layout, in the ABSOLUTE frame.
    /// Mirrors [`Self::Panel`]: a `SidebarPanelHit::ToolbarButton` resolves
    /// to the clicked button's own id; `SidebarPanelHit::Content` is left
    /// to the content subtree pushed after this entry. `toolbar` is a
    /// clone of [`quadraui::SidebarPanel::toolbar`] as painted — `None`
    /// whenever `layout.toolbar_bounds` is (no header slot reserved);
    /// `Some` otherwise, kept alongside the layout (which only carries
    /// bounds, not the button descriptors) so
    /// [`crate::semantic::semantic_event_for_hit`] can resolve a
    /// `SidebarPanelToolbarButton` hit into a typed `UiEvent::Toolbar`
    /// the same way [`Self::Extra`]'s own `Toolbar` chrome does.
    SidebarPanel {
        id: WidgetId,
        layout: SidebarPanelLayout,
        toolbar: Option<Toolbar>,
    },
    /// One additive primitive (anything registered in a binding's own
    /// `EXTRA_PRIMITIVES` decode table),
    /// painted by `draw_bands`'s extras loop or by a
    /// [`crate::layout::LeafDescriptor::Extra`]
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
        /// Absolute bounds + action id of each clickable child
        /// [`ExtraPaint::paint_chrome`] resolved this frame — empty for
        /// every extra but `Toolbar`'s buttons, `StatusBar`'s segments, and
        /// `ActivityBar`'s items. Checked by [`hit_test_painted`] (and
        /// [`chrome_hover_hit`], for `MouseMoved`) ahead of
        /// [`ExtraPaint::hit_test`], so a click on one of these children
        /// resolves to the *child's* id rather than just "landed somewhere
        /// on the bar".
        chrome_hits: Vec<(Rect, WidgetId)>,
    },
    /// A [`crate::layout::LayoutNode::Scroll`]'s own viewport, in the
    /// ABSOLUTE frame (matching [`Self::Split`]) — `viewport` already
    /// excludes the scrollbar gutter `crate::render::paint_layout`'s
    /// `LayoutNode::Scroll` arm reserves. Pushed BEFORE that arm recurses
    /// into its (culled, offset) content, same ordering as [`Self::Panel`]/
    /// [`Self::SidebarPanel`]'s own chrome hit — a descendant content hit
    /// (pushed after, so checked first in reverse order) wins over this
    /// one. Content scrolled out of view never gets its own
    /// [`PaintedHit`] at all (culled before painting), so there is nothing
    /// for this entry to mask; it exists so a click or wheel landing on
    /// empty space inside the viewport (nothing painted under the pointer)
    /// still resolves to the `Scroll` itself, via [`HitDetail::ScrollBody`].
    /// `axis`/`content_len`/`offset` are the same values
    /// [`crate::render::paint_scroll`] fed its own [`quadraui::ScrollView`]
    /// this frame — read back by the engine's wheel/page-key/drag handling
    /// to resolve a new offset without re-measuring the content. `scrollbar`
    /// is `None` when the content fits without scrolling, mirroring
    /// [`Self::Tree`]'s own `scrollbar` field; its `track` is an absolute
    /// rect, outside `viewport` (reserved alongside it, not inside it), and
    /// is narrowed by [`clip_hits_to_viewport`] the same way it narrows
    /// [`Self::Tree`]'s — a `Scroll` nested inside another `Scroll`'s
    /// viewport needs its own track clipped to the OUTER viewport too, the
    /// same class of leak a nested `Tree`'s track is already guarded
    /// against. `children` is one [`ScrollChildHit`] per direct
    /// [`crate::layout::LayoutNode::Leaf`] child when this scroll's
    /// content is a `Column`/`Row` matching its own axis — read by the
    /// engine to scroll a child that just received focus fully into view.
    /// Empty for `Atomic` content or a non-`Leaf` direct child (a nested
    /// `Row`/`Column`/`Split`/…), where that scroll-into-view isn't
    /// implemented yet.
    Scroll {
        id: WidgetId,
        viewport: Rect,
        axis: ScrollAxis,
        content_len: f32,
        offset: f32,
        scrollbar: Option<Scrollbar>,
        children: Vec<ScrollChildHit>,
    },
}

/// One [`PaintedHit::Scroll::children`] entry: `id` is the direct
/// [`crate::layout::LayoutNode::Leaf`] child's own id; `start`/`len` are
/// its main-axis span within the scroll's content, content-relative — NOT
/// yet offset by [`PaintedHit::Scroll::offset`], and not clamped to
/// `viewport` the way the child's own painted rect may be, since the
/// engine's scroll-into-view math needs the un-clamped span. Mirrors
/// [`crate::render::ScrollChildSlot`]'s own `start`/`len` fields, which
/// this struct is built from.
#[derive(Debug, Clone, PartialEq)]
pub struct ScrollChildHit {
    pub id: WidgetId,
    pub start: f32,
    pub len: f32,
}

/// Intersects `hits[start..]` — one [`crate::render::paint_scroll`] call's
/// own descendant content, painted inside `viewport` between its
/// `Backend::push_clip(viewport)`/`pop_clip` pair — with `viewport`
/// itself, dropping an entry whose bounds don't overlap it at all.
///
/// Without this, a descendant's own recorded bounds can reach past
/// `viewport` on a backend whose `Rect` carries a negative-origin main
/// axis correctly (every non-cell-grid one — see
/// [`crate::render::scroll_content_slots`]'s own `cell_grid` doc, which
/// already clips a cell-grid backend's bounds before they're ever
/// painted, making this pass a no-op there), or on any backend whose
/// `ScrollSlots::Atomic` content is sized to the scroll's full
/// `content_len` rather than clamped to one viewport's worth (the common
/// case — see `scroll_content_slots`'s own doc). Checked in reverse paint
/// order by [`hit_test_painted`], a hit whose bounds leak past its own
/// scroll's viewport can otherwise mask a sibling painted earlier at that
/// same screen position (e.g. a button above a scroll whose own
/// oversized content reaches back up over it) — this keeps every
/// descendant's bounds inside the viewport it was actually painted into,
/// so a point outside it never resolves to that scroll's content and a
/// sibling painted before the scroll keeps its own clicks.
///
/// Only the hit kinds whose containment test lives in a plain field this
/// module can reach are narrowed, via [`primary_bounds_mut`]:
/// [`PaintedHit::Button`]'s `layout.bounds`; [`PaintedHit::Extra`]'s
/// `area`; [`PaintedHit::Tree`]/[`PaintedHit::List`]/
/// [`PaintedHit::DataTable`]/[`PaintedHit::TabBar`]/
/// [`PaintedHit::TextDisplay`]'s own `clip` (NOT `area` — see
/// [`PaintedHit::Tree`]'s own doc for why moving a local-frame kind's
/// `area` instead would desync it from the layout `area`'s origin
/// translates a click into); and a nested [`PaintedHit::Scroll`]'s own
/// `viewport`. Two more sub-rect collections, checked by
/// [`hit_test_painted`] ahead of a kind's own primary bounds, are narrowed
/// alongside it: [`PaintedHit::Tree`]'s and [`PaintedHit::Scroll`]'s own
/// `scrollbar.track` (an absolute rect, like `Button`'s `layout.bounds` —
/// narrowed directly, never dropping the surrounding `Tree`/`Scroll` entry
/// even when the track itself ends up with no overlap) and
/// [`PaintedHit::Extra`]'s `chrome_hits` (each
/// entry independently intersected and dropped on no overlap, the same
/// per-entry treatment `hits` itself gets). The chrome-layout kinds
/// (`Split`/`SplitTree`/`Panel`/`SidebarPanel`) carry several independent
/// sub-region rects apiece with no single outer bounds to narrow, and
/// aren't realistic `Scroll` content — left as-is.
pub(crate) fn clip_hits_to_viewport(hits: &mut Vec<PaintedHit>, start: usize, viewport: Rect) {
    let mut i = start;
    while i < hits.len() {
        let Some(bounds) = primary_bounds_mut(&mut hits[i]).map(|r| *r) else {
            i += 1;
            continue;
        };
        let Some(clipped) = intersect_rect(bounds, viewport) else {
            hits.remove(i);
            continue;
        };
        *primary_bounds_mut(&mut hits[i]).expect("checked above") = clipped;
        match &mut hits[i] {
            PaintedHit::Tree {
                scrollbar: Some(scrollbar),
                ..
            }
            | PaintedHit::Scroll {
                scrollbar: Some(scrollbar),
                ..
            } => {
                scrollbar.track = clip_or_empty(scrollbar.track, viewport);
            }
            PaintedHit::Extra { chrome_hits, .. } => {
                chrome_hits.retain_mut(|(rect, _)| match intersect_rect(*rect, viewport) {
                    Some(clipped) => {
                        *rect = clipped;
                        true
                    }
                    None => false,
                });
            }
            _ => {}
        }
        i += 1;
    }
}

/// The hit kind's own containment-test rect — [`PaintedHit::Button`]'s
/// `layout.bounds`, [`PaintedHit::Extra`]'s `area`, the local-frame kinds'
/// own `clip` (see [`PaintedHit::Tree`]'s doc), and a nested
/// [`PaintedHit::Scroll`]'s own `viewport` — or `None` for a chrome-layout
/// kind with no single outer bounds. The one place [`clip_hits_to_viewport`]
/// reaches into a [`PaintedHit`] by mutable reference, so narrowing one
/// kind's field and narrowing a different kind's field can't drift apart
/// into two independently-maintained match arms.
fn primary_bounds_mut(hit: &mut PaintedHit) -> Option<&mut Rect> {
    match hit {
        PaintedHit::Button { layout, .. } => Some(&mut layout.bounds),
        PaintedHit::Tree { clip, .. }
        | PaintedHit::List { clip, .. }
        | PaintedHit::DataTable { clip, .. }
        | PaintedHit::TabBar { clip, .. }
        | PaintedHit::TextDisplay { clip, .. } => Some(clip),
        PaintedHit::Extra { area, .. } => Some(area),
        PaintedHit::Scroll { viewport, .. } => Some(viewport),
        PaintedHit::Split { .. }
        | PaintedHit::SplitTree { .. }
        | PaintedHit::Panel { .. }
        | PaintedHit::SidebarPanel { .. } => None,
    }
}

/// `rect` intersected with `viewport`, or a zero-area rect at `rect`'s own
/// origin when they don't overlap at all — for a hit's own secondary
/// sub-rect (a scrollbar's `track`, one `chrome_hits` entry) where "no
/// longer inside the viewport" should make just that sub-rect
/// unreachable, never drop the [`PaintedHit`] entry it's attached to (its
/// own primary bounds, narrowed separately by [`clip_hits_to_viewport`],
/// already decides that).
fn clip_or_empty(rect: Rect, viewport: Rect) -> Rect {
    intersect_rect(rect, viewport).unwrap_or(Rect::new(rect.x, rect.y, 0.0, 0.0))
}

/// The overlapping region of `a` and `b`, or `None` when they don't
/// overlap at all (zero-width/height counts as no overlap, matching
/// [`Rect::contains`]'s own half-open convention).
fn intersect_rect(a: Rect, b: Rect) -> Option<Rect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.width).min(b.x + b.width);
    let y1 = (a.y + a.height).min(b.y + b.height);
    if x1 > x0 && y1 > y0 {
        Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
    } else {
        None
    }
}

/// The sub-region of a primitive an incoming pointer event landed on,
/// attached to the enriched event's `"hit"` key (alongside `widget`, the
/// primitive's own id) so the binding's own event handler can react to
/// "row 3" or "the divider" without re-deriving hit geometry itself.
///
/// `PartialEq`/`Eq` let [`InteractionEngine::handle`][crate::engine::InteractionEngine::handle]
/// compare a `MouseMoved`'s new hit against its previous one to decide
/// whether to synthesize `MouseEntered`/`MouseLeft` — two hits with the
/// same `WidgetId` but a different detail (e.g. a `Tree`'s row 2 vs. row
/// 3) still count as a hover change.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
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
    TextDisplayLine {
        line: usize,
    },
    TextDisplayScrollbarThumb,
    TextDisplayScrollbarTrackBefore,
    TextDisplayScrollbarTrackAfter,
    SplitFirstPane,
    SplitSecondPane,
    SplitDivider,
    /// Landed on a [`quadraui::SplitTree`] divider — `split_index` is the
    /// pre-order index of the `SplitTree::Split` node being resized (see
    /// [`quadraui::SplitTreeDivider::split_index`]). Arms a drag in the
    /// binding's own event handler, mirroring `SplitDivider`'s
    /// [`crate::drag::ActiveDrag`] handling.
    SplitTreeDivider {
        split_index: usize,
    },
    /// Landed on a [`quadraui::Panel`]'s title-bar action button. `id`
    /// resolves to the matched hit returned alongside this detail as the
    /// action button's own id (`PanelAction::id`), same as a plain
    /// `Button` click.
    PanelAction,
    /// Landed on a [`quadraui::Panel`]'s title bar (not an action button).
    /// `id` resolves to the panel's own id.
    PanelTitleBar,
    /// Landed on a [`quadraui::SidebarPanel`]'s toolbar button. `id`
    /// resolves to the matched hit returned alongside this detail as the
    /// clicked button's own id, same as `PanelAction`. `bar` is the
    /// embedded [`quadraui::Toolbar`]'s own id (not the `SidebarPanel`'s),
    /// carried here so [`crate::semantic::semantic_event_for_hit`] can look
    /// the toolbar back up in the hit map and build its typed
    /// `UiEvent::Toolbar`, mirroring `ExtraChrome::bar`.
    SidebarPanelToolbarButton {
        bar: WidgetId,
    },
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
    /// [`quadraui::Switch`] and [`quadraui::TextInput`]). Carries no
    /// sub-region detail yet —
    /// just enough for the binding's own event handler to set `widget` and
    /// attach this as `hit`, so it sees the click reached the primitive.
    /// Per-kind sub-region detail (e.g. which combo-box row) is a later,
    /// separate extension of this variant.
    ExtraBody,
    /// Landed on one of a chrome-bar extra's clickable children
    /// (`Toolbar`'s button, `StatusBar`'s segment, `ActivityBar`'s item).
    /// [`hit_test_painted`] returns the CHILD's own id alongside this
    /// detail (mirroring [`TabBarHit::RightSegment`]'s button-id-wins
    /// convention) — a raw `MouseDown` with no semantic substitution (a
    /// `Toolbar` button: no typed `UiEvent` exists for it) therefore gets
    /// `widget` set to the button itself, same as a plain `Button` click.
    /// `bar` is the enclosing bar's own id, carried here so
    /// [`crate::semantic::semantic_event_for_hit`] can look it back up in
    /// the hit map to build `StatusBar`'s/`ActivityBar`'s typed event
    /// (which wraps the bar's id, not the clicked child's).
    ExtraChrome {
        bar: WidgetId,
    },
    /// Landed inside a [`crate::layout::LayoutNode::Scroll`]'s viewport but
    /// on no more specific descendant hit — e.g. empty space between
    /// stacked content items. Carries no sub-region detail, mirroring
    /// [`Self::ExtraBody`]; the binding's own wheel routing uses this to
    /// resolve which `Scroll` a wheel event targets.
    ScrollBody,
    /// Landed before a `Scroll`'s own scrollbar thumb (page-up/left zone)
    /// — mirrors [`Self::TreeScrollbarTrackBefore`].
    ScrollScrollbarTrackBefore,
    /// Landed on a `Scroll`'s own scrollbar thumb — mirrors
    /// [`Self::TreeScrollbarThumb`].
    ScrollScrollbarThumb,
    /// Landed after a `Scroll`'s own scrollbar thumb (page-down/right zone)
    /// — mirrors [`Self::TreeScrollbarTrackAfter`].
    ScrollScrollbarTrackAfter,
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
                clip,
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
                // `clip`, not `area` — `area`'s own origin is what
                // `layout.hit_test` below translates the click into
                // (see `PaintedHit::Tree`'s own doc), and `clip` is the
                // sub-rect of it actually inside every enclosing scroll's
                // viewport.
                if !clip.contains(point) {
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
            PaintedHit::List {
                id,
                area,
                clip,
                layout,
                ..
            } => {
                if !clip.contains(point) {
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
                clip,
                layout,
                scroll_offset,
                total_rows,
                ..
            } => {
                if !clip.contains(point) {
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
            PaintedHit::TabBar {
                id,
                area,
                clip,
                layout,
                ..
            } => {
                if !clip.contains(point) {
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
            PaintedHit::TextDisplay {
                id,
                area,
                clip,
                layout,
                body_offset,
                ..
            } => {
                if !clip.contains(point) {
                    continue;
                }
                match layout.hit_test(point.x - area.x, point.y - area.y - body_offset) {
                    TextDisplayHit::Line(line) => {
                        return Some((*id, HitDetail::TextDisplayLine { line }))
                    }
                    TextDisplayHit::ScrollbarThumb => {
                        return Some((*id, HitDetail::TextDisplayScrollbarThumb))
                    }
                    TextDisplayHit::ScrollbarTrackBefore => {
                        return Some((*id, HitDetail::TextDisplayScrollbarTrackBefore))
                    }
                    TextDisplayHit::ScrollbarTrackAfter => {
                        return Some((*id, HitDetail::TextDisplayScrollbarTrackAfter))
                    }
                    TextDisplayHit::Empty => {}
                }
            }
            PaintedHit::Split { layout } => match layout.hit_test(point.x, point.y) {
                SplitHit::FirstPane(id) => return Some((id, HitDetail::SplitFirstPane)),
                SplitHit::SecondPane(id) => return Some((id, HitDetail::SplitSecondPane)),
                SplitHit::Divider(id) => return Some((id, HitDetail::SplitDivider)),
                SplitHit::Outside => {}
            },
            PaintedHit::SplitTree { id, layout } => {
                // A TUI mouse event's `point` is already quantized to whole
                // cells (`x = column as f32`), but `SplitTreeDivider::position`
                // is the unquantized float the layout resolved — e.g. ratio
                // 0.5 on a 40-column pane gives `position = 19.5`, which the
                // TUI rasteriser paints at column 19 (`cell_position`
                // truncates). A zero-tolerance band test on the raw float
                // position misses that painted cell for every fractional
                // position, i.e. half of all width/ratio combinations. `1.0`
                // is the minimum tolerance that provably covers the
                // truncation (`position - floor(position) < 1.0` always), and
                // matches the tolerance other backends' own divider hit-tests
                // use (`win::split_tree`, `gtk::split_tree`); `examples/common/split_tree_app.rs`
                // and `macos::split_tree` go further and use `2.0` for a
                // thicker visual grab target.
                if let Some(split_index) = layout.hit_test_divider(point, 1.0) {
                    return Some((*id, HitDetail::SplitTreeDivider { split_index }));
                }
            }
            PaintedHit::Panel { layout, .. } => match layout.hit_test(point.x, point.y) {
                PanelHit::Action(action_id) => return Some((action_id, HitDetail::PanelAction)),
                PanelHit::TitleBar(panel_id) => return Some((panel_id, HitDetail::PanelTitleBar)),
                PanelHit::Content(_) | PanelHit::Outside => {}
            },
            PaintedHit::SidebarPanel {
                id,
                layout,
                toolbar,
            } => match layout.hit_test(point.x, point.y) {
                SidebarPanelHit::ToolbarButton(button_id) => {
                    // `layout.toolbar_bounds` (hence this arm) is only
                    // `Some` when the panel's own `toolbar` is `Some` too
                    // — see `SidebarPanel::layout`'s `match &self.toolbar`.
                    let bar = toolbar.as_ref().map(|t| t.id).unwrap_or(*id);
                    return Some((button_id, HitDetail::SidebarPanelToolbarButton { bar }));
                }
                SidebarPanelHit::ToolbarEmpty
                | SidebarPanelHit::Content { .. }
                | SidebarPanelHit::Empty => {}
            },
            PaintedHit::Extra {
                id,
                area,
                extra,
                chrome_hits,
            } => {
                if let Some((_, action)) = chrome_hits.iter().find(|(r, _)| r.contains(point)) {
                    return Some((*action, HitDetail::ExtraChrome { bar: *id }));
                }
                if extra.hit_test(*area, point) {
                    return Some((*id, HitDetail::ExtraBody));
                }
            }
            PaintedHit::Scroll {
                id,
                viewport,
                scrollbar,
                ..
            } => {
                if let Some(scrollbar) = scrollbar {
                    if scrollbar.track.contains(point) {
                        let pos = match scrollbar.axis {
                            ScrollAxis::Vertical => point.y,
                            ScrollAxis::Horizontal => point.x,
                        };
                        return Some((
                            *id,
                            match scrollbar.press_zone(pos) {
                                PressZone::Before => HitDetail::ScrollScrollbarTrackBefore,
                                PressZone::Thumb => HitDetail::ScrollScrollbarThumb,
                                PressZone::After => HitDetail::ScrollScrollbarTrackAfter,
                            },
                        ));
                    }
                }
                if viewport.contains(point) {
                    return Some((*id, HitDetail::ScrollBody));
                }
            }
        }
    }
    None
}

/// The `(row_paths, viewport_rows)` of the `Tree` hit painted under id
/// `id` this frame, or `None` if that tree isn't in the current hit map,
/// or if its own selected row is currently mid-rename (`editing.is_some()`).
/// The latter guard matters for the cursor-navigation key handler this
/// feeds (`crate::host::apply_tree_nav_key`): while
/// [`crate::semantic::focus_scoped_key_event`] is routing an ordinary
/// text-editing key (`Home`/`End`/arrows included) to
/// `crate::tree_row_rename_key_event` for the row being renamed, the same
/// keypress must not also move the stored navigation cursor — without
/// this guard, `Home`/`End` mid-rename would jump the cursor to the first/
/// last row underneath the edit in progress. Doesn't require a scrollbar
/// to have painted — used by the tree-cursor key handler, which needs the
/// visual row order and viewport capacity regardless of whether the tree
/// currently has anything to scroll (e.g. the very first navigation key on
/// a tree short enough to fit without one).
pub fn tree_rows_and_viewport(
    hit_map: &[PaintedHit],
    id: WidgetId,
) -> Option<(Vec<TreePath>, usize)> {
    hit_map.iter().find_map(|hit| match hit {
        PaintedHit::Tree {
            id: hit_id,
            row_paths,
            viewport_rows,
            editing,
            ..
        } if *hit_id == id && editing.is_none() => Some((row_paths.clone(), *viewport_rows)),
        _ => None,
    })
}

/// The `(total_items, viewport_rows)` of the `List` hit painted under id
/// `id` this frame, or `None` if that list isn't in the current hit map —
/// the `List` counterpart of [`tree_rows_and_viewport`], read by
/// `crate::host::apply_list_nav_key` to move and scroll the uncontrolled
/// cursor.
pub fn list_rows_and_viewport(hit_map: &[PaintedHit], id: WidgetId) -> Option<(usize, usize)> {
    hit_map.iter().find_map(|hit| match hit {
        PaintedHit::List {
            id: hit_id,
            total_items,
            viewport_rows,
            ..
        } if *hit_id == id => Some((*total_items, *viewport_rows)),
        _ => None,
    })
}

/// The `(total_rows, viewport_rows)` of the `DataTable` hit painted under
/// id `id` this frame, or `None` if that table isn't in the current hit
/// map — the `DataTable` counterpart of [`tree_rows_and_viewport`], read
/// by `crate::host::apply_data_table_nav_key` to move and scroll the
/// uncontrolled cursor.
pub fn data_table_rows_and_viewport(
    hit_map: &[PaintedHit],
    id: WidgetId,
) -> Option<(usize, usize)> {
    hit_map.iter().find_map(|hit| match hit {
        PaintedHit::DataTable {
            id: hit_id,
            total_rows,
            viewport_rows,
            ..
        } if *hit_id == id => Some((*total_rows, *viewport_rows)),
        _ => None,
    })
}

/// Hit-tests `point` against only the `chrome_hits` lists
/// [`PaintedHit::Extra`] entries carry (`Toolbar`'s buttons, `StatusBar`'s
/// segments, `ActivityBar`'s items) — the subset [`InteractionEngine`][crate::engine::InteractionEngine]
/// needs on every `MouseMoved` for hover bookkeeping, which
/// [`hit_test_painted`] never runs for: its own [`event_position`] doesn't
/// resolve a position for that event kind (enriching `MouseMoved`'s own
/// `widget` field isn't this module's job). Checked in reverse paint
/// order, matching [`hit_test_painted`]'s own precedence. Returns
/// `(bar_id, action_id)`.
pub fn chrome_hover_hit(hit_map: &[PaintedHit], point: Point) -> Option<(WidgetId, WidgetId)> {
    for hit in hit_map.iter().rev() {
        if let PaintedHit::Extra {
            id, chrome_hits, ..
        } = hit
        {
            if let Some((_, action)) = chrome_hits.iter().find(|(r, _)| r.contains(point)) {
                return Some((*id, *action));
            }
        }
    }
    None
}

/// The screen position to hit-test against [`PaintedHit`]s, for the one
/// `event` variants whose own `widget` field this module enriches. `None`
/// for every other variant (e.g. `MouseMoved`, `MouseEntered`, key events),
/// which the binding's own event handler forwards to user code exactly as
/// received.
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

#[cfg(test)]
mod tests {
    use super::*;
    use quadraui::{
        Decoration, SelectionMode, StyledSpan, StyledText, TextDisplay, TextDisplayLine, TreeRow,
        TreeRowMeasure, TreeStyle, TreeView,
    };

    /// A single-row `Tree` hit, painted at a real `TreeView::layout` call
    /// (not a hand-built `TreeViewLayout` literal — that type is
    /// `#[non_exhaustive]`, so only the primitive's own constructor can
    /// build one). `editing` is threaded straight onto the returned
    /// [`PaintedHit::Tree`], the same shape `render::draw_bands`/
    /// `render::paint_leaf` produce from a row's own `TreeRow::edit`.
    fn single_row_tree_hit(editing: Option<(TreePath, TreeRowEditState)>) -> PaintedHit {
        let row = TreeRow {
            path: vec![0],
            indent: 0,
            icon: None,
            text: StyledText::plain("row 0"),
            badge: None,
            is_expanded: None,
            decoration: Decoration::default(),
            edit: editing.as_ref().map(|(_, edit)| edit.clone()),
        };
        let tree = TreeView {
            id: WidgetId::new("hit-test:tree"),
            rows: vec![row],
            selection_mode: SelectionMode::Single,
            selected_path: None,
            scroll_offset: 0,
            style: TreeStyle::default(),
            has_focus: true,
        };
        let layout = tree.layout(80.0, 24.0, |_| TreeRowMeasure::new(1.0));
        let area = Rect::new(0.0, 0.0, 80.0, 24.0);
        PaintedHit::Tree {
            id: tree.id,
            area,
            clip: area,
            layout,
            row_paths: tree.rows.iter().map(|row| row.path.clone()).collect(),
            scrollbar: None,
            scroll_offset: 0,
            viewport_rows: 24,
            editing,
        }
    }

    /// With no row mid-rename, [`tree_rows_and_viewport`] returns the
    /// tree's own row order and viewport capacity, same as before
    /// `editing` existed on [`PaintedHit::Tree`] at all.
    #[test]
    fn tree_rows_and_viewport_returns_rows_when_no_row_is_editing() {
        let hit_map = vec![single_row_tree_hit(None)];
        let id = WidgetId::new("hit-test:tree");
        assert_eq!(
            tree_rows_and_viewport(&hit_map, id),
            Some((vec![vec![0]], 24))
        );
    }

    /// While the tree's own selected row is mid-rename
    /// (`PaintedHit::Tree::editing.is_some()`), [`tree_rows_and_viewport`]
    /// must return `None` — the guard `crate::host::apply_tree_nav_key`'s
    /// own caller relies on so an ordinary text-editing key
    /// (`Home`/`End` included) routed to
    /// `crate::tree_row_rename_key_event` never also moves the stored
    /// navigation cursor out from under the rename in progress.
    #[test]
    fn tree_rows_and_viewport_returns_none_while_a_row_is_mid_rename() {
        let editing = Some((
            vec![0],
            TreeRowEditState {
                text: "row 0".to_string(),
                cursor: 5,
                selection_anchor: None,
                placeholder: None,
            },
        ));
        let hit_map = vec![single_row_tree_hit(editing)];
        let id = WidgetId::new("hit-test:tree");
        assert_eq!(tree_rows_and_viewport(&hit_map, id), None);
    }

    /// A `TextDisplay` hit whose own `area` reaches past `viewport` — the
    /// shape a `ScrollSlots::Atomic` leaf sized to its whole `content_len`
    /// and shifted by `-offset` gets on a backend that keeps the
    /// unclipped rect (see [`crate::render::scroll_content_slots`]'s own
    /// `cell_grid` doc), independent of whatever `viewport` it's painted
    /// inside.
    fn oversized_text_display_hit(viewport: Rect) -> PaintedHit {
        let mut td = TextDisplay::new(WidgetId::new("log"));
        for i in 0..100 {
            td.append_line(TextDisplayLine {
                spans: vec![StyledSpan::plain(format!("line {i}"))],
                decoration: Decoration::default(),
                timestamp: None,
            });
        }
        td.auto_scroll = false;
        let area = Rect::new(viewport.x, viewport.y - 4.0, viewport.width, 100.0);
        let layout = td.layout(area.width, area.height, |_| {
            quadraui::TextDisplayLineMeasure::new(1.0)
        });
        PaintedHit::TextDisplay {
            id: td.id,
            area,
            clip: area,
            layout,
            body_offset: 0.0,
            total_lines: 100,
            viewport_lines: viewport.height as usize,
        }
    }

    /// [`clip_hits_to_viewport`]'s whole acceptance scenario: a button
    /// painted above a scroll, whose own descendant content's `area`
    /// leaked up past the scroll's `viewport` and over the button's own
    /// row. A click on the button's row still resolves to the button —
    /// not the leaked content — and a point well above either never
    /// resolves to anything.
    #[test]
    fn clip_hits_to_viewport_keeps_a_sibling_painted_before_the_scroll() {
        let viewport = Rect::new(0.0, 1.0, 20.0, 9.0);
        let button = PaintedHit::Button {
            id: WidgetId::new("above"),
            layout: ButtonLayout {
                bounds: Rect::new(0.0, 0.0, 20.0, 1.0),
            },
        };
        let scroll = PaintedHit::Scroll {
            id: WidgetId::new("scroll"),
            viewport,
            axis: ScrollAxis::Vertical,
            content_len: 0.0,
            offset: 0.0,
            scrollbar: None,
            children: Vec::new(),
        };
        let mut hits = vec![button, scroll];
        let hits_before = hits.len();
        hits.push(oversized_text_display_hit(viewport));
        clip_hits_to_viewport(&mut hits, hits_before, viewport);

        // The button's own row (y=0) sits outside the scroll's
        // `viewport` (which starts at y=1) and inside the leaked hit's
        // own pre-clip bounds, so an unclipped hit map would resolve this
        // click to the scroll's content instead of the button.
        assert_eq!(
            hit_test_painted(&hits, Point { x: 0.0, y: 0.0 }),
            Some((WidgetId::new("above"), HitDetail::ButtonBody))
        );

        // Still resolves to the `TextDisplay` for a point genuinely
        // inside the viewport -- narrowing a hit's bounds doesn't drop
        // it outright.
        let (id, detail) = hit_test_painted(&hits, Point { x: 0.0, y: 5.0 })
            .expect("a point inside the viewport still hits the text display");
        assert_eq!(id, WidgetId::new("log"));
        assert!(matches!(detail, HitDetail::TextDisplayLine { .. }));
    }

    /// A hit whose bounds don't overlap `viewport` at all is dropped
    /// outright, not just narrowed to an empty rect that would never
    /// match any point anyway — so a caller iterating `hits` directly
    /// (rather than through [`hit_test_painted`]) never sees a
    /// zero-sized leftover.
    #[test]
    fn clip_hits_to_viewport_drops_a_hit_with_no_overlap() {
        let viewport = Rect::new(0.0, 10.0, 20.0, 5.0);
        let mut hits = vec![PaintedHit::Button {
            id: WidgetId::new("far-above"),
            layout: ButtonLayout {
                bounds: Rect::new(0.0, 0.0, 20.0, 1.0),
            },
        }];
        clip_hits_to_viewport(&mut hits, 0, viewport);
        assert!(hits.is_empty());
    }

    /// A 100-item `List`, painted into `area = (0, -320, 200, 1600)` by a
    /// `cell_grid: false` (GTK/macOS/Win) backend's own `scroll_content_slots`
    /// `Atomic` branch — `char_width: 8.0`, `line_height: 16.0`, a 200x160
    /// viewport, scrolled by `offset: 320.0` (`20` whole rows). A click at
    /// the viewport's own top-left (absolute `y: 8.0`) must still resolve
    /// to item `20` — `local y = 8.0 - area.y = 328.0`, `328.0 / 16.0 =
    /// 20.5`, floor `20` — the same row actually under the pointer, which
    /// only holds when [`clip_hits_to_viewport`] narrows `clip`
    /// (`(0, 0, 200, 160)`, the intersection with `viewport`) rather than
    /// moving `area` itself: `area`'s own origin is what the translation
    /// above is computed against, so clamping it to `(0, 0, ...)` the way
    /// narrowing `area` in place would have shifted every resolved row by
    /// the clipped amount (here, down to item `0`).
    #[test]
    fn clip_hits_to_viewport_keeps_a_local_frame_hits_translation_origin_unmoved() {
        use quadraui::{ListItem, ListItemMeasure, ListView, StyledText};

        let list = ListView {
            id: WidgetId::new("pixel-list"),
            title: None,
            items: (0..100)
                .map(|i| ListItem {
                    text: StyledText::plain(format!("item {i}")),
                    icon: None,
                    detail: None,
                    decoration: Decoration::default(),
                })
                .collect(),
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
        };
        let area = Rect::new(0.0, -320.0, 200.0, 1600.0);
        let layout = list.layout(area.width, area.height, 0.0, |_| ListItemMeasure::new(16.0));
        let mut hits = vec![PaintedHit::List {
            id: list.id,
            area,
            clip: area,
            layout,
            selected_idx: 0,
            total_items: 100,
            scroll_offset: 0,
            viewport_rows: 10,
        }];
        let viewport = Rect::new(0.0, 0.0, 200.0, 160.0);
        clip_hits_to_viewport(&mut hits, 0, viewport);

        let PaintedHit::List {
            area: kept_area,
            clip,
            ..
        } = &hits[0]
        else {
            panic!("still a List hit");
        };
        assert_eq!(*kept_area, area, "area keeps its own painted origin");
        assert_eq!(*clip, viewport, "clip narrows to the viewport intersection");

        let (id, detail) = hit_test_painted(&hits, Point { x: 0.0, y: 8.0 })
            .expect("inside the viewport, still resolves to the list");
        assert_eq!(id, list.id);
        assert_eq!(detail, HitDetail::ListItem { index: 20 });

        // A point outside the viewport (but still inside the pre-clip
        // `area`) doesn't resolve to anything.
        assert_eq!(hit_test_painted(&hits, Point { x: 0.0, y: 200.0 }), None);
    }

    /// A `Tree`'s own `scrollbar.track` is an absolute rect (see
    /// [`quadraui::Scrollbar`]'s own doc), so [`clip_hits_to_viewport`]
    /// narrows it directly — a track that reaches past `viewport` (as it
    /// would painted at a negative `area.y`) doesn't claim a click below
    /// the viewport's own bottom edge, even though the surrounding `Tree`
    /// entry is kept (its own `clip` still overlaps `viewport`).
    #[test]
    fn clip_hits_to_viewport_narrows_a_trees_scrollbar_track() {
        let viewport = Rect::new(0.0, 0.0, 20.0, 10.0);
        let area = Rect::new(0.0, 0.0, 80.0, 24.0);
        let track = Rect::new(19.0, 0.0, 1.0, 24.0);
        let scrollbar = Some(Scrollbar::vertical("track", track, 0.0, 100.0, 24.0, 1.0));
        let tree = TreeView {
            id: WidgetId::new("hit-test:tree"),
            rows: Vec::new(),
            selection_mode: SelectionMode::Single,
            selected_path: None,
            scroll_offset: 0,
            style: TreeStyle::default(),
            has_focus: true,
        };
        let mut hits = vec![PaintedHit::Tree {
            id: tree.id,
            area,
            clip: area,
            layout: tree.layout(area.width, area.height, |_| TreeRowMeasure::new(1.0)),
            row_paths: Vec::new(),
            scrollbar,
            scroll_offset: 0,
            viewport_rows: 24,
            editing: None,
        }];
        clip_hits_to_viewport(&mut hits, 0, viewport);

        let PaintedHit::Tree {
            scrollbar: Some(scrollbar),
            ..
        } = &hits[0]
        else {
            panic!("still a Tree hit with a scrollbar");
        };
        // The track's own pre-clip span (`y` in `[0, 24)`) reached past
        // `viewport`'s bottom edge (`y` in `[0, 10)`) -- narrowed, a point
        // past the viewport's own edge doesn't resolve to it.
        assert!(!scrollbar.track.contains(Point { x: 19.0, y: 20.0 }));
        assert!(scrollbar.track.contains(Point { x: 19.0, y: 5.0 }));
    }

    /// [`PaintedHit::Scroll`]'s own `scrollbar.track` is narrowed the same
    /// way [`PaintedHit::Tree`]'s is (see the test just above) — a nested
    /// `Scroll` painted inside an outer one, straddling the OUTER
    /// viewport's own edge, must not keep a clickable track reaching past
    /// it.
    #[test]
    fn clip_hits_to_viewport_narrows_a_scrolls_scrollbar_track() {
        let viewport = Rect::new(0.0, 0.0, 20.0, 10.0);
        let track = Rect::new(19.0, 0.0, 1.0, 24.0);
        let scrollbar = Some(Scrollbar::vertical("track", track, 0.0, 100.0, 24.0, 1.0));
        let mut hits = vec![PaintedHit::Scroll {
            id: WidgetId::new("hit-test:scroll"),
            viewport: Rect::new(0.0, 0.0, 20.0, 24.0),
            axis: ScrollAxis::Vertical,
            content_len: 100.0,
            offset: 0.0,
            scrollbar,
            children: Vec::new(),
        }];
        clip_hits_to_viewport(&mut hits, 0, viewport);

        let PaintedHit::Scroll {
            scrollbar: Some(scrollbar),
            ..
        } = &hits[0]
        else {
            panic!("still a Scroll hit with a scrollbar");
        };
        // Same assertion as the `Tree` test: the track's own pre-clip span
        // (`y` in `[0, 24)`) reached past `viewport`'s bottom edge (`y` in
        // `[0, 10)`) -- narrowed, a point past the viewport's own edge
        // doesn't resolve to it.
        assert!(!scrollbar.track.contains(Point { x: 19.0, y: 20.0 }));
        assert!(scrollbar.track.contains(Point { x: 19.0, y: 5.0 }));
    }

    /// [`PaintedHit::Extra::chrome_hits`] entries are absolute rects,
    /// checked by [`hit_test_painted`] ahead of the surrounding `Extra`'s
    /// own `area`/[`ExtraPaint::hit_test`] — [`clip_hits_to_viewport`]
    /// narrows each one independently (dropping it outright when it
    /// doesn't overlap `viewport` at all), the same per-entry treatment
    /// `hits` itself gets, so a chrome child painted partly or fully
    /// outside the viewport it's actually inside never keeps claiming
    /// clicks there.
    #[test]
    fn clip_hits_to_viewport_drops_a_chrome_hit_with_no_overlap() {
        let extra: Rc<dyn ExtraPaint> = Rc::new(quadraui::ProgressBar {
            id: WidgetId::new("bar"),
            label: String::new(),
            value: None,
            frame_idx: 0,
            cancellable: false,
            accent: None,
        });
        let viewport = Rect::new(0.0, 0.0, 20.0, 10.0);
        let mut hits = vec![PaintedHit::Extra {
            id: WidgetId::new("bar"),
            area: Rect::new(0.0, 0.0, 20.0, 20.0),
            extra,
            chrome_hits: vec![(Rect::new(0.0, 15.0, 5.0, 1.0), WidgetId::new("button"))],
        }];
        clip_hits_to_viewport(&mut hits, 0, viewport);

        let PaintedHit::Extra { chrome_hits, .. } = &hits[0] else {
            panic!("still an Extra hit");
        };
        assert!(chrome_hits.is_empty());
    }
}
