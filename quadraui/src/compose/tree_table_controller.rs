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
//! Navigation, selection, scroll-to-follow and activation are not
//! reimplemented here — every one of them goes through
//! [`crate::compose::tree_nav::TreeNavCore`], the same row-agnostic core
//! [`crate::compose::TreeController`] uses for `TreeView`. This controller's
//! own job is translating between [`NodeId`] (stable across a sort, which
//! reorders a flattened row's [`crate::TreePath`]) and the positional
//! [`crate::TreePath`] [`TreeNavCore`] tracks selection by: every navigation
//! call re-resolves the selected id's current path from the latest
//! flatten, hands that to [`TreeNavCore`], and translates its answer back
//! to an id.
//!
//! # Lazy loading
//!
//! A node built via [`TreeTableNodeData::lazy_branch`] has `has_children:
//! true` but no loaded children yet. Expanding it (chevron click, Right,
//! or Space) emits [`TreeTableControllerEvent::ChildrenRequested`] exactly
//! once — further toggles of the same node before [`Self::set_children`]
//! answers it are no-ops on that front — and [`Self::flatten`] shows a
//! single dim `…` placeholder row (via [`crate::Decoration::Muted`]) as
//! that node's lone visible child in the meantime.
//!
//! # Sibling-scoped sort
//!
//! [`Self::header_clicked`] (an app's own [`TreeTableHit::Header`]
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

use crate::compose::tree_controller::{classify_tree_nav_key, TreeNavKey};
use crate::compose::tree_nav::{
    Edge, HorizontalDir, HorizontalNav, NavOutcome, NavRow, TreeNavCore,
};
use crate::primitives::data_table::{Column, DataRow, DataTable, SortDirection};
use crate::primitives::tree_table::{TreeTable, TreeTableHit, TreeTableNode};
use crate::types::{Decoration, Icon, StyledText, TreePath};
use crate::{Backend, Key, Modifiers, MouseButton, Point, Rect, UiEvent, WidgetId};

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
#[derive(Debug, Clone, PartialEq)]
pub struct TreeTableNodeData {
    id: NodeId,
    cells: Vec<StyledText>,
    icon: Option<Icon>,
    has_children: bool,
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
    /// Selection, scroll position and (for a future scrollbar-drag
    /// consumer) drag state — the row-agnostic core shared with
    /// [`crate::compose::TreeController`]; see [`crate::compose::tree_nav`].
    nav: TreeNavCore,
    has_focus: bool,
    show_scrollbar: bool,
    bordered: bool,
}

impl TreeTableController {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: WidgetId::new(id),
            roots: Vec::new(),
            expanded: HashSet::new(),
            requested: HashSet::new(),
            selected_id: None,
            sort: None,
            folders_first: false,
            nav: TreeNavCore::new(),
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
        self.nav.scroll_offset()
    }

    pub fn set_scroll_offset(&mut self, offset: usize) {
        self.nav.set_scroll_offset(offset);
    }

    pub fn node(&self, id: &NodeId) -> Option<&TreeTableNodeData> {
        TreeTableNodeData::find(&self.roots, id)
    }

    // ── Flatten + render ──────────────────────────────────────────────

    /// Flatten the current model (honoring the expanded set and the
    /// not-yet-loaded placeholder) into a [`TreeTable`] — what
    /// [`Self::render`] paints, and what a caller doing its own
    /// hit-testing (rather than going through [`Self::handle`]) should
    /// resolve clicks against via [`crate::Backend::tree_table_layout`].
    ///
    /// `columns` is taken by value each call rather than stored (a host
    /// typically recomputes it from the current viewport width, e.g. to
    /// drop trailing columns on a narrow rect — see
    /// [`crate::primitives::tree_table::TreeTable::layout`]'s own
    /// `min_tree_col_width` rule), so this never goes stale the way a
    /// `set_columns` cached on the controller would across a resize that
    /// doesn't otherwise touch the model.
    pub fn flatten(&self, columns: Vec<Column>) -> TreeTable {
        let rows = self.flatten_rows();
        let selected_idx = self
            .selected_id
            .as_ref()
            .and_then(|id| rows.iter().position(|r| &r.id == id));
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
            tree.push(node);
        }
        TreeTable {
            table: DataTable {
                id: self.id,
                columns,
                rows: table_rows,
                selected_idx,
                scroll_offset: self.nav.scroll_offset(),
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
            },
            tree,
        }
    }

    pub fn render(&self, backend: &mut dyn Backend, rect: Rect, columns: Vec<Column>) {
        let table = self.flatten(columns);
        backend.draw_tree_table(rect, &table, None);
    }

    // ── Handle ────────────────────────────────────────────────────────

    pub fn handle(
        &mut self,
        event: &UiEvent,
        backend: &mut dyn Backend,
        rect: Rect,
        columns: Vec<Column>,
    ) -> TreeTableControllerEvent {
        match event {
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => self.click(backend, rect, *position, columns),

            UiEvent::DoubleClick { position, .. } => {
                self.double_click(backend, rect, *position, columns)
            }

            UiEvent::Scroll { delta, .. } => {
                let vr = self.viewport_rows(backend, rect, columns);
                let row_height = backend.line_height();
                let rows = self.flatten_rows();
                self.nav.scroll_by_wheel(
                    backend.wheel_delta_rows(delta.y, row_height),
                    row_height,
                    rows.len(),
                    vr,
                );
                TreeTableControllerEvent::Consumed
            }

            UiEvent::KeyPressed { key, modifiers, .. } => {
                self.handle_key(key, modifiers, backend, rect, columns)
            }

            _ => TreeTableControllerEvent::Ignored,
        }
    }

    // ── Navigation primitives ─────────────────────────────────────────

    pub fn move_selection_by(
        &mut self,
        delta: isize,
        viewport_rows: usize,
    ) -> TreeTableControllerEvent {
        let rows = self.flatten_rows();
        self.sync_nav_selection(&rows);
        match self.nav.move_selection_by(&rows, delta, viewport_rows) {
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
        let rows = self.flatten_rows();
        self.sync_nav_selection(&rows);
        match self.nav.move_selection_to_edge(&rows, edge, viewport_rows) {
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
        let rows = self.flatten_rows();
        self.sync_nav_selection(&rows);
        let Some(path) = self.nav.toggle_expand(&rows) else {
            return TreeTableControllerEvent::Ignored;
        };
        let Some(id) = Self::id_for_path(&rows, &path) else {
            return TreeTableControllerEvent::Ignored;
        };
        let expand = !self.expanded.contains(&id);
        self.set_expanded(id, expand)
    }

    /// Left/Right: collapse-or-select-parent / expand-or-select-first-child,
    /// via [`crate::compose::tree_nav::TreeNavCore::horizontal_nav`] —
    /// the same rule [`crate::TreeTableLayout::chevron_toggle`]'s
    /// Left/Right counterpart documents, applied against this
    /// controller's own id-keyed model instead of re-derived.
    pub fn handle_horizontal(&mut self, dir: HorizontalDir) -> TreeTableControllerEvent {
        let rows = self.flatten_rows();
        self.sync_nav_selection(&rows);
        match self.nav.horizontal_nav(&rows, dir) {
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
        self.nav.scroll_to_visible(row, viewport_rows);
    }

    pub fn page_scroll(&mut self, delta: isize, viewport_rows: usize) {
        let total = self.flatten_rows().len();
        self.nav.page_scroll(delta, total, viewport_rows);
    }

    // ── Sort ──────────────────────────────────────────────────────────

    /// An app's own [`TreeTableHit::Header`] resolution: toggles
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
        self.sort = Some((col, ascending));
        Self::sort_level(&mut self.roots, col, ascending, self.folders_first);
        TreeTableControllerEvent::SortChanged { col, ascending }
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
        self.nav.set_selected_path(path);
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
        let mut out = Vec::new();
        Self::flatten_into(&self.roots, 0, &[], &self.expanded, &mut out);
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
                        });
                    }
                }
            }
        }
    }

    fn click(
        &mut self,
        backend: &mut dyn Backend,
        rect: Rect,
        position: Point,
        columns: Vec<Column>,
    ) -> TreeTableControllerEvent {
        if !rect.contains(position) {
            return TreeTableControllerEvent::Ignored;
        }
        let table = self.flatten(columns);
        let layout = backend.tree_table_layout(rect, &table);
        let scroll_offset = table.table.scroll_offset;
        let total_rows = table.table.rows.len();
        match layout.hit_test(
            position.x - rect.x,
            position.y - rect.y,
            scroll_offset,
            total_rows,
        ) {
            TreeTableHit::Header { col } => self.header_clicked(col),
            TreeTableHit::HeaderDivider { .. } => TreeTableControllerEvent::Ignored,
            TreeTableHit::Chevron { idx } => {
                let rows = self.flatten_rows();
                let Some(row) = rows.get(idx) else {
                    return TreeTableControllerEvent::Ignored;
                };
                if row.loading {
                    return TreeTableControllerEvent::Consumed;
                }
                let id = row.id.clone();
                let expand = row.is_expanded == Some(false);
                self.selected_id = Some(id.clone());
                self.set_expanded(id, expand)
            }
            TreeTableHit::Row { idx } => {
                let rows = self.flatten_rows();
                match rows.get(idx) {
                    Some(row) if !row.loading => {
                        self.selected_id = Some(row.id.clone());
                        TreeTableControllerEvent::RowSelected { id: row.id.clone() }
                    }
                    _ => TreeTableControllerEvent::Consumed,
                }
            }
            TreeTableHit::Footer | TreeTableHit::Empty => TreeTableControllerEvent::Consumed,
        }
    }

    fn double_click(
        &mut self,
        backend: &mut dyn Backend,
        rect: Rect,
        position: Point,
        columns: Vec<Column>,
    ) -> TreeTableControllerEvent {
        if !rect.contains(position) {
            return TreeTableControllerEvent::Ignored;
        }
        let table = self.flatten(columns);
        let layout = backend.tree_table_layout(rect, &table);
        let scroll_offset = table.table.scroll_offset;
        let total_rows = table.table.rows.len();
        match layout.hit_test(
            position.x - rect.x,
            position.y - rect.y,
            scroll_offset,
            total_rows,
        ) {
            TreeTableHit::Row { idx } | TreeTableHit::Chevron { idx } => {
                let rows = self.flatten_rows();
                match rows.get(idx) {
                    Some(row) if !row.loading => {
                        self.selected_id = Some(row.id.clone());
                        TreeTableControllerEvent::RowActivated { id: row.id.clone() }
                    }
                    _ => TreeTableControllerEvent::Consumed,
                }
            }
            _ => TreeTableControllerEvent::Consumed,
        }
    }

    fn handle_key(
        &mut self,
        key: &Key,
        _modifiers: &Modifiers,
        backend: &mut dyn Backend,
        rect: Rect,
        columns: Vec<Column>,
    ) -> TreeTableControllerEvent {
        if let Some(nav) = classify_tree_nav_key(key, true) {
            let vr = self.viewport_rows(backend, rect, columns);
            return match nav {
                TreeNavKey::Up => self.move_selection_by(-1, vr),
                TreeNavKey::Down => self.move_selection_by(1, vr),
                TreeNavKey::Home => self.move_selection_to_edge(Edge::Start, vr),
                TreeNavKey::End => self.move_selection_to_edge(Edge::End, vr),
                TreeNavKey::PageUp => {
                    self.move_selection_by(-((vr.max(1) - 1).max(1) as isize), vr)
                }
                TreeNavKey::PageDown => self.move_selection_by((vr.max(1) - 1).max(1) as isize, vr),
                TreeNavKey::Enter => self.activate_selection(),
                TreeNavKey::ToggleExpand => self.toggle_expand_selected(),
            };
        }
        use crate::NamedKey;
        match key {
            Key::Named(NamedKey::Left) => self.handle_horizontal(HorizontalDir::Left),
            Key::Named(NamedKey::Right) => self.handle_horizontal(HorizontalDir::Right),
            _ => TreeTableControllerEvent::Ignored,
        }
    }

    fn viewport_rows(&self, backend: &dyn Backend, rect: Rect, columns: Vec<Column>) -> usize {
        let mut table = self.flatten(columns);
        table.table.scroll_offset = 0;
        let layout = backend.tree_table_layout(rect, &table);
        layout.table.visible_rows
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
}
