//! Keyboard cursor + hover state for `AppShell`'s activity bar.
//!
//! Split out of `app_shell.rs`, which otherwise interleaves this bar's own
//! focus/cursor/hover bookkeeping with its builder methods, click
//! dispatch, and layout. [`ActivityBarKeyboard`] owns only the four pieces
//! of state the keyboard model needs; it has no notion of `PanelDefinition`
//! or `WidgetId` — `AppShell` still resolves "which item is at this index"
//! itself, passing in `total` (the combined top-panel + bottom-item count)
//! each time a bound is needed.

/// Keyboard focus/cursor + mouse-hover state for an activity bar's
/// logical item sequence (top panels first, then bottom items).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct ActivityBarKeyboard {
    /// Whether the bar currently holds keyboard focus — see
    /// [`crate::compose::AppShell::activity_keyboard_focused`].
    focused: bool,
    /// Cursor position in the logical item sequence. Saturates at the
    /// ends; does not wrap. Meaningful even when `!focused`, so a
    /// `cursor_visible`-only host (see below) still has somewhere to
    /// paint the highlight.
    cursor: usize,
    /// Show the keyboard cursor highlight without the key-event redirect
    /// `focused` triggers — see
    /// [`crate::compose::AppShell::set_activity_cursor_visible`].
    cursor_visible: bool,
    /// Index of the item currently under the mouse pointer, if any.
    hovered_idx: Option<usize>,
}

impl ActivityBarKeyboard {
    /// Unfocused, cursor at the top item, nothing hovered.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn focused(&self) -> bool {
        self.focused
    }

    pub(crate) fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    pub(crate) fn cursor_visible(&self) -> bool {
        self.cursor_visible
    }

    pub(crate) fn set_cursor_visible(&mut self, visible: bool) {
        self.cursor_visible = visible;
    }

    pub(crate) fn hovered_idx(&self) -> Option<usize> {
        self.hovered_idx
    }

    pub(crate) fn set_hovered_idx(&mut self, idx: Option<usize>) {
        self.hovered_idx = idx;
    }

    pub(crate) fn cursor(&self) -> usize {
        self.cursor
    }

    /// Set the cursor to an exact index with no clamping against `total`.
    /// For callers that have already computed a valid index themselves
    /// (`AppShell`'s panel add/remove bookkeeping, which knows the new
    /// item count) — [`Self::set_cursor`] below is the clamping entry
    /// point for the public-facing navigation API.
    pub(crate) fn set_cursor_raw(&mut self, idx: usize) {
        self.cursor = idx;
    }

    /// Move the cursor to the next item. Saturates at `total - 1` —
    /// does not wrap. No-op when `total == 0`.
    pub(crate) fn select_next(&mut self, total: usize) {
        if total > 0 {
            self.cursor = (self.cursor + 1).min(total - 1);
        }
    }

    /// Move the cursor to the previous item. Saturates at index 0 —
    /// does not wrap.
    pub(crate) fn select_prev(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// Set the cursor to `idx`, clamped to `[0, total - 1]`. No-op when
    /// `total == 0`.
    pub(crate) fn set_cursor(&mut self, idx: usize, total: usize) {
        if total > 0 {
            self.cursor = idx.min(total - 1);
        }
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
