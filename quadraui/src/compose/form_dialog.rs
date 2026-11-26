//! [`FormDialog`] — a [`Dialog`] hosting a multi-field [`Form`] on the
//! widget contract, plus [`FormDialogController`], its thin
//! event-driven wrapper.
//!
//! A `FormDialog` composes two contract widgets: the modal chrome and
//! button row paint and hit-test through [`Dialog`]'s own [`Primitive`],
//! and the body paints through [`Form`]'s, in the dialog's form slot
//! ([`DialogInput::Form`]). Input on the form goes to `Form`'s own
//! [`Interactive`], so a dialog's embedded form gets keystroke editing,
//! validation display and visibility exactly like a full-page form. The
//! dialog's own job is the piece neither child has: one keyboard focus
//! order across the form's fields and the dialog's buttons.
//!
//! # Usage pattern
//!
//! ```rust,ignore
//! let mut dialog = FormDialogController::new("create_milestone", "Create milestone")
//!     .with_buttons(vec![
//!         DialogButton { id: WidgetId::new("cancel"), label: "Cancel".into(), is_cancel: true, is_default: false, tint: None, spec: None },
//!         DialogButton { id: WidgetId::new("create"), label: "Create".into(), is_default: true, is_cancel: false, tint: None, spec: None },
//!     ]);
//! dialog.set_form(my_form); // rebuilt every frame from app-owned field values
//!
//! // AppLogic::render:
//! dialog.render(backend);
//!
//! // AppLogic::handle:
//! match dialog.handle_event(&event, backend) {
//!     Handled::Event(FormDialogAction::ButtonClicked(id)) if id.as_str() == "create" => {
//!         // Read the app's own canonical field values (already synced via
//!         // FormAction(FormEvent::TextInputChanged { .. }) below) — same
//!         // "app owns the data" contract `Form`'s own module doc states.
//!     }
//!     Handled::Event(FormDialogAction::FormAction(event)) => { /* apply to app state */ }
//!     _ => {}
//! }
//! ```
//!
//! # Focus model
//!
//! [`Interactive::focus`] is [`FocusSpec::Children`]: one stop per
//! focusable form field (in [`focus_order`]), followed by one stop per
//! button. Tab/Shift-Tab walk those stops as one cyclic order — stepping
//! past the last field lands on the first button, and stepping back past
//! the first field lands on the last button. Within the form, Tab is the
//! form's own field traversal. Left/Right additionally cycle among the
//! buttons once they have focus. Enter activates the focused button when
//! one has focus, otherwise it goes to the form (commits a `TextInput`,
//! inserts a newline in a `TextArea`, …). Escape resolves to
//! [`Dialog::cancel_button_id`], falling back to
//! [`Dialog::default_button_id`], regardless of which button holds focus
//! — *except* while the focused field is an open `ComboBox`, where Escape
//! goes to the form first so it closes the popup instead of the whole
//! dialog.
//!
//! Field focus is the form's [`Form::focused_field`], which the host
//! owns: moving focus into the form reports
//! [`FormEvent::FocusChanged`] for the host to apply, the same event the
//! form's own Tab reports. The controller applies it to its stored form
//! itself, since a dialog is a self-contained modal; a consumer that
//! mirrors `focused_field` elsewhere still sees every change as a
//! [`FormDialogAction::FormAction`].
//!
//! # No visual keyboard-focus ring (known limitation)
//!
//! [`DialogButton`] carries no "keyboard-focused" field, so a button
//! holding keyboard focus renders no differently from an unfocused one
//! beyond `is_default`'s existing styling. While a button has focus, the
//! form paints without its focus highlight, so nothing on screen is
//! highlighted until focus returns to the form. Enter/Escape/click
//! routing is fully correct regardless.

use crate::a11y::{A11yChild, A11yInfo, A11yPrimitive};
use crate::backend::Backend;
use crate::backend::DrawsPrimitives;
use crate::compose::widget_host;
use crate::compose::Handled;
use crate::event::{Point, Rect, UiEvent};
use crate::interactive::{
    EventCx, FocusSpec, FocusStop, Interactive, KeyEvent, PointerKind, RoutedEvent, SubTarget,
    WidgetAction,
};
use crate::primitive::{LayoutCtx, Primitive, Size};
use crate::primitives::dialog::{
    Dialog, DialogButton, DialogHit, DialogInput, DialogLayout, DialogPrimitiveLayout,
    DialogSeverity,
};
use crate::primitives::form::contract as form_contract;
use crate::primitives::form::{
    focus_order, FieldKind, Form, FormEvent, FormPrimitiveLayout, FormState,
};
use crate::types::{StyledText, WidgetId};
use crate::{Key, Modifiers, NamedKey};

/// Semantic payload of [`Handled::Event`] returned by
/// [`FormDialogController::handle_event`], and the [`FormDialog`]
/// widget's action.
#[derive(Debug, Clone, PartialEq)]
pub enum FormDialogAction {
    /// A button was clicked, activated via Enter while it had keyboard
    /// focus, or resolved via Escape (which always targets
    /// [`Dialog::cancel_button_id`]/[`Dialog::default_button_id`]
    /// regardless of keyboard focus — see this module's "Focus model"
    /// doc section). The consumer should check `id` against the
    /// [`DialogButton::id`] values it built the dialog from — e.g. a
    /// click on the "Create" button is the submit signal; the app reads
    /// its own canonical field values (already kept in sync via
    /// `FormAction` below) rather than this event carrying them.
    ButtonClicked(WidgetId),
    /// A field-level event occurred inside the form (text typed, a
    /// toggle flipped, focus moved between fields, …). Apply it the same
    /// way a direct [`Form`] host would.
    FormAction(FormEvent),
}

impl WidgetAction for FormDialogAction {
    /// A button rides [`UiEvent::MessageDialogResolved`], the crate's one
    /// "an in-canvas dialog was answered" channel, under the button's own
    /// id. A field event rides [`UiEvent::Form`] under the dialog's id —
    /// the channel every form host already matches on.
    fn into_ui_event(self, id: WidgetId) -> UiEvent {
        match self {
            FormDialogAction::ButtonClicked(button) => UiEvent::MessageDialogResolved(button),
            FormDialogAction::FormAction(event) => UiEvent::Form(id, event),
        }
    }
}

/// What happened after [`FormDialogController::handle`] processed an event.
#[derive(Debug, Clone, PartialEq)]
#[deprecated(
    since = "0.9.0",
    note = "use `FormDialogController::handle_event`'s `Handled<FormDialogAction>` instead — `Consumed`/`Ignored` are `Handled::Consumed`/`Handled::Ignored`, `ButtonClicked`/`FormAction` move to `FormDialogAction` under `Handled::Event`"
)]
pub enum FormDialogEvent {
    /// A button was clicked, activated via Enter while it had keyboard
    /// focus, or resolved via Escape. See
    /// [`FormDialogAction::ButtonClicked`].
    ButtonClicked(WidgetId),
    /// A field-level event occurred inside the form. See
    /// [`FormDialogAction::FormAction`].
    FormAction(FormEvent),
    /// Event consumed — internal state (keyboard/button focus, scroll)
    /// changed, caller should redraw.
    Consumed,
    /// Event not relevant to this controller.
    Ignored,
}

// ── Widget ────────────────────────────────────────────────────────────────

/// A modal [`Dialog`] whose body is a multi-field [`Form`].
///
/// The modal centres itself in the rect it is laid out in, so a host
/// hands it the whole viewport. Field values and the focused field live
/// on [`Self::form`], owned by the host; [`FormDialogState`] holds the
/// form's editing position and which button has keyboard focus.
#[derive(Debug, Clone, PartialEq)]
pub struct FormDialog {
    /// The dialog's id, also the painted [`Dialog`]'s.
    pub id: WidgetId,
    /// Title row text.
    pub title: StyledText,
    /// Body text lines, painted above the form (instructions, or a status
    /// line).
    pub body: Vec<StyledText>,
    /// Severity tint.
    pub severity: Option<DialogSeverity>,
    /// Stack buttons vertically instead of the default right-aligned row.
    pub vertical_buttons: bool,
    /// The button row.
    pub buttons: Vec<DialogButton>,
    /// The form in the dialog's body, `None` for a dialog with no form
    /// yet.
    pub form: Option<Form>,
}

/// [`Interactive::State`] for [`FormDialog`]: the embedded form's own
/// state (caret, scrub position, item cursor, scroll) plus which button,
/// if any, holds keyboard focus.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FormDialogState {
    /// The embedded form's interaction state.
    pub form: FormState,
    /// `None` while keyboard focus is on the form; `Some(i)` while it is
    /// on [`FormDialog::buttons`]`[i]`.
    pub focused_button: Option<usize>,
}

/// [`FormDialog`]'s resolved layout: the modal's own layout and, when the
/// dialog has a form, the form's layout inside the dialog's form slot.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct FormDialogLayout {
    /// The modal chrome, body and button row.
    pub dialog: DialogPrimitiveLayout,
    /// The form, laid out in the dialog's `input_bounds`.
    pub form: Option<FormPrimitiveLayout>,
}

impl FormDialog {
    /// A form-less dialog titled `title`, under `id`.
    pub fn new(id: WidgetId, title: impl Into<String>) -> Self {
        Self {
            id,
            title: StyledText::plain(title.into()),
            body: Vec::new(),
            severity: None,
            vertical_buttons: false,
            buttons: Vec::new(),
            form: None,
        }
    }

    /// The form as painted: the descriptor overlaid with `state`'s caret,
    /// scrub position and scroll, and showing its focus highlight only
    /// while no button holds keyboard focus.
    pub fn live_form(&self, state: &FormDialogState) -> Option<Form> {
        let mut form = self.form.clone()?;
        form_contract::overlay_state(&mut form, &state.form);
        form.has_focus = form.has_focus && state.focused_button.is_none();
        Some(form)
    }

    /// The [`Dialog`] this widget paints its chrome through, carrying
    /// `form` in its form slot so the modal reserves the form's height.
    pub fn dialog_with(&self, form: Option<Form>) -> Dialog {
        Dialog {
            id: self.id,
            title: self.title.clone(),
            body: self.body.clone(),
            buttons: self.buttons.clone(),
            severity: self.severity,
            vertical_buttons: self.vertical_buttons,
            table: None,
            input: form.map(DialogInput::Form),
        }
    }

    /// [`Dialog::cancel_button_id`], falling back to
    /// [`Dialog::default_button_id`] — Escape's "there's always something
    /// to resolve to".
    fn cancel_or_default_button_id(&self) -> Option<WidgetId> {
        self.buttons
            .iter()
            .find(|b| b.is_cancel)
            .or_else(|| self.buttons.iter().find(|b| b.is_default))
            .or_else(|| self.buttons.last())
            .map(|b| b.id)
    }

    /// The form's focus stops, in Tab order.
    fn form_stops(&self) -> Vec<WidgetId> {
        self.form.as_ref().map(focus_order).unwrap_or_default()
    }

    /// Whether the focused field is an enabled, currently-open `ComboBox`
    /// — see this module's "Focus model" doc section for why Escape
    /// checks this before resolving to a button.
    fn form_has_open_combo_box(&self) -> bool {
        let Some(form) = &self.form else {
            return false;
        };
        let Some(focused_id) = form.focused_field else {
            return false;
        };
        form.fields
            .iter()
            .find(|f| f.id == focused_id)
            .is_some_and(|f| {
                !f.disabled && matches!(&f.kind, FieldKind::ComboBox(combo) if combo.open)
            })
    }

    /// Park keyboard focus on button `idx`.
    fn focus_button(&self, state: &mut FormDialogState, idx: usize, cx: &mut EventCx) {
        state.focused_button = Some(idx);
        let stop = self.form_stops().len() + idx;
        cx.request_focus(Some(SubTarget(stop as u64)));
    }

    /// Move focus from the buttons onto the form's first (`delta > 0`) or
    /// last (`delta < 0`) field, reporting it as the
    /// [`FormEvent::FocusChanged`] the host applies.
    fn enter_form(&self, state: &mut FormDialogState, delta: isize) -> Handled<FormDialogAction> {
        state.focused_button = None;
        let stops = self.form_stops();
        let target = if delta > 0 {
            stops.first()
        } else {
            stops.last()
        };
        match target {
            Some(id) => Handled::Event(FormDialogAction::FormAction(FormEvent::FocusChanged {
                id: *id,
            })),
            None => Handled::Consumed,
        }
    }

    /// Step keyboard focus `delta` stops along the one cyclic order this
    /// widget's [`Interactive::focus`] declares: the form's fields, then
    /// the buttons.
    fn step_focus(
        &self,
        state: &mut FormDialogState,
        delta: isize,
        key: &KeyEvent,
        layout: &FormDialogLayout,
        cx: &mut EventCx,
    ) -> Handled<FormDialogAction> {
        match state.focused_button {
            None => {
                let stops = self.form_stops();
                let current = self
                    .form
                    .as_ref()
                    .and_then(|f| f.focused_field)
                    .and_then(|id| stops.iter().position(|s| *s == id));
                let at_boundary = match current {
                    _ if stops.is_empty() => true,
                    Some(i) if delta > 0 => i + 1 >= stops.len(),
                    Some(0) if delta < 0 => true,
                    None if delta < 0 => true,
                    _ => false,
                };
                if !at_boundary {
                    return self.forward_to_form(state, &RoutedEvent::Key(key.clone()), layout, cx);
                }
                if self.buttons.is_empty() {
                    return Handled::Ignored;
                }
                let idx = if delta > 0 { 0 } else { self.buttons.len() - 1 };
                self.focus_button(state, idx, cx);
                Handled::Consumed
            }
            Some(i) => {
                let n = self.buttons.len();
                if n == 0 {
                    return Handled::Ignored;
                }
                if (delta > 0 && i + 1 >= n) || (delta < 0 && i == 0) {
                    return self.enter_form(state, delta);
                }
                self.focus_button(state, (i as isize + delta) as usize, cx);
                Handled::Consumed
            }
        }
    }

    /// Hand `event` to the embedded form's own [`Interactive::handle`],
    /// with a pointer position translated into the form's local space.
    fn forward_to_form(
        &self,
        state: &mut FormDialogState,
        event: &RoutedEvent,
        layout: &FormDialogLayout,
        cx: &mut EventCx,
    ) -> Handled<FormDialogAction> {
        let (Some(form), Some(form_layout)) = (&self.form, &layout.form) else {
            return Handled::Ignored;
        };
        let origin = layout.dialog.area;
        let to_form = |local: &Point| {
            Point::new(
                origin.x + local.x - form_layout.area.x,
                origin.y + local.y - form_layout.area.y,
            )
        };
        let event = match event {
            RoutedEvent::Pointer {
                kind,
                sub,
                local,
                mods,
            } => RoutedEvent::Pointer {
                kind: *kind,
                sub: *sub,
                local: to_form(local),
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
                local: to_form(local),
                mods: *mods,
            },
            other => other.clone(),
        };
        match Interactive::handle(form, &mut state.form, &event, form_layout, cx) {
            Handled::Event(e) => Handled::Event(FormDialogAction::FormAction(e)),
            Handled::Consumed => Handled::Consumed,
            Handled::Ignored => Handled::Ignored,
        }
    }

    /// [`Interactive::handle`]'s `Key` arm — see this module's "Focus
    /// model".
    fn handle_key(
        &self,
        state: &mut FormDialogState,
        key: &KeyEvent,
        layout: &FormDialogLayout,
        cx: &mut EventCx,
    ) -> Handled<FormDialogAction> {
        let Modifiers {
            ctrl,
            alt,
            cmd,
            shift,
            ..
        } = key.modifiers;
        let on_form = state.focused_button.is_none();
        match &key.key {
            Key::Named(NamedKey::Escape) if on_form && self.form_has_open_combo_box() => {
                self.forward_to_form(state, &RoutedEvent::Key(key.clone()), layout, cx)
            }
            Key::Named(NamedKey::Escape) => match self.cancel_or_default_button_id() {
                Some(id) => Handled::Event(FormDialogAction::ButtonClicked(id)),
                None => Handled::Ignored,
            },
            Key::Named(NamedKey::Enter) => match state.focused_button {
                Some(i) => match self.buttons.get(i) {
                    Some(b) => Handled::Event(FormDialogAction::ButtonClicked(b.id)),
                    None => Handled::Ignored,
                },
                None => self.forward_to_form(state, &RoutedEvent::Key(key.clone()), layout, cx),
            },
            Key::Named(NamedKey::Tab) if !ctrl && !alt && !cmd => {
                self.step_focus(state, if shift { -1 } else { 1 }, key, layout, cx)
            }
            Key::Named(NamedKey::BackTab) => self.step_focus(state, -1, key, layout, cx),
            Key::Named(NamedKey::Left | NamedKey::Right) if !on_form => {
                let delta = if matches!(key.key, Key::Named(NamedKey::Left)) {
                    -1
                } else {
                    1
                };
                let i = state.focused_button.unwrap_or(0);
                let n = self.buttons.len();
                if n == 0 {
                    return Handled::Ignored;
                }
                self.focus_button(state, crate::focus::cycle_index(i, delta, n), cx);
                Handled::Consumed
            }
            _ if on_form => self.forward_to_form(state, &RoutedEvent::Key(key.clone()), layout, cx),
            _ => Handled::Ignored,
        }
    }
}

impl A11yPrimitive for FormDialog {
    fn a11y_info(&self) -> A11yInfo {
        self.dialog_with(self.form.clone()).a11y_info()
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        self.dialog_with(self.form.clone()).a11y_children()
    }
}

impl Primitive for FormDialog {
    type Layout = FormDialogLayout;
    type Event = FormDialogAction;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn paint_bleed(&self, style: &crate::Style) -> f32 {
        Primitive::paint_bleed(&self.dialog_with(None), style)
    }

    fn measure(&self, ctx: &LayoutCtx, avail: Size) -> crate::NaturalSize {
        Primitive::measure(&self.dialog_with(self.form.clone()), ctx, avail)
    }

    /// The modal centres itself in `rect` through [`Dialog`]'s layout;
    /// the form is laid out through its own in the dialog's form slot.
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> FormDialogLayout {
        let dialog = Primitive::layout(&self.dialog_with(self.form.clone()), ctx, rect);
        let form = match (&self.form, dialog.layout.input_bounds) {
            (Some(form), Some(bounds)) => Some(Primitive::layout(form, ctx, bounds)),
            _ => None,
        };
        FormDialogLayout { dialog, form }
    }

    /// The modal through [`Dialog`]'s painter, then the form through
    /// [`Form`]'s, overlaid with the widget's own live state.
    fn paint(
        &self,
        layout: &FormDialogLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        let state = env.widget_state_as::<FormDialogState>();
        let default = FormDialogState::default();
        let live = self.live_form(state.unwrap_or(&default));
        Primitive::paint(
            &self.dialog_with(live.clone()),
            &layout.dialog,
            surface,
            env,
        );
        if let (Some(form), Some(form_layout)) = (&live, &layout.form) {
            Primitive::paint(form, form_layout, surface, env);
        }
    }

    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &FormDialogLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        let state = env.widget_state_as::<FormDialogState>();
        let default = FormDialogState::default();
        let live = self.live_form(state.unwrap_or(&default));
        Primitive::paint_cells(&self.dialog_with(live.clone()), &layout.dialog, cells, env);
        if let (Some(form), Some(form_layout)) = (&live, &layout.form) {
            Primitive::paint_cells(form, form_layout, cells, env);
        }
    }
}

impl Interactive for FormDialog {
    type State = FormDialogState;
    type Action = FormDialogAction;

    /// The form's own stops, then one stop per button — see this
    /// module's "Focus model".
    fn focus(&self) -> FocusSpec {
        let n = self.form_stops().len() + self.buttons.len();
        if n == 0 {
            FocusSpec::None
        } else {
            FocusSpec::Children(
                (0..n)
                    .map(|i| FocusStop {
                        sub: SubTarget(i as u64),
                    })
                    .collect(),
            )
        }
    }

    /// Every button, addressed by its focus stop.
    fn sub_regions(&self, layout: &FormDialogLayout) -> Vec<(Rect, SubTarget)> {
        let offset = self.form_stops().len() as u64;
        Interactive::sub_regions(&self.dialog_with(None), &layout.dialog)
            .into_iter()
            .map(|(r, sub)| (r, SubTarget(sub.0 + offset)))
            .collect()
    }

    fn handle(
        &self,
        state: &mut FormDialogState,
        event: &RoutedEvent,
        layout: &FormDialogLayout,
        cx: &mut EventCx,
    ) -> Handled<FormDialogAction> {
        match event {
            RoutedEvent::Key(key) => self.handle_key(state, key, layout, cx),
            RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::MouseButton::Left),
                local,
                ..
            } => {
                let area = layout.dialog.area;
                let (x, y) = (area.x + local.x, area.y + local.y);
                match layout.dialog.hit_test(x, y) {
                    DialogHit::Button(id) => {
                        if let Some(i) = self.buttons.iter().position(|b| b.id == id) {
                            self.focus_button(state, i, cx);
                        }
                        Handled::Event(FormDialogAction::ButtonClicked(id))
                    }
                    DialogHit::Outside => Handled::Ignored,
                    _ => {
                        let on_form = layout
                            .form
                            .as_ref()
                            .is_some_and(|f| f.area.contains(Point::new(x, y)));
                        if !on_form {
                            return Handled::Consumed;
                        }
                        state.focused_button = None;
                        match self.forward_to_form(state, event, layout, cx) {
                            Handled::Ignored => Handled::Consumed,
                            other => other,
                        }
                    }
                }
            }
            RoutedEvent::Pointer { .. } | RoutedEvent::Wheel { .. } | RoutedEvent::Text(_)
                if state.focused_button.is_none() =>
            {
                self.forward_to_form(state, event, layout, cx)
            }
            RoutedEvent::FocusIn(Some(sub)) => {
                let stops = self.form_stops();
                let idx = sub.0 as usize;
                if let Some(id) = stops.get(idx) {
                    state.focused_button = None;
                    Handled::Event(FormDialogAction::FormAction(FormEvent::FocusChanged {
                        id: *id,
                    }))
                } else if idx - stops.len() < self.buttons.len() {
                    state.focused_button = Some(idx - stops.len());
                    Handled::Consumed
                } else {
                    Handled::Ignored
                }
            }
            _ => Handled::Ignored,
        }
    }
}

// ── Controller ────────────────────────────────────────────────────────────

/// Event-driven wrapper over a [`FormDialog`] and its state, for a host
/// that calls it with raw [`UiEvent`]s rather than routing it through the
/// runtime's engine.
///
/// See the [module-level documentation](self) for a usage example.
pub struct FormDialogController {
    widget: FormDialog,
    state: FormDialogState,
}

impl FormDialogController {
    /// Build an empty controller — no form, no buttons. Call
    /// [`Self::set_form`] and [`Self::with_buttons`] before using it.
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            widget: FormDialog::new(WidgetId::new(id.into()), title),
            state: FormDialogState::default(),
        }
    }

    /// Set the dialog's buttons. Returns `self` for builder-style
    /// chaining.
    pub fn with_buttons(mut self, buttons: Vec<DialogButton>) -> Self {
        self.widget.buttons = buttons;
        self
    }

    /// Set the dialog's severity tint. Returns `self` for builder-style
    /// chaining.
    pub fn with_severity(mut self, severity: DialogSeverity) -> Self {
        self.widget.severity = Some(severity);
        self
    }

    /// Stack buttons vertically instead of the default right-aligned
    /// horizontal row. See [`Dialog::vertical_buttons`]. Returns `self`
    /// for builder-style chaining.
    pub fn with_vertical_buttons(mut self, vertical: bool) -> Self {
        self.widget.vertical_buttons = vertical;
        self
    }

    /// Set body text lines, rendered above the form (e.g. instructions,
    /// or a status line a consumer updates after a submit). See
    /// [`Dialog::body`]'s own "title → body → table → input → buttons"
    /// layout order. Returns `self` for builder-style chaining.
    pub fn with_body(mut self, body: Vec<StyledText>) -> Self {
        self.widget.body = body;
        self
    }

    /// Replace the body text lines set via [`Self::with_body`] — unlike
    /// that builder, callable after construction, for a status line a
    /// consumer wants to update in response to its own action handling
    /// (e.g. "Created: …" after a submit).
    pub fn set_body(&mut self, body: Vec<StyledText>) {
        self.widget.body = body;
    }

    /// Push a fresh `form` into the controller — call every frame with
    /// the app's own canonical field values.
    pub fn set_form(&mut self, form: Form) {
        self.widget.form = Some(form);
    }

    /// The form most recently pushed via [`Self::set_form`], with every
    /// focus change this controller applied since.
    pub fn form(&self) -> Option<&Form> {
        self.widget.form.as_ref()
    }

    /// The [`FormDialog`] widget this controller drives.
    pub fn widget(&self) -> &FormDialog {
        &self.widget
    }

    /// Whether keyboard focus is currently on the form (as opposed to
    /// one of the buttons).
    pub fn is_form_focused(&self) -> bool {
        self.state.focused_button.is_none()
    }

    /// The `WidgetId` of the currently keyboard-focused button, or
    /// `None` when focus is on the form.
    pub fn focused_button_id(&self) -> Option<&WidgetId> {
        self.state
            .focused_button
            .and_then(|i| self.widget.buttons.get(i))
            .map(|b| &b.id)
    }

    /// The underlying [`Dialog`] descriptor, built fresh from this
    /// controller's current state. Its form slot carries the form as it
    /// is painted — in particular `has_focus`, which reports whether
    /// keyboard focus is really on the form or on a button.
    pub fn dialog(&self) -> Dialog {
        self.widget.dialog_with(self.widget.live_form(&self.state))
    }

    /// The dialog's full on-screen bounds for this backend's current
    /// viewport — see [`crate::compose::MessageDialogController::bounds`]'s
    /// own doc for why a caller tracking a [`crate::ModalStack`] entry
    /// wants this.
    pub fn bounds(&self, backend: &dyn Backend) -> Rect {
        self.layout(backend).bounds
    }

    /// The whole viewport, which the modal centres itself in.
    fn viewport(backend: &dyn Backend) -> Rect {
        let vp = backend.viewport();
        Rect::new(0.0, 0.0, vp.width, vp.height)
    }

    /// The widget as painted: its form overlaid with this controller's
    /// live state, so a `Surface::Widget` paint and this one agree.
    fn painted_widget(&self) -> FormDialog {
        FormDialog {
            form: self.widget.live_form(&self.state),
            ..self.widget.clone()
        }
    }

    /// Resolve the modal's geometry through the widget contract, from the
    /// backend's own viewport and font metrics, so paint and hit-test can
    /// never disagree.
    fn layout(&self, backend: &dyn Backend) -> DialogLayout {
        backend
            .layout_primitive(Self::viewport(backend), &self.painted_widget())
            .dialog
            .layout
    }

    /// Paint the dialog, centered in the backend's current viewport.
    ///
    /// Call this from `AppLogic::render` whenever the dialog is open.
    pub fn render(&self, backend: &mut dyn Backend) {
        let viewport = Self::viewport(backend);
        backend.draw_dyn(viewport, &self.painted_widget(), &());
    }

    /// Drive the dialog with a backend-neutral [`UiEvent`].
    ///
    /// A press outside the modal is swallowed: the dialog is modal.
    pub fn handle_event(
        &mut self,
        event: &UiEvent,
        backend: &mut dyn Backend,
    ) -> Handled<FormDialogAction> {
        let viewport = Self::viewport(backend);
        let layout = backend.layout_primitive(viewport, &self.painted_widget());
        let subs = Interactive::sub_regions(&self.widget, &layout);
        let Some(routed) = widget_host::routed_event(event, viewport, &subs) else {
            return Handled::Ignored;
        };
        let units = crate::backend::primitive_layout_ctx(backend).units();
        let driven = widget_host::drive(
            &self.widget,
            self.widget.id,
            &mut self.state,
            &routed,
            &layout,
            units,
        );
        let handled = match (driven.handled, &routed) {
            (
                Handled::Ignored,
                RoutedEvent::Pointer {
                    kind: PointerKind::Down(_),
                    ..
                },
            ) => Handled::Consumed,
            (handled, _) => handled,
        };
        if let Handled::Event(FormDialogAction::FormAction(FormEvent::FocusChanged { id })) =
            &handled
        {
            if let Some(form) = &mut self.widget.form {
                form.focused_field = Some(*id);
            }
        }
        handled
    }

    /// Drive the state machine with a backend-neutral [`UiEvent`].
    #[deprecated(
        since = "0.9.0",
        note = "use `handle_event`, which returns `Handled<FormDialogAction>` instead of `FormDialogEvent`"
    )]
    #[allow(deprecated)] // `FormDialogEvent` is `#[deprecated]`; this method is its own shim.
    pub fn handle(&mut self, event: &UiEvent, backend: &mut dyn Backend) -> FormDialogEvent {
        match self.handle_event(event, backend) {
            Handled::Event(FormDialogAction::ButtonClicked(id)) => {
                FormDialogEvent::ButtonClicked(id)
            }
            Handled::Event(FormDialogAction::FormAction(e)) => FormDialogEvent::FormAction(e),
            Handled::Consumed => FormDialogEvent::Consumed,
            Handled::Ignored => FormDialogEvent::Ignored,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// Gated on `tui` for the same reason `compose::message_dialog`'s own test
// module is: `layout()`/`render()`/`handle()` all need a concrete
// `Backend` to exercise `backend.measure()`/`backend.viewport()` calls,
// and `TuiBackend` is the cheapest one in-tree.
#[cfg(all(test, feature = "tui"))]
mod tests {
    use super::*;
    use crate::primitives::form::FormField;
    use crate::MouseButton;

    fn name_description_form() -> Form {
        Form {
            id: WidgetId::new("form:create_milestone"),
            fields: vec![
                FormField {
                    id: WidgetId::new("name"),
                    label: StyledText::plain("Name"),
                    kind: FieldKind::TextInput {
                        value: String::new(),
                        placeholder: "Milestone name".into(),
                        cursor: None,
                        selection_anchor: None,
                    },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                    label_position: None,
                    label_align: None,
                    label_width: None,
                    tab_index: None,
                    control_width: None,
                    min_control_width: None,
                    row_align: None,
                },
                FormField {
                    id: WidgetId::new("description"),
                    label: StyledText::plain("Description"),
                    kind: FieldKind::TextArea {
                        value: String::new(),
                        placeholder: "Optional".into(),
                        cursor: None,
                        visible_rows: 3,
                    },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                    label_position: None,
                    label_align: None,
                    label_width: None,
                    tab_index: None,
                    control_width: None,
                    min_control_width: None,
                    row_align: None,
                },
            ],
            focused_field: Some(WidgetId::new("name")),
            scroll_offset: 0,
            has_focus: true,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        }
    }

    fn controller() -> FormDialogController {
        let mut c =
            FormDialogController::new("create_milestone", "Create milestone").with_buttons(vec![
                DialogButton {
                    id: WidgetId::new("cancel"),
                    label: "Cancel".into(),
                    is_default: false,
                    is_cancel: true,
                    tint: None,
                    spec: None,
                },
                DialogButton {
                    id: WidgetId::new("create"),
                    label: "Create".into(),
                    is_default: true,
                    is_cancel: false,
                    tint: None,
                    spec: None,
                },
            ]);
        c.set_form(name_description_form());
        c
    }

    fn tab_event(shift: bool) -> UiEvent {
        UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Tab),
            modifiers: Modifiers {
                shift,
                ..Modifiers::default()
            },
            repeat: false,
        }
    }

    #[test]
    fn dialog_input_is_a_form_with_both_fields() {
        let c = controller();
        match c.dialog().input {
            Some(DialogInput::Form(f)) => assert_eq!(f.fields.len(), 2),
            other => panic!("expected DialogInput::Form, got {:?}", other),
        }
    }

    /// `dialog()`'s embedded `Form` reports `has_focus` matching this
    /// controller's *actual* keyboard-focus state, not whatever
    /// `has_focus` the consumer's own pushed descriptor carries (the
    /// shipped demo always pushes `true`, since it isn't the one
    /// tracking button-vs-form focus) — otherwise a screen reader (via
    /// `a11y::chrome`) or a future rasteriser painting straight off
    /// `dialog()` would report the form focused even once focus has
    /// moved to a button.
    #[test]
    fn dialog_form_has_focus_reflects_actual_keyboard_focus() {
        let mut c = controller();
        match c.dialog().input {
            Some(DialogInput::Form(f)) => assert!(f.has_focus, "form should start focused"),
            other => panic!("expected DialogInput::Form, got {:?}", other),
        }
        let mut backend = crate::tui::backend::TuiBackend::new();
        let _ = c.handle_event(&tab_event(false), &mut backend); // name -> description
        let _ = c.handle_event(&tab_event(false), &mut backend); // description -> cancel
        assert!(!c.is_form_focused());
        match c.dialog().input {
            Some(DialogInput::Form(f)) => {
                assert!(
                    !f.has_focus,
                    "form should report unfocused once a button has focus"
                )
            }
            other => panic!("expected DialogInput::Form, got {:?}", other),
        }
    }

    /// Escape forwards into the form instead of resolving to a button
    /// while the focused field is an open `ComboBox` — the popup should
    /// close without dismissing the whole dialog.
    #[test]
    fn escape_closes_an_open_combo_box_instead_of_resolving_to_cancel() {
        use crate::primitives::combo_box::ComboBox;
        let mut c = controller();
        let mut form = c.form().unwrap().clone();
        form.fields.push(FormField {
            id: WidgetId::new("kind"),
            label: StyledText::plain("Kind"),
            kind: FieldKind::ComboBox(ComboBox {
                id: WidgetId::new("kind-combo"),
                options: vec!["Alpha".into(), "Beta".into()],
                text: "Al".into(),
                committed: String::new(),
                open: true,
                highlighted: Some(0),
                disabled: false,
                focused: true,
            }),
            hint: StyledText::default(),
            disabled: false,
            validation: None,
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width: None,
            min_control_width: None,
            row_align: None,
        });
        form.focused_field = Some(WidgetId::new("kind"));
        c.set_form(form);
        let mut backend = crate::tui::backend::TuiBackend::new();
        let esc = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let result = c.handle_event(&esc, &mut backend);
        // The combo box's own Escape path cancels the edit back to the
        // committed value and closes the popup — not a `ButtonClicked`.
        assert!(
            matches!(result, Handled::Event(FormDialogAction::FormAction(_))),
            "Escape should have been forwarded into the open combo box, got {:?}",
            result
        );
        assert!(c.is_form_focused(), "dialog should still be open");
    }

    /// `set_body` updates the rendered `Dialog::body` after construction
    /// — a consumer uses this for a status line (e.g. "Created: …")
    /// shown after a submit, without rebuilding the whole controller.
    #[test]
    fn set_body_updates_the_dialogs_body_text() {
        let mut c = controller();
        assert!(c.dialog().body.is_empty());
        c.set_body(vec![StyledText::plain("Created: Alpha")]);
        assert_eq!(c.dialog().body.len(), 1);
    }

    #[test]
    fn starts_with_form_focused() {
        let c = controller();
        assert!(c.is_form_focused());
        assert_eq!(c.focused_button_id(), None);
    }

    #[test]
    fn tab_moves_from_name_to_description_field() {
        let mut c = controller();
        let mut backend = crate::tui::backend::TuiBackend::new();
        let ev = tab_event(false);
        let result = c.handle_event(&ev, &mut backend);
        assert!(matches!(
            result,
            Handled::Event(FormDialogAction::FormAction(FormEvent::FocusChanged { .. }))
        ));
        assert_eq!(
            c.form().and_then(|f| f.focused_field).unwrap().as_str(),
            "description"
        );
        assert!(c.is_form_focused());
    }

    #[test]
    fn tab_past_the_last_field_moves_to_the_first_button() {
        let mut c = controller();
        let mut backend = crate::tui::backend::TuiBackend::new();
        // name -> description -> (past the last field) first button.
        let _ = c.handle_event(&tab_event(false), &mut backend);
        let result = c.handle_event(&tab_event(false), &mut backend);
        assert_eq!(result, Handled::Consumed);
        assert!(!c.is_form_focused());
        assert_eq!(c.focused_button_id().unwrap().as_str(), "cancel");
    }

    #[test]
    fn tab_past_the_last_button_wraps_to_the_first_field() {
        let mut c = controller();
        let mut backend = crate::tui::backend::TuiBackend::new();
        let _ = c.handle_event(&tab_event(false), &mut backend); // name -> description
        let _ = c.handle_event(&tab_event(false), &mut backend); // description -> cancel
        let _ = c.handle_event(&tab_event(false), &mut backend); // cancel -> create
        let result = c.handle_event(&tab_event(false), &mut backend); // create -> name
                                                                      // `enter_form` surfaces `FocusChanged` (not a bare `Consumed`)
                                                                      // so a consumer mirroring `focused_field` onto its own
                                                                      // canonical `Form` (the documented `set_form`-every-frame
                                                                      // contract) learns about the hand-off too — see this module's
                                                                      // "Keyboard model" doc section.
        assert_eq!(
            result,
            Handled::Event(FormDialogAction::FormAction(FormEvent::FocusChanged {
                id: WidgetId::new("name")
            }))
        );
        assert!(c.is_form_focused());
        assert_eq!(
            c.form().and_then(|f| f.focused_field).unwrap().as_str(),
            "name"
        );
    }

    /// Typing immediately after a Tab-wrap back onto the form (the
    /// specific regression this module's "Keyboard model" doc section
    /// now documents) must land in the field `enter_form` actually
    /// focused, not wherever a caller's stale `focused_field` mirror
    /// still points — i.e. the mirror itself must have been updated via
    /// the `FocusChanged` [`tab_past_the_last_button_wraps_to_the_first_field`]
    /// asserts above, which this test exercises end-to-end.
    #[test]
    fn typing_after_tab_wrap_back_onto_the_form_lands_in_the_focused_field() {
        let mut c = controller();
        let mut backend = crate::tui::backend::TuiBackend::new();
        let _ = c.handle_event(&tab_event(false), &mut backend); // name -> description
        let _ = c.handle_event(&tab_event(false), &mut backend); // description -> cancel
        let _ = c.handle_event(&tab_event(false), &mut backend); // cancel -> create
        let _ = c.handle_event(&tab_event(false), &mut backend); // create -> (wraps) name
        let result = c.handle_event(&UiEvent::CharTyped('z'), &mut backend);
        assert_eq!(
            result,
            Handled::Event(FormDialogAction::FormAction(FormEvent::TextInputChanged {
                id: WidgetId::new("name"),
                value: "z".into(),
            }))
        );
    }

    #[test]
    fn shift_tab_from_the_first_field_moves_to_the_last_button() {
        let mut c = controller();
        let mut backend = crate::tui::backend::TuiBackend::new();
        let result = c.handle_event(&tab_event(true), &mut backend);
        assert_eq!(result, Handled::Consumed);
        assert!(!c.is_form_focused());
        assert_eq!(c.focused_button_id().unwrap().as_str(), "create");
    }

    #[test]
    fn left_right_cycle_buttons_once_focused() {
        let mut c = controller();
        let mut backend = crate::tui::backend::TuiBackend::new();
        let _ = c.handle_event(&tab_event(false), &mut backend);
        let _ = c.handle_event(&tab_event(false), &mut backend); // now on "cancel"
        let right = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Right),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let _ = c.handle_event(&right, &mut backend);
        assert_eq!(c.focused_button_id().unwrap().as_str(), "create");
    }

    #[test]
    fn typing_while_name_focused_updates_the_form_value() {
        let mut c = controller();
        let mut backend = crate::tui::backend::TuiBackend::new();
        let ev = UiEvent::CharTyped('x');
        let result = c.handle_event(&ev, &mut backend);
        match result {
            Handled::Event(FormDialogAction::FormAction(FormEvent::TextInputChanged {
                id,
                value,
            })) => {
                assert_eq!(id.as_str(), "name");
                assert_eq!(value, "x");
            }
            other => panic!("expected TextInputChanged, got {:?}", other),
        }
    }

    #[test]
    fn enter_on_focused_create_button_resolves_it() {
        let mut c = controller();
        let mut backend = crate::tui::backend::TuiBackend::new();
        let _ = c.handle_event(&tab_event(true), &mut backend); // shift-tab: focus -> "create"
        let enter = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let result = c.handle_event(&enter, &mut backend);
        assert_eq!(
            result,
            Handled::Event(FormDialogAction::ButtonClicked(WidgetId::new("create")))
        );
    }

    #[test]
    fn escape_resolves_the_cancel_button_regardless_of_focus() {
        let mut c = controller();
        let mut backend = crate::tui::backend::TuiBackend::new();
        let esc = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let result = c.handle_event(&esc, &mut backend);
        assert_eq!(
            result,
            Handled::Event(FormDialogAction::ButtonClicked(WidgetId::new("cancel")))
        );
    }

    #[test]
    fn click_on_a_button_focuses_and_resolves_it() {
        let c = controller();
        let backend = crate::tui::backend::TuiBackend::new();
        let layout = c.layout(&backend);
        let btn = layout
            .visible_buttons
            .iter()
            .find(|b| b.id.as_str() == "create")
            .unwrap();
        let cx = btn.bounds.x + 1.0;
        let cy = btn.bounds.y;
        let mut c = c;
        let mut backend = backend;
        let ev = UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(cx, cy),
            modifiers: Modifiers::default(),
        };
        let result = c.handle_event(&ev, &mut backend);
        assert_eq!(
            result,
            Handled::Event(FormDialogAction::ButtonClicked(WidgetId::new("create")))
        );
        assert_eq!(c.focused_button_id().unwrap().as_str(), "create");
    }

    #[test]
    fn bounds_matches_the_layout_render_and_handle_both_use() {
        let c = controller();
        let backend = crate::tui::backend::TuiBackend::new();
        assert_eq!(c.bounds(&backend), c.layout(&backend).bounds);
    }

    /// The input slot reserved for a `TextArea { visible_rows: 3 }`
    /// field must be at least 3 rows tall (plus the Name field's own
    /// row) — a flat one-row-per-field estimate would clip it to a
    /// single row and drop two-thirds of what the user typed off
    /// screen. `name_description_form`'s Description field is exactly
    /// this shape.
    #[test]
    fn layout_reserves_the_text_areas_full_visible_rows_not_one_row_per_field() {
        let c = controller();
        let backend = crate::tui::backend::TuiBackend::new();
        let input_bounds = c
            .layout(&backend)
            .input_bounds
            .expect("Form input should reserve a slot");
        // Name's single row + Description's 3 `visible_rows` == 4,
        // against the TUI backend's 1.0 line height.
        assert!(
            input_bounds.height >= 4.0,
            "input slot should be tall enough for a 1-row field + a \
             3-row TextArea, got height {}",
            input_bounds.height
        );
    }

    #[test]
    fn other_events_with_buttons_focused_are_ignored() {
        let mut c = controller();
        let mut backend = crate::tui::backend::TuiBackend::new();
        let _ = c.handle_event(&tab_event(true), &mut backend); // focus -> "create"
        let ev = UiEvent::CharTyped('x');
        assert_eq!(c.handle_event(&ev, &mut backend), Handled::Ignored);
    }

    /// `focus_order` skips `Label`/`ReadOnly`/disabled fields, matching
    /// `FormController`'s own private traversal order.
    #[test]
    fn focus_order_skips_non_focusable_fields() {
        let mut form = name_description_form();
        form.fields.insert(
            0,
            FormField {
                id: WidgetId::new("header"),
                label: StyledText::plain("Section"),
                kind: FieldKind::Label,
                hint: StyledText::default(),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            },
        );
        let order = focus_order(&form);
        assert_eq!(
            order,
            vec![WidgetId::new("name"), WidgetId::new("description")]
        );
    }
}
