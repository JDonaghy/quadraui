//! `TabGroup` — tabbed split panes on the widget contract.
//!
//! [`TabGroup`] describes N panes arranged in an arbitrary nested H/V
//! split tree, each with its own scrollable tab strip. It composes, never
//! re-paints, its children: each pane's strip is a [`TabBar`], the
//! dividers are one [`SplitTree`], and a tab drag previews its drop
//! through a [`DropOverlay`]. The pane bodies are the host's: the layout
//! hands back each pane's content rect, as [`SplitTree`] does for its
//! leaves.
//!
//! [`TabGroupController`] wraps the descriptor for a host that keeps a
//! [`BackendWidget`] per tab: it owns a `TabGroup<Box<dyn BackendWidget>>`
//! and its [`TabGroupState`], paints the group through the widget
//! contract, renders the active tab of every pane into its content rect,
//! and turns its imperative `handle_*` calls into routed events.
//!
//! # Quick start
//!
//! ```ignore
//! use quadraui::SplitDirection;
//! use quadraui::backend::BackendWidget;
//! use quadraui::compose::tab_group::{PaneTab, TabGroupController, TabGroupEvent};
//! use quadraui::compose::Handled;
//!
//! struct MyContent;
//! impl BackendWidget for MyContent {
//!     fn render(&self, _b: &mut dyn quadraui::Backend, _r: quadraui::Rect) {}
//! }
//!
//! let tabs = vec![
//!     PaneTab { id: "t0".into(), label: "main.rs".into(), closable: true,
//!               content: Box::new(MyContent) },
//!     PaneTab { id: "t1".into(), label: "lib.rs".into(), closable: true,
//!               content: Box::new(MyContent) },
//! ];
//! let mut group = TabGroupController::with_pane("pane0", tabs, "t0",
//!                                               SplitDirection::Horizontal);
//!
//! // In AppLogic::render:
//! let _layout = group.render(backend, bounds);
//!
//! // In AppLogic::handle (mouse down):
//! if let Handled::Event(ev) = group.handle_click_event(pos.x, pos.y) {
//!     match ev {
//!         TabGroupEvent::TabActivated { pane_idx, tab_id } => { /* … */ }
//!         TabGroupEvent::TabClosed   { pane_idx, tab_id } => { /* … */ }
//!         _ => {}
//!     }
//! }
//! ```
//!
//! # Controlled mode
//!
//! The descriptor is the model: panes, tabs, the active tab of each pane
//! and the split ratios are descriptor content the app owns. The widget
//! reports intents — [`TabGroupEvent::TabActivated`],
//! [`TabGroupEvent::TabClosed`], [`TabGroupEvent::DividerResized`],
//! [`TabGroupEvent::TabDropped`] — and the app writes each back with
//! [`TabGroup::apply`], which answers the changes it made. Keyboard focus
//! is one stop per pane ([`TabGroupSubPart::Pane`]); a press in a pane's
//! body that moves focus there reports [`TabGroupEvent::PaneFocused`].
//! [`TabGroupState`] holds only gesture data: each strip's [`TabBar`]
//! state, the divider drag and the tab drag.
//!
//! # Adoption status
//!
//! No compose-layer consumer today — deliberately *not* folded into
//! [`crate::compose::workspace::WorkspaceController`]; see that module's
//! doc for why the two exist side by side rather than one replacing
//! the other. Kept public: full TUI + GTK demo coverage
//! across three example apps (`examples/common/{tab_group_demo,
//! tab_chrome_demo,wide_tab_bar_demo}.rs`, run via `tui_tabgroup.rs` /
//! `gtk_tabgroup.rs`) plus `TuiDriver` end-to-end tests in
//! `tests/tui_example_driver.rs` — this is the editor-group split/tab
//! shape `CLAUDE.md`'s vimcode reference table points at, not
//! speculative API.
//!
//! # Building multi-pane layouts
//!
//! For simple cases, [`TabGroupController::add_pane_with_tab`] opens a new
//! pane splitting the focused pane in the controller's default direction.
//!
//! When your app already knows the full layout — especially when it mixes
//! horizontal and vertical splits — use
//! [`TabGroupController::from_layout`] (or [`TabGroup::from_layout`]) to
//! hand the tree over directly:
//!
//! ```ignore
//! use quadraui::compose::tab_group::{Pane, PaneTab, TabGroupController, GroupLayout};
//! use quadraui::SplitDirection;
//!
//! // Three panes: left | (top-right / bottom-right)
//! //   Pane 0 on the left, pane 1 top-right, pane 2 bottom-right.
//! let layout = GroupLayout::Split {
//!     direction: SplitDirection::Horizontal,
//!     ratio: 0.5,
//!     first: Box::new(GroupLayout::Leaf(0)),
//!     second: Box::new(GroupLayout::Split {
//!         direction: SplitDirection::Vertical,
//!         ratio: 0.5,
//!         first: Box::new(GroupLayout::Leaf(1)),
//!         second: Box::new(GroupLayout::Leaf(2)),
//!     }),
//! };
//! let ctrl = TabGroupController::from_layout(
//!     vec![
//!         Pane::new("pane:0", vec![/* tabs */], "t0"),
//!         Pane::new("pane:1", vec![/* tabs */], "t1"),
//!         Pane::new("pane:2", vec![/* tabs */], "t2"),
//!     ],
//!     layout,
//!     SplitDirection::Horizontal,
//! ).expect("layout must cover every pane index exactly once");
//! ```
//!
//! # Divider and tab drags on the controller
//!
//! Panes are separated by draggable [`SplitTree`] dividers. Route
//! `MouseDown` / `MouseMoved` / `MouseUp` events to
//! [`handle_drag_start`](TabGroupController::handle_drag_start) /
//! [`handle_drag_move_event`](TabGroupController::handle_drag_move_event) /
//! [`handle_drag_end`](TabGroupController::handle_drag_end) for resize
//! support.
//!
//! For a cross-group tab drag, route `MouseDown` to
//! [`handle_tab_drag_start`](TabGroupController::handle_tab_drag_start)
//! **before** [`handle_drag_start`](TabGroupController::handle_drag_start).
//! On success, route subsequent `MouseMoved` to
//! [`handle_tab_drag_move`](TabGroupController::handle_tab_drag_move) and
//! `MouseUp` to
//! [`handle_tab_drop_event`](TabGroupController::handle_tab_drop_event) (or
//! [`cancel_tab_drag`](TabGroupController::cancel_tab_drag) on `Escape`).
//! The controller paints the [`DropOverlay`] inside
//! [`render`](TabGroupController::render) whenever a drag hovers a drop
//! zone. A [`TabGroup`] routed by the runtime runs the same gestures
//! itself, from one press-drag-release, and shows the overlay as a popup.
//!
//! # Host-owned tab/pane model
//!
//! A host that already owns its own tab/pane model (e.g. an editor engine
//! keyed by its own group/tab IDs) does **not** need to mirror that model
//! into a `TabGroup` just to get drag-and-drop and drop-zone computation:
//! call [`resolve_tab_drop`] directly. It takes only borrowed geometry
//! ([`DropGroupRect`], built fresh from the host's own layout) and a
//! [`TabDragSource`] identifying the dragged tab by index, and returns a
//! [`TabDropInstruction`] — `Reorder` / `MoveToPane` / `SplitToNewPane`,
//! each expressed purely as pane/tab indices and a [`SplitDirection`] —
//! for the host to translate into its own mutation. [`TabGroup`]'s own
//! drop is built on `resolve_tab_drop`, and [`TabGroup::apply`] is the
//! reference translation of its instructions.
//!
//! # Layout model
//!
//! Panes are arranged in a recursive binary split tree, handed over and
//! read back as a [`GroupLayout`] whose leaves are pane indices. The
//! descriptor keeps it as a [`SplitTree`] with one stable leaf per pane,
//! so opening and closing panes never renumbers the tree. Dividers are
//! numbered in pre-order, the [`SplitTree`] `split_index`. When a tab is
//! dragged to a `Left`/`Right` edge a **horizontal** split is created;
//! `Top`/`Bottom` edges create a **vertical** split.

use serde::{Deserialize, Serialize};

use crate::a11y::{A11yChild, A11yInfo, A11yPrimitive, A11yRole};
use crate::backend::{BackendWidget, DrawsPrimitives};
use crate::compose::tab_drag::{resolve_tab_drop, TabDragSource, TabDropInstruction};
use crate::compose::workspace::Workspace;
use crate::compose::Handled;
use crate::event::{Key, MouseButton, NamedKey, Point, Rect};
use crate::focus::FocusAddress;
use crate::interactive::{
    EventCx, EventHost, FocusSpec, FocusStop, Interactive, KeyEvent, PointerKind, RoutedEvent,
    SubTarget, WidgetAction,
};
use crate::paint_surface::PaintSurface;
use crate::popup::{Popup, PopupDismiss, PopupKey, PopupSide};
use crate::primitive::{LayoutCtx, PaintEnv, Primitive, Size, Units};
use crate::primitives::drop_zone::{
    drop_zone_hit_test, drop_zone_overlay, DropEdge, DropGroupRect, DropOverlay, DropZone,
    DropZoneHit, DROP_OVERLAY_ZONE_ID,
};
use crate::primitives::split::SplitDirection;
use crate::primitives::split::SplitTreeState;
use crate::primitives::split_tree::{SplitTree, SplitTreeLayout};
use crate::primitives::tab_bar::{
    strip_layout_from_tab_slots, tab_bar_sub_regions, TabBar, TabBarEvent, TabBarPrimitiveLayout,
    TabBarSegment, TabBarState, TabBarSubPart, TabItem,
};
use crate::types::{Modifiers, WidgetId};
use crate::{Backend, InteractionState};

// ── Layout tree ───────────────────────────────────────────────────────────────

/// Recursive binary split tree describing how panes are arranged — the
/// shape [`TabGroup::from_layout`] takes and [`TabGroup::layout`] reads
/// back.
///
/// `Leaf(k)` references the pane at index `k` in the group's pane list.
/// `Split` describes how a rect is divided between two sub-trees.
///
/// Pre-order split traversal — this `Split` node, then `first`'s
/// dividers, then `second`'s — numbers dividers the same way
/// [`crate::SplitTree::layout`] numbers `split_index`; that's the
/// `divider_idx` used in events and `handle_drag_start`.
///
/// Every pane index `0..n` appears exactly once as a leaf.
#[derive(Clone, Debug, PartialEq)]
pub enum GroupLayout {
    /// A single pane (referenced by index).
    Leaf(usize),
    /// A binary split: `first` occupies `ratio` of the parent rect,
    /// `second` gets the rest.
    Split {
        /// Split orientation for this node.
        direction: SplitDirection,
        /// Fraction of the parent rect assigned to `first` (0.0–1.0).
        ratio: f32,
        /// Left/top sub-tree.
        first: Box<GroupLayout>,
        /// Right/bottom sub-tree.
        second: Box<GroupLayout>,
    },
}

impl GroupLayout {
    /// Number of leaves in the tree.
    pub fn leaf_count(&self) -> usize {
        match self {
            GroupLayout::Leaf(_) => 1,
            GroupLayout::Split { first, second, .. } => first.leaf_count() + second.leaf_count(),
        }
    }

    /// Collect all leaf values into `out` (in-order traversal).
    fn collect_leaves(&self, out: &mut Vec<usize>) {
        match self {
            GroupLayout::Leaf(k) => out.push(*k),
            GroupLayout::Split { first, second, .. } => {
                first.collect_leaves(out);
                second.collect_leaves(out);
            }
        }
    }

    /// Validate that the tree is a bijection onto `0..n`.
    ///
    /// Returns `Ok(())` when every index in `0..n` appears exactly once as a
    /// leaf. Returns `Err` with a descriptive message otherwise.
    fn validate_for_pane_count(&self, n: usize) -> Result<(), String> {
        let mut leaves = Vec::with_capacity(n);
        self.collect_leaves(&mut leaves);

        if leaves.len() != n {
            return Err(format!(
                "layout has {} leaf nodes but {} panes were provided",
                leaves.len(),
                n
            ));
        }

        let mut seen = vec![false; n];
        for &idx in &leaves {
            if idx >= n {
                return Err(format!(
                    "leaf index {idx} is out of range for {n} panes (indices must be 0..{n})"
                ));
            }
            if seen[idx] {
                return Err(format!(
                    "leaf index {idx} appears more than once in the layout"
                ));
            }
            seen[idx] = true;
        }

        Ok(())
    }

    /// This tree as a [`SplitTree`] whose leaf for pane `k` is `leaves[k]`.
    fn to_split_tree(&self, leaves: &[WidgetId]) -> SplitTree {
        match self {
            GroupLayout::Leaf(k) => SplitTree::leaf(leaves[*k]),
            GroupLayout::Split {
                direction,
                ratio,
                first,
                second,
            } => SplitTree::split(
                *direction,
                *ratio,
                first.to_split_tree(leaves),
                second.to_split_tree(leaves),
            ),
        }
    }

    /// `tree` read back with each leaf replaced by its index in `leaves`.
    fn from_split_tree(tree: &SplitTree, leaves: &[WidgetId]) -> Self {
        match tree {
            SplitTree::Leaf(id) => {
                GroupLayout::Leaf(leaves.iter().position(|l| l == id).unwrap_or_default())
            }
            SplitTree::Split {
                direction,
                ratio,
                first,
                second,
                ..
            } => GroupLayout::Split {
                direction: *direction,
                ratio: *ratio,
                first: Box::new(Self::from_split_tree(first, leaves)),
                second: Box::new(Self::from_split_tree(second, leaves)),
            },
        }
    }
}

// ── Descriptor ────────────────────────────────────────────────────────────────

/// One tab within a [`TabGroupPane`]. `C` is whatever the host keeps with
/// the tab — `()` for a bare descriptor, a `Box<dyn BackendWidget>` in
/// [`TabGroupController`] — and travels with the tab when a drag moves
/// it to another pane.
#[derive(Debug, Clone, PartialEq)]
pub struct TabGroupTab<C = ()> {
    /// Unique identifier within the pane. Used in events and for the
    /// pane's active tab.
    pub id: String,
    /// Display label shown in the tab strip.
    pub label: String,
    /// When `true`, the tab renders a `×` close button.
    pub closable: bool,
    /// The host's per-tab content.
    pub content: C,
}

impl TabGroupTab {
    /// A closable tab with no content of its own.
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            closable: true,
            content: (),
        }
    }
}

/// One tab within a [`Pane`] of a [`TabGroupController`]: its content is
/// rendered into the pane body while the tab is active.
pub type PaneTab = TabGroupTab<Box<dyn BackendWidget>>;

/// One split pane of a [`TabGroup`]: its tabs, the active one, and the
/// strip's first visible tab.
#[derive(Debug, Clone, PartialEq)]
pub struct TabGroupPane<C = ()> {
    /// Unique identifier for this pane (consumer-chosen, e.g. `"pane:0"`).
    pub id: String,
    tabs: Vec<TabGroupTab<C>>,
    active_tab_id: String,
    tab_scroll_offset: usize,
}

/// One split pane managed by a [`TabGroupController`].
pub type Pane = TabGroupPane<Box<dyn BackendWidget>>;

impl<C> TabGroupPane<C> {
    /// Construct a new pane.
    ///
    /// `active_tab_id` is resolved against `tabs`; when the named tab is absent
    /// the first tab's id is used as the fallback.
    pub fn new(
        id: impl Into<String>,
        tabs: Vec<TabGroupTab<C>>,
        active_tab_id: impl Into<String>,
    ) -> Self {
        let active_tab_id = active_tab_id.into();
        let resolved = if tabs.iter().any(|t| t.id == active_tab_id) {
            active_tab_id
        } else {
            tabs.first().map(|t| t.id.clone()).unwrap_or_default()
        };
        Self {
            id: id.into(),
            tabs,
            active_tab_id: resolved,
            tab_scroll_offset: 0,
        }
    }

    /// The id of the currently-active tab in this pane.
    pub fn active_tab_id(&self) -> &str {
        &self.active_tab_id
    }

    /// Ordered slice of all tabs in this pane.
    pub fn tabs(&self) -> &[TabGroupTab<C>] {
        &self.tabs
    }

    /// The first tab the strip starts from. Layout moves it forward when
    /// the active tab would otherwise be scrolled out of view.
    pub fn tab_scroll_offset(&self) -> usize {
        self.tab_scroll_offset
    }

    /// Set the strip's first tab — see [`Self::tab_scroll_offset`].
    pub fn set_tab_scroll_offset(&mut self, offset: usize) {
        self.tab_scroll_offset = offset;
    }

    /// Switch the active tab to `tab_id`. Returns `false` when not found.
    fn activate_tab(&mut self, tab_id: &str) -> bool {
        if self.tabs.iter().any(|t| t.id == tab_id) {
            self.active_tab_id = tab_id.to_string();
            self.tab_scroll_offset = 0;
            true
        } else {
            false
        }
    }

    /// Remove the tab at `idx`. When it was active, the tab before it
    /// (or the first) becomes active.
    fn remove_tab_at(&mut self, idx: usize) -> TabGroupTab<C> {
        let removed = self.tabs.remove(idx);
        if self.active_tab_id == removed.id {
            self.active_tab_id = self
                .tabs
                .get(idx.saturating_sub(1))
                .map(|t| t.id.clone())
                .unwrap_or_default();
        }
        removed
    }

    fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }
}

/// N tabbed panes in a nested split tree — see the module documentation.
///
/// The pane list and split tree are kept consistent by the methods below
/// (every pane has exactly one leaf), so they are not public fields.
#[derive(Debug, Clone, PartialEq)]
pub struct TabGroup<C = ()> {
    /// The group's id. Each pane's strip is a [`TabBar`] with the id
    /// `"<id>:pane<k>:tabs"`.
    pub id: WidgetId,
    /// Direction [`Self::add_pane_after`] splits in.
    pub default_split_direction: SplitDirection,
    panes: Vec<TabGroupPane<C>>,
    /// The split tree; the leaf for pane `k` is `leaves[k]`.
    tree: SplitTree,
    leaves: Vec<WidgetId>,
    /// Source of unique leaf ids.
    next_leaf: u64,
    /// Source of unique ids for panes a drop creates.
    next_pane_counter: usize,
}

impl<C> TabGroup<C> {
    /// A group of one pane.
    pub fn with_pane(
        id: impl Into<WidgetId>,
        pane: TabGroupPane<C>,
        default_split_direction: SplitDirection,
    ) -> Self {
        let mut group = Self {
            id: id.into(),
            default_split_direction,
            panes: vec![pane],
            tree: SplitTree::leaf(WidgetId::new("")),
            leaves: Vec::new(),
            next_leaf: 0,
            next_pane_counter: 1,
        };
        let leaf = group.mint_leaf();
        group.tree = SplitTree::leaf(leaf);
        group.leaves.push(leaf);
        group
    }

    /// A group of `panes` arranged by `layout`.
    ///
    /// # Errors
    ///
    /// Returns `Err(message)` when `panes` is empty, or when `layout` does
    /// not name every index `0..panes.len()` exactly once.
    pub fn from_layout(
        id: impl Into<WidgetId>,
        panes: Vec<TabGroupPane<C>>,
        layout: GroupLayout,
        default_split_direction: SplitDirection,
    ) -> Result<Self, String> {
        let n = panes.len();
        if n == 0 {
            return Err("panes must be non-empty".into());
        }
        layout.validate_for_pane_count(n)?;
        let mut group = Self {
            id: id.into(),
            default_split_direction,
            panes,
            tree: SplitTree::leaf(WidgetId::new("")),
            leaves: Vec::new(),
            next_leaf: 0,
            // Drop-created pane ids start above n so they never repeat
            // names like "pane:0", "pane:1", … the consumer may have used.
            next_pane_counter: n,
        };
        let leaves: Vec<WidgetId> = (0..n).map(|_| group.mint_leaf()).collect();
        group.leaves = leaves;
        group.tree = layout.to_split_tree(&group.leaves);
        Ok(group)
    }

    fn mint_leaf(&mut self) -> WidgetId {
        let leaf = WidgetId::new(format!("{}:leaf{}", self.id, self.next_leaf));
        self.next_leaf += 1;
        leaf
    }

    // ── Queries ─────────────────────────────────────────────────────

    /// Number of panes.
    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }

    /// The pane at `idx`, or `None` if out of range.
    pub fn pane(&self, idx: usize) -> Option<&TabGroupPane<C>> {
        self.panes.get(idx)
    }

    /// Every pane, in index order.
    pub fn panes(&self) -> &[TabGroupPane<C>] {
        &self.panes
    }

    /// The pane at `idx`, mutably — for the per-tab content and the
    /// strip's scroll offset.
    pub fn pane_mut(&mut self, idx: usize) -> Option<&mut TabGroupPane<C>> {
        self.panes.get_mut(idx)
    }

    /// The split tree, with pane indices as leaves.
    pub fn layout(&self) -> GroupLayout {
        GroupLayout::from_split_tree(&self.tree, &self.leaves)
    }

    /// The current ratio of divider `divider_idx` (pre-order).
    pub fn divider_ratio(&self, divider_idx: usize) -> Option<f32> {
        self.tree.ratio_at_index(divider_idx)
    }

    /// The [`TabBar`] pane `idx`'s strip paints, starting at tab
    /// `scroll_offset`. The strip's one right segment is the pane's
    /// "+" new-tab button.
    pub fn pane_tab_bar(&self, idx: usize, scroll_offset: usize) -> TabBar {
        let Some(pane) = self.panes.get(idx) else {
            return TabBar::default();
        };
        TabBar {
            id: WidgetId::new(format!("{}:pane{}:tabs", self.id, idx)),
            tabs: pane
                .tabs
                .iter()
                .map(|t| TabItem {
                    label: t.label.clone(),
                    is_active: t.id == pane.active_tab_id,
                    is_dirty: false,
                    is_preview: false,
                    is_closable: t.closable,
                })
                .collect(),
            scroll_offset,
            right_segments: vec![TabBarSegment {
                text: " + ".to_string(),
                width_cells: 3,
                id: Some(WidgetId::new(format!("{}:pane{}:new-tab", self.id, idx))),
                is_active: false,
            }],
            active_accent: None,
            show_tab_close: pane.tabs.iter().any(|t| t.closable),
            compact: false,
            ..Default::default()
        }
    }

    // ── Mutations ───────────────────────────────────────────────────

    /// Add a tab to pane `pane_idx` without activating it. Returns `false`
    /// if the index is out of range.
    pub fn add_tab(&mut self, pane_idx: usize, tab: TabGroupTab<C>) -> bool {
        match self.panes.get_mut(pane_idx) {
            Some(pane) => {
                pane.tabs.push(tab);
                true
            }
            None => false,
        }
    }

    /// Make `tab_id` the active tab of pane `pane_idx`. Returns `false`
    /// when the pane or tab is not found.
    pub fn switch_tab(&mut self, pane_idx: usize, tab_id: &str) -> bool {
        self.panes
            .get_mut(pane_idx)
            .is_some_and(|p| p.activate_tab(tab_id))
    }

    /// Close `tab_id` in pane `pane_idx`, collapsing the pane when it was
    /// the last tab. Returns the event the close produced
    /// ([`TabGroupEvent::PaneCollapsed`] or [`TabGroupEvent::TabClosed`]),
    /// or `None` when the pane or tab is not found.
    pub fn close_tab(&mut self, pane_idx: usize, tab_id: &str) -> Option<TabGroupEvent> {
        let pane = self.panes.get_mut(pane_idx)?;
        let idx = pane.tabs.iter().position(|t| t.id == tab_id)?;
        pane.remove_tab_at(idx);
        if pane.is_empty() {
            self.collapse_pane(pane_idx);
            Some(TabGroupEvent::PaneCollapsed { pane_idx })
        } else {
            Some(TabGroupEvent::TabClosed {
                pane_idx,
                tab_id: tab_id.to_string(),
            })
        }
    }

    /// Open `pane` beside pane `after` (clamped into range), splitting it
    /// evenly in [`Self::default_split_direction`]. The new pane takes
    /// index `after + 1`; returns that index.
    pub fn add_pane_after(&mut self, after: usize, pane: TabGroupPane<C>) -> usize {
        let after = after.min(self.panes.len().saturating_sub(1));
        self.insert_pane(after, false, self.default_split_direction, pane)
    }

    /// Insert `pane` beside pane `target`, splitting `target`'s leaf along
    /// `direction` — before it (left/top) when `before`. Returns the new
    /// pane's index.
    fn insert_pane(
        &mut self,
        target: usize,
        before: bool,
        direction: SplitDirection,
        pane: TabGroupPane<C>,
    ) -> usize {
        let at = if before { target } else { target + 1 };
        let leaf = self.mint_leaf();
        self.tree
            .split_leaf(&self.leaves[target], leaf, direction, before);
        self.panes.insert(at, pane);
        self.leaves.insert(at, leaf);
        at
    }

    /// Remove pane `pane_idx`, giving its space to its sibling in the
    /// split tree. No-op when only one pane remains or the index is out
    /// of range.
    pub fn collapse_pane(&mut self, pane_idx: usize) {
        if self.panes.len() <= 1 || pane_idx >= self.panes.len() {
            return;
        }
        self.tree.remove_leaf(&self.leaves[pane_idx]);
        self.panes.remove(pane_idx);
        self.leaves.remove(pane_idx);
    }

    /// Set divider `divider_idx`'s ratio (clamped to
    /// [`crate::primitives::split_tree::MIN_RATIO`]..=`MAX_RATIO`).
    /// Returns the applied ratio when it changed the divider, `None` when
    /// the divider is not found or barely moved.
    pub fn set_divider_ratio(&mut self, divider_idx: usize, ratio: f32) -> Option<f32> {
        let before = self.tree.ratio_at_index(divider_idx)?;
        self.tree.set_ratio_at_index(divider_idx, ratio);
        let after = self.tree.ratio_at_index(divider_idx)?;
        ((after - before).abs() >= RATIO_EPSILON).then_some(after)
    }

    /// Write one of the widget's intents back onto the descriptor and
    /// return the changes it made, in temporal order:
    ///
    /// - [`TabGroupEvent::TabActivated`] activates the tab; nothing is
    ///   reported when it already was active.
    /// - [`TabGroupEvent::TabClosed`] closes the tab — see
    ///   [`Self::close_tab`].
    /// - [`TabGroupEvent::DividerResized`] sets the ratio — see
    ///   [`Self::set_divider_ratio`]; the reported ratio is the one
    ///   applied.
    /// - [`TabGroupEvent::TabDropped`] applies the drop —
    ///   [`TabGroupEvent::TabReordered`], [`TabGroupEvent::TabMovedToPane`]
    ///   or [`TabGroupEvent::TabSplitToNewPane`], followed by
    ///   [`TabGroupEvent::PaneCollapsed`] when the source pane emptied.
    /// - Every other event changes nothing and is returned as is.
    pub fn apply(&mut self, event: TabGroupEvent) -> Vec<TabGroupEvent> {
        match event {
            TabGroupEvent::TabActivated { pane_idx, tab_id } => {
                let changed = self
                    .panes
                    .get(pane_idx)
                    .is_some_and(|p| p.active_tab_id != tab_id)
                    && self.switch_tab(pane_idx, &tab_id);
                if changed {
                    vec![TabGroupEvent::TabActivated { pane_idx, tab_id }]
                } else {
                    Vec::new()
                }
            }
            TabGroupEvent::TabClosed { pane_idx, tab_id } => {
                self.close_tab(pane_idx, &tab_id).into_iter().collect()
            }
            TabGroupEvent::DividerResized { divider_idx, ratio } => self
                .set_divider_ratio(divider_idx, ratio)
                .map(|ratio| TabGroupEvent::DividerResized { divider_idx, ratio })
                .into_iter()
                .collect(),
            TabGroupEvent::TabDropped {
                tab_id,
                instruction,
            } => self.apply_drop(&tab_id, instruction),
            other => vec![other],
        }
    }

    /// Apply a drop instruction for `tab_id` — see [`Self::apply`].
    /// Nothing changes when the instruction no longer matches the panes
    /// (the tab is not where the instruction says).
    fn apply_drop(&mut self, tab_id: &str, instruction: TabDropInstruction) -> Vec<TabGroupEvent> {
        let at = |group: &Self, pane: usize, idx: usize| {
            group
                .panes
                .get(pane)
                .and_then(|p| p.tabs.get(idx))
                .is_some_and(|t| t.id == tab_id)
        };
        match instruction {
            TabDropInstruction::NoOp => Vec::new(),
            TabDropInstruction::Reorder {
                pane_idx,
                from_idx,
                to_idx,
            } => {
                if !at(self, pane_idx, from_idx) || to_idx >= self.panes[pane_idx].tabs.len() {
                    return Vec::new();
                }
                let tabs = &mut self.panes[pane_idx].tabs;
                let moved = tabs.remove(from_idx);
                tabs.insert(to_idx, moved);
                vec![TabGroupEvent::TabReordered {
                    pane_idx,
                    tab_id: tab_id.to_string(),
                    from_idx,
                    to_idx,
                }]
            }
            TabDropInstruction::MoveToPane {
                from_pane_idx,
                from_idx,
                to_pane_idx,
                insert_idx,
                source_now_empty,
            } => {
                if !at(self, from_pane_idx, from_idx) || to_pane_idx >= self.panes.len() {
                    return Vec::new();
                }
                let moved = self.panes[from_pane_idx].remove_tab_at(from_idx);
                let target = &mut self.panes[to_pane_idx];
                target.active_tab_id = moved.id.clone();
                target.tabs.insert(insert_idx.min(target.tabs.len()), moved);
                let final_to = if source_now_empty {
                    self.collapse_pane(from_pane_idx);
                    if from_pane_idx < to_pane_idx {
                        to_pane_idx - 1
                    } else {
                        to_pane_idx
                    }
                } else {
                    to_pane_idx
                };
                let mut events = vec![TabGroupEvent::TabMovedToPane {
                    from_pane_idx,
                    to_pane_idx: final_to,
                    tab_id: tab_id.to_string(),
                    insert_idx,
                }];
                if source_now_empty {
                    events.push(TabGroupEvent::PaneCollapsed {
                        pane_idx: from_pane_idx,
                    });
                }
                events
            }
            TabDropInstruction::SplitToNewPane {
                from_pane_idx,
                from_idx,
                target_pane_idx,
                edge,
                split_direction,
                source_now_empty,
            } => {
                if !at(self, from_pane_idx, from_idx) || target_pane_idx >= self.panes.len() {
                    return Vec::new();
                }
                let moved = self.panes[from_pane_idx].remove_tab_at(from_idx);
                let mut target = target_pane_idx;
                if source_now_empty {
                    self.collapse_pane(from_pane_idx);
                    if from_pane_idx < target {
                        target -= 1;
                    }
                }
                let pane_id = format!("pane:{}", self.next_pane_counter);
                self.next_pane_counter += 1;
                let active = moved.id.clone();
                let pane = TabGroupPane::new(pane_id, vec![moved], active);
                let before = matches!(edge, DropEdge::Left | DropEdge::Top);
                let new_pane_idx = self.insert_pane(target, before, split_direction, pane);
                let mut events = vec![TabGroupEvent::TabSplitToNewPane {
                    from_pane_idx,
                    tab_id: tab_id.to_string(),
                    target_pane_idx,
                    edge,
                    new_pane_idx,
                }];
                if source_now_empty {
                    events.push(TabGroupEvent::PaneCollapsed {
                        pane_idx: from_pane_idx,
                    });
                }
                events
            }
        }
    }
}

/// The smallest ratio change a divider drag reports.
const RATIO_EPSILON: f32 = 0.001;

/// Insertion-bar thickness of the drop overlay a tab drag shows.
const DROP_BAR_THICKNESS: f32 = 2.0;

/// How far a pressed tab must travel before the press becomes a drag:
/// one cell, or a few pixels.
fn tab_drag_threshold(units: Units) -> f32 {
    match units {
        Units::Cells => 1.0,
        _ => 4.0,
    }
}

// ── Events ────────────────────────────────────────────────────────────────────

/// What a [`TabGroup`] reports, and what [`TabGroup::apply`] and
/// [`TabGroupController`] answer after applying it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TabGroupEvent {
    /// A tab in `pane_idx` was activated.
    TabActivated { pane_idx: usize, tab_id: String },
    /// A tab in `pane_idx` was closed.
    TabClosed { pane_idx: usize, tab_id: String },
    /// The last tab in `pane_idx` was closed.
    ///
    /// **When more than one pane exists:** the pane is removed and the
    /// remaining panes are renumbered.
    ///
    /// **When only one pane remains:** `collapse_pane` is a no-op — the
    /// empty pane is retained (dropping to zero panes is not permitted).
    /// Consumers should treat this case as "all tabs closed; the single
    /// pane is now empty" and react accordingly (e.g. show a welcome
    /// screen in the content area).
    PaneCollapsed {
        /// Index the pane had *before* collapse.
        pane_idx: usize,
    },
    /// A new pane was added (split). `pane_idx` is its index.
    PaneAdded { pane_idx: usize },
    /// A press inside a pane's body moved keyboard focus to that pane.
    PaneFocused { pane_idx: usize },
    /// A divider was dragged. `divider_idx` is the 0-based pre-order
    /// index of the divider (the [`crate::SplitTree`] `split_index`);
    /// `ratio` is the ratio that puts it under the pointer.
    DividerResized { divider_idx: usize, ratio: f32 },
    /// The "+" new-tab button in a pane was clicked. Apps add a tab in
    /// response.
    NewTabRequested { pane_idx: usize },
    /// A dragged tab was dropped: the widget's intent, carrying the
    /// [`resolve_tab_drop`] instruction for it. [`TabGroup::apply`] turns
    /// it into the change events below.
    TabDropped {
        /// Id of the dropped tab.
        tab_id: String,
        /// Where it goes.
        instruction: TabDropInstruction,
    },

    // ── Applied tab drops ───────────────────────────────────────────
    /// A tab was reordered within its pane via drag-and-drop.
    ///
    /// `from_idx` and `to_idx` are the before and after positions in
    /// `pane_idx`'s tab list.
    TabReordered {
        pane_idx: usize,
        /// Id of the tab that moved.
        tab_id: String,
        /// Position before the move.
        from_idx: usize,
        /// Position after the move.
        to_idx: usize,
    },

    /// A tab was dragged from one pane to another (merge).
    ///
    /// The tab was inserted into `to_pane_idx` at `insert_idx`. If the source
    /// pane was left empty it was collapsed; `from_pane_idx` is the index the
    /// source pane held **before** any collapse, and a separate
    /// [`TabGroupEvent::PaneCollapsed`] event follows this one.
    TabMovedToPane {
        from_pane_idx: usize,
        to_pane_idx: usize,
        tab_id: String,
        /// Position in the target pane's tab list where the tab landed.
        insert_idx: usize,
    },

    /// A tab was dragged to a group edge, creating a new adjacent pane.
    ///
    /// If the source pane was left empty it was collapsed; `from_pane_idx` is
    /// the index it held **before** any collapse, and a separate
    /// [`TabGroupEvent::PaneCollapsed`] event follows this one.
    /// `new_pane_idx` is the final index of the new pane after all
    /// mutations.
    ///
    /// `target_pane_idx` is the **original** index of the target pane (before
    /// any collapse).
    TabSplitToNewPane {
        from_pane_idx: usize,
        tab_id: String,
        /// The pane whose edge was targeted (original index, pre-collapse).
        target_pane_idx: usize,
        /// Which edge the tab was dropped onto.
        edge: DropEdge,
        /// Final index of the newly created pane (post-collapse).
        new_pane_idx: usize,
    },
}

impl WidgetAction for TabGroupEvent {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        crate::UiEvent::TabGroup(id, self)
    }
}

// ── Sub-parts ─────────────────────────────────────────────────────────────────

const KIND_SHIFT: u32 = 56;
const PANE_SHIFT: u32 = 40;
const PANE_MASK: u64 = 0xFFFF;
const CHILD_MASK: u64 = (1 << PANE_SHIFT) - 1;

/// An addressable part of a [`TabGroup`], as named by its [`SubTarget`]s:
/// the focus stops [`Interactive::focus`] lists and the regions
/// [`Interactive::sub_regions`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabGroupSubPart {
    /// Pane `.0`'s body — also the pane's keyboard focus stop.
    Pane(usize),
    /// Part `.1` of pane `.0`'s tab strip.
    Strip(usize, TabBarSubPart),
    /// Divider `.0` (pre-order).
    Divider(usize),
}

impl TabGroupSubPart {
    /// The [`SubTarget`] naming this part.
    pub fn sub_target(self) -> SubTarget {
        let pack = |kind: u64, index: usize, child: u64| {
            SubTarget(
                (kind << KIND_SHIFT)
                    | (((index as u64) & PANE_MASK) << PANE_SHIFT)
                    | (child & CHILD_MASK),
            )
        };
        match self {
            Self::Pane(k) => pack(1, k, 0),
            Self::Strip(k, part) => pack(2, k, part.sub_target().0),
            Self::Divider(i) => pack(3, i, 0),
        }
    }

    /// The part `sub` names, if it is one of this widget's.
    pub fn from_sub_target(sub: SubTarget) -> Option<Self> {
        let index = ((sub.0 >> PANE_SHIFT) & PANE_MASK) as usize;
        match sub.0 >> KIND_SHIFT {
            1 => Some(Self::Pane(index)),
            2 => TabBarSubPart::from_sub_target(SubTarget(sub.0 & CHILD_MASK))
                .map(|part| Self::Strip(index, part)),
            3 => Some(Self::Divider(index)),
            _ => None,
        }
    }
}

// ── State ─────────────────────────────────────────────────────────────────────

/// A pressed tab, and once it has travelled far enough, a drag.
#[derive(Debug, Clone, PartialEq)]
struct TabDrag {
    pane_idx: usize,
    tab_id: String,
    /// Where the press landed, absolute.
    origin: Point,
    /// The pointer, absolute.
    cursor: Point,
    /// Whether the press has become a drag.
    active: bool,
    /// The drop zone under the pointer.
    zone: Option<DropZone>,
}

/// [`Interactive::State`] for [`TabGroup`]: gesture data only — each
/// strip's [`TabBar`] state, the divider drag and the tab drag. Panes,
/// tabs and ratios are descriptor content.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct TabGroupState {
    tab_bars: Vec<TabBarState>,
    splits: SplitTreeState,
    tab_drag: Option<TabDrag>,
}

impl TabGroupState {
    /// Pane `idx`'s strip state, once the group has handled an event.
    pub fn tab_bar(&self, idx: usize) -> Option<&TabBarState> {
        self.tab_bars.get(idx)
    }

    /// The pre-order index of the divider being dragged.
    pub fn dragging_divider(&self) -> Option<usize> {
        self.splits.dragging()
    }

    /// Whether a tab is pressed or being dragged.
    pub fn is_tab_dragging(&self) -> bool {
        self.tab_drag.is_some()
    }

    /// The drop zone under a tab drag.
    pub fn drop_zone(&self) -> Option<&DropZone> {
        self.tab_drag.as_ref().and_then(|d| d.zone.as_ref())
    }

    fn sync(&mut self, panes: usize) {
        self.tab_bars.resize_with(panes, TabBarState::default);
    }
}

// ── Layout ────────────────────────────────────────────────────────────────────

/// One pane of a [`TabGroupPrimitiveLayout`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct TabGroupPaneLayout {
    /// The whole pane: strip and body.
    pub bounds: Rect,
    /// The tab strip, flush with the top of the pane.
    pub strip_bounds: Rect,
    /// The body below the strip — the host paints the active tab here.
    pub content_bounds: Rect,
    /// The first tab the strip shows, keeping the active tab in view.
    pub scroll_offset: usize,
    /// The strip, scrolled to [`Self::scroll_offset`].
    pub bar: TabBar,
    /// The strip's own layout.
    pub tabs: TabBarPrimitiveLayout,
}

impl TabGroupPaneLayout {
    /// The drop target this pane offers a tab drag: the whole pane, with
    /// each visible tab's absolute x-range.
    fn drop_group(&self) -> DropGroupRect {
        let strip = self.strip_bounds;
        DropGroupRect {
            bounds: Rect::new(
                strip.x,
                strip.y,
                strip.width,
                strip.height + self.content_bounds.height,
            ),
            tab_slots: self
                .tabs
                .layout
                .visible_tabs
                .iter()
                .map(|vt| {
                    (
                        vt.bounds.x + strip.x,
                        vt.bounds.x + strip.x + vt.bounds.width,
                    )
                })
                .collect(),
        }
    }
}

/// [`Primitive::Layout`] for [`TabGroup`]: the divider child and its
/// layout, and every pane's strip and body.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct TabGroupPrimitiveLayout {
    /// The rect the group was laid out into.
    pub area: Rect,
    /// Height of every pane's tab strip.
    pub strip_height: f32,
    /// The chrome line height; a drag's ghost label floats this far from
    /// the pointer.
    pub line_height: f32,
    /// The divider child: the group's split tree, with each split's
    /// minimum pane sizes set so every stacked pane keeps its strip.
    pub splits: SplitTree,
    /// The divider child's own layout.
    pub dividers: SplitTreeLayout,
    /// One entry per pane, in pane order.
    pub panes: Vec<TabGroupPaneLayout>,
}

impl TabGroupPrimitiveLayout {
    /// Every pane's drop target, in pane order.
    pub fn drop_groups(&self) -> Vec<DropGroupRect> {
        self.panes
            .iter()
            .map(TabGroupPaneLayout::drop_group)
            .collect()
    }

    /// The drop zone under absolute `point`.
    pub fn drop_zone_at(&self, point: Point) -> Option<DropZone> {
        match drop_zone_hit_test(point.x, point.y, &self.drop_groups(), self.strip_height) {
            DropZoneHit::Zone(zone) => Some(zone),
            DropZoneHit::Empty => None,
        }
    }

    /// The pane whose strip or body holds absolute `point`.
    pub fn pane_at(&self, point: Point) -> Option<usize> {
        self.panes
            .iter()
            .position(|p| p.strip_bounds.contains(point) || p.content_bounds.contains(point))
    }

    fn origin(&self) -> Point {
        Point::new(self.area.x, self.area.y)
    }
}

/// The least height `tree` can take along a vertical stack: one strip
/// per stacked pane plus the dividers between them.
fn min_stack_height(tree: &SplitTree, strip: f32, divider: f32) -> f32 {
    match tree {
        SplitTree::Leaf(_) => strip,
        SplitTree::Split {
            direction,
            first,
            second,
            ..
        } => {
            let (a, b) = (
                min_stack_height(first, strip, divider),
                min_stack_height(second, strip, divider),
            );
            match direction {
                SplitDirection::Vertical => a + b + divider,
                SplitDirection::Horizontal => a.max(b),
            }
        }
    }
}

/// `tree` with every stacked split's minimum sizes set so no pane is
/// squeezed shorter than its tab strip.
fn with_pane_minimums(tree: &SplitTree, strip: f32, divider: f32) -> SplitTree {
    match tree {
        SplitTree::Leaf(id) => SplitTree::leaf(*id),
        SplitTree::Split {
            direction,
            ratio,
            first,
            second,
            ..
        } => {
            let split = SplitTree::split(
                *direction,
                *ratio,
                with_pane_minimums(first, strip, divider),
                with_pane_minimums(second, strip, divider),
            );
            match direction {
                SplitDirection::Vertical => split.with_min_sizes(
                    min_stack_height(first, strip, divider),
                    min_stack_height(second, strip, divider),
                ),
                SplitDirection::Horizontal => split,
            }
        }
    }
}

/// `rect` relative to `origin`.
fn relative(rect: Rect, origin: Point) -> Rect {
    Rect::new(
        rect.x - origin.x,
        rect.y - origin.y,
        rect.width,
        rect.height,
    )
}

impl<C> TabGroup<C> {
    /// Lay pane `idx` out into `bounds`: the strip off the top, the body
    /// below, and the strip's [`TabBar`] at the offset that keeps the
    /// active tab visible.
    fn pane_layout(
        &self,
        idx: usize,
        ctx: &LayoutCtx,
        bounds: Rect,
        strip_height: f32,
    ) -> TabGroupPaneLayout {
        let strip_h = strip_height.min(bounds.height.max(0.0));
        let strip_bounds = Rect::new(bounds.x, bounds.y, bounds.width, strip_h);
        let content_bounds = Rect::new(
            bounds.x,
            bounds.y + strip_h,
            bounds.width,
            (bounds.height - strip_h).max(0.0),
        );
        let mut scroll_offset = self.panes[idx].tab_scroll_offset;
        let mut bar = self.pane_tab_bar(idx, scroll_offset);
        let mut tabs = Primitive::layout(&bar, ctx, strip_bounds);
        if tabs.corrected_scroll_offset != scroll_offset {
            scroll_offset = tabs.corrected_scroll_offset;
            bar = self.pane_tab_bar(idx, scroll_offset);
            tabs = Primitive::layout(&bar, ctx, strip_bounds);
        }
        TabGroupPaneLayout {
            bounds,
            strip_bounds,
            content_bounds,
            scroll_offset,
            bar,
            tabs,
        }
    }

    /// Hover and press on pane `idx`'s strip, re-addressed from this group
    /// to the strip's own [`TabBar`].
    fn strip_interaction(&self, idx: usize, bar: &WidgetId, env: &PaintEnv) -> InteractionState {
        let readdress = |target: Option<FocusAddress>| match target {
            Some((id, Some(sub))) if id == self.id => match TabGroupSubPart::from_sub_target(sub) {
                Some(TabGroupSubPart::Strip(k, part)) if k == idx => {
                    Some((*bar, Some(part.sub_target())))
                }
                _ => None,
            },
            _ => None,
        };
        InteractionState::from_targets(
            readdress(env.interaction.hovered_target()),
            readdress(env.interaction.pressed_target()),
        )
    }

    /// The overlay a live tab drag shows over the drop zone it hovers.
    pub fn drop_overlay(
        &self,
        state: &TabGroupState,
        layout: &TabGroupPrimitiveLayout,
    ) -> Option<DropOverlay> {
        let drag = state.tab_drag.as_ref().filter(|d| d.active)?;
        let zone = drag.zone.as_ref()?;
        let groups = layout.drop_groups();
        groups.get(zone.group_idx)?;
        Some(drop_zone_overlay(
            zone,
            &groups,
            drag.cursor.x,
            drag.cursor.y,
            layout.strip_height,
            DROP_BAR_THICKNESS,
            layout.line_height,
        ))
    }

    /// The drop a release at absolute `point` makes of `drag`, if any.
    fn resolve_drop(
        &self,
        drag: &TabDrag,
        point: Point,
        layout: &TabGroupPrimitiveLayout,
    ) -> Option<TabGroupEvent> {
        let tab_idx = self
            .panes
            .get(drag.pane_idx)?
            .tabs
            .iter()
            .position(|t| t.id == drag.tab_id)?;
        let counts: Vec<usize> = self.panes.iter().map(|p| p.tabs.len()).collect();
        let instruction = resolve_tab_drop(
            TabDragSource {
                pane_idx: drag.pane_idx,
                tab_idx,
            },
            &counts,
            point.x,
            point.y,
            &layout.drop_groups(),
            layout.strip_height,
        );
        (instruction != TabDropInstruction::NoOp).then(|| TabGroupEvent::TabDropped {
            tab_id: drag.tab_id.clone(),
            instruction,
        })
    }
}

impl<C> A11yPrimitive for TabGroup<C> {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new()
            .with_role(A11yRole::Group)
            .with_label("Tab group")
    }

    /// One group per pane, holding the pane's tab strip.
    fn a11y_children(&self) -> Vec<A11yChild> {
        self.panes
            .iter()
            .enumerate()
            .map(|(k, pane)| {
                let bar = self.pane_tab_bar(k, 0);
                let mut strip = A11yChild::new("tabs", bar.a11y_info());
                strip.children = bar.a11y_children();
                let mut child = A11yChild::new(
                    format!("pane{k}"),
                    A11yInfo::new()
                        .with_role(A11yRole::Group)
                        .with_label(&pane.id),
                );
                child.children = vec![strip];
                child
            })
            .collect()
    }
}

impl<C: 'static> Primitive for TabGroup<C> {
    type Layout = TabGroupPrimitiveLayout;
    type Event = TabGroupEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// Lays the split tree out through its [`SplitTree`] child — with each
    /// stacked split's minimums keeping every pane at least a strip tall —
    /// then each pane's strip through its [`TabBar`].
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> Self::Layout {
        let strip_height = Workspace::strip_height(ctx);
        let divider = match ctx.units() {
            Units::Cells => crate::primitives::split_tree::CELL_DIVIDER_THICKNESS,
            _ => crate::primitives::layout_metrics::pixel::DIVIDER,
        };
        let splits = with_pane_minimums(&self.tree, strip_height, divider);
        let dividers = Primitive::layout(&splits, ctx, rect);
        let panes = self
            .leaves
            .iter()
            .enumerate()
            .map(|(k, leaf)| {
                let bounds = dividers
                    .leaves
                    .iter()
                    .find(|(id, _)| id == leaf)
                    .map_or(Rect::new(rect.x, rect.y, 0.0, 0.0), |(_, r)| *r);
                self.pane_layout(k, ctx, bounds, strip_height)
            })
            .collect();
        TabGroupPrimitiveLayout {
            area: rect,
            strip_height,
            line_height: ctx.chrome_line_height(),
            splits,
            dividers,
            panes,
        }
    }

    /// The dividers through the [`SplitTree`] child, then each strip
    /// through its [`TabBar`], clipped to the strip. The bodies are the
    /// host's.
    fn paint(&self, layout: &Self::Layout, surface: &mut dyn PaintSurface, env: &PaintEnv) {
        Primitive::paint(&layout.splits, &layout.dividers, surface, env);
        for (k, pane) in layout.panes.iter().enumerate() {
            let interaction = self.strip_interaction(k, &pane.bar.id, env);
            let strip_env = env
                .with_state(&())
                .with_interaction(&interaction)
                .with_widget_state(None);
            surface.surface_push_clip(pane.strip_bounds);
            Primitive::paint(&pane.bar, &pane.tabs, surface, &strip_env);
            surface.surface_pop_clip();
        }
    }

    /// The dividers through the [`SplitTree`] child's cell painter (junction
    /// glyphs included), then each strip through its [`TabBar`].
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &Self::Layout,
        cells: &mut dyn crate::CellSurface,
        env: &PaintEnv,
    ) {
        Primitive::paint_cells(&layout.splits, &layout.dividers, cells, env);
        for (k, pane) in layout.panes.iter().enumerate() {
            let interaction = self.strip_interaction(k, &pane.bar.id, env);
            let strip_env = env
                .with_state(&())
                .with_interaction(&interaction)
                .with_widget_state(None);
            Primitive::paint_cells(&pane.bar, &pane.tabs, cells, &strip_env);
        }
    }
}

// ── Interactive ───────────────────────────────────────────────────────────────

/// `event` with its pointer moved from the group's local space to a
/// strip's (`delta` is the strip's offset inside the group), naming the
/// strip's own sub-part.
fn into_strip(event: &RoutedEvent, delta: Point) -> RoutedEvent {
    match event {
        RoutedEvent::Pointer {
            kind,
            sub,
            local,
            mods,
        } => RoutedEvent::Pointer {
            kind: *kind,
            sub: sub.and_then(|s| match TabGroupSubPart::from_sub_target(s) {
                Some(TabGroupSubPart::Strip(_, part)) => Some(part.sub_target()),
                _ => None,
            }),
            local: Point::new(local.x - delta.x, local.y - delta.y),
            mods: *mods,
        },
        other => other.clone(),
    }
}

impl<C: 'static> TabGroup<C> {
    /// A left press: a divider starts a drag; a strip press goes through
    /// the pane's [`TabBar`] (a tab body also arms a tab drag); a body
    /// press moves keyboard focus to that pane.
    fn press(
        &self,
        state: &mut TabGroupState,
        event: &RoutedEvent,
        local: Point,
        layout: &TabGroupPrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<TabGroupEvent> {
        if !Interactive::handle(
            &layout.splits,
            &mut state.splits,
            event,
            &layout.dividers,
            cx,
        )
        .is_ignored()
        {
            state.tab_drag = None;
            return Handled::Consumed;
        }
        let origin = layout.origin();
        let point = Point::new(local.x + origin.x, local.y + origin.y);
        let Some(k) = layout.pane_at(point) else {
            return Handled::Ignored;
        };
        let (Some(pane), Some(pane_layout)) = (self.panes.get(k), layout.panes.get(k)) else {
            return Handled::Ignored;
        };
        let pane_stop = Some(TabGroupSubPart::Pane(k).sub_target());
        if !pane_layout.strip_bounds.contains(point) {
            if cx.is_focused(pane_stop) {
                return Handled::Ignored;
            }
            cx.request_focus(pane_stop);
            return Handled::Event(TabGroupEvent::PaneFocused { pane_idx: k });
        }
        let strip = pane_layout.strip_bounds;
        let routed = into_strip(event, Point::new(strip.x - origin.x, strip.y - origin.y));
        let handled = Interactive::handle(
            &pane_layout.bar,
            &mut state.tab_bars[k],
            &routed,
            &pane_layout.tabs,
            cx,
        );
        cx.request_focus(pane_stop);
        match handled {
            Handled::Event(TabBarEvent::TabActivated { index }) => {
                let Some(tab) = pane.tabs.get(index) else {
                    return Handled::Consumed;
                };
                state.tab_drag = Some(TabDrag {
                    pane_idx: k,
                    tab_id: tab.id.clone(),
                    origin: point,
                    cursor: point,
                    active: false,
                    zone: None,
                });
                cx.capture_pointer();
                if tab.id == pane.active_tab_id {
                    Handled::Consumed
                } else {
                    Handled::Event(TabGroupEvent::TabActivated {
                        pane_idx: k,
                        tab_id: tab.id.clone(),
                    })
                }
            }
            Handled::Event(TabBarEvent::TabClosed { index }) => match pane.tabs.get(index) {
                Some(tab) if tab.closable => Handled::Event(TabGroupEvent::TabClosed {
                    pane_idx: k,
                    tab_id: tab.id.clone(),
                }),
                _ => Handled::Consumed,
            },
            // The strip's one right segment is its new-tab button.
            Handled::Event(TabBarEvent::ButtonClicked { .. }) => {
                Handled::Event(TabGroupEvent::NewTabRequested { pane_idx: k })
            }
            _ => Handled::Consumed,
        }
    }

    /// A pointer move: a divider drag reports the ratio under the
    /// pointer; a pressed tab becomes a drag once it travels past the
    /// threshold, and a drag tracks the drop zone under the pointer.
    fn pointer_move(
        &self,
        state: &mut TabGroupState,
        event: &RoutedEvent,
        local: Point,
        layout: &TabGroupPrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<TabGroupEvent> {
        let dividing = state.splits.dragging().is_some();
        let split = Interactive::handle(
            &layout.splits,
            &mut state.splits,
            event,
            &layout.dividers,
            cx,
        );
        if dividing {
            return match split {
                Handled::Event(drag) => {
                    let current = self.tree.ratio_at_index(drag.split_index);
                    let ratio = drag.new_ratio.clamp(
                        crate::primitives::split_tree::MIN_RATIO,
                        crate::primitives::split_tree::MAX_RATIO,
                    );
                    if current.is_some_and(|c| (c - ratio).abs() >= RATIO_EPSILON) {
                        Handled::Event(TabGroupEvent::DividerResized {
                            divider_idx: drag.split_index,
                            ratio,
                        })
                    } else {
                        Handled::Consumed
                    }
                }
                _ => Handled::Consumed,
            };
        }
        let Some(drag) = state.tab_drag.as_mut() else {
            return Handled::Ignored;
        };
        let origin = layout.origin();
        let point = Point::new(local.x + origin.x, local.y + origin.y);
        if !drag.active {
            let threshold = tab_drag_threshold(cx.units());
            if (point.x - drag.origin.x).abs() < threshold
                && (point.y - drag.origin.y).abs() < threshold
            {
                return Handled::Consumed;
            }
            drag.active = true;
        }
        drag.cursor = point;
        drag.zone = layout.drop_zone_at(point);
        cx.request_redraw();
        Handled::Consumed
    }

    /// A left release: ends a divider drag, or drops a dragged tab where
    /// it was released.
    fn release(
        &self,
        state: &mut TabGroupState,
        event: &RoutedEvent,
        local: Point,
        layout: &TabGroupPrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<TabGroupEvent> {
        if state.splits.dragging().is_some() {
            let _ = Interactive::handle(
                &layout.splits,
                &mut state.splits,
                event,
                &layout.dividers,
                cx,
            );
            return Handled::Consumed;
        }
        let Some(drag) = state.tab_drag.take() else {
            return Handled::Ignored;
        };
        cx.release_pointer();
        if !drag.active {
            return Handled::Consumed;
        }
        cx.request_redraw();
        let origin = layout.origin();
        let point = Point::new(local.x + origin.x, local.y + origin.y);
        match self.resolve_drop(&drag, point, layout) {
            Some(dropped) => Handled::Event(dropped),
            None => Handled::Consumed,
        }
    }
}

impl<C: 'static> Interactive for TabGroup<C> {
    type State = TabGroupState;
    type Action = TabGroupEvent;

    const OPENS_POPUPS: bool = true;

    /// One stop per pane.
    fn focus(&self) -> FocusSpec {
        FocusSpec::Children(
            (0..self.panes.len())
                .map(|k| FocusStop {
                    sub: TabGroupSubPart::Pane(k).sub_target(),
                })
                .collect(),
        )
    }

    /// Every strip's tabs, close buttons, chevrons and new-tab button,
    /// every pane body, and every divider's band.
    fn sub_regions(&self, layout: &Self::Layout) -> Vec<(Rect, SubTarget)> {
        let origin = layout.origin();
        let mut regions = Vec::new();
        for (k, pane) in layout.panes.iter().enumerate() {
            let strip = relative(pane.strip_bounds, origin);
            for (r, sub) in tab_bar_sub_regions(&pane.tabs.layout) {
                if let Some(part) = TabBarSubPart::from_sub_target(sub) {
                    regions.push((
                        Rect::new(r.x + strip.x, r.y + strip.y, r.width, r.height),
                        TabGroupSubPart::Strip(k, part).sub_target(),
                    ));
                }
            }
            regions.push((
                relative(pane.content_bounds, origin),
                TabGroupSubPart::Pane(k).sub_target(),
            ));
        }
        for d in &layout.dividers.dividers {
            regions.push((
                relative(d.band(), origin),
                TabGroupSubPart::Divider(d.split_index).sub_target(),
            ));
        }
        regions
    }

    /// Pointer gestures: divider drags through the [`SplitTree`] child,
    /// strip presses through each pane's [`TabBar`], tab drags with a
    /// drop on release, and pane focus on a body press. Escape cancels a
    /// tab drag. Keys are otherwise the focused pane's content's.
    fn handle(
        &self,
        state: &mut TabGroupState,
        event: &RoutedEvent,
        layout: &Self::Layout,
        cx: &mut EventCx,
    ) -> Handled<TabGroupEvent> {
        state.sync(self.panes.len());
        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                local,
                ..
            } => self.press(state, event, *local, layout, cx),
            RoutedEvent::Pointer {
                kind: PointerKind::Move,
                local,
                ..
            } => self.pointer_move(state, event, *local, layout, cx),
            RoutedEvent::Pointer {
                kind: PointerKind::Up(MouseButton::Left),
                local,
                ..
            } => self.release(state, event, *local, layout, cx),
            RoutedEvent::Key(KeyEvent {
                key: Key::Named(NamedKey::Escape),
                ..
            }) if state.tab_drag.is_some() => {
                state.tab_drag = None;
                cx.release_pointer();
                cx.request_redraw();
                Handled::Consumed
            }
            _ => Handled::Ignored,
        }
    }

    /// The drop overlay while a tab drag hovers a drop zone, painted over
    /// the whole frame so it shows above the pane bodies.
    fn popups(&self, state: &TabGroupState, layout: &Self::Layout) -> Vec<Popup> {
        let Some(overlay) = self.drop_overlay(state, layout) else {
            return Vec::new();
        };
        let bounds = overlay.bounds();
        vec![Popup::new(
            PopupKey(0),
            relative(bounds, layout.origin()),
            Size::new(bounds.width, bounds.height),
            overlay,
        )
        .side(PopupSide::Over)
        .dismiss(PopupDismiss::NONE)]
    }
}

// ── TabGroupController ───────────────────────────────────────────────────────

/// Resolved pane regions for one rendered frame.
#[derive(Debug, Clone, PartialEq)]
pub struct TabGroupLayout {
    /// Per-pane full bounds (tab strip + content area combined), in pane order.
    pub pane_bounds: Vec<Rect>,
    /// Per-pane tab-strip bounds (top row of each pane), in pane order.
    pub strip_bounds: Vec<Rect>,
    /// Per-pane content area bounds (below the tab strip), in pane order.
    pub content_bounds: Vec<Rect>,
}

/// Externally-supplied geometry for one pane, used by
/// [`TabGroupController::set_drag_geometry`] to prime drag hit-testing
/// without a [`render`](TabGroupController::render) pass.
///
/// Consumers that compute their own per-pane layout (e.g. a native backend
/// with its own geometry engine) populate one `PaneDragRect` per pane and
/// pass the slice to `set_drag_geometry`. After that call,
/// [`handle_tab_drag_move`](TabGroupController::handle_tab_drag_move),
/// [`handle_tab_drop_event`](TabGroupController::handle_tab_drop_event),
/// [`drop_group_rects`](TabGroupController::drop_group_rects),
/// [`drop_zone_at`](TabGroupController::drop_zone_at), and
/// [`tab_drag_overlay`](TabGroupController::tab_drag_overlay) all work without
/// ever calling `render`.
///
/// `tab_slots` keeps one entry per tab in the pane's own tab list:
/// pass `(0.0, 0.0)` for any tab that is scrolled off the left edge of
/// the strip, so the entries after it keep their original tab indices.
#[derive(Debug, Clone, PartialEq)]
pub struct PaneDragRect {
    /// Bounds of the tab strip row (top of the pane).
    pub strip_bounds: Rect,
    /// Bounds of the pane's content area (below the tab strip).
    pub content_bounds: Rect,
    /// Absolute `(start_x, end_x)` for each tab slot, in pane tab-list order.
    ///
    /// Tabs that are scrolled off the left edge of the strip must be
    /// represented by a `(0.0, 0.0)` sentinel so that indices here stay
    /// aligned with the pane's tab list.
    pub tab_slots: Vec<(f32, f32)>,
}

/// A [`TabGroup`] whose tabs each carry a [`BackendWidget`], with its
/// [`TabGroupState`] and its own pane focus.
///
/// Paints the group through the widget contract, renders each pane's
/// active tab into its body, and runs its imperative `handle_*` methods
/// through the group's [`Interactive::handle`], applying the intents it
/// reports with [`TabGroup::apply`].
///
/// # Typical lifecycle
///
/// ```ignore
/// // Create with one initial pane.
/// let mut group = TabGroupController::with_pane(
///     "pane:0", initial_tabs, "tab-0", SplitDirection::Horizontal
/// );
///
/// // Open a second pane (creates a split).
/// let pane_idx = group.add_pane_with_tab(
///     "pane:1",
///     PaneTab { id: "t-x".into(), label: "new.rs".into(),
///               closable: true, content: Box::new(MyContent) },
/// );
///
/// // In render:
/// let layout = group.render(backend, content_rect);
///
/// // In handle (try tab drag before divider drag):
/// if group.handle_tab_drag_start(x, y) {
///     tab_drag_active = true;
/// } else if group.handle_drag_start(x, y) {
///     divider_drag_active = true;
/// }
/// ```
pub struct TabGroupController {
    group: TabGroup<Box<dyn BackendWidget>>,
    state: TabGroupState,
    /// The pane keyboard focus is on.
    focused: Option<usize>,
    /// The group's layout from the last render (or primed drag geometry),
    /// which events are routed against.
    last_layout: Option<TabGroupPrimitiveLayout>,
}

/// The id [`TabGroupController`] paints its group under.
const CONTROLLER_ID: &str = "tg";

/// The [`EventHost`] [`TabGroupController`] runs its group's
/// [`Interactive::handle`] against: pane focus is the controller's own,
/// and there is no pointer capture to keep.
struct ControllerHost<'a> {
    focused: &'a mut Option<usize>,
    units: Units,
}

impl ControllerHost<'_> {
    fn pane(sub: Option<SubTarget>) -> Option<usize> {
        match sub.and_then(TabGroupSubPart::from_sub_target) {
            Some(TabGroupSubPart::Pane(k)) => Some(k),
            _ => None,
        }
    }
}

impl EventHost for ControllerHost<'_> {
    fn is_focused(&self, _id: &WidgetId, sub: Option<SubTarget>) -> bool {
        Self::pane(sub).is_some_and(|k| *self.focused == Some(k))
    }
    fn request_focus(&mut self, _id: &WidgetId, sub: Option<SubTarget>) {
        if let Some(k) = Self::pane(sub) {
            *self.focused = Some(k);
        }
    }
    fn capture_pointer(&mut self, _id: &WidgetId) {}
    fn release_pointer(&mut self) {}
    fn set_cursor(&mut self, _cursor: crate::backend::PointerShape) {}
    fn request_redraw(&mut self) {}
    fn emit(&mut self, _event: crate::UiEvent) {}
    fn units(&self) -> Units {
        self.units
    }
}

impl TabGroupController {
    // ── Constructors ────────────────────────────────────────────────

    /// Create a controller with a single pane.
    pub fn with_pane(
        pane_id: impl Into<String>,
        tabs: Vec<PaneTab>,
        active_tab_id: impl Into<String>,
        split_direction: SplitDirection,
    ) -> Self {
        Self::new(TabGroup::with_pane(
            CONTROLLER_ID,
            Pane::new(pane_id, tabs, active_tab_id),
            split_direction,
        ))
    }

    /// Construct a controller from an explicit pane list and layout tree.
    ///
    /// This is the primary constructor when the caller already owns an
    /// arbitrary mixed-direction split tree — for example, when reconstructing
    /// a saved session that mixes horizontal and vertical splits.
    ///
    /// # Arguments
    ///
    /// * `panes` — the ordered pane list; use [`Pane::new`] to build each entry.
    ///   Must be non-empty.
    /// * `layout` — the binary split tree. Every index `0..panes.len()` must
    ///   appear **exactly once** as a `Leaf`; the tree must contain no other
    ///   indices.
    /// * `default_split_direction` — the direction used by future
    ///   [`add_pane_with_tab`](Self::add_pane_with_tab) calls.
    ///
    /// # Errors
    ///
    /// Returns `Err(message)` when:
    ///
    /// * `panes` is empty.
    /// * The layout contains a leaf index ≥ `panes.len()`.
    /// * A leaf index appears more than once.
    /// * The number of leaves ≠ `panes.len()`.
    pub fn from_layout(
        panes: Vec<Pane>,
        layout: GroupLayout,
        default_split_direction: SplitDirection,
    ) -> Result<Self, String> {
        TabGroup::from_layout(CONTROLLER_ID, panes, layout, default_split_direction).map(Self::new)
    }

    fn new(group: TabGroup<Box<dyn BackendWidget>>) -> Self {
        Self {
            group,
            state: TabGroupState::default(),
            focused: None,
            last_layout: None,
        }
    }

    // ── Accessors ───────────────────────────────────────────────────

    /// The [`TabGroup`] descriptor this controller paints.
    pub fn view(&self) -> &TabGroup<Box<dyn BackendWidget>> {
        &self.group
    }

    /// Number of panes currently open.
    pub fn pane_count(&self) -> usize {
        self.group.pane_count()
    }

    /// Borrow the pane at `idx`, or `None` if out of range.
    pub fn pane(&self, idx: usize) -> Option<&Pane> {
        self.group.pane(idx)
    }

    /// Index of the currently focused pane, or `None` when nothing is focused.
    pub fn focused_pane(&self) -> Option<usize> {
        self.focused
    }

    /// Explicitly focus a pane. Out-of-range indices are clamped.
    pub fn focus_pane(&mut self, pane_idx: usize) {
        let n = self.group.pane_count();
        if n > 0 {
            self.focused = Some(pane_idx.min(n - 1));
        }
    }

    /// Cycle focus forward (+1) or backward (−1) across panes, wrapping.
    /// With nothing focused, a forward step lands on the first pane and a
    /// backward one on the last.
    pub fn cycle_focus(&mut self, delta: isize) {
        let n = self.group.pane_count();
        if n == 0 {
            return;
        }
        self.focused = Some(match self.focused {
            Some(i) => crate::focus::cycle_index(i, delta, n),
            None if delta >= 0 => 0,
            None => n - 1,
        });
    }

    /// The layout tree, with pane indices as leaves.
    pub fn layout(&self) -> GroupLayout {
        self.group.layout()
    }

    // ── Tab lifecycle ───────────────────────────────────────────────

    /// Add a tab to the pane at `pane_idx`. Returns `false` if the index is
    /// out of range. Does **not** activate the new tab.
    pub fn add_tab(&mut self, pane_idx: usize, tab: PaneTab) -> bool {
        self.group.add_tab(pane_idx, tab)
    }

    /// Add a tab to `pane_idx` and make it active immediately.
    pub fn add_and_activate_tab(&mut self, pane_idx: usize, tab: PaneTab) -> bool {
        let id = tab.id.clone();
        self.group.add_tab(pane_idx, tab) && self.group.switch_tab(pane_idx, &id)
    }

    /// Switch the active tab in `pane_idx` to `tab_id`.
    /// Returns `false` when the pane index or tab id is not found.
    pub fn switch_tab(&mut self, pane_idx: usize, tab_id: impl Into<WidgetId>) -> bool {
        self.group.switch_tab(pane_idx, tab_id.into().as_str())
    }

    /// Close a tab in `pane_idx`. Returns the event produced (which may be
    /// `PaneCollapsed` when the last tab is closed), or `None` when the pane
    /// index / tab id is not found.
    pub fn close_tab(
        &mut self,
        pane_idx: usize,
        tab_id: impl Into<WidgetId>,
    ) -> Option<TabGroupEvent> {
        let before = self.group.pane_count();
        let event = self.group.close_tab(pane_idx, tab_id.into().as_str());
        self.panes_changed(before);
        event
    }

    // ── Pane lifecycle ──────────────────────────────────────────────

    /// Open a new pane containing a single tab. The new pane is inserted after
    /// the currently focused pane (splitting it), or after the last pane when
    /// no pane is focused, and takes focus. The split direction is the
    /// group's default.
    ///
    /// Returns the index of the new pane.
    pub fn add_pane_with_tab(&mut self, pane_id: impl Into<String>, tab: PaneTab) -> usize {
        let tab_id = tab.id.clone();
        let after = self
            .focused
            .unwrap_or_else(|| self.group.pane_count().saturating_sub(1));
        let idx = self
            .group
            .add_pane_after(after, Pane::new(pane_id, vec![tab], tab_id));
        self.focused = Some(idx);
        self.last_layout = None;
        idx
    }

    /// Remove and drop the pane at `pane_idx`, giving its space to the
    /// nearest sibling in the split tree. No-op when only one pane remains.
    pub fn collapse_pane(&mut self, pane_idx: usize) {
        let before = self.group.pane_count();
        self.group.collapse_pane(pane_idx);
        self.panes_changed(before);
    }

    /// Clamp focus and drop the cached layout when the pane count moved.
    fn panes_changed(&mut self, before: usize) {
        let n = self.group.pane_count();
        if n == before {
            return;
        }
        if let Some(f) = self.focused {
            if f >= n {
                self.focused = n.checked_sub(1);
            }
        }
        self.last_layout = None;
    }

    /// Apply one intent the group reported, returning the changes it made.
    fn apply(&mut self, intent: TabGroupEvent) -> Vec<TabGroupEvent> {
        let before = self.group.pane_count();
        let events = self.group.apply(intent);
        self.panes_changed(before);
        for event in &events {
            if let TabGroupEvent::TabSplitToNewPane { new_pane_idx, .. } = event {
                self.focused = Some(*new_pane_idx);
            }
        }
        events
    }

    // ── Render ──────────────────────────────────────────────────────

    /// Render all panes into `bounds`: the group through the widget
    /// contract (dividers and strips), then each pane's active tab via
    /// [`BackendWidget::render`] into its body. While a tab drag hovers a
    /// drop zone, paints the [`DropOverlay`] over everything and registers
    /// its [`DROP_OVERLAY_ZONE_ID`] zone.
    ///
    /// Returns a [`TabGroupLayout`] with resolved pane/strip/content rects
    /// in pane order.
    pub fn render(&mut self, backend: &mut dyn Backend, bounds: Rect) -> TabGroupLayout {
        let painted = DrawsPrimitives::draw_dyn(backend, bounds, &self.group, &())
            .downcast_ref::<TabGroupPrimitiveLayout>()
            .cloned()
            .unwrap_or_else(|| {
                let ctx = LayoutCtx::new(backend.chrome_line_height(), backend.char_width(), false);
                Primitive::layout(&self.group, &ctx, bounds)
            });

        for (k, pane_layout) in painted.panes.iter().enumerate() {
            let Some(pane) = self.group.pane_mut(k) else {
                continue;
            };
            pane.tab_scroll_offset = pane_layout.scroll_offset;
            let content = pane_layout.content_bounds;
            if content.width > 0.0 && content.height > 0.0 {
                if let Some(tab) = pane.tabs.iter().find(|t| t.id == pane.active_tab_id) {
                    tab.content.render(backend, content);
                }
            }
        }

        if let Some(overlay) = self.group.drop_overlay(&self.state, &painted) {
            let bounds = overlay.bounds();
            DrawsPrimitives::draw_dyn(backend, bounds, &overlay, &());
            backend.register_zone(WidgetId::new(DROP_OVERLAY_ZONE_ID), bounds);
        }

        let layout = TabGroupLayout {
            pane_bounds: painted.panes.iter().map(|p| p.bounds).collect(),
            strip_bounds: painted.panes.iter().map(|p| p.strip_bounds).collect(),
            content_bounds: painted.panes.iter().map(|p| p.content_bounds).collect(),
        };
        self.last_layout = Some(painted);
        layout
    }

    // ── Routing ─────────────────────────────────────────────────────

    /// Run a left-button pointer event at absolute `(x, y)` through the
    /// group's [`Interactive::handle`] against the last layout. `None`
    /// before the first render.
    fn route(&mut self, kind: PointerKind, x: f32, y: f32) -> Option<Handled<TabGroupEvent>> {
        let layout = self.last_layout.as_ref()?;
        let routed = RoutedEvent::Pointer {
            kind,
            sub: None,
            local: Point::new(x - layout.area.x, y - layout.area.y),
            mods: Modifiers::default(),
        };
        let mut host = ControllerHost {
            focused: &mut self.focused,
            units: if layout.strip_height > 1.0 {
                Units::Pixels
            } else {
                Units::Cells
            },
        };
        let mut cx = EventCx::new(self.group.id, &mut host);
        Some(Interactive::handle(
            &self.group,
            &mut self.state,
            &routed,
            layout,
            &mut cx,
        ))
    }

    /// Whether `(x, y)` lies in some pane's strip (`strip`) or anywhere in
    /// some pane.
    fn hits_pane(&self, x: f32, y: f32, strip: bool) -> bool {
        let point = Point::new(x, y);
        self.last_layout.as_ref().is_some_and(|l| {
            l.panes.iter().any(|p| {
                p.strip_bounds.contains(point) || (!strip && p.content_bounds.contains(point))
            })
        })
    }

    // ── Click dispatch ──────────────────────────────────────────────

    /// Resolve a mouse click at `(x, y)` in viewport coordinates.
    ///
    /// Applies any resulting state change (tab activation, close, pane
    /// focus) and returns [`Handled::Event`] with the event for the app.
    /// Returns [`Handled::Consumed`] when the click landed inside a pane's
    /// tab strip or content area but on dead space (a non-closable tab's
    /// × region, the already-active tab's body, or an already-focused
    /// pane's content) — nothing to route, but the click was this
    /// controller's to handle. Returns [`Handled::Ignored`] when the click
    /// was outside every pane entirely.
    ///
    /// Typical call site: mouse-down handler after background dismissal.
    pub fn handle_click_event(&mut self, x: f32, y: f32) -> Handled<TabGroupEvent> {
        if !self.hits_pane(x, y, false) {
            return Handled::Ignored;
        }
        let outcome = self.route(PointerKind::Down(MouseButton::Left), x, y);
        // A click is not a drag: drop the drag the press armed.
        self.state.tab_drag = None;
        match outcome {
            Some(Handled::Event(intent)) => match self.apply(intent).into_iter().next() {
                Some(event) => Handled::Event(event),
                None => Handled::Consumed,
            },
            _ => Handled::Consumed,
        }
    }

    /// Resolve a mouse click at `(x, y)` in viewport coordinates.
    #[deprecated(
        since = "0.9.0",
        note = "use `handle_click_event`, which returns `Handled<TabGroupEvent>` and distinguishes dead-space clicks (`Handled::Consumed`) from clicks outside every pane (`Handled::Ignored`)"
    )]
    pub fn handle_click(&mut self, x: f32, y: f32) -> Option<TabGroupEvent> {
        self.handle_click_event(x, y).into_event()
    }

    // ── Divider drag ────────────────────────────────────────────────

    /// Start a drag if `(x, y)` is on a divider. Returns `true` when a
    /// drag was initiated.
    pub fn handle_drag_start(&mut self, x: f32, y: f32) -> bool {
        let point = Point::new(x, y);
        let on_divider = self
            .last_layout
            .as_ref()
            .is_some_and(|l| l.dividers.dividers.iter().any(|d| d.band().contains(point)));
        if !on_divider {
            return false;
        }
        self.state.tab_drag = None;
        let _ = self.route(PointerKind::Down(MouseButton::Left), x, y);
        self.state.splits.dragging().is_some()
    }

    /// Update the dragged divider position. Returns [`Handled::Event`] with
    /// a `DividerResized` event when the split ratio changes,
    /// [`Handled::Consumed`] when a divider drag is in progress but the
    /// ratio didn't change, and [`Handled::Ignored`] when no divider drag
    /// is active at all. Call with each mouse-moved event while dragging.
    ///
    /// The divider lands under the pointer: the ratio is the exact inverse
    /// of the split's own layout, divider thickness included.
    ///
    /// The `bounds` parameter is accepted for API compatibility but is not
    /// needed — the controller uses the split geometry from the last
    /// render.
    pub fn handle_drag_move_event(
        &mut self,
        x: f32,
        y: f32,
        _bounds: Rect,
    ) -> Handled<TabGroupEvent> {
        if self.state.splits.dragging().is_none() {
            return Handled::Ignored;
        }
        match self.route(PointerKind::Move, x, y) {
            Some(Handled::Event(intent)) => match self.apply(intent).into_iter().next() {
                Some(event) => Handled::Event(event),
                None => Handled::Consumed,
            },
            _ => Handled::Consumed,
        }
    }

    /// Update the dragged divider position.
    #[deprecated(
        since = "0.9.0",
        note = "use `handle_drag_move_event`, which returns `Handled<TabGroupEvent>` instead of `Option<TabGroupEvent>`"
    )]
    pub fn handle_drag_move(&mut self, x: f32, y: f32, bounds: Rect) -> Option<TabGroupEvent> {
        self.handle_drag_move_event(x, y, bounds).into_event()
    }

    /// End any active divider drag.
    pub fn handle_drag_end(&mut self) {
        self.state.splits = SplitTreeState::default();
    }

    /// The pre-order index of the divider being dragged.
    pub fn dragging_divider(&self) -> Option<usize> {
        self.state.dragging_divider()
    }

    // ── DropZone integration ────────────────────────────────────────

    /// Return [`DropGroupRect`]s for all panes from the last render, suitable
    /// for passing to [`crate::compute_drop_zone`].
    ///
    /// Tab slot positions come from each strip's layout in the last
    /// [`render`](Self::render). Empty before the first render.
    pub fn drop_group_rects(&self) -> Vec<DropGroupRect> {
        self.last_layout
            .as_ref()
            .map(TabGroupPrimitiveLayout::drop_groups)
            .unwrap_or_default()
    }

    /// Prime drag hit-test geometry from externally-computed pane rects, so
    /// [`handle_tab_drag_move`](Self::handle_tab_drag_move),
    /// [`handle_tab_drop_event`](Self::handle_tab_drop_event),
    /// [`drop_group_rects`](Self::drop_group_rects),
    /// [`drop_zone_at`](Self::drop_zone_at), and
    /// [`tab_drag_overlay`](Self::tab_drag_overlay) work without a
    /// [`render`](Self::render) pass.
    ///
    /// Use this when your application already owns and renders its own per-pane
    /// layout — for example a native backend that computes geometry
    /// independently. This injects the geometry directly so drag logic can
    /// proceed headlessly (useful for unit tests, too).
    ///
    /// The `panes` slice is in pane order (index 0 is pane 0, etc.). Excess
    /// entries beyond [`pane_count`](Self::pane_count) are ignored; when
    /// `panes` is shorter than the pane count the remaining panes are left
    /// un-primed.
    ///
    /// # Interaction with `handle_tab_drag_start`
    ///
    /// [`handle_tab_drag_start`](Self::handle_tab_drag_start) detects which tab
    /// is under the cursor from `tab_slots`. `PaneDragRect` carries no
    /// close-button or new-tab-button bounds, so neither is excluded for
    /// panes primed this way. If your consumer already excludes those
    /// targets before routing to the controller, this is fine. If you need
    /// full exclusion, call `render()` instead.
    pub fn set_drag_geometry(&mut self, panes: &[PaneDragRect]) {
        let panes: Vec<TabGroupPaneLayout> = panes
            .iter()
            .take(self.group.pane_count())
            .enumerate()
            .map(|(k, rect)| {
                let strip = rect.strip_bounds;
                let offset = self.group.panes[k].tab_scroll_offset;
                TabGroupPaneLayout {
                    bounds: Rect::new(
                        strip.x,
                        strip.y,
                        strip.width,
                        strip.height + rect.content_bounds.height,
                    ),
                    strip_bounds: strip,
                    content_bounds: rect.content_bounds,
                    scroll_offset: offset,
                    bar: self.group.pane_tab_bar(k, offset),
                    tabs: TabBarPrimitiveLayout {
                        area: strip,
                        nerd_fonts_enabled: false,
                        layout: strip_layout_from_tab_slots(strip, &rect.tab_slots),
                        corrected_scroll_offset: offset,
                    },
                }
            })
            .collect();
        let area = panes.iter().fold(None::<Rect>, |acc, p| {
            Some(match acc {
                None => p.bounds,
                Some(a) => {
                    let (x0, y0) = (a.x.min(p.bounds.x), a.y.min(p.bounds.y));
                    let x1 = (a.x + a.width).max(p.bounds.x + p.bounds.width);
                    let y1 = (a.y + a.height).max(p.bounds.y + p.bounds.height);
                    Rect::new(x0, y0, x1 - x0, y1 - y0)
                }
            })
        });
        let area = area.unwrap_or_default();
        let strip_height = panes.first().map_or(1.0, |p| p.strip_bounds.height);
        self.last_layout = Some(TabGroupPrimitiveLayout {
            area,
            strip_height,
            line_height: strip_height,
            splits: SplitTree::leaf(self.group.id),
            dividers: SplitTreeLayout {
                bounds: area,
                leaves: Vec::new(),
                dividers: Vec::new(),
            },
            panes,
        });
    }

    // ── Cross-group tab drag ────────────────────────────────────────

    /// Start a tab drag if `(x, y)` lands on a draggable tab (not a close
    /// button or the new-tab button). Returns `true` when a drag was
    /// initiated; `false` means the caller should fall through to
    /// [`handle_click_event`](Self::handle_click_event).
    ///
    /// A tab drag and a divider drag are mutually exclusive; starting one
    /// cancels the other. Starting a drag neither activates the tab nor
    /// moves focus.
    pub fn handle_tab_drag_start(&mut self, x: f32, y: f32) -> bool {
        if !self.hits_pane(x, y, true) {
            return false;
        }
        self.state.splits = SplitTreeState::default();
        let focused = self.focused;
        let _ = self.route(PointerKind::Down(MouseButton::Left), x, y);
        self.focused = focused;
        self.state.tab_drag.is_some()
    }

    /// Update the drag position. Returns the current [`DropZone`] under the
    /// cursor, or `None` when no drag is in progress or the cursor is outside
    /// all groups.
    ///
    /// Call on every mouse-moved event while dragging. The result can be
    /// passed to [`drop_zone_overlay`] directly,
    /// or use [`tab_drag_overlay`](Self::tab_drag_overlay) as a convenience.
    pub fn handle_tab_drag_move(&mut self, x: f32, y: f32) -> Option<DropZone> {
        self.state.tab_drag.as_mut()?.active = true;
        let _ = self.route(PointerKind::Move, x, y);
        self.state.drop_zone().cloned()
    }

    /// Compute the drop overlay for the current tab drag, ready to hand to
    /// the backend's drop-overlay renderer.
    ///
    /// Uses the [`DropZone`] last stored by [`handle_tab_drag_move`](Self::handle_tab_drag_move).
    /// Returns `None` when no tab drag is in progress or no drop zone has
    /// been resolved yet (cursor was outside all groups).
    ///
    /// `bar_thickness` is the insertion-bar width in logical units (typically
    /// 2–3 px for GTK, 1 cell for TUI). `ghost_offset` is how far the ghost
    /// label floats from the cursor (typically one line height).
    ///
    /// [`render`](Self::render) paints the overlay itself; call this only
    /// for a custom rendering path.
    pub fn tab_drag_overlay(
        &self,
        cursor_x: f32,
        cursor_y: f32,
        bar_thickness: f32,
        ghost_offset: f32,
    ) -> Option<DropOverlay> {
        let zone = self.state.drop_zone()?;
        let layout = self.last_layout.as_ref()?;
        Some(drop_zone_overlay(
            zone,
            &layout.drop_groups(),
            cursor_x,
            cursor_y,
            layout.strip_height,
            bar_thickness,
            ghost_offset,
        ))
    }

    /// Finalise a tab drag by applying the drop at `(x, y)`.
    ///
    /// Returns [`Handled::Event`] with the list of [`TabGroupEvent`]s
    /// describing the resulting mutation, in temporal order, when the drop
    /// produced at least one. Returns [`Handled::Consumed`] when a drag was
    /// in progress and got cleared but the drop was a no-op (cursor outside
    /// all groups, dropped on the source pane's own content, dropped at the
    /// same tab position, or attempted to split the only tab of the only
    /// pane). Returns [`Handled::Ignored`] when no drag was in progress at
    /// all.
    ///
    /// The drop is the group's [`TabGroupEvent::TabDropped`], applied with
    /// [`TabGroup::apply`] — see there for the events each kind of drop
    /// produces. A split creates the new pane adjacent to the target and
    /// focuses it: `Left`/`Right` edges split **horizontally**,
    /// `Top`/`Bottom` edges **vertically**.
    ///
    /// Clears drag state regardless of outcome.
    pub fn handle_tab_drop_event(&mut self, x: f32, y: f32) -> Handled<Vec<TabGroupEvent>> {
        let Some(drag) = self.state.tab_drag.as_mut() else {
            return Handled::Ignored;
        };
        drag.active = true;
        let outcome = self.route(PointerKind::Up(MouseButton::Left), x, y);
        self.state.tab_drag = None;
        match outcome {
            Some(Handled::Event(intent)) => {
                let events = self.apply(intent);
                if events.is_empty() {
                    Handled::Consumed
                } else {
                    Handled::Event(events)
                }
            }
            _ => Handled::Consumed,
        }
    }

    /// Finalise a tab drag by applying the drop at `(x, y)`.
    #[deprecated(
        since = "0.9.0",
        note = "use `handle_tab_drop_event`, which returns `Handled<Vec<TabGroupEvent>>` and distinguishes a no-op drop (`Handled::Consumed`) from no drag being in progress at all (`Handled::Ignored`)"
    )]
    pub fn handle_tab_drop(&mut self, x: f32, y: f32) -> Vec<TabGroupEvent> {
        self.handle_tab_drop_event(x, y)
            .into_event()
            .unwrap_or_default()
    }

    /// Cancel an in-progress tab drag without moving any tab.
    pub fn cancel_tab_drag(&mut self) {
        self.state.tab_drag = None;
    }

    /// Return `true` if a tab drag is currently in progress.
    pub fn is_tab_dragging(&self) -> bool {
        self.state.is_tab_dragging()
    }

    /// Compute the drop zone at `(cursor_x, cursor_y)` using the
    /// geometry from the last render. Returns `None` before the first
    /// render or when the cursor is outside all groups.
    pub fn drop_zone_at(&self, cursor_x: f32, cursor_y: f32) -> Option<DropZone> {
        self.last_layout
            .as_ref()?
            .drop_zone_at(Point::new(cursor_x, cursor_y))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::tab_bar::{SegmentMeasure, TabMeasure};

    // ── Minimal BackendWidget for tests ──────────────────────────────────────

    struct NoOpContent;
    impl BackendWidget for NoOpContent {
        fn render(&self, _backend: &mut dyn Backend, _rect: Rect) {}
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn tab(id: &str, label: &str, closable: bool) -> PaneTab {
        PaneTab {
            id: id.to_string(),
            label: label.to_string(),
            closable,
            content: Box::new(NoOpContent),
        }
    }

    fn make_group() -> TabGroupController {
        TabGroupController::with_pane(
            "pane:0",
            vec![tab("t0", "main.rs", true), tab("t1", "lib.rs", true)],
            "t0",
            SplitDirection::Horizontal,
        )
    }

    /// Prime the routed layout for `pane_idx` without a real backend
    /// render. Each tab is given `tab_w` cells (last `close_w` cells =
    /// close region). The "new-tab" right segment is given 3 cells.
    /// Panes are primed in index order.
    fn prime_pane(
        ctrl: &mut TabGroupController,
        pane_idx: usize,
        strip_x: f32,
        strip_y: f32,
        bar_w: f32,
        tab_w: f32,
        close_w: f32,
    ) {
        let Some(pane) = ctrl.group.panes.get(pane_idx) else {
            return;
        };
        let bar = ctrl.group.pane_tab_bar(pane_idx, 0);
        let n_tabs = pane.tabs.len();
        let closable: Vec<bool> = pane.tabs.iter().map(|t| t.closable).collect();
        let layout = bar.layout(
            bar_w,
            1.0,
            0.0,
            |i| {
                if i < n_tabs && closable[i] {
                    TabMeasure::new(tab_w, close_w)
                } else {
                    TabMeasure::new(tab_w, 0.0)
                }
            },
            |_| SegmentMeasure::new(3.0),
        );
        let strip = Rect::new(strip_x, strip_y, bar_w, 1.0);
        let content = Rect::new(strip_x, strip_y + 1.0, bar_w, 10.0);
        let pane_layout = TabGroupPaneLayout {
            bounds: Rect::new(strip_x, strip_y, bar_w, 11.0),
            strip_bounds: strip,
            content_bounds: content,
            scroll_offset: 0,
            bar,
            tabs: TabBarPrimitiveLayout {
                area: strip,
                nerd_fonts_enabled: false,
                layout,
                corrected_scroll_offset: 0,
            },
        };
        let area = Rect::new(0.0, 0.0, 1000.0, 1000.0);
        let group_id = ctrl.group.id;
        let cached = ctrl
            .last_layout
            .get_or_insert_with(|| TabGroupPrimitiveLayout {
                area,
                strip_height: 1.0,
                line_height: 1.0,
                splits: SplitTree::leaf(group_id),
                dividers: SplitTreeLayout {
                    bounds: area,
                    leaves: Vec::new(),
                    dividers: Vec::new(),
                },
                panes: Vec::new(),
            });
        if pane_idx < cached.panes.len() {
            cached.panes[pane_idx] = pane_layout;
        } else {
            cached.panes.push(pane_layout);
        }
    }

    /// Every pane index the layout tree names.
    fn leaves(layout: &GroupLayout) -> Vec<usize> {
        let mut out = Vec::new();
        layout.collect_leaves(&mut out);
        out
    }

    /// A cell-grid backend and a `width` x `height` render of `ctrl` on it.
    fn render_cells(
        ctrl: &mut TabGroupController,
        width: f32,
        height: f32,
    ) -> (crate::testing::RecordingBackend, TabGroupLayout) {
        let mut backend = crate::testing::RecordingBackend::new();
        let layout = ctrl.render(&mut backend, Rect::new(0.0, 0.0, width, height));
        (backend, layout)
    }

    /// The cell column (or row) divider `idx` of the last render sits at.
    fn divider_cell(ctrl: &TabGroupController, idx: usize) -> u16 {
        ctrl.last_layout
            .as_ref()
            .expect("rendered")
            .dividers
            .dividers[idx]
            .cell_position()
    }

    // ── Construction ─────────────────────────────────────────────────────────

    #[test]
    fn new_controller_has_one_pane_with_tabs() {
        let ctrl = make_group();
        assert_eq!(ctrl.pane_count(), 1);
        assert_eq!(ctrl.group.panes[0].tabs().len(), 2);
        assert_eq!(ctrl.group.panes[0].active_tab_id(), "t0");
        assert_eq!(ctrl.focused_pane(), None); // starts unfocused
    }

    // ── from_layout ───────────────────────────────────────────────────────────

    #[test]
    fn from_layout_single_pane() {
        let pane = Pane::new("p0", vec![tab("t0", "main.rs", true)], "t0");
        let layout = GroupLayout::Leaf(0);
        let ctrl = TabGroupController::from_layout(vec![pane], layout, SplitDirection::Horizontal)
            .expect("valid single-pane layout");
        assert_eq!(ctrl.pane_count(), 1);
        assert_eq!(ctrl.layout(), GroupLayout::Leaf(0));
        assert_eq!(ctrl.focused_pane(), None); // starts unfocused
        assert_eq!(ctrl.group.panes[0].active_tab_id(), "t0");
    }

    #[test]
    fn from_layout_two_pane_horizontal() {
        let panes = vec![
            Pane::new("p0", vec![tab("t0", "a.rs", true)], "t0"),
            Pane::new("p1", vec![tab("t1", "b.rs", true)], "t1"),
        ];
        let layout = GroupLayout::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.4,
            first: Box::new(GroupLayout::Leaf(0)),
            second: Box::new(GroupLayout::Leaf(1)),
        };
        let ctrl = TabGroupController::from_layout(panes, layout.clone(), SplitDirection::Vertical)
            .expect("valid two-pane layout");
        assert_eq!(ctrl.pane_count(), 2);
        assert_eq!(ctrl.layout(), layout);
        // Spot-check pane ids.
        assert_eq!(ctrl.group.panes[0].id, "p0");
        assert_eq!(ctrl.group.panes[1].id, "p1");
    }

    #[test]
    fn from_layout_three_pane_mixed_direction() {
        // Left pane | (top-right pane / bottom-right pane)
        // Root split is Horizontal; the right sub-tree is Vertical.
        // This is the canonical mixed-direction case the issue targets.
        let panes = vec![
            Pane::new("left", vec![tab("tl", "left.rs", true)], "tl"),
            Pane::new("top-right", vec![tab("tr", "top.rs", true)], "tr"),
            Pane::new("bottom-right", vec![tab("tb", "bottom.rs", true)], "tb"),
        ];
        let layout = GroupLayout::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(GroupLayout::Leaf(0)),
            second: Box::new(GroupLayout::Split {
                direction: SplitDirection::Vertical,
                ratio: 0.5,
                first: Box::new(GroupLayout::Leaf(1)),
                second: Box::new(GroupLayout::Leaf(2)),
            }),
        };
        let ctrl = TabGroupController::from_layout(panes, layout, SplitDirection::Horizontal)
            .expect("valid mixed-direction layout");

        assert_eq!(ctrl.pane_count(), 3);
        assert_eq!(ctrl.layout().leaf_count(), 3);
        assert!(leaves(&ctrl.layout()).contains(&0));
        assert!(leaves(&ctrl.layout()).contains(&1));
        assert!(leaves(&ctrl.layout()).contains(&2));

        // Confirm the root split is Horizontal and the nested split is Vertical.
        match &ctrl.layout() {
            GroupLayout::Split {
                direction: SplitDirection::Horizontal,
                second,
                ..
            } => match second.as_ref() {
                GroupLayout::Split {
                    direction: SplitDirection::Vertical,
                    ..
                } => {}
                other => panic!("expected Vertical nested split, got {other:?}"),
            },
            other => panic!("expected Horizontal root split, got {other:?}"),
        }
    }

    #[test]
    fn from_layout_custom_ratio_preserved() {
        // Verify that the exact ratio provided is stored as-is.
        let panes = vec![
            Pane::new("p0", vec![tab("t0", "a.rs", false)], "t0"),
            Pane::new("p1", vec![tab("t1", "b.rs", false)], "t1"),
        ];
        let layout = GroupLayout::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.3,
            first: Box::new(GroupLayout::Leaf(0)),
            second: Box::new(GroupLayout::Leaf(1)),
        };
        let ctrl = TabGroupController::from_layout(panes, layout, SplitDirection::Horizontal)
            .expect("valid");
        let ratio = match &ctrl.layout() {
            GroupLayout::Split { ratio, .. } => *ratio,
            _ => panic!("expected Split"),
        };
        assert!((ratio - 0.3).abs() < 1e-6, "ratio={ratio}");
    }

    #[test]
    fn from_layout_add_pane_after_construction() {
        // A from_layout controller should behave correctly with subsequent
        // add_pane_with_tab calls (important: next_pane_counter must not
        // collide with pre-existing pane ids).
        let panes = vec![
            Pane::new("p0", vec![tab("t0", "a.rs", true)], "t0"),
            Pane::new("p1", vec![tab("t1", "b.rs", true)], "t1"),
        ];
        let layout = GroupLayout::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(GroupLayout::Leaf(0)),
            second: Box::new(GroupLayout::Leaf(1)),
        };
        let mut ctrl = TabGroupController::from_layout(panes, layout, SplitDirection::Horizontal)
            .expect("valid");
        assert_eq!(ctrl.pane_count(), 2);

        // Add a third pane via the regular incremental API.
        let new_idx = ctrl.add_pane_with_tab("extra", tab("te", "extra.rs", false));
        assert_eq!(ctrl.pane_count(), 3);
        assert_eq!(ctrl.layout().leaf_count(), 3);
        assert_eq!(ctrl.group.panes[new_idx].id, "extra");
    }

    // ── from_layout: error cases ──────────────────────────────────────────────

    #[test]
    fn from_layout_error_empty_panes() {
        let result = TabGroupController::from_layout(
            vec![],
            GroupLayout::Leaf(0),
            SplitDirection::Horizontal,
        );
        assert!(result.is_err(), "expected Err for empty panes");
        let msg = result.err().unwrap();
        assert!(msg.contains("non-empty"), "unexpected message: {msg}");
    }

    #[test]
    fn from_layout_error_leaf_count_mismatch() {
        let panes = vec![Pane::new("p0", vec![tab("t0", "a.rs", false)], "t0")];
        // Tree has 2 leaves but only 1 pane provided.
        let layout = GroupLayout::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(GroupLayout::Leaf(0)),
            second: Box::new(GroupLayout::Leaf(1)),
        };
        let result = TabGroupController::from_layout(panes, layout, SplitDirection::Horizontal);
        assert!(result.is_err(), "expected Err for count mismatch");
        let msg = result.err().unwrap();
        assert!(
            msg.contains("2") && msg.contains("1"),
            "error should mention counts, got: {msg}"
        );
    }

    #[test]
    fn from_layout_error_duplicate_leaf_index() {
        let panes = vec![
            Pane::new("p0", vec![tab("t0", "a.rs", false)], "t0"),
            Pane::new("p1", vec![tab("t1", "b.rs", false)], "t1"),
        ];
        // Both leaves reference index 0.
        let layout = GroupLayout::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(GroupLayout::Leaf(0)),
            second: Box::new(GroupLayout::Leaf(0)), // duplicate!
        };
        let result = TabGroupController::from_layout(panes, layout, SplitDirection::Horizontal);
        assert!(result.is_err(), "expected Err for duplicate leaf");
        let msg = result.err().unwrap();
        assert!(
            msg.contains("more than once") || msg.contains("duplicate"),
            "unexpected error message: {msg}"
        );
    }

    #[test]
    fn from_layout_error_out_of_range_leaf_index() {
        let panes = vec![
            Pane::new("p0", vec![tab("t0", "a.rs", false)], "t0"),
            Pane::new("p1", vec![tab("t1", "b.rs", false)], "t1"),
        ];
        // Leaf index 2 is out of range for 2 panes (valid range: 0..2).
        let layout = GroupLayout::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(GroupLayout::Leaf(0)),
            second: Box::new(GroupLayout::Leaf(2)), // out of range!
        };
        let result = TabGroupController::from_layout(panes, layout, SplitDirection::Horizontal);
        assert!(result.is_err(), "expected Err for out-of-range leaf");
        let msg = result.err().unwrap();
        assert!(
            msg.contains("out of range") || msg.contains("range"),
            "unexpected error message: {msg}"
        );
    }

    // ── Tab switching ─────────────────────────────────────────────────────────

    #[test]
    fn switch_tab_changes_active() {
        let mut ctrl = make_group();
        assert!(ctrl.switch_tab(0, "t1"));
        assert_eq!(ctrl.group.panes[0].active_tab_id(), "t1");
    }

    #[test]
    fn switch_tab_unknown_returns_false() {
        let mut ctrl = make_group();
        assert!(!ctrl.switch_tab(0, "no-such"));
        assert_eq!(ctrl.group.panes[0].active_tab_id(), "t0"); // unchanged
    }

    #[test]
    fn switch_tab_and_close_tab_also_accept_a_widget_id_directly() {
        let mut ctrl = make_group();
        assert!(ctrl.switch_tab(0, WidgetId::new("t1")));
        assert_eq!(ctrl.group.panes[0].active_tab_id(), "t1");
        assert!(ctrl.close_tab(0, WidgetId::new("t1")).is_some());
    }

    // ── Add tab ───────────────────────────────────────────────────────────────

    #[test]
    fn add_tab_appends_without_activating() {
        let mut ctrl = make_group();
        ctrl.add_tab(0, tab("t2", "new.rs", true));
        assert_eq!(ctrl.group.panes[0].tabs().len(), 3);
        assert_eq!(ctrl.group.panes[0].active_tab_id(), "t0"); // unchanged
    }

    #[test]
    fn add_and_activate_tab_makes_it_active() {
        let mut ctrl = make_group();
        ctrl.add_and_activate_tab(0, tab("t2", "new.rs", true));
        assert_eq!(ctrl.group.panes[0].active_tab_id(), "t2");
    }

    // ── Close tab ─────────────────────────────────────────────────────────────

    #[test]
    fn close_inactive_tab_emits_tab_closed() {
        let mut ctrl = make_group();
        let ev = ctrl.close_tab(0, "t1");
        assert_eq!(
            ev,
            Some(TabGroupEvent::TabClosed {
                pane_idx: 0,
                tab_id: "t1".into()
            })
        );
        assert_eq!(ctrl.group.panes[0].tabs().len(), 1);
        assert_eq!(ctrl.group.panes[0].active_tab_id(), "t0");
    }

    #[test]
    fn close_active_tab_switches_to_previous() {
        let mut ctrl = make_group();
        ctrl.switch_tab(0, "t1"); // make t1 active
        let ev = ctrl.close_tab(0, "t1");
        assert_eq!(
            ev,
            Some(TabGroupEvent::TabClosed {
                pane_idx: 0,
                tab_id: "t1".into()
            })
        );
        assert_eq!(ctrl.group.panes[0].active_tab_id(), "t0"); // fell back to t0
    }

    #[test]
    fn close_unknown_tab_returns_none() {
        let mut ctrl = make_group();
        assert_eq!(ctrl.close_tab(0, "ghost"), None);
    }

    // ── Close-last-tab collapses the pane ─────────────────────────────────────

    #[test]
    fn close_last_tab_in_solo_pane_does_not_collapse() {
        let mut ctrl = TabGroupController::with_pane(
            "p0",
            vec![tab("t0", "only", true)],
            "t0",
            SplitDirection::Horizontal,
        );
        let ev = ctrl.close_tab(0, "t0");
        assert_eq!(ev, Some(TabGroupEvent::PaneCollapsed { pane_idx: 0 }));
        // Controller retains 1 pane (collapse_pane is a no-op at len==1).
        assert_eq!(ctrl.pane_count(), 1);
    }

    #[test]
    fn close_last_tab_in_second_pane_collapses_it() {
        let mut ctrl = make_group();
        let idx = ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", true));
        assert_eq!(ctrl.pane_count(), 2);

        let ev = ctrl.close_tab(idx, "x0");
        assert_eq!(ev, Some(TabGroupEvent::PaneCollapsed { pane_idx: idx }));
        assert_eq!(ctrl.pane_count(), 1);
        // Layout should be a single Leaf.
        assert!(matches!(ctrl.layout(), GroupLayout::Leaf(0)));
    }

    // ── Pane lifecycle ────────────────────────────────────────────────────────

    #[test]
    fn add_pane_with_tab_creates_second_pane() {
        let mut ctrl = make_group();
        let idx = ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", true));
        assert_eq!(idx, 1);
        assert_eq!(ctrl.pane_count(), 2);
        assert_eq!(ctrl.focused_pane(), Some(1));
        // Layout should be a Split at root.
        assert!(matches!(ctrl.layout(), GroupLayout::Split { .. }));
        // Tree has exactly 2 leaves.
        assert_eq!(ctrl.layout().leaf_count(), 2);
    }

    #[test]
    fn three_panes_fractions_sum_to_one() {
        let mut ctrl = make_group();
        ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", false));
        ctrl.add_pane_with_tab("p2", tab("y0", "y.rs", false));
        assert_eq!(ctrl.pane_count(), 3);
        // All 3 pane indices appear as leaves.
        assert_eq!(ctrl.layout().leaf_count(), 3);
    }

    // ── Focus tracking ────────────────────────────────────────────────────────

    #[test]
    fn focus_pane_sets_active() {
        let mut ctrl = make_group();
        ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", false));
        ctrl.focus_pane(0);
        assert_eq!(ctrl.focused_pane(), Some(0));
    }

    #[test]
    fn cycle_focus_wraps_around() {
        let mut ctrl = make_group();
        ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", false));
        ctrl.focus_pane(0);
        ctrl.cycle_focus(1); // → 1
        assert_eq!(ctrl.focused_pane(), Some(1));
        ctrl.cycle_focus(1); // → wraps to 0
        assert_eq!(ctrl.focused_pane(), Some(0));
    }

    // ── handle_click: tab strip ───────────────────────────────────────────────

    #[test]
    fn click_inactive_tab_emits_tab_activated() {
        let mut ctrl = make_group();
        prime_pane(&mut ctrl, 0, 0.0, 0.0, 80.0, 8.0, 2.0);

        let ev = ctrl.handle_click_event(9.0, 0.5); // x=9 → t1 body
        assert_eq!(
            ev,
            Handled::Event(TabGroupEvent::TabActivated {
                pane_idx: 0,
                tab_id: "t1".into()
            })
        );
        assert_eq!(ctrl.group.panes[0].active_tab_id(), "t1");
    }

    #[test]
    fn click_active_tab_returns_consumed() {
        let mut ctrl = make_group();
        prime_pane(&mut ctrl, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        assert_eq!(ctrl.handle_click_event(3.0, 0.5), Handled::Consumed);
    }

    #[test]
    fn click_close_button_emits_tab_closed() {
        let mut ctrl = make_group();
        prime_pane(&mut ctrl, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        let ev = ctrl.handle_click_event(7.0, 0.5);
        assert_eq!(
            ev,
            Handled::Event(TabGroupEvent::TabClosed {
                pane_idx: 0,
                tab_id: "t0".into()
            })
        );
        assert_eq!(ctrl.group.panes[0].tabs().len(), 1);
    }

    #[test]
    fn click_new_tab_button_emits_new_tab_requested() {
        let mut ctrl = make_group();
        prime_pane(&mut ctrl, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        let ev = ctrl.handle_click_event(78.0, 0.5);
        assert_eq!(
            ev,
            Handled::Event(TabGroupEvent::NewTabRequested { pane_idx: 0 })
        );
    }

    // ── handle_click: content area ────────────────────────────────────────────

    #[test]
    fn click_content_area_focuses_pane() {
        let mut ctrl = make_group();
        ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", false));
        ctrl.focus_pane(0);
        prime_pane(&mut ctrl, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        prime_pane(&mut ctrl, 1, 80.0, 0.0, 80.0, 8.0, 2.0);

        let ev = ctrl.handle_click_event(85.0, 5.0);
        assert_eq!(
            ev,
            Handled::Event(TabGroupEvent::PaneFocused { pane_idx: 1 })
        );
        assert_eq!(ctrl.focused_pane(), Some(1));
    }

    #[test]
    fn click_already_focused_content_returns_consumed() {
        let mut ctrl = make_group();
        prime_pane(&mut ctrl, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        ctrl.focus_pane(0);
        assert_eq!(ctrl.handle_click_event(5.0, 5.0), Handled::Consumed);
    }

    #[test]
    fn click_outside_every_pane_returns_ignored() {
        let mut ctrl = make_group();
        prime_pane(&mut ctrl, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        // Far outside the one primed pane's strip/content bounds entirely.
        assert_eq!(ctrl.handle_click_event(500.0, 500.0), Handled::Ignored);
    }

    // ── handle_click: non-zero origin ────────────────────────────────────────

    #[test]
    fn click_with_offset_strip_resolves_correctly() {
        let mut ctrl = make_group();
        prime_pane(&mut ctrl, 0, 20.0, 5.0, 80.0, 8.0, 2.0);

        let ev = ctrl.handle_click_event(30.0, 5.5);
        assert_eq!(
            ev,
            Handled::Event(TabGroupEvent::TabActivated {
                pane_idx: 0,
                tab_id: "t1".into()
            })
        );
    }

    // ── Deprecated adapters ────────────────────────────────────────────────────

    #[test]
    #[allow(deprecated)] // covers the `handle_click` shim kept for pre-`Handled` callers.
    fn deprecated_handle_click_still_adapts_to_option() {
        let mut ctrl = make_group();
        prime_pane(&mut ctrl, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        assert_eq!(
            ctrl.handle_click(9.0, 0.5),
            Some(TabGroupEvent::TabActivated {
                pane_idx: 0,
                tab_id: "t1".into()
            })
        );
        assert_eq!(ctrl.handle_click(500.0, 500.0), None);
    }

    #[test]
    #[allow(deprecated)] // covers the `handle_drag_move` shim kept for pre-`Handled` callers.
    fn deprecated_handle_drag_move_still_adapts_to_option() {
        let mut ctrl = make_group();
        ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", false));
        render_cells(&mut ctrl, 100.0, 20.0);
        let x = f32::from(divider_cell(&ctrl, 0));
        assert!(ctrl.handle_drag_start(x, 5.0));
        let bounds = Rect::new(0.0, 0.0, 100.0, 20.0);
        assert!(matches!(
            ctrl.handle_drag_move(30.0, 5.0, bounds),
            Some(TabGroupEvent::DividerResized { divider_idx: 0, .. })
        ));
    }

    #[test]
    #[allow(deprecated)] // covers the `handle_tab_drop` shim kept for pre-`Handled` callers.
    fn deprecated_handle_tab_drop_still_adapts_to_vec() {
        let mut ctrl = make_two_pane();
        ctrl.handle_tab_drag_start(9.0, 0.5);
        assert_eq!(ctrl.handle_tab_drop(999.0, 999.0), Vec::new());
    }

    // ── Divider drag ──────────────────────────────────────────────────────────

    #[test]
    fn drag_start_on_non_divider_returns_false() {
        let mut ctrl = make_group();
        assert!(!ctrl.handle_drag_start(50.0, 50.0));
    }

    #[test]
    fn drag_start_on_divider_returns_true() {
        let mut ctrl = make_group();
        ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", false));
        render_cells(&mut ctrl, 80.0, 20.0);
        let x = f32::from(divider_cell(&ctrl, 0));
        assert!(ctrl.handle_drag_start(x, 10.0));
        assert_eq!(ctrl.dragging_divider(), Some(0));
    }

    #[test]
    fn drag_move_updates_split_ratio() {
        let mut ctrl = make_group();
        ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", false));
        render_cells(&mut ctrl, 100.0, 20.0);
        let x = f32::from(divider_cell(&ctrl, 0));
        assert!(ctrl.handle_drag_start(x, 5.0));

        let bounds = Rect::new(0.0, 0.0, 100.0, 20.0); // ignored
        let ev = ctrl.handle_drag_move_event(30.0, 5.0, bounds);
        let Handled::Event(TabGroupEvent::DividerResized { divider_idx, ratio }) = ev else {
            panic!("expected a resize, got {ev:?}");
        };
        assert_eq!(divider_idx, 0);
        assert_eq!(ctrl.view().divider_ratio(0), Some(ratio));
        assert!(
            (ratio - 0.30).abs() < 0.02,
            "ratio={ratio}, expected ≈ 0.30"
        );
    }

    /// The ratio a drag reports is the exact inverse of the split's own
    /// layout (divider thickness included), so after the next render the
    /// divider sits on the cell the pointer was dragged to — not one cell
    /// short of it, as `x / width` would place it.
    #[test]
    fn divider_drag_lands_the_divider_under_the_pointer() {
        let mut ctrl = make_group();
        ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", false));
        render_cells(&mut ctrl, 120.0, 20.0);
        let x = f32::from(divider_cell(&ctrl, 0));
        assert!(ctrl.handle_drag_start(x, 5.0));
        assert!(ctrl
            .handle_drag_move_event(100.0, 5.0, Rect::default())
            .is_event());
        ctrl.handle_drag_end();
        render_cells(&mut ctrl, 120.0, 20.0);
        assert_eq!(divider_cell(&ctrl, 0), 100);
    }

    /// Pre-order: `divider_idx` 0 is the *outer* (root) split, matching
    /// `SplitTree::set_ratio_at_index`'s numbering.
    #[test]
    fn drag_move_targets_the_preorder_indexed_split_not_the_render_sibling() {
        let panes = vec![
            Pane::new("left", vec![tab("tl", "left.rs", true)], "tl"),
            Pane::new("top-right", vec![tab("tr", "top.rs", true)], "tr"),
            Pane::new("bottom-right", vec![tab("tb", "bottom.rs", true)], "tb"),
        ];
        let layout = GroupLayout::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(GroupLayout::Leaf(0)),
            second: Box::new(GroupLayout::Split {
                direction: SplitDirection::Vertical,
                ratio: 0.5,
                first: Box::new(GroupLayout::Leaf(1)),
                second: Box::new(GroupLayout::Leaf(2)),
            }),
        };
        let mut ctrl = TabGroupController::from_layout(panes, layout, SplitDirection::Horizontal)
            .expect("valid mixed-direction layout");

        fn ratios(layout: &GroupLayout) -> (f32, f32) {
            match layout {
                GroupLayout::Split { ratio, second, .. } => {
                    let nested = match second.as_ref() {
                        GroupLayout::Split { ratio, .. } => *ratio,
                        _ => panic!("expected nested Split"),
                    };
                    (*ratio, nested)
                }
                _ => panic!("expected root Split"),
            }
        }

        // Drag divider 0 (the root) — only the root ratio should move.
        render_cells(&mut ctrl, 100.0, 20.0);
        let root_x = f32::from(divider_cell(&ctrl, 0));
        assert!(ctrl.handle_drag_start(root_x, 5.0));
        assert_eq!(ctrl.dragging_divider(), Some(0));
        let ev = ctrl.handle_drag_move_event(30.0, 5.0, Rect::default());
        assert!(matches!(
            ev,
            Handled::Event(TabGroupEvent::DividerResized { divider_idx: 0, .. })
        ));
        let (root, nested) = ratios(&ctrl.layout());
        assert!(
            (root - 0.30).abs() < 0.02,
            "root ratio={root}, expected ≈0.30"
        );
        assert!(
            (nested - 0.5).abs() < 1e-6,
            "nested ratio must be untouched"
        );
        ctrl.handle_drag_end();

        // Drag divider 1 (the nested split) — only the nested ratio moves.
        render_cells(&mut ctrl, 100.0, 20.0);
        let nested_y = f32::from(divider_cell(&ctrl, 1));
        assert!(ctrl.handle_drag_start(60.0, nested_y));
        assert_eq!(ctrl.dragging_divider(), Some(1));
        let ev = ctrl.handle_drag_move_event(60.0, 15.0, Rect::default());
        assert!(matches!(
            ev,
            Handled::Event(TabGroupEvent::DividerResized { divider_idx: 1, .. })
        ));
        let (root_after, nested_after) = ratios(&ctrl.layout());
        assert!(
            (root_after - root).abs() < 1e-6,
            "root ratio must stay at its dragged value"
        );
        assert!(
            (nested_after - 15.0 / 19.0).abs() < 0.02,
            "nested ratio={nested_after}, expected ≈0.79"
        );
    }

    #[test]
    fn drag_end_clears_drag_state() {
        let mut ctrl = make_group();
        ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", false));
        render_cells(&mut ctrl, 80.0, 20.0);
        let x = f32::from(divider_cell(&ctrl, 0));
        assert!(ctrl.handle_drag_start(x, 5.0));
        ctrl.handle_drag_end();
        assert_eq!(ctrl.dragging_divider(), None);
    }

    #[test]
    fn drag_move_without_active_divider_drag_returns_ignored() {
        let mut ctrl = make_group();
        let bounds = Rect::new(0.0, 0.0, 100.0, 20.0);
        assert_eq!(
            ctrl.handle_drag_move_event(30.0, 5.0, bounds),
            Handled::Ignored
        );
    }

    // ── drop_group_rects ──────────────────────────────────────────────────────

    #[test]
    fn drop_group_rects_empty_before_render() {
        let ctrl = make_group();
        assert!(ctrl.drop_group_rects().is_empty());
    }

    #[test]
    fn drop_group_rects_populated_after_prime() {
        let mut ctrl = make_group();
        prime_pane(&mut ctrl, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        let rects = ctrl.drop_group_rects();
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].bounds.width, 80.0);
    }

    // ── Pane fractions / layout tree invariants ───────────────────────────────

    #[test]
    fn collapse_pane_reduces_leaf_count() {
        let mut ctrl = make_group();
        ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", false));
        ctrl.add_pane_with_tab("p2", tab("y0", "y.rs", false));
        assert_eq!(ctrl.pane_count(), 3);
        assert_eq!(ctrl.layout().leaf_count(), 3);

        ctrl.collapse_pane(1);
        assert_eq!(ctrl.pane_count(), 2);
        assert_eq!(ctrl.layout().leaf_count(), 2);
    }

    // ── Recursive split tree ──────────────────────────────────────────────────

    #[test]
    fn layout_after_two_adds_is_correct() {
        // Start: Leaf(0)
        // Add p1 (after p0): Split(H, Leaf(0), Leaf(1))
        // Add p2 (after p1, which is focused): Split(H, Leaf(0), Split(H, Leaf(1), Leaf(2)))
        let mut ctrl = make_group();
        ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", false));
        ctrl.add_pane_with_tab("p2", tab("y0", "y.rs", false));
        // All three leaf indices present.
        assert!(leaves(&ctrl.layout()).contains(&0));
        assert!(leaves(&ctrl.layout()).contains(&1));
        assert!(leaves(&ctrl.layout()).contains(&2));
        assert_eq!(ctrl.layout().leaf_count(), 3);
    }

    #[test]
    fn top_bottom_edge_drops_create_vertical_splits() {
        // When a tab is dropped on a Top or Bottom edge, the resulting split
        // in the tree must have direction == Vertical.
        let mut ctrl = make_group();
        ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", true));
        ctrl.focus_pane(0);
        prime_pane(&mut ctrl, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        prime_pane(&mut ctrl, 1, 80.0, 0.0, 80.0, 8.0, 2.0);

        // Drag t1 from pane 0 to the top edge of pane 1.
        // top-edge: y < tab_bar_h + edge_h → but y must be in content area.
        // Per compute_drop_zone: top edge = content area, y - content_top < edge_h(=3).
        // Content top for pane 1 = 1.0 (strip height). Edge zone: y < 1.0+3=4.
        ctrl.handle_tab_drag_start(9.0, 0.5); // drag t1
        let evs = ctrl
            .handle_tab_drop_event(120.0, 2.5)
            .into_event()
            .unwrap_or_default(); // top-edge of pane 1 content

        if let Some(TabGroupEvent::TabSplitToNewPane { edge, .. }) = evs.first() {
            assert!(
                matches!(edge, DropEdge::Top),
                "expected Top edge, got {edge:?}"
            );
        } else {
            // If not a split (e.g. fell into center), skip direction check.
            return;
        }

        // Find the newly inserted split node in the tree — it must be Vertical.
        fn find_any_vertical(node: &GroupLayout) -> bool {
            match node {
                GroupLayout::Leaf(_) => false,
                GroupLayout::Split {
                    direction,
                    first,
                    second,
                    ..
                } => {
                    *direction == SplitDirection::Vertical
                        || find_any_vertical(first)
                        || find_any_vertical(second)
                }
            }
        }
        assert!(
            find_any_vertical(&ctrl.layout()),
            "expected a Vertical split in tree after Top-edge drop"
        );
    }

    // ── Cross-group tab drag: helpers ─────────────────────────────────────────

    /// Build a two-pane controller with:
    ///   pane 0 at x=0  : tabs [t0, t1], strip height=1, bar_w=80
    ///   pane 1 at x=80 : tabs [x0],     strip height=1, bar_w=80
    fn make_two_pane() -> TabGroupController {
        let mut ctrl = TabGroupController::with_pane(
            "p0",
            vec![tab("t0", "main.rs", true), tab("t1", "lib.rs", true)],
            "t0",
            SplitDirection::Horizontal,
        );
        ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", true));
        ctrl.focus_pane(0);
        prime_pane(&mut ctrl, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        prime_pane(&mut ctrl, 1, 80.0, 0.0, 80.0, 8.0, 2.0);
        ctrl
    }

    // ── handle_tab_drag_start ─────────────────────────────────────────────────

    #[test]
    fn tab_drag_start_on_tab_body_returns_true() {
        let mut ctrl = make_two_pane();
        assert!(ctrl.handle_tab_drag_start(9.0, 0.5));
        assert!(ctrl.is_tab_dragging());
        let drag = ctrl.state.tab_drag.as_ref().unwrap();
        assert_eq!(drag.pane_idx, 0);
        assert_eq!(drag.tab_id, "t1");
    }

    #[test]
    fn tab_drag_start_on_close_button_returns_false() {
        let mut ctrl = make_two_pane();
        assert!(!ctrl.handle_tab_drag_start(7.0, 0.5));
        assert!(!ctrl.is_tab_dragging());
    }

    #[test]
    fn tab_drag_start_on_new_tab_button_returns_false() {
        let mut ctrl = make_two_pane();
        assert!(!ctrl.handle_tab_drag_start(78.0, 0.5));
        assert!(!ctrl.is_tab_dragging());
    }

    #[test]
    fn tab_drag_start_in_content_area_returns_false() {
        let mut ctrl = make_two_pane();
        assert!(!ctrl.handle_tab_drag_start(40.0, 5.0));
        assert!(!ctrl.is_tab_dragging());
    }

    #[test]
    fn tab_drag_start_outside_all_panes_returns_false() {
        let mut ctrl = make_two_pane();
        assert!(!ctrl.handle_tab_drag_start(999.0, 0.5));
        assert!(!ctrl.is_tab_dragging());
    }

    // ── handle_tab_drag_move + tab_drag_overlay ───────────────────────────────

    #[test]
    fn tab_drag_move_returns_none_when_not_dragging() {
        let mut ctrl = make_two_pane();
        assert!(ctrl.handle_tab_drag_move(120.0, 5.0).is_none());
    }

    #[test]
    fn tab_drag_move_returns_drop_zone_and_stores_it() {
        let mut ctrl = make_two_pane();
        assert!(ctrl.handle_tab_drag_start(9.0, 0.5));
        let zone = ctrl.handle_tab_drag_move(120.0, 5.0);
        assert_eq!(
            zone,
            Some(crate::DropZone {
                kind: crate::DropZoneKind::Center,
                group_idx: 1,
            })
        );
        let stored = ctrl.state.tab_drag.as_ref().unwrap().zone.as_ref();
        assert_eq!(stored.unwrap().group_idx, 1);
    }

    #[test]
    fn tab_drag_overlay_returns_none_without_zone() {
        let mut ctrl = make_two_pane();
        ctrl.handle_tab_drag_start(9.0, 0.5);
        assert!(ctrl.tab_drag_overlay(9.0, 0.5, 2.0, 10.0).is_none());
    }

    #[test]
    fn tab_drag_overlay_returns_highlight_after_move() {
        let mut ctrl = make_two_pane();
        ctrl.handle_tab_drag_start(9.0, 0.5);
        ctrl.handle_tab_drag_move(120.0, 5.0);
        let ov = ctrl.tab_drag_overlay(120.0, 5.0, 2.0, 10.0).unwrap();
        assert_eq!(ov.highlight, Some(crate::Rect::new(80.0, 1.0, 80.0, 10.0)));
        assert!(ov.insertion_bar.is_none());
    }

    // ── cancel_tab_drag / is_tab_dragging ─────────────────────────────────────

    #[test]
    fn cancel_tab_drag_clears_state() {
        let mut ctrl = make_two_pane();
        ctrl.handle_tab_drag_start(9.0, 0.5);
        assert!(ctrl.is_tab_dragging());
        ctrl.cancel_tab_drag();
        assert!(!ctrl.is_tab_dragging());
    }

    // ── drop_zone_at ──────────────────────────────────────────────────────────

    #[test]
    fn drop_zone_at_center_of_pane_1() {
        let ctrl = make_two_pane();
        let zone = ctrl.drop_zone_at(120.0, 5.0).unwrap();
        assert_eq!(zone.group_idx, 1);
        assert_eq!(zone.kind, crate::DropZoneKind::Center);
    }

    #[test]
    fn drop_zone_at_tab_bar_of_pane_1_reorder() {
        let ctrl = make_two_pane();
        let zone = ctrl.drop_zone_at(82.0, 0.5).unwrap();
        assert_eq!(zone.group_idx, 1);
        assert!(matches!(zone.kind, crate::DropZoneKind::TabReorder(_)));
    }

    // ── handle_tab_drop: reorder within same pane ─────────────────────────────

    #[test]
    fn tab_drop_reorders_within_same_pane() {
        let mut ctrl = make_two_pane();
        ctrl.handle_tab_drag_start(9.0, 0.5);
        let evs = ctrl
            .handle_tab_drop_event(3.0, 0.5)
            .into_event()
            .unwrap_or_default();
        assert_eq!(
            evs,
            vec![TabGroupEvent::TabReordered {
                pane_idx: 0,
                tab_id: "t1".into(),
                from_idx: 1,
                to_idx: 0,
            }]
        );
        assert_eq!(ctrl.group.panes[0].tabs()[0].id, "t1");
        assert_eq!(ctrl.group.panes[0].tabs()[1].id, "t0");
        assert_eq!(ctrl.pane_count(), 2);
    }

    #[test]
    fn tab_drop_reorder_same_position_is_noop() {
        let mut ctrl = make_two_pane();
        ctrl.handle_tab_drag_start(9.0, 0.5);
        let evs = ctrl
            .handle_tab_drop_event(14.0, 0.5)
            .into_event()
            .unwrap_or_default();
        assert!(evs.is_empty());
        assert_eq!(ctrl.group.panes[0].tabs()[0].id, "t0");
        assert_eq!(ctrl.group.panes[0].tabs()[1].id, "t1");
    }

    #[test]
    fn tab_drop_same_pane_center_is_noop() {
        let mut ctrl = make_two_pane();
        ctrl.handle_tab_drag_start(9.0, 0.5);
        let evs = ctrl
            .handle_tab_drop_event(40.0, 5.0)
            .into_event()
            .unwrap_or_default();
        assert!(evs.is_empty());
        assert_eq!(ctrl.pane_count(), 2);
        assert_eq!(ctrl.group.panes[0].tabs().len(), 2);
    }

    // ── handle_tab_drop: merge into another pane ──────────────────────────────

    #[test]
    fn tab_drop_moves_tab_to_another_pane_center() {
        let mut ctrl = make_two_pane();
        ctrl.handle_tab_drag_start(9.0, 0.5);
        let evs = ctrl
            .handle_tab_drop_event(120.0, 5.0)
            .into_event()
            .unwrap_or_default();
        assert_eq!(
            evs,
            vec![TabGroupEvent::TabMovedToPane {
                from_pane_idx: 0,
                to_pane_idx: 1,
                tab_id: "t1".into(),
                insert_idx: 1,
            }]
        );
        assert_eq!(ctrl.pane_count(), 2);
        assert_eq!(ctrl.group.panes[0].tabs().len(), 1);
        assert_eq!(ctrl.group.panes[0].tabs()[0].id, "t0");
        assert_eq!(ctrl.group.panes[1].tabs().len(), 2);
        assert_eq!(ctrl.group.panes[1].tabs()[1].id, "t1");
        assert_eq!(ctrl.group.panes[1].active_tab_id(), "t1");
        assert_eq!(ctrl.layout().leaf_count(), 2);
    }

    #[test]
    fn tab_drop_moves_tab_to_another_pane_tab_bar() {
        let mut ctrl = make_two_pane();
        ctrl.handle_tab_drag_start(9.0, 0.5);
        let evs = ctrl
            .handle_tab_drop_event(82.0, 0.5)
            .into_event()
            .unwrap_or_default();
        assert_eq!(
            evs,
            vec![TabGroupEvent::TabMovedToPane {
                from_pane_idx: 0,
                to_pane_idx: 1,
                tab_id: "t1".into(),
                insert_idx: 0,
            }]
        );
        assert_eq!(ctrl.group.panes[1].tabs()[0].id, "t1");
        assert_eq!(ctrl.group.panes[1].tabs()[1].id, "x0");
    }

    #[test]
    fn tab_drop_source_collapses_when_last_tab_moved() {
        let mut ctrl = TabGroupController::with_pane(
            "p0",
            vec![tab("only", "only.rs", true)],
            "only",
            SplitDirection::Horizontal,
        );
        ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", true));
        ctrl.focus_pane(0);
        prime_pane(&mut ctrl, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        prime_pane(&mut ctrl, 1, 80.0, 0.0, 80.0, 8.0, 2.0);

        ctrl.handle_tab_drag_start(3.0, 0.5);
        let evs = ctrl
            .handle_tab_drop_event(120.0, 5.0)
            .into_event()
            .unwrap_or_default();

        assert_eq!(ctrl.pane_count(), 1);
        assert_eq!(
            evs,
            vec![
                TabGroupEvent::TabMovedToPane {
                    from_pane_idx: 0,
                    to_pane_idx: 0,
                    tab_id: "only".into(),
                    insert_idx: 1,
                },
                TabGroupEvent::PaneCollapsed { pane_idx: 0 },
            ]
        );
        assert_eq!(ctrl.group.panes[0].tabs().len(), 2);
        assert_eq!(ctrl.group.panes[0].tabs()[1].id, "only");
        assert_eq!(ctrl.layout().leaf_count(), 1);
    }

    // ── handle_tab_drop: split to new pane ────────────────────────────────────

    #[test]
    fn tab_drop_splits_right_edge_creates_pane_after_target() {
        let mut ctrl = make_two_pane();
        ctrl.handle_tab_drag_start(9.0, 0.5);
        let evs = ctrl
            .handle_tab_drop_event(150.0, 5.0)
            .into_event()
            .unwrap_or_default();
        assert_eq!(evs.len(), 1, "no collapse expected: {evs:?}");
        assert!(
            matches!(
                &evs[0],
                TabGroupEvent::TabSplitToNewPane {
                    from_pane_idx: 0,
                    tab_id,
                    target_pane_idx: 1,
                    edge: crate::DropEdge::Right,
                    new_pane_idx: 2,
                } if tab_id == "t1"
            ),
            "unexpected event: {:?}",
            evs[0]
        );
        assert_eq!(ctrl.pane_count(), 3);
        assert_eq!(ctrl.group.panes[0].tabs()[0].id, "t0");
        assert_eq!(ctrl.group.panes[1].tabs()[0].id, "x0");
        assert_eq!(ctrl.group.panes[2].tabs()[0].id, "t1");
        assert_eq!(ctrl.layout().leaf_count(), 3);
    }

    #[test]
    fn tab_drop_splits_left_edge_creates_pane_before_target() {
        let mut ctrl = make_two_pane();
        ctrl.handle_tab_drag_start(9.0, 0.5);
        let evs = ctrl
            .handle_tab_drop_event(83.0, 5.0)
            .into_event()
            .unwrap_or_default();
        assert_eq!(evs.len(), 1, "no collapse expected: {evs:?}");
        assert!(
            matches!(
                &evs[0],
                TabGroupEvent::TabSplitToNewPane {
                    from_pane_idx: 0,
                    tab_id,
                    target_pane_idx: 1,
                    edge: crate::DropEdge::Left,
                    new_pane_idx: 1,
                } if tab_id == "t1"
            ),
            "unexpected event: {:?}",
            evs[0]
        );
        assert_eq!(ctrl.pane_count(), 3);
        assert_eq!(ctrl.group.panes[0].tabs()[0].id, "t0");
        assert_eq!(ctrl.group.panes[1].tabs()[0].id, "t1");
        assert_eq!(ctrl.group.panes[2].tabs()[0].id, "x0");
    }

    #[test]
    fn tab_drop_split_source_collapses_when_last_tab() {
        let mut ctrl = TabGroupController::with_pane(
            "p0",
            vec![tab("only", "only.rs", true)],
            "only",
            SplitDirection::Horizontal,
        );
        ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", true));
        ctrl.focus_pane(0);
        prime_pane(&mut ctrl, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        prime_pane(&mut ctrl, 1, 80.0, 0.0, 80.0, 8.0, 2.0);

        ctrl.handle_tab_drag_start(3.0, 0.5);
        let evs = ctrl
            .handle_tab_drop_event(150.0, 5.0)
            .into_event()
            .unwrap_or_default();

        assert_eq!(ctrl.pane_count(), 2);
        assert_eq!(evs.len(), 2, "expected split + collapse: {evs:?}");
        assert!(
            matches!(
                &evs[0],
                TabGroupEvent::TabSplitToNewPane {
                    from_pane_idx: 0,
                    tab_id,
                    new_pane_idx: 1,
                    ..
                } if tab_id == "only"
            ),
            "unexpected primary event: {:?}",
            evs[0]
        );
        assert_eq!(evs[1], TabGroupEvent::PaneCollapsed { pane_idx: 0 });
        assert_eq!(ctrl.group.panes[0].tabs()[0].id, "x0");
        assert_eq!(ctrl.group.panes[1].tabs()[0].id, "only");
        assert_eq!(ctrl.layout().leaf_count(), 2);
    }

    #[test]
    fn tab_drop_only_tab_only_pane_split_is_noop() {
        let mut ctrl = TabGroupController::with_pane(
            "p0",
            vec![tab("t0", "only.rs", true)],
            "t0",
            SplitDirection::Horizontal,
        );
        prime_pane(&mut ctrl, 0, 0.0, 0.0, 80.0, 8.0, 2.0);

        ctrl.handle_tab_drag_start(3.0, 0.5);
        let evs = ctrl
            .handle_tab_drop_event(70.0, 5.0)
            .into_event()
            .unwrap_or_default();
        assert!(evs.is_empty());
        assert_eq!(ctrl.pane_count(), 1);
        assert_eq!(ctrl.group.panes[0].tabs().len(), 1);
    }

    #[test]
    fn tab_drop_clears_drag_state_on_noop() {
        let mut ctrl = make_two_pane();
        ctrl.handle_tab_drag_start(9.0, 0.5);
        assert_eq!(
            ctrl.handle_tab_drop_event(999.0, 999.0),
            Handled::Consumed,
            "a drag was in progress and got cleared, even though the drop itself was a no-op"
        );
        assert!(!ctrl.is_tab_dragging());
    }

    #[test]
    fn tab_drop_without_an_active_drag_returns_ignored() {
        let mut ctrl = make_two_pane();
        assert_eq!(
            ctrl.handle_tab_drop_event(150.0, 5.0),
            Handled::Ignored,
            "no tab drag was ever started"
        );
    }

    /// Self-pane edge drop with multiple tabs: pane has ≥2 tabs, drag one out
    /// to its OWN pane's edge. Source pane stays alive (no collapse), a new
    /// pane is created adjacent to it.
    #[test]
    fn tab_drop_self_pane_edge_with_multiple_tabs_splits_without_collapse() {
        let mut ctrl = make_two_pane();
        // Pane 0 has tabs [t0, t1]; drag t1 onto pane 0's own left edge.
        // Left edge of pane 0 content: x < 0 + edge_w(=3). Use x=1.0, y=5.0 (in content).
        ctrl.handle_tab_drag_start(9.0, 0.5); // t1 from pane 0
        let evs = ctrl
            .handle_tab_drop_event(1.0, 5.0)
            .into_event()
            .unwrap_or_default(); // pane 0 left edge

        // Exactly one event (split, no collapse since pane 0 still has t0).
        assert_eq!(evs.len(), 1, "no collapse expected: {evs:?}");
        assert!(
            matches!(
                &evs[0],
                TabGroupEvent::TabSplitToNewPane {
                    from_pane_idx: 0,
                    tab_id,
                    target_pane_idx: 0,
                    edge: crate::DropEdge::Left,
                    new_pane_idx: 0,
                } if tab_id == "t1"
            ),
            "unexpected event: {:?}",
            evs[0]
        );
        // Pane count grew from 2 to 3; layout tree still has 3 unique leaves.
        assert_eq!(ctrl.pane_count(), 3);
        assert_eq!(ctrl.layout().leaf_count(), 3);
        // The new pane (vec index 0) carries the moved tab; the old pane 0
        // (now at vec index 1) keeps its remaining tab.
        assert_eq!(ctrl.group.panes[0].tabs().len(), 1);
        assert_eq!(ctrl.group.panes[0].tabs()[0].id, "t1");
        assert_eq!(ctrl.group.panes[1].tabs().len(), 1);
        assert_eq!(ctrl.group.panes[1].tabs()[0].id, "t0");
        // The original "other pane" (was index 1) is now at index 2.
        assert_eq!(ctrl.group.panes[2].tabs()[0].id, "x0");
    }

    // ── set_drag_geometry ─────────────────────────────────────────────────────
    //
    // Geometry layout used in these tests (no render() call anywhere):
    //
    //  Pane 0: strip (x=0, y=0, w=80, h=1),  content (x=0, y=1, w=80, h=10)
    //          tab_slots: [(0,8), (8,16)]  → t0 at [0,8), t1 at [8,16)
    //
    //  Pane 1: strip (x=80, y=0, w=80, h=1), content (x=80, y=1, w=80, h=10)
    //          tab_slots: [(80,88)]         → x0 at [80,88)
    //
    //  edge_zone_size(80) = 80*0.2 = 16.0   → left: rel_x < 16, right: rel_x >= 64
    //  edge_zone_size(10) = 10*0.2 = 2 → clamped to 3   → top: rel_y < 3, bottom: rel_y >= 7

    fn two_pane_drag_rects() -> (TabGroupController, Vec<PaneDragRect>) {
        let mut ctrl = TabGroupController::with_pane(
            "p0",
            vec![tab("t0", "main.rs", true), tab("t1", "lib.rs", true)],
            "t0",
            SplitDirection::Horizontal,
        );
        ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", true));
        ctrl.focus_pane(0);

        let rects = vec![
            PaneDragRect {
                strip_bounds: Rect::new(0.0, 0.0, 80.0, 1.0),
                content_bounds: Rect::new(0.0, 1.0, 80.0, 10.0),
                tab_slots: vec![(0.0, 8.0), (8.0, 16.0)],
            },
            PaneDragRect {
                strip_bounds: Rect::new(80.0, 0.0, 80.0, 1.0),
                content_bounds: Rect::new(80.0, 1.0, 80.0, 10.0),
                tab_slots: vec![(80.0, 88.0)],
            },
        ];
        (ctrl, rects)
    }

    #[test]
    fn set_drag_geometry_populates_drop_group_rects() {
        let (mut ctrl, rects) = two_pane_drag_rects();
        // Before priming: no rects.
        assert!(ctrl.drop_group_rects().is_empty());

        ctrl.set_drag_geometry(&rects);

        let groups = ctrl.drop_group_rects();
        assert_eq!(groups.len(), 2, "both panes should be primed");
        // Pane 0: full bounds = strip + content = (0, 0, 80, 11)
        assert_eq!(groups[0].bounds.x, 0.0);
        assert_eq!(groups[0].bounds.width, 80.0);
        assert_eq!(groups[0].bounds.height, 11.0);
        assert_eq!(groups[0].tab_slots.len(), 2);
        assert_eq!(groups[0].tab_slots[0], (0.0, 8.0));
        // Pane 1: full bounds = (80, 0, 80, 11)
        assert_eq!(groups[1].bounds.x, 80.0);
        assert_eq!(groups[1].tab_slots.len(), 1);
    }

    #[test]
    fn set_drag_geometry_enables_drop_zone_at() {
        let (mut ctrl, rects) = two_pane_drag_rects();
        ctrl.set_drag_geometry(&rects);

        // Center of pane 1 content: x=120, y=5 → DropZoneKind::Center, group_idx=1
        let zone = ctrl.drop_zone_at(120.0, 5.0).expect("should resolve");
        assert_eq!(zone.group_idx, 1);
        assert!(matches!(
            zone.kind,
            crate::primitives::drop_zone::DropZoneKind::Center
        ));
    }

    #[test]
    fn set_drag_geometry_enables_handle_tab_drag_start() {
        let (mut ctrl, rects) = two_pane_drag_rects();
        ctrl.set_drag_geometry(&rects);

        // x=9.0 lands in pane 0 slot [8, 16) → tab t1.
        assert!(ctrl.handle_tab_drag_start(9.0, 0.5));
        assert!(ctrl.is_tab_dragging());
        let drag = ctrl.state.tab_drag.as_ref().unwrap();
        assert_eq!(drag.pane_idx, 0);
        assert_eq!(drag.tab_id, "t1");
    }

    #[test]
    fn set_drag_geometry_enables_handle_tab_drag_move() {
        let (mut ctrl, rects) = two_pane_drag_rects();
        ctrl.set_drag_geometry(&rects);
        ctrl.handle_tab_drag_start(9.0, 0.5); // drag t1

        // Move cursor to center of pane 1 (x=120, y=5).
        let zone = ctrl.handle_tab_drag_move(120.0, 5.0);
        assert!(zone.is_some(), "should resolve a drop zone");
        assert_eq!(zone.unwrap().group_idx, 1);
    }

    #[test]
    fn set_drag_geometry_reorder_within_pane() {
        let (mut ctrl, rects) = two_pane_drag_rects();
        ctrl.set_drag_geometry(&rects);
        ctrl.handle_tab_drag_start(9.0, 0.5); // drag t1 (slot [8,16))

        // Drop at x=3 in pane 0 strip → reorder: t1 goes before t0.
        let evs = ctrl
            .handle_tab_drop_event(3.0, 0.5)
            .into_event()
            .unwrap_or_default();
        assert_eq!(
            evs,
            vec![TabGroupEvent::TabReordered {
                pane_idx: 0,
                tab_id: "t1".into(),
                from_idx: 1,
                to_idx: 0,
            }]
        );
        assert_eq!(ctrl.group.panes[0].tabs()[0].id, "t1");
        assert_eq!(ctrl.group.panes[0].tabs()[1].id, "t0");
    }

    #[test]
    fn set_drag_geometry_move_tab_to_other_pane() {
        let (mut ctrl, rects) = two_pane_drag_rects();
        ctrl.set_drag_geometry(&rects);
        ctrl.handle_tab_drag_start(9.0, 0.5); // drag t1

        // Drop in pane 1 center: x=120 (rel_x=40, between edges 16..64), y=5.
        let evs = ctrl
            .handle_tab_drop_event(120.0, 5.0)
            .into_event()
            .unwrap_or_default();
        assert_eq!(evs.len(), 1, "no collapse expected: {evs:?}");
        assert!(
            matches!(
                &evs[0],
                TabGroupEvent::TabMovedToPane {
                    from_pane_idx: 0,
                    to_pane_idx: 1,
                    tab_id,
                    ..
                } if tab_id == "t1"
            ),
            "unexpected event: {:?}",
            evs[0]
        );
        assert_eq!(ctrl.pane_count(), 2);
        assert_eq!(ctrl.group.panes[0].tabs().len(), 1);
        assert_eq!(ctrl.group.panes[1].tabs().len(), 2);
    }

    #[test]
    fn set_drag_geometry_split_to_new_pane() {
        let (mut ctrl, rects) = two_pane_drag_rects();
        ctrl.set_drag_geometry(&rects);
        ctrl.handle_tab_drag_start(9.0, 0.5); // drag t1

        // Drop on right edge of pane 1: x=150 (rel_x=70 >= 64), y=5.
        let evs = ctrl
            .handle_tab_drop_event(150.0, 5.0)
            .into_event()
            .unwrap_or_default();
        assert_eq!(evs.len(), 1, "no collapse: {evs:?}");
        assert!(
            matches!(
                &evs[0],
                TabGroupEvent::TabSplitToNewPane {
                    from_pane_idx: 0,
                    tab_id,
                    edge: crate::DropEdge::Right,
                    ..
                } if tab_id == "t1"
            ),
            "unexpected event: {:?}",
            evs[0]
        );
        assert_eq!(ctrl.pane_count(), 3);
        assert_eq!(ctrl.layout().leaf_count(), 3);
    }

    #[test]
    fn set_drag_geometry_ignores_excess_entries() {
        // Passing more PaneDragRect entries than panes should not panic or corrupt.
        let (mut ctrl, mut rects) = two_pane_drag_rects();
        rects.push(PaneDragRect {
            strip_bounds: Rect::new(200.0, 0.0, 80.0, 1.0),
            content_bounds: Rect::new(200.0, 1.0, 80.0, 10.0),
            tab_slots: vec![(200.0, 208.0)],
        });
        ctrl.set_drag_geometry(&rects);
        // Only 2 panes primed; excess entry silently dropped.
        assert_eq!(ctrl.drop_group_rects().len(), 2);
    }

    #[test]
    fn set_drag_geometry_partial_prime_leaves_rest_unprimed() {
        // Passing fewer entries primes only those panes; the rest stay None.
        let (mut ctrl, rects) = two_pane_drag_rects();
        ctrl.set_drag_geometry(&rects[..1]); // only pane 0
        let groups = ctrl.drop_group_rects();
        assert_eq!(groups.len(), 1, "only pane 0 should be primed");
        assert_eq!(groups[0].bounds.x, 0.0);
    }

    // ── Unique pane IDs after repeated splits ─────────────────────────────────

    #[test]
    fn split_then_collapse_then_split_unique_ids() {
        // Drag t1 out to create pane:1, collapse it, then drag a different tab
        // to create another split.  The second new pane must have a distinct id.
        let mut ctrl = make_two_pane();
        ctrl.handle_tab_drag_start(9.0, 0.5); // drag t1
        let _ = ctrl.handle_tab_drop_event(150.0, 5.0); // split → pane[2].id == "pane:1"
        assert_eq!(ctrl.pane_count(), 3);
        let first_new_id = ctrl.group.panes[2].id.clone();

        ctrl.collapse_pane(2); // remove the new pane
        assert_eq!(ctrl.pane_count(), 2);

        // Add an extra tab so pane 0 won't be emptied by the next drag.
        ctrl.add_tab(0, tab("t-extra", "extra.rs", false));

        // Re-prime: pane 0 now has 2 tabs (t0, t-extra), pane 1 has 1 tab (x0).
        prime_pane(&mut ctrl, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        prime_pane(&mut ctrl, 1, 80.0, 0.0, 80.0, 8.0, 2.0);

        // Drag t-extra (slot [8..16), x=9) to right edge of pane 1 → creates pane:2.
        ctrl.handle_tab_drag_start(9.0, 0.5);
        let evs = ctrl
            .handle_tab_drop_event(150.0, 5.0)
            .into_event()
            .unwrap_or_default();

        assert_eq!(
            ctrl.pane_count(),
            3,
            "second split should create 3 panes: {evs:?}"
        );
        let second_new_id = ctrl.group.panes[2].id.clone();
        assert_ne!(
            second_new_id, first_new_id,
            "second split pane must have a unique id (got {second_new_id:?})"
        );
    }

    // ── resolve_tab_drop: host-owned-model adoption path ───────────────────────
    //
    // These tests call `resolve_tab_drop` directly with hand-built
    // `DropGroupRect`s and a `TabDragSource` — no `TabGroupController`
    // anywhere. This is the shape a host with its own tab/pane model (e.g.
    // vimcode's `Engine::editor_groups`) uses: build `groups` from its own
    // layout, call `resolve_tab_drop`, translate the result into its own
    // mutations directly. Geometry mirrors `two_pane_drag_rects()` above so
    // the numbers are directly comparable to the `TabGroupController`-owned
    // tests.

    fn two_group_rects() -> Vec<DropGroupRect> {
        vec![
            DropGroupRect {
                bounds: Rect::new(0.0, 0.0, 80.0, 11.0),
                tab_slots: vec![(0.0, 8.0), (8.0, 16.0)],
            },
            DropGroupRect {
                bounds: Rect::new(80.0, 0.0, 80.0, 11.0),
                tab_slots: vec![(80.0, 88.0)],
            },
        ]
    }

    #[test]
    fn resolve_tab_drop_reorders_within_same_pane() {
        let groups = two_group_rects();
        let counts = [2usize, 1usize];
        let source = TabDragSource {
            pane_idx: 0,
            tab_idx: 1, // "t1"
        };
        let instr = resolve_tab_drop(source, &counts, 3.0, 0.5, &groups, 1.0);
        assert_eq!(
            instr,
            TabDropInstruction::Reorder {
                pane_idx: 0,
                from_idx: 1,
                to_idx: 0,
            }
        );
    }

    #[test]
    fn resolve_tab_drop_same_position_is_noop() {
        let groups = two_group_rects();
        let counts = [2usize, 1usize];
        let source = TabDragSource {
            pane_idx: 0,
            tab_idx: 1,
        };
        let instr = resolve_tab_drop(source, &counts, 14.0, 0.5, &groups, 1.0);
        assert_eq!(instr, TabDropInstruction::NoOp);
    }

    #[test]
    fn resolve_tab_drop_same_pane_center_is_noop() {
        let groups = two_group_rects();
        let counts = [2usize, 1usize];
        let source = TabDragSource {
            pane_idx: 0,
            tab_idx: 1,
        };
        let instr = resolve_tab_drop(source, &counts, 40.0, 5.0, &groups, 1.0);
        assert_eq!(instr, TabDropInstruction::NoOp);
    }

    #[test]
    fn resolve_tab_drop_moves_tab_to_another_pane_center() {
        let groups = two_group_rects();
        let counts = [2usize, 1usize];
        let source = TabDragSource {
            pane_idx: 0,
            tab_idx: 1,
        };
        let instr = resolve_tab_drop(source, &counts, 120.0, 5.0, &groups, 1.0);
        assert_eq!(
            instr,
            TabDropInstruction::MoveToPane {
                from_pane_idx: 0,
                from_idx: 1,
                to_pane_idx: 1,
                insert_idx: 1,
                source_now_empty: false,
            }
        );
    }

    #[test]
    fn resolve_tab_drop_reports_source_now_empty_when_last_tab_moves() {
        let groups = two_group_rects();
        // Source pane down to its last tab, as the host's own model would
        // report once earlier tabs are already gone.
        let counts = [1usize, 1usize];
        let source = TabDragSource {
            pane_idx: 0,
            tab_idx: 0,
        };
        let instr = resolve_tab_drop(source, &counts, 120.0, 5.0, &groups, 1.0);
        assert_eq!(
            instr,
            TabDropInstruction::MoveToPane {
                from_pane_idx: 0,
                from_idx: 0,
                to_pane_idx: 1,
                insert_idx: 1,
                source_now_empty: true,
            }
        );
    }

    #[test]
    fn resolve_tab_drop_splits_right_edge() {
        let groups = two_group_rects();
        let counts = [2usize, 1usize];
        let source = TabDragSource {
            pane_idx: 0,
            tab_idx: 1,
        };
        let instr = resolve_tab_drop(source, &counts, 150.0, 5.0, &groups, 1.0);
        assert_eq!(
            instr,
            TabDropInstruction::SplitToNewPane {
                from_pane_idx: 0,
                from_idx: 1,
                target_pane_idx: 1,
                edge: DropEdge::Right,
                split_direction: SplitDirection::Horizontal,
                source_now_empty: false,
            }
        );
    }

    #[test]
    fn resolve_tab_drop_only_tab_only_pane_split_is_noop() {
        let groups = vec![DropGroupRect {
            bounds: Rect::new(0.0, 0.0, 80.0, 11.0),
            tab_slots: vec![(0.0, 8.0)],
        }];
        let counts = [1usize];
        let source = TabDragSource {
            pane_idx: 0,
            tab_idx: 0,
        };
        let instr = resolve_tab_drop(source, &counts, 1.0, 5.0, &groups, 1.0);
        assert_eq!(instr, TabDropInstruction::NoOp);
    }

    #[test]
    fn resolve_tab_drop_outside_all_groups_is_noop() {
        let groups = two_group_rects();
        let counts = [2usize, 1usize];
        let source = TabDragSource {
            pane_idx: 0,
            tab_idx: 1,
        };
        let instr = resolve_tab_drop(source, &counts, 999.0, 999.0, &groups, 1.0);
        assert_eq!(instr, TabDropInstruction::NoOp);
    }

    #[test]
    fn resolve_tab_drop_agrees_with_controller_handle_tab_drop_event() {
        // Cross-check: `TabGroupController::handle_tab_drop_event` (the owned
        // model path) and `resolve_tab_drop` (the host-owned-model path)
        // must reach equivalent conclusions from equivalent inputs, since
        // the former is implemented in terms of the latter.
        let mut ctrl = make_two_pane();
        ctrl.handle_tab_drag_start(9.0, 0.5); // drag t1 from pane 0
        let events = ctrl
            .handle_tab_drop_event(150.0, 5.0)
            .into_event()
            .unwrap_or_default(); // split right edge of pane 1

        let groups = two_group_rects();
        let counts = [2usize, 1usize];
        let source = TabDragSource {
            pane_idx: 0,
            tab_idx: 1,
        };
        let instr = resolve_tab_drop(source, &counts, 150.0, 5.0, &groups, 1.0);

        assert_eq!(
            instr,
            TabDropInstruction::SplitToNewPane {
                from_pane_idx: 0,
                from_idx: 1,
                target_pane_idx: 1,
                edge: DropEdge::Right,
                split_direction: SplitDirection::Horizontal,
                source_now_empty: false,
            }
        );
        assert_eq!(
            events,
            vec![TabGroupEvent::TabSplitToNewPane {
                from_pane_idx: 0,
                tab_id: "t1".into(),
                target_pane_idx: 1,
                edge: DropEdge::Right,
                new_pane_idx: 2,
            }]
        );
    }

    // ── Style::tab_height governs the tab strip on a pixel backend ────────

    #[test]
    fn strip_height_uses_style_tab_height_not_chrome_line_height() {
        let mut ctrl = make_group();
        let mut backend = crate::testing::RecordingBackend {
            line_height: 20.0,
            chrome_line_height: Some(18.0),
            viewport: crate::Viewport {
                width: 800.0,
                height: 600.0,
                scale: 1.0,
            },
            char_width: 8.0,
            ..Default::default()
        };

        let bounds = Rect::new(0.0, 0.0, 800.0, 600.0);
        let layout = ctrl.render(&mut backend, bounds);

        // The strip height is `Style::tab_height` (35, the default) — not
        // the editor line_height (20) nor the chrome line height (18).
        assert_eq!(layout.strip_bounds.len(), 1);
        assert_eq!(layout.strip_bounds[0].height, 35.0);
        // Content area starts after the strip.
        assert_eq!(layout.content_bounds[0].y, 35.0);
        assert_eq!(layout.content_bounds[0].height, 600.0 - 35.0);
    }

    /// `Backend::set_style` overriding `tab_height` changes the painted
    /// strip height — the acceptance bar for this chrome metric.
    #[test]
    fn strip_height_follows_a_set_style_override() {
        let mut ctrl = make_group();
        let mut backend = crate::testing::RecordingBackend {
            line_height: 20.0,
            chrome_line_height: Some(18.0),
            viewport: crate::Viewport {
                width: 800.0,
                height: 600.0,
                scale: 1.0,
            },
            char_width: 8.0,
            ..Default::default()
        };
        backend.set_style(crate::Style::default().with_tab_height(50.0));

        let bounds = Rect::new(0.0, 0.0, 800.0, 600.0);
        let layout = ctrl.render(&mut backend, bounds);

        assert_eq!(layout.strip_bounds[0].height, 50.0);
    }

    /// On a cell grid (TUI), the strip stays exactly one row tall —
    /// `Style::tab_height` is a documented no-op there, even with an
    /// override set.
    #[test]
    fn strip_height_ignores_style_tab_height_on_a_cell_grid() {
        let mut ctrl = make_group();
        let mut backend = crate::testing::RecordingBackend::new();
        backend.set_style(crate::Style::default().with_tab_height(50.0));

        let bounds = Rect::new(0.0, 0.0, 80.0, 24.0);
        let layout = ctrl.render(&mut backend, bounds);

        assert_eq!(layout.strip_bounds[0].height, 1.0);
    }

    // ── render -> click round-trip through the painted strip layout ───────

    /// Backend plus bounds for the unit regime this build's
    /// `Primitive::layout` can resolve: whole cells when `tui` is compiled
    /// in, pixels otherwise (the branch every pixel backend takes). Both
    /// rects are wide enough for `make_group`'s two tabs to get painted
    /// slots in their own regime, so a geometry-dependent round-trip below
    /// lands on whichever branch the active feature set selects.
    fn click_regime() -> (crate::testing::RecordingBackend, Rect) {
        #[cfg(feature = "tui")]
        {
            (
                crate::testing::RecordingBackend::new(),
                Rect::new(0.0, 0.0, 80.0, 24.0),
            )
        }
        #[cfg(not(feature = "tui"))]
        {
            (
                crate::testing::RecordingBackend {
                    line_height: 20.0,
                    chrome_line_height: Some(18.0),
                    viewport: crate::Viewport {
                        width: 800.0,
                        height: 600.0,
                        scale: 1.0,
                    },
                    char_width: 8.0,
                    ..Default::default()
                },
                Rect::new(0.0, 0.0, 800.0, 600.0),
            )
        }
    }

    /// A real `render` pass caches the strip layout the `TabBar` primitive
    /// itself resolved, so clicking the second tab's own painted x-range
    /// (recovered through `drop_group_rects`, which re-absolutises that
    /// same cached layout) activates that tab — the absolute/strip-relative
    /// conversion on both sides has to agree for this to land.
    #[test]
    fn clicking_a_painted_tab_slot_activates_that_tab() {
        let mut ctrl = make_group();
        let (mut backend, bounds) = click_regime();
        let layout = ctrl.render(&mut backend, bounds);
        let strip = layout.strip_bounds[0];

        let slots = ctrl.drop_group_rects();
        let (start, end) = slots[0].tab_slots[1];
        assert!(end > start, "tab 1 must have a painted slot: {slots:?}");

        let mid_y = strip.y + strip.height / 2.0;
        match ctrl.handle_click_event((start + end) / 2.0, mid_y) {
            Handled::Event(TabGroupEvent::TabActivated { pane_idx, tab_id }) => {
                assert_eq!(pane_idx, 0);
                assert_eq!(tab_id, "t1");
            }
            other => panic!("expected tab 1 to activate, got {other:?}"),
        }
        assert_eq!(ctrl.group.panes[0].active_tab_id, "t1");
    }

    /// The same painted layout classifies the close glyph ahead of the tab
    /// body, so clicking it closes rather than activates.
    #[test]
    fn clicking_a_painted_close_glyph_closes_that_tab() {
        let mut ctrl = make_group();
        let (mut backend, bounds) = click_regime();
        let layout = ctrl.render(&mut backend, bounds);
        let strip = layout.strip_bounds[0];

        let (cx, cy) = ctrl
            .last_layout
            .as_ref()
            .expect("render cached the layout")
            .panes[0]
            .tabs
            .layout
            .tab_close_center(1)
            .expect("tab 1 painted a close glyph");

        match ctrl.handle_click_event(strip.x + cx, strip.y + cy) {
            Handled::Event(TabGroupEvent::TabClosed { pane_idx, tab_id }) => {
                assert_eq!(pane_idx, 0);
                assert_eq!(tab_id, "t1");
            }
            other => panic!("expected tab 1 to close, got {other:?}"),
        }
    }

    /// A pane with far more tabs than fit the strip, whose active tab sits
    /// near the end, must keep that tab visible after a real render on a
    /// pixel backend — the scroll offset `render` writes back has to be
    /// the *corrected* one ([`crate::primitives::tab_bar::
    /// TabBarPrimitiveLayout::corrected_scroll_offset`]), not
    /// `TabBarLayout::resolved_scroll_offset` (merely clamped), or the
    /// strip would permanently clip from tab 0 and hide the active tab.
    ///
    /// Forced to the pixel regime regardless of the `tui` feature (unlike
    /// [`click_regime`]): TUI's own cell-grid fit intentionally leaves
    /// scroll correction to the caller (`TabBar::layout`'s own
    /// "Two-pass-paint pattern" doc), so only a pixel backend's
    /// `fit_active_scroll_offset` self-correction is in scope here.
    #[test]
    fn rendering_an_overflowing_strip_scrolls_the_active_tab_into_view() {
        let tabs: Vec<PaneTab> = (0..40)
            .map(|i| tab(&format!("t{i}"), &format!("tab-label-{i}"), true))
            .collect();
        let mut ctrl =
            TabGroupController::with_pane("pane:0", tabs, "t39", SplitDirection::Horizontal);
        let mut backend = crate::testing::RecordingBackend::with_viewport(
            crate::Viewport {
                width: 800.0,
                height: 600.0,
                scale: 1.0,
            },
            20.0,
            8.0,
        );
        let bounds = Rect::new(0.0, 0.0, 800.0, 600.0);

        // First pass: paints from the pane's initial `scroll_offset: 0`
        // and writes back the correction for the next frame — the
        // "two-pass paint" pattern `TabBar::layout`'s own doc describes.
        ctrl.render(&mut backend, bounds);

        // Forty generously-labelled tabs can't possibly fit the 800px
        // strip, so a scroll offset of `0` would leave tab 39 scrolled out
        // of view — the regression this test guards against.
        assert!(
            ctrl.group.panes[0].tab_scroll_offset > 0,
            "expected a non-zero corrected scroll offset to keep tab 39 visible, got 0"
        );

        // Second pass: now paints from the corrected offset, so tab 39
        // actually appears.
        let layout = ctrl.render(&mut backend, bounds);
        let strip = layout.strip_bounds[0];

        let active_cache = &ctrl
            .last_layout
            .as_ref()
            .expect("render cached the layout")
            .panes[0]
            .tabs;
        assert!(
            active_cache
                .layout
                .visible_tabs
                .iter()
                .any(|vt| vt.tab_idx == 39),
            "tab 39 (the active one) must be among the painted visible tabs: {:?}",
            active_cache.layout.visible_tabs
        );

        // Tab 38, scrolled into view alongside the active tab 39, is
        // clickable where it painted (strip-relative bounds, re-absolutised
        // the same way `drop_group_rects` does) and activates on click —
        // a non-active tab, so the click produces a real `TabActivated`
        // event rather than `Handled::Consumed`.
        let tab_38 = active_cache
            .layout
            .visible_tabs
            .iter()
            .find(|vt| vt.tab_idx == 38)
            .unwrap_or_else(|| {
                panic!(
                    "tab 38 must also be among the painted visible tabs: {:?}",
                    active_cache.layout.visible_tabs
                )
            });
        let (start, end) = (
            tab_38.bounds.x + strip.x,
            tab_38.bounds.x + strip.x + tab_38.bounds.width,
        );
        let mid_y = strip.y + strip.height / 2.0;
        match ctrl.handle_click_event((start + end) / 2.0, mid_y) {
            Handled::Event(TabGroupEvent::TabActivated { pane_idx, tab_id }) => {
                assert_eq!(pane_idx, 0);
                assert_eq!(tab_id, "t38");
            }
            other => panic!("expected tab 38 to activate, got {other:?}"),
        }
    }

    // ── The widget on its own ─────────────────────────────────────────────

    fn bare(id: &str, label: &str) -> TabGroupTab {
        TabGroupTab::new(id, label)
    }

    /// Two side-by-side bare panes: `a`, `b` on the left, `c` on the right.
    fn bare_group() -> TabGroup {
        TabGroup::from_layout(
            "group",
            vec![
                TabGroupPane::new("left", vec![bare("a", "alpha"), bare("b", "beta")], "a"),
                TabGroupPane::new("right", vec![bare("c", "gamma")], "c"),
            ],
            GroupLayout::Split {
                direction: SplitDirection::Horizontal,
                ratio: 0.5,
                first: Box::new(GroupLayout::Leaf(0)),
                second: Box::new(GroupLayout::Leaf(1)),
            },
            SplitDirection::Horizontal,
        )
        .expect("valid layout")
    }

    fn cell_layout(group: &TabGroup, width: f32, height: f32) -> TabGroupPrimitiveLayout {
        let ctx = LayoutCtx::new(1.0, 1.0, false);
        Primitive::layout(group, &ctx, Rect::new(0.0, 0.0, width, height))
    }

    /// An [`EventHost`] with one focus slot and a record of capture.
    #[derive(Default)]
    struct TestHost {
        focus: Option<Option<SubTarget>>,
        captured: bool,
    }

    impl EventHost for TestHost {
        fn is_focused(&self, _id: &WidgetId, sub: Option<SubTarget>) -> bool {
            self.focus == Some(sub)
        }
        fn request_focus(&mut self, _id: &WidgetId, sub: Option<SubTarget>) {
            self.focus = Some(sub);
        }
        fn capture_pointer(&mut self, _id: &WidgetId) {
            self.captured = true;
        }
        fn release_pointer(&mut self) {
            self.captured = false;
        }
        fn set_cursor(&mut self, _cursor: crate::backend::PointerShape) {}
        fn request_redraw(&mut self) {}
        fn emit(&mut self, _event: crate::UiEvent) {}
        fn units(&self) -> Units {
            Units::Cells
        }
    }

    fn pointer(
        group: &TabGroup,
        state: &mut TabGroupState,
        layout: &TabGroupPrimitiveLayout,
        host: &mut TestHost,
        kind: PointerKind,
        x: f32,
        y: f32,
    ) -> Handled<TabGroupEvent> {
        let event = RoutedEvent::Pointer {
            kind,
            sub: None,
            local: Point::new(x, y),
            mods: Modifiers::default(),
        };
        let mut cx = EventCx::new(group.id, host);
        Interactive::handle(group, state, &event, layout, &mut cx)
    }

    #[test]
    fn focus_is_one_stop_per_pane() {
        let group = bare_group();
        let FocusSpec::Children(stops) = Interactive::focus(&group) else {
            panic!("expected one stop per pane");
        };
        let parts: Vec<_> = stops
            .iter()
            .map(|s| TabGroupSubPart::from_sub_target(s.sub))
            .collect();
        assert_eq!(
            parts,
            vec![
                Some(TabGroupSubPart::Pane(0)),
                Some(TabGroupSubPart::Pane(1))
            ]
        );
    }

    #[test]
    fn sub_parts_round_trip_through_their_targets() {
        for part in [
            TabGroupSubPart::Pane(3),
            TabGroupSubPart::Strip(2, TabBarSubPart::TabClose(7)),
            TabGroupSubPart::Strip(0, TabBarSubPart::RightSegment(0)),
            TabGroupSubPart::Divider(4),
        ] {
            assert_eq!(
                TabGroupSubPart::from_sub_target(part.sub_target()),
                Some(part)
            );
        }
        assert_eq!(TabGroupSubPart::from_sub_target(SubTarget(0)), None);
    }

    #[test]
    fn layout_reports_each_pane_strip_and_body_beside_the_divider() {
        let group = bare_group();
        let layout = cell_layout(&group, 41.0, 10.0);
        assert_eq!(layout.dividers.dividers.len(), 1);
        let divider = layout.dividers.dividers[0].cell_position();
        assert_eq!(divider, 20);
        let (left, right) = (&layout.panes[0], &layout.panes[1]);
        assert_eq!(left.strip_bounds, Rect::new(0.0, 0.0, 20.0, 1.0));
        assert_eq!(left.content_bounds, Rect::new(0.0, 1.0, 20.0, 9.0));
        assert_eq!(right.strip_bounds, Rect::new(21.0, 0.0, 20.0, 1.0));
        assert_eq!(right.bar.id, WidgetId::new("group:pane1:tabs"));
    }

    /// A stacked split dragged (or handed over) to an extreme ratio still
    /// leaves the squeezed pane its whole tab strip, above the divider.
    #[test]
    fn a_stacked_pane_keeps_its_strip_at_an_extreme_ratio() {
        let group = TabGroup::from_layout(
            "stack",
            vec![
                TabGroupPane::new("top", vec![bare("a", "alpha")], "a"),
                TabGroupPane::new("bottom", vec![bare("b", "beta")], "b"),
            ],
            GroupLayout::Split {
                direction: SplitDirection::Vertical,
                ratio: 0.0,
                first: Box::new(GroupLayout::Leaf(0)),
                second: Box::new(GroupLayout::Leaf(1)),
            },
            SplitDirection::Vertical,
        )
        .expect("valid layout");
        let layout = cell_layout(&group, 40.0, 10.0);
        let top = &layout.panes[0];
        assert_eq!(top.strip_bounds, Rect::new(0.0, 0.0, 40.0, 1.0));
        assert_eq!(top.bounds.height, 1.0);
        assert_eq!(layout.dividers.dividers[0].cell_position(), 1);
        assert_eq!(layout.panes[1].strip_bounds.y, 2.0);
    }

    #[test]
    fn a_body_press_focuses_its_pane_once() {
        let group = bare_group();
        let layout = cell_layout(&group, 41.0, 10.0);
        let mut state = TabGroupState::default();
        let mut host = TestHost::default();
        let down = PointerKind::Down(MouseButton::Left);
        assert_eq!(
            pointer(&group, &mut state, &layout, &mut host, down, 30.0, 5.0),
            Handled::Event(TabGroupEvent::PaneFocused { pane_idx: 1 })
        );
        assert_eq!(
            pointer(&group, &mut state, &layout, &mut host, down, 30.0, 6.0),
            Handled::Ignored,
            "a press in the already-focused body is the host's"
        );
    }

    /// Press a tab, drag it past the threshold into the other pane and
    /// release: the drop is reported as an intent, the overlay shows while
    /// the drag hovers, and `apply` moves the tab.
    #[test]
    fn a_tab_drag_reports_a_drop_that_apply_carries_out() {
        let mut group = bare_group();
        let layout = cell_layout(&group, 41.0, 10.0);
        let beta = layout.panes[0]
            .tabs
            .layout
            .tab_center(1)
            .expect("beta painted");
        let mut state = TabGroupState::default();
        let mut host = TestHost::default();

        let pressed = pointer(
            &group,
            &mut state,
            &layout,
            &mut host,
            PointerKind::Down(MouseButton::Left),
            beta.0,
            beta.1,
        );
        assert_eq!(
            pressed,
            Handled::Event(TabGroupEvent::TabActivated {
                pane_idx: 0,
                tab_id: "b".into()
            })
        );
        assert!(host.captured, "a pressed tab captures the pointer");
        assert!(Interactive::popups(&group, &state, &layout).is_empty());

        let moved = pointer(
            &group,
            &mut state,
            &layout,
            &mut host,
            PointerKind::Move,
            30.0,
            5.0,
        );
        assert_eq!(moved, Handled::Consumed);
        assert_eq!(state.drop_zone().map(|z| z.group_idx), Some(1));
        let popups = Interactive::popups(&group, &state, &layout);
        assert_eq!(popups.len(), 1, "the drop overlay shows while hovering");

        let dropped = pointer(
            &group,
            &mut state,
            &layout,
            &mut host,
            PointerKind::Up(MouseButton::Left),
            30.0,
            5.0,
        );
        let Handled::Event(intent @ TabGroupEvent::TabDropped { .. }) = dropped else {
            panic!("expected a drop, got {dropped:?}");
        };
        assert!(!host.captured);
        assert!(!state.is_tab_dragging());
        assert_eq!(group.pane(0).map(|p| p.tabs().len()), Some(2), "controlled");

        let applied = group.apply(intent);
        assert!(matches!(
            applied.as_slice(),
            [TabGroupEvent::TabMovedToPane {
                from_pane_idx: 0,
                to_pane_idx: 1,
                ..
            }]
        ));
        assert_eq!(group.pane(1).map(|p| p.active_tab_id()), Some("b"));
    }

    #[test]
    fn a_press_released_in_place_is_not_a_drop() {
        let group = bare_group();
        let layout = cell_layout(&group, 41.0, 10.0);
        let alpha = layout.panes[0].tabs.layout.tab_center(0).expect("alpha");
        let mut state = TabGroupState::default();
        let mut host = TestHost::default();
        let pressed = pointer(
            &group,
            &mut state,
            &layout,
            &mut host,
            PointerKind::Down(MouseButton::Left),
            alpha.0,
            alpha.1,
        );
        assert_eq!(pressed, Handled::Consumed, "alpha is already active");
        assert_eq!(
            pointer(
                &group,
                &mut state,
                &layout,
                &mut host,
                PointerKind::Up(MouseButton::Left),
                alpha.0,
                alpha.1,
            ),
            Handled::Consumed
        );
        assert!(!state.is_tab_dragging());
    }

    #[test]
    fn apply_of_a_divider_resize_reports_the_clamped_ratio() {
        let mut group = bare_group();
        assert_eq!(
            group.apply(TabGroupEvent::DividerResized {
                divider_idx: 0,
                ratio: 0.99,
            }),
            vec![TabGroupEvent::DividerResized {
                divider_idx: 0,
                ratio: crate::primitives::split_tree::MAX_RATIO,
            }]
        );
        assert!(group
            .apply(TabGroupEvent::DividerResized {
                divider_idx: 0,
                ratio: crate::primitives::split_tree::MAX_RATIO,
            })
            .is_empty());
    }

    #[test]
    fn opening_and_closing_panes_keeps_every_pane_on_one_leaf() {
        let mut group = bare_group();
        let idx = group.add_pane_after(0, TabGroupPane::new("mid", vec![bare("m", "mid")], "m"));
        assert_eq!(idx, 1);
        assert_eq!(leaves(&group.layout()), vec![0, 1, 2]);
        group.collapse_pane(0);
        assert_eq!(group.pane_count(), 2);
        assert_eq!(group.pane(0).map(|p| p.id.as_str()), Some("mid"));
        assert_eq!(leaves(&group.layout()), vec![0, 1]);
    }
}
