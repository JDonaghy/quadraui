//! `TabGroup` — tabbed split panes on the widget contract.
//!
//! [`TabGroup`] describes N panes arranged in an arbitrary nested H/V
//! split tree, each with its own scrollable tab strip. It composes, never
//! re-paints, its children: each pane's strip is a [`TabBar`], the
//! dividers are one [`SplitTree`], and a tab drag previews its drop
//! through a [`DropOverlay`] popup. The pane bodies are the host's: the
//! layout hands back each pane's content rect, as [`SplitTree`] does for
//! its leaves.
//!
//! # Quick start
//!
//! ```ignore
//! use quadraui::compose::tab_group::{TabGroup, TabGroupEvent, TabGroupPane, TabGroupTab};
//! use quadraui::SplitDirection;
//!
//! let mut group = TabGroup::with_pane(
//!     WidgetId::new("editors"),
//!     TabGroupPane::new(
//!         "pane:0",
//!         vec![TabGroupTab::new("t0", "main.rs"), TabGroupTab::new("t1", "lib.rs")],
//!         "t0",
//!     ),
//!     SplitDirection::Horizontal,
//! );
//!
//! // In AppLogic::render — paint the group, then each pane's active tab
//! // into its body:
//! let painted = backend.draw_dyn(bounds, &group, &());
//! if let Some(layout) = painted.downcast_ref::<TabGroupPrimitiveLayout>() {
//!     for (pane, body) in group.panes().iter().zip(&layout.panes) {
//!         paint_document(backend, pane.active_tab_id(), body.content_bounds);
//!     }
//! }
//!
//! // In AppLogic::frame_tree:
//! tree.push(group.id, bounds, Box::new(group.clone()));
//!
//! // In AppLogic::handle:
//! if let UiEvent::TabGroup(_, intent) = event {
//!     for ev in group.apply(intent) {
//!         match ev {
//!             TabGroupEvent::TabActivated { pane_idx, tab_id } => { /* … */ }
//!             TabGroupEvent::TabClosed { pane_idx, tab_id } => { /* … */ }
//!             _ => {}
//!         }
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
//! Full TUI + GTK demo coverage across three example apps
//! (`examples/common/{tab_group_demo,tab_chrome_demo,wide_tab_bar_demo}.rs`,
//! run via `tui_tabgroup.rs` / `gtk_tabgroup.rs`) plus `TuiDriver`
//! end-to-end tests in `tests/tui_example_driver.rs` and
//! `tests/widget_contract_tab_group.rs` — this is the editor-group
//! split/tab shape `CLAUDE.md`'s vimcode reference table points at, not
//! speculative API. [`crate::compose::workspace::Workspace`] is the
//! single-strip counterpart; see that module's doc for how the two
//! differ.
//!
//! # Building multi-pane layouts
//!
//! For simple cases, [`TabGroup::add_pane_after`] opens a new pane
//! splitting an existing one in the group's default direction.
//!
//! When your app already knows the full layout — especially when it mixes
//! horizontal and vertical splits — use [`TabGroup::from_layout`] to hand
//! the tree over directly:
//!
//! ```ignore
//! use quadraui::compose::tab_group::{GroupLayout, TabGroup, TabGroupPane};
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
//! let group = TabGroup::from_layout(
//!     WidgetId::new("editors"),
//!     vec![
//!         TabGroupPane::new("pane:0", vec![/* tabs */], "t0"),
//!         TabGroupPane::new("pane:1", vec![/* tabs */], "t1"),
//!         TabGroupPane::new("pane:2", vec![/* tabs */], "t2"),
//!     ],
//!     layout,
//!     SplitDirection::Horizontal,
//! ).expect("layout must cover every pane index exactly once");
//! ```
//!
//! # Divider and tab drags
//!
//! Panes are separated by draggable [`SplitTree`] dividers. A routed
//! group runs both drags itself from one press-drag-release: a press on a
//! divider drags it and reports [`TabGroupEvent::DividerResized`]; a press
//! on a tab body arms a tab drag that becomes live once the pointer
//! travels past a threshold, previews its drop as a popup, and reports
//! [`TabGroupEvent::TabDropped`] on release. Escape cancels a tab drag.
//!
//! The overlay paints no text, so a host that wants it attributed in a
//! frame inventory registers [`crate::DROP_OVERLAY_ZONE_ID`] at the rect
//! [`TabGroup::drop_overlay`] reports for the group's stored state.
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
use crate::compose::tab_drag::{resolve_tab_drop, TabDragSource, TabDropInstruction};
use crate::compose::workspace::Workspace;
use crate::compose::Handled;
use crate::event::{Key, MouseButton, NamedKey, Point, Rect};
use crate::focus::FocusAddress;
use crate::interactive::{
    EventCx, FocusSpec, FocusStop, Interactive, KeyEvent, PointerKind, RoutedEvent, SubTarget,
    WidgetAction,
};
use crate::paint_surface::PaintSurface;
use crate::popup::{Popup, PopupDismiss, PopupKey, PopupSide};
use crate::primitive::{LayoutCtx, PaintEnv, Primitive, Size, Units};
use crate::primitives::drop_zone::{
    drop_zone_hit_test, drop_zone_overlay, DropEdge, DropGroupRect, DropOverlay, DropZone,
    DropZoneHit,
};
use crate::primitives::split::SplitDirection;
use crate::primitives::split::SplitTreeState;
use crate::primitives::split_tree::{SplitTree, SplitTreeLayout};
use crate::primitives::tab_bar::{
    tab_bar_sub_regions, TabBar, TabBarEvent, TabBarPrimitiveLayout, TabBarSegment, TabBarState,
    TabBarSubPart, TabItem,
};
use crate::types::WidgetId;
use crate::InteractionState;

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
/// the tab — `()` for a bare descriptor — and travels with the tab when a
/// drag moves it to another pane.
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

/// What a [`TabGroup`] reports, and what [`TabGroup::apply`] answers
/// after applying it.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interactive::EventHost;
    use crate::primitives::tab_bar::{SegmentMeasure, TabMeasure};
    use crate::types::Modifiers;

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn tab(id: &str, label: &str, closable: bool) -> TabGroupTab {
        TabGroupTab {
            closable,
            ..TabGroupTab::new(id, label)
        }
    }

    fn one_pane(pane_id: &str, tabs: Vec<TabGroupTab>, active: &str) -> TabGroup {
        TabGroup::with_pane(
            "tg",
            TabGroupPane::new(pane_id, tabs, active),
            SplitDirection::Horizontal,
        )
    }

    fn make_group() -> TabGroup {
        one_pane(
            "pane:0",
            vec![tab("t0", "main.rs", true), tab("t1", "lib.rs", true)],
            "t0",
        )
    }

    /// Open `tab` in a new pane after the last one; returns its index.
    fn add_pane(group: &mut TabGroup, pane_id: &str, tab: TabGroupTab) -> usize {
        let id = tab.id.clone();
        let last = group.pane_count() - 1;
        group.add_pane_after(last, TabGroupPane::new(pane_id, vec![tab], id))
    }

    /// A host for one group, as an app is: routes pointer and key input
    /// through the group's own [`Interactive::handle`] against its layout,
    /// keeps the pane focus the group asks for, and writes every intent
    /// back with [`TabGroup::apply`].
    struct Host {
        group: TabGroup,
        state: TabGroupState,
        host: TestHost,
        layout: Option<TabGroupPrimitiveLayout>,
        /// The rect a real layout fills, re-laid out after every change;
        /// `None` while the layout is primed by hand.
        area: Option<Rect>,
    }

    impl Host {
        fn new(group: TabGroup) -> Self {
            Self {
                group,
                state: TabGroupState::default(),
                host: TestHost::default(),
                layout: None,
                area: None,
            }
        }

        /// Lay the group out on a `width` x `height` cell grid at the
        /// origin.
        fn lay_out_cells(&mut self, width: f32, height: f32) {
            self.area = Some(Rect::new(0.0, 0.0, width, height));
            self.relayout();
        }

        fn relayout(&mut self) {
            if let Some(area) = self.area {
                let ctx = LayoutCtx::new(1.0, 1.0, false);
                self.layout = Some(Primitive::layout(&self.group, &ctx, area));
            }
        }

        fn layout(&self) -> &TabGroupPrimitiveLayout {
            self.layout.as_ref().expect("laid out")
        }

        fn focused_pane(&self) -> Option<usize> {
            match self
                .host
                .focus
                .flatten()
                .map(TabGroupSubPart::from_sub_target)
            {
                Some(Some(TabGroupSubPart::Pane(k))) => Some(k),
                _ => None,
            }
        }

        fn focus_pane(&mut self, k: usize) {
            self.host.focus = Some(Some(TabGroupSubPart::Pane(k).sub_target()));
        }

        /// Route `event`: [`Handled::Event`] with the changes the intent
        /// made once applied, [`Handled::Consumed`] when the group claimed
        /// it but nothing changed, [`Handled::Ignored`] when it was not
        /// the group's.
        fn route(&mut self, event: RoutedEvent) -> Handled<Vec<TabGroupEvent>> {
            let Some(layout) = self.layout.as_ref() else {
                return Handled::Ignored;
            };
            let handled = {
                let mut cx = EventCx::new(self.group.id, &mut self.host);
                Interactive::handle(&self.group, &mut self.state, &event, layout, &mut cx)
            };
            let claimed = !handled.is_ignored();
            let events = match handled.into_event() {
                Some(intent) => self.group.apply(intent),
                None => Vec::new(),
            };
            self.relayout();
            match (claimed, events.is_empty()) {
                (_, false) => Handled::Event(events),
                (true, true) => Handled::Consumed,
                (false, true) => Handled::Ignored,
            }
        }

        fn pointer(&mut self, kind: PointerKind, x: f32, y: f32) -> Handled<Vec<TabGroupEvent>> {
            let origin = self
                .layout
                .as_ref()
                .map_or(Point::new(0.0, 0.0), |l| l.origin());
            self.route(RoutedEvent::Pointer {
                kind,
                sub: None,
                local: Point::new(x - origin.x, y - origin.y),
                mods: Modifiers::default(),
            })
        }

        fn press(&mut self, x: f32, y: f32) -> Handled<Vec<TabGroupEvent>> {
            self.pointer(PointerKind::Down(MouseButton::Left), x, y)
        }

        fn move_to(&mut self, x: f32, y: f32) -> Handled<Vec<TabGroupEvent>> {
            self.pointer(PointerKind::Move, x, y)
        }

        fn release(&mut self, x: f32, y: f32) -> Handled<Vec<TabGroupEvent>> {
            self.pointer(PointerKind::Up(MouseButton::Left), x, y)
        }

        /// A press and release in place, answered with the first change
        /// the press made.
        fn click(&mut self, x: f32, y: f32) -> Handled<TabGroupEvent> {
            let pressed = self.press(x, y);
            let _ = self.release(x, y);
            match pressed {
                Handled::Event(events) => Handled::Event(events[0].clone()),
                Handled::Consumed => Handled::Consumed,
                Handled::Ignored => Handled::Ignored,
            }
        }

        /// A tab dragged from `from` and dropped at `to`, answered with
        /// what the drop changed.
        fn drag_tab(&mut self, from: (f32, f32), to: (f32, f32)) -> Vec<TabGroupEvent> {
            let _ = self.press(from.0, from.1);
            let _ = self.move_to(to.0, to.1);
            self.release(to.0, to.1).into_event().unwrap_or_default()
        }

        fn escape(&mut self) -> Handled<Vec<TabGroupEvent>> {
            self.route(RoutedEvent::Key(KeyEvent {
                key: Key::Named(NamedKey::Escape),
                modifiers: Modifiers::default(),
                repeat: false,
            }))
        }

        fn drag(&self) -> Option<&TabDrag> {
            self.state.tab_drag.as_ref()
        }

        /// The cell column (or row) divider `idx` sits at.
        fn divider_cell(&self, idx: usize) -> u16 {
            self.layout().dividers.dividers[idx].cell_position()
        }
    }

    /// Prime `h`'s layout for `pane_idx` by hand, without a real layout
    /// pass. Each tab is given `tab_w` cells (last `close_w` cells = close
    /// region). The "new-tab" right segment is given 3 cells. Panes are
    /// primed in index order.
    fn prime_pane(
        h: &mut Host,
        pane_idx: usize,
        strip_x: f32,
        strip_y: f32,
        bar_w: f32,
        tab_w: f32,
        close_w: f32,
    ) {
        let Some(pane) = h.group.panes.get(pane_idx) else {
            return;
        };
        let bar = h.group.pane_tab_bar(pane_idx, 0);
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
        let group_id = h.group.id;
        h.area = None;
        let cached = h.layout.get_or_insert_with(|| TabGroupPrimitiveLayout {
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

    // ── Construction ─────────────────────────────────────────────────────────

    #[test]
    fn with_pane_has_one_pane_with_tabs() {
        let group = make_group();
        assert_eq!(group.pane_count(), 1);
        assert_eq!(group.panes[0].tabs().len(), 2);
        assert_eq!(group.panes[0].active_tab_id(), "t0");
        assert_eq!(Host::new(group).focused_pane(), None); // starts unfocused
    }

    // ── from_layout ───────────────────────────────────────────────────────────

    #[test]
    fn from_layout_single_pane() {
        let pane = TabGroupPane::new("p0", vec![tab("t0", "main.rs", true)], "t0");
        let group = TabGroup::from_layout(
            "tg",
            vec![pane],
            GroupLayout::Leaf(0),
            SplitDirection::Horizontal,
        )
        .expect("valid single-pane layout");
        assert_eq!(group.pane_count(), 1);
        assert_eq!(group.layout(), GroupLayout::Leaf(0));
        assert_eq!(group.panes[0].active_tab_id(), "t0");
    }

    #[test]
    fn from_layout_two_pane_horizontal() {
        let panes = vec![
            TabGroupPane::new("p0", vec![tab("t0", "a.rs", true)], "t0"),
            TabGroupPane::new("p1", vec![tab("t1", "b.rs", true)], "t1"),
        ];
        let layout = GroupLayout::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.4,
            first: Box::new(GroupLayout::Leaf(0)),
            second: Box::new(GroupLayout::Leaf(1)),
        };
        let group = TabGroup::from_layout("tg", panes, layout.clone(), SplitDirection::Vertical)
            .expect("valid two-pane layout");
        assert_eq!(group.pane_count(), 2);
        assert_eq!(group.layout(), layout);
        assert_eq!(group.default_split_direction, SplitDirection::Vertical);
        // Spot-check pane ids.
        assert_eq!(group.panes[0].id, "p0");
        assert_eq!(group.panes[1].id, "p1");
    }

    fn three_pane_layout() -> GroupLayout {
        GroupLayout::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(GroupLayout::Leaf(0)),
            second: Box::new(GroupLayout::Split {
                direction: SplitDirection::Vertical,
                ratio: 0.5,
                first: Box::new(GroupLayout::Leaf(1)),
                second: Box::new(GroupLayout::Leaf(2)),
            }),
        }
    }

    fn three_panes() -> Vec<TabGroupPane> {
        vec![
            TabGroupPane::new("left", vec![tab("tl", "left.rs", true)], "tl"),
            TabGroupPane::new("top-right", vec![tab("tr", "top.rs", true)], "tr"),
            TabGroupPane::new("bottom-right", vec![tab("tb", "bottom.rs", true)], "tb"),
        ]
    }

    #[test]
    fn from_layout_three_pane_mixed_direction() {
        // Left pane | (top-right pane / bottom-right pane)
        // Root split is Horizontal; the right sub-tree is Vertical.
        let group = TabGroup::from_layout(
            "tg",
            three_panes(),
            three_pane_layout(),
            SplitDirection::Horizontal,
        )
        .expect("valid mixed-direction layout");

        assert_eq!(group.pane_count(), 3);
        assert_eq!(group.layout().leaf_count(), 3);
        assert!(leaves(&group.layout()).contains(&0));
        assert!(leaves(&group.layout()).contains(&1));
        assert!(leaves(&group.layout()).contains(&2));

        // Confirm the root split is Horizontal and the nested split is Vertical.
        match &group.layout() {
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
            TabGroupPane::new("p0", vec![tab("t0", "a.rs", false)], "t0"),
            TabGroupPane::new("p1", vec![tab("t1", "b.rs", false)], "t1"),
        ];
        let layout = GroupLayout::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.3,
            first: Box::new(GroupLayout::Leaf(0)),
            second: Box::new(GroupLayout::Leaf(1)),
        };
        let group =
            TabGroup::from_layout("tg", panes, layout, SplitDirection::Horizontal).expect("valid");
        let ratio = match &group.layout() {
            GroupLayout::Split { ratio, .. } => *ratio,
            _ => panic!("expected Split"),
        };
        assert!((ratio - 0.3).abs() < 1e-6, "ratio={ratio}");
    }

    #[test]
    fn from_layout_add_pane_after_construction() {
        // A from_layout group behaves correctly with later add_pane_after
        // calls.
        let panes = vec![
            TabGroupPane::new("p0", vec![tab("t0", "a.rs", true)], "t0"),
            TabGroupPane::new("p1", vec![tab("t1", "b.rs", true)], "t1"),
        ];
        let layout = GroupLayout::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(GroupLayout::Leaf(0)),
            second: Box::new(GroupLayout::Leaf(1)),
        };
        let mut group =
            TabGroup::from_layout("tg", panes, layout, SplitDirection::Horizontal).expect("valid");
        assert_eq!(group.pane_count(), 2);

        let new_idx = add_pane(&mut group, "extra", tab("te", "extra.rs", false));
        assert_eq!(group.pane_count(), 3);
        assert_eq!(group.layout().leaf_count(), 3);
        assert_eq!(group.panes[new_idx].id, "extra");
    }

    // ── from_layout: error cases ──────────────────────────────────────────────

    #[test]
    fn from_layout_error_empty_panes() {
        let result = TabGroup::<()>::from_layout(
            "tg",
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
        let panes = vec![TabGroupPane::new(
            "p0",
            vec![tab("t0", "a.rs", false)],
            "t0",
        )];
        // Tree has 2 leaves but only 1 pane provided.
        let layout = GroupLayout::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(GroupLayout::Leaf(0)),
            second: Box::new(GroupLayout::Leaf(1)),
        };
        let result = TabGroup::from_layout("tg", panes, layout, SplitDirection::Horizontal);
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
            TabGroupPane::new("p0", vec![tab("t0", "a.rs", false)], "t0"),
            TabGroupPane::new("p1", vec![tab("t1", "b.rs", false)], "t1"),
        ];
        // Both leaves reference index 0.
        let layout = GroupLayout::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(GroupLayout::Leaf(0)),
            second: Box::new(GroupLayout::Leaf(0)),
        };
        let result = TabGroup::from_layout("tg", panes, layout, SplitDirection::Horizontal);
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
            TabGroupPane::new("p0", vec![tab("t0", "a.rs", false)], "t0"),
            TabGroupPane::new("p1", vec![tab("t1", "b.rs", false)], "t1"),
        ];
        // Leaf index 2 is out of range for 2 panes (valid range: 0..2).
        let layout = GroupLayout::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(GroupLayout::Leaf(0)),
            second: Box::new(GroupLayout::Leaf(2)),
        };
        let result = TabGroup::from_layout("tg", panes, layout, SplitDirection::Horizontal);
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
        let mut group = make_group();
        assert!(group.switch_tab(0, "t1"));
        assert_eq!(group.panes[0].active_tab_id(), "t1");
    }

    #[test]
    fn switch_tab_unknown_returns_false() {
        let mut group = make_group();
        assert!(!group.switch_tab(0, "no-such"));
        assert_eq!(group.panes[0].active_tab_id(), "t0"); // unchanged
    }

    // ── Add tab ───────────────────────────────────────────────────────────────

    #[test]
    fn add_tab_appends_without_activating() {
        let mut group = make_group();
        assert!(group.add_tab(0, tab("t2", "new.rs", true)));
        assert_eq!(group.panes[0].tabs().len(), 3);
        assert_eq!(group.panes[0].active_tab_id(), "t0"); // unchanged
        assert!(group.switch_tab(0, "t2"), "the added tab can be activated");
        assert_eq!(group.panes[0].active_tab_id(), "t2");
        assert!(!group.add_tab(5, tab("t3", "x.rs", true)), "no pane 5");
    }

    // ── Close tab ─────────────────────────────────────────────────────────────

    #[test]
    fn close_inactive_tab_emits_tab_closed() {
        let mut group = make_group();
        let ev = group.close_tab(0, "t1");
        assert_eq!(
            ev,
            Some(TabGroupEvent::TabClosed {
                pane_idx: 0,
                tab_id: "t1".into()
            })
        );
        assert_eq!(group.panes[0].tabs().len(), 1);
        assert_eq!(group.panes[0].active_tab_id(), "t0");
    }

    #[test]
    fn close_active_tab_switches_to_previous() {
        let mut group = make_group();
        group.switch_tab(0, "t1"); // make t1 active
        let ev = group.close_tab(0, "t1");
        assert_eq!(
            ev,
            Some(TabGroupEvent::TabClosed {
                pane_idx: 0,
                tab_id: "t1".into()
            })
        );
        assert_eq!(group.panes[0].active_tab_id(), "t0"); // fell back to t0
    }

    #[test]
    fn close_unknown_tab_returns_none() {
        let mut group = make_group();
        assert_eq!(group.close_tab(0, "ghost"), None);
    }

    // ── Close-last-tab collapses the pane ─────────────────────────────────────

    #[test]
    fn close_last_tab_in_solo_pane_does_not_collapse() {
        let mut group = one_pane("p0", vec![tab("t0", "only", true)], "t0");
        let ev = group.close_tab(0, "t0");
        assert_eq!(ev, Some(TabGroupEvent::PaneCollapsed { pane_idx: 0 }));
        // The group retains 1 pane (collapse_pane is a no-op at len==1).
        assert_eq!(group.pane_count(), 1);
    }

    #[test]
    fn close_last_tab_in_second_pane_collapses_it() {
        let mut group = make_group();
        let idx = add_pane(&mut group, "p1", tab("x0", "x.rs", true));
        assert_eq!(group.pane_count(), 2);

        let ev = group.close_tab(idx, "x0");
        assert_eq!(ev, Some(TabGroupEvent::PaneCollapsed { pane_idx: idx }));
        assert_eq!(group.pane_count(), 1);
        // Layout should be a single Leaf.
        assert!(matches!(group.layout(), GroupLayout::Leaf(0)));
    }

    // ── Pane lifecycle ────────────────────────────────────────────────────────

    #[test]
    fn add_pane_after_creates_second_pane() {
        let mut group = make_group();
        let idx = add_pane(&mut group, "p1", tab("x0", "x.rs", true));
        assert_eq!(idx, 1);
        assert_eq!(group.pane_count(), 2);
        // Layout should be a Split at root.
        assert!(matches!(group.layout(), GroupLayout::Split { .. }));
        // Tree has exactly 2 leaves.
        assert_eq!(group.layout().leaf_count(), 2);
    }

    #[test]
    fn add_pane_after_clamps_an_out_of_range_target() {
        let mut group = make_group();
        let idx = group.add_pane_after(
            9,
            TabGroupPane::new("p1", vec![tab("x0", "x.rs", true)], "x0"),
        );
        assert_eq!(idx, 1, "a target past the end opens after the last pane");
    }

    #[test]
    fn three_panes_cover_three_leaves() {
        let mut group = make_group();
        add_pane(&mut group, "p1", tab("x0", "x.rs", false));
        add_pane(&mut group, "p2", tab("y0", "y.rs", false));
        assert_eq!(group.pane_count(), 3);
        // All 3 pane indices appear as leaves.
        assert_eq!(group.layout().leaf_count(), 3);
    }

    // ── Clicks on the strip ──────────────────────────────────────────────────

    #[test]
    fn click_inactive_tab_emits_tab_activated() {
        let mut h = Host::new(make_group());
        prime_pane(&mut h, 0, 0.0, 0.0, 80.0, 8.0, 2.0);

        let ev = h.click(9.0, 0.5); // x=9 → t1 body
        assert_eq!(
            ev,
            Handled::Event(TabGroupEvent::TabActivated {
                pane_idx: 0,
                tab_id: "t1".into()
            })
        );
        assert_eq!(h.group.panes[0].active_tab_id(), "t1");
    }

    #[test]
    fn click_active_tab_returns_consumed() {
        let mut h = Host::new(make_group());
        prime_pane(&mut h, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        assert_eq!(h.click(3.0, 0.5), Handled::Consumed);
    }

    #[test]
    fn click_close_button_emits_tab_closed() {
        let mut h = Host::new(make_group());
        prime_pane(&mut h, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        let ev = h.click(7.0, 0.5);
        assert_eq!(
            ev,
            Handled::Event(TabGroupEvent::TabClosed {
                pane_idx: 0,
                tab_id: "t0".into()
            })
        );
        assert_eq!(h.group.panes[0].tabs().len(), 1);
    }

    #[test]
    fn click_new_tab_button_emits_new_tab_requested() {
        let mut h = Host::new(make_group());
        prime_pane(&mut h, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        let ev = h.click(78.0, 0.5);
        assert_eq!(
            ev,
            Handled::Event(TabGroupEvent::NewTabRequested { pane_idx: 0 })
        );
    }

    #[test]
    fn a_strip_click_focuses_its_pane() {
        let mut group = make_group();
        add_pane(&mut group, "p1", tab("x0", "x.rs", false));
        let mut h = Host::new(group);
        prime_pane(&mut h, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        prime_pane(&mut h, 1, 80.0, 0.0, 80.0, 8.0, 2.0);
        h.focus_pane(0);
        let _ = h.click(83.0, 0.5);
        assert_eq!(h.focused_pane(), Some(1));
    }

    // ── Clicks in a pane body ─────────────────────────────────────────────────

    #[test]
    fn click_content_area_focuses_pane() {
        let mut group = make_group();
        add_pane(&mut group, "p1", tab("x0", "x.rs", false));
        let mut h = Host::new(group);
        h.focus_pane(0);
        prime_pane(&mut h, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        prime_pane(&mut h, 1, 80.0, 0.0, 80.0, 8.0, 2.0);

        let ev = h.click(85.0, 5.0);
        assert_eq!(
            ev,
            Handled::Event(TabGroupEvent::PaneFocused { pane_idx: 1 })
        );
        assert_eq!(h.focused_pane(), Some(1));
    }

    /// A press in the body of the pane that already has focus is the
    /// host's: the group leaves it for the pane's content.
    #[test]
    fn click_already_focused_content_is_left_for_the_host() {
        let mut h = Host::new(make_group());
        prime_pane(&mut h, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        h.focus_pane(0);
        assert_eq!(h.click(5.0, 5.0), Handled::Ignored);
        assert_eq!(h.focused_pane(), Some(0));
    }

    #[test]
    fn click_outside_every_pane_returns_ignored() {
        let mut h = Host::new(make_group());
        prime_pane(&mut h, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        // Far outside the one primed pane's strip/content bounds entirely.
        assert_eq!(h.click(500.0, 500.0), Handled::Ignored);
    }

    #[test]
    fn click_with_offset_strip_resolves_correctly() {
        let mut h = Host::new(make_group());
        prime_pane(&mut h, 0, 20.0, 5.0, 80.0, 8.0, 2.0);

        let ev = h.click(30.0, 5.5);
        assert_eq!(
            ev,
            Handled::Event(TabGroupEvent::TabActivated {
                pane_idx: 0,
                tab_id: "t1".into()
            })
        );
    }

    // ── Divider drag ──────────────────────────────────────────────────────────

    fn two_cell_panes(width: f32) -> Host {
        let mut group = make_group();
        add_pane(&mut group, "p1", tab("x0", "x.rs", false));
        let mut h = Host::new(group);
        h.lay_out_cells(width, 20.0);
        h
    }

    #[test]
    fn a_press_off_the_dividers_starts_no_divider_drag() {
        let mut h = two_cell_panes(80.0);
        let _ = h.press(10.0, 10.0);
        assert_eq!(h.state.dragging_divider(), None);
    }

    #[test]
    fn a_press_on_a_divider_starts_its_drag() {
        let mut h = two_cell_panes(80.0);
        let x = f32::from(h.divider_cell(0));
        assert_eq!(h.press(x, 10.0), Handled::Consumed);
        assert_eq!(h.state.dragging_divider(), Some(0));
        assert!(!h.state.is_tab_dragging());
    }

    #[test]
    fn drag_move_updates_split_ratio() {
        let mut h = two_cell_panes(100.0);
        let x = f32::from(h.divider_cell(0));
        let _ = h.press(x, 5.0);

        let ev = h.move_to(30.0, 5.0);
        let Handled::Event(events) = ev else {
            panic!("expected a resize, got {ev:?}");
        };
        let [TabGroupEvent::DividerResized { divider_idx, ratio }] = events.as_slice() else {
            panic!("expected one resize, got {events:?}");
        };
        assert_eq!(*divider_idx, 0);
        assert_eq!(h.group.divider_ratio(0), Some(*ratio));
        assert!(
            (ratio - 0.30).abs() < 0.02,
            "ratio={ratio}, expected ≈ 0.30"
        );
    }

    /// The ratio a drag reports is the exact inverse of the split's own
    /// layout (divider thickness included), so after the next layout the
    /// divider sits on the cell the pointer was dragged to — not one cell
    /// short of it, as `x / width` would place it.
    #[test]
    fn divider_drag_lands_the_divider_under_the_pointer() {
        let mut h = two_cell_panes(120.0);
        let x = f32::from(h.divider_cell(0));
        let _ = h.press(x, 5.0);
        assert!(h.move_to(100.0, 5.0).is_event());
        let _ = h.release(100.0, 5.0);
        assert_eq!(h.divider_cell(0), 100);
    }

    /// Pre-order: `divider_idx` 0 is the *outer* (root) split, matching
    /// `SplitTree::set_ratio_at_index`'s numbering.
    #[test]
    fn drag_move_targets_the_preorder_indexed_split_not_the_render_sibling() {
        let group = TabGroup::from_layout(
            "tg",
            three_panes(),
            three_pane_layout(),
            SplitDirection::Horizontal,
        )
        .expect("valid mixed-direction layout");
        let mut h = Host::new(group);
        h.lay_out_cells(100.0, 20.0);

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
        let root_x = f32::from(h.divider_cell(0));
        let _ = h.press(root_x, 5.0);
        assert_eq!(h.state.dragging_divider(), Some(0));
        let ev = h.move_to(30.0, 5.0);
        assert!(matches!(
            ev,
            Handled::Event(ref events)
                if matches!(events.as_slice(), [TabGroupEvent::DividerResized { divider_idx: 0, .. }])
        ));
        let (root, nested) = ratios(&h.group.layout());
        assert!(
            (root - 0.30).abs() < 0.02,
            "root ratio={root}, expected ≈0.30"
        );
        assert!(
            (nested - 0.5).abs() < 1e-6,
            "nested ratio must be untouched"
        );
        let _ = h.release(30.0, 5.0);

        // Drag divider 1 (the nested split) — only the nested ratio moves.
        let nested_y = f32::from(h.divider_cell(1));
        let _ = h.press(60.0, nested_y);
        assert_eq!(h.state.dragging_divider(), Some(1));
        let ev = h.move_to(60.0, 15.0);
        assert!(matches!(
            ev,
            Handled::Event(ref events)
                if matches!(events.as_slice(), [TabGroupEvent::DividerResized { divider_idx: 1, .. }])
        ));
        let (root_after, nested_after) = ratios(&h.group.layout());
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
    fn a_release_ends_the_divider_drag() {
        let mut h = two_cell_panes(80.0);
        let x = f32::from(h.divider_cell(0));
        let _ = h.press(x, 5.0);
        assert_eq!(h.release(x, 5.0), Handled::Consumed);
        assert_eq!(h.state.dragging_divider(), None);
    }

    #[test]
    fn a_move_with_no_gesture_is_ignored() {
        let mut h = two_cell_panes(100.0);
        assert_eq!(h.move_to(30.0, 5.0), Handled::Ignored);
    }

    // ── Drop targets ──────────────────────────────────────────────────────────

    #[test]
    fn drop_groups_cover_every_laid_out_pane() {
        let mut h = Host::new(make_group());
        prime_pane(&mut h, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        let rects = h.layout().drop_groups();
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].bounds.width, 80.0);
        assert_eq!(rects[0].bounds.height, 11.0, "strip plus body");
        assert_eq!(rects[0].tab_slots, vec![(0.0, 8.0), (8.0, 16.0)]);
    }

    // ── Pane fractions / layout tree invariants ───────────────────────────────

    #[test]
    fn collapse_pane_reduces_leaf_count() {
        let mut group = make_group();
        add_pane(&mut group, "p1", tab("x0", "x.rs", false));
        add_pane(&mut group, "p2", tab("y0", "y.rs", false));
        assert_eq!(group.pane_count(), 3);
        assert_eq!(group.layout().leaf_count(), 3);

        group.collapse_pane(1);
        assert_eq!(group.pane_count(), 2);
        assert_eq!(group.layout().leaf_count(), 2);
    }

    #[test]
    fn layout_after_two_adds_is_correct() {
        let mut group = make_group();
        add_pane(&mut group, "p1", tab("x0", "x.rs", false));
        add_pane(&mut group, "p2", tab("y0", "y.rs", false));
        // All three leaf indices present.
        assert!(leaves(&group.layout()).contains(&0));
        assert!(leaves(&group.layout()).contains(&1));
        assert!(leaves(&group.layout()).contains(&2));
        assert_eq!(group.layout().leaf_count(), 3);
    }

    /// Build a two-pane host with:
    ///   pane 0 at x=0  : tabs [t0, t1], strip height=1, bar_w=80
    ///   pane 1 at x=80 : tabs [x0],     strip height=1, bar_w=80
    fn make_two_pane() -> Host {
        let mut group = make_group();
        add_pane(&mut group, "p1", tab("x0", "x.rs", true));
        two_primed(group)
    }

    fn two_primed(group: TabGroup) -> Host {
        let mut h = Host::new(group);
        h.focus_pane(0);
        prime_pane(&mut h, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        prime_pane(&mut h, 1, 80.0, 0.0, 80.0, 8.0, 2.0);
        h
    }

    /// A lone tab in pane 0, `x0` in pane 1, both primed.
    fn lone_tab_two_pane() -> Host {
        let mut group = one_pane("p0", vec![tab("only", "only.rs", true)], "only");
        add_pane(&mut group, "p1", tab("x0", "x.rs", true));
        two_primed(group)
    }

    #[test]
    fn top_bottom_edge_drops_create_vertical_splits() {
        // A tab dropped on a Top or Bottom edge makes a split whose
        // direction is Vertical.
        let mut h = make_two_pane();
        // Content top for pane 1 = 1.0 (strip height); its top edge band
        // ends 3 rows below.
        let evs = h.drag_tab((9.0, 0.5), (120.0, 2.5));
        assert!(
            matches!(
                evs.first(),
                Some(TabGroupEvent::TabSplitToNewPane {
                    edge: DropEdge::Top,
                    ..
                })
            ),
            "expected a Top-edge split, got {evs:?}"
        );

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
            find_any_vertical(&h.group.layout()),
            "expected a Vertical split in tree after Top-edge drop"
        );
    }

    // ── Arming a tab drag ─────────────────────────────────────────────────────

    #[test]
    fn a_press_on_a_tab_body_arms_a_drag_of_that_tab() {
        let mut h = make_two_pane();
        let _ = h.press(9.0, 0.5);
        assert!(h.state.is_tab_dragging());
        let drag = h.drag().unwrap();
        assert_eq!(drag.pane_idx, 0);
        assert_eq!(drag.tab_id, "t1");
        assert!(!drag.active, "a press alone is not yet a drag");
        assert!(h.host.captured, "the group captures the pointer");
    }

    #[test]
    fn a_press_on_a_close_button_arms_no_drag() {
        let mut h = make_two_pane();
        let _ = h.press(7.0, 0.5);
        assert!(!h.state.is_tab_dragging());
    }

    #[test]
    fn a_press_on_the_new_tab_button_arms_no_drag() {
        let mut h = make_two_pane();
        let _ = h.press(78.0, 0.5);
        assert!(!h.state.is_tab_dragging());
    }

    #[test]
    fn a_press_in_a_pane_body_arms_no_drag() {
        let mut h = make_two_pane();
        let _ = h.press(40.0, 5.0);
        assert!(!h.state.is_tab_dragging());
    }

    #[test]
    fn a_press_outside_every_pane_arms_no_drag() {
        let mut h = make_two_pane();
        assert_eq!(h.press(999.0, 0.5), Handled::Ignored);
        assert!(!h.state.is_tab_dragging());
    }

    // ── Moving a tab drag + its overlay ───────────────────────────────────────

    #[test]
    fn a_move_without_a_drag_resolves_no_zone() {
        let mut h = make_two_pane();
        let _ = h.move_to(120.0, 5.0);
        assert!(h.state.drop_zone().is_none());
    }

    #[test]
    fn a_drag_move_resolves_and_stores_the_drop_zone() {
        let mut h = make_two_pane();
        let _ = h.press(9.0, 0.5);
        let _ = h.move_to(120.0, 5.0);
        assert_eq!(
            h.state.drop_zone(),
            Some(&crate::DropZone {
                kind: crate::DropZoneKind::Center,
                group_idx: 1,
            })
        );
        assert!(h.drag().is_some_and(|d| d.active));
    }

    #[test]
    fn a_move_inside_the_threshold_keeps_the_press_a_click() {
        let mut h = make_two_pane();
        let _ = h.press(9.0, 0.5);
        let _ = h.move_to(9.5, 0.5);
        assert!(h.drag().is_some_and(|d| !d.active));
        assert!(h.state.drop_zone().is_none());
    }

    #[test]
    fn no_overlay_before_the_drag_moves() {
        let mut h = make_two_pane();
        let _ = h.press(9.0, 0.5);
        assert!(h.group.drop_overlay(&h.state, h.layout()).is_none());
    }

    #[test]
    fn the_overlay_highlights_the_hovered_pane() {
        let mut h = make_two_pane();
        let _ = h.press(9.0, 0.5);
        let _ = h.move_to(120.0, 5.0);
        let ov = h.group.drop_overlay(&h.state, h.layout()).unwrap();
        assert_eq!(ov.highlight, Some(crate::Rect::new(80.0, 1.0, 80.0, 10.0)));
        assert!(ov.insertion_bar.is_none());
        let popups = Interactive::popups(&h.group, &h.state, h.layout());
        assert_eq!(popups.len(), 1, "the overlay shows as one popup");
    }

    #[test]
    fn escape_cancels_a_tab_drag() {
        let mut h = make_two_pane();
        let _ = h.press(9.0, 0.5);
        assert!(h.state.is_tab_dragging());
        assert_eq!(h.escape(), Handled::Consumed);
        assert!(!h.state.is_tab_dragging());
        assert!(!h.host.captured);
        assert_eq!(h.escape(), Handled::Ignored, "no drag left to cancel");
    }

    // ── drop_zone_at ──────────────────────────────────────────────────────────

    #[test]
    fn drop_zone_at_center_of_pane_1() {
        let h = make_two_pane();
        let zone = h.layout().drop_zone_at(Point::new(120.0, 5.0)).unwrap();
        assert_eq!(zone.group_idx, 1);
        assert_eq!(zone.kind, crate::DropZoneKind::Center);
    }

    #[test]
    fn drop_zone_at_tab_bar_of_pane_1_reorder() {
        let h = make_two_pane();
        let zone = h.layout().drop_zone_at(Point::new(82.0, 0.5)).unwrap();
        assert_eq!(zone.group_idx, 1);
        assert!(matches!(zone.kind, crate::DropZoneKind::TabReorder(_)));
    }

    // ── Dropping: reorder within the same pane ────────────────────────────────

    #[test]
    fn tab_drop_reorders_within_same_pane() {
        let mut h = make_two_pane();
        let evs = h.drag_tab((9.0, 0.5), (3.0, 0.5));
        assert_eq!(
            evs,
            vec![TabGroupEvent::TabReordered {
                pane_idx: 0,
                tab_id: "t1".into(),
                from_idx: 1,
                to_idx: 0,
            }]
        );
        assert_eq!(h.group.panes[0].tabs()[0].id, "t1");
        assert_eq!(h.group.panes[0].tabs()[1].id, "t0");
        assert_eq!(h.group.pane_count(), 2);
    }

    #[test]
    fn tab_drop_reorder_same_position_is_noop() {
        let mut h = make_two_pane();
        let evs = h.drag_tab((9.0, 0.5), (14.0, 0.5));
        assert!(evs.is_empty());
        assert_eq!(h.group.panes[0].tabs()[0].id, "t0");
        assert_eq!(h.group.panes[0].tabs()[1].id, "t1");
    }

    #[test]
    fn tab_drop_same_pane_center_is_noop() {
        let mut h = make_two_pane();
        let evs = h.drag_tab((9.0, 0.5), (40.0, 5.0));
        assert!(evs.is_empty());
        assert_eq!(h.group.pane_count(), 2);
        assert_eq!(h.group.panes[0].tabs().len(), 2);
    }

    // ── Dropping: merge into another pane ─────────────────────────────────────

    #[test]
    fn tab_drop_moves_tab_to_another_pane_center() {
        let mut h = make_two_pane();
        let evs = h.drag_tab((9.0, 0.5), (120.0, 5.0));
        assert_eq!(
            evs,
            vec![TabGroupEvent::TabMovedToPane {
                from_pane_idx: 0,
                to_pane_idx: 1,
                tab_id: "t1".into(),
                insert_idx: 1,
            }]
        );
        assert_eq!(h.group.pane_count(), 2);
        assert_eq!(h.group.panes[0].tabs().len(), 1);
        assert_eq!(h.group.panes[0].tabs()[0].id, "t0");
        assert_eq!(h.group.panes[0].active_tab_id(), "t0");
        assert_eq!(h.group.panes[1].tabs().len(), 2);
        assert_eq!(h.group.panes[1].tabs()[1].id, "t1");
        assert_eq!(h.group.panes[1].active_tab_id(), "t1");
        assert_eq!(h.group.layout().leaf_count(), 2);
    }

    #[test]
    fn tab_drop_moves_tab_to_another_pane_tab_bar() {
        let mut h = make_two_pane();
        let evs = h.drag_tab((9.0, 0.5), (82.0, 0.5));
        assert_eq!(
            evs,
            vec![TabGroupEvent::TabMovedToPane {
                from_pane_idx: 0,
                to_pane_idx: 1,
                tab_id: "t1".into(),
                insert_idx: 0,
            }]
        );
        assert_eq!(h.group.panes[1].tabs()[0].id, "t1");
        assert_eq!(h.group.panes[1].tabs()[1].id, "x0");
    }

    #[test]
    fn tab_drop_source_collapses_when_last_tab_moved() {
        let mut h = lone_tab_two_pane();
        let evs = h.drag_tab((3.0, 0.5), (120.0, 5.0));

        assert_eq!(h.group.pane_count(), 1);
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
        assert_eq!(h.group.panes[0].tabs().len(), 2);
        assert_eq!(h.group.panes[0].tabs()[1].id, "only");
        assert_eq!(h.group.layout().leaf_count(), 1);
    }

    // ── Dropping: split to a new pane ─────────────────────────────────────────

    #[test]
    fn tab_drop_splits_right_edge_creates_pane_after_target() {
        let mut h = make_two_pane();
        let evs = h.drag_tab((9.0, 0.5), (150.0, 5.0));
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
        assert_eq!(h.group.pane_count(), 3);
        assert_eq!(h.group.panes[0].tabs()[0].id, "t0");
        assert_eq!(h.group.panes[1].tabs()[0].id, "x0");
        assert_eq!(h.group.panes[2].tabs()[0].id, "t1");
        assert_eq!(h.group.layout().leaf_count(), 3);
    }

    #[test]
    fn tab_drop_splits_left_edge_creates_pane_before_target() {
        let mut h = make_two_pane();
        let evs = h.drag_tab((9.0, 0.5), (83.0, 5.0));
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
        assert_eq!(h.group.pane_count(), 3);
        assert_eq!(h.group.panes[0].tabs()[0].id, "t0");
        assert_eq!(h.group.panes[1].tabs()[0].id, "t1");
        assert_eq!(h.group.panes[2].tabs()[0].id, "x0");
    }

    #[test]
    fn tab_drop_split_source_collapses_when_last_tab() {
        let mut h = lone_tab_two_pane();
        let evs = h.drag_tab((3.0, 0.5), (150.0, 5.0));

        assert_eq!(h.group.pane_count(), 2);
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
        assert_eq!(h.group.panes[0].tabs()[0].id, "x0");
        assert_eq!(h.group.panes[1].tabs()[0].id, "only");
        assert_eq!(h.group.layout().leaf_count(), 2);
    }

    #[test]
    fn tab_drop_only_tab_only_pane_split_is_noop() {
        let mut h = Host::new(one_pane("p0", vec![tab("t0", "only.rs", true)], "t0"));
        prime_pane(&mut h, 0, 0.0, 0.0, 80.0, 8.0, 2.0);

        let evs = h.drag_tab((3.0, 0.5), (70.0, 5.0));
        assert!(evs.is_empty());
        assert_eq!(h.group.pane_count(), 1);
        assert_eq!(h.group.panes[0].tabs().len(), 1);
    }

    #[test]
    fn a_no_op_drop_still_ends_the_drag() {
        let mut h = make_two_pane();
        let _ = h.press(9.0, 0.5);
        let _ = h.move_to(500.0, 500.0);
        assert_eq!(
            h.release(999.0, 999.0),
            Handled::Consumed,
            "a drag was in progress and got cleared, even though the drop itself was a no-op"
        );
        assert!(!h.state.is_tab_dragging());
        assert!(!h.host.captured);
    }

    #[test]
    fn a_release_without_a_drag_is_ignored() {
        let mut h = make_two_pane();
        assert_eq!(
            h.release(150.0, 5.0),
            Handled::Ignored,
            "no tab drag was ever started"
        );
    }

    /// Self-pane edge drop with multiple tabs: pane has ≥2 tabs, drag one out
    /// to its OWN pane's edge. Source pane stays alive (no collapse), a new
    /// pane is created adjacent to it.
    #[test]
    fn tab_drop_self_pane_edge_with_multiple_tabs_splits_without_collapse() {
        let mut h = make_two_pane();
        // Pane 0 has tabs [t0, t1]; drag t1 onto pane 0's own left edge.
        let evs = h.drag_tab((9.0, 0.5), (1.0, 5.0));

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
        assert_eq!(h.group.pane_count(), 3);
        assert_eq!(h.group.layout().leaf_count(), 3);
        // The new pane (vec index 0) carries the moved tab; the old pane 0
        // (now at vec index 1) keeps its remaining tab.
        assert_eq!(h.group.panes[0].tabs().len(), 1);
        assert_eq!(h.group.panes[0].tabs()[0].id, "t1");
        assert_eq!(h.group.panes[1].tabs().len(), 1);
        assert_eq!(h.group.panes[1].tabs()[0].id, "t0");
        // The original "other pane" (was index 1) is now at index 2.
        assert_eq!(h.group.panes[2].tabs()[0].id, "x0");
    }

    // ── Unique pane IDs after repeated splits ─────────────────────────────────

    #[test]
    fn split_then_collapse_then_split_unique_ids() {
        // Drag t1 out to create a pane, collapse it, then drag a different
        // tab to create another split. The second new pane has a distinct id.
        let mut h = make_two_pane();
        let _ = h.drag_tab((9.0, 0.5), (150.0, 5.0));
        assert_eq!(h.group.pane_count(), 3);
        let first_new_id = h.group.panes[2].id.clone();

        h.group.collapse_pane(2);
        assert_eq!(h.group.pane_count(), 2);

        // Add an extra tab so pane 0 won't be emptied by the next drag.
        h.group.add_tab(0, tab("t-extra", "extra.rs", false));

        // Re-prime: pane 0 now has 2 tabs (t0, t-extra), pane 1 has 1 tab (x0).
        prime_pane(&mut h, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        prime_pane(&mut h, 1, 80.0, 0.0, 80.0, 8.0, 2.0);
        h.layout.as_mut().unwrap().panes.truncate(2);

        // Drag t-extra (slot [8..16), x=9) to right edge of pane 1.
        let evs = h.drag_tab((9.0, 0.5), (150.0, 5.0));
        assert_eq!(
            h.group.pane_count(),
            3,
            "second split should create 3 panes: {evs:?}"
        );
        let second_new_id = h.group.panes[2].id.clone();
        assert_ne!(
            second_new_id, first_new_id,
            "second split pane must have a unique id (got {second_new_id:?})"
        );
    }

    // ── resolve_tab_drop: host-owned-model adoption path ───────────────────────
    //
    // These tests call `resolve_tab_drop` directly with hand-built
    // `DropGroupRect`s and a `TabDragSource` — no `TabGroup` anywhere.
    // This is the shape a host with its own tab/pane model (e.g. vimcode's
    // `Engine::editor_groups`) uses: build `groups` from its own layout,
    // call `resolve_tab_drop`, translate the result into its own mutations
    // directly. Geometry mirrors `make_two_pane()` above so the numbers are
    // directly comparable to the routed tests.

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
    fn resolve_tab_drop_agrees_with_a_routed_drop() {
        // Cross-check: a drop routed through the group's own `Interactive`
        // and applied (the owned-model path) and `resolve_tab_drop` (the
        // host-owned-model path) reach equivalent conclusions from
        // equivalent inputs, since the former is built on the latter.
        let mut h = make_two_pane();
        let events = h.drag_tab((9.0, 0.5), (150.0, 5.0)); // split right edge of pane 1

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

    fn gui_backend() -> crate::testing::RecordingBackend {
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
        }
    }

    /// Paint `group` into `bounds` through the widget contract, as a
    /// host's `Surface::Widget` does, and read back the layout it painted.
    fn paint(
        group: &TabGroup,
        backend: &mut crate::testing::RecordingBackend,
        bounds: Rect,
    ) -> TabGroupPrimitiveLayout {
        use crate::backend::DrawsPrimitives;
        backend
            .draw_dyn(bounds, group, &())
            .downcast_ref::<TabGroupPrimitiveLayout>()
            .cloned()
            .expect("a tab group paints its own layout")
    }

    #[test]
    fn strip_height_uses_style_tab_height_not_chrome_line_height() {
        let mut backend = gui_backend();
        let layout = paint(
            &make_group(),
            &mut backend,
            Rect::new(0.0, 0.0, 800.0, 600.0),
        );

        // The strip height is `Style::tab_height` (35, the default) — not
        // the editor line_height (20) nor the chrome line height (18).
        assert_eq!(layout.panes.len(), 1);
        assert_eq!(layout.panes[0].strip_bounds.height, 35.0);
        // Content area starts after the strip.
        assert_eq!(layout.panes[0].content_bounds.y, 35.0);
        assert_eq!(layout.panes[0].content_bounds.height, 600.0 - 35.0);
    }

    /// `Backend::set_style` overriding `tab_height` changes the painted
    /// strip height — the acceptance bar for this chrome metric.
    #[test]
    fn strip_height_follows_a_set_style_override() {
        let mut backend = gui_backend();
        crate::Backend::set_style(&mut backend, crate::Style::default().with_tab_height(50.0));
        let layout = paint(
            &make_group(),
            &mut backend,
            Rect::new(0.0, 0.0, 800.0, 600.0),
        );
        assert_eq!(layout.panes[0].strip_bounds.height, 50.0);
    }

    /// On a cell grid (TUI), the strip stays exactly one row tall —
    /// `Style::tab_height` is a documented no-op there, even with an
    /// override set.
    #[test]
    fn strip_height_ignores_style_tab_height_on_a_cell_grid() {
        let mut backend = crate::testing::RecordingBackend::new();
        crate::Backend::set_style(&mut backend, crate::Style::default().with_tab_height(50.0));
        let layout = paint(&make_group(), &mut backend, Rect::new(0.0, 0.0, 80.0, 24.0));
        assert_eq!(layout.panes[0].strip_bounds.height, 1.0);
    }

    // ── paint -> click round-trip through the painted strip layout ────────

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
            (gui_backend(), Rect::new(0.0, 0.0, 800.0, 600.0))
        }
    }

    /// A host routing against the layout `group` painted on `backend`.
    fn painted_host(
        group: TabGroup,
        backend: &mut crate::testing::RecordingBackend,
        bounds: Rect,
    ) -> Host {
        let layout = paint(&group, backend, bounds);
        let mut h = Host::new(group);
        h.layout = Some(layout);
        h
    }

    /// The painted layout resolves the strip's tabs, so clicking the
    /// second tab's own painted x-range (recovered through `drop_groups`,
    /// which re-absolutises that same layout) activates that tab — the
    /// absolute/strip-relative conversion on both sides has to agree for
    /// this to land.
    #[test]
    fn clicking_a_painted_tab_slot_activates_that_tab() {
        let (mut backend, bounds) = click_regime();
        let mut h = painted_host(make_group(), &mut backend, bounds);
        let strip = h.layout().panes[0].strip_bounds;

        let slots = h.layout().drop_groups();
        let (start, end) = slots[0].tab_slots[1];
        assert!(end > start, "tab 1 must have a painted slot: {slots:?}");

        let mid_y = strip.y + strip.height / 2.0;
        match h.click((start + end) / 2.0, mid_y) {
            Handled::Event(TabGroupEvent::TabActivated { pane_idx, tab_id }) => {
                assert_eq!(pane_idx, 0);
                assert_eq!(tab_id, "t1");
            }
            other => panic!("expected tab 1 to activate, got {other:?}"),
        }
        assert_eq!(h.group.panes[0].active_tab_id, "t1");
    }

    /// The same painted layout classifies the close glyph ahead of the tab
    /// body, so clicking it closes rather than activates.
    #[test]
    fn clicking_a_painted_close_glyph_closes_that_tab() {
        let (mut backend, bounds) = click_regime();
        let mut h = painted_host(make_group(), &mut backend, bounds);
        let strip = h.layout().panes[0].strip_bounds;
        let (cx, cy) = h.layout().panes[0]
            .tabs
            .layout
            .tab_close_center(1)
            .expect("tab 1 painted a close glyph");

        match h.click(strip.x + cx, strip.y + cy) {
            Handled::Event(TabGroupEvent::TabClosed { pane_idx, tab_id }) => {
                assert_eq!(pane_idx, 0);
                assert_eq!(tab_id, "t1");
            }
            other => panic!("expected tab 1 to close, got {other:?}"),
        }
    }

    /// A pane with far more tabs than fit the strip, whose active tab sits
    /// near the end, keeps that tab visible in the very first paint on a
    /// pixel backend: the pane's layout adopts the strip's *corrected*
    /// offset ([`crate::primitives::tab_bar::
    /// TabBarPrimitiveLayout::corrected_scroll_offset`]) within the same
    /// pass, so the strip never clips from tab 0 and hides the active tab.
    ///
    /// Forced to the pixel regime regardless of the `tui` feature (unlike
    /// [`click_regime`]): TUI's own cell-grid fit already starts the strip
    /// at an offset that shows the active tab.
    #[test]
    fn painting_an_overflowing_strip_scrolls_the_active_tab_into_view() {
        let tabs: Vec<TabGroupTab> = (0..40)
            .map(|i| tab(&format!("t{i}"), &format!("tab-label-{i}"), true))
            .collect();
        let group = one_pane("pane:0", tabs, "t39");
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
        let mut h = painted_host(group, &mut backend, bounds);

        // Forty generously-labelled tabs can't possibly fit the 800px
        // strip, so a scroll offset of `0` would leave tab 39 scrolled out
        // of view — the regression this test guards against.
        assert!(
            h.layout().panes[0].scroll_offset > 0,
            "expected a non-zero corrected scroll offset to keep tab 39 visible, got 0"
        );
        let strip = h.layout().panes[0].strip_bounds;
        let active = h.layout().panes[0].tabs.clone();
        assert!(
            active.layout.visible_tabs.iter().any(|vt| vt.tab_idx == 39),
            "tab 39 (the active one) must be among the painted visible tabs: {:?}",
            active.layout.visible_tabs
        );

        // Tab 38, scrolled into view alongside the active tab 39, is
        // clickable where it painted and activates on click — a non-active
        // tab, so the click produces a real `TabActivated` event rather
        // than `Handled::Consumed`.
        let tab_38 = active
            .layout
            .visible_tabs
            .iter()
            .find(|vt| vt.tab_idx == 38)
            .unwrap_or_else(|| {
                panic!(
                    "tab 38 must also be among the painted visible tabs: {:?}",
                    active.layout.visible_tabs
                )
            });
        let (start, end) = (
            tab_38.bounds.x + strip.x,
            tab_38.bounds.x + strip.x + tab_38.bounds.width,
        );
        let mid_y = strip.y + strip.height / 2.0;
        match h.click((start + end) / 2.0, mid_y) {
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
    #[cfg(feature = "tui")]
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

    #[cfg(feature = "tui")]
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
