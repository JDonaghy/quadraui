//! [`UndoStack`] — a generic, snapshot-based undo/redo history.
//!
//! See issue #833: `TextInput` had insert/delete/cursor/selection state
//! but no undo — the accelerator *names* (`KeyBinding::Undo`,
//! `KeyBinding::Redo`, see [`crate::accelerator`]) existed with nothing
//! behind them. This module gives any primitive (not just `TextInput`) a
//! reusable history type instead of a bespoke one per primitive.
//!
//! `UndoStack<T>` is deliberately snapshot-based rather than op-based: `T`
//! is whatever the caller considers "the state to restore" — a full
//! clone of the editable fields is simplest and is what
//! [`crate::primitives::text_input::TextInput::apply`] uses internally.
//! Callers that want operation-based (diff) undo instead can still use
//! this stack — just make `T` an inverse-op type — the stack itself only
//! ever moves whole `T` values between its two sides, it doesn't
//! interpret them.
//!
//! The stack does not record automatically: the caller decides what
//! counts as one undo step by choosing when to call [`UndoStack::record`]
//! (e.g. once per keystroke, or coalesced per typing burst — that policy
//! lives entirely with the caller).

use std::collections::VecDeque;

/// Generic bounded undo/redo stack over snapshots of type `T`.
///
/// `record` pushes the pre-mutation state onto the undo side and clears
/// any redo history (a fresh edit invalidates a previously-undone
/// future — standard editor semantics). `undo`/`redo` take the caller's
/// *current* live state so they can push it onto the opposite side,
/// mirroring the classic two-stack undo/redo implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoStack<T> {
    past: VecDeque<T>,
    future: Vec<T>,
    limit: usize,
}

impl<T> Default for UndoStack<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> UndoStack<T> {
    /// An unbounded stack — history grows until [`Self::clear`] is called.
    pub fn new() -> Self {
        Self::with_limit(usize::MAX)
    }

    /// A stack that discards its oldest entry once `limit` is exceeded.
    /// `limit == 0` disables history: [`Self::record`] becomes a no-op
    /// and [`Self::undo`]/[`Self::redo`] always return `None`.
    pub fn with_limit(limit: usize) -> Self {
        Self {
            past: VecDeque::new(),
            future: Vec::new(),
            limit,
        }
    }

    /// Record `before` — the state immediately prior to a mutation the
    /// caller is about to apply. Clears the redo side.
    pub fn record(&mut self, before: T) {
        if self.limit == 0 {
            return;
        }
        self.past.push_back(before);
        while self.past.len() > self.limit {
            self.past.pop_front();
        }
        self.future.clear();
    }

    /// Undo one step. `current` is the caller's live state — pushed onto
    /// the redo side so a following [`Self::redo`] can restore it.
    /// Returns the state to restore, or `None` if there's nothing to
    /// undo.
    pub fn undo(&mut self, current: T) -> Option<T> {
        let prev = self.past.pop_back()?;
        self.future.push(current);
        Some(prev)
    }

    /// Redo one step, mirroring [`Self::undo`]: `current` is pushed back
    /// onto the undo side.
    pub fn redo(&mut self, current: T) -> Option<T> {
        let next = self.future.pop()?;
        self.past.push_back(current);
        Some(next)
    }

    /// Whether [`Self::undo`] would return `Some`.
    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    /// Whether [`Self::redo`] would return `Some`.
    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    /// Drop all history — e.g. after loading a fresh document, where
    /// undoing past the load point makes no sense.
    pub fn clear(&mut self) {
        self.past.clear();
        self.future.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_with_empty_history_returns_none() {
        let mut stack: UndoStack<i32> = UndoStack::new();
        assert_eq!(stack.undo(0), None);
        assert!(!stack.can_undo());
    }

    #[test]
    fn redo_with_empty_future_returns_none() {
        let mut stack: UndoStack<i32> = UndoStack::new();
        assert_eq!(stack.redo(0), None);
        assert!(!stack.can_redo());
    }

    #[test]
    fn record_then_undo_restores_prior_state() {
        let mut stack = UndoStack::new();
        stack.record(0); // state before moving to 1
        let restored = stack.undo(1);
        assert_eq!(restored, Some(0));
    }

    #[test]
    fn undo_then_redo_round_trips() {
        let mut stack = UndoStack::new();
        stack.record(0);
        let after_undo = stack.undo(1).unwrap();
        assert_eq!(after_undo, 0);
        let after_redo = stack.redo(after_undo).unwrap();
        assert_eq!(after_redo, 1);
    }

    #[test]
    fn record_clears_redo_history() {
        let mut stack = UndoStack::new();
        stack.record(0);
        stack.undo(1); // future now has [1]
        assert!(stack.can_redo());
        stack.record(5); // a fresh edit invalidates the redo branch
        assert!(!stack.can_redo());
    }

    #[test]
    fn multiple_undo_steps_pop_in_lifo_order() {
        let mut stack = UndoStack::new();
        stack.record(0); // -> 1
        stack.record(1); // -> 2
        stack.record(2); // -> 3
        assert_eq!(stack.undo(3), Some(2));
        assert_eq!(stack.undo(2), Some(1));
        assert_eq!(stack.undo(1), Some(0));
        assert_eq!(stack.undo(0), None);
    }

    #[test]
    fn with_limit_zero_disables_history() {
        let mut stack = UndoStack::with_limit(0);
        stack.record(0);
        assert!(!stack.can_undo());
        assert_eq!(stack.undo(1), None);
    }

    #[test]
    fn with_limit_evicts_oldest_entry() {
        let mut stack = UndoStack::with_limit(2);
        stack.record(0);
        stack.record(1);
        stack.record(2); // 0 should be evicted, only [1, 2] remain
        assert_eq!(stack.undo(3), Some(2));
        assert_eq!(stack.undo(2), Some(1));
        assert_eq!(
            stack.undo(1),
            None,
            "oldest entry (0) should have been evicted"
        );
    }

    #[test]
    fn clear_drops_both_stacks() {
        let mut stack = UndoStack::new();
        stack.record(0);
        stack.undo(1);
        assert!(stack.can_redo());
        stack.clear();
        assert!(!stack.can_undo());
        assert!(!stack.can_redo());
    }
}
