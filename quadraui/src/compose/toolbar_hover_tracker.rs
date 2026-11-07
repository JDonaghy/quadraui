//! Host-side hover trackers: convert `MouseMoved` events into the
//! "which item is under the cursor" answer a primitive's rasteriser
//! needs, without the host hand-rolling screen-space-to-layout
//! conversion itself.
//!
//! Every tracker here ([`ToolbarHoverTracker`], [`ListHoverTracker`],
//! [`TreeHoverTracker`]) is a thin, item-typed wrapper over the same
//! private [`HoverCore`] engine: feed it a layout's `hit_test` result
//! on every mouse move, and it tells you whether the hovered item
//! changed (so you know to redraw) and what it is now. The item type
//! differs per primitive — [`crate::Toolbar`] buttons have a stable
//! [`WidgetId`], but [`crate::ListView`]/[`crate::TreeView`] rows don't
//! (see `interaction.rs`'s module doc for why [`crate::InteractionState`]
//! stays `WidgetId`-keyed and these two trackers use a row index
//! instead) — so each wrapper picks the item type its primitive's
//! `*Hit` enum actually produces.
//!
//! ## Usage
//!
//! ```ignore
//! struct App {
//!     hover: ToolbarHoverTracker,
//!     // ...
//! }
//!
//! impl AppLogic for App {
//!     fn handle(&mut self, ev: UiEvent, backend: &mut dyn Backend) -> Reaction {
//!         if let UiEvent::MouseMoved { position, .. } = ev {
//!             let layout = backend.toolbar_layout(self.bar_rect(backend), &self.bar());
//!             if self.hover.update(&layout, position.x, position.y) {
//!                 return Reaction::Redraw;
//!             }
//!         }
//!         // ...
//!     }
//!
//!     fn render(&self, backend: &mut dyn Backend, _area: ()) {
//!         backend.draw_toolbar_interactive(
//!             self.bar_rect(backend),
//!             &self.bar(),
//!             &InteractionState::from_parts(self.hover.hovered_id().cloned(), None),
//!         );
//!     }
//! }
//! ```

use crate::primitives::list::{ListViewHit, ListViewLayout};
use crate::primitives::toolbar::{ToolbarHit, ToolbarLayout};
use crate::primitives::tree::{TreeViewHit, TreeViewLayout};
use crate::types::WidgetId;

/// Shared bookkeeping every tracker in this module wraps: "what's the
/// current item, and did it just change?" — one `Option<T>` plus the
/// equality check every `update`/`clear` call needs. Not `pub`: each
/// primitive's tracker is the public-facing, correctly-typed wrapper.
#[derive(Debug, Clone)]
struct HoverCore<T: Clone + PartialEq> {
    current: Option<T>,
}

// Manual `Default` rather than `#[derive(Default)]`: the derive would
// require `T: Default` even though `Option<T>`'s own `None` default
// doesn't need one — and every tracker's item type here (`WidgetId`,
// `usize`) is `Copy`/`Clone` but not all are `Default`.
impl<T: Clone + PartialEq> Default for HoverCore<T> {
    fn default() -> Self {
        Self { current: None }
    }
}

impl<T: Clone + PartialEq> HoverCore<T> {
    fn new() -> Self {
        Self { current: None }
    }

    /// Returns `true` if `new` differs from the stored value (caller
    /// should redraw).
    fn set(&mut self, new: Option<T>) -> bool {
        if new != self.current {
            self.current = new;
            true
        } else {
            false
        }
    }

    fn clear(&mut self) -> bool {
        self.set(None)
    }

    fn current(&self) -> Option<&T> {
        self.current.as_ref()
    }
}

/// Tracks the toolbar button currently under the mouse. Cheap to keep
/// per toolbar (or per panel, if a panel hosts multiple) — one
/// `Option<WidgetId>` plus a single `hit_test` call per mouse-move.
#[derive(Debug, Default, Clone)]
pub struct ToolbarHoverTracker {
    core: HoverCore<WidgetId>,
}

impl ToolbarHoverTracker {
    /// Construct an empty tracker. No button is hovered initially.
    pub fn new() -> Self {
        Self {
            core: HoverCore::new(),
        }
    }

    /// Update the hover state from a mouse-position + the current
    /// toolbar layout. Returns `true` if the hovered button changed
    /// (caller should redraw), `false` if it stayed the same.
    pub fn update(&mut self, layout: &ToolbarLayout, x: f32, y: f32) -> bool {
        let new_hover = match layout.hit_test(x, y) {
            ToolbarHit::Button(id) => Some(id),
            ToolbarHit::Empty => None,
        };
        self.core.set(new_hover)
    }

    /// Clear the hover state — call from `MouseLeft` handlers (or
    /// when the toolbar is dismissed) so the highlight doesn't stick
    /// after the cursor has left.
    pub fn clear(&mut self) -> bool {
        self.core.clear()
    }

    /// Borrowed view of the current hovered id, ready to pass straight
    /// to `Backend::draw_toolbar(..., hovered_id, ...)`.
    pub fn hovered_id(&self) -> Option<&WidgetId> {
        self.core.current()
    }

    /// Owned copy of the current hovered id. Convenience for hosts
    /// that need to compare against other state.
    pub fn current(&self) -> Option<WidgetId> {
        self.core.current().copied()
    }
}

/// Tracks the [`crate::ListView`] row index currently under the mouse.
///
/// Row-index-keyed rather than [`WidgetId`]-keyed — a `ListItem` has no
/// stable id of its own (see `interaction.rs`'s module doc) — so this
/// is the dedicated tracker [`crate::Backend::draw_list_with_state`]
/// hosts feed instead of an [`crate::InteractionState`].
#[derive(Debug, Default, Clone)]
pub struct ListHoverTracker {
    core: HoverCore<usize>,
}

impl ListHoverTracker {
    /// Construct an empty tracker. No row is hovered initially.
    pub fn new() -> Self {
        Self {
            core: HoverCore::new(),
        }
    }

    /// Update the hover state from a mouse position + the current
    /// list layout. Returns `true` if the hovered row changed (caller
    /// should redraw), `false` if it stayed the same. Hovering the
    /// title row or empty space below the last item clears the hover,
    /// same as the cursor leaving the list entirely.
    pub fn update(&mut self, layout: &ListViewLayout, x: f32, y: f32) -> bool {
        let new_hover = match layout.hit_test(x, y) {
            ListViewHit::Item(idx) => Some(idx),
            ListViewHit::Title | ListViewHit::Empty => None,
        };
        self.core.set(new_hover)
    }

    /// Clear the hover state — call from `MouseLeft` handlers so the
    /// highlight doesn't stick after the cursor has left the list.
    pub fn clear(&mut self) -> bool {
        self.core.clear()
    }

    /// The currently hovered item index, ready to pass straight to
    /// `ListPaintState::with_hovered_idx` for
    /// `Backend::draw_list_with_state`.
    pub fn hovered_idx(&self) -> Option<usize> {
        self.core.current().copied()
    }
}

/// Tracks the [`crate::TreeView`] row index currently under the mouse.
///
/// Like [`ListHoverTracker`], row-index-keyed rather than
/// [`WidgetId`]-keyed since a `TreeRow` has no stable id of its own —
/// only a [`crate::TreePath`] that shifts as siblings expand/collapse.
/// A hovered [`TreeViewHit::Chevron`] counts as hovering its row (the
/// chevron is part of the row's visual band), so the row background
/// tints the same way whether the cursor sits over the glyph or the
/// label.
#[derive(Debug, Default, Clone)]
pub struct TreeHoverTracker {
    core: HoverCore<usize>,
}

impl TreeHoverTracker {
    /// Construct an empty tracker. No row is hovered initially.
    pub fn new() -> Self {
        Self {
            core: HoverCore::new(),
        }
    }

    /// Update the hover state from a mouse position + the current
    /// tree layout. Returns `true` if the hovered row changed (caller
    /// should redraw), `false` if it stayed the same.
    pub fn update(&mut self, layout: &TreeViewLayout, x: f32, y: f32) -> bool {
        let new_hover = match layout.hit_test(x, y) {
            TreeViewHit::Row(idx) | TreeViewHit::Chevron(idx) => Some(idx),
            TreeViewHit::Empty => None,
        };
        self.core.set(new_hover)
    }

    /// Clear the hover state — call from `MouseLeft` handlers so the
    /// highlight doesn't stick after the cursor has left the tree.
    pub fn clear(&mut self) -> bool {
        self.core.clear()
    }

    /// The currently hovered row index, ready to pass straight to
    /// `TreePaintState::with_hovered_idx` for
    /// `Backend::draw_tree_with_state`.
    pub fn hovered_idx(&self) -> Option<usize> {
        self.core.current().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::toolbar::{Toolbar, ToolbarButton, ToolbarItemMeasure};

    fn mk_action(id: &str, label: &str) -> ToolbarButton {
        ToolbarButton::Action {
            spec: None,
            id: WidgetId::new(id),
            label: label.to_string(),
            icon: None,
            key_hint: None,
            enabled: true,
            is_active: false,
            tooltip: String::new(),
        }
    }

    fn sample_layout() -> ToolbarLayout {
        let bar = Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![mk_action("a", "A"), mk_action("b", "B")],
            bg: None,
            focused_index: None,
        };
        bar.layout(0.0, 0.0, 40.0, 1.0, |_| ToolbarItemMeasure::new(6.0))
    }

    #[test]
    fn update_records_button_under_cursor() {
        let layout = sample_layout();
        let mut t = ToolbarHoverTracker::new();
        assert!(t.update(&layout, 1.0, 0.0));
        assert_eq!(t.hovered_id().map(|i| i.as_str()), Some("a"));
    }

    #[test]
    fn update_returns_false_when_unchanged() {
        let layout = sample_layout();
        let mut t = ToolbarHoverTracker::new();
        // First update changes (None -> Some(a)).
        assert!(t.update(&layout, 1.0, 0.0));
        // Second update with the same position should not signal change.
        assert!(!t.update(&layout, 2.0, 0.0));
    }

    #[test]
    fn update_returns_true_when_moving_between_buttons() {
        let layout = sample_layout();
        let mut t = ToolbarHoverTracker::new();
        assert!(t.update(&layout, 1.0, 0.0)); // hits "a"
        assert!(t.update(&layout, 7.0, 0.0)); // hits "b"
        assert_eq!(t.hovered_id().map(|i| i.as_str()), Some("b"));
    }

    #[test]
    fn update_clears_when_cursor_leaves_buttons() {
        let layout = sample_layout();
        let mut t = ToolbarHoverTracker::new();
        assert!(t.update(&layout, 1.0, 0.0));
        // Far past the right edge of the painted items.
        assert!(t.update(&layout, 30.0, 0.0));
        assert!(t.hovered_id().is_none());
    }

    #[test]
    fn clear_resets_hover_state() {
        let layout = sample_layout();
        let mut t = ToolbarHoverTracker::new();
        t.update(&layout, 1.0, 0.0);
        assert!(t.clear());
        assert!(t.hovered_id().is_none());
        // Clearing an already-empty tracker is a no-op (returns false).
        assert!(!t.clear());
    }
}

#[cfg(test)]
mod list_tests {
    use super::*;
    use crate::primitives::list::{ListItem, ListItemMeasure, ListView};
    use crate::types::{Decoration, StyledText};

    fn item(label: &str) -> ListItem {
        ListItem {
            text: StyledText::plain(label.to_string()),
            icon: None,
            detail: None,
            decoration: Decoration::Normal,
        }
    }

    fn sample_layout() -> ListViewLayout {
        let list = ListView {
            id: WidgetId::new("l"),
            title: None,
            items: vec![item("a"), item("b"), item("c")],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
        };
        list.layout(40.0, 10.0, 0.0, |_| ListItemMeasure::new(1.0))
    }

    #[test]
    fn update_records_row_under_cursor() {
        let layout = sample_layout();
        let mut t = ListHoverTracker::new();
        assert!(t.update(&layout, 1.0, 1.0));
        assert_eq!(t.hovered_idx(), Some(1));
    }

    #[test]
    fn update_returns_false_when_unchanged() {
        let layout = sample_layout();
        let mut t = ListHoverTracker::new();
        assert!(t.update(&layout, 1.0, 0.0));
        assert!(!t.update(&layout, 2.0, 0.0));
    }

    #[test]
    fn update_clears_when_cursor_leaves_rows() {
        let layout = sample_layout();
        let mut t = ListHoverTracker::new();
        assert!(t.update(&layout, 1.0, 0.0));
        // Below the last row (3 rows of height 1.0 inside a 10.0-tall
        // viewport) is the empty tail.
        assert!(t.update(&layout, 1.0, 9.0));
        assert!(t.hovered_idx().is_none());
    }

    #[test]
    fn clear_resets_hover_state() {
        let layout = sample_layout();
        let mut t = ListHoverTracker::new();
        t.update(&layout, 1.0, 0.0);
        assert!(t.clear());
        assert!(t.hovered_idx().is_none());
        assert!(!t.clear());
    }
}

#[cfg(test)]
mod tree_tests {
    use super::*;
    use crate::primitives::tree::{TreeRow, TreeRowMeasure, TreeView};
    use crate::types::{Decoration, SelectionMode, StyledText, TreeStyle};

    fn row(path: Vec<u16>, label: &str) -> TreeRow {
        TreeRow {
            path,
            indent: 0,
            icon: None,
            text: StyledText::plain(label.to_string()),
            badge: None,
            is_expanded: None,
            decoration: Decoration::Normal,
            edit: None,
        }
    }

    fn sample_layout() -> TreeViewLayout {
        let tree = TreeView {
            id: WidgetId::new("t"),
            rows: vec![row(vec![0], "a"), row(vec![1], "b"), row(vec![2], "c")],
            selection_mode: SelectionMode::Single,
            selected_path: None,
            scroll_offset: 0,
            style: TreeStyle::default(),
            has_focus: true,
        };
        tree.layout(40.0, 10.0, |_| TreeRowMeasure::new(1.0))
    }

    #[test]
    fn update_records_row_under_cursor() {
        let layout = sample_layout();
        let mut t = TreeHoverTracker::new();
        assert!(t.update(&layout, 1.0, 1.0));
        assert_eq!(t.hovered_idx(), Some(1));
    }

    #[test]
    fn update_returns_false_when_unchanged() {
        let layout = sample_layout();
        let mut t = TreeHoverTracker::new();
        assert!(t.update(&layout, 1.0, 0.0));
        assert!(!t.update(&layout, 2.0, 0.0));
    }

    #[test]
    fn update_clears_when_cursor_leaves_rows() {
        let layout = sample_layout();
        let mut t = TreeHoverTracker::new();
        assert!(t.update(&layout, 1.0, 0.0));
        assert!(t.update(&layout, 1.0, 9.0));
        assert!(t.hovered_idx().is_none());
    }

    #[test]
    fn clear_resets_hover_state() {
        let layout = sample_layout();
        let mut t = TreeHoverTracker::new();
        t.update(&layout, 1.0, 0.0);
        assert!(t.clear());
        assert!(t.hovered_idx().is_none());
        assert!(!t.clear());
    }
}
