//! `TreeTableController` — a composed controller for a single
//! keyboard-navigable [`TreeTable`] driven from a hierarchical model
//! instead of an app-maintained flattened row list.
//!
//! An app building directly on [`TreeTable`] owns the flattened
//! `tree`/`table.rows` pair itself: expanding a node means re-walking its
//! own tree, re-flattening, and re-sorting siblings by hand.
//! `TreeTableController` does that work once: it owns
//! [`TreeTableNodeData`] nodes keyed by a stable [`NodeId`], the expanded
//! set, and the current sort column, and flattens them into a
//! [`TreeTable`] on demand via [`Self::flatten`] (also what [`Self::render`]
//! paints).
//!
//! Behaviour is not reimplemented here: [`Self::handle`] flattens the
//! model into a [`TreeTable`] descriptor and drives that widget's own
//! `Interactive` impl, then applies the [`crate::TreeTableEvent`] it
//! reports to the model. This controller's own job is the model and the
//! translation between [`NodeId`] (stable across a sort, which reorders a
//! flattened row's [`crate::TreePath`]) and the positional path or row
//! index the widget reports.
//!
//! # Lazy loading
//!
//! A node built via [`TreeTableNodeData::lazy_branch`] has `has_children:
//! true` but no loaded children yet. Expanding it (chevron click, Right,
//! or Space) emits [`TreeTableControllerEvent::ChildrenRequested`] exactly
//! once — further toggles of the same node before [`Self::set_children`]
//! answers it are no-ops on that front — and [`Self::flatten`] shows a
//! single dim `…` placeholder row (via [`crate::Decoration::Muted`] and
//! [`crate::TreeTableNode::placeholder`]) as that node's lone visible
//! child in the meantime.
//!
//! # Sibling-scoped sort
//!
//! [`Self::header_clicked`] (an app's own header-click
//! resolution) sorts every loaded level's children by that column's cell
//! text — recursively, so a grandchild's siblings sort the same as a
//! root's — rather than flattening the whole tree into one column-sorted
//! list, which would scramble the hierarchy. It emits
//! [`TreeTableControllerEvent::SortChanged`] so a host that wants a
//! different comparator (e.g. numeric for a Size column) can follow up
//! with its own [`Self::set_nodes`]/[`Self::set_children`] call, overriding
//! the default lexicographic-on-cell-text order this method already
//! applied.

use std::cmp::Ordering;
use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::a11y::{A11yChild, A11yInfo, A11yPrimitive};
use crate::behaviour::nav::{Edge, HorizontalDir, HorizontalNav, NavOutcome, NavRow, TreeNavCore};
use crate::compose::widget_host::{self, ChildHost};
use crate::interactive::{EventCx, FocusSpec, Interactive, PointerKind, RoutedEvent, SubTarget};
use crate::primitive::{LayoutCtx, PaintEnv, Primitive};
use crate::primitives::data_table::{Column, DataRow, DataTable, SortDirection};
use crate::primitives::tree_table::{
    TreeTable, TreeTableEvent, TreeTableModelEvent, TreeTableNode, TreeTablePrimitiveLayout,
    TreeTableState,
};
use crate::types::{Decoration, Icon, StyledText, TreePath};
use crate::{Backend, DrawsPrimitives, Handled, Rect, UiEvent, WidgetId};

/// A node's stable identity in [`TreeTableController`]'s hierarchical
/// model — an app-chosen key (file path, database id, …) that never
/// changes for a given node across an expand, collapse or sort. Unlike
/// [`crate::TreePath`] (a node's *position*, reassigned on every
/// [`TreeTableController::flatten`] call), this is what selection and the
/// expanded/requested sets are keyed by.
pub type NodeId = String;

/// One node in [`TreeTableController`]'s hierarchical model: an app-level
/// row, not yet flattened into a [`TreeTableNode`]/[`DataRow`] pair.
///
/// `children: None` means "not loaded yet" — distinct from `Some(vec![])`
/// ("loaded, and has none"). [`Self::has_children`] is the branch/leaf
/// flag [`crate::TreeTableNode::is_expanded`] needs even while `children`
/// is still `None`, so a lazy branch's chevron paints before its children
/// ever arrive.
///
/// On the wire a node is `{"id", "cells", "icon", "has_children",
/// "children"}`: a leaf by default, a lazy branch with `has_children` and
/// no `children`, and a loaded branch with both. `children` on a node
/// without `has_children` is never shown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(rename = "TreeTableControllerNode"))]
pub struct TreeTableNodeData {
    id: NodeId,
    #[serde(default)]
    cells: Vec<StyledText>,
    #[serde(default)]
    icon: Option<Icon>,
    #[serde(default)]
    has_children: bool,
    #[serde(default)]
    children: Option<Vec<TreeTableNodeData>>,
}

impl TreeTableNodeData {
    /// A leaf node: no chevron, never expandable.
    #[must_use]
    pub fn leaf(id: impl Into<NodeId>, cells: Vec<StyledText>) -> Self {
        Self {
            id: id.into(),
            cells,
            icon: None,
            has_children: false,
            children: None,
        }
    }

    /// A branch node whose children are already known (eagerly loaded —
    /// expanding it never emits [`TreeTableControllerEvent::ChildrenRequested`]).
    #[must_use]
    pub fn branch(id: impl Into<NodeId>, cells: Vec<StyledText>, children: Vec<Self>) -> Self {
        Self {
            id: id.into(),
            cells,
            icon: None,
            has_children: true,
            children: Some(children),
        }
    }

    /// A branch node whose children are not loaded yet — expanding it
    /// emits [`TreeTableControllerEvent::ChildrenRequested`] once, and
    /// [`TreeTableController::flatten`] shows a loading placeholder row
    /// until [`TreeTableController::set_children`] answers it.
    #[must_use]
    pub fn lazy_branch(id: impl Into<NodeId>, cells: Vec<StyledText>) -> Self {
        Self {
            id: id.into(),
            cells,
            icon: None,
            has_children: true,
            children: None,
        }
    }

    #[must_use]
    pub fn with_icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    pub fn id(&self) -> &NodeId {
        &self.id
    }

    pub fn has_children(&self) -> bool {
        self.has_children
    }

    /// `None` when this is a [`Self::lazy_branch`] whose children haven't
    /// arrived yet (or a leaf, for which the question doesn't apply).
    pub fn children(&self) -> Option<&[Self]> {
        self.children.as_deref()
    }

    fn find_mut<'a>(nodes: &'a mut [Self], id: &NodeId) -> Option<&'a mut Self> {
        for node in nodes.iter_mut() {
            if &node.id == id {
                return Some(node);
            }
            if let Some(children) = node.children.as_mut() {
                if let Some(found) = Self::find_mut(children, id) {
                    return Some(found);
                }
            }
        }
        None
    }

    fn find<'a>(nodes: &'a [Self], id: &NodeId) -> Option<&'a Self> {
        for node in nodes {
            if &node.id == id {
                return Some(node);
            }
            if let Some(children) = &node.children {
                if let Some(found) = Self::find(children, id) {
                    return Some(found);
                }
            }
        }
        None
    }
}

/// What happened after [`TreeTableController::handle`] processed an event,
/// or after calling one of its navigation/mutation methods directly.
#[derive(Debug, Clone, PartialEq)]
pub enum TreeTableControllerEvent {
    /// Row selected — click or keyboard navigation.
    RowSelected { id: NodeId },
    /// Enter pressed, or a row/chevron double-clicked.
    RowActivated { id: NodeId },
    /// A branch node's expand state flipped. The children were already
    /// loaded (or this collapses), so no [`Self::ChildrenRequested`]
    /// follows.
    ToggleExpand { id: NodeId, expand: bool },
    /// A lazy branch ([`TreeTableNodeData::lazy_branch`]) was expanded for
    /// the first time with no loaded children — the host should fetch
    /// them and call [`TreeTableController::set_children`]. Fires exactly
    /// once per node per "not yet answered" stretch: a second expand
    /// attempt before `set_children` answers this one is a no-op, not a
    /// second `ChildrenRequested`.
    ChildrenRequested { id: NodeId },
    /// A header click (re-)sorted every loaded level's children by
    /// `col`'s cell text — see [`TreeTableController::header_clicked`]'s
    /// own doc for how to override the default comparator.
    SortChanged { col: usize, ascending: bool },
    /// Event consumed (e.g. a scroll) — caller should redraw.
    Consumed,
    /// Event not relevant to this controller.
    Ignored,
}

/// One flattened row, carrying both its [`NodeId`] and the positional
/// [`crate::TreePath`] [`TreeNavCore`] navigates by — the seam
/// [`TreeTableController`]'s id/path translation crosses on every
/// navigation call. Not `pub`: a caller never sees this, only
/// [`TreeTable`]/[`TreeTableNode`] (via [`TreeTableController::flatten`])
/// or a [`NodeId`] (via every `TreeTableControllerEvent`).
#[derive(Debug, Clone)]
struct FlatRow {
    id: NodeId,
    path: TreePath,
    indent: u16,
    is_expanded: Option<bool>,
    icon: Option<Icon>,
    cells: Vec<StyledText>,
    /// `true` for the synthetic "…" row standing in for a lazy branch's
    /// not-yet-loaded children — never a real node, so never selectable
    /// or activatable.
    loading: bool,
    /// A lazy branch whose children have not arrived.
    children_pending: bool,
}

impl NavRow for FlatRow {
    fn path(&self) -> &TreePath {
        &self.path
    }
    fn indent(&self) -> u16 {
        self.indent
    }
    fn is_expanded(&self) -> Option<bool> {
        self.is_expanded
    }
}

pub struct TreeTableController {
    id: WidgetId,
    roots: Vec<TreeTableNodeData>,
    expanded: HashSet<NodeId>,
    /// Nodes for which [`TreeTableControllerEvent::ChildrenRequested`] has
    /// already fired and not yet been answered by [`Self::set_children`] —
    /// the once-per-node guard [`Self::set_expanded`] checks.
    requested: HashSet<NodeId>,
    selected_id: Option<NodeId>,
    sort: Option<(usize, bool)>,
    folders_first: bool,
    /// The widget's own gesture state; its navigation core carries the
    /// scroll position the descriptor is flattened with.
    state: TreeTableState,
    has_focus: bool,
    show_scrollbar: bool,
    bordered: bool,
}

impl TreeTableController {
    pub fn new(id: impl Into<String>) -> Self {
        Self::with_id(WidgetId::new(id))
    }

    fn with_id(id: WidgetId) -> Self {
        Self {
            id,
            roots: Vec::new(),
            expanded: HashSet::new(),
            requested: HashSet::new(),
            selected_id: None,
            sort: None,
            folders_first: false,
            state: TreeTableState::default(),
            has_focus: true,
            show_scrollbar: true,
            bordered: false,
        }
    }

    // ── Per-frame / model data ────────────────────────────────────────

    /// Replace the top-level nodes. `expanded`/`selected_id`/`requested`
    /// are keyed by [`NodeId`] and untouched, so a host that re-sends the
    /// same ids (e.g. after a sort override, or a server refresh that
    /// keeps paths stable) doesn't lose expand/selection state — only ids
    /// absent from the new tree simply stop resolving to a row.
    pub fn set_nodes(&mut self, nodes: Vec<TreeTableNodeData>) {
        self.roots = nodes;
    }

    /// Answer a [`TreeTableControllerEvent::ChildrenRequested`]: attach
    /// `children` to the node at `id` (found anywhere in the tree, not
    /// just at the top level) and clear it from the pending-request set,
    /// so a later collapse/expand of the same node can request again if
    /// the host ever clears its children back to "not loaded" itself.
    /// A no-op (never panics) if `id` isn't found.
    pub fn set_children(&mut self, id: &NodeId, children: Vec<TreeTableNodeData>) {
        if let Some(node) = TreeTableNodeData::find_mut(&mut self.roots, id) {
            node.children = Some(children);
        }
        self.requested.remove(id);
    }

    /// Folders-before-files tie-break for [`Self::header_clicked`]'s
    /// default comparator: when `true`, every branch node sorts before
    /// every leaf at the same level, regardless of column text. Default
    /// `false` (pure lexicographic-on-cell-text, matching a plain
    /// [`DataTable`]'s own header-click convention).
    pub fn set_folders_first(&mut self, folders_first: bool) {
        self.folders_first = folders_first;
    }

    pub fn set_has_focus(&mut self, has_focus: bool) {
        self.has_focus = has_focus;
    }

    pub fn has_focus(&self) -> bool {
        self.has_focus
    }

    pub fn set_show_scrollbar(&mut self, show: bool) {
        self.show_scrollbar = show;
    }

    pub fn set_bordered(&mut self, bordered: bool) {
        self.bordered = bordered;
    }

    // ── State accessors ───────────────────────────────────────────────

    pub fn selected_id(&self) -> Option<&NodeId> {
        self.selected_id.as_ref()
    }

    pub fn set_selected_id(&mut self, id: Option<NodeId>) {
        self.selected_id = id;
    }

    pub fn is_expanded(&self, id: &NodeId) -> bool {
        self.expanded.contains(id)
    }

    pub fn scroll_offset(&self) -> usize {
        self.state.nav.scroll_offset()
    }

    pub fn set_scroll_offset(&mut self, offset: usize) {
        self.state.nav.set_scroll_offset(offset);
    }

    pub fn node(&self, id: &NodeId) -> Option<&TreeTableNodeData> {
        TreeTableNodeData::find(&self.roots, id)
    }

    // ── Flatten + render ──────────────────────────────────────────────

    /// Flatten the current model (honoring the expanded set and the
    /// not-yet-loaded placeholder) into a [`TreeTable`] — what
    /// [`Self::render`] paints, and what a caller doing its own
    /// hit-testing (rather than going through [`Self::handle`]) should
    /// resolve clicks against via the table's own laid-out layout.
    ///
    /// `columns` is taken by value each call rather than stored (a host
    /// typically recomputes it from the current viewport width, e.g. to
    /// drop trailing columns on a narrow rect — see
    /// [`crate::primitives::tree_table::TreeTable::layout`]'s own
    /// `min_tree_col_width` rule), so this never goes stale the way a
    /// `set_columns` cached on the controller would across a resize that
    /// doesn't otherwise touch the model.
    pub fn flatten(&self, columns: Vec<Column>) -> TreeTable {
        self.flatten_with(&self.roots, self.selected_id.as_ref(), columns)
    }

    /// [`Self::flatten`] for `roots` and `selected` in place of the stored
    /// model and selection, keeping this controller's expanded set, sort
    /// indicator, scroll position and flags.
    fn flatten_with(
        &self,
        roots: &[TreeTableNodeData],
        selected: Option<&NodeId>,
        columns: Vec<Column>,
    ) -> TreeTable {
        let rows = self.flatten_rows_of(roots);
        let selected_idx = selected.and_then(|id| rows.iter().position(|r| &r.id == id));
        let mut table_rows = Vec::with_capacity(rows.len());
        let mut tree = Vec::with_capacity(rows.len());
        for row in &rows {
            table_rows.push(DataRow {
                cells: row.cells.clone(),
                decoration: if row.loading {
                    Decoration::Muted
                } else {
                    Decoration::Normal
                },
            });
            let mut node = TreeTableNode::new(row.path.clone(), row.indent);
            if let Some(expanded) = row.is_expanded {
                node = node.with_expanded(expanded);
            }
            if let Some(icon) = &row.icon {
                node = node.with_icon(icon.clone());
            }
            if row.children_pending {
                node = node.with_children_pending();
            }
            if row.loading {
                node = node.as_placeholder();
            }
            tree.push(node);
        }
        TreeTable {
            vim_keys: true,
            table: DataTable {
                selection_mode: Default::default(),
                selected_indices: Vec::new(),
                id: self.id,
                columns,
                rows: table_rows,
                selected_idx,
                scroll_offset: self.state.nav.scroll_offset(),
                sort: self.sort.map(|(col, ascending)| {
                    (
                        col,
                        if ascending {
                            SortDirection::Ascending
                        } else {
                            SortDirection::Descending
                        },
                    )
                }),
                has_focus: self.has_focus,
                show_scrollbar: self.show_scrollbar,
                min_total_width: None,
                h_scroll: 0.0,
                column_overrides: Vec::new(),
                footer: None,
                bordered: self.bordered,
                striped: false,
                glyphs: None,
            },
            tree,
        }
    }

    pub fn render(&self, backend: &mut dyn Backend, rect: Rect, columns: Vec<Column>) {
        let table = self.flatten(columns);
        backend.draw_dyn(rect, &table, &());
    }

    // ── Handle ────────────────────────────────────────────────────────

    /// Drive the widget with a backend-neutral [`UiEvent`] against the
    /// layout [`Self::render`] paints at `rect`, and apply what it reports
    /// to the model.
    pub fn handle(
        &mut self,
        event: &UiEvent,
        backend: &mut dyn Backend,
        rect: Rect,
        columns: Vec<Column>,
    ) -> TreeTableControllerEvent {
        if let UiEvent::MouseDown { position, .. } | UiEvent::DoubleClick { position, .. } = event {
            if !rect.contains(*position) {
                return TreeTableControllerEvent::Ignored;
            }
        }
        let table = self.flatten(columns);
        let layout = backend.layout_primitive(rect, &table);
        let routed = match event {
            // The widget scrolls by whole rows; the backend converts its
            // native wheel delta.
            UiEvent::Scroll {
                delta, position, ..
            } => RoutedEvent::Wheel {
                dx: 0.0,
                dy: backend.wheel_delta_rows(delta.y, layout.layout.table.row_height),
                local: crate::Point::new(position.x - rect.x, position.y - rect.y),
                mods: crate::Modifiers::default(),
            },
            _ => {
                let subs = Interactive::sub_regions(&table, &layout);
                match widget_host::routed_event(event, rect, &subs) {
                    Some(routed) => routed,
                    None => return TreeTableControllerEvent::Ignored,
                }
            }
        };
        let units = crate::backend::primitive_layout_ctx(backend).units();
        let (outcome, _) = widget_host::with_cx(table.table.id, units, |cx| {
            self.handle_routed(&table, &routed, &layout, cx)
        });
        outcome
    }

    /// Drive `table` — this model flattened, laid out as `layout` — with
    /// one routed event against `cx`, and apply what it reports to the
    /// model. Focus, pointer-capture and cursor requests go to `cx`; the
    /// table's own events are translated into the one event this
    /// controller reports.
    fn handle_routed(
        &mut self,
        table: &TreeTable,
        routed: &RoutedEvent,
        layout: &TreeTablePrimitiveLayout,
        cx: &mut EventCx,
    ) -> TreeTableControllerEvent {
        // The controller claims the tree's navigation keys only; the
        // wrapped table's type-to-jump and select-all keys stay the app's
        // shortcuts.
        if let RoutedEvent::Key(key) = routed {
            if !TreeNavCore::claims_key(key, table.vim_keys) {
                return TreeTableControllerEvent::Ignored;
            }
        }
        let mut host = ChildHost::same(cx).collecting();
        let handled = {
            let mut table_cx = EventCx::new(table.table.id, &mut host);
            Interactive::handle(table, &mut self.state, routed, layout, &mut table_cx)
        };
        let emitted = host.take_collected();
        let pressed = matches!(
            routed,
            RoutedEvent::Pointer {
                kind: PointerKind::Down(_),
                ..
            }
        );
        let rows = self.flatten_rows();
        let mut outcome = match handled {
            Handled::Event(ev) => self.apply(ev, &rows, pressed),
            Handled::Consumed => TreeTableControllerEvent::Consumed,
            Handled::Ignored => TreeTableControllerEvent::Ignored,
        };
        for emitted in emitted {
            if let UiEvent::TreeTable(_, ev) = emitted {
                let applied = self.apply(ev, &rows, pressed);
                if matches!(
                    outcome,
                    TreeTableControllerEvent::Consumed | TreeTableControllerEvent::Ignored
                ) {
                    outcome = applied;
                }
            }
        }
        outcome
    }

    /// Apply one event the widget reported to the model, as the event
    /// this controller reports for it. `pressed` marks an event a pointer
    /// press produced: a chevron press also selects its row.
    fn apply(
        &mut self,
        event: TreeTableEvent,
        rows: &[FlatRow],
        pressed: bool,
    ) -> TreeTableControllerEvent {
        let id_at = |idx: usize| rows.get(idx).filter(|r| !r.loading).map(|r| r.id.clone());
        match event {
            TreeTableEvent::RowSelected { idx, .. } => match id_at(idx) {
                Some(id) => {
                    self.selected_id = Some(id.clone());
                    TreeTableControllerEvent::RowSelected { id }
                }
                None => TreeTableControllerEvent::Consumed,
            },
            TreeTableEvent::RowActivated { idx, .. }
            | TreeTableEvent::RowDoubleClicked { idx, .. } => match id_at(idx) {
                Some(id) => {
                    self.selected_id = Some(id.clone());
                    TreeTableControllerEvent::RowActivated { id }
                }
                None => TreeTableControllerEvent::Consumed,
            },
            TreeTableEvent::ToggleExpanded { path, expand } => {
                let Some(id) = Self::id_for_path(rows, &path) else {
                    return TreeTableControllerEvent::Ignored;
                };
                if pressed {
                    self.selected_id = Some(id.clone());
                }
                self.set_expanded(id, expand)
            }
            TreeTableEvent::SortChanged { col, ascending } => {
                self.sort_by(col, ascending);
                TreeTableControllerEvent::SortChanged { col, ascending }
            }
            TreeTableEvent::ScrollChanged { scroll_offset } => {
                if self.state.nav.scroll_offset() != scroll_offset {
                    self.state.nav.set_scroll_offset(scroll_offset);
                }
                TreeTableControllerEvent::Consumed
            }
            // `set_expanded` already answers a lazy expand with its own
            // once-per-node request.
            TreeTableEvent::ChildrenRequested { .. }
            | TreeTableEvent::SelectionSet { .. }
            | TreeTableEvent::ColumnResized { .. }
            | TreeTableEvent::Scroll { .. } => TreeTableControllerEvent::Consumed,
            TreeTableEvent::ContextMenuRequested { .. } | TreeTableEvent::RowClicked { .. } => {
                TreeTableControllerEvent::Ignored
            }
        }
    }

    // ── Navigation primitives ─────────────────────────────────────────

    pub fn move_selection_by(
        &mut self,
        delta: isize,
        viewport_rows: usize,
    ) -> TreeTableControllerEvent {
        let rows = self.navigable_rows();
        self.sync_nav_selection(&rows);
        match self
            .state
            .nav
            .move_selection_by(&rows, delta, viewport_rows)
        {
            NavOutcome::Empty => TreeTableControllerEvent::Ignored,
            NavOutcome::Unchanged => TreeTableControllerEvent::Consumed,
            NavOutcome::Selected(path) => self.select_path(&rows, &path),
        }
    }

    pub fn move_selection_to_edge(
        &mut self,
        edge: Edge,
        viewport_rows: usize,
    ) -> TreeTableControllerEvent {
        let rows = self.navigable_rows();
        self.sync_nav_selection(&rows);
        match self
            .state
            .nav
            .move_selection_to_edge(&rows, edge, viewport_rows)
        {
            NavOutcome::Empty => TreeTableControllerEvent::Ignored,
            NavOutcome::Unchanged => TreeTableControllerEvent::Consumed,
            NavOutcome::Selected(path) => self.select_path(&rows, &path),
        }
    }

    pub fn activate_selection(&self) -> TreeTableControllerEvent {
        match &self.selected_id {
            Some(id) => TreeTableControllerEvent::RowActivated { id: id.clone() },
            None => TreeTableControllerEvent::Ignored,
        }
    }

    /// Toggle expand/collapse on the currently selected row — the Space
    /// handler. [`TreeTableControllerEvent::Ignored`] when nothing is
    /// selected or the selection is a leaf.
    pub fn toggle_expand_selected(&mut self) -> TreeTableControllerEvent {
        let rows = self.navigable_rows();
        self.sync_nav_selection(&rows);
        let Some(path) = self.state.nav.toggle_expand(&rows) else {
            return TreeTableControllerEvent::Ignored;
        };
        let Some(id) = Self::id_for_path(&rows, &path) else {
            return TreeTableControllerEvent::Ignored;
        };
        let expand = !self.expanded.contains(&id);
        self.set_expanded(id, expand)
    }

    /// Left/Right: collapse-or-select-parent / expand-or-select-first-child,
    /// via [`crate::behaviour::nav::TreeNavCore::horizontal_nav`] —
    /// the same rule [`crate::TreeTableLayout::chevron_toggle`]'s
    /// Left/Right counterpart documents, applied against this
    /// controller's own id-keyed model instead of re-derived.
    pub fn handle_horizontal(&mut self, dir: HorizontalDir) -> TreeTableControllerEvent {
        let rows = self.navigable_rows();
        self.sync_nav_selection(&rows);
        match self.state.nav.horizontal_nav(&rows, dir) {
            HorizontalNav::Collapse(path) => match Self::id_for_path(&rows, &path) {
                Some(id) => self.set_expanded(id, false),
                None => TreeTableControllerEvent::Ignored,
            },
            HorizontalNav::Expand(path) => match Self::id_for_path(&rows, &path) {
                Some(id) => self.set_expanded(id, true),
                None => TreeTableControllerEvent::Ignored,
            },
            HorizontalNav::SelectParent(path) | HorizontalNav::SelectFirstChild(path) => {
                self.select_path(&rows, &path)
            }
            HorizontalNav::None => TreeTableControllerEvent::Ignored,
        }
    }

    pub fn scroll_to_visible(&mut self, row: usize, viewport_rows: usize) {
        self.state.nav.scroll_to_visible(row, viewport_rows);
    }

    pub fn page_scroll(&mut self, delta: isize, viewport_rows: usize) {
        let total = self.flatten_rows().len();
        self.state.nav.page_scroll(delta, total, viewport_rows);
    }

    // ── Sort ──────────────────────────────────────────────────────────

    /// An app's own header-click resolution: toggles
    /// ascending/descending on a repeat click of the same column (else
    /// starts ascending on a new one), then sorts every loaded level's
    /// children by that column's cell text — case-sensitive
    /// lexicographic, [`Self::set_folders_first`]-aware — recursively, so
    /// the hierarchy survives. A host wanting a different comparator
    /// (numeric, locale-aware, …) reacts to the returned
    /// [`TreeTableControllerEvent::SortChanged`] with its own
    /// [`Self::set_nodes`]/[`Self::set_children`] call, re-ordering
    /// children however it likes — this method's own reorder is simply
    /// overwritten by that follow-up.
    pub fn header_clicked(&mut self, col: usize) -> TreeTableControllerEvent {
        let ascending = match self.sort {
            Some((c, ascending)) if c == col => !ascending,
            _ => true,
        };
        self.sort_by(col, ascending);
        TreeTableControllerEvent::SortChanged { col, ascending }
    }

    /// Sort every loaded level's children by `col`'s cell text.
    fn sort_by(&mut self, col: usize, ascending: bool) {
        self.sort = Some((col, ascending));
        Self::sort_level(&mut self.roots, col, ascending, self.folders_first);
    }

    fn sort_level(
        nodes: &mut [TreeTableNodeData],
        col: usize,
        ascending: bool,
        folders_first: bool,
    ) {
        nodes.sort_by(|a, b| {
            if folders_first {
                match (a.has_children, b.has_children) {
                    (true, false) => return Ordering::Less,
                    (false, true) => return Ordering::Greater,
                    _ => {}
                }
            }
            let at = cell_text(a.cells.get(col));
            let bt = cell_text(b.cells.get(col));
            if ascending {
                at.cmp(&bt)
            } else {
                bt.cmp(&at)
            }
        });
        for node in nodes.iter_mut() {
            if let Some(children) = node.children.as_mut() {
                Self::sort_level(children, col, ascending, folders_first);
            }
        }
    }

    // ── Lazy loading / expand-collapse ────────────────────────────────

    /// Expand or collapse `id` directly — what [`Self::handle`]'s own
    /// chevron-click/Left/Right/Space dispatch calls internally, exposed
    /// for a host that wants to drive expand state programmatically (e.g.
    /// restoring a persisted expanded set, or "expand all"). Returns
    /// [`TreeTableControllerEvent::ChildrenRequested`] exactly once per
    /// not-yet-loaded lazy branch — see the module doc's *Lazy loading*
    /// section.
    pub fn set_expanded(&mut self, id: NodeId, expand: bool) -> TreeTableControllerEvent {
        if expand {
            self.expanded.insert(id.clone());
            let needs_children = self
                .node(&id)
                .map(|n| n.has_children && n.children.is_none())
                .unwrap_or(false);
            if needs_children {
                if self.requested.insert(id.clone()) {
                    return TreeTableControllerEvent::ChildrenRequested { id };
                }
                // Already requested and still unanswered — a repeat
                // expand attempt is a no-op, not a second request.
                return TreeTableControllerEvent::Consumed;
            }
        } else {
            self.expanded.remove(&id);
        }
        TreeTableControllerEvent::ToggleExpand { id, expand }
    }

    // ── Internal: id/path translation, flatten, hit dispatch ─────────

    /// Point [`TreeNavCore`]'s own selection at wherever `self.selected_id`
    /// currently lands in `rows` — `None` if it doesn't resolve there (the
    /// node was removed, or nothing is selected yet).
    fn sync_nav_selection(&mut self, rows: &[FlatRow]) {
        let path = self
            .selected_id
            .as_ref()
            .and_then(|id| Self::path_for_id(rows, id));
        self.state.nav.set_selected_path(path);
    }

    fn select_path(&mut self, rows: &[FlatRow], path: &TreePath) -> TreeTableControllerEvent {
        match Self::id_for_path(rows, path) {
            Some(id) => {
                self.selected_id = Some(id.clone());
                TreeTableControllerEvent::RowSelected { id }
            }
            None => TreeTableControllerEvent::Ignored,
        }
    }

    fn path_for_id(rows: &[FlatRow], id: &NodeId) -> Option<TreePath> {
        rows.iter().find(|r| &r.id == id).map(|r| r.path.clone())
    }

    fn id_for_path(rows: &[FlatRow], path: &TreePath) -> Option<NodeId> {
        rows.iter().find(|r| &r.path == path).map(|r| r.id.clone())
    }

    fn flatten_rows(&self) -> Vec<FlatRow> {
        self.flatten_rows_of(&self.roots)
    }

    fn flatten_rows_of(&self, roots: &[TreeTableNodeData]) -> Vec<FlatRow> {
        let mut out = Vec::new();
        Self::flatten_into(roots, 0, &[], &self.expanded, &mut out);
        out
    }

    fn flatten_into(
        nodes: &[TreeTableNodeData],
        indent: u16,
        prefix: &[u16],
        expanded: &HashSet<NodeId>,
        out: &mut Vec<FlatRow>,
    ) {
        for (i, node) in nodes.iter().enumerate() {
            let mut path = prefix.to_vec();
            path.push(i as u16);
            let is_expanded = node.has_children.then(|| expanded.contains(&node.id));
            out.push(FlatRow {
                id: node.id.clone(),
                path: path.clone(),
                indent,
                is_expanded,
                icon: node.icon.clone(),
                cells: node.cells.clone(),
                loading: false,
                children_pending: node.has_children && node.children.is_none(),
            });
            if node.has_children && expanded.contains(&node.id) {
                match &node.children {
                    Some(children) => {
                        Self::flatten_into(children, indent + 1, &path, expanded, out)
                    }
                    None => {
                        let mut loading_path = path.clone();
                        loading_path.push(0);
                        out.push(FlatRow {
                            id: format!("{}::__loading__", node.id),
                            path: loading_path,
                            indent: indent + 1,
                            is_expanded: None,
                            icon: None,
                            cells: vec![StyledText::plain("…")],
                            loading: true,
                            children_pending: false,
                        });
                    }
                }
            }
        }
    }

    /// [`Self::flatten_rows`] with the synthetic "…" lazy-load placeholder
    /// filtered out — what every keyboard navigation entry point
    /// ([`Self::move_selection_by`], [`Self::move_selection_to_edge`],
    /// [`Self::toggle_expand_selected`], [`Self::handle_horizontal`]) feeds
    /// [`crate::behaviour::nav::TreeNavCore`], so Down/Right/Home/End
    /// skip over a pending lazy branch's placeholder row, instead of
    /// selecting or activating a row with no real [`NodeId`] behind it.
    fn navigable_rows(&self) -> Vec<FlatRow> {
        self.flatten_rows()
            .into_iter()
            .filter(|r| !r.loading)
            .collect()
    }
}

// ── The hierarchical model as a contract widget ──────────────────────

/// `serde(default = ...)` for [`TreeTableModel`]'s flags whose default
/// is `true`.
fn default_true() -> bool {
    true
}

/// A hierarchical tree table as a contract widget — the wire kind
/// `"tree_table_controller"`.
///
/// The descriptor is content: the app sends the whole node tree every
/// frame, lazily loaded children included, in the order it wants when it
/// sorts rows itself (`sort_override`). The widget keeps what the user
/// does to that tree — which branches are expanded, the selection, the
/// scroll position and the header sort — in its [`TreeTableModelState`],
/// keyed by node id so it survives a re-sort or a refresh, and reports
/// each change as a [`TreeTableModelEvent`].
///
/// Behaviour is [`TreeTableController`]'s: the state holds one, synced
/// to the descriptor before every event, and layout, paint and routing
/// all go through the [`TreeTable`] it flattens to.
///
/// `selected_id` selects a node whenever the app changes it; between
/// changes the user's own selection stands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(rename = "TreeTableControllerDescriptor"))]
pub struct TreeTableModel {
    pub id: WidgetId,
    #[serde(default)]
    pub columns: Vec<Column>,
    #[serde(default)]
    pub nodes: Vec<TreeTableNodeData>,
    #[serde(default)]
    pub selected_id: Option<NodeId>,
    #[serde(default = "default_true")]
    pub has_focus: bool,
    #[serde(default = "default_true")]
    pub show_scrollbar: bool,
    #[serde(default)]
    pub bordered: bool,
    /// Sort branches before leaves at every level when a header click
    /// sorts.
    #[serde(default)]
    pub folders_first: bool,
    /// `nodes` already arrive in the app's own order for the current
    /// sort: a header click still toggles the sort indicator and reports
    /// [`TreeTableModelEvent::SortChanged`], but the widget does not
    /// re-order the rows itself.
    #[serde(default)]
    pub sort_override: bool,
}

/// [`Interactive::State`] for [`TreeTableModel`]: the controller holding
/// the user's expand, selection, scroll and sort state, created on the
/// first event, and the descriptor `selected_id` it last adopted.
#[derive(Default)]
pub struct TreeTableModelState {
    controller: Option<TreeTableController>,
    adopted_selection: Option<Option<NodeId>>,
}

impl TreeTableModelState {
    /// The controller, brought up to date with `model`: its nodes (sorted
    /// by the current header sort unless the app sorts them), its flags,
    /// a changed `selected_id`, and the pending child requests still
    /// unanswered.
    fn sync(&mut self, model: &TreeTableModel) -> &mut TreeTableController {
        let selection = model.selection(self).cloned();
        let adopted = Some(model.selected_id.clone());
        let ctl = self
            .controller
            .get_or_insert_with(|| TreeTableController::with_id(model.id));
        ctl.id = model.id;
        ctl.roots = model.ordered_nodes(ctl.sort);
        ctl.selected_id = selection;
        ctl.folders_first = model.folders_first;
        ctl.has_focus = model.has_focus;
        ctl.show_scrollbar = model.show_scrollbar;
        ctl.bordered = model.bordered;
        let roots = &ctl.roots;
        ctl.requested.retain(|id| {
            TreeTableNodeData::find(roots, id)
                .is_some_and(|n| n.has_children && n.children.is_none())
        });
        self.adopted_selection = adopted;
        ctl
    }
}

impl TreeTableModel {
    /// `nodes` in the order the widget shows them: sorted by `sort`'s
    /// column the way a header click sorts, unless the app sorts them
    /// itself or nothing is sorted yet.
    fn ordered_nodes(&self, sort: Option<(usize, bool)>) -> Vec<TreeTableNodeData> {
        let mut nodes = self.nodes.clone();
        if let (Some((col, ascending)), false) = (sort, self.sort_override) {
            TreeTableController::sort_level(&mut nodes, col, ascending, self.folders_first);
        }
        nodes
    }

    /// The node the widget shows selected: `selected_id` when the app
    /// changed it since the widget last adopted it, else the user's own
    /// selection.
    fn selection<'a>(&'a self, state: &'a TreeTableModelState) -> Option<&'a NodeId> {
        match (&state.controller, &state.adopted_selection) {
            (Some(ctl), Some(adopted)) if *adopted == self.selected_id => ctl.selected_id.as_ref(),
            _ => self.selected_id.as_ref(),
        }
    }

    /// The flat [`TreeTable`] this model shows for `state` (no state yet:
    /// every branch collapsed, unsorted, scrolled to the top).
    fn flatten(&self, state: Option<&TreeTableModelState>) -> TreeTable {
        let fresh;
        let ctl = match state.and_then(|s| s.controller.as_ref()) {
            Some(ctl) => ctl,
            None => {
                fresh = TreeTableController::with_id(self.id);
                &fresh
            }
        };
        let selected = match state {
            Some(state) => self.selection(state),
            None => self.selected_id.as_ref(),
        };
        let roots = self.ordered_nodes(ctl.sort);
        let mut table = ctl.flatten_with(&roots, selected, self.columns.clone());
        table.table.id = self.id;
        table.table.has_focus = self.has_focus;
        table.table.show_scrollbar = self.show_scrollbar;
        table.table.bordered = self.bordered;
        table
    }

    /// [`Self::flatten`] against this widget's stored state.
    fn flatten_stored(&self) -> TreeTable {
        crate::widget_state::with_widget_state_ref(Some(&self.id), |state| {
            self.flatten(state.and_then(|s| s.downcast_ref::<TreeTableModelState>()))
        })
    }
}

/// [`Primitive::Layout`] for [`TreeTableModel`]: the flat table the model
/// was laid out as, and that table's own layout.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeTableModelLayout {
    /// The model flattened against the widget's state at layout time.
    pub table: TreeTable,
    /// [`Self::table`]'s layout.
    pub inner: TreeTablePrimitiveLayout,
}

impl A11yPrimitive for TreeTableModel {
    fn a11y_info(&self) -> A11yInfo {
        A11yPrimitive::a11y_info(&self.flatten_stored())
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        A11yPrimitive::a11y_children(&self.flatten_stored())
    }
}

impl Primitive for TreeTableModel {
    type Layout = TreeTableModelLayout;
    type Event = TreeTableModelEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// Lays out the model as flattened against the widget's stored state,
    /// which decides the rows a layout has: the expanded branches, the
    /// sort and the scroll position.
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> Self::Layout {
        let table = self.flatten_stored();
        let inner = Primitive::layout(&table, ctx, rect);
        TreeTableModelLayout { table, inner }
    }

    fn paint(
        &self,
        layout: &Self::Layout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &PaintEnv,
    ) {
        Primitive::paint(&layout.table, &layout.inner, surface, env);
    }

    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &Self::Layout,
        cells: &mut dyn crate::CellSurface,
        env: &PaintEnv,
    ) {
        Primitive::paint_cells(&layout.table, &layout.inner, cells, env);
    }
}

impl Interactive for TreeTableModel {
    type State = TreeTableModelState;
    type Action = TreeTableModelEvent;

    fn focus(&self) -> FocusSpec {
        FocusSpec::Stop
    }

    fn sub_regions(&self, layout: &Self::Layout) -> Vec<(Rect, SubTarget)> {
        Interactive::sub_regions(&layout.table, &layout.inner)
    }

    fn handle(
        &self,
        state: &mut TreeTableModelState,
        event: &RoutedEvent,
        layout: &Self::Layout,
        cx: &mut EventCx,
    ) -> Handled<TreeTableModelEvent> {
        let outcome = state
            .sync(self)
            .handle_routed(&layout.table, event, &layout.inner, cx);
        let event = match outcome {
            TreeTableControllerEvent::Ignored => return Handled::Ignored,
            TreeTableControllerEvent::Consumed => None,
            TreeTableControllerEvent::RowSelected { id } => {
                Some(TreeTableModelEvent::RowSelected { id })
            }
            TreeTableControllerEvent::RowActivated { id } => {
                Some(TreeTableModelEvent::RowActivated { id })
            }
            TreeTableControllerEvent::ToggleExpand { id, expand } => {
                Some(TreeTableModelEvent::ToggleExpand { id, expand })
            }
            TreeTableControllerEvent::ChildrenRequested { id } => {
                Some(TreeTableModelEvent::ChildrenRequested { id })
            }
            TreeTableControllerEvent::SortChanged { col, ascending } => {
                Some(TreeTableModelEvent::SortChanged { col, ascending })
            }
        };
        // The rows, selection or scroll the widget paints from its own
        // state may have moved even when the app is told nothing.
        cx.request_redraw();
        match event {
            Some(event) => Handled::Event(event),
            None => Handled::Consumed,
        }
    }

    fn a11y_actions(&self, state: &TreeTableModelState) -> Vec<crate::a11y::A11yAction> {
        Interactive::a11y_actions(&self.flatten(Some(state)), &TreeTableState::default())
    }
}

/// Plain concatenated text of a cell — the default sort key
/// [`TreeTableController::header_clicked`] uses. A missing cell (a row
/// shorter than `col`) sorts as empty text, matching
/// [`crate::primitives::data_table`]'s own "degrade without panicking"
/// convention for a short row.
fn cell_text(cell: Option<&StyledText>) -> String {
    cell.map(|c| c.spans.iter().map(|s| s.text.as_str()).collect::<String>())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::data_table::ColumnWidth;
    use crate::ColumnAlign;

    fn leaf(id: &str, label: &str) -> TreeTableNodeData {
        TreeTableNodeData::leaf(id, vec![StyledText::plain(label)])
    }

    fn branch(id: &str, label: &str, children: Vec<TreeTableNodeData>) -> TreeTableNodeData {
        TreeTableNodeData::branch(id, vec![StyledText::plain(label)], children)
    }

    fn lazy(id: &str, label: &str) -> TreeTableNodeData {
        TreeTableNodeData::lazy_branch(id, vec![StyledText::plain(label)])
    }

    fn columns() -> Vec<Column> {
        vec![Column {
            title: "Name".into(),
            width: ColumnWidth::Flex(1.0),
            align: ColumnAlign::Left,
        }]
    }

    fn controller(nodes: Vec<TreeTableNodeData>) -> TreeTableController {
        let mut c = TreeTableController::new("tt");
        c.set_nodes(nodes);
        c
    }

    fn labels(table: &TreeTable) -> Vec<String> {
        table
            .table
            .rows
            .iter()
            .map(|r| cell_text(r.cells.first()))
            .collect()
    }

    // ── Flatten order / expand / collapse ─────────────────────────────

    #[test]
    fn collapsed_branch_hides_its_children() {
        let c = controller(vec![
            branch("src", "src", vec![leaf("main", "main.rs")]),
            leaf("cargo", "Cargo.toml"),
        ]);
        let table = c.flatten(columns());
        assert_eq!(labels(&table), vec!["src", "Cargo.toml"]);
    }

    #[test]
    fn expand_reveals_children_in_depth_first_order() {
        let mut c = controller(vec![
            branch(
                "src",
                "src",
                vec![leaf("main", "main.rs"), leaf("lib", "lib.rs")],
            ),
            leaf("cargo", "Cargo.toml"),
        ]);
        let ev = c.set_expanded("src".to_string(), true);
        assert_eq!(
            ev,
            TreeTableControllerEvent::ToggleExpand {
                id: "src".to_string(),
                expand: true,
            }
        );
        let table = c.flatten(columns());
        assert_eq!(
            labels(&table),
            vec!["src", "main.rs", "lib.rs", "Cargo.toml"]
        );
    }

    #[test]
    fn collapse_hides_descendants_again() {
        let mut c = controller(vec![branch(
            "src",
            "src",
            vec![branch("util", "util", vec![leaf("h", "helpers.rs")])],
        )]);
        c.set_expanded("src".to_string(), true);
        c.set_expanded("util".to_string(), true);
        assert_eq!(
            labels(&c.flatten(columns())),
            vec!["src", "util", "helpers.rs"]
        );

        c.set_expanded("src".to_string(), false);
        assert_eq!(
            labels(&c.flatten(columns())),
            vec!["src"],
            "collapsing an ancestor hides every descendant, not just its direct children"
        );
    }

    // ── Lazy loading ───────────────────────────────────────────────────

    #[test]
    fn expanding_a_lazy_branch_requests_children_once() {
        let mut c = controller(vec![lazy("remote", "remote/")]);
        let ev = c.set_expanded("remote".to_string(), true);
        assert_eq!(
            ev,
            TreeTableControllerEvent::ChildrenRequested {
                id: "remote".to_string()
            }
        );
        // Collapsing and re-expanding before the host answers must not
        // fire a second request.
        c.set_expanded("remote".to_string(), false);
        let ev2 = c.set_expanded("remote".to_string(), true);
        assert_ne!(
            ev2,
            TreeTableControllerEvent::ChildrenRequested {
                id: "remote".to_string()
            },
            "ChildrenRequested must fire once per node while unanswered"
        );
    }

    #[test]
    fn loading_placeholder_shown_while_children_are_pending() {
        let mut c = controller(vec![lazy("remote", "remote/")]);
        c.set_expanded("remote".to_string(), true);
        let table = c.flatten(columns());
        assert_eq!(labels(&table), vec!["remote/", "…"]);
        assert_eq!(table.table.rows[1].decoration, Decoration::Muted);
    }

    #[test]
    fn set_children_answers_the_request_and_clears_the_placeholder() {
        let mut c = controller(vec![lazy("remote", "remote/")]);
        c.set_expanded("remote".to_string(), true);
        c.set_children(
            &"remote".to_string(),
            vec![leaf("f1", "one.txt"), leaf("f2", "two.txt")],
        );
        let table = c.flatten(columns());
        assert_eq!(labels(&table), vec!["remote/", "one.txt", "two.txt"]);

        // Having answered once, a later collapse/expand resolves to a
        // plain toggle (children are now loaded) rather than a fresh
        // ChildrenRequested.
        c.set_expanded("remote".to_string(), false);
        let ev = c.set_expanded("remote".to_string(), true);
        assert_eq!(
            ev,
            TreeTableControllerEvent::ToggleExpand {
                id: "remote".to_string(),
                expand: true,
            }
        );
    }

    // ── Sibling-scoped sort ────────────────────────────────────────────

    #[test]
    fn header_click_sorts_each_parents_children_independently() {
        let mut c = controller(vec![
            branch("b", "b-dir", vec![leaf("b2", "zeta"), leaf("b1", "alpha")]),
            branch(
                "a",
                "a-dir",
                vec![leaf("a2", "yankee"), leaf("a1", "bravo")],
            ),
        ]);
        c.set_expanded("a".to_string(), true);
        c.set_expanded("b".to_string(), true);
        let ev = c.header_clicked(0);
        assert_eq!(
            ev,
            TreeTableControllerEvent::SortChanged {
                col: 0,
                ascending: true,
            }
        );
        let table = c.flatten(columns());
        // Roots sort ascending by their own label, and each root's own
        // children sort ascending independently — the hierarchy survives,
        // it isn't flattened into one column-sorted list.
        assert_eq!(
            labels(&table),
            vec!["a-dir", "bravo", "yankee", "b-dir", "alpha", "zeta"]
        );
    }

    #[test]
    fn repeat_header_click_on_the_same_column_reverses_direction() {
        let mut c = controller(vec![leaf("a", "alpha"), leaf("b", "bravo")]);
        c.header_clicked(0);
        let ev = c.header_clicked(0);
        assert_eq!(
            ev,
            TreeTableControllerEvent::SortChanged {
                col: 0,
                ascending: false,
            }
        );
        assert_eq!(labels(&c.flatten(columns())), vec!["bravo", "alpha"]);
    }

    #[test]
    fn folders_first_keeps_branches_ahead_of_leaves_regardless_of_text() {
        let mut c = controller(vec![leaf("z", "zzz"), branch("a", "aaa-dir", vec![])]);
        c.set_folders_first(true);
        c.header_clicked(0);
        assert_eq!(labels(&c.flatten(columns())), vec!["aaa-dir", "zzz"]);
    }

    // ── Selection stability ─────────────────────────────────────────────

    #[test]
    fn selection_survives_a_sort_by_node_id() {
        let mut c = controller(vec![leaf("z", "zebra"), leaf("a", "apple")]);
        c.set_selected_id(Some("z".to_string()));
        c.header_clicked(0); // ascending: apple, zebra — "z" moves to index 1
        assert_eq!(c.selected_id(), Some(&"z".to_string()));
        let table = c.flatten(columns());
        assert_eq!(table.table.selected_idx, Some(1));
    }

    #[test]
    fn selection_survives_expand_and_collapse() {
        let mut c = controller(vec![
            branch("src", "src", vec![leaf("main", "main.rs")]),
            leaf("cargo", "Cargo.toml"),
        ]);
        c.set_selected_id(Some("cargo".to_string()));
        c.set_expanded("src".to_string(), true);
        assert_eq!(c.selected_id(), Some(&"cargo".to_string()));
        c.set_expanded("src".to_string(), false);
        assert_eq!(c.selected_id(), Some(&"cargo".to_string()));
    }

    // ── Navigation (delegated to TreeNavCore) ──────────────────────────

    #[test]
    fn move_selection_down_through_expanded_children() {
        let mut c = controller(vec![
            branch("src", "src", vec![leaf("main", "main.rs")]),
            leaf("cargo", "Cargo.toml"),
        ]);
        c.set_expanded("src".to_string(), true);
        let ev = c.move_selection_by(1, 10);
        assert_eq!(
            ev,
            TreeTableControllerEvent::RowSelected { id: "src".into() }
        );
        let ev = c.move_selection_by(1, 10);
        assert_eq!(
            ev,
            TreeTableControllerEvent::RowSelected { id: "main".into() }
        );
        let ev = c.move_selection_by(1, 10);
        assert_eq!(
            ev,
            TreeTableControllerEvent::RowSelected { id: "cargo".into() }
        );
    }

    #[test]
    fn move_to_edge_selects_first_and_last_row() {
        let mut c = controller(vec![leaf("a", "a"), leaf("b", "b"), leaf("c", "c")]);
        c.move_selection_to_edge(Edge::End, 10);
        assert_eq!(c.selected_id(), Some(&"c".to_string()));
        c.move_selection_to_edge(Edge::Start, 10);
        assert_eq!(c.selected_id(), Some(&"a".to_string()));
    }

    #[test]
    fn toggle_expand_selected_toggles_the_current_branch() {
        let mut c = controller(vec![branch("src", "src", vec![leaf("main", "main.rs")])]);
        c.set_selected_id(Some("src".to_string()));
        let ev = c.toggle_expand_selected();
        assert_eq!(
            ev,
            TreeTableControllerEvent::ToggleExpand {
                id: "src".to_string(),
                expand: true,
            }
        );
        assert!(c.is_expanded(&"src".to_string()));
    }

    #[test]
    fn horizontal_right_on_collapsed_branch_expands() {
        let mut c = controller(vec![branch("src", "src", vec![leaf("main", "main.rs")])]);
        c.set_selected_id(Some("src".to_string()));
        let ev = c.handle_horizontal(HorizontalDir::Right);
        assert_eq!(
            ev,
            TreeTableControllerEvent::ToggleExpand {
                id: "src".to_string(),
                expand: true,
            }
        );
    }

    #[test]
    fn horizontal_right_on_expanded_branch_selects_first_child() {
        let mut c = controller(vec![branch("src", "src", vec![leaf("main", "main.rs")])]);
        c.set_selected_id(Some("src".to_string()));
        c.set_expanded("src".to_string(), true);
        let ev = c.handle_horizontal(HorizontalDir::Right);
        assert_eq!(
            ev,
            TreeTableControllerEvent::RowSelected { id: "main".into() }
        );
    }

    #[test]
    fn horizontal_left_on_leaf_selects_parent() {
        let mut c = controller(vec![branch("src", "src", vec![leaf("main", "main.rs")])]);
        c.set_expanded("src".to_string(), true);
        c.set_selected_id(Some("main".to_string()));
        let ev = c.handle_horizontal(HorizontalDir::Left);
        assert_eq!(
            ev,
            TreeTableControllerEvent::RowSelected { id: "src".into() }
        );
    }

    #[test]
    fn move_selection_on_empty_tree_is_ignored() {
        let mut c = TreeTableController::new("tt");
        assert_eq!(
            c.move_selection_by(1, 10),
            TreeTableControllerEvent::Ignored
        );
    }

    #[test]
    fn activate_with_no_selection_is_ignored() {
        let c = controller(vec![leaf("a", "a")]);
        assert_eq!(c.activate_selection(), TreeTableControllerEvent::Ignored);
    }

    #[test]
    fn activate_selection_emits_row_activated() {
        let mut c = controller(vec![leaf("a", "a")]);
        c.set_selected_id(Some("a".to_string()));
        assert_eq!(
            c.activate_selection(),
            TreeTableControllerEvent::RowActivated { id: "a".into() }
        );
    }

    #[test]
    fn set_nodes_preserves_expanded_and_selection_state_by_id() {
        let mut c = controller(vec![branch("src", "src", vec![leaf("main", "main.rs")])]);
        c.set_expanded("src".to_string(), true);
        c.set_selected_id(Some("main".to_string()));
        c.set_nodes(vec![branch(
            "src",
            "src",
            vec![leaf("main", "main.rs"), leaf("lib", "lib.rs")],
        )]);
        assert!(c.is_expanded(&"src".to_string()));
        assert_eq!(c.selected_id(), Some(&"main".to_string()));
        assert_eq!(
            labels(&c.flatten(columns())),
            vec!["src", "main.rs", "lib.rs"]
        );
    }

    // ── Keyboard navigation never lands on the lazy-load placeholder ──

    #[test]
    fn moving_down_from_an_unanswered_lazy_branch_skips_its_placeholder_row() {
        let mut c = controller(vec![lazy("remote", "remote/"), leaf("local", "local")]);
        c.set_selected_id(Some("remote".to_string()));
        // Expanding fires `ChildrenRequested` and leaves the "…" placeholder
        // as `remote`'s only visible child until `set_children` answers it.
        assert_eq!(
            c.set_expanded("remote".to_string(), true),
            TreeTableControllerEvent::ChildrenRequested {
                id: "remote".to_string()
            }
        );
        assert_eq!(labels(&c.flatten(columns())), vec!["remote/", "…", "local"]);

        // Down from `remote` must land on the real `local` sibling, never
        // on the synthetic placeholder row in between.
        let ev = c.move_selection_by(1, 10);
        assert_eq!(
            ev,
            TreeTableControllerEvent::RowSelected { id: "local".into() }
        );
        assert_eq!(c.selected_id(), Some(&"local".to_string()));
    }

    #[test]
    fn move_to_end_on_an_unanswered_lazy_branch_stops_before_the_placeholder() {
        let mut c = controller(vec![lazy("remote", "remote/")]);
        c.set_expanded("remote".to_string(), true);
        c.set_selected_id(Some("remote".to_string()));
        // No real row after the placeholder, so End resolves back to
        // `remote` itself rather than the synthetic "…" row: `remote` is
        // already the last *navigable* row.
        assert_eq!(
            c.move_selection_to_edge(Edge::End, 10),
            TreeTableControllerEvent::RowSelected {
                id: "remote".into()
            }
        );
        assert_eq!(c.selected_id(), Some(&"remote".to_string()));
    }

    #[test]
    fn right_arrow_on_an_expanded_lazy_branch_never_selects_the_placeholder() {
        let mut c = controller(vec![lazy("remote", "remote/")]);
        c.set_expanded("remote".to_string(), true);
        c.set_selected_id(Some("remote".to_string()));
        // "Select first child" would otherwise resolve to the placeholder's
        // path; with no real child yet, there is nothing to select.
        assert_eq!(
            c.handle_horizontal(HorizontalDir::Right),
            TreeTableControllerEvent::Ignored
        );
        assert_eq!(c.selected_id(), Some(&"remote".to_string()));
    }

    // ── `handle` — scrollbar drag + wheel scroll ──────────────────────

    #[cfg(feature = "tui")]
    mod handle_tests {
        use super::*;
        use crate::tui::TuiBackend;
        use crate::{ButtonMask, Modifiers, MouseButton, Point};

        fn many_leaves(n: usize) -> Vec<TreeTableNodeData> {
            (0..n)
                .map(|i| leaf(&i.to_string(), &i.to_string()))
                .collect()
        }

        fn mouse_down(x: f32, y: f32) -> UiEvent {
            UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(x, y),
                modifiers: Modifiers::default(),
            }
        }

        fn mouse_moved(x: f32, y: f32) -> UiEvent {
            UiEvent::MouseMoved {
                position: Point::new(x, y),
                buttons: ButtonMask {
                    left: true,
                    ..Default::default()
                },
            }
        }

        fn mouse_up(x: f32, y: f32) -> UiEvent {
            UiEvent::MouseUp {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(x, y),
            }
        }

        /// 20 rows in a 6-row-tall rect (1 header + 5 visible body rows) —
        /// enough overflow for `TreeTableController::scrollbar` to paint a
        /// track, on TUI's `row_height == header_height == scrollbar_width
        /// == 1.0`: track = `Rect(19, 1, 1, 5)`, thumb `len == 1.25`
        /// starting at the track's top (`fit_thumb`'s own math — see that
        /// function's doc).
        #[test]
        fn dragging_the_scrollbar_thumb_moves_the_scroll_offset() {
            let mut c = controller(many_leaves(20));
            let mut backend = TuiBackend::new();
            let rect = Rect::new(0.0, 0.0, 20.0, 6.0);

            // Press inside the thumb (track y in [1, 6), thumb spans
            // [1, 2.25)).
            let ev = c.handle(&mouse_down(19.5, 1.5), &mut backend, rect, columns());
            assert_eq!(ev, TreeTableControllerEvent::Consumed);
            assert_eq!(c.scroll_offset(), 0, "a thumb press alone doesn't scroll");

            // Drag to the bottom of the track's travel — clamps to the max
            // offset (20 total - 5 visible == 15).
            let ev = c.handle(&mouse_moved(19.5, 5.25), &mut backend, rect, columns());
            assert_eq!(ev, TreeTableControllerEvent::Consumed);
            assert_eq!(c.scroll_offset(), 15);

            c.handle(&mouse_up(19.5, 5.25), &mut backend, rect, columns());
            // Once released, a further `MouseMoved` with the button held
            // (a stray event after release) has no active drag to advance.
            let ev = c.handle(&mouse_moved(19.5, 1.5), &mut backend, rect, columns());
            assert_eq!(ev, TreeTableControllerEvent::Ignored);
            assert_eq!(c.scroll_offset(), 15);
        }

        #[test]
        fn clicking_the_track_above_the_thumb_pages_up() {
            let mut c = controller(many_leaves(20));
            let mut backend = TuiBackend::new();
            let rect = Rect::new(0.0, 0.0, 20.0, 6.0);
            c.set_scroll_offset(10);

            // Thumb for a scroll offset of 10 sits well below the track's
            // top (`thumb_start == 2.5` of a `3.75`-long travel); clicking
            // at the very top (`y == 1.0`) lands "Before" it.
            let ev = c.handle(&mouse_down(19.5, 1.0), &mut backend, rect, columns());
            assert_eq!(ev, TreeTableControllerEvent::Consumed);
            assert_eq!(c.scroll_offset(), 5, "page up by the 5 visible rows");
        }

        #[test]
        fn wheel_scroll_advances_the_offset_by_the_real_row_pitch() {
            let mut c = controller(many_leaves(20));
            let mut backend = TuiBackend::new();
            let rect = Rect::new(0.0, 0.0, 20.0, 6.0);

            // Negative `delta.y` scrolls content *down* (increases the
            // offset) per `ScrollDelta`'s own sign convention.
            let ev = c.handle(
                &UiEvent::Scroll {
                    widget: None,
                    delta: crate::ScrollDelta::new(0.0, -3.0),
                    position: Point::new(0.0, 0.0),
                },
                &mut backend,
                rect,
                columns(),
            );
            assert_eq!(ev, TreeTableControllerEvent::Consumed);
            // TUI's `row_height == line_height == 1.0`, so this doesn't by
            // itself distinguish the two — the regression this guards is
            // `handle`'s `Scroll` arm reading `DataTableLayout::row_height`
            // (via `TreeTableController::layout`) rather than
            // `Backend::line_height` directly, matching
            // the backend's own row pitch
            // rule.
            assert_eq!(c.scroll_offset(), 3);
        }

        fn key_with(key: crate::Key, modifiers: Modifiers) -> UiEvent {
            UiEvent::KeyPressed {
                key,
                modifiers,
                repeat: false,
            }
        }

        fn press_key(c: &mut TreeTableController, key: crate::Key) -> TreeTableControllerEvent {
            let mut backend = TuiBackend::new();
            let rect = Rect::new(0.0, 0.0, 30.0, 12.0);
            c.handle(
                &key_with(key, Modifiers::default()),
                &mut backend,
                rect,
                columns(),
            )
        }

        fn down(c: &mut TreeTableController) -> TreeTableControllerEvent {
            press_key(c, crate::Key::Named(crate::NamedKey::Down))
        }

        #[test]
        fn letters_and_select_all_stay_the_apps_keys() {
            let mut c = controller(vec![leaf("alpha", "alpha"), leaf("beta", "beta")]);
            assert_eq!(
                press_key(&mut c, crate::Key::Char('q')),
                TreeTableControllerEvent::Ignored
            );
            // A letter that would type-ahead to a row is not claimed either.
            assert_eq!(
                press_key(&mut c, crate::Key::Char('b')),
                TreeTableControllerEvent::Ignored
            );
            assert_eq!(c.selected_id(), None);
            let mut backend = TuiBackend::new();
            let ctrl_a = key_with(
                crate::Key::Char('a'),
                Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
            );
            assert_eq!(
                c.handle(
                    &ctrl_a,
                    &mut backend,
                    Rect::new(0.0, 0.0, 30.0, 12.0),
                    columns()
                ),
                TreeTableControllerEvent::Ignored
            );
            assert_eq!(
                down(&mut c),
                TreeTableControllerEvent::RowSelected {
                    id: "alpha".to_string()
                }
            );
        }

        #[test]
        fn down_after_clearing_the_selection_starts_at_the_first_row() {
            let mut c = controller(vec![leaf("a", "a"), leaf("b", "b"), leaf("c", "c")]);
            down(&mut c);
            down(&mut c);
            assert_eq!(c.selected_id(), Some(&"b".to_string()));
            c.set_selected_id(None);
            assert_eq!(
                down(&mut c),
                TreeTableControllerEvent::RowSelected {
                    id: "a".to_string()
                }
            );
        }
    }

    // ── TreeTableModel: the descriptor is content, the state is the user's ──

    mod model {
        use super::*;
        use crate::event::{Key, NamedKey, Point};
        use crate::interactive::KeyEvent;
        use crate::types::Modifiers;

        fn model(nodes: Vec<TreeTableNodeData>) -> TreeTableModel {
            TreeTableModel {
                id: WidgetId::new("tt"),
                columns: columns(),
                nodes,
                selected_id: None,
                has_focus: true,
                show_scrollbar: false,
                bordered: false,
                folders_first: false,
                sort_override: false,
            }
        }

        fn layout(m: &TreeTableModel, state: &TreeTableModelState) -> TreeTableModelLayout {
            let table = m.flatten(Some(state));
            let inner = Primitive::layout(
                &table,
                &LayoutCtx::new(1.0, 1.0, false),
                Rect::new(0.0, 0.0, 30.0, 12.0),
            );
            TreeTableModelLayout { table, inner }
        }

        fn run(
            m: &TreeTableModel,
            state: &mut TreeTableModelState,
            event: RoutedEvent,
        ) -> Handled<TreeTableModelEvent> {
            let layout = layout(m, state);
            let (handled, _) = widget_host::with_cx(m.id, crate::primitive::Units::Cells, |cx| {
                Interactive::handle(m, state, &event, &layout, cx)
            });
            handled
        }

        fn key(key: Key) -> RoutedEvent {
            RoutedEvent::Key(KeyEvent {
                key,
                modifiers: Modifiers::default(),
                repeat: false,
            })
        }

        fn header_click() -> RoutedEvent {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::event::MouseButton::Left),
                sub: None,
                local: Point::new(2.0, 0.0),
                mods: Modifiers::default(),
            }
        }

        fn shown(m: &TreeTableModel, state: &TreeTableModelState) -> Vec<String> {
            labels(&m.flatten(Some(state)))
        }

        #[test]
        fn lazy_children_arrive_as_descriptor_content() {
            let mut m = model(vec![lazy("root", "root")]);
            m.selected_id = Some("root".into());
            let mut state = TreeTableModelState::default();
            assert_eq!(
                run(&m, &mut state, key(Key::Char(' '))),
                Handled::Event(TreeTableModelEvent::ChildrenRequested { id: "root".into() })
            );
            assert_eq!(shown(&m, &state), vec!["root", "…"]);
            // Expanding again before the app answers asks nothing more.
            let _ = run(&m, &mut state, key(Key::Named(NamedKey::Left)));
            assert_eq!(
                run(&m, &mut state, key(Key::Named(NamedKey::Right))),
                Handled::Consumed
            );

            m.nodes = vec![branch("root", "root", vec![leaf("b", "b"), leaf("a", "a")])];
            assert_eq!(shown(&m, &state), vec!["root", "b", "a"]);
            assert_eq!(
                run(&m, &mut state, key(Key::Named(NamedKey::Down))),
                Handled::Event(TreeTableModelEvent::RowSelected { id: "b".into() })
            );
        }

        #[test]
        fn a_header_click_sorts_the_sent_nodes_unless_the_app_sorts_them() {
            let mut m = model(vec![leaf("b", "b"), leaf("c", "c"), leaf("a", "a")]);
            let mut state = TreeTableModelState::default();
            assert_eq!(
                run(&m, &mut state, header_click()),
                Handled::Event(TreeTableModelEvent::SortChanged {
                    col: 0,
                    ascending: true
                })
            );
            assert_eq!(shown(&m, &state), vec!["a", "b", "c"]);

            m.sort_override = true;
            m.nodes = vec![leaf("c", "c"), leaf("a", "a"), leaf("b", "b")];
            assert_eq!(shown(&m, &state), vec!["c", "a", "b"]);
        }

        #[test]
        fn the_user_selection_stands_until_the_app_changes_selected_id() {
            let mut m = model(vec![leaf("a", "a"), leaf("b", "b"), leaf("c", "c")]);
            m.selected_id = Some("a".into());
            let mut state = TreeTableModelState::default();
            let _ = run(&m, &mut state, key(Key::Named(NamedKey::Down)));
            assert_eq!(m.selection(&state), Some(&"b".to_string()));

            m.selected_id = Some("c".into());
            assert_eq!(m.selection(&state), Some(&"c".to_string()));
            assert_eq!(
                run(&m, &mut state, key(Key::Named(NamedKey::Up))),
                Handled::Event(TreeTableModelEvent::RowSelected { id: "b".into() })
            );
        }

        #[test]
        fn letter_keys_stay_the_apps() {
            let m = model(vec![leaf("a", "a")]);
            let mut state = TreeTableModelState::default();
            assert_eq!(run(&m, &mut state, key(Key::Char('x'))), Handled::Ignored);
        }
    }
}
