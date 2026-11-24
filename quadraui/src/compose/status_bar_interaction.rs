//! `UiEvent` adapter for [`crate::StatusBar`]'s own hover/press state.
//!
//! Consumers store a [`StatusBarInteraction`] alongside their status bar,
//! call [`StatusBarInteraction::handle_event`] with each mouse event and
//! the bar's rect, then read [`hovered_id`](StatusBarInteraction::hovered_id)
//! and [`pressed_id`](StatusBarInteraction::pressed_id) when calling
//! `Backend::draw_status_bar`.
//!
//! The state machine itself is the widget's own
//! ([`StatusBarState::hover`]/[`press`](StatusBarState::press)/
//! [`release`](StatusBarState::release), hit-tested by
//! [`crate::StatusBar::segment_at`]), the same one
//! [`crate::StatusBar`]'s [`crate::interactive::Interactive`] impl drives
//! from a [`crate::interactive::RoutedEvent`]. This type only translates
//! raw [`UiEvent`]s and a bar rect into those calls, for a host that
//! paints its bar through `Backend::draw_status_bar` rather than reaching
//! the frame as a widget.

use std::cell::RefCell;

use crate::compose::Handled;
use crate::event::{MouseButton, Point, Rect, UiEvent};
use crate::primitives::status_bar::{StatusBar, StatusBarLayout, StatusBarRelease, StatusBarState};
use crate::types::WidgetId;

/// Tracks hover and pressed state for a status bar's clickable segments.
#[derive(Debug, Clone, Default)]
pub struct StatusBarInteraction {
    layout: RefCell<Option<StatusBarLayout>>,
    state: StatusBarState,
}

/// What happened after [`StatusBarInteraction::handle`] processed an event.
#[derive(Debug, Clone, PartialEq, Eq)]
#[deprecated(
    since = "0.9.0",
    note = "use `StatusBarInteraction::handle_event`'s `Handled<WidgetId>` instead — `Redraw` is `Handled::Consumed`, `Ignored` is `Handled::Ignored`, `Clicked(id)` is `Handled::Event(id)`"
)]
pub enum StatusBarAction {
    /// A clickable segment was clicked (mouse-down + mouse-up on the same segment).
    Clicked(WidgetId),
    /// State changed (hover entered/left, press started) — caller should redraw.
    Redraw,
    /// Event not relevant to the status bar.
    Ignored,
}

impl StatusBarInteraction {
    pub fn new() -> Self {
        Self::default()
    }

    /// Update the stored layout. Call this after each `draw_status_bar`
    /// with the returned `StatusBarLayout`. Takes `&self` so it can
    /// be called from render paths where only a shared reference is available.
    pub fn set_layout(&self, layout: StatusBarLayout) {
        *self.layout.borrow_mut() = Some(layout);
    }

    pub fn hovered_id(&self) -> Option<&WidgetId> {
        self.state.hovered.as_ref()
    }

    pub fn pressed_id(&self) -> Option<&WidgetId> {
        self.state.pressed.as_ref()
    }

    /// Process a mouse event relative to the status bar's rect.
    ///
    /// Returns [`Handled::Event`] with the clicked segment's id when a
    /// clickable segment was clicked (mouse-down + mouse-up on the same
    /// segment), [`Handled::Consumed`] when hover/press state changed
    /// (caller should redraw), or [`Handled::Ignored`] when the event
    /// wasn't relevant to the status bar.
    ///
    /// Note: `Handled::Ignored` here also covers "inside `bar_rect`, but
    /// nothing changed" — a `MouseMoved` whose hover target is unchanged,
    /// or a `MouseUp` with nothing pressed. Those are events the status bar
    /// genuinely saw (unlike a click entirely outside `bar_rect`), just with
    /// no state change and so no redraw to request; callers that fall
    /// through on `Handled::Ignored` (e.g. to a title-bar-drag gesture) rely
    /// on this.
    pub fn handle_event(&mut self, event: &UiEvent, bar_rect: Rect) -> Handled<WidgetId> {
        match event {
            UiEvent::MouseMoved { position, .. } => {
                let hit = self.hit_test(bar_rect, *position);
                if self.state.hover(hit) {
                    Handled::Consumed
                } else {
                    Handled::Ignored
                }
            }
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => {
                let hit = self.hit_test(bar_rect, *position);
                if self.state.press(hit) {
                    Handled::Consumed
                } else {
                    Handled::Ignored
                }
            }
            UiEvent::MouseUp {
                button: MouseButton::Left,
                position,
                ..
            } => {
                let hit = self.hit_test(bar_rect, *position);
                match self.state.release(hit) {
                    StatusBarRelease::Clicked(id) => Handled::Event(id),
                    StatusBarRelease::Dropped => Handled::Consumed,
                    StatusBarRelease::Idle => Handled::Ignored,
                }
            }
            _ => Handled::Ignored,
        }
    }

    /// Process a mouse event relative to the status bar's rect.
    /// Returns what action (if any) resulted.
    #[deprecated(
        since = "0.9.0",
        note = "use `handle_event`, which returns `Handled<WidgetId>` instead of `StatusBarAction`"
    )]
    #[allow(deprecated)] // `StatusBarAction` is `#[deprecated]`; this method is its own shim.
    pub fn handle(&mut self, event: &UiEvent, bar_rect: Rect) -> StatusBarAction {
        match self.handle_event(event, bar_rect) {
            Handled::Event(id) => StatusBarAction::Clicked(id),
            Handled::Consumed => StatusBarAction::Redraw,
            Handled::Ignored => StatusBarAction::Ignored,
        }
    }

    /// Which clickable segment `position` lands on, translated into the
    /// bar-local space [`crate::StatusBar::segment_at`] answers in.
    fn hit_test(&self, bar_rect: Rect, position: Point) -> Option<WidgetId> {
        if position.x < bar_rect.x
            || position.x >= bar_rect.x + bar_rect.width
            || position.y < bar_rect.y
            || position.y >= bar_rect.y + bar_rect.height
        {
            return None;
        }
        let local_x = position.x - bar_rect.x;
        let local_y = position.y - bar_rect.y;
        let guard = self.layout.borrow();
        guard
            .as_ref()
            .and_then(|l| StatusBar::segment_at(l, local_x, local_y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::status_bar::StatusBarHit;

    fn bar_rect() -> Rect {
        Rect::new(0.0, 100.0, 80.0, 1.0)
    }

    fn test_layout() -> StatusBarLayout {
        StatusBarLayout {
            bar_width: 80.0,
            bar_height: 1.0,
            visible_segments: Vec::new(),
            hit_regions: vec![
                (
                    crate::event::Rect::new(0.0, 0.0, 10.0, 1.0),
                    StatusBarHit::Segment(WidgetId::new("run")),
                ),
                (
                    crate::event::Rect::new(10.0, 0.0, 10.0, 1.0),
                    StatusBarHit::Segment(WidgetId::new("stop")),
                ),
            ],
            resolved_right_start: 0,
        }
    }

    #[test]
    fn hover_sets_hovered_id() {
        let mut sbi = StatusBarInteraction::new();
        sbi.set_layout(test_layout());
        let ev = UiEvent::MouseMoved {
            position: Point { x: 5.0, y: 100.5 },
            buttons: Default::default(),
        };
        let action = sbi.handle_event(&ev, bar_rect());
        assert_eq!(action, Handled::Consumed);
        assert_eq!(sbi.hovered_id(), Some(&WidgetId::new("run")));
    }

    #[test]
    fn hover_outside_clears() {
        let mut sbi = StatusBarInteraction::new();
        sbi.set_layout(test_layout());
        // Move into "run" first.
        let _ = sbi.handle_event(
            &UiEvent::MouseMoved {
                position: Point { x: 5.0, y: 100.5 },
                buttons: Default::default(),
            },
            bar_rect(),
        );
        // Move outside the bar.
        let action = sbi.handle_event(
            &UiEvent::MouseMoved {
                position: Point { x: 5.0, y: 50.0 },
                buttons: Default::default(),
            },
            bar_rect(),
        );
        assert_eq!(action, Handled::Consumed);
        assert_eq!(sbi.hovered_id(), None);
    }

    #[test]
    fn click_emits_clicked() {
        let mut sbi = StatusBarInteraction::new();
        sbi.set_layout(test_layout());
        let rect = bar_rect();
        // Mouse down on "stop".
        let _ = sbi.handle_event(
            &UiEvent::MouseDown {
                button: MouseButton::Left,
                position: Point { x: 15.0, y: 100.5 },
                modifiers: Default::default(),
                widget: None,
            },
            rect,
        );
        assert_eq!(sbi.pressed_id(), Some(&WidgetId::new("stop")));
        // Mouse up on same segment.
        let action = sbi.handle_event(
            &UiEvent::MouseUp {
                button: MouseButton::Left,
                position: Point { x: 15.0, y: 100.5 },
                widget: None,
            },
            rect,
        );
        assert_eq!(action, Handled::Event(WidgetId::new("stop")));
        assert_eq!(sbi.pressed_id(), None);
    }

    #[test]
    fn press_drag_away_does_not_click() {
        let mut sbi = StatusBarInteraction::new();
        sbi.set_layout(test_layout());
        let rect = bar_rect();
        // Mouse down on "run".
        let _ = sbi.handle_event(
            &UiEvent::MouseDown {
                button: MouseButton::Left,
                position: Point { x: 5.0, y: 100.5 },
                modifiers: Default::default(),
                widget: None,
            },
            rect,
        );
        // Mouse up outside any segment.
        let action = sbi.handle_event(
            &UiEvent::MouseUp {
                button: MouseButton::Left,
                position: Point { x: 50.0, y: 100.5 },
                widget: None,
            },
            rect,
        );
        assert_eq!(action, Handled::Consumed);
    }

    #[test]
    #[allow(deprecated)] // covers the `handle` shim kept for pre-`Handled` callers.
    fn deprecated_handle_still_adapts_to_status_bar_action() {
        let mut sbi = StatusBarInteraction::new();
        sbi.set_layout(test_layout());
        let rect = bar_rect();
        sbi.handle(
            &UiEvent::MouseDown {
                button: MouseButton::Left,
                position: Point { x: 15.0, y: 100.5 },
                modifiers: Default::default(),
                widget: None,
            },
            rect,
        );
        let action = sbi.handle(
            &UiEvent::MouseUp {
                button: MouseButton::Left,
                position: Point { x: 15.0, y: 100.5 },
                widget: None,
            },
            rect,
        );
        assert_eq!(action, StatusBarAction::Clicked(WidgetId::new("stop")));
    }
}
