//! `FocusManager` — the single owner of "what widget currently has
//! keyboard focus".
//!
//! # Ownership: one per backend, one writer
//!
//! Each concrete backend (`TuiBackend`, `GtkBackend`, `MacBackend`,
//! `WinBackend`) owns exactly one `FocusManager`, reachable read-only
//! via [`crate::Backend::focus_manager`]. The only code that *mutates*
//! it is the shared Tab/Shift+Tab intercept in
//! [`crate::runtime::preprocess_event`] (via the crate-private
//! `PreprocessBackend::focus_manager_mut`) — every backend's `run.rs`
//! routes through that one function, so "what's focused" can't drift
//! between backends. Apps never construct or mutate a `FocusManager`
//! directly; they read it off the backend, and drive it indirectly by
//! returning tab stops from [`crate::runner::AppLogic::tab_stops`].
//!
//! # Opt-in, not forced
//!
//! An app participates purely by overriding
//! [`crate::runner::AppLogic::tab_stops`] (default: empty). While it
//! returns `[]`, Tab/Shift+Tab pass through to `AppLogic::handle`
//! completely unclaimed — **zero behavior change** for every app
//! written before this feature. Once an app returns a non-empty list
//! from `tab_stops`, the runner claims Tab/Shift+Tab globally: they
//! stop reaching `AppLogic::handle` as a raw `KeyPressed` and arrive
//! instead as [`crate::UiEvent::FocusChanged`]. An app with a widget
//! that treats a literal Tab keystroke as input (a code editor's
//! indent command, say) should leave that widget out of `tab_stops`
//! entirely, or keep `tab_stops` empty while that widget holds
//! focus — see that method's doc for the full contract.
//!
//! # Focus-ring paint convention
//!
//! [`crate::Backend::draw_focus_ring`] paints the visual cue for
//! whichever widget [`Self::focused`] currently names — a themed,
//! border-only stroke (`Theme::accent_fg`, the same colour `Form`
//! already uses for "this control has focus") drawn *after* the app's
//! own `render`, so it overlays without needing per-primitive support.
//! Every runner (`tui::run`, `gtk::run`, `macos::run`, `win::run`) calls
//! it once per frame, right after `AppLogic::render`, whenever
//! `focus_manager().focused()` names a widget this frame's tab stops
//! can still resolve a rect for.

use crate::event::Rect;
use crate::types::WidgetId;

// Focus-ring stroke width is not a constant in this module: it is the
// app-overridable [`crate::Style::focus_ring_width`] token (native
// units — GTK/macOS/Win-GUI pixels), which every `PaintSurface`-backed
// pixel backend's `Backend::draw_focus_ring` reads via
// `Backend::style()` so all three paint the same weight without three
// independent literals to keep in sync. See that field's doc for the
// TUI story (`crate::tui::draw_focus_ring` paints a 1-cell border and
// consumes no `Style` token).

/// Single owner of keyboard focus. See the module doc for
/// the full rationale and the six representations this replaces (or, in
/// three documented cases, deliberately leaves for a later migration).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FocusManager {
    focused: Option<WidgetId>,
    tab_order: Vec<WidgetId>,
}

impl FocusManager {
    /// A manager with nothing focused and an empty tab order.
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild the tab order from this frame's tab stops — typically
    /// [`crate::frame::ScreenLayout::tab_stops`]'s output, already
    /// sorted in reading order.
    ///
    /// If the previously-focused widget's id is still present in the
    /// new order, focus is preserved (matched **by id**, not position —
    /// a widget that moved on screen keeps focus; one that vanished
    /// doesn't hand focus to whatever now sits at its old index).
    /// Otherwise focus is cleared.
    pub fn sync_tab_order(&mut self, stops: &[(WidgetId, Rect)]) {
        self.tab_order = stops.iter().map(|(id, _)| id.clone()).collect();
        if let Some(cur) = &self.focused {
            if !self.tab_order.contains(cur) {
                self.focused = None;
            }
        }
    }

    /// The current tab order, in cycle order.
    pub fn tab_order(&self) -> &[WidgetId] {
        &self.tab_order
    }

    /// The currently focused widget, if any.
    pub fn focused(&self) -> Option<&WidgetId> {
        self.focused.as_ref()
    }

    /// Whether `id` currently has focus.
    pub fn is_focused(&self, id: &WidgetId) -> bool {
        self.focused.as_ref() == Some(id)
    }

    /// Move focus to the next tab stop, wrapping around. Starting from
    /// "nothing focused", lands on the first stop. No-op (returns
    /// `false`) if the tab order is empty. Returns whether focus
    /// actually changed.
    pub fn focus_next(&mut self) -> bool {
        self.step(1)
    }

    /// Move focus to the previous tab stop, wrapping around. Starting
    /// from "nothing focused", lands on the last stop. No-op (returns
    /// `false`) if the tab order is empty. Returns whether focus
    /// actually changed.
    pub fn focus_prev(&mut self) -> bool {
        self.step(-1)
    }

    fn step(&mut self, dir: i32) -> bool {
        if self.tab_order.is_empty() {
            return self.set_focus(None);
        }
        let len = self.tab_order.len() as i32;
        let next_idx = match &self.focused {
            None if dir >= 0 => 0,
            None => len - 1,
            Some(cur) => {
                let idx = self.tab_order.iter().position(|w| w == cur).unwrap_or(0) as i32;
                (idx + dir).rem_euclid(len)
            }
        };
        let next = self.tab_order[next_idx as usize].clone();
        self.set_focus(Some(next))
    }

    /// Set focus explicitly — e.g. click-to-focus. `None` clears focus.
    /// Returns whether it actually changed.
    pub fn set_focus(&mut self, id: Option<WidgetId>) -> bool {
        if self.focused != id {
            self.focused = id;
            true
        } else {
            false
        }
    }

    /// Clear focus. Equivalent to `set_focus(None)`.
    pub fn clear(&mut self) -> bool {
        self.set_focus(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stops(ids: &[&str]) -> Vec<(WidgetId, Rect)> {
        ids.iter()
            .enumerate()
            .map(|(i, id)| {
                (
                    WidgetId::new(*id),
                    Rect::new(0.0, i as f32 * 3.0, 10.0, 3.0),
                )
            })
            .collect()
    }

    #[test]
    fn new_manager_has_no_focus() {
        let fm = FocusManager::new();
        assert_eq!(fm.focused(), None);
        assert!(fm.tab_order().is_empty());
    }

    #[test]
    fn sync_tab_order_populates_order() {
        let mut fm = FocusManager::new();
        fm.sync_tab_order(&stops(&["a", "b", "c"]));
        assert_eq!(
            fm.tab_order(),
            &[WidgetId::new("a"), WidgetId::new("b"), WidgetId::new("c")]
        );
    }

    #[test]
    fn focus_next_from_none_lands_on_first() {
        let mut fm = FocusManager::new();
        fm.sync_tab_order(&stops(&["a", "b", "c"]));
        assert!(fm.focus_next());
        assert_eq!(fm.focused(), Some(&WidgetId::new("a")));
    }

    #[test]
    fn focus_prev_from_none_lands_on_last() {
        let mut fm = FocusManager::new();
        fm.sync_tab_order(&stops(&["a", "b", "c"]));
        assert!(fm.focus_prev());
        assert_eq!(fm.focused(), Some(&WidgetId::new("c")));
    }

    #[test]
    fn focus_next_wraps() {
        let mut fm = FocusManager::new();
        fm.sync_tab_order(&stops(&["a", "b", "c"]));
        fm.focus_next(); // a
        fm.focus_next(); // b
        fm.focus_next(); // c
        assert_eq!(fm.focused(), Some(&WidgetId::new("c")));
        fm.focus_next(); // wraps to a
        assert_eq!(fm.focused(), Some(&WidgetId::new("a")));
    }

    #[test]
    fn focus_prev_wraps() {
        let mut fm = FocusManager::new();
        fm.sync_tab_order(&stops(&["a", "b", "c"]));
        fm.focus_next(); // a
        fm.focus_prev(); // wraps to c
        assert_eq!(fm.focused(), Some(&WidgetId::new("c")));
    }

    #[test]
    fn focus_next_noop_on_empty_order() {
        let mut fm = FocusManager::new();
        assert!(!fm.focus_next());
        assert_eq!(fm.focused(), None);
    }

    #[test]
    fn set_focus_returns_whether_changed() {
        let mut fm = FocusManager::new();
        assert!(fm.set_focus(Some(WidgetId::new("a"))));
        assert!(
            !fm.set_focus(Some(WidgetId::new("a"))),
            "same id is a no-op"
        );
        assert!(fm.set_focus(Some(WidgetId::new("b"))));
        assert!(fm.clear());
        assert!(!fm.clear(), "clearing an already-clear focus is a no-op");
    }

    #[test]
    fn sync_tab_order_preserves_focus_by_id() {
        let mut fm = FocusManager::new();
        fm.sync_tab_order(&stops(&["a", "b", "c"]));
        fm.set_focus(Some(WidgetId::new("b")));
        // Geometry moved (b now at index 0), but the id is still present.
        fm.sync_tab_order(&stops(&["b", "a", "c"]));
        assert_eq!(
            fm.focused(),
            Some(&WidgetId::new("b")),
            "focus follows the id, not the index"
        );
    }

    #[test]
    fn sync_tab_order_clears_focus_when_id_removed() {
        let mut fm = FocusManager::new();
        fm.sync_tab_order(&stops(&["a", "b", "c"]));
        fm.set_focus(Some(WidgetId::new("b")));
        fm.sync_tab_order(&stops(&["a", "c"]));
        assert_eq!(fm.focused(), None);
    }

    #[test]
    fn is_focused_checks_current_id() {
        let mut fm = FocusManager::new();
        fm.sync_tab_order(&stops(&["a", "b"]));
        fm.set_focus(Some(WidgetId::new("a")));
        assert!(fm.is_focused(&WidgetId::new("a")));
        assert!(!fm.is_focused(&WidgetId::new("b")));
    }

    #[test]
    fn single_stop_next_and_prev_stay_put() {
        let mut fm = FocusManager::new();
        fm.sync_tab_order(&stops(&["only"]));
        assert!(fm.focus_next());
        assert_eq!(fm.focused(), Some(&WidgetId::new("only")));
        assert!(
            !fm.focus_next(),
            "focusing the same single stop again is a no-op"
        );
        assert!(!fm.focus_prev());
    }
}
