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
//! # Relationship to [`FocusGroup`]
//!
//! Both types cycle a "current" position with wrap-around.
//! `FocusRing` is a thin [`WidgetId`]-keyed shim over a
//! [`FocusGroup`] — the cycling itself has exactly one implementation,
//! in [`FocusGroup::cycle`]. The two types exist separately
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
///
/// # Focus-visible
///
/// Alongside "what is focused" ([`Self::current`]), `FocusRing` tracks
/// *how* it got there, in [`Self::is_focus_visible`] — VS Code's
/// `:focus-visible` convention: a keyboard move ([`Self::advance`],
/// [`Self::retreat`], [`Self::focus_first`]) shows the ring; a pointer
/// move ([`Self::set`], the click-to-focus path) focuses the widget but
/// suppresses it. A shared painter never sees this distinction — it only
/// receives whatever `bool` the host passes as `has_focus`/`focused` on
/// the primitive's own descriptor — so a host that wants the suppression
/// computes that bool as `ring.is_focused(id) && ring.is_focus_visible()`
/// rather than `ring.is_focused(id)` alone. [`crate::Button::focused`]
/// and [`crate::TextInput::has_focus`] both follow this convention.
#[derive(Debug, Clone)]
pub struct FocusRing {
    items: Vec<WidgetId>,
    group: FocusGroup,
    focus_visible: bool,
}

impl FocusRing {
    /// Create a new ring from anything convertible to `WidgetId`.
    ///
    /// The first item is focused initially, with the ring visible (as if
    /// reached by keyboard). Pass an empty vec for no focus.
    pub fn new(ids: Vec<impl Into<WidgetId>>) -> Self {
        let items: Vec<WidgetId> = ids.into_iter().map(Into::into).collect();
        let mut group = FocusGroup::new(items.len());
        if !items.is_empty() {
            group.set_active(Some(0));
        }
        Self {
            items,
            group,
            focus_visible: true,
        }
    }

    /// Move focus to the next item (wraps around), and mark the ring
    /// focus-visible. No-op if nothing is currently focused (empty ring,
    /// or focus was [`Self::clear`]ed).
    pub fn advance(&mut self) {
        if self.group.active().is_some() {
            self.group.cycle(1);
            self.focus_visible = true;
        }
    }

    /// Move focus to the previous item (wraps around), and mark the ring
    /// focus-visible. No-op if nothing is currently focused (empty ring,
    /// or focus was [`Self::clear`]ed).
    pub fn retreat(&mut self) {
        if self.group.active().is_some() {
            self.group.cycle(-1);
            self.focus_visible = true;
        }
    }

    /// Set focus to a specific widget by ID — the click-to-focus path.
    /// No-op if the ID isn't in the ring. Marks the ring **not**
    /// focus-visible: a mouse click focuses the widget without showing
    /// the ring, matching VS Code. A host that wants the ring shown
    /// after a programmatic `set` anyway (keyboard-equivalent focus,
    /// e.g. restoring focus after a dialog closes) should call
    /// [`Self::set_focus_visible`] immediately after.
    pub fn set(&mut self, id: &WidgetId) {
        if let Some(idx) = self.items.iter().position(|item| item == id) {
            self.group.set_active(Some(idx));
            self.focus_visible = false;
        }
    }

    /// Clear focus (nothing focused).
    pub fn clear(&mut self) {
        self.group.set_active(None);
    }

    /// Focus the first item, and mark the ring focus-visible. No-op if
    /// empty.
    pub fn focus_first(&mut self) {
        if !self.items.is_empty() {
            self.group.set_active(Some(0));
            self.focus_visible = true;
        }
    }

    /// Whether the focus ring should currently be painted — `true` after
    /// a keyboard move, `false` after [`Self::set`] (a mouse click). See
    /// the type doc's "Focus-visible" section for how a host applies
    /// this to a primitive's `has_focus`/`focused` descriptor field.
    pub fn is_focus_visible(&self) -> bool {
        self.focus_visible
    }

    /// Force the focus-visible flag directly, independent of how focus
    /// last moved — e.g. a host that wants the ring shown immediately
    /// after a programmatic [`Self::set`] (focus restored by code, not a
    /// click).
    pub fn set_focus_visible(&mut self, visible: bool) {
        self.focus_visible = visible;
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
    fn new_ring_starts_focus_visible() {
        let ring = FocusRing::new(vec!["a", "b"]);
        assert!(ring.is_focus_visible());
    }

    /// The acceptance scenario for VS Code's `:focus-visible` convention:
    /// a Tab-reached widget shows the ring, and clicking a different one
    /// suppresses it even though focus did move.
    #[test]
    fn click_to_focus_suppresses_the_ring_but_keyboard_move_restores_it() {
        let mut ring = FocusRing::new(vec!["a", "b", "c"]);
        assert!(ring.is_focus_visible(), "Tab landed here, so it's visible");

        ring.set(&WidgetId::new("b"));
        assert!(ring.is_focused(&WidgetId::new("b")), "click did move focus");
        assert!(
            !ring.is_focus_visible(),
            "a mouse click must suppress the ring"
        );

        ring.advance();
        assert!(
            ring.is_focus_visible(),
            "a subsequent keyboard move restores the ring"
        );
    }

    #[test]
    fn retreat_and_focus_first_also_mark_focus_visible() {
        let mut ring = FocusRing::new(vec!["a", "b"]);
        ring.set(&WidgetId::new("b"));
        assert!(!ring.is_focus_visible());

        ring.retreat();
        assert!(ring.is_focus_visible(), "Shift+Tab restores visibility");

        ring.set(&WidgetId::new("a"));
        assert!(!ring.is_focus_visible());
        ring.focus_first();
        assert!(ring.is_focus_visible());
    }

    #[test]
    fn set_focus_visible_overrides_directly() {
        let mut ring = FocusRing::new(vec!["a"]);
        ring.set(&WidgetId::new("a"));
        assert!(!ring.is_focus_visible());

        ring.set_focus_visible(true);
        assert!(ring.is_focus_visible());
    }

    /// Clicking an id that isn't in the ring is already a no-op for
    /// focus; it must be a no-op for visibility too.
    #[test]
    fn set_unknown_id_does_not_change_focus_visible() {
        let mut ring = FocusRing::new(vec!["a", "b"]);
        assert!(ring.is_focus_visible());
        ring.set(&WidgetId::new("z"));
        assert!(ring.is_focus_visible());
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

    /// A headless proof that `TextInput` follows the same `has_focus`
    /// composition convention [`FocusRing::is_focus_visible`]'s type
    /// doc describes for `Button::focused`: a Tab-reached text input
    /// reports focus-visible, a clicked one doesn't — even though the
    /// click did move focus to it. Every rasteriser that paints a
    /// ring (or, on TUI, a focus-tinted border) keys off the single
    /// composed bool below, never off `is_focused` alone.
    #[test]
    fn focused_text_input_reports_focus_visible_after_tab_not_after_click() {
        use crate::TextInput;

        let id = WidgetId::new("search:input");
        let mut ring = FocusRing::new(vec![id.clone()]);
        let mut input = TextInput::new(id.clone());

        // `FocusRing::new` starts focus-visible, as if Tab just landed
        // here.
        input.has_focus = ring.is_focused(&id) && ring.is_focus_visible();
        assert!(
            input.has_focus,
            "a Tab-reached text input must report focus-visible"
        );

        // A mouse click re-focuses the same widget via `set`.
        ring.set(&id);
        input.has_focus = ring.is_focused(&id) && ring.is_focus_visible();
        assert!(
            ring.is_focused(&id),
            "the click did move focus to this input"
        );
        assert!(
            !input.has_focus,
            "a mouse click must suppress the text input's focus-visible ring"
        );

        // Shift+Tab afterwards restores it.
        ring.retreat();
        input.has_focus = ring.is_focused(&id) && ring.is_focus_visible();
        assert!(
            input.has_focus,
            "a subsequent keyboard move restores focus-visible"
        );
    }
}
