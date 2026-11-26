//! Driving a contract widget from a compose controller.
//!
//! A controller such as [`crate::compose::MessageDialogController`] holds
//! a widget descriptor plus its [`Interactive::State`] and is called with
//! raw [`UiEvent`]s, outside the runtime's routing engine. This module is
//! the small piece of that engine a controller needs: it turns one
//! `UiEvent` into the [`RoutedEvent`] the widget's
//! [`Interactive::handle`] takes, resolved against the layout the
//! controller paints, and runs `handle` against a host that records what
//! the widget asked for.

use crate::compose::Handled;
use crate::event::{Point, Rect, UiEvent};
use crate::interactive::{
    EventCx, EventHost, Interactive, KeyEvent, PointerKind, RoutedEvent, SubTarget,
};
use crate::primitive::Units;
use crate::types::WidgetId;
use crate::{MouseButton, PointerShape};

/// What one [`drive`] call produced: the widget's own answer plus every
/// [`UiEvent`] it handed to [`EventCx::emit`] along the way, in order.
pub(crate) struct Driven<A> {
    /// The widget's answer.
    pub(crate) handled: Handled<A>,
    /// Events the widget emitted besides its answer.
    pub(crate) emitted: Vec<UiEvent>,
}

/// The [`EventHost`] a controller drives its widget against. The widget
/// is the controller's whole world, so it always holds focus; focus,
/// capture and cursor requests have nowhere further to go.
struct ControllerHost {
    units: Units,
    emitted: Vec<UiEvent>,
}

impl EventHost for ControllerHost {
    fn is_focused(&self, _id: &WidgetId, _sub: Option<SubTarget>) -> bool {
        true
    }

    fn request_focus(&mut self, _id: &WidgetId, _sub: Option<SubTarget>) {}

    fn capture_pointer(&mut self, _id: &WidgetId) {}

    fn release_pointer(&mut self) {}

    fn set_cursor(&mut self, _cursor: PointerShape) {}

    fn request_redraw(&mut self) {}

    fn emit(&mut self, event: UiEvent) {
        self.emitted.push(event);
    }

    fn units(&self) -> Units {
        self.units
    }
}

/// Run `widget`'s [`Interactive::handle`] for `event` against `layout`.
pub(crate) fn drive<W: Interactive>(
    widget: &W,
    id: WidgetId,
    state: &mut W::State,
    event: &RoutedEvent,
    layout: &W::Layout,
    units: Units,
) -> Driven<W::Action> {
    let mut host = ControllerHost {
        units,
        emitted: Vec::new(),
    };
    let handled = {
        let mut cx = EventCx::new(id, &mut host);
        widget.handle(state, event, layout, &mut cx)
    };
    Driven {
        handled,
        emitted: host.emitted,
    }
}

/// The [`RoutedEvent`] `event` becomes for a widget painted at `area`, or
/// `None` for an event no widget handles (a window or app event).
///
/// Pointer positions are made relative to `area`'s origin, and `sub` is
/// the first of the widget's sub-regions that contains the pointer —
/// the same resolution the runtime's routing engine performs.
pub(crate) fn routed_event(
    event: &UiEvent,
    area: Rect,
    sub_regions: &[(Rect, SubTarget)],
) -> Option<RoutedEvent> {
    let local = |p: Point| Point::new(p.x - area.x, p.y - area.y);
    let sub_at = |p: Point| {
        sub_regions
            .iter()
            .find(|(r, _)| r.contains(p))
            .map(|(_, sub)| *sub)
    };
    let pointer = |kind: PointerKind, position: Point, mods: crate::Modifiers| {
        let local = local(position);
        RoutedEvent::Pointer {
            kind,
            sub: sub_at(local),
            local,
            mods,
        }
    };
    match event {
        UiEvent::KeyPressed {
            key,
            modifiers,
            repeat,
        } => Some(RoutedEvent::Key(KeyEvent {
            key: key.clone(),
            modifiers: *modifiers,
            repeat: *repeat,
        })),
        UiEvent::CharTyped(c) => Some(RoutedEvent::Text(c.to_string())),
        UiEvent::ClipboardPaste(text) => Some(RoutedEvent::Text(text.clone())),
        UiEvent::MouseDown {
            button,
            position,
            modifiers,
            ..
        } => Some(pointer(PointerKind::Down(*button), *position, *modifiers)),
        UiEvent::MouseUp {
            button, position, ..
        } => Some(pointer(
            PointerKind::Up(*button),
            *position,
            crate::Modifiers::default(),
        )),
        UiEvent::MouseMoved { position, .. } => Some(pointer(
            PointerKind::Move,
            *position,
            crate::Modifiers::default(),
        )),
        UiEvent::DoubleClick { position, .. } => Some(pointer(
            PointerKind::DoubleClick(MouseButton::Left),
            *position,
            crate::Modifiers::default(),
        )),
        UiEvent::Scroll {
            delta, position, ..
        } => Some(RoutedEvent::Wheel {
            dx: delta.x,
            dy: delta.y,
            local: local(*position),
            mods: crate::Modifiers::default(),
        }),
        _ => None,
    }
}
