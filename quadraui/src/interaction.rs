//! `InteractionState` — a single hover/pressed store keyed by
//! [`WidgetId`], meant to replace the positional `hovered_id: Option<&
//! WidgetId>` / `pressed_id: Option<&WidgetId>` pairs that several
//! primitives' rasterisers take today (issue #819, child of #786's
//! "one convention" series).
//!
//! ## Why this exists
//!
//! Before this type, every primitive that wanted hover/pressed
//! highlighting grew its own bespoke tracker:
//! [`crate::compose::ToolbarHoverTracker`] for [`crate::Toolbar`],
//! [`crate::compose::StatusBarInteraction`] for [`crate::StatusBar`],
//! and a handful of examples (`examples/common/toolbar_app.rs`) that
//! just inlined two `Option<WidgetId>` fields by hand. Each one
//! reimplements the same three lines of "did the hovered/pressed id
//! change since last time" bookkeeping, and each one is scoped to a
//! single primitive even though hover/pressed state is a generic,
//! per-frame concept that applies to any widget with an id.
//!
//! `InteractionState` is the one generic replacement: a single
//! instance can track the hovered/pressed widget across an entire
//! frame's worth of primitives — a toolbar, a status bar, a sidebar
//! panel — because it's keyed by [`WidgetId`], not tied to one
//! primitive's layout type.
//!
//! ## Usage
//!
//! ```
//! use quadraui::{mouse_moved, InteractionState, WidgetId};
//!
//! let mut interaction = InteractionState::new();
//!
//! // On every event, feed it through with a hit-test closure that
//! // knows how to resolve a screen position to a widget id (built
//! // from whatever layout the app already computed to paint).
//! let event = mouse_moved(5.0, 0.0, Default::default());
//! let hit_test = |x: f32, _y: f32| {
//!     if x < 10.0 {
//!         Some(WidgetId::new("demo:button"))
//!     } else {
//!         None
//!     }
//! };
//! if interaction.handle_mouse(&event, hit_test) {
//!     // redraw
//! }
//!
//! // At paint time, pass the two accessors straight into the existing
//! // `Backend::draw_*` positional slots — no signature change needed
//! // on the rasteriser side to start benefiting from a single shared
//! // state store instead of N bespoke ones.
//! // backend.draw_toolbar(rect, &bar, interaction.hovered(), interaction.pressed());
//! assert_eq!(interaction.hovered(), Some(&WidgetId::new("demo:button")));
//! ```
//!
//! ## What this does NOT do (yet)
//!
//! It does not change any `Backend` trait method's signature or any
//! primitive's `paint`/`draw_*` parameter list — `StatusBar`,
//! `Toolbar`, `SidebarPanel`, `ActivityBar`, `TabBar`, `DataTable`, and
//! `Chart` all still take their existing positional hover/pressed
//! arguments (`Option<&WidgetId>`, `Option<usize>`, or
//! `Option<(usize, usize)>` depending on the primitive). Those are
//! `pub` `Backend` trait methods with real call sites in both
//! downstream consumers (`coord-tui`, `vimcode`); replacing them is a
//! breaking change that needs the two-PR deprecate-then-remove
//! sequence `CLAUDE.md` rule 3 requires, plus migration issues filed
//! in both consumer repos. That is out of scope for the PR that adds
//! this type — see the issue tracker for the follow-up.
//!
//! What *is* in scope, and done: `InteractionState` is a drop-in
//! replacement for the ad hoc `Option<WidgetId>` fields / bespoke
//! trackers an app already owns, feeding the exact same positional
//! slots. `examples/common/toolbar_app.rs` demonstrates the pattern.

use crate::event::{MouseButton, UiEvent};
use crate::types::WidgetId;

/// Tracks which widget is hovered and which is pressed, keyed by
/// [`WidgetId`] rather than by index or raw coordinates.
///
/// Cheap to keep one per frame (or one per app, reused every frame) —
/// two `Option<WidgetId>` slots plus whatever hit-test the caller
/// supplies to [`Self::handle_mouse`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InteractionState {
    hovered: Option<WidgetId>,
    pressed: Option<WidgetId>,
}

impl InteractionState {
    /// Construct an empty state — nothing hovered, nothing pressed.
    pub fn new() -> Self {
        Self::default()
    }

    /// The currently hovered widget, if any.
    pub fn hovered(&self) -> Option<&WidgetId> {
        self.hovered.as_ref()
    }

    /// The currently pressed widget, if any.
    pub fn pressed(&self) -> Option<&WidgetId> {
        self.pressed.as_ref()
    }

    /// Whether `id` is the currently hovered widget.
    pub fn is_hovered(&self, id: &WidgetId) -> bool {
        self.hovered.as_ref() == Some(id)
    }

    /// Whether `id` is the currently pressed widget.
    pub fn is_pressed(&self, id: &WidgetId) -> bool {
        self.pressed.as_ref() == Some(id)
    }

    /// Set the hovered widget directly. Returns `true` if it changed
    /// (caller should redraw).
    pub fn set_hovered(&mut self, id: Option<WidgetId>) -> bool {
        if self.hovered != id {
            self.hovered = id;
            true
        } else {
            false
        }
    }

    /// Set the pressed widget directly. Returns `true` if it changed
    /// (caller should redraw).
    pub fn set_pressed(&mut self, id: Option<WidgetId>) -> bool {
        if self.pressed != id {
            self.pressed = id;
            true
        } else {
            false
        }
    }

    /// Clear both hovered and pressed state — e.g. on `MouseLeft`, on
    /// window blur, or when the widget tree they refer to is torn
    /// down. Returns `true` if anything changed.
    pub fn clear(&mut self) -> bool {
        let changed = self.hovered.is_some() || self.pressed.is_some();
        self.hovered = None;
        self.pressed = None;
        changed
    }

    /// Update hovered/pressed state from a raw [`UiEvent`].
    ///
    /// `hit_test` resolves a screen position to the widget under it —
    /// callers supply this from whatever layout they already computed
    /// to paint (a `ToolbarLayout::hit_test`, a `StatusBarLayout`
    /// lookup, etc.), since the event itself doesn't always carry a
    /// resolved id: [`UiEvent::MouseMoved`] never does (hover
    /// resolution is layout-dependent and happens on every pixel of
    /// movement, so nothing upstream can afford to precompute it), but
    /// [`UiEvent::MouseDown`] / [`UiEvent::MouseUp`] carry a `widget`
    /// field that's used when already resolved, falling back to
    /// `hit_test` when it's `None`.
    ///
    /// Returns `true` if hovered or pressed state changed (caller
    /// should redraw). Left-button-only for press/release — matches
    /// every existing bespoke tracker this type replaces
    /// ([`crate::compose::ToolbarHoverTracker`],
    /// [`crate::compose::StatusBarInteraction`]).
    pub fn handle_mouse(
        &mut self,
        event: &UiEvent,
        hit_test: impl FnOnce(f32, f32) -> Option<WidgetId>,
    ) -> bool {
        match event {
            UiEvent::MouseMoved { position, .. } => {
                self.set_hovered(hit_test(position.x, position.y))
            }
            UiEvent::MouseDown {
                widget,
                position,
                button: MouseButton::Left,
                ..
            } => {
                let id = widget.clone().or_else(|| hit_test(position.x, position.y));
                self.set_pressed(id)
            }
            UiEvent::MouseUp {
                button: MouseButton::Left,
                ..
            } => self.set_pressed(None),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Point;

    fn mouse_moved(x: f32, y: f32) -> UiEvent {
        UiEvent::MouseMoved {
            position: Point::new(x, y),
            buttons: Default::default(),
        }
    }

    fn mouse_down(widget: Option<WidgetId>, x: f32, y: f32) -> UiEvent {
        UiEvent::MouseDown {
            widget,
            button: MouseButton::Left,
            position: Point::new(x, y),
            modifiers: Default::default(),
        }
    }

    fn mouse_up(x: f32, y: f32) -> UiEvent {
        UiEvent::MouseUp {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(x, y),
        }
    }

    #[test]
    fn new_state_has_nothing_hovered_or_pressed() {
        let state = InteractionState::new();
        assert_eq!(state.hovered(), None);
        assert_eq!(state.pressed(), None);
    }

    #[test]
    fn set_hovered_reports_change() {
        let mut state = InteractionState::new();
        assert!(state.set_hovered(Some(WidgetId::new("a"))));
        assert_eq!(state.hovered(), Some(&WidgetId::new("a")));
        // Setting the same id again is not a change.
        assert!(!state.set_hovered(Some(WidgetId::new("a"))));
        // Moving to a different id is.
        assert!(state.set_hovered(Some(WidgetId::new("b"))));
        assert_eq!(state.hovered(), Some(&WidgetId::new("b")));
    }

    #[test]
    fn is_hovered_and_is_pressed_key_by_widget_id() {
        let mut state = InteractionState::new();
        state.set_hovered(Some(WidgetId::new("btn-a")));
        state.set_pressed(Some(WidgetId::new("btn-b")));
        assert!(state.is_hovered(&WidgetId::new("btn-a")));
        assert!(!state.is_hovered(&WidgetId::new("btn-b")));
        assert!(state.is_pressed(&WidgetId::new("btn-b")));
        assert!(!state.is_pressed(&WidgetId::new("btn-a")));
    }

    #[test]
    fn clear_resets_both_and_reports_whether_anything_changed() {
        let mut state = InteractionState::new();
        assert!(!state.clear(), "clearing an empty state is a no-op");
        state.set_hovered(Some(WidgetId::new("a")));
        state.set_pressed(Some(WidgetId::new("a")));
        assert!(state.clear());
        assert_eq!(state.hovered(), None);
        assert_eq!(state.pressed(), None);
    }

    #[test]
    fn handle_mouse_moved_resolves_hover_via_hit_test() {
        let mut state = InteractionState::new();
        let changed = state.handle_mouse(&mouse_moved(5.0, 0.0), |x, _y| {
            if x < 10.0 {
                Some(WidgetId::new("hit"))
            } else {
                None
            }
        });
        assert!(changed);
        assert_eq!(state.hovered(), Some(&WidgetId::new("hit")));

        // Moving off the hit region clears hover.
        let changed = state.handle_mouse(&mouse_moved(50.0, 0.0), |x, _y| {
            if x < 10.0 {
                Some(WidgetId::new("hit"))
            } else {
                None
            }
        });
        assert!(changed);
        assert_eq!(state.hovered(), None);
    }

    #[test]
    fn handle_mouse_down_prefers_the_events_own_resolved_widget() {
        let mut state = InteractionState::new();
        // Event already carries a resolved widget — hit_test must not
        // be consulted (it would panic-on-call via `unreachable!` if
        // it were, since `FnOnce` only tolerates a single call, but we
        // assert the *value* instead of relying on that).
        let changed = state.handle_mouse(
            &mouse_down(Some(WidgetId::new("resolved")), 0.0, 0.0),
            |_x, _y| Some(WidgetId::new("from-hit-test")),
        );
        assert!(changed);
        assert_eq!(state.pressed(), Some(&WidgetId::new("resolved")));
    }

    #[test]
    fn handle_mouse_down_falls_back_to_hit_test_when_unresolved() {
        let mut state = InteractionState::new();
        let changed = state.handle_mouse(&mouse_down(None, 3.0, 0.0), |x, _y| {
            if x < 10.0 {
                Some(WidgetId::new("fallback"))
            } else {
                None
            }
        });
        assert!(changed);
        assert_eq!(state.pressed(), Some(&WidgetId::new("fallback")));
    }

    #[test]
    fn handle_mouse_up_clears_pressed() {
        let mut state = InteractionState::new();
        state.set_pressed(Some(WidgetId::new("held")));
        let changed = state.handle_mouse(&mouse_up(0.0, 0.0), |_, _| None);
        assert!(changed);
        assert_eq!(state.pressed(), None);
    }

    #[test]
    fn handle_mouse_ignores_unrelated_events() {
        let mut state = InteractionState::new();
        let changed = state.handle_mouse(
            &UiEvent::KeyPressed {
                key: crate::event::Key::Char('a'),
                modifiers: Default::default(),
                repeat: false,
            },
            |_, _| Some(WidgetId::new("irrelevant")),
        );
        assert!(!changed);
        assert_eq!(state.hovered(), None);
    }
}
