//! `InteractionState` — a single hover/pressed store keyed by
//! [`WidgetId`] and the widget's own sub-part.
//!
//! ## Why this exists
//!
//! Before this type, every primitive that wanted hover/pressed
//! highlighting grew its own bespoke tracker:
//! [`crate::compose::ToolbarHoverTracker`] for [`crate::Toolbar`],
//! [`crate::compose::StatusBarInteraction`] for [`crate::StatusBar`],
//! and a handful of examples that inlined two `Option<WidgetId>`
//! fields by hand. Each reimplements the same bookkeeping, and each
//! is scoped to a single primitive even though hover/pressed state
//! is a generic, per-frame concept that applies to any widget with
//! an id.
//!
//! `InteractionState` is the one generic store: a single instance can
//! track the hovered/pressed widget across an entire frame's worth of
//! primitives — a toolbar, a status bar, a sidebar panel — because
//! it's keyed by [`WidgetId`], not tied to one primitive's layout
//! type.
//!
//! ## The `Backend` trait
//!
//! No `Backend` paint method takes an `InteractionState`. A widget painted
//! through the widget contract ([`crate::DrawsPrimitives::draw_dyn`])
//! reads hover and press from the [`crate::PaintEnv`] it is handed, which
//! carries either the caller's own `InteractionState` (passed as the
//! [`crate::PaintState`] argument) or, when the caller passes `&()`, the
//! ambient pointer state this module tracks.
//!
//! ## Methods that remain positional
//!
//! Several `Backend` methods take a positional hover argument, keyed
//! by position within a collection rather than [`WidgetId`]:
//!
//! - `primitives::tab_bar::native_surface_paint::TabBarPaintState`'s
//!   `hovered_close_tab`/`hovered_tab`/`pressed_tab: Option<usize>` —
//!   `TabItem` carries no [`WidgetId`] of its own, only `TabBar::id`
//!   (one per *bar*), which can't name which tab is hovered or pressed
//! - a data table's paint state (`&DataTablePaintState`, carrying a
//!   positional `hovered_idx: Option<usize>`)
//! - [`crate::Chart::hovered_point`] (`Option<(usize, usize)>`, a
//!   descriptor field the app owns)
//! - A list's or tree's paint state (`&ListPaintState` /
//!   `&TreePaintState`, each carrying a positional
//!   `hovered_idx: Option<usize>` / `pressed_idx: Option<usize>` plus an
//!   optional pixel-precise `scroll_px`)
//!
//! These would need stable ids for their items before moving into
//! `InteractionState`. `ListView`/`TreeView` rows join this group rather
//! than taking an `InteractionState` the way a toolbar button's
//! hover does: a `ListItem`/`TreeRow` carries no [`WidgetId`] of its own, and
//! minting one per row index on the fly would permanently intern a new
//! string per distinct (list id, row index) pair ever shown — an
//! unbounded leak for a growing log pane. The row-family widgets report
//! the hovered row as a sub-part of their own id instead.
//!
//! ## Ownership
//!
//! Two owners coexist. The runner feeds every event through
//! [`track_pointer`] (from [`crate::runtime::preprocess_event`]), which
//! keeps one ambient, per-thread `InteractionState` resolved against the
//! widgets [`crate::DrawsPrimitives::draw_dyn`] painted in the frame on
//! screen; [`ambient_interaction`] reads it and
//! [`reset_ambient_interaction`] clears it. A consumer that wants
//! different hover (a test pinning a state, a host suppressing hover)
//! owns its own instance and passes it to `draw_dyn` explicitly, which
//! takes precedence over the ambient one for that call.
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
//! // argument, not two positional slots.
//! assert_eq!(interaction.hovered(), Some(&WidgetId::new("demo:button")));
//! ```
//!
//! `examples/common/toolbar_app.rs` demonstrates the whole loop
//! end-to-end against a real backend.

use std::cell::Cell;

use crate::event::{MouseButton, Point, Rect, UiEvent};
use crate::focus::FocusAddress;
use crate::interactive::SubTarget;
use crate::types::WidgetId;

/// Tracks which widget is hovered and which is pressed, keyed by
/// [`FocusAddress`] — a [`WidgetId`] plus, when the widget names one, the
/// sub-part ([`crate::interactive::SubTarget`]) under the pointer —
/// rather than by index or raw coordinates.
///
/// Cheap to keep one per frame (or one per app, reused every frame) —
/// two `Option<FocusAddress>` slots plus whatever hit-test the caller
/// supplies to [`Self::handle_mouse`] / [`Self::handle_mouse_at`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InteractionState {
    hovered: Option<FocusAddress>,
    pressed: Option<FocusAddress>,
}

impl InteractionState {
    /// Construct an empty state — nothing hovered, nothing pressed.
    pub const fn new() -> Self {
        Self {
            hovered: None,
            pressed: None,
        }
    }

    /// Build a state directly from a hovered / pressed widget pair, with
    /// no sub-part on either side.
    pub fn from_parts(hovered: Option<WidgetId>, pressed: Option<WidgetId>) -> Self {
        Self {
            hovered: hovered.map(|id| (id, None)),
            pressed: pressed.map(|id| (id, None)),
        }
    }

    /// Build a state directly from a hovered / pressed address pair.
    pub fn from_targets(hovered: Option<FocusAddress>, pressed: Option<FocusAddress>) -> Self {
        Self { hovered, pressed }
    }

    /// The currently hovered widget, if any.
    pub fn hovered(&self) -> Option<&WidgetId> {
        self.hovered.as_ref().map(|(id, _)| id)
    }

    /// The currently pressed widget, if any.
    pub fn pressed(&self) -> Option<&WidgetId> {
        self.pressed.as_ref().map(|(id, _)| id)
    }

    /// The currently hovered address — widget plus sub-part — if any.
    pub fn hovered_target(&self) -> Option<FocusAddress> {
        self.hovered
    }

    /// The currently pressed address — widget plus sub-part — if any.
    pub fn pressed_target(&self) -> Option<FocusAddress> {
        self.pressed
    }

    /// Whether `id` is the currently hovered widget, on any sub-part.
    pub fn is_hovered(&self, id: &WidgetId) -> bool {
        self.hovered() == Some(id)
    }

    /// Whether `id` is the currently pressed widget, on any sub-part.
    pub fn is_pressed(&self, id: &WidgetId) -> bool {
        self.pressed() == Some(id)
    }

    /// Set the hovered widget directly, with no sub-part. Returns `true`
    /// if it changed (caller should redraw).
    pub fn set_hovered(&mut self, id: Option<WidgetId>) -> bool {
        self.set_hovered_target(id.map(|id| (id, None)))
    }

    /// Set the hovered address directly. Returns `true` if it changed
    /// (caller should redraw).
    pub fn set_hovered_target(&mut self, target: Option<FocusAddress>) -> bool {
        if self.hovered != target {
            self.hovered = target;
            true
        } else {
            false
        }
    }

    /// Set the pressed widget directly, with no sub-part. Returns `true`
    /// if it changed (caller should redraw).
    pub fn set_pressed(&mut self, id: Option<WidgetId>) -> bool {
        self.set_pressed_target(id.map(|id| (id, None)))
    }

    /// Set the pressed address directly. Returns `true` if it changed
    /// (caller should redraw).
    pub fn set_pressed_target(&mut self, target: Option<FocusAddress>) -> bool {
        if self.pressed != target {
            self.pressed = target;
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
    /// | [`UiEvent::MouseDown`] (left only) | `pressed = event.widget, else hit_test(pos)` | only decides the result when `widget` is `None` |
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
    /// should redraw). Every address this sets carries no sub-part; see
    /// [`Self::handle_mouse_at`] for the sub-part-aware form.
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
        self.handle_mouse_at(event, |x, y| hit_test(x, y).map(|id| (id, None)))
    }

    /// [`Self::handle_mouse`], with a hit test that resolves a position
    /// to a full [`FocusAddress`] so hover and press track the sub-part
    /// under the pointer, not only the widget.
    ///
    /// A left `MouseDown` whose event carries a resolved `widget` presses
    /// that widget: on the sub-part `hit_test` found when it agrees on the
    /// widget, on no sub-part otherwise. With no resolved `widget`, the
    /// hit-test result is pressed as-is.
    pub fn handle_mouse_at(
        &mut self,
        event: &UiEvent,
        hit_test: impl FnOnce(f32, f32) -> Option<FocusAddress>,
    ) -> bool {
        match event {
            UiEvent::MouseMoved { position, .. } => {
                self.set_hovered_target(hit_test(position.x, position.y))
            }
            UiEvent::MouseDown {
                widget,
                position,
                button: MouseButton::Left,
                ..
            } => self.set_pressed_target(press_target(*widget, hit_test(position.x, position.y))),
            UiEvent::MouseUp {
                button: MouseButton::Left,
                ..
            } => self.set_pressed(None),
            _ => false,
        }
    }

    /// Update hovered/pressed state from `event`, hit-testing against the
    /// widgets [`crate::DrawsPrimitives::draw_dyn`] painted in the frame
    /// currently on screen ([`crate::primitive::painted_hit`]).
    ///
    /// Follows [`Self::handle_mouse_at`], plus these pointer rules:
    ///
    /// - a left `MouseDown` also moves hover to the address under the
    ///   press (the pointer is there, whether or not a `MouseMoved`
    ///   preceded it);
    /// - a [`UiEvent::DoubleClick`] is the second press of a click pair,
    ///   so it hovers and presses exactly as a left `MouseDown` does;
    /// - a `MouseMoved` whose button mask holds no left button clears
    ///   the press, so a release this window never saw (the button let
    ///   go outside it) does not leave a press latched;
    /// - [`UiEvent::WindowFocused`]`(false)` clears both slots, since this
    ///   window cannot see where the pointer is.
    ///
    /// Applying the same event twice changes nothing the second time.
    pub fn track_painted(&mut self, event: &UiEvent) -> bool {
        match event {
            UiEvent::WindowFocused(false) => self.clear(),
            UiEvent::MouseDown {
                widget,
                position,
                button: MouseButton::Left,
                ..
            }
            | UiEvent::DoubleClick {
                widget,
                position,
                button: MouseButton::Left,
            } => {
                let hit = crate::primitive::painted_hit(position.x, position.y);
                let hovered = self.set_hovered_target(hit);
                let pressed = self.set_pressed_target(press_target(*widget, hit));
                hovered || pressed
            }
            UiEvent::MouseMoved { buttons, .. } => {
                let released = !buttons.left && self.set_pressed_target(None);
                let hovered = self.handle_mouse_at(event, crate::primitive::painted_hit);
                released || hovered
            }
            UiEvent::MouseUp {
                button: MouseButton::Left,
                ..
            } => self.handle_mouse_at(event, crate::primitive::painted_hit),
            _ => false,
        }
    }
}

/// The address a left press lands on: the press event's own resolved
/// `widget`, on the sub-part `hit` found when it agrees on the widget and
/// on no sub-part otherwise; `hit` as-is when the event names no widget.
fn press_target(widget: Option<WidgetId>, hit: Option<FocusAddress>) -> Option<FocusAddress> {
    match (widget, hit) {
        (Some(w), Some(h)) if h.0 == w => Some(h),
        (Some(w), _) => Some((w, None)),
        (None, hit) => hit,
    }
}

thread_local! {
    /// The pointer's hover/press state for the frame on screen, resolved
    /// against [`crate::primitive::PaintedWidgets`].
    ///
    /// A `thread_local!` for the same reasons that store is one: it is
    /// read from inside [`crate::DrawsPrimitives::draw_dyn`], which can
    /// only reach backend state through `Backend` methods (whose count is
    /// ratcheted downwards), and every backend handles input and paints
    /// on the one thread that owns its event loop.
    static AMBIENT: Cell<InteractionState> = const { Cell::new(InteractionState::new()) };

    /// Where the last pointer event on this thread put the pointer, or
    /// `None` once the window has lost sight of it — what
    /// [`ambient_for_painted`] re-resolves hover against.
    static POINTER: Cell<Option<Point>> = const { Cell::new(None) };
}

/// Feed `event` into this thread's ambient pointer state
/// ([`InteractionState::track_painted`]). Returns `true` if hover or
/// press changed, so the caller should redraw.
pub fn track_pointer(event: &UiEvent) -> bool {
    match event {
        UiEvent::MouseMoved { position, .. }
        | UiEvent::MouseDown { position, .. }
        | UiEvent::MouseUp { position, .. }
        | UiEvent::DoubleClick { position, .. }
        | UiEvent::Scroll { position, .. } => POINTER.with(|p| p.set(Some(*position))),
        UiEvent::WindowFocused(false) => POINTER.with(|p| p.set(None)),
        _ => {}
    }
    AMBIENT.with(|ambient| {
        let mut state = ambient.get();
        let changed = state.track_painted(event);
        ambient.set(state);
        changed
    })
}

/// The ambient pointer state as widget `id` paints it at `rect`, whose
/// absolute sub-part rects are `sub_regions`.
///
/// Hover is resolved against the frame on screen when a pointer event
/// arrives, so content that moves under a still pointer (a scroll, a key,
/// a timer) leaves it naming a part the pointer has left.
/// While `id` holds the ambient hover, this resolves that hover again
/// against the geometry `id` is painted at now: the sub-part under the
/// last pointer position, or no hover when the pointer is outside `rect`.
/// The result is stored as the ambient state, so the next pointer event
/// starts from what is painted. Hover on any other widget is returned
/// unchanged.
pub(crate) fn ambient_for_painted(
    id: &WidgetId,
    rect: Rect,
    sub_regions: &[(Rect, SubTarget)],
) -> InteractionState {
    let mut state = ambient_interaction();
    if state.hovered() != Some(id) {
        return state;
    }
    let Some(pointer) = POINTER.with(Cell::get) else {
        return state;
    };
    let (x, y) = (pointer.x, pointer.y);
    let target = crate::primitive::contains(rect, x, y).then(|| {
        let sub = sub_regions
            .iter()
            .find(|(r, _)| crate::primitive::contains(*r, x, y))
            .map(|(_, sub)| *sub);
        (*id, sub)
    });
    if state.set_hovered_target(target) {
        AMBIENT.with(|ambient| ambient.set(state));
    }
    state
}

/// A snapshot of this thread's ambient pointer state — what
/// [`crate::DrawsPrimitives::draw_dyn`] paints with when its caller
/// passes `&()`.
pub fn ambient_interaction() -> InteractionState {
    AMBIENT.with(Cell::get)
}

/// Clear this thread's ambient pointer state — for a test that must not
/// see hover left over from an earlier click, or a host tearing down the
/// widget tree the state refers to.
pub fn reset_ambient_interaction() {
    AMBIENT.with(|ambient| ambient.set(InteractionState::new()));
    POINTER.with(|p| p.set(None));
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

    /// A move with the left button held — a drag.
    fn mouse_dragged(x: f32, y: f32) -> UiEvent {
        UiEvent::MouseMoved {
            position: Point::new(x, y),
            buttons: crate::event::ButtonMask {
                left: true,
                ..Default::default()
            },
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

    use crate::interactive::SubTarget;
    use crate::primitive::{clear_painted_widgets, record_painted_widget, PaintedWidget};

    /// Paint one widget into this thread's record: `id` over x in
    /// `[0, 20)`, with sub-part 0 over `[0, 10)` and sub-part 1 over
    /// `[10, 20)`.
    fn paint_two_part_widget(id: WidgetId) {
        clear_painted_widgets();
        record_painted_widget(PaintedWidget {
            id,
            rect: crate::event::Rect::new(0.0, 0.0, 20.0, 1.0),
            layout: Box::new(()),
            sub_regions: vec![
                (crate::event::Rect::new(0.0, 0.0, 10.0, 1.0), SubTarget(0)),
                (crate::event::Rect::new(10.0, 0.0, 10.0, 1.0), SubTarget(1)),
            ],
            hover_targets: None,
            paint_state: Default::default(),
            focus_ring: None,
        });
    }

    #[test]
    fn track_painted_follows_the_sub_part_under_the_pointer() {
        let id = WidgetId::new("two-part");
        paint_two_part_widget(id);
        let mut state = InteractionState::new();
        assert!(state.track_painted(&mouse_moved(2.0, 0.0)));
        assert_eq!(state.hovered_target(), Some((id, Some(SubTarget(0)))));
        assert!(state.track_painted(&mouse_moved(12.0, 0.0)));
        assert_eq!(state.hovered_target(), Some((id, Some(SubTarget(1)))));
        assert!(
            !state.track_painted(&mouse_moved(13.0, 0.0)),
            "a move within the same sub-part is not a change"
        );
        assert!(state.track_painted(&mouse_moved(50.0, 0.0)));
        assert_eq!(state.hovered_target(), None);
        clear_painted_widgets();
    }

    #[test]
    fn track_painted_press_holds_its_address_while_the_pointer_drags_off() {
        let id = WidgetId::new("two-part");
        paint_two_part_widget(id);
        let mut state = InteractionState::new();
        assert!(state.track_painted(&mouse_down(None, 3.0, 0.0)));
        assert_eq!(state.pressed_target(), Some((id, Some(SubTarget(0)))));
        assert_eq!(
            state.hovered_target(),
            Some((id, Some(SubTarget(0)))),
            "a press moves hover to the pressed address"
        );
        assert!(
            !state.track_painted(&mouse_down(None, 3.0, 0.0)),
            "the same event twice changes nothing"
        );
        assert!(state.track_painted(&mouse_dragged(15.0, 0.0)));
        assert_eq!(state.pressed_target(), Some((id, Some(SubTarget(0)))));
        assert_eq!(state.hovered_target(), Some((id, Some(SubTarget(1)))));
        assert!(state.track_painted(&mouse_up(15.0, 0.0)));
        assert_eq!(state.pressed_target(), None);
        clear_painted_widgets();
    }

    /// A release delivered somewhere else (the button let go outside the
    /// window) shows up as a move with no button held: that move ends the
    /// press, so returning to the widget hovers it and does not press it.
    #[test]
    fn track_painted_a_move_with_no_button_held_ends_a_press() {
        let id = WidgetId::new("two-part");
        paint_two_part_widget(id);
        let mut state = InteractionState::new();
        state.track_painted(&mouse_down(None, 3.0, 0.0));
        assert!(state.track_painted(&mouse_moved(3.0, 0.0)));
        assert_eq!(state.pressed_target(), None);
        assert_eq!(state.hovered_target(), Some((id, Some(SubTarget(0)))));
        assert!(!state.track_painted(&mouse_moved(3.0, 0.0)));
        clear_painted_widgets();
    }

    /// The second press of a double-click hovers and presses the address
    /// under it, the same as a plain left press.
    #[test]
    fn track_painted_a_double_click_presses_what_is_under_it() {
        let id = WidgetId::new("two-part");
        paint_two_part_widget(id);
        let mut state = InteractionState::new();
        assert!(state.track_painted(&UiEvent::DoubleClick {
            widget: None,
            button: crate::MouseButton::Left,
            position: Point::new(12.0, 0.0),
        }));
        assert_eq!(state.pressed_target(), Some((id, Some(SubTarget(1)))));
        assert_eq!(state.hovered_target(), Some((id, Some(SubTarget(1)))));
        assert!(state.track_painted(&mouse_up(12.0, 0.0)));
        assert_eq!(state.pressed_target(), None);
        clear_painted_widgets();
    }

    /// A widget holding the ambient hover that is painted somewhere else
    /// keeps the hover only on the sub-part now under the last pointer
    /// position, and loses it once the pointer is outside it.
    #[test]
    fn ambient_hover_follows_a_widget_that_moves_under_a_still_pointer() {
        let id = WidgetId::new("two-part");
        paint_two_part_widget(id);
        reset_ambient_interaction();
        track_pointer(&mouse_moved(3.0, 0.0));
        assert_eq!(
            ambient_interaction().hovered_target(),
            Some((id, Some(SubTarget(0))))
        );

        let shifted = |x: f32| {
            vec![
                (crate::event::Rect::new(x, 0.0, 10.0, 1.0), SubTarget(0)),
                (
                    crate::event::Rect::new(x + 10.0, 0.0, 10.0, 1.0),
                    SubTarget(1),
                ),
            ]
        };
        let painted = ambient_for_painted(
            &id,
            crate::event::Rect::new(-10.0, 0.0, 20.0, 1.0),
            &shifted(-10.0),
        );
        assert_eq!(painted.hovered_target(), Some((id, Some(SubTarget(1)))));
        assert_eq!(ambient_interaction(), painted);

        let gone = ambient_for_painted(
            &id,
            crate::event::Rect::new(30.0, 0.0, 20.0, 1.0),
            &shifted(30.0),
        );
        assert_eq!(gone.hovered_target(), None);
        assert_eq!(ambient_interaction().hovered_target(), None);
        reset_ambient_interaction();
        clear_painted_widgets();
    }

    /// Re-resolving one widget leaves another widget's hover alone.
    #[test]
    fn ambient_for_painted_ignores_a_widget_that_is_not_hovered() {
        let id = WidgetId::new("two-part");
        paint_two_part_widget(id);
        reset_ambient_interaction();
        track_pointer(&mouse_moved(3.0, 0.0));
        let other = WidgetId::new("elsewhere");
        let painted =
            ambient_for_painted(&other, crate::event::Rect::new(0.0, 0.0, 20.0, 1.0), &[]);
        assert_eq!(painted.hovered_target(), Some((id, Some(SubTarget(0)))));
        reset_ambient_interaction();
        clear_painted_widgets();
    }

    #[test]
    fn track_painted_press_falls_back_to_the_events_own_widget() {
        let id = WidgetId::new("two-part");
        paint_two_part_widget(id);
        let other = WidgetId::new("resolved-elsewhere");
        let mut state = InteractionState::new();
        state.track_painted(&mouse_down(Some(other), 3.0, 0.0));
        assert_eq!(state.pressed_target(), Some((other, None)));
        state.track_painted(&mouse_down(Some(id), 12.0, 0.0));
        assert_eq!(state.pressed_target(), Some((id, Some(SubTarget(1)))));
        clear_painted_widgets();
    }

    #[test]
    fn track_painted_window_blur_clears_both_slots() {
        let id = WidgetId::new("two-part");
        paint_two_part_widget(id);
        let mut state = InteractionState::new();
        state.track_painted(&mouse_down(None, 3.0, 0.0));
        assert!(state.track_painted(&UiEvent::WindowFocused(false)));
        assert_eq!(state, InteractionState::new());
        assert!(!state.track_painted(&UiEvent::WindowFocused(false)));
        clear_painted_widgets();
    }

    #[test]
    fn handle_mouse_keeps_widget_level_addresses() {
        let mut state = InteractionState::new();
        state.handle_mouse(&mouse_moved(1.0, 0.0), |_, _| Some(WidgetId::new("w")));
        assert_eq!(state.hovered_target(), Some((WidgetId::new("w"), None)));
    }

    #[test]
    fn ambient_state_tracks_pointer_events_until_reset() {
        let id = WidgetId::new("two-part");
        paint_two_part_widget(id);
        reset_ambient_interaction();
        assert!(track_pointer(&mouse_moved(12.0, 0.0)));
        assert!(!track_pointer(&mouse_moved(12.0, 0.0)));
        assert_eq!(
            ambient_interaction().hovered_target(),
            Some((id, Some(SubTarget(1))))
        );
        reset_ambient_interaction();
        assert_eq!(ambient_interaction(), InteractionState::new());
        clear_painted_widgets();
    }
}
