//! `TabGroup` — tabbed split-pane compose helper.
//!
//! Wires [`TabBar`](crate::TabBar) + [`Split`](crate::Split) +
//! [`DropZone`](crate::DropZone) + [`FocusGroup`] into an
//! editor-group-style layout: N panes arranged side-by-side (or
//! stacked top/bottom), each with its own scrollable tab bar.
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
//! available space evenly. Panes are separated by draggable
//! [`Split`](crate::Split) dividers. Route `MouseDown` / `MouseMoved` /
//! `MouseUp` events to [`handle_drag_start`](TabGroupController::handle_drag_start)
//! / [`handle_drag_move`](TabGroupController::handle_drag_move) /
//! [`handle_drag_end`](TabGroupController::handle_drag_end) for resize support.
//!
//! # Cross-group tab drag
//!
//! [`crate::compute_drop_zone`] data types for detecting drop targets are
//! exposed via [`TabGroupController::drop_group_rects`]. Full wiring is deferred.
//!
//! TODO(#144-followup): wire cross-group drag-and-drop using `compute_drop_zone`
//! + `drop_zone_overlay` + `Backend::draw_drop_overlay`.

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
    /// User dragged a divider. `divider_idx` is the 0-based index of the
    /// divider (between pane `divider_idx` and pane `divider_idx + 1`).
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
    /// index of the new pane after all mutations.
    TabSplitToNewPane {
        from_pane_idx: usize,
        tab_id: String,
        /// The pane whose edge was targeted (original index).
        target_pane_idx: usize,
        /// Which edge the tab was dropped onto.
        edge: DropEdge,
        /// Final index of the newly created pane.
        new_pane_idx: usize,
    },
}

// ── Layout ────────────────────────────────────────────────────────────────────

/// Resolved pane regions for one rendered frame.
#[derive(Debug, Clone, PartialEq)]
pub struct TabGroupLayout {
    /// Per-pane full bounds (tab strip + content area combined).
    pub pane_bounds: Vec<Rect>,
    /// Per-pane tab-strip bounds (top row of each pane).
    pub strip_bounds: Vec<Rect>,
    /// Per-pane content area bounds (below the tab strip).
    pub content_bounds: Vec<Rect>,
}

// ── Controller ────────────────────────────────────────────────────────────────

/// Stateful controller that manages N split panes, each with its own tab bar.
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
/// // In handle:
/// if let Some(ev) = group.handle_click(x, y) { … }
/// group.handle_drag_start(x, y);
/// if let Some(ev) = group.handle_drag_move(x, y) { … }
/// group.handle_drag_end();
/// ```
pub struct TabGroupController {
    panes: Vec<Pane>,
    focus: FocusGroup,
    /// One fraction per pane; sum ≈ 1.0. Represents how much of the
    /// total split-direction width (or height) each pane occupies.
    pane_fractions: Vec<f32>,
    split_direction: SplitDirection,

    // ── Hit-test cache from last render ────────────────────────────
    last_bounds: Option<Rect>,
    /// Per-pane: (hits, strip_bounds). `None` before first render.
    last_pane_hits: Vec<Option<PaneHitCache>>,
    /// Bounds of each inter-pane divider (N-1 entries).
    last_divider_bounds: Vec<Rect>,

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
            pane_fractions: vec![1.0],
            split_direction,
            last_bounds: None,
            last_pane_hits: (0..1).map(|_| None).collect(),
            last_divider_bounds: vec![],
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

    /// Open a new pane containing a single tab. The new pane is appended
    /// after the currently focused pane (or at the end) and given an equal
    /// share of the available space. Returns the index of the new pane.
    pub fn add_pane_with_tab(&mut self, pane_id: impl Into<String>, tab: PaneTab) -> usize {
        let tab_id = tab.id.clone();
        let new_pane = Pane::new(pane_id, vec![tab], tab_id);

        // Insert after the focused pane (or at the end).
        let insert_at = self
            .focus
            .active()
            .map(|i| (i + 1).min(self.panes.len()))
            .unwrap_or(self.panes.len());

        self.panes.insert(insert_at, new_pane);

        // Redistribute fractions evenly.
        let n = self.panes.len();
        let each = 1.0 / n as f32;
        self.pane_fractions = vec![each; n];

        // Update focus group count (preserve focused index, or move to new pane).
        self.focus.set_count(n);
        self.focus.set_active(Some(insert_at));

        // Resize hit cache to match new pane count.
        self.last_pane_hits = (0..n).map(|_| None).collect();
        self.last_divider_bounds = vec![];

        insert_at
    }

    /// Remove and drop the pane at `pane_idx`, redistributing its space
    /// to its neighbors. No-op when only one pane remains (never collapse
    /// to zero panes from outside code — use [`close_tab`](Self::close_tab)
    /// which handles the case gracefully).
    pub fn collapse_pane(&mut self, pane_idx: usize) {
        if self.panes.len() <= 1 || pane_idx >= self.panes.len() {
            return;
        }
        let removed_fraction = self.pane_fractions[pane_idx];
        self.panes.remove(pane_idx);
        self.pane_fractions.remove(pane_idx);

        // Give the removed fraction to the neighbor that comes right after
        // (or the one before when the last pane was removed).
        let neighbor = if pane_idx < self.pane_fractions.len() {
            pane_idx
        } else {
            pane_idx.saturating_sub(1)
        };
        if let Some(f) = self.pane_fractions.get_mut(neighbor) {
            *f += removed_fraction;
        }

        // Normalise to ensure sum == 1.0 despite floating-point drift.
        let total: f32 = self.pane_fractions.iter().sum();
        if total > 0.0 {
            for f in &mut self.pane_fractions {
                *f /= total;
            }
        }

        let n = self.panes.len();
        self.focus.set_count(n);
        // Keep focused index in bounds.
        if let Some(fi) = self.focus.active() {
            if fi >= n {
                self.focus.set_active(n.checked_sub(1));
            }
        }
        self.last_pane_hits = (0..n).map(|_| None).collect();
        self.last_divider_bounds = vec![];
    }

    // ── Render ──────────────────────────────────────────────────────

    /// Render all panes into `bounds`. Calls `backend.draw_split` for each
    /// inter-pane divider and `backend.draw_tab_bar` for each pane's tab
    /// strip. Active-pane content is rendered via [`BackendWidget::render`].
    ///
    /// Returns a [`TabGroupLayout`] with resolved pane/strip/content rects.
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

        // ── Step 1: compute per-pane full bounds ────────────────────
        let pane_bounds =
            compute_pane_bounds(backend, bounds, &self.pane_fractions, self.split_direction);

        // ── Step 2: draw dividers and cache their hit bounds ────────
        self.last_divider_bounds.clear();
        self.last_divider_bounds.reserve(n.saturating_sub(1));

        for i in 0..(n.saturating_sub(1)) {
            let pb_i = pane_bounds[i];
            let pb_next = pane_bounds[i + 1];
            // Build a synthetic Split covering pane i .. end of pane i+1,
            // just so draw_split can render the divider at the right position.
            let (combined_rect, ratio) = match self.split_direction {
                SplitDirection::Horizontal => {
                    // combined width = pb_i.width + divider + pb_next.width
                    // We infer divider thickness from the gap between panes.
                    let combined_w = (pb_next.x + pb_next.width) - pb_i.x;
                    let rect = Rect::new(pb_i.x, pb_i.y, combined_w, pb_i.height);
                    let ratio = pb_i.width / (combined_w.max(0.001));
                    (rect, ratio)
                }
                SplitDirection::Vertical => {
                    let combined_h = (pb_next.y + pb_next.height) - pb_i.y;
                    let rect = Rect::new(pb_i.x, pb_i.y, pb_i.width, combined_h);
                    let ratio = pb_i.height / (combined_h.max(0.001));
                    (rect, ratio)
                }
            };

            let split = Split {
                id: WidgetId::new(format!("tg:div{}", i)),
                direction: self.split_direction,
                ratio,
                first_min: 0.0,
                second_min: 0.0,
            };
            let split_layout = backend.draw_split(combined_rect, &split);
            self.last_divider_bounds.push(split_layout.divider_bounds);
        }

        // ── Step 3: for each pane, draw tab strip + content ─────────
        let lh = backend.line_height();
        let mut strip_bounds = Vec::with_capacity(n);
        let mut content_bounds = Vec::with_capacity(n);

        // Ensure cache vec is the right length.
        if self.last_pane_hits.len() != n {
            self.last_pane_hits = (0..n).map(|_| None).collect();
        }

        for (pane_idx, pane) in self.panes.iter_mut().enumerate() {
            let pb = pane_bounds[pane_idx];

            let strip = Rect::new(pb.x, pb.y, pb.width, lh);
            let content_h = (pb.height - lh).max(0.0);
            let content = Rect::new(pb.x, pb.y + lh, pb.width, content_h);

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

        TabGroupLayout {
            pane_bounds,
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
        for (i, div) in self.last_divider_bounds.iter().enumerate() {
            if x >= div.x && x < div.x + div.width && y >= div.y && y < div.y + div.height {
                self.dragging_divider = Some(i);
                return true;
            }
        }
        false
    }

    /// Update the dragged divider position. Returns a `DividerResized` event
    /// when the fractions change. Call with each mouse-moved event while
    /// dragging.
    ///
    /// `bounds` must be the same rect passed to the most recent [`render`](Self::render).
    pub fn handle_drag_move(&mut self, x: f32, y: f32, bounds: Rect) -> Option<TabGroupEvent> {
        let div_idx = self.dragging_divider?;
        if div_idx >= self.pane_fractions.len().saturating_sub(1) {
            return None;
        }

        // Compute the new cumulative split position in [0, 1].
        let cursor_frac = match self.split_direction {
            SplitDirection::Horizontal => {
                if bounds.width > 0.0 {
                    ((x - bounds.x) / bounds.width).clamp(0.01, 0.99)
                } else {
                    return None;
                }
            }
            SplitDirection::Vertical => {
                if bounds.height > 0.0 {
                    ((y - bounds.y) / bounds.height).clamp(0.01, 0.99)
                } else {
                    return None;
                }
            }
        };

        // Compute cumulative positions before and after the dragged divider.
        let cumsum: Vec<f32> = {
            let mut v = Vec::with_capacity(self.pane_fractions.len());
            let mut acc = 0.0_f32;
            for f in &self.pane_fractions {
                acc += f;
                v.push(acc);
            }
            v
        };
        let left_bound = if div_idx > 0 {
            cumsum[div_idx - 1]
        } else {
            0.0
        };
        let right_bound = if div_idx + 2 < cumsum.len() {
            cumsum[div_idx + 1]
        } else {
            1.0
        };

        // Clamp cursor_frac so neither adjacent pane collapses below 5%.
        let clamped = cursor_frac.clamp(left_bound + 0.05, right_bound - 0.05);
        let prev_cum = if div_idx > 0 {
            cumsum[div_idx - 1]
        } else {
            0.0
        };
        let next_cum = cumsum[div_idx + 1];

        let new_left_frac = clamped - prev_cum;
        let new_right_frac = next_cum - clamped;

        if (new_left_frac - self.pane_fractions[div_idx]).abs() < 0.001 {
            return None; // negligible change
        }

        self.pane_fractions[div_idx] = new_left_frac;
        self.pane_fractions[div_idx + 1] = new_right_frac;

        Some(TabGroupEvent::DividerResized {
            divider_idx: div_idx,
        })
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
    ///   edge is a no-op.
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
                // Insert the tab into the target pane (pane vec unchanged here).
                let raw_insert = raw_insert.min(self.panes[to].tabs.len());
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

                // Determine insertion position relative to the target pane.
                // Left/Top → insert before target; Right/Bottom → insert after.
                let insert_before = matches!(edge, DropEdge::Left | DropEdge::Top);
                let mut insert_pos = if insert_before { to } else { to + 1 };

                // Collapse source first if it's empty, adjusting insert_pos.
                if source_empty {
                    self.collapse_pane(from);
                    if from < insert_pos {
                        insert_pos -= 1;
                    }
                }

                // Build and insert the new pane.
                let new_tab_id = moved_tab.id.clone();
                let new_pane_id = format!("pane:{}", self.panes.len());
                let new_pane = Pane::new(new_pane_id, vec![moved_tab], new_tab_id);

                let actual_pos = insert_pos.min(self.panes.len());
                self.panes.insert(actual_pos, new_pane);
                let new_n = self.panes.len();
                let each = 1.0 / new_n as f32;
                self.pane_fractions = vec![each; new_n];
                self.focus.set_count(new_n);
                self.focus.set_active(Some(actual_pos));
                self.last_pane_hits = (0..new_n).map(|_| None).collect();
                self.last_divider_bounds = vec![];

                Some(TabGroupEvent::TabSplitToNewPane {
                    from_pane_idx: from,
                    tab_id,
                    target_pane_idx: to,
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

/// Compute the bounds for each pane by recursively applying binary splits.
///
/// For N panes with fractions `f[0..N]` (summing to 1.0), the layout is a
/// left-leaning binary tree of [`Split`]s:
/// - Level 0: split `bounds` at ratio `f[0]` → pane 0 gets `first_bounds`.
/// - Level 1: split remaining `second_bounds` at ratio `f[1]/(f[1]+…+f[N-1])`.
/// - …and so on until all panes are placed.
///
/// Each call to `backend.split_layout` returns a layout using the backend's
/// native divider thickness — the compose helper never hard-codes it.
fn compute_pane_bounds(
    backend: &dyn Backend,
    bounds: Rect,
    fractions: &[f32],
    direction: SplitDirection,
) -> Vec<Rect> {
    let n = fractions.len();
    if n == 0 {
        return vec![];
    }
    if n == 1 {
        return vec![bounds];
    }

    let rest_sum: f32 = fractions[1..].iter().sum();
    let ratio = if fractions[0] + rest_sum > 0.0 {
        fractions[0] / (fractions[0] + rest_sum)
    } else {
        0.5
    };
    let split = Split {
        id: WidgetId::new("tg:layout-split"),
        direction,
        ratio,
        first_min: 0.0,
        second_min: 0.0,
    };
    let layout = backend.split_layout(bounds, &split);

    let mut result = vec![layout.first_bounds];
    result.extend(compute_pane_bounds(
        backend,
        layout.second_bounds,
        &fractions[1..],
        direction,
    ));
    result
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
        // When only 1 pane remains, close_tab on the last tab still returns
        // PaneCollapsed but the controller keeps 1 (empty) pane rather than
        // dropping to 0 — collapse_pane no-ops at len==1.
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
        // Fractions renormalise.
        assert!((ctrl.pane_fractions[0] - 1.0).abs() < 0.01);
    }

    // ── Pane lifecycle ────────────────────────────────────────────────────────

    #[test]
    fn add_pane_with_tab_creates_second_pane() {
        let mut ctrl = make_group();
        let idx = ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", true));
        assert_eq!(idx, 1);
        assert_eq!(ctrl.pane_count(), 2);
        assert_eq!(ctrl.focused_pane(), Some(1));
        // Fractions sum to 1.0.
        let sum: f32 = ctrl.pane_fractions.iter().sum();
        assert!((sum - 1.0).abs() < 0.001);
    }

    #[test]
    fn three_panes_fractions_sum_to_one() {
        let mut ctrl = make_group();
        ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", false));
        ctrl.add_pane_with_tab("p2", tab("y0", "y.rs", false));
        assert_eq!(ctrl.pane_count(), 3);
        let sum: f32 = ctrl.pane_fractions.iter().sum();
        assert!((sum - 1.0).abs() < 0.001);
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
        // Layout: t0=[0..8), t1=[8..16), close=[14..16), new-tab=[77..80)
        // strip at y=0, bar_w=80, tab_w=8, close_w=2
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
        // t0 is active at [0..8). Click on it → None.
        assert_eq!(ctrl.handle_click(3.0, 0.5), None);
    }

    #[test]
    fn click_close_button_emits_tab_closed() {
        // t0 close region at [6..8) (tab_w=8, close_w=2 → close=[6..8))
        let mut ctrl = make_group();
        prime_pane(&mut ctrl, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        let ev = ctrl.handle_click(7.0, 0.5); // x=7 → close of t0
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
        // new-tab at [77..80) (bar_w=80, right_segment width=3)
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
        // add_pane_with_tab auto-focuses the new pane (idx 1). Move focus back
        // to pane 0 so clicking pane 1's content triggers a focus change.
        ctrl.focus_pane(0);
        // Prime both panes side by side: pane 0 at x=0, pane 1 at x=80.
        prime_pane(&mut ctrl, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        prime_pane(&mut ctrl, 1, 80.0, 0.0, 80.0, 8.0, 2.0);

        // Click in pane 1's content area (x=85, y=5).
        let ev = ctrl.handle_click(85.0, 5.0);
        assert_eq!(ev, Some(TabGroupEvent::PaneFocused { pane_idx: 1 }));
        assert_eq!(ctrl.focused_pane(), Some(1));
    }

    #[test]
    fn click_already_focused_content_returns_none() {
        let mut ctrl = make_group();
        prime_pane(&mut ctrl, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        ctrl.focus_pane(0);
        // Click in content area of already-focused pane → None.
        assert_eq!(ctrl.handle_click(5.0, 5.0), None);
    }

    // ── handle_click: non-zero origin ────────────────────────────────────────

    #[test]
    fn click_with_offset_strip_resolves_correctly() {
        // strip_x=20 → t0=[20..28), t1=[28..36), t0-close=[26..28)
        let mut ctrl = make_group();
        prime_pane(&mut ctrl, 0, 20.0, 5.0, 80.0, 8.0, 2.0);

        // Click t1 body (x=30).
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
        // No dividers with 1 pane.
        assert!(!ctrl.handle_drag_start(50.0, 50.0));
    }

    #[test]
    fn drag_start_on_divider_returns_true() {
        let mut ctrl = make_group();
        ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", false));
        // Manually place a divider rect.
        ctrl.last_divider_bounds = vec![Rect::new(40.0, 0.0, 1.0, 20.0)];
        assert!(ctrl.handle_drag_start(40.5, 10.0));
        assert_eq!(ctrl.dragging_divider, Some(0));
    }

    #[test]
    fn drag_move_updates_fractions() {
        let mut ctrl = make_group();
        ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", false));
        ctrl.last_divider_bounds = vec![Rect::new(50.0, 0.0, 1.0, 20.0)];
        ctrl.handle_drag_start(50.5, 5.0);

        let bounds = Rect::new(0.0, 0.0, 100.0, 20.0);
        let ev = ctrl.handle_drag_move(30.0, 5.0, bounds); // drag to x=30 → 30%
        assert_eq!(ev, Some(TabGroupEvent::DividerResized { divider_idx: 0 }));
        assert!((ctrl.pane_fractions[0] - 0.30).abs() < 0.02);
        assert!((ctrl.pane_fractions[1] - 0.70).abs() < 0.02);
        let sum: f32 = ctrl.pane_fractions.iter().sum();
        assert!((sum - 1.0).abs() < 0.001);
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

    // ── Pane fractions normalisation ──────────────────────────────────────────

    #[test]
    fn collapse_pane_renormalises_fractions() {
        let mut ctrl = make_group();
        ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", false));
        ctrl.add_pane_with_tab("p2", tab("y0", "y.rs", false));
        assert_eq!(ctrl.pane_count(), 3);

        ctrl.collapse_pane(1);
        assert_eq!(ctrl.pane_count(), 2);
        let sum: f32 = ctrl.pane_fractions.iter().sum();
        assert!((sum - 1.0).abs() < 0.001);
    }

    // ── Cross-group tab drag: helpers ─────────────────────────────────────────

    /// Build a two-pane controller with:
    ///   pane 0 at x=0  : tabs [t0, t1], strip height=1, bar_w=80
    ///   pane 1 at x=80 : tabs [x0],     strip height=1, bar_w=80
    ///
    /// Layout reference (absolute x, tab_w=8, close_w=2):
    ///   pane 0: t0=[0..8), t0-close=[6..8); t1=[8..16), t1-close=[14..16); new-tab=[77..80)
    ///   pane 1: x0=[80..88), x0-close=[86..88); new-tab=[157..160)
    ///
    /// drop_group_rects for this layout:
    ///   group 0: bounds=(0,0,80,11), tab_slots=[(0,8),(8,16)]
    ///   group 1: bounds=(80,0,80,11), tab_slots=[(80,88)]
    ///   tab_bar_height = 1.0
    ///
    /// Content edge zones (edge_w=16, edge_h=3):
    ///   pane 0 left-edge:  x < 16; right-edge: x >= 64
    ///   pane 1 left-edge: x-80 < 16 → x < 96; right-edge: x-80 >= 64 → x >= 144
    fn make_two_pane() -> TabGroupController {
        let mut ctrl = TabGroupController::with_pane(
            "p0",
            vec![tab("t0", "main.rs", true), tab("t1", "lib.rs", true)],
            "t0",
            SplitDirection::Horizontal,
        );
        ctrl.add_pane_with_tab("p1", tab("x0", "x.rs", true));
        // Ensure focus is explicitly set so tests are deterministic.
        ctrl.focus_pane(0);
        // Prime hit caches: pane 0 at x=0, pane 1 at x=80.
        prime_pane(&mut ctrl, 0, 0.0, 0.0, 80.0, 8.0, 2.0);
        prime_pane(&mut ctrl, 1, 80.0, 0.0, 80.0, 8.0, 2.0);
        ctrl
    }

    // ── handle_tab_drag_start ─────────────────────────────────────────────────

    #[test]
    fn tab_drag_start_on_tab_body_returns_true() {
        let mut ctrl = make_two_pane();
        // x=9 lands on t1 body in pane 0's strip.
        assert!(ctrl.handle_tab_drag_start(9.0, 0.5));
        assert!(ctrl.is_tab_dragging());
        let drag = ctrl.dragging_tab.as_ref().unwrap();
        assert_eq!(drag.source_pane_idx, 0);
        assert_eq!(drag.tab_id, "t1");
    }

    #[test]
    fn tab_drag_start_on_close_button_returns_false() {
        let mut ctrl = make_two_pane();
        // x=7 is inside t0's close region [6..8).
        assert!(!ctrl.handle_tab_drag_start(7.0, 0.5));
        assert!(!ctrl.is_tab_dragging());
    }

    #[test]
    fn tab_drag_start_on_new_tab_button_returns_false() {
        let mut ctrl = make_two_pane();
        // x=78 is inside the new-tab segment [77..80).
        assert!(!ctrl.handle_tab_drag_start(78.0, 0.5));
        assert!(!ctrl.is_tab_dragging());
    }

    #[test]
    fn tab_drag_start_in_content_area_returns_false() {
        let mut ctrl = make_two_pane();
        // y=5 is in the content area, not the strip.
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
        assert!(ctrl.handle_tab_drag_start(9.0, 0.5)); // drag t1
                                                       // Move to center of pane 1 content area (x=120, y=5).
        let zone = ctrl.handle_tab_drag_move(120.0, 5.0);
        assert_eq!(
            zone,
            Some(crate::DropZone {
                kind: crate::DropZoneKind::Center,
                group_idx: 1,
            })
        );
        // Zone should be stored in drag state.
        let stored = ctrl.dragging_tab.as_ref().unwrap().current_zone.as_ref();
        assert_eq!(stored.unwrap().group_idx, 1);
    }

    #[test]
    fn tab_drag_overlay_returns_none_without_zone() {
        let mut ctrl = make_two_pane();
        ctrl.handle_tab_drag_start(9.0, 0.5);
        // No move yet → no stored zone.
        assert!(ctrl.tab_drag_overlay(9.0, 0.5, 2.0, 10.0).is_none());
    }

    #[test]
    fn tab_drag_overlay_returns_highlight_after_move() {
        let mut ctrl = make_two_pane();
        ctrl.handle_tab_drag_start(9.0, 0.5);
        ctrl.handle_tab_drag_move(120.0, 5.0); // Center in group 1
        let ov = ctrl.tab_drag_overlay(120.0, 5.0, 2.0, 10.0).unwrap();
        // Center highlight covers the content area of group 1: (80, 1, 80, 10).
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
        // x=82 in pane 1's tab bar (y=0.5 < tab_bar_height=1).
        let zone = ctrl.drop_zone_at(82.0, 0.5).unwrap();
        assert_eq!(zone.group_idx, 1);
        assert!(matches!(zone.kind, crate::DropZoneKind::TabReorder(_)));
    }

    // ── handle_tab_drop: reorder within same pane ─────────────────────────────

    #[test]
    fn tab_drop_reorders_within_same_pane() {
        let mut ctrl = make_two_pane();
        // pane 0 has [t0, t1]. Drag t1 (at x=9) to before t0 (TabReorder(0)).
        // In the pane 0 tab bar: TabReorder(0) is at x=3 (left of t0's midpoint=4).
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
        // pane 0 tabs are now [t1, t0].
        assert_eq!(ctrl.panes[0].tabs()[0].id, "t1");
        assert_eq!(ctrl.panes[0].tabs()[1].id, "t0");
        // pane count unchanged.
        assert_eq!(ctrl.pane_count(), 2);
    }

    #[test]
    fn tab_drop_reorder_same_position_is_noop() {
        let mut ctrl = make_two_pane();
        // Drag t1 (index 1) and drop at TabReorder(2) = after t1's midpoint.
        // x=14 in pane 0's tab bar: slot 1 mid=12, 14 >= 12 → insert after slot 1 = idx 2.
        // cur_idx=1, insert_idx=2 → cur_idx+1 == insert_idx → no-op.
        ctrl.handle_tab_drag_start(9.0, 0.5);
        let ev = ctrl.handle_tab_drop(14.0, 0.5);
        assert_eq!(ev, None);
        // Tabs unchanged.
        assert_eq!(ctrl.panes[0].tabs()[0].id, "t0");
        assert_eq!(ctrl.panes[0].tabs()[1].id, "t1");
    }

    #[test]
    fn tab_drop_same_pane_center_is_noop() {
        let mut ctrl = make_two_pane();
        // Drag t1, drop on center of pane 0 content area.
        ctrl.handle_tab_drag_start(9.0, 0.5);
        // Center of pane 0: x=40, y=5 (not near any edge).
        let ev = ctrl.handle_tab_drop(40.0, 5.0);
        assert_eq!(ev, None);
        // State unaffected.
        assert_eq!(ctrl.pane_count(), 2);
        assert_eq!(ctrl.panes[0].tabs().len(), 2);
    }

    // ── handle_tab_drop: merge into another pane ──────────────────────────────

    #[test]
    fn tab_drop_moves_tab_to_another_pane_center() {
        let mut ctrl = make_two_pane();
        // Drag t1 from pane 0, drop on center of pane 1.
        ctrl.handle_tab_drag_start(9.0, 0.5); // drag t1
        let ev = ctrl.handle_tab_drop(120.0, 5.0).unwrap(); // center of pane 1
        assert_eq!(
            ev,
            TabGroupEvent::TabMovedToPane {
                from_pane_idx: 0,
                to_pane_idx: 1,
                tab_id: "t1".into(),
                insert_idx: 1, // appended after x0
            }
        );
        // pane 0 still has t0; pane 1 now has [x0, t1].
        assert_eq!(ctrl.pane_count(), 2);
        assert_eq!(ctrl.panes[0].tabs().len(), 1);
        assert_eq!(ctrl.panes[0].tabs()[0].id, "t0");
        assert_eq!(ctrl.panes[1].tabs().len(), 2);
        assert_eq!(ctrl.panes[1].tabs()[1].id, "t1");
        assert_eq!(ctrl.panes[1].active_tab_id(), "t1");
        // Fractions intact.
        let sum: f32 = ctrl.pane_fractions.iter().sum();
        assert!((sum - 1.0).abs() < 0.001);
    }

    #[test]
    fn tab_drop_moves_tab_to_another_pane_tab_bar() {
        let mut ctrl = make_two_pane();
        // Drag t1 from pane 0, drop on pane 1's tab bar at TabReorder(0)
        // (x=82, y=0.5 — left of x0's midpoint 84).
        ctrl.handle_tab_drag_start(9.0, 0.5);
        let ev = ctrl.handle_tab_drop(82.0, 0.5).unwrap();
        assert_eq!(
            ev,
            TabGroupEvent::TabMovedToPane {
                from_pane_idx: 0,
                to_pane_idx: 1,
                tab_id: "t1".into(),
                insert_idx: 0, // inserted before x0
            }
        );
        // pane 1 now has [t1, x0].
        assert_eq!(ctrl.panes[1].tabs()[0].id, "t1");
        assert_eq!(ctrl.panes[1].tabs()[1].id, "x0");
    }

    #[test]
    fn tab_drop_source_collapses_when_last_tab_moved() {
        // Build a controller where pane 0 has only one tab.
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

        // Drag "only" from pane 0 to center of pane 1.
        ctrl.handle_tab_drag_start(3.0, 0.5); // x=3 hits "only" slot
        let ev = ctrl.handle_tab_drop(120.0, 5.0).unwrap();

        // Source pane (idx 0) was collapsed; what was pane 1 is now pane 0.
        assert_eq!(ctrl.pane_count(), 1);
        assert_eq!(
            ev,
            TabGroupEvent::TabMovedToPane {
                from_pane_idx: 0,
                to_pane_idx: 0, // what was pane 1 is now pane 0 after collapse
                tab_id: "only".into(),
                insert_idx: 1,
            }
        );
        assert_eq!(ctrl.panes[0].tabs().len(), 2);
        assert_eq!(ctrl.panes[0].tabs()[1].id, "only");
        let sum: f32 = ctrl.pane_fractions.iter().sum();
        assert!((sum - 1.0).abs() < 0.001);
    }

    // ── handle_tab_drop: split to new pane ────────────────────────────────────

    #[test]
    fn tab_drop_splits_right_edge_creates_pane_after_target() {
        let mut ctrl = make_two_pane();
        // Drag t1 from pane 0, drop on right edge of pane 1 (x=150, y=5).
        // rel_x = 150-80=70 >= edge_w=64 → Split(Right).
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
        // Three panes: pane0(t0), pane1(x0), pane2(t1).
        assert_eq!(ctrl.pane_count(), 3);
        assert_eq!(ctrl.panes[0].tabs()[0].id, "t0");
        assert_eq!(ctrl.panes[1].tabs()[0].id, "x0");
        assert_eq!(ctrl.panes[2].tabs()[0].id, "t1");
        let sum: f32 = ctrl.pane_fractions.iter().sum();
        assert!((sum - 1.0).abs() < 0.001);
    }

    #[test]
    fn tab_drop_splits_left_edge_creates_pane_before_target() {
        let mut ctrl = make_two_pane();
        // Drop on left edge of pane 1 (x=83, y=5 → rel_x=3 < edge_w=16 → Split(Left)).
        ctrl.handle_tab_drag_start(9.0, 0.5); // drag t1
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
        // Three panes: pane0(t0), pane1(t1), pane2(x0).
        assert_eq!(ctrl.pane_count(), 3);
        assert_eq!(ctrl.panes[0].tabs()[0].id, "t0");
        assert_eq!(ctrl.panes[1].tabs()[0].id, "t1");
        assert_eq!(ctrl.panes[2].tabs()[0].id, "x0");
    }

    #[test]
    fn tab_drop_split_source_collapses_when_last_tab() {
        // Pane 0 has only "only". Dragging it to the right edge of pane 1
        // should: collapse pane 0, insert new pane (with "only") after pane 1.
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

        ctrl.handle_tab_drag_start(3.0, 0.5); // drag "only"
                                              // Drop on right edge of pane 1 (x=150, y=5).
        let ev = ctrl.handle_tab_drop(150.0, 5.0).unwrap();

        // Pane 0 ("only") collapses; remaining panes: old-pane1 → idx 0, new → idx 1.
        // insert_pos was 2 (after pane 1 at idx 1), then adjusted to 1 (from < insert_pos).
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
        let sum: f32 = ctrl.pane_fractions.iter().sum();
        assert!((sum - 1.0).abs() < 0.001);
    }

    #[test]
    fn tab_drop_only_tab_only_pane_split_is_noop() {
        // Only one pane with one tab: splitting to a new pane would be meaningless.
        let mut ctrl = TabGroupController::with_pane(
            "p0",
            vec![tab("t0", "only.rs", true)],
            "t0",
            SplitDirection::Horizontal,
        );
        prime_pane(&mut ctrl, 0, 0.0, 0.0, 80.0, 8.0, 2.0);

        ctrl.handle_tab_drag_start(3.0, 0.5);
        // Drop on right edge of pane 0 (x=70, y=5 → rel_x=70 >= 64 → Split(Right)).
        let ev = ctrl.handle_tab_drop(70.0, 5.0);
        assert_eq!(ev, None);
        assert_eq!(ctrl.pane_count(), 1);
        assert_eq!(ctrl.panes[0].tabs().len(), 1);
    }

    #[test]
    fn tab_drop_clears_drag_state_on_noop() {
        let mut ctrl = make_two_pane();
        ctrl.handle_tab_drag_start(9.0, 0.5);
        // Drop outside all groups.
        let ev = ctrl.handle_tab_drop(999.0, 999.0);
        assert_eq!(ev, None);
        assert!(!ctrl.is_tab_dragging());
    }
}
