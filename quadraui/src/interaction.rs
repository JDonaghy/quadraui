//! `InteractionState` — a single hover/pressed store keyed by
//! [`WidgetId`], replacing the positional `hovered_id: Option<&
//! WidgetId>` / `pressed_id: Option<&WidgetId>` pairs that the
//! `WidgetId`-keyed primitives' rasterisers used to take (issue #819,
//! child of #786's "one convention" series).
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
//! `InteractionState` is the one generic store: a single instance can
//! track the hovered/pressed widget across an entire frame's worth of
//! primitives — a toolbar, a status bar, a sidebar panel — because
//! it's keyed by [`WidgetId`], not tied to one primitive's layout
//! type.
//!
//! ## What this PR changed on the `Backend` trait
//!
//! Every `Backend` method that took a `WidgetId`-keyed hover/pressed
//! *pair* now has an `*_interactive` twin that takes one
//! `&InteractionState` instead, and **the twin is the method each
//! backend actually implements**:
//!
//! | positional (now deprecated) | implemented today |
//! |---|---|
//! | `draw_toolbar(rect, bar, hovered_id, pressed_id)` | [`crate::Backend::draw_toolbar_interactive`] |
//! | `draw_status_bar(rect, bar, hovered_id, pressed_id)` | [`crate::Backend::draw_status_bar_interactive`] |
//! | `draw_sidebar_panel(rect, panel, hovered_toolbar_id, pressed_toolbar_id)` | [`crate::Backend::draw_sidebar_panel_interactive`] |
//!
//! The three positional names survive only as `#[deprecated]` trait
//! *defaults* that rebuild an `InteractionState` via
//! [`InteractionState::from_parts`] and forward — the deprecate-first
//! half of `CLAUDE.md` rule 3, because `coord-tui` and `vimcode` both
//! call them today. Every in-repo call site is already migrated, so
//! the shims exist purely for those two consumers.
//!
//! ## What is deliberately *not* migrated
//!
//! Four `Backend` methods still take a positional hover argument, and
//! none of them can move into an `InteractionState` as it stands,
//! because this type is keyed by [`WidgetId`] and their hover state is
//! keyed by *position within a collection*:
//!
//! - `draw_activity_bar(.., hovered_idx: Option<usize>)`
//! - `draw_tab_bar(.., hovered_close_tab: Option<usize>)`
//! - `draw_data_table(.., hovered_idx: Option<usize>)`
//! - `draw_chart(.., hovered_point: Option<(usize, usize)>)`
//!
//! Giving those a `WidgetId`-keyed shape means first giving tabs,
//! activity items, table rows and chart points stable ids — a
//! primitive-model change, not a signature change, and out of scope
//! here. **Follow-up work item:** "#819 part 2 — give ActivityBar /
//! TabBar / DataTable / Chart rows stable `WidgetId`s so their hover
//! state can move into `InteractionState`". Until that lands, those
//! four keep their index arguments, unchanged and undeprecated.
//!
//! [`crate::compose::ToolbarHoverTracker`] and
//! [`crate::compose::StatusBarInteraction`] are likewise **untouched**
//! by this change. `InteractionState` is designed to eventually
//! subsume both (it does strictly more: pressed as well as hover, and
//! across primitives rather than one), but neither is deprecated or
//! rewired here — they remain the live implementation for their
//! existing call sites in `examples/common/sidebar_panel_app.rs`,
//! `examples/common/multi_tree.rs`, `examples/common/full_chrome_demo.rs`,
//! and in both downstream consumers. Retiring them is its own
//! rule-3 sequence.
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
//! // At paint time, hand the whole state to the rasteriser — one
//! // argument, not two positional slots:
//! // backend.draw_toolbar_interactive(rect, &bar, &interaction);
//! assert_eq!(interaction.hovered(), Some(&WidgetId::new("demo:button")));
//! ```
//!
//! `examples/common/toolbar_app.rs` demonstrates the whole loop
//! end-to-end against a real backend, and
//! `tests/tui_example_driver.rs`'s
//! `toolbar_hover_paints_hover_background_at_the_cursor` asserts the
//! *painted* consequence.

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

    /// Build a state directly from an owned hovered / pressed pair.
    ///
    /// This is the adapter the deprecated positional `Backend::draw_*`
    /// shims use to forward `(hovered_id, pressed_id)` into their
    /// `*_interactive` twins (issue #819), and it is equally useful for
    /// a host mid-migration that still keeps the two ids in separate
    /// fields.
    pub fn from_parts(hovered: Option<WidgetId>, pressed: Option<WidgetId>) -> Self {
        Self { hovered, pressed }
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
    /// Exactly three event shapes do anything; everything else returns
    /// `false` without touching state.
    ///
    /// | event | effect | uses `hit_test`? |
    /// |---|---|---|
    /// | [`UiEvent::MouseMoved`] | `hovered = hit_test(pos)` | **always** — `MouseMoved` never carries a resolved `widget` |
    /// | [`UiEvent::MouseDown`] (left only) | `pressed = event.widget, else hit_test(pos)` | only when `widget` is `None` |
    /// | [`UiEvent::MouseUp`] (left only) | `pressed = None` | **never** — release always clears, wherever it lands |
    ///
    /// `hit_test` resolves a screen position to the widget under it;
    /// callers supply it from whatever layout they already computed to
    /// paint (a `ToolbarLayout::hit_test`, a `StatusBarLayout` lookup,
    /// etc.). Note the asymmetry in the table: `MouseUp` ignores both
    /// its own `widget` field and `hit_test` entirely, because
    /// "released" is a global state, not a per-widget one — deciding
    /// whether the release *counts as a click* is the caller's job
    /// (compare the id this returned before the call against the id
    /// under the release position, as `examples/common/toolbar_app.rs`
    /// does).
    ///
    /// Returns `true` if hovered or pressed state changed (caller
    /// should redraw).
    ///
    /// # Two deliberate behaviours worth knowing
    ///
    /// 1. **Left button only** for press/release — matches every
    ///    existing bespoke tracker
    ///    ([`crate::compose::ToolbarHoverTracker`],
    ///    [`crate::compose::StatusBarInteraction`]). A right- or
    ///    middle-button `MouseDown` does **not** set pressed state, so
    ///    a right-click never paints a pressed highlight. Covered by
    ///    `handle_mouse_ignores_non_left_mouse_down`.
    /// 2. **A left `MouseDown` that misses everything clears pressed
    ///    state** rather than leaving a stale id behind — `hit_test`
    ///    returning `None` is `set_pressed(None)`. This is what stops a
    ///    pressed highlight sticking after the user presses a button,
    ///    drags off it, and presses again on empty space. Covered by
    ///    `handle_mouse_down_on_empty_space_clears_stale_pressed`.
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

    /// A right- (or middle-) button press must not paint a pressed
    /// highlight. Left-only is the documented contract, and it differs
    /// from the hand-rolled `matches!(UiEvent::MouseDown { .. })` arms
    /// this type replaces, which fired for any button.
    #[test]
    fn handle_mouse_ignores_non_left_mouse_down() {
        let mut state = InteractionState::new();
        for button in [MouseButton::Right, MouseButton::Middle] {
            let event = UiEvent::MouseDown {
                widget: Some(WidgetId::new("btn")),
                button,
                position: Point::new(1.0, 0.0),
                modifiers: Default::default(),
            };
            let changed = state.handle_mouse(&event, |_, _| Some(WidgetId::new("btn")));
            assert!(
                !changed,
                "{button:?} MouseDown must not change pressed state"
            );
            assert_eq!(state.pressed(), None, "{button:?} must not press anything");
        }
    }

    /// A left press that lands on empty space clears whatever was
    /// pressed before, instead of leaving a stale highlight lit.
    #[test]
    fn handle_mouse_down_on_empty_space_clears_stale_pressed() {
        let mut state = InteractionState::new();
        state.set_pressed(Some(WidgetId::new("stale")));
        // Nothing under the cursor, and the event carries no resolved
        // widget either.
        let changed = state.handle_mouse(&mouse_down(None, 999.0, 999.0), |_, _| None);
        assert!(changed, "clearing a stale pressed id is a change");
        assert_eq!(state.pressed(), None);
    }

    #[test]
    fn from_parts_round_trips_the_positional_pair() {
        let state =
            InteractionState::from_parts(Some(WidgetId::new("hov")), Some(WidgetId::new("prs")));
        assert_eq!(state.hovered(), Some(&WidgetId::new("hov")));
        assert_eq!(state.pressed(), Some(&WidgetId::new("prs")));
        assert_eq!(
            InteractionState::from_parts(None, None),
            InteractionState::new()
        );
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
