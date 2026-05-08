//! Lightweight hover/press state tracker for [`crate::StatusBar`].
//!
//! Consumers store a [`StatusBarInteraction`] alongside their status bar,
//! call [`StatusBarInteraction::handle`] with each mouse event and the
//! bar's rect, then read [`hovered_id`](StatusBarInteraction::hovered_id)
//! and [`pressed_id`](StatusBarInteraction::pressed_id) when calling
//! `Backend::draw_status_bar`.
//!
//! This eliminates per-backend mouse routing code for status bar hover —
//! backends just forward `UiEvent`s uniformly.

use std::cell::RefCell;

use crate::event::{MouseButton, Point, Rect, UiEvent};
use crate::primitives::status_bar::StatusBarHitRegion;
use crate::types::WidgetId;

/// Tracks hover and pressed state for a status bar's clickable segments.
#[derive(Debug, Clone, Default)]
pub struct StatusBarInteraction {
    hit_regions: RefCell<Vec<StatusBarHitRegion>>,
    hovered: Option<WidgetId>,
    pressed: Option<WidgetId>,
}

/// What happened after [`StatusBarInteraction::handle`] processed an event.
#[derive(Debug, Clone, PartialEq, Eq)]
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

    /// Update the stored hit regions. Call this after each `draw_status_bar`
    /// with the returned `Vec<StatusBarHitRegion>`. Takes `&self` so it can
    /// be called from render paths where only a shared reference is available.
    pub fn set_hit_regions(&self, regions: Vec<StatusBarHitRegion>) {
        *self.hit_regions.borrow_mut() = regions;
    }

    pub fn hovered_id(&self) -> Option<&WidgetId> {
        self.hovered.as_ref()
    }

    pub fn pressed_id(&self) -> Option<&WidgetId> {
        self.pressed.as_ref()
    }

    /// Process a mouse event relative to the status bar's rect.
    /// Returns what action (if any) resulted.
    pub fn handle(&mut self, event: &UiEvent, bar_rect: Rect) -> StatusBarAction {
        match event {
            UiEvent::MouseMoved { position, .. } => {
                let new_hover = self.hit_test(bar_rect, *position);
                if new_hover != self.hovered {
                    self.hovered = new_hover;
                    StatusBarAction::Redraw
                } else {
                    StatusBarAction::Ignored
                }
            }
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => {
                let hit = self.hit_test(bar_rect, *position);
                if hit.is_some() {
                    self.pressed = hit;
                    StatusBarAction::Redraw
                } else {
                    StatusBarAction::Ignored
                }
            }
            UiEvent::MouseUp {
                button: MouseButton::Left,
                position,
                ..
            } => {
                let was_pressed = self.pressed.take();
                if let Some(ref pressed_id) = was_pressed {
                    let hit = self.hit_test(bar_rect, *position);
                    if hit.as_ref() == Some(pressed_id) {
                        return StatusBarAction::Clicked(pressed_id.clone());
                    }
                }
                if was_pressed.is_some() {
                    StatusBarAction::Redraw
                } else {
                    StatusBarAction::Ignored
                }
            }
            _ => StatusBarAction::Ignored,
        }
    }

    fn hit_test(&self, bar_rect: Rect, position: Point) -> Option<WidgetId> {
        if position.x < bar_rect.x
            || position.x >= bar_rect.x + bar_rect.width
            || position.y < bar_rect.y
            || position.y >= bar_rect.y + bar_rect.height
        {
            return None;
        }
        let local_x = (position.x - bar_rect.x) as u16;
        self.hit_regions
            .borrow()
            .iter()
            .find(|r| local_x >= r.col && local_x < r.col + r.width)
            .map(|r| r.id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar_rect() -> Rect {
        Rect::new(0.0, 100.0, 80.0, 1.0)
    }

    fn regions() -> Vec<StatusBarHitRegion> {
        vec![
            StatusBarHitRegion {
                col: 0,
                width: 10,
                id: WidgetId::new("run"),
            },
            StatusBarHitRegion {
                col: 10,
                width: 10,
                id: WidgetId::new("stop"),
            },
        ]
    }

    #[test]
    fn hover_sets_hovered_id() {
        let mut sbi = StatusBarInteraction::new();
        sbi.set_hit_regions(regions());
        let ev = UiEvent::MouseMoved {
            position: Point { x: 5.0, y: 100.5 },
            buttons: Default::default(),
        };
        let action = sbi.handle(&ev, bar_rect());
        assert_eq!(action, StatusBarAction::Redraw);
        assert_eq!(sbi.hovered_id(), Some(&WidgetId::new("run")));
    }

    #[test]
    fn hover_outside_clears() {
        let mut sbi = StatusBarInteraction::new();
        sbi.set_hit_regions(regions());
        // Move into "run" first.
        sbi.handle(
            &UiEvent::MouseMoved {
                position: Point { x: 5.0, y: 100.5 },
                buttons: Default::default(),
            },
            bar_rect(),
        );
        // Move outside the bar.
        let action = sbi.handle(
            &UiEvent::MouseMoved {
                position: Point { x: 5.0, y: 50.0 },
                buttons: Default::default(),
            },
            bar_rect(),
        );
        assert_eq!(action, StatusBarAction::Redraw);
        assert_eq!(sbi.hovered_id(), None);
    }

    #[test]
    fn click_emits_clicked() {
        let mut sbi = StatusBarInteraction::new();
        sbi.set_hit_regions(regions());
        let rect = bar_rect();
        // Mouse down on "stop".
        sbi.handle(
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
        let action = sbi.handle(
            &UiEvent::MouseUp {
                button: MouseButton::Left,
                position: Point { x: 15.0, y: 100.5 },
                widget: None,
            },
            rect,
        );
        assert_eq!(action, StatusBarAction::Clicked(WidgetId::new("stop")));
        assert_eq!(sbi.pressed_id(), None);
    }

    #[test]
    fn press_drag_away_does_not_click() {
        let mut sbi = StatusBarInteraction::new();
        sbi.set_hit_regions(regions());
        let rect = bar_rect();
        // Mouse down on "run".
        sbi.handle(
            &UiEvent::MouseDown {
                button: MouseButton::Left,
                position: Point { x: 5.0, y: 100.5 },
                modifiers: Default::default(),
                widget: None,
            },
            rect,
        );
        // Mouse up outside any segment.
        let action = sbi.handle(
            &UiEvent::MouseUp {
                button: MouseButton::Left,
                position: Point { x: 50.0, y: 100.5 },
                widget: None,
            },
            rect,
        );
        assert_eq!(action, StatusBarAction::Redraw);
    }
}
