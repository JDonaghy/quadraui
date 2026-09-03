//! `FocusRing` — a compose helper for Tab/Shift+Tab focus cycling
//! through a list of [`WidgetId`]s.
//!
//! Eliminates repeated modulo arithmetic and ID-array management.
//! Apps register focusable widget IDs once, then call [`advance`],
//! [`retreat`], or [`set`] from their event handler.
//!
//! ```ignore
//! let mut ring = FocusRing::new(vec!["search", "toggles", "replace", "buttons"]);
//! // Tab:
//! ring.advance();
//! // Shift+Tab:
//! ring.retreat();
//! // Click-to-focus:
//! ring.set(&WidgetId::new("replace"));
//! // Query:
//! assert_eq!(ring.current(), Some(&WidgetId::new("replace")));
//! ```
//!
//! # Relationship to [`FocusGroup`](super::FocusGroup)
//!
//! Both types cycle a "current" position with wrap-around, and used to
//! carry two independent copies of the same modulo arithmetic (#509).
//! `FocusRing` is now a thin [`WidgetId`]-keyed shim over a
//! [`FocusGroup`] — the cycling itself has exactly one implementation,
//! in [`FocusGroup::cycle`]. The two types still exist separately
//! because their call sites need different keys: `FocusRing` suits a
//! fixed list of named widgets (form fields looked up by the
//! [`WidgetId`] a hit-test returns — see `examples/common/form_groups.rs`),
//! while `FocusGroup` suits a dynamically-resized run of anonymous
//! regions indexed 0..N (sidebar sections, tab-group panes — see
//! [`crate::compose::sidebar_system`] / [`crate::compose::tab_group`]),
//! where there's no natural `WidgetId` to key by and the count changes
//! at runtime as sections/panes are added or removed.

use super::focus_group::FocusGroup;
use crate::types::WidgetId;

/// Manages Tab/Shift+Tab cycling through a fixed list of focusable widgets.
#[derive(Debug, Clone)]
pub struct FocusRing {
    items: Vec<WidgetId>,
    group: FocusGroup,
}

impl FocusRing {
    /// Create a new ring from anything convertible to `WidgetId`.
    ///
    /// The first item is focused initially. Pass an empty vec for no focus.
    pub fn new(ids: Vec<impl Into<WidgetId>>) -> Self {
        let items: Vec<WidgetId> = ids.into_iter().map(Into::into).collect();
        let mut group = FocusGroup::new(items.len());
        if !items.is_empty() {
            group.set_active(Some(0));
        }
        Self { items, group }
    }

    /// Move focus to the next item (wraps around). No-op if nothing is
    /// currently focused (empty ring, or focus was [`Self::clear`]ed).
    pub fn advance(&mut self) {
        if self.group.active().is_some() {
            self.group.cycle(1);
        }
    }

    /// Move focus to the previous item (wraps around). No-op if nothing
    /// is currently focused (empty ring, or focus was [`Self::clear`]ed).
    pub fn retreat(&mut self) {
        if self.group.active().is_some() {
            self.group.cycle(-1);
        }
    }

    /// Set focus to a specific widget by ID. No-op if the ID isn't in the ring.
    pub fn set(&mut self, id: &WidgetId) {
        if let Some(idx) = self.items.iter().position(|item| item == id) {
            self.group.set_active(Some(idx));
        }
    }

    /// Clear focus (nothing focused).
    pub fn clear(&mut self) {
        self.group.set_active(None);
    }

    /// Focus the first item. No-op if empty.
    pub fn focus_first(&mut self) {
        if !self.items.is_empty() {
            self.group.set_active(Some(0));
        }
    }

    /// The currently focused widget ID, or `None` if nothing is focused.
    pub fn current(&self) -> Option<&WidgetId> {
        self.group.active().and_then(|i| self.items.get(i))
    }

    /// The index of the currently focused item.
    pub fn current_index(&self) -> Option<usize> {
        self.group.active()
    }

    /// Whether the given ID is currently focused.
    pub fn is_focused(&self, id: &WidgetId) -> bool {
        self.current() == Some(id)
    }

    /// The number of items in the ring.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the ring is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The list of widget IDs in order.
    pub fn items(&self) -> &[WidgetId] {
        &self.items
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_focuses_first() {
        let ring = FocusRing::new(vec!["a", "b", "c"]);
        assert_eq!(ring.current(), Some(&WidgetId::new("a")));
        assert_eq!(ring.current_index(), Some(0));
    }

    #[test]
    fn empty_ring() {
        let ring = FocusRing::new(Vec::<&str>::new());
        assert_eq!(ring.current(), None);
        assert!(ring.is_empty());
    }

    #[test]
    fn advance_wraps() {
        let mut ring = FocusRing::new(vec!["a", "b", "c"]);
        ring.advance();
        assert_eq!(ring.current(), Some(&WidgetId::new("b")));
        ring.advance();
        assert_eq!(ring.current(), Some(&WidgetId::new("c")));
        ring.advance();
        assert_eq!(ring.current(), Some(&WidgetId::new("a")));
    }

    #[test]
    fn retreat_wraps() {
        let mut ring = FocusRing::new(vec!["a", "b", "c"]);
        ring.retreat();
        assert_eq!(ring.current(), Some(&WidgetId::new("c")));
        ring.retreat();
        assert_eq!(ring.current(), Some(&WidgetId::new("b")));
    }

    #[test]
    fn set_by_id() {
        let mut ring = FocusRing::new(vec!["a", "b", "c"]);
        ring.set(&WidgetId::new("c"));
        assert_eq!(ring.current_index(), Some(2));
        assert!(ring.is_focused(&WidgetId::new("c")));
        assert!(!ring.is_focused(&WidgetId::new("a")));
    }

    #[test]
    fn set_unknown_id_is_noop() {
        let mut ring = FocusRing::new(vec!["a", "b"]);
        ring.set(&WidgetId::new("z"));
        assert_eq!(ring.current(), Some(&WidgetId::new("a")));
    }

    #[test]
    fn clear_and_refocus() {
        let mut ring = FocusRing::new(vec!["a", "b"]);
        ring.clear();
        assert_eq!(ring.current(), None);
        ring.advance(); // no-op when cleared
        assert_eq!(ring.current(), None);
        ring.focus_first();
        assert_eq!(ring.current(), Some(&WidgetId::new("a")));
    }

    #[test]
    fn advance_after_set() {
        let mut ring = FocusRing::new(vec!["a", "b", "c"]);
        ring.set(&WidgetId::new("b"));
        ring.advance();
        assert_eq!(ring.current(), Some(&WidgetId::new("c")));
    }

    #[test]
    fn single_item_ring() {
        let mut ring = FocusRing::new(vec!["only"]);
        assert_eq!(ring.current(), Some(&WidgetId::new("only")));
        ring.advance();
        assert_eq!(ring.current(), Some(&WidgetId::new("only")));
        ring.retreat();
        assert_eq!(ring.current(), Some(&WidgetId::new("only")));
    }
}
