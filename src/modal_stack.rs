//! Cross-backend modal-overlay tracking.
//!
//! Backends hold a [`ModalStack`] and apps push to it when they open a
//! modal overlay (command palette, file picker, context menu, dialog,
//! tab switcher, …) and pop when it closes. Quadraui's event dispatcher
//! ([`crate::dispatch`]) consults the stack **before** running any
//! base-layer hit test so events that land inside a modal's bounds
//! can't fall through to whatever is behind it.
//!
//! # Why this belongs in quadraui
//!
//! Every backend before quadraui had its own "is a modal open?" check
//! scattered across its event handlers — TUI's `mouse.rs` had a chain
//! of `if engine.picker_open { ... return; }` early-outs, GTK's
//! `mod.rs` had the same logic duplicated per modal kind. The "drag
//! leaks through the palette" family of bugs (#192) is the direct
//! consequence of one backend forgetting to add a new modal to its
//! own list.
//!
//! Centralising the stack here means:
//! - Apps call [`Self::push`] / [`Self::pop`] once per modal open/close.
//! - Adding a new modal kind doesn't require any per-backend change.
//! - The dispatch algorithm is the same on TUI / GTK / Win-GUI / macOS.
//!
//! # What this does **not** do
//!
//! - **Focus**: focus tracking lives in [`crate::Backend`] (v1.x) — the
//!   modal stack is about hit-test precedence, not keyboard focus.
//! - **Painting**: the stack has no opinions on draw order. Apps still
//!   paint modals last (highest z); the stack is queried only when
//!   *events* arrive.
//! - **Inner hit refinement**: the stack resolves modal-vs-base
//!   arbitration only. Once a hit lands inside a modal, the app still
//!   asks the primitive itself (e.g. [`crate::PaletteLayout::hit_test`])
//!   for the semantic hit inside it.

use crate::event::{Point, Rect};
use crate::types::WidgetId;

/// One entry in the modal stack: a widget id + its outer bounds in
/// backend-native coordinates.
///
/// `Rect` rather than a full [`crate::PaletteLayout`] (or tree/dialog
/// layout) because the stack only arbitrates "modal vs base" — inner
/// hit refinement stays a concern of the individual primitive's
/// layout, which the app queries after the stack identifies the
/// topmost modal under the cursor.
#[derive(Debug, Clone, PartialEq)]
pub struct ModalEntry {
    pub id: WidgetId,
    pub bounds: Rect,
}

/// Top-of-stack-is-topmost ordered list of open modal overlays.
///
/// Backends hold one instance (typically on their concrete backend
/// struct; the trait exposes
/// [`crate::Backend::modal_stack_mut`][crate::Backend::modal_stack_mut]).
/// The app mutates it through modal-open and modal-close code paths
/// (typically in the engine's picker/dialog-open state transitions);
/// quadraui's dispatcher reads it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModalStack {
    entries: Vec<ModalEntry>,
}

impl ModalStack {
    /// Empty stack — no modals open.
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a modal onto the stack. The new entry becomes the topmost
    /// (most-recently-opened) modal.
    ///
    /// If `id` is already present (misuse — apps should pop first),
    /// the existing entry is removed before the new one is pushed so
    /// the stack never contains duplicates.
    pub fn push(&mut self, id: WidgetId, bounds: Rect) {
        self.entries.retain(|e| e.id != id);
        self.entries.push(ModalEntry { id, bounds });
    }

    /// Remove the modal with this id, if present. Returns true on
    /// successful removal. Called by the app on palette/dialog close.
    pub fn pop(&mut self, id: &WidgetId) -> bool {
        let len_before = self.entries.len();
        self.entries.retain(|e| e.id != *id);
        self.entries.len() < len_before
    }

    /// Pop the topmost modal regardless of id. Used by "click outside
    /// any modal dismisses the topmost one" flows.
    pub fn pop_top(&mut self) -> Option<ModalEntry> {
        self.entries.pop()
    }

    /// Peek at the topmost modal without mutating.
    pub fn top(&self) -> Option<&ModalEntry> {
        self.entries.last()
    }

    /// `true` when no modals are open.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of modals open (stacked).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Iterate entries top-down (topmost first). Used by the
    /// dispatcher's hit walk.
    pub fn iter_top_down(&self) -> impl Iterator<Item = &ModalEntry> {
        self.entries.iter().rev()
    }

    /// Return the id of the topmost modal whose bounds contain `point`,
    /// or `None` if no modal contains it (click landed outside every
    /// open modal).
    pub fn hit_test(&self, point: Point) -> Option<&WidgetId> {
        for entry in self.iter_top_down() {
            if rect_contains(&entry.bounds, point) {
                return Some(&entry.id);
            }
        }
        None
    }
}

fn rect_contains(rect: &Rect, point: Point) -> bool {
    point.x >= rect.x
        && point.x < rect.x + rect.width
        && point.y >= rect.y
        && point.y < rect.y + rect.height
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> WidgetId {
        WidgetId::new(s)
    }

    fn pt(x: f32, y: f32) -> Point {
        Point { x, y }
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn empty_stack_reports_empty() {
        let s = ModalStack::new();
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
        assert!(s.top().is_none());
        assert!(s.hit_test(pt(0.0, 0.0)).is_none());
    }

    #[test]
    fn push_adds_to_top() {
        let mut s = ModalStack::new();
        s.push(id("palette"), rect(10.0, 10.0, 100.0, 100.0));
        assert_eq!(s.len(), 1);
        assert_eq!(s.top().unwrap().id, id("palette"));
    }

    #[test]
    fn hit_test_walks_top_down() {
        // Two overlapping modals — the second pushed should win for
        // points inside both. This is the classic "dialog on top of
        // palette" case.
        let mut s = ModalStack::new();
        s.push(id("palette"), rect(0.0, 0.0, 100.0, 100.0));
        s.push(id("dialog"), rect(20.0, 20.0, 60.0, 60.0));

        // Point inside both → topmost (dialog) wins.
        assert_eq!(s.hit_test(pt(30.0, 30.0)), Some(&id("dialog")));
        // Point inside palette only → palette.
        assert_eq!(s.hit_test(pt(5.0, 5.0)), Some(&id("palette")));
        // Outside both → None.
        assert_eq!(s.hit_test(pt(200.0, 200.0)), None);
    }

    #[test]
    fn pop_by_id_removes_even_when_not_top() {
        // Stack tolerates out-of-order closes. The "normal" case is
        // LIFO (close the top one); non-LIFO happens when code paths
        // race (e.g. an async result closes a picker that's no longer
        // the topmost modal).
        let mut s = ModalStack::new();
        s.push(id("a"), rect(0.0, 0.0, 10.0, 10.0));
        s.push(id("b"), rect(0.0, 0.0, 10.0, 10.0));
        s.push(id("c"), rect(0.0, 0.0, 10.0, 10.0));
        assert!(s.pop(&id("b")));
        assert_eq!(s.len(), 2);
        // Remaining order: a, c (c still top).
        assert_eq!(s.top().unwrap().id, id("c"));
        // Popping something that isn't there is a no-op.
        assert!(!s.pop(&id("missing")));
    }

    #[test]
    fn pop_top_returns_the_popped_entry() {
        let mut s = ModalStack::new();
        s.push(id("x"), rect(1.0, 2.0, 3.0, 4.0));
        let popped = s.pop_top().unwrap();
        assert_eq!(popped.id, id("x"));
        assert!(s.is_empty());
        assert!(s.pop_top().is_none());
    }

    #[test]
    fn push_with_existing_id_moves_to_top_without_duplication() {
        let mut s = ModalStack::new();
        s.push(id("a"), rect(0.0, 0.0, 10.0, 10.0));
        s.push(id("b"), rect(0.0, 0.0, 10.0, 10.0));
        // Re-push "a" with new bounds — should bump to top, not duplicate.
        s.push(id("a"), rect(5.0, 5.0, 20.0, 20.0));
        assert_eq!(s.len(), 2);
        assert_eq!(s.top().unwrap().id, id("a"));
        assert_eq!(s.top().unwrap().bounds, rect(5.0, 5.0, 20.0, 20.0));
    }

    #[test]
    fn hit_test_edges_are_exclusive_on_right_and_bottom() {
        // `x < rect.x + width`, not `<=` — matches half-open-interval
        // convention used everywhere else in quadraui (tab bar
        // segments, status bar hit regions).
        let mut s = ModalStack::new();
        s.push(id("m"), rect(0.0, 0.0, 10.0, 10.0));
        assert_eq!(s.hit_test(pt(0.0, 0.0)), Some(&id("m"))); // inclusive
        assert!(s.hit_test(pt(10.0, 5.0)).is_none()); // exclusive right edge
        assert!(s.hit_test(pt(5.0, 10.0)).is_none()); // exclusive bottom edge
    }
}
