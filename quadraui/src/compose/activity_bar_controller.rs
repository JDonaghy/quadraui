//! `AppShell`'s view of [`crate::ActivityBar`]'s own keyboard/hover state.
//!
//! [`ActivityBarKeyboard`] is a borrow-friendly handle onto an
//! [`ActivityBarState`] — the widget's own
//! [`crate::interactive::Interactive::State`], where the cursor model
//! lives ([`ActivityBarState::select_next`] /
//! [`select_prev`](ActivityBarState::select_prev) /
//! [`set_cursor`](ActivityBarState::set_cursor), all driven by
//! [`crate::behaviour::nav::FlatNav`]). `AppShell` otherwise interleaves
//! this bar's focus/cursor/hover bookkeeping with its builder methods,
//! click dispatch and layout, so it keeps reaching the state through the
//! accessors here; it has no notion of `PanelDefinition` or `WidgetId`
//! either — `AppShell` still resolves "which item is at this index"
//! itself, passing in `total` (the combined top-panel + bottom-item
//! count) each time a bound is needed.

use crate::primitives::activity_bar::ActivityBarState;

/// Keyboard focus/cursor + mouse-hover state for an activity bar's
/// logical item sequence (top panels first, then bottom items).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct ActivityBarKeyboard {
    state: ActivityBarState,
}

impl ActivityBarKeyboard {
    /// Unfocused, cursor at the top item, nothing hovered.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn focused(&self) -> bool {
        self.state.focused
    }

    pub(crate) fn set_focused(&mut self, focused: bool) {
        self.state.focused = focused;
    }

    pub(crate) fn cursor_visible(&self) -> bool {
        self.state.cursor_visible
    }

    pub(crate) fn set_cursor_visible(&mut self, visible: bool) {
        self.state.cursor_visible = visible;
    }

    pub(crate) fn hovered_idx(&self) -> Option<usize> {
        self.state.hovered_idx
    }

    pub(crate) fn set_hovered_idx(&mut self, idx: Option<usize>) {
        self.state.hovered_idx = idx;
    }

    pub(crate) fn cursor(&self) -> usize {
        self.state.cursor
    }

    /// Set the cursor to an exact index with no clamping against `total`
    /// — see [`ActivityBarState::set_cursor_raw`].
    pub(crate) fn set_cursor_raw(&mut self, idx: usize) {
        self.state.set_cursor_raw(idx);
    }

    /// Move the cursor to the next item. Saturates at `total - 1` —
    /// does not wrap. No-op when `total == 0`.
    pub(crate) fn select_next(&mut self, total: usize) {
        self.state.select_next(total);
    }

    /// Move the cursor to the previous item. Saturates at index 0 —
    /// does not wrap.
    pub(crate) fn select_prev(&mut self) {
        self.state.select_prev();
    }

    /// Set the cursor to `idx`, clamped to `[0, total - 1]`. No-op when
    /// `total == 0`.
    pub(crate) fn set_cursor(&mut self, idx: usize, total: usize) {
        self.state.set_cursor(idx, total);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_is_unfocused_with_cursor_at_zero() {
        let k = ActivityBarKeyboard::new();
        assert!(!k.focused());
        assert!(!k.cursor_visible());
        assert_eq!(k.cursor(), 0);
        assert_eq!(k.hovered_idx(), None);
    }

    #[test]
    fn select_next_saturates_at_total_minus_one() {
        let mut k = ActivityBarKeyboard::new();
        k.select_next(2);
        assert_eq!(k.cursor(), 1);
        k.select_next(2);
        assert_eq!(k.cursor(), 1);
    }

    #[test]
    fn select_next_is_a_no_op_when_total_is_zero() {
        let mut k = ActivityBarKeyboard::new();
        k.select_next(0);
        assert_eq!(k.cursor(), 0);
    }

    #[test]
    fn select_next_from_a_cursor_past_the_end_lands_on_the_last_item() {
        let mut k = ActivityBarKeyboard::new();
        // `set_cursor_raw` does not clamp, so a shrunk item count can leave
        // the cursor past the end; a forward move pulls it to the last item
        // rather than jumping back to the first.
        k.set_cursor_raw(9);
        k.select_next(3);
        assert_eq!(k.cursor(), 2);
    }

    #[test]
    fn select_prev_saturates_at_zero() {
        let mut k = ActivityBarKeyboard::new();
        k.set_cursor_raw(1);
        k.select_prev();
        assert_eq!(k.cursor(), 0);
        k.select_prev();
        assert_eq!(k.cursor(), 0);
    }

    #[test]
    fn set_cursor_clamps_to_total_minus_one() {
        let mut k = ActivityBarKeyboard::new();
        k.set_cursor(10, 3);
        assert_eq!(k.cursor(), 2);
    }

    #[test]
    fn set_cursor_is_a_no_op_when_total_is_zero() {
        let mut k = ActivityBarKeyboard::new();
        k.set_cursor_raw(1);
        k.set_cursor(0, 0);
        assert_eq!(k.cursor(), 1);
    }

    #[test]
    fn focused_and_cursor_visible_are_independent() {
        let mut k = ActivityBarKeyboard::new();
        k.set_cursor_visible(true);
        assert!(!k.focused());
        assert!(k.cursor_visible());
        k.set_focused(true);
        assert!(k.focused());
    }

    #[test]
    fn hovered_idx_round_trips() {
        let mut k = ActivityBarKeyboard::new();
        k.set_hovered_idx(Some(2));
        assert_eq!(k.hovered_idx(), Some(2));
        k.set_hovered_idx(None);
        assert_eq!(k.hovered_idx(), None);
    }
}
