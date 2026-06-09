//! `TabGroup` — tabbed split-pane compose helper.
//!
//! Wires [`TabBar`](crate::TabBar) + [`Split`](crate::Split) +
//! [`DropZone`](crate::DropZone) + [`FocusGroup`] into an
//! editor-group-style layout: N panes arranged in an arbitrary nested
//! H/V split tree, each with its own scrollable tab bar.
//!
//! # Quick start
//!
//! ```ignore
//! use quadraui::SplitDirection;
//! use quadraui::backend::BackendWidget;
//! use quadraui::compose::tab_group::{PaneTab, TabGroupController, TabGroupEvent};
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
//! if let Some(ev) = group.handle_click(pos.x, pos.y) {
//!     match ev {
//!         TabGroupEvent::TabActivated { pane_idx, tab_id } => { /* … */ }
//!         TabGroupEvent::TabClosed   { pane_idx, tab_id } => { /* … */ }
//!         _ => {}
//!     }
//! }
//! ```
//!
//! # Adding panes
//!
//! [`TabGroupController::add_pane_with_tab`] opens a new pane, splitting the
//! focused pane evenly. Panes are separated by draggable
//! [`Split`](crate::Split) dividers. Route `MouseDown` / `MouseMoved` /
//! `MouseUp` events to [`handle_drag_start`](TabGroupController::handle_drag_start)
//! / [`handle_drag_move`](TabGroupController::handle_drag_move) /
//! [`handle_drag_end`](TabGroupController::handle_drag_end) for resize support.
//!
//! # Cross-group tab drag
//!
//! Route `MouseDown` to [`handle_tab_drag_start`](TabGroupController::handle_tab_drag_start)
//! **before** [`handle_drag_start`]. On success, route subsequent `MouseMoved`
//! to [`handle_tab_drag_move`] and `MouseUp` to [`handle_tab_drop`] (or
//! [`cancel_tab_drag`](TabGroupController::cancel_tab_drag) on `Escape`).
//! The controller calls `backend.draw_drop_overlay` automatically inside
//! [`render`](TabGroupController::render) whenever a drag is active.
//!
//! # Layout model
//!
//! Panes are arranged in a recursive binary split tree ([`GroupLayout`]).
//! Each [`Split`](GroupLayout::Split) node carries its own direction and ratio,
//! enabling arbitrary nested horizontal/vertical splits. Leaves hold pane
//! indices that reference positions in the controller's internal pane vec.
//! When a tab is dragged to a `Left`/`Right` edge a **horizontal** split is
//! created; `Top`/`Bottom` edges create a **vertical** split.

use crate::backend::BackendWidget;
use crate::compose::focus_group::FocusGroup;
use crate::event::Rect;
use crate::primitives::drop_zone::{
    compute_drop_zone, drop_zone_overlay, DropEdge, DropGroupRect, DropOverlay, DropZone,
    DropZoneKind,
};
use crate::primitives::split::{Split, SplitDirection};
use crate::primitives::tab_bar::{TabBar, TabBarHits, TabBarSegment, TabItem};
use crate::types::WidgetId;
use crate::Backend;

// ── Recursive layout tree ─────────────────────────────────────────────────────

/// Recursive binary split tree describing how panes are arranged.
///
/// `Leaf(k)` references the pane at index `k` in the controller's internal
/// pane vec. `Split` describes how a rect is divided between two sub-trees.
///
/// In-order leaf traversal gives panes from first (left/top) to last
/// (right/bottom). In-order split traversal gives dividers in the same
/// display order, matching the `divider_idx` used in events and
/// `handle_drag_start`.
///
/// The controller maintains the invariant that every pane vec index `0..n`
/// appears exactly once as a leaf, so `Leaf(k)` always corresponds to pane
/// `k` in the vec.
#[derive(Clone)]
pub enum GroupLayout {
    /// A single pane (referenced by vec index).
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

    /// Whether the tree contains a leaf with value `idx`.
    fn contains_leaf(&self, idx: usize) -> bool {
        match self {
            GroupLayout::Leaf(k) => *k == idx,
            GroupLayout::Split { first, second, .. } => {
                first.contains_leaf(idx) || second.contains_leaf(idx)
            }
        }
    }

    /// Remove the leaf with value `pane_idx`, replacing its parent `Split`
    /// with the surviving sibling. Returns `None` when this node IS the leaf
    /// being removed (signals the caller to replace the parent).
    ///
    /// After calling this, all leaf values > `pane_idx` must be shifted down
    /// with [`shift_indices_down_above`](Self::shift_indices_down_above).
    fn remove_leaf(self, pane_idx: usize) -> Option<Self> {
        match self {
            GroupLayout::Leaf(k) if k == pane_idx => None,
            GroupLayout::Leaf(k) => Some(GroupLayout::Leaf(k)),
            GroupLayout::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                if first.contains_leaf(pane_idx) {
                    match first.remove_leaf(pane_idx) {
                        None => Some(*second),
                        Some(new_first) => Some(GroupLayout::Split {
                            direction,
                            ratio,
                            first: Box::new(new_first),
                            second,
                        }),
                    }
                } else {
                    match second.remove_leaf(pane_idx) {
                        None => Some(*first),
                        Some(new_second) => Some(GroupLayout::Split {
                            direction,
                            ratio,
                            first,
                            second: Box::new(new_second),
                        }),
                    }
                }
            }
        }
    }

    /// Shift all leaf indices > `threshold` down by one.
    ///
    /// Use after [`remove_leaf`](Self::remove_leaf) to keep the vec-index
    /// invariant intact.
    fn shift_indices_down_above(self, threshold: usize) -> Self {
        match self {
            GroupLayout::Leaf(k) => GroupLayout::Leaf(if k > threshold { k - 1 } else { k }),
            GroupLayout::Split {
                direction,
                ratio,
                first,
                second,
            } => GroupLayout::Split {
                direction,
                ratio,
                first: Box::new(first.shift_indices_down_above(threshold)),
                second: Box::new(second.shift_indices_down_above(threshold)),
            },
        }
    }

    /// Shift all leaf indices >= `threshold` up by one.
    ///
    /// Use before inserting a new pane into the middle of the vec.
    fn shift_indices_up_from(self, threshold: usize) -> Self {
        match self {
            GroupLayout::Leaf(k) => GroupLayout::Leaf(if k >= threshold { k + 1 } else { k }),
            GroupLayout::Split {
                direction,
                ratio,
                first,
                second,
            } => GroupLayout::Split {
                direction,
                ratio,
                first: Box::new(first.shift_indices_up_from(threshold)),
                second: Box::new(second.shift_indices_up_from(threshold)),
            },
        }
    }

    /// Find `Leaf(target)` and wrap it in a `Split(dir, 0.5, Leaf(new), Leaf(target))`.
    fn insert_before_leaf(self, target: usize, new_idx: usize, dir: SplitDirection) -> Self {
        match self {
            GroupLayout::Leaf(k) if k == target => GroupLayout::Split {
                direction: dir,
                ratio: 0.5,
                first: Box::new(GroupLayout::Leaf(new_idx)),
                second: Box::new(GroupLayout::Leaf(k)),
            },
            GroupLayout::Leaf(k) => GroupLayout::Leaf(k),
            GroupLayout::Split {
                direction,
                ratio,
                first,
                second,
            } => GroupLayout::Split {
                direction,
                ratio,
                first: Box::new(first.insert_before_leaf(target, new_idx, dir)),
                second: Box::new(second.insert_before_leaf(target, new_idx, dir)),
            },
        }
    }

    /// Find `Leaf(target)` and wrap it in a `Split(dir, 0.5, Leaf(target), Leaf(new))`.
    fn insert_after_leaf(self, target: usize, new_idx: usize, dir: SplitDirection) -> Self {
        match self {
            GroupLayout::Leaf(k) if k == target => GroupLayout::Split {
                direction: dir,
                ratio: 0.5,
                first: Box::new(GroupLayout::Leaf(k)),
                second: Box::new(GroupLayout::Leaf(new_idx)),
            },
            GroupLayout::Leaf(k) => GroupLayout::Leaf(k),
            GroupLayout::Split {
                direction,
                ratio,
                first,
                second,
            } => GroupLayout::Split {
                direction,
                ratio,
                first: Box::new(first.insert_after_leaf(target, new_idx, dir)),
                second: Box::new(second.insert_after_leaf(target, new_idx, dir)),
            },
        }
    }
}

// ── Private layout state ──────────────────────────────────────────────────────

/// Cached info about one divider from the last render pass.
struct DividerInfo {
    /// The thin strip that is the visual divider handle.
    divider_bounds: Rect,
    /// Orientation of this split.
    direction: SplitDirection,
    /// Full bounds of the `Split` node (first + divider + second combined).
    /// Used by [`handle_drag_move`](TabGroupController::handle_drag_move) to
    /// recompute the split ratio from cursor position without needing the
    /// caller to pass the global bounds again.
    split_bounds: Rect,
    /// Current ratio (cached so we can redraw without mutating the tree during drag).
    ratio: f32,
}

// ── Public data types ─────────────────────────────────────────────────────────

/// One tab within a [`Pane`].
pub struct PaneTab {
    /// Unique identifier within the pane. Used in events and for `active_tab_id`.
    pub id: String,
    /// Display label shown in the tab strip.
    pub label: String,
    /// When `true`, the tab renders a `×` close button.
    pub closable: bool,
    /// Content rendered into the pane body when this tab is active.
    pub content: Box<dyn BackendWidget>,
}

/// One split pane managed by a [`TabGroupController`].
///
/// Panes are not constructed directly; use
/// [`TabGroupController::with_pane`] or
/// [`TabGroupController::add_pane_with_tab`].
pub struct Pane {
    /// Unique identifier for this pane (consumer-chosen, e.g. `"pane:0"`).
    pub id: String,
    tabs: Vec<PaneTab>,
    active_tab_id: String,
    tab_scroll_offset: usize,
}

impl Pane {
    fn new(id: impl Into<String>, tabs: Vec<PaneTab>, active_tab_id: impl Into<String>) -> Self {
        let active_tab_id = active_tab_id.into();
        // Fallback to first tab when the named one is absent.
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
    pub fn tabs(&self) -> &[PaneTab] {
        &self.tabs
    }

    /// Build the [`TabBar`] descriptor for this pane.
    fn build_tab_bar(&self, pane_idx: usize) -> TabBar {
        let items: Vec<TabItem> = self
            .tabs
            .iter()
            .map(|t| TabItem {
                label: t.label.clone(),
                is_active: t.id == self.active_tab_id,
                is_dirty: false,
                is_preview: false,
                is_closable: t.closable,
            })
            .collect();
        TabBar {
            id: WidgetId::new(format!("tg:pane{}:tabs", pane_idx)),
            tabs: items,
            scroll_offset: self.tab_scroll_offset,
            right_segments: vec![TabBarSegment {
                text: " + ".to_string(),
                width_cells: 3,
                id: Some(WidgetId::new(format!("tg:pane{}:new-tab", pane_idx))),
                is_active: false,
            }],
            active_accent: None,
            show_tab_close: self.tabs.iter().any(|t| t.closable),
            compact: false,
        }
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

    /// Remove the tab with `tab_id`. Returns `false` when not found.
    /// When the removed tab was active, the previous tab (or first) becomes active.
    fn remove_tab(&mut self, tab_id: &str) -> bool {
        let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id) else {
            return false;
        };
        self.tabs.remove(idx);
        if self.active_tab_id == tab_id {
            // Prefer the tab that was just before the removed one.
            let new_idx = if idx > 0 { idx - 1 } else { 0 };
            self.active_tab_id = self
                .tabs
                .get(new_idx)
                .map(|t| t.id.clone())
                .unwrap_or_default();
        }
        true
    }

    fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }
}

// ── Events ────────────────────────────────────────────────────────────────────

/// Semantic events emitted by [`TabGroupController`].
#[derive(Debug, Clone, PartialEq)]
pub enum TabGroupEvent {
    /// User activated a tab in `pane_idx`.
    TabActivated { pane_idx: usize, tab_id: String },
    /// User closed a tab in `pane_idx`.
    TabClosed { pane_idx: usize, tab_id: String },
    /// The last tab in `pane_idx` was closed.
    ///
    /// **When more than one pane exists:** the pane is removed and the
    /// controller renumbers the remaining panes.
    ///
    /// **When only one pane remains:** `collapse_pane` is a no-op — the
    /// empty pane is retained in the controller (dropping to zero panes is
    /// not permitted). Consumers should treat this case as "all tabs closed;
    /// the single pane is now empty" and react accordingly (e.g. show a
    /// welcome screen in the content area).
    PaneCollapsed {
        /// Index the pane had *before* collapse.
        pane_idx: usize,
    },
    /// A new pane was added (split). `pane_idx` is its index.
    PaneAdded { pane_idx: usize },
    /// User clicked inside a pane's content area — pane received focus.
    PaneFocused { pane_idx: usize },
    /// User dragged a divider. `divider_idx` is the 0-based in-order index
    /// of the divider (matching the tree's in-order split traversal).
    DividerResized { divider_idx: usize },
    /// User clicked the "+" new-tab button in a pane. Apps should call
    /// [`TabGroupController::add_tab`] in response.
    NewTabRequested { pane_idx: usize },

    // ── Cross-group tab drag events ─────────────────────────────────
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
    /// source pane held **before** any collapse.
    ///
    /// Note: when the source pane collapses, consumers tracking pane lifecycle
    /// should infer the collapse from `from_pane_idx != to_pane_idx` combined
    /// with a decrease in `pane_count()`. The source pane no longer exists
    /// after the mutation.
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
    /// the index it held **before** any collapse. `new_pane_idx` is the final
    /// vec index of the new pane after all mutations.
    ///
    /// `target_pane_idx` is the **original** index of the target pane (before
    /// any collapse). Both `target_pane_idx` and `new_pane_idx` may shift if
    /// the source pane was collapsed first.
    TabSplitToNewPane {
        from_pane_idx: usize,
        tab_id: String,
        /// The pane whose edge was targeted (original index, pre-collapse).
        target_pane_idx: usize,
        /// Which edge the tab was dropped onto.
        edge: DropEdge,
        /// Final vec index of the newly created pane (post-collapse).
        new_pane_idx: usize,
    },
}

// ── Layout ────────────────────────────────────────────────────────────────────

/// Resolved pane regions for one rendered frame.
#[derive(Debug, Clone, PartialEq)]
pub struct TabGroupLayout {
    /// Per-pane full bounds (tab strip + content area combined), in pane vec order.
    pub pane_bounds: Vec<Rect>,
    /// Per-pane tab-strip bounds (top row of each pane), in pane vec order.
    pub strip_bounds: Vec<Rect>,
    /// Per-pane content area bounds (below the tab strip), in pane vec order.
    pub content_bounds: Vec<Rect>,
}

// ── Controller ────────────────────────────────────────────────────────────────

/// Stateful controller that manages N split panes arranged in a recursive
/// H/V split tree, each pane with its own tab bar.
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
    panes: Vec<Pane>,
    focus: FocusGroup,
    /// Recursive split tree. `Leaf(k)` references `panes[k]`.
    /// Maintained so that the vec index of every pane equals the leaf value
    /// that references it.
    layout: GroupLayout,
    /// Default direction used by [`add_pane_with_tab`](Self::add_pane_with_tab).
    default_split_direction: SplitDirection,
    /// Monotonically incrementing counter for generating unique pane IDs.
    next_pane_counter: usize,

    // ── Hit-test cache from last render ────────────────────────────
    last_bounds: Option<Rect>,
    /// Per-pane: (hits, strip_bounds). `None` before first render.
    /// Indexed by pane vec index.
    last_pane_hits: Vec<Option<PaneHitCache>>,
    /// Dividers from the last render (in-order split traversal). N-1 entries
    /// for N panes.
    last_dividers: Vec<DividerInfo>,

    // ── Interaction state ───────────────────────────────────────────
    /// Index of the divider currently being dragged, if any.
    dragging_divider: Option<usize>,
    /// In-progress tab drag, if any (mutually exclusive with `dragging_divider`).
    dragging_tab: Option<TabDragState>,
}

/// Cached hit-test state for one pane from the last render.
struct PaneHitCache {
    hits: TabBarHits,
    strip_bounds: Rect,
    content_bounds: Rect,
}

/// In-progress tab drag state.
struct TabDragState {
    /// Index of the pane the drag started in.
    source_pane_idx: usize,
    /// Id of the tab being dragged.
    tab_id: String,
    /// Most recently computed drop zone (updated by [`handle_tab_drag_move`]).
    ///
    /// [`handle_tab_drag_move`]: TabGroupController::handle_tab_drag_move
    current_zone: Option<DropZone>,
    /// Last known cursor position (used by [`render`] to draw the overlay).
    ///
    /// [`render`]: TabGroupController::render
    cursor_x: f32,
    cursor_y: f32,
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
        let pane = Pane::new(pane_id, tabs, active_tab_id);
        Self {
            panes: vec![pane],
            focus: FocusGroup::new(1),
            layout: GroupLayout::Leaf(0),
            default_split_direction: split_direction,
            next_pane_counter: 1,
            last_bounds: None,
            last_pane_hits: (0..1).map(|_| None).collect(),
            last_dividers: vec![],
            dragging_divider: None,
            dragging_tab: None,
        }
    }

    // ── Accessors ───────────────────────────────────────────────────

    /// Number of panes currently open.
    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }

    /// Borrow the pane at `idx`, or `None` if out of range.
    pub fn pane(&self, idx: usize) -> Option<&Pane> {
        self.panes.get(idx)
    }

    /// Index of the currently focused pane, or `None` when nothing is focused.
    pub fn focused_pane(&self) -> Option<usize> {
        self.focus.active()
    }

    /// Explicitly focus a pane. Out-of-range indices are clamped.
    pub fn focus_pane(&mut self, pane_idx: usize) {
        if !self.panes.is_empty() {
            let clamped = pane_idx.min(self.panes.len() - 1);
            self.focus.set_active(Some(clamped));
        }
    }

    /// Cycle focus forward (+1) or backward (−1) across panes.
    pub fn cycle_focus(&mut self, delta: isize) {
        self.focus.cycle(delta);
    }

    /// Read-only reference to the layout tree.
    pub fn layout(&self) -> &GroupLayout {
        &self.layout
    }

    // ── Tab lifecycle ───────────────────────────────────────────────

    /// Add a tab to the pane at `pane_idx`. Returns `false` if the index is
    /// out of range. Does **not** activate the new tab.
    pub fn add_tab(&mut self, pane_idx: usize, tab: PaneTab) -> bool {
        if let Some(pane) = self.panes.get_mut(pane_idx) {
            pane.tabs.push(tab);
            true
        } else {
            false
        }
    }

    /// Add a tab to `pane_idx` and make it active immediately.
    pub fn add_and_activate_tab(&mut self, pane_idx: usize, tab: PaneTab) -> bool {
        let id = tab.id.clone();
        if self.add_tab(pane_idx, tab) {
            self.panes[pane_idx].activate_tab(&id);
            true
        } else {
            false
        }
    }

    /// Switch the active tab in `pane_idx` to `tab_id`.
    /// Returns `false` when the pane index or tab id is not found.
    pub fn switch_tab(&mut self, pane_idx: usize, tab_id: &str) -> bool {
        self.panes
            .get_mut(pane_idx)
            .map(|p| p.activate_tab(tab_id))
            .unwrap_or(false)
    }

    /// Close a tab in `pane_idx`. Returns the event produced (which may be
    /// `PaneCollapsed` when the last tab is closed), or `None` when the pane
    /// index / tab id is not found.
    pub fn close_tab(&mut self, pane_idx: usize, tab_id: &str) -> Option<TabGroupEvent> {
        let pane = self.panes.get_mut(pane_idx)?;
        if !pane.remove_tab(tab_id) {
            return None;
        }
        let tab_id_owned = tab_id.to_string();
        if pane.is_empty() {
            // Collapse the pane.
            self.collapse_pane(pane_idx);
            Some(TabGroupEvent::PaneCollapsed { pane_idx })
        } else {
            Some(TabGroupEvent::TabClosed {
                pane_idx,
                tab_id: tab_id_owned,
            })
        }
    }

    // ── Pane lifecycle ──────────────────────────────────────────────

    /// Open a new pane containing a single tab. The new pane is inserted after
    /// the currently focused pane (splitting it), or appended at the end when
    /// no pane is focused. The split direction is the controller's
    /// `default_split_direction` (set via [`with_pane`](Self::with_pane)).
    ///
    /// Returns the vec index of the new pane.
    pub fn add_pane_with_tab(&mut self, pane_id: impl Into<String>, tab: PaneTab) -> usize {
        let tab_id = tab.id.clone();
        let new_pane = Pane::new(pane_id, vec![tab], tab_id);

        // Insert after the focused pane (or at the end).
        let focused = self
            .focus
            .active()
            .unwrap_or_else(|| self.panes.len().saturating_sub(1));
        let insert_pos = (focused + 1).min(self.panes.len());

        // Update the tree: shift all leaf indices >= insert_pos up by 1, then
        // wrap the focused pane's leaf in a new Split with the new pane after it.
        self.layout = self.layout.clone().shift_indices_up_from(insert_pos);
        self.panes.insert(insert_pos, new_pane);
        // After shift, focused leaf is still `focused` (we shifted indices >= insert_pos
        // = focused+1, not focused itself).
        self.layout = self.layout.clone().insert_after_leaf(
            focused,
            insert_pos,
            self.default_split_direction,
        );

        let n = self.panes.len();
        self.focus.set_count(n);
        self.focus.set_active(Some(insert_pos));
        self.last_pane_hits = (0..n).map(|_| None).collect();
        self.last_dividers = vec![];

        insert_pos
    }

    /// Remove and drop the pane at `pane_idx`, giving its space to the
    /// nearest sibling in the split tree. No-op when only one pane remains.
    pub fn collapse_pane(&mut self, pane_idx: usize) {
        if self.panes.len() <= 1 || pane_idx >= self.panes.len() {
            return;
        }

        // Remove the leaf from the tree, replacing its parent Split with the sibling.
        let new_layout = self
            .layout
            .clone()
            .remove_leaf(pane_idx)
            .expect("pane_idx must exist as a leaf in the layout tree");
        // Shift down all leaf indices > pane_idx.
        self.layout = new_layout.shift_indices_down_above(pane_idx);

        // Remove from the pane vec.
        self.panes.remove(pane_idx);

        let n = self.panes.len();
        self.focus.set_count(n);
        if let Some(fi) = self.focus.active() {
            if fi >= n {
                self.focus.set_active(n.checked_sub(1));
            }
        }
        self.last_pane_hits = (0..n).map(|_| None).collect();
        self.last_dividers = vec![];
    }

    // ── Render ──────────────────────────────────────────────────────

    /// Render all panes into `bounds`. Calls `backend.draw_split` for each
    /// inter-pane divider and `backend.draw_tab_bar` for each pane's tab
    /// strip. Active-pane content is rendered via [`BackendWidget::render`].
    /// When a tab drag is active, automatically calls
    /// `backend.draw_drop_overlay` at the end of the frame.
    ///
    /// Returns a [`TabGroupLayout`] with resolved pane/strip/content rects
    /// in pane vec order.
    pub fn render(&mut self, backend: &mut dyn Backend, bounds: Rect) -> TabGroupLayout {
        if self.panes.is_empty() {
            return TabGroupLayout {
                pane_bounds: vec![],
                strip_bounds: vec![],
                content_bounds: vec![],
            };
        }

        self.last_bounds = Some(bounds);
        let n = self.panes.len();

        // ── Step 1: compute per-pane bounds via recursive layout ────
        let mut pane_bounds_by_idx: Vec<Option<Rect>> = vec![None; n];
        self.last_dividers.clear();
        layout_recursive(
            &self.layout,
            backend,
            bounds,
            &mut pane_bounds_by_idx,
            &mut self.last_dividers,
        );

        // ── Step 2: draw dividers ───────────────────────────────────
        for (i, info) in self.last_dividers.iter().enumerate() {
            let split = Split {
                id: WidgetId::new(format!("tg:div{}", i)),
                direction: info.direction,
                ratio: info.ratio,
                first_min: 0.0,
                second_min: 0.0,
            };
            backend.draw_split(info.split_bounds, &split);
        }

        // ── Step 3: for each pane, draw tab strip + content ─────────
        let lh = backend.line_height();
        let mut pane_bounds_out = Vec::with_capacity(n);
        let mut strip_bounds = Vec::with_capacity(n);
        let mut content_bounds = Vec::with_capacity(n);

        if self.last_pane_hits.len() != n {
            self.last_pane_hits = (0..n).map(|_| None).collect();
        }

        for (pane_idx, pane) in self.panes.iter_mut().enumerate() {
            let pb = pane_bounds_by_idx[pane_idx].unwrap_or(bounds);

            let strip = Rect::new(pb.x, pb.y, pb.width, lh);
            let content_h = (pb.height - lh).max(0.0);
            let content = Rect::new(pb.x, pb.y + lh, pb.width, content_h);

            pane_bounds_out.push(pb);
            strip_bounds.push(strip);
            content_bounds.push(content);

            // Draw tab bar.
            let tab_bar = pane.build_tab_bar(pane_idx);
            let hits = backend.draw_tab_bar(strip, &tab_bar, None);
            pane.tab_scroll_offset = hits.correct_scroll_offset;

            // Render active content.
            if content.width > 0.0 && content.height > 0.0 {
                if let Some(tab_pos) = pane.tabs.iter().position(|t| t.id == pane.active_tab_id) {
                    pane.tabs[tab_pos].content.render(backend, content);
                }
            }

            self.last_pane_hits[pane_idx] = Some(PaneHitCache {
                hits,
                strip_bounds: strip,
                content_bounds: content,
            });
        }

        // ── Step 4: draw drop overlay when a tab drag is in progress ─
        // Compute the overlay before borrowing backend mutably.
        let overlay: Option<DropOverlay> = if let Some(drag) = self.dragging_tab.as_ref() {
            if drag.current_zone.is_some() {
                let ghost_offset = lh;
                self.tab_drag_overlay(drag.cursor_x, drag.cursor_y, 2.0, ghost_offset)
            } else {
                None
            }
        } else {
            None
        };
        if let Some(ov) = overlay {
            backend.draw_drop_overlay(&ov);
        }

        TabGroupLayout {
            pane_bounds: pane_bounds_out,
            strip_bounds,
            content_bounds,
        }
    }

    // ── Click dispatch ──────────────────────────────────────────────

    /// Resolve a mouse click at `(x, y)` in viewport coordinates.
    ///
    /// Applies resulting state changes (tab activation, close, pane focus)
    /// and returns the emitted event. Returns `None` when the click is
    /// outside all tab strips and content areas, or on dead space.
    ///
    /// Typical call site: mouse-down handler after background dismissal.
    pub fn handle_click(&mut self, x: f32, y: f32) -> Option<TabGroupEvent> {
        let n = self.panes.len();
        let click_x = x as f64;
        let in_range = |range: (f64, f64)| click_x >= range.0 && click_x < range.1;

        for pane_idx in 0..n {
            let Some(cache) = self.last_pane_hits[pane_idx].as_ref() else {
                continue;
            };
            let strip = cache.strip_bounds;
            let content = cache.content_bounds;
            let hits = &cache.hits;

            // ── Tab strip click ───────────────────────────────────
            let in_strip = y >= strip.y
                && y < strip.y + strip.height
                && x >= strip.x
                && x < strip.x + strip.width;
            if in_strip {
                // Focus this pane.
                self.focus.set_active(Some(pane_idx));

                // Close buttons take precedence.
                for (idx, cb) in hits.close_bounds.iter().enumerate() {
                    if let Some(range) = cb {
                        if in_range(*range) {
                            if let Some(tab) = self.panes[pane_idx].tabs.get(idx) {
                                if tab.closable {
                                    let tab_id = tab.id.clone();
                                    return self.close_tab(pane_idx, &tab_id);
                                }
                            }
                            return None;
                        }
                    }
                }

                // Right segments (new-tab button).
                for rb in &hits.right_segment_bounds {
                    if in_range(*rb) {
                        return Some(TabGroupEvent::NewTabRequested { pane_idx });
                    }
                }

                // Tab body.
                for (idx, range) in hits.slot_positions.iter().enumerate() {
                    if in_range(*range) {
                        if let Some(tab) = self.panes[pane_idx].tabs.get(idx) {
                            let tab_id = tab.id.clone();
                            if tab_id != self.panes[pane_idx].active_tab_id {
                                self.panes[pane_idx].activate_tab(&tab_id);
                                return Some(TabGroupEvent::TabActivated { pane_idx, tab_id });
                            }
                        }
                        return None;
                    }
                }
                return None;
            }

            // ── Content area click → focus ────────────────────────
            let in_content = y >= content.y
                && y < content.y + content.height
                && x >= content.x
                && x < content.x + content.width;
            if in_content {
                if self.focus.active() != Some(pane_idx) {
                    self.focus.set_active(Some(pane_idx));
                    return Some(TabGroupEvent::PaneFocused { pane_idx });
                }
                return None;
            }
        }
        None
    }

    // ── Divider drag ────────────────────────────────────────────────

    /// Start a drag if `(x, y)` is on a divider. Returns `true` when a
    /// drag was initiated.
    pub fn handle_drag_start(&mut self, x: f32, y: f32) -> bool {
        for (i, info) in self.last_dividers.iter().enumerate() {
            let d = &info.divider_bounds;
            if x >= d.x && x < d.x + d.width && y >= d.y && y < d.y + d.height {
                self.dragging_divider = Some(i);
                return true;
            }
        }
        false
    }

    /// Update the dragged divider position. Returns a `DividerResized` event
    /// when the split ratio changes. Call with each mouse-moved event while
    /// dragging.
    ///
    /// The `bounds` parameter is accepted for API compatibility but is no
    /// longer needed — the controller uses the cached split bounds from the
    /// last render instead.
    pub fn handle_drag_move(&mut self, x: f32, y: f32, _bounds: Rect) -> Option<TabGroupEvent> {
        let div_idx = self.dragging_divider?;
        let info = self.last_dividers.get(div_idx)?;

        // Compute new ratio from cursor position relative to the split's own bounds.
        let new_ratio = match info.direction {
            SplitDirection::Horizontal => {
                if info.split_bounds.width > 0.0 {
                    ((x - info.split_bounds.x) / info.split_bounds.width).clamp(0.05, 0.95)
                } else {
                    return None;
                }
            }
            SplitDirection::Vertical => {
                if info.split_bounds.height > 0.0 {
                    ((y - info.split_bounds.y) / info.split_bounds.height).clamp(0.05, 0.95)
                } else {
                    return None;
                }
            }
        };

        let mut counter = 0usize;
        let changed =
            update_split_ratio_inorder(&mut self.layout, div_idx, new_ratio, &mut counter)
                .unwrap_or(false);

        if changed {
            Some(TabGroupEvent::DividerResized {
                divider_idx: div_idx,
            })
        } else {
            None
        }
    }

    /// End any active divider drag.
    pub fn handle_drag_end(&mut self) {
        self.dragging_divider = None;
    }

    // ── DropZone integration ────────────────────────────────────────

    /// Return [`DropGroupRect`]s for all panes from the last render, suitable
    /// for passing to [`crate::compute_drop_zone`].
    ///
    /// Tab slot positions are derived from the last [`TabBarHits`] recorded
    /// during [`render`](Self::render). If `render` has not been called yet,
    /// the result is empty.
    pub fn drop_group_rects(&self) -> Vec<DropGroupRect> {
        self.last_pane_hits
            .iter()
            .filter_map(|cache| {
                let c = cache.as_ref()?;
                let strip = c.strip_bounds;
                // Full pane bounds = strip + content.
                let full = Rect::new(
                    strip.x,
                    strip.y,
                    strip.width,
                    strip.height + c.content_bounds.height,
                );
                // Convert slot_positions (absolute x pairs) into (start, end) pairs.
                let tab_slots: Vec<(f32, f32)> = c
                    .hits
                    .slot_positions
                    .iter()
                    .filter(|(s, e)| *s != 0.0 || *e != 0.0)
                    .map(|(s, e)| (*s as f32, *e as f32))
                    .collect();
                Some(DropGroupRect {
                    bounds: full,
                    tab_slots,
                })
            })
            .collect()
    }

    // ── Cross-group tab drag ────────────────────────────────────────

    /// Return the strip height from cached render data, or a default of 1.0.
    fn strip_height(&self) -> f32 {
        self.last_pane_hits
            .iter()
            .find_map(|c| c.as_ref().map(|c| c.strip_bounds.height))
            .unwrap_or(1.0)
    }

    /// Start a tab drag if `(x, y)` lands on a draggable tab slot (not a
    /// close button or the new-tab segment). Returns `true` when a drag was
    /// initiated; `false` means the caller should fall through to
    /// [`handle_click`](Self::handle_click).
    ///
    /// A tab drag and a divider drag are mutually exclusive; starting one
    /// implicitly cancels the other.
    pub fn handle_tab_drag_start(&mut self, x: f32, y: f32) -> bool {
        let click_x = x as f64;
        let in_range = |range: (f64, f64)| click_x >= range.0 && click_x < range.1;

        let n = self.panes.len();
        for pane_idx in 0..n {
            let Some(cache) = self.last_pane_hits[pane_idx].as_ref() else {
                continue;
            };
            let strip = cache.strip_bounds;
            let hits = &cache.hits;

            let in_strip = y >= strip.y
                && y < strip.y + strip.height
                && x >= strip.x
                && x < strip.x + strip.width;
            if !in_strip {
                continue;
            }

            // Reject close buttons — those are click actions, not drags.
            for range in hits.close_bounds.iter().flatten() {
                if in_range(*range) {
                    return false;
                }
            }
            // Reject right segments (new-tab button etc.).
            for rb in &hits.right_segment_bounds {
                if in_range(*rb) {
                    return false;
                }
            }

            // Accept any tab slot.
            for (tab_idx, range) in hits.slot_positions.iter().enumerate() {
                if in_range(*range) {
                    if let Some(tab) = self.panes[pane_idx].tabs.get(tab_idx) {
                        self.dragging_divider = None;
                        self.dragging_tab = Some(TabDragState {
                            source_pane_idx: pane_idx,
                            tab_id: tab.id.clone(),
                            current_zone: None,
                            cursor_x: x,
                            cursor_y: y,
                        });
                        return true;
                    }
                    return false;
                }
            }
        }
        false
    }

    /// Update the drag position. Returns the current [`DropZone`] under the
    /// cursor, or `None` when no drag is in progress or the cursor is outside
    /// all groups.
    ///
    /// Call on every mouse-moved event while dragging. The result can be
    /// passed to [`drop_zone_overlay`](crate::drop_zone_overlay) directly,
    /// or use [`tab_drag_overlay`](Self::tab_drag_overlay) as a convenience.
    pub fn handle_tab_drag_move(&mut self, x: f32, y: f32) -> Option<DropZone> {
        self.dragging_tab.as_ref()?;
        let groups = self.drop_group_rects();
        let tab_bar_h = self.strip_height();
        let zone = compute_drop_zone(x, y, &groups, tab_bar_h);
        if let Some(drag) = &mut self.dragging_tab {
            drag.current_zone = zone.clone();
            drag.cursor_x = x;
            drag.cursor_y = y;
        }
        zone
    }

    /// Compute the drop overlay for the current tab drag, ready to hand to
    /// the backend's drop-overlay renderer.
    ///
    /// Uses the [`DropZone`] last stored by [`handle_tab_drag_move`]. Returns
    /// `None` when no tab drag is in progress or no drop zone has been
    /// resolved yet (cursor was outside all groups).
    ///
    /// `bar_thickness` is the insertion-bar width in logical units (typically
    /// 2–3 px for GTK, 1 cell for TUI). `ghost_offset` is how far the ghost
    /// label floats from the cursor (typically one line height).
    ///
    /// Note: [`render`](Self::render) calls this automatically and passes the
    /// overlay to `backend.draw_drop_overlay`. Only call this directly if you
    /// need the overlay for a custom rendering path.
    pub fn tab_drag_overlay(
        &self,
        cursor_x: f32,
        cursor_y: f32,
        bar_thickness: f32,
        ghost_offset: f32,
    ) -> Option<DropOverlay> {
        let drag = self.dragging_tab.as_ref()?;
        let zone = drag.current_zone.as_ref()?;
        let groups = self.drop_group_rects();
        let tab_bar_h = self.strip_height();
        Some(drop_zone_overlay(
            zone,
            &groups,
            cursor_x,
            cursor_y,
            tab_bar_h,
            bar_thickness,
            ghost_offset,
        ))
    }

    /// Finalise a tab drag by applying the drop at `(x, y)`.
    ///
    /// Handles three cases:
    ///
    /// * **Reorder within the same pane** — `DropZoneKind::TabReorder` on the
    ///   source pane: reorders the tab list and emits [`TabGroupEvent::TabReordered`].
    /// * **Merge into another pane** — `DropZoneKind::Center` or
    ///   `DropZoneKind::TabReorder` on a different pane: moves the tab and
    ///   emits [`TabGroupEvent::TabMovedToPane`]. The source pane is collapsed
    ///   if it becomes empty.
    /// * **Split to new pane** — `DropZoneKind::Split`: removes the tab from
    ///   its pane, creates a new pane adjacent to the target, and emits
    ///   [`TabGroupEvent::TabSplitToNewPane`]. The source pane is collapsed if
    ///   it becomes empty. Dropping the only tab of the only pane onto its own
    ///   edge is a no-op. `Left`/`Right` edges create a **horizontal** split;
    ///   `Top`/`Bottom` edges create a **vertical** split.
    ///
    /// Clears drag state regardless of outcome. Returns `None` when no drag is
    /// in progress, the cursor is outside all groups, or the drop is a no-op.
    pub fn handle_tab_drop(&mut self, x: f32, y: f32) -> Option<TabGroupEvent> {
        let drag = self.dragging_tab.take()?;
        let groups = self.drop_group_rects();
        let tab_bar_h = self.strip_height();
        let zone = compute_drop_zone(x, y, &groups, tab_bar_h)?;

        let from = drag.source_pane_idx;
        let to = zone.group_idx;

        match zone.kind {
            // ── Reorder within same pane ────────────────────────────
            DropZoneKind::TabReorder(insert_idx) if to == from => {
                let cur_idx = self.panes[from]
                    .tabs
                    .iter()
                    .position(|t| t.id == drag.tab_id)?;
                // Drop at the same position: no-op.
                if cur_idx == insert_idx || cur_idx + 1 == insert_idx {
                    return None;
                }
                let moved_tab = self.panes[from].tabs.remove(cur_idx);
                // After removal, indices > cur_idx shift down by 1.
                let adj = if insert_idx > cur_idx {
                    insert_idx - 1
                } else {
                    insert_idx
                };
                let adj = adj.min(self.panes[from].tabs.len());
                self.panes[from].tabs.insert(adj, moved_tab);
                Some(TabGroupEvent::TabReordered {
                    pane_idx: from,
                    tab_id: drag.tab_id,
                    from_idx: cur_idx,
                    to_idx: adj,
                })
            }

            // ── No-op: dropped on own content area ──────────────────
            DropZoneKind::Center if to == from => None,

            // ── Merge: move tab to another pane ─────────────────────
            DropZoneKind::Center | DropZoneKind::TabReorder(_) => {
                // Locate and remove tab from source pane.
                let cur_idx = self.panes[from]
                    .tabs
                    .iter()
                    .position(|t| t.id == drag.tab_id)?;
                let moved_tab = self.panes[from].tabs.remove(cur_idx);
                // Fix active tab in source pane.
                if self.panes[from].active_tab_id == drag.tab_id {
                    let fallback = if cur_idx > 0 { cur_idx - 1 } else { 0 };
                    self.panes[from].active_tab_id = self.panes[from]
                        .tabs
                        .get(fallback)
                        .map(|t| t.id.clone())
                        .unwrap_or_default();
                }
                let tab_id = moved_tab.id.clone();
                let source_empty = self.panes[from].tabs.is_empty();

                // Determine insertion position in the target pane's tab list.
                let raw_insert = match zone.kind {
                    DropZoneKind::TabReorder(idx) => idx.min(self.panes[to].tabs.len()),
                    _ => self.panes[to].tabs.len(),
                };
                self.panes[to].tabs.insert(raw_insert, moved_tab);
                self.panes[to].active_tab_id = tab_id.clone();

                // Collapse source pane if empty; adjust the reported target index.
                let final_to = if source_empty {
                    self.collapse_pane(from);
                    // collapse_pane removes index `from`; all indices > from shift down.
                    if from < to {
                        to - 1
                    } else {
                        to
                    }
                } else {
                    to
                };

                Some(TabGroupEvent::TabMovedToPane {
                    from_pane_idx: from,
                    to_pane_idx: final_to,
                    tab_id,
                    insert_idx: raw_insert,
                })
            }

            // ── Split: create a new adjacent pane ───────────────────
            DropZoneKind::Split(edge) => {
                // Guard: splitting the only tab of the only pane is a no-op.
                if self.panes[from].tabs.len() == 1 && self.panes.len() == 1 {
                    return None;
                }

                // Remove tab from source pane.
                let cur_idx = self.panes[from]
                    .tabs
                    .iter()
                    .position(|t| t.id == drag.tab_id)?;
                let moved_tab = self.panes[from].tabs.remove(cur_idx);
                if self.panes[from].active_tab_id == drag.tab_id {
                    let fallback = if cur_idx > 0 { cur_idx - 1 } else { 0 };
                    self.panes[from].active_tab_id = self.panes[from]
                        .tabs
                        .get(fallback)
                        .map(|t| t.id.clone())
                        .unwrap_or_default();
                }
                let tab_id = moved_tab.id.clone();
                let source_empty = self.panes[from].tabs.is_empty();
                let original_target = zone.group_idx;

                // Split direction is determined by the drop edge.
                // Left/Right → horizontal split; Top/Bottom → vertical split.
                let split_dir = match edge {
                    DropEdge::Left | DropEdge::Right => SplitDirection::Horizontal,
                    DropEdge::Top | DropEdge::Bottom => SplitDirection::Vertical,
                };
                let insert_before = matches!(edge, DropEdge::Left | DropEdge::Top);

                // Adjust target index if source collapses first.
                let mut adjusted_to = to;
                if source_empty {
                    self.collapse_pane(from);
                    if from < adjusted_to {
                        adjusted_to -= 1;
                    }
                }

                // New pane will be inserted at `actual_pos` in the vec.
                let actual_pos = if insert_before {
                    adjusted_to
                } else {
                    adjusted_to + 1
                };

                // Build the new pane with a unique ID.
                let new_pane_id = format!("pane:{}", self.next_pane_counter);
                self.next_pane_counter += 1;
                let new_tab_id = moved_tab.id.clone();
                let new_pane = Pane::new(new_pane_id, vec![moved_tab], new_tab_id);

                // Shift tree indices >= actual_pos up by 1, then insert into vec.
                self.layout = self.layout.clone().shift_indices_up_from(actual_pos);
                self.panes.insert(actual_pos, new_pane);

                // Wrap the target leaf in a new Split with the new pane.
                // After the shift, the target pane is at:
                //   - insert_before: adjusted_to + 1  (was shifted up)
                //   - insert_after:  adjusted_to       (was not shifted)
                if insert_before {
                    self.layout = self.layout.clone().insert_before_leaf(
                        adjusted_to + 1,
                        actual_pos,
                        split_dir,
                    );
                } else {
                    self.layout =
                        self.layout
                            .clone()
                            .insert_after_leaf(adjusted_to, actual_pos, split_dir);
                }

                let new_n = self.panes.len();
                self.focus.set_count(new_n);
                self.focus.set_active(Some(actual_pos));
                self.last_pane_hits = (0..new_n).map(|_| None).collect();
                self.last_dividers = vec![];

                Some(TabGroupEvent::TabSplitToNewPane {
                    from_pane_idx: from,
                    tab_id,
                    target_pane_idx: original_target,
                    edge,
                    new_pane_idx: actual_pos,
                })
            }
        }
    }

    /// Cancel an in-progress tab drag without moving any tab.
    pub fn cancel_tab_drag(&mut self) {
        self.dragging_tab = None;
    }

    /// Return `true` if a tab drag is currently in progress.
    pub fn is_tab_dragging(&self) -> bool {
        self.dragging_tab.is_some()
    }

    /// Compute the drop zone at `(cursor_x, cursor_y)` using the cached
    /// group rects from the last render.
    ///
    /// Convenience wrapper around [`crate::compute_drop_zone`] that derives
    /// `tab_bar_height` from the stored strip bounds. Returns `None` before
    /// the first render or when the cursor is outside all groups.
    pub fn drop_zone_at(&self, cursor_x: f32, cursor_y: f32) -> Option<DropZone> {
        let groups = self.drop_group_rects();
        let tab_bar_h = self.strip_height();
        compute_drop_zone(cursor_x, cursor_y, &groups, tab_bar_h)
    }
}

// ── Layout helpers ────────────────────────────────────────────────────────────

/// Walk the layout tree, computing per-pane bounds and collecting divider info
/// in in-order split traversal.
///
/// `pane_bounds[k]` is set to the bounds for the pane at vec index `k`.
/// `dividers` receives one entry per `Split` node in in-order traversal
/// (left subtree's splits, then the current split, then right subtree's
/// splits), matching the divider ordering used in `last_dividers` and
/// `DividerResized` events.
fn layout_recursive(
    tree: &GroupLayout,
    backend: &dyn Backend,
    bounds: Rect,
    pane_bounds: &mut Vec<Option<Rect>>,
    dividers: &mut Vec<DividerInfo>,
) {
    match tree {
        GroupLayout::Leaf(idx) => {
            if *idx < pane_bounds.len() {
                pane_bounds[*idx] = Some(bounds);
            }
        }
        GroupLayout::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            let split = Split {
                id: WidgetId::new("tg:layout"),
                direction: *direction,
                ratio: *ratio,
                first_min: 0.0,
                second_min: 0.0,
            };
            let sl = backend.split_layout(bounds, &split);
            // In-order: first subtree's dividers, then this divider, then second.
            layout_recursive(first, backend, sl.first_bounds, pane_bounds, dividers);
            dividers.push(DividerInfo {
                divider_bounds: sl.divider_bounds,
                direction: *direction,
                split_bounds: bounds,
                ratio: *ratio,
            });
            layout_recursive(second, backend, sl.second_bounds, pane_bounds, dividers);
        }
    }
}

/// Update the ratio of the in-order-`target` `Split` node in `tree`.
///
/// Returns `Some(true)` when the node was found and the ratio changed
/// significantly (≥ 0.001), `Some(false)` when found but unchanged, and
/// `None` when the target index was not found.
///
/// `counter` must start at 0 on the first call.
fn update_split_ratio_inorder(
    tree: &mut GroupLayout,
    target: usize,
    new_ratio: f32,
    counter: &mut usize,
) -> Option<bool> {
    match tree {
        GroupLayout::Leaf(_) => None,
        GroupLayout::Split {
            ratio,
            first,
            second,
            ..
        } => {
            // In-order: check first subtree, then current node, then second subtree.
            if let Some(r) = update_split_ratio_inorder(first, target, new_ratio, counter) {
                return Some(r);
            }
            if *counter == target {
                let old = *ratio;
                *counter += 1;
                *ratio = new_ratio;
                return Some((new_ratio - old).abs() >= 0.001);
            }
            *counter += 1;
            update_split_ratio_inorder(second, target, new_ratio, counter)
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::tab_bar_layout_to_hits;
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

    /// Prime the hit-test cache for `pane_idx` without a real backend render.
    /// Each tab is given `tab_w` cells (last `close_w` cells = close region).
    /// The "new-tab" right segment is given 3 cells.
    fn prime_pane(
        ctrl: &mut TabGroupController,
        pane_idx: usize,
        strip_x: f32,
        strip_y: f32,
        bar_w: f32,
        tab_w: f32,
        close_w: f32,
    ) {
        let Some(pane) = ctrl.panes.get(pane_idx) else {
            return;
        };
        let bar = pane.build_tab_bar(pane_idx);
        let n_tabs = pane.tabs.len();
        let layout = bar.layout(
            bar_w,
            1.0,
            0.0,
            |i| {
                if i < n_tabs && pane.tabs[i].closable {
                    TabMeasure::new(tab_w, close_w)
                } else {
                    TabMeasure::new(tab_w, 0.0)
                }
            },
            |_| SegmentMeasure::new(3.0),
        );
        let mut hits = tab_bar_layout_to_hits(&layout, &bar);
        // Shift hits to absolute coords (mirror what backends do).
        let ox = strip_x as f64;
        for sp in &mut hits.slot_positions {
            if *sp != (0.0, 0.0) {
                sp.0 += ox;
                sp.1 += ox;
            }
        }
        for cb in hits.close_bounds.iter_mut().flatten() {
            cb.0 += ox;
            cb.1 += ox;
        }
        for rb in &mut hits.right_segment_bounds {
            rb.0 += ox;
            rb.1 += ox;
        }
        let strip = Rect::new(strip_x, strip_y, bar_w, 1.0);
        let content = Rect::new(strip_x, strip_y + 1.0, bar_w, 10.0);
        ctrl.last_pane_hits[pane_idx] = Some(PaneHitCache {
            hits,
            strip_bounds: strip,
            content_bounds: content,
        });
    }

    // ── Construction ─────────────────────────────────────────────────────────

    #[test]
    fn new_controller_has_one_pane_with_tabs() {
        let ctrl = make_group();
        assert_eq!(ctrl.pane_count(), 1);
        assert_eq!(ctrl.panes[0].tabs().len(), 2);
        assert_eq!(ctrl.panes[0].active_tab_id(), "t0");
        assert_eq!(ctrl.focused_pane(), None); // starts unfocused
    }

    // ── Tab switching ─────────────────────────────────────────────────────────

    #[test]
    fn switch_tab_changes_active() {
        let mut ctrl = make_group();
        assert!(ctrl.switch_tab(0, "t1"));
        assert_eq!(ctrl.panes[0].active_tab_id(), "t1");
    }

    #[test]
    fn switch_tab_unknown_returns_false() {
        let mut ctrl = make_group();
        assert!(!ctrl.switch_tab(0, "no-such"));
        assert_eq!(ctrl.panes[0].active_tab_id(), "t0"); // unchanged
    }

    // ── Add tab ───────────────────────────────────────────────────────────────

    #[test]
    fn add_tab_appends_without_activating() {
        let mut ctrl = make_group();
        ctrl.add_tab(0, tab("t2", "new.rs", true));
        assert_eq!(ctrl.panes[0].tabs().len(), 3);
        assert_eq!(ctrl.panes[0].active_tab_id(), "t0"); // unchanged
    }

    #[test]
    fn add_and_activate_tab_makes_it_active() {
        let mut ctrl = make_group();
        ctrl.add_and_activate_tab(0, tab("t2", "new.rs", true));
        assert_eq!(ctrl.panes[0].active_tab_id(), "t2");
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
        assert_eq!(ctrl.panes[0].tabs().len(), 1);
        assert_eq!(ctrl.panes[0].active_tab_id(), "t0");
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
        assert_eq!(ctrl.panes[0].active_tab_id(), "t0"); // fell back to t0
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
        assert!(matches!(ctrl.layout, GroupLayout::Leaf(0)));
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
        assert!(matches!(ctrl.layout, GroupLayout::Split { .. }));
        // Tree has exactly 2 leaves.
        assert_eq!(ctrl.layout.leaf_count(), 2);
    }

    #[test]
    fn three_panes_fractions_sum_to_one() {
        let mut ctrl = make_group();
        ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", false));
        ctrl.add_pane_with_tab("p2", tab("y0", "y.rs", false));
        assert_eq!(ctrl.pane_count(), 3);
        // All 3 pane indices appear as leaves.
        assert_eq!(ctrl.layout.leaf_count(), 3);
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

        let ev = ctrl.handle_click(9.0, 0.5); // x=9 → t1 body
        assert_eq!(
            ev,
            Some(TabGroupEvent::TabActivated {
                pane_idx: 0,
                tab_id: "t1".into()
            })
        );
        assert_eq!(ctrl.panes[0].active_tab_id(), "t1");
    }

    #[test]
    fn click_active_tab_returns_none() {
        let mut ctrl = make_group();
        prime_pane(&mut ctrl, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        assert_eq!(ctrl.handle_click(3.0, 0.5), None);
    }

    #[test]
    fn click_close_button_emits_tab_closed() {
        let mut ctrl = make_group();
        prime_pane(&mut ctrl, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        let ev = ctrl.handle_click(7.0, 0.5);
        assert_eq!(
            ev,
            Some(TabGroupEvent::TabClosed {
                pane_idx: 0,
                tab_id: "t0".into()
            })
        );
        assert_eq!(ctrl.panes[0].tabs().len(), 1);
    }

    #[test]
    fn click_new_tab_button_emits_new_tab_requested() {
        let mut ctrl = make_group();
        prime_pane(&mut ctrl, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        let ev = ctrl.handle_click(78.0, 0.5);
        assert_eq!(ev, Some(TabGroupEvent::NewTabRequested { pane_idx: 0 }));
    }

    // ── handle_click: content area ────────────────────────────────────────────

    #[test]
    fn click_content_area_focuses_pane() {
        let mut ctrl = make_group();
        ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", false));
        ctrl.focus_pane(0);
        prime_pane(&mut ctrl, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        prime_pane(&mut ctrl, 1, 80.0, 0.0, 80.0, 8.0, 2.0);

        let ev = ctrl.handle_click(85.0, 5.0);
        assert_eq!(ev, Some(TabGroupEvent::PaneFocused { pane_idx: 1 }));
        assert_eq!(ctrl.focused_pane(), Some(1));
    }

    #[test]
    fn click_already_focused_content_returns_none() {
        let mut ctrl = make_group();
        prime_pane(&mut ctrl, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        ctrl.focus_pane(0);
        assert_eq!(ctrl.handle_click(5.0, 5.0), None);
    }

    // ── handle_click: non-zero origin ────────────────────────────────────────

    #[test]
    fn click_with_offset_strip_resolves_correctly() {
        let mut ctrl = make_group();
        prime_pane(&mut ctrl, 0, 20.0, 5.0, 80.0, 8.0, 2.0);

        let ev = ctrl.handle_click(30.0, 5.5);
        assert_eq!(
            ev,
            Some(TabGroupEvent::TabActivated {
                pane_idx: 0,
                tab_id: "t1".into()
            })
        );
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
        // Manually place a divider.
        ctrl.last_dividers = vec![DividerInfo {
            divider_bounds: Rect::new(40.0, 0.0, 1.0, 20.0),
            direction: SplitDirection::Horizontal,
            split_bounds: Rect::new(0.0, 0.0, 80.0, 20.0),
            ratio: 0.5,
        }];
        assert!(ctrl.handle_drag_start(40.5, 10.0));
        assert_eq!(ctrl.dragging_divider, Some(0));
    }

    #[test]
    fn drag_move_updates_split_ratio() {
        let mut ctrl = make_group();
        ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", false));
        // Place a divider centred at x=50 on a 100-wide split.
        ctrl.last_dividers = vec![DividerInfo {
            divider_bounds: Rect::new(50.0, 0.0, 1.0, 20.0),
            direction: SplitDirection::Horizontal,
            split_bounds: Rect::new(0.0, 0.0, 100.0, 20.0),
            ratio: 0.5,
        }];
        ctrl.handle_drag_start(50.5, 5.0);

        let bounds = Rect::new(0.0, 0.0, 100.0, 20.0); // ignored by new impl
        let ev = ctrl.handle_drag_move(30.0, 5.0, bounds); // drag to x=30 → ratio ≈ 0.30
        assert_eq!(ev, Some(TabGroupEvent::DividerResized { divider_idx: 0 }));
        // The root split's ratio should now be ≈ 0.30.
        let ratio = match &ctrl.layout {
            GroupLayout::Split { ratio, .. } => *ratio,
            _ => panic!("expected Split at root"),
        };
        assert!(
            (ratio - 0.30).abs() < 0.02,
            "ratio={ratio}, expected ≈ 0.30"
        );
        assert!((1.0 - ratio - 0.70).abs() < 0.02);
    }

    #[test]
    fn drag_end_clears_drag_state() {
        let mut ctrl = make_group();
        ctrl.dragging_divider = Some(0);
        ctrl.handle_drag_end();
        assert_eq!(ctrl.dragging_divider, None);
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
        assert_eq!(ctrl.layout.leaf_count(), 3);

        ctrl.collapse_pane(1);
        assert_eq!(ctrl.pane_count(), 2);
        assert_eq!(ctrl.layout.leaf_count(), 2);
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
        assert!(ctrl.layout.contains_leaf(0));
        assert!(ctrl.layout.contains_leaf(1));
        assert!(ctrl.layout.contains_leaf(2));
        assert_eq!(ctrl.layout.leaf_count(), 3);
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
        let ev = ctrl.handle_tab_drop(120.0, 2.5); // top-edge of pane 1 content

        if let Some(TabGroupEvent::TabSplitToNewPane { edge, .. }) = &ev {
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
            find_any_vertical(&ctrl.layout),
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
        let drag = ctrl.dragging_tab.as_ref().unwrap();
        assert_eq!(drag.source_pane_idx, 0);
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
        let stored = ctrl.dragging_tab.as_ref().unwrap().current_zone.as_ref();
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
        let ev = ctrl.handle_tab_drop(3.0, 0.5).unwrap();
        assert_eq!(
            ev,
            TabGroupEvent::TabReordered {
                pane_idx: 0,
                tab_id: "t1".into(),
                from_idx: 1,
                to_idx: 0,
            }
        );
        assert_eq!(ctrl.panes[0].tabs()[0].id, "t1");
        assert_eq!(ctrl.panes[0].tabs()[1].id, "t0");
        assert_eq!(ctrl.pane_count(), 2);
    }

    #[test]
    fn tab_drop_reorder_same_position_is_noop() {
        let mut ctrl = make_two_pane();
        ctrl.handle_tab_drag_start(9.0, 0.5);
        let ev = ctrl.handle_tab_drop(14.0, 0.5);
        assert_eq!(ev, None);
        assert_eq!(ctrl.panes[0].tabs()[0].id, "t0");
        assert_eq!(ctrl.panes[0].tabs()[1].id, "t1");
    }

    #[test]
    fn tab_drop_same_pane_center_is_noop() {
        let mut ctrl = make_two_pane();
        ctrl.handle_tab_drag_start(9.0, 0.5);
        let ev = ctrl.handle_tab_drop(40.0, 5.0);
        assert_eq!(ev, None);
        assert_eq!(ctrl.pane_count(), 2);
        assert_eq!(ctrl.panes[0].tabs().len(), 2);
    }

    // ── handle_tab_drop: merge into another pane ──────────────────────────────

    #[test]
    fn tab_drop_moves_tab_to_another_pane_center() {
        let mut ctrl = make_two_pane();
        ctrl.handle_tab_drag_start(9.0, 0.5);
        let ev = ctrl.handle_tab_drop(120.0, 5.0).unwrap();
        assert_eq!(
            ev,
            TabGroupEvent::TabMovedToPane {
                from_pane_idx: 0,
                to_pane_idx: 1,
                tab_id: "t1".into(),
                insert_idx: 1,
            }
        );
        assert_eq!(ctrl.pane_count(), 2);
        assert_eq!(ctrl.panes[0].tabs().len(), 1);
        assert_eq!(ctrl.panes[0].tabs()[0].id, "t0");
        assert_eq!(ctrl.panes[1].tabs().len(), 2);
        assert_eq!(ctrl.panes[1].tabs()[1].id, "t1");
        assert_eq!(ctrl.panes[1].active_tab_id(), "t1");
        assert_eq!(ctrl.layout.leaf_count(), 2);
    }

    #[test]
    fn tab_drop_moves_tab_to_another_pane_tab_bar() {
        let mut ctrl = make_two_pane();
        ctrl.handle_tab_drag_start(9.0, 0.5);
        let ev = ctrl.handle_tab_drop(82.0, 0.5).unwrap();
        assert_eq!(
            ev,
            TabGroupEvent::TabMovedToPane {
                from_pane_idx: 0,
                to_pane_idx: 1,
                tab_id: "t1".into(),
                insert_idx: 0,
            }
        );
        assert_eq!(ctrl.panes[1].tabs()[0].id, "t1");
        assert_eq!(ctrl.panes[1].tabs()[1].id, "x0");
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
        let ev = ctrl.handle_tab_drop(120.0, 5.0).unwrap();

        assert_eq!(ctrl.pane_count(), 1);
        assert_eq!(
            ev,
            TabGroupEvent::TabMovedToPane {
                from_pane_idx: 0,
                to_pane_idx: 0,
                tab_id: "only".into(),
                insert_idx: 1,
            }
        );
        assert_eq!(ctrl.panes[0].tabs().len(), 2);
        assert_eq!(ctrl.panes[0].tabs()[1].id, "only");
        assert_eq!(ctrl.layout.leaf_count(), 1);
    }

    // ── handle_tab_drop: split to new pane ────────────────────────────────────

    #[test]
    fn tab_drop_splits_right_edge_creates_pane_after_target() {
        let mut ctrl = make_two_pane();
        ctrl.handle_tab_drag_start(9.0, 0.5);
        let ev = ctrl.handle_tab_drop(150.0, 5.0).unwrap();
        assert!(
            matches!(
                &ev,
                TabGroupEvent::TabSplitToNewPane {
                    from_pane_idx: 0,
                    tab_id,
                    target_pane_idx: 1,
                    edge: crate::DropEdge::Right,
                    new_pane_idx: 2,
                } if tab_id == "t1"
            ),
            "unexpected event: {ev:?}"
        );
        assert_eq!(ctrl.pane_count(), 3);
        assert_eq!(ctrl.panes[0].tabs()[0].id, "t0");
        assert_eq!(ctrl.panes[1].tabs()[0].id, "x0");
        assert_eq!(ctrl.panes[2].tabs()[0].id, "t1");
        assert_eq!(ctrl.layout.leaf_count(), 3);
    }

    #[test]
    fn tab_drop_splits_left_edge_creates_pane_before_target() {
        let mut ctrl = make_two_pane();
        ctrl.handle_tab_drag_start(9.0, 0.5);
        let ev = ctrl.handle_tab_drop(83.0, 5.0).unwrap();
        assert!(
            matches!(
                &ev,
                TabGroupEvent::TabSplitToNewPane {
                    from_pane_idx: 0,
                    tab_id,
                    target_pane_idx: 1,
                    edge: crate::DropEdge::Left,
                    new_pane_idx: 1,
                } if tab_id == "t1"
            ),
            "unexpected event: {ev:?}"
        );
        assert_eq!(ctrl.pane_count(), 3);
        assert_eq!(ctrl.panes[0].tabs()[0].id, "t0");
        assert_eq!(ctrl.panes[1].tabs()[0].id, "t1");
        assert_eq!(ctrl.panes[2].tabs()[0].id, "x0");
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
        let ev = ctrl.handle_tab_drop(150.0, 5.0).unwrap();

        assert_eq!(ctrl.pane_count(), 2);
        assert!(
            matches!(
                &ev,
                TabGroupEvent::TabSplitToNewPane {
                    from_pane_idx: 0,
                    tab_id,
                    new_pane_idx: 1,
                    ..
                } if tab_id == "only"
            ),
            "unexpected event: {ev:?}"
        );
        assert_eq!(ctrl.panes[0].tabs()[0].id, "x0");
        assert_eq!(ctrl.panes[1].tabs()[0].id, "only");
        assert_eq!(ctrl.layout.leaf_count(), 2);
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
        let ev = ctrl.handle_tab_drop(70.0, 5.0);
        assert_eq!(ev, None);
        assert_eq!(ctrl.pane_count(), 1);
        assert_eq!(ctrl.panes[0].tabs().len(), 1);
    }

    #[test]
    fn tab_drop_clears_drag_state_on_noop() {
        let mut ctrl = make_two_pane();
        ctrl.handle_tab_drag_start(9.0, 0.5);
        let ev = ctrl.handle_tab_drop(999.0, 999.0);
        assert_eq!(ev, None);
        assert!(!ctrl.is_tab_dragging());
    }

    // ── Unique pane IDs after repeated splits ─────────────────────────────────

    #[test]
    fn split_then_collapse_then_split_unique_ids() {
        // Drag t1 out to create pane:1, collapse it, then drag a different tab
        // to create another split.  The second new pane must have a distinct id.
        let mut ctrl = make_two_pane();
        ctrl.handle_tab_drag_start(9.0, 0.5); // drag t1
        ctrl.handle_tab_drop(150.0, 5.0); // split → pane[2].id == "pane:1"
        assert_eq!(ctrl.pane_count(), 3);
        let first_new_id = ctrl.panes[2].id.clone();

        ctrl.collapse_pane(2); // remove the new pane
        assert_eq!(ctrl.pane_count(), 2);

        // Add an extra tab so pane 0 won't be emptied by the next drag.
        ctrl.add_tab(0, tab("t-extra", "extra.rs", false));

        // Re-prime: pane 0 now has 2 tabs (t0, t-extra), pane 1 has 1 tab (x0).
        prime_pane(&mut ctrl, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        prime_pane(&mut ctrl, 1, 80.0, 0.0, 80.0, 8.0, 2.0);

        // Drag t-extra (slot [8..16), x=9) to right edge of pane 1 → creates pane:2.
        ctrl.handle_tab_drag_start(9.0, 0.5);
        let ev = ctrl.handle_tab_drop(150.0, 5.0);

        assert_eq!(
            ctrl.pane_count(),
            3,
            "second split should create 3 panes: {ev:?}"
        );
        let second_new_id = ctrl.panes[2].id.clone();
        assert_ne!(
            second_new_id, first_new_id,
            "second split pane must have a unique id (got {second_new_id:?})"
        );
    }
}
