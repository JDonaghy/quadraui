//! Driving a contract widget from a compose controller.
//!
//! A controller such as [`crate::compose::MessageDialogController`], or a
//! host whose events do not pass through the runtime's routing engine
//! ([`drive_ui_event`]), holds a widget descriptor plus its
//! [`Interactive::State`] and is called with raw [`UiEvent`]s. This module
//! is the small piece of that engine such a host needs: it turns one
//! `UiEvent` into the [`RoutedEvent`] the widget's
//! [`Interactive::handle`] takes, resolved against the layout the
//! controller paints, and runs `handle` against a host that records what
//! the widget asked for.

use crate::backend::{Backend, DrawsPrimitives};
use crate::compose::Handled;
use crate::event::{Point, Rect, UiEvent};
use crate::interactive::{
    EventCx, EventHost, Interactive, KeyEvent, PointerKind, RoutedEvent, SubTarget,
};
use crate::primitive::Units;
use crate::types::WidgetId;
use crate::{MouseButton, PointerShape};

/// What one [`drive_ui_event`] call produced: the widget's own answer
/// plus every [`UiEvent`] it handed to [`EventCx::emit`] along the way,
/// in order.
#[derive(Debug)]
pub struct Driven<A> {
    /// The widget's answer.
    pub handled: Handled<A>,
    /// Events the widget emitted besides its answer.
    pub emitted: Vec<UiEvent>,
}

/// The [`EventHost`] a controller drives its widget against. The widget
/// is the controller's whole world, so it holds focus — on every part,
/// or only on `focused` when the controller names one; focus, capture
/// and cursor requests have nowhere further to go.
struct ControllerHost {
    units: Units,
    /// `None` focuses every part; `Some(part)` only `part`, and
    /// `Some(None)` no part at all.
    focused: Option<Option<SubTarget>>,
    emitted: Vec<UiEvent>,
}

impl EventHost for ControllerHost {
    fn is_focused(&self, _id: &WidgetId, sub: Option<SubTarget>) -> bool {
        match self.focused {
            None => true,
            Some(focused) => focused.is_some() && sub == focused,
        }
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
    let (handled, emitted) = with_cx(id, units, |cx| widget.handle(state, event, layout, cx));
    Driven { handled, emitted }
}

/// Drive `widget` with one raw [`UiEvent`], laid out at `rect` through
/// `backend` exactly as [`DrawsPrimitives::draw_dyn`] paints it there —
/// for a host whose events do not reach the widget through
/// [`crate::runner::AppLogic::frame_tree`], such as a
/// [`crate::shell::ShellApp`] or a gallery demo. The host keeps the
/// widget's [`Interactive::State`] and applies what it reports.
///
/// The widget is the host's whole world here, so every part of it holds
/// focus. `None` for an event no widget handles (a window or app event).
pub fn drive_ui_event<W: Interactive>(
    widget: &W,
    state: &mut W::State,
    event: &UiEvent,
    backend: &dyn Backend,
    rect: Rect,
) -> Option<Driven<W::Action>> {
    let layout = backend.layout_primitive(rect, widget);
    let subs = Interactive::sub_regions(widget, &layout);
    let routed = routed_event(event, rect, &subs)?;
    let id = crate::primitive::Primitive::id(widget)
        .copied()
        .unwrap_or_else(|| WidgetId::new("widget"));
    let units = crate::backend::primitive_layout_ctx(backend).units();
    Some(drive(widget, id, state, &routed, &layout, units))
}

/// [`drive`], with only the widget's `focused` part holding focus (no
/// part, for `None`) — for a controller that tracks which of a
/// composite's parts the keyboard addresses.
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub(crate) fn drive_focused<W: Interactive>(
    widget: &W,
    id: WidgetId,
    state: &mut W::State,
    event: &RoutedEvent,
    layout: &W::Layout,
    units: Units,
    focused: Option<SubTarget>,
) -> Driven<W::Action> {
    let (handled, emitted) = run(id, units, Some(focused), |cx| {
        widget.handle(state, event, layout, cx)
    });
    Driven { handled, emitted }
}

/// Run `f` with an [`EventCx`] for `id` over a controller host, returning
/// its answer and every [`UiEvent`] it emitted, in order — for a
/// controller that drives a widget's behaviour functions directly rather
/// than through [`Interactive::handle`].
pub(crate) fn with_cx<R>(
    id: WidgetId,
    units: Units,
    f: impl FnOnce(&mut EventCx) -> R,
) -> (R, Vec<UiEvent>) {
    run(id, units, None, f)
}

fn run<R>(
    id: WidgetId,
    units: Units,
    focused: Option<Option<SubTarget>>,
    f: impl FnOnce(&mut EventCx) -> R,
) -> (R, Vec<UiEvent>) {
    let mut host = ControllerHost {
        units,
        focused,
        emitted: Vec::new(),
    };
    let answer = {
        let mut cx = EventCx::new(id, &mut host);
        f(&mut cx)
    };
    (answer, host.emitted)
}

/// The [`EventHost`] a composite widget's child handles an event
/// against: every request goes to the composite's own [`EventCx`], with
/// focus re-addressed to one of the composite's parts (or, for a child
/// that shares the composite's id, passed through unchanged). A pointer
/// capture or release is recorded so the composite keeps routing the
/// gesture to the child, and emitted events can be collected for the
/// composite to translate instead of forwarded.
pub(crate) struct ChildHost<'a, 'b> {
    parent: &'a mut EventCx<'b>,
    /// `Some(part)` re-addresses focus to the composite's `part`; `None`
    /// passes the child's own sub-target through.
    part: Option<Option<SubTarget>>,
    /// `Some(true)` once the child captured the pointer, `Some(false)`
    /// once it released it.
    pub(crate) captured: Option<bool>,
    collected: Option<Vec<UiEvent>>,
}

impl<'a, 'b> ChildHost<'a, 'b> {
    /// A host addressing `part` of the composite `parent` handles for.
    pub(crate) fn new(parent: &'a mut EventCx<'b>, part: Option<SubTarget>) -> Self {
        Self {
            parent,
            part: Some(part),
            captured: None,
            collected: None,
        }
    }

    #[cfg(any(
        feature = "gtk",
        feature = "tui",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    /// A host for a child that shares `parent`'s id, so its sub-targets
    /// are the composite's own.
    pub(crate) fn same(parent: &'a mut EventCx<'b>) -> Self {
        Self {
            parent,
            part: None,
            captured: None,
            collected: None,
        }
    }

    #[cfg(any(
        feature = "gtk",
        feature = "tui",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    /// This host, keeping what the child emits for
    /// [`Self::take_collected`] instead of forwarding it.
    pub(crate) fn collecting(mut self) -> Self {
        self.collected = Some(Vec::new());
        self
    }

    #[cfg(any(
        feature = "gtk",
        feature = "tui",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    /// The events the child emitted, in order.
    pub(crate) fn take_collected(&mut self) -> Vec<UiEvent> {
        self.collected.take().unwrap_or_default()
    }
}

impl EventHost for ChildHost<'_, '_> {
    fn is_focused(&self, _id: &WidgetId, sub: Option<SubTarget>) -> bool {
        self.parent.is_focused(self.part.unwrap_or(sub))
    }

    fn request_focus(&mut self, _id: &WidgetId, sub: Option<SubTarget>) {
        let part = self.part.unwrap_or(sub);
        self.parent.request_focus(part);
    }

    fn capture_pointer(&mut self, _id: &WidgetId) {
        self.parent.capture_pointer();
        self.captured = Some(true);
    }

    fn release_pointer(&mut self) {
        self.parent.release_pointer();
        self.captured = Some(false);
    }

    fn set_cursor(&mut self, cursor: PointerShape) {
        self.parent.set_cursor(cursor);
    }

    fn request_redraw(&mut self) {
        self.parent.request_redraw();
    }

    fn emit(&mut self, event: UiEvent) {
        match &mut self.collected {
            Some(collected) => collected.push(event),
            None => self.parent.emit(event),
        }
    }

    fn units(&self) -> Units {
        self.parent.units()
    }
}

/// `event` moved into a child's local space: a pointer or wheel
/// position shifted by `-delta` (the child's offset inside its parent),
/// and a pointer's sub-target replaced by `sub`, the child's own part.
pub(crate) fn child_event(
    event: &RoutedEvent,
    delta: Point,
    sub: Option<SubTarget>,
) -> RoutedEvent {
    let shift = |p: &Point| Point::new(p.x - delta.x, p.y - delta.y);
    match event {
        RoutedEvent::Pointer {
            kind, local, mods, ..
        } => RoutedEvent::Pointer {
            kind: *kind,
            sub,
            local: shift(local),
            mods: *mods,
        },
        RoutedEvent::Wheel {
            dx,
            dy,
            local,
            mods,
        } => RoutedEvent::Wheel {
            dx: *dx,
            dy: *dy,
            local: shift(local),
            mods: *mods,
        },
        other => other.clone(),
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

/// Run `widget`'s [`Interactive::handle`] for `event`, laid out on a cell
/// grid at `rect` — for a controller with no backend in reach, whose
/// keystrokes read nothing of the layout but its page size.
pub(crate) fn drive_in_cells<W: Interactive>(
    widget: &W,
    state: &mut W::State,
    event: &UiEvent,
    rect: Rect,
) -> Option<Driven<W::Action>> {
    let ctx = crate::primitive::LayoutCtx::new(1.0, 1.0, false);
    let layout = crate::primitive::Primitive::layout(widget, &ctx, rect);
    let routed = routed_event(event, rect, &[])?;
    let id = widget
        .id()
        .copied()
        .unwrap_or_else(|| WidgetId::new("controller"));
    Some(drive(widget, id, state, &routed, &layout, ctx.units()))
}
