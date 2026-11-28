//! [`FormDialog`] — a [`Dialog`] hosting a multi-field [`Form`] on the
//! widget contract.
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
//! # Hosting
//!
//! An app on the runtime's routing engine pushes the [`FormDialog`]
//! itself as a `Surface::Widget` over the whole viewport and registers it
//! in its frame tree; its actions arrive as [`UiEvent::MessageDialogResolved`]
//! (a button) and [`UiEvent::Form`] (a field).
//!
//! A host that reads raw [`UiEvent`]s instead keeps the widget and its
//! [`FormDialogState`], paints [`FormDialog::painted`] with `draw_dyn` over
//! the viewport and resolves each event through [`FormDialog::handle_event`]:
//!
//! ```rust,ignore
//! let mut dialog = FormDialog::new(WidgetId::new("create_milestone"), "Create milestone")
//!     .with_buttons(vec![
//!         DialogButton { id: WidgetId::new("cancel"), label: "Cancel".into(), is_cancel: true, is_default: false, tint: None, spec: None },
//!         DialogButton { id: WidgetId::new("create"), label: "Create".into(), is_default: true, is_cancel: false, tint: None, spec: None },
//!     ]);
//! let mut state = FormDialogState::default();
//! dialog.form = Some(my_form); // rebuilt every frame from app-owned field values
//!
//! // AppLogic::render:
//! backend.draw_dyn(backend.viewport().rect(), &dialog.painted(&state), &());
//!
//! // AppLogic::handle:
//! match dialog.handle_event(&mut state, &event, backend) {
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
//! form's own Tab reports. [`FormDialog::handle_event`] applies it to the
//! widget's form itself, since a dialog is a self-contained modal; a consumer that
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
use crate::primitives::form::{
    focus_order, FieldKind, Form, FormEvent, FormState, ScrolledForm, ScrolledFormLayout,
};
use crate::types::{StyledText, WidgetId};
use crate::{Key, Modifiers, NamedKey};

/// The [`FormDialog`] widget's action: the semantic payload of
/// [`Handled::Event`] that [`FormDialog::handle_event`] returns.
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
/// state (caret, scrub position, item cursor, scroll, scrollbar drag)
/// plus which button, if any, holds keyboard focus.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq)]
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
    /// The form and the scrollbar its fields need, laid out as a
    /// [`ScrolledForm`] in the dialog's `input_bounds`.
    pub form: Option<ScrolledFormLayout>,
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
    /// scrub position, scroll and field focus, and showing its focus
    /// highlight only while no button holds keyboard focus.
    pub fn live_form(&self, state: &FormDialogState) -> Option<Form> {
        let mut form = self
            .form
            .as_ref()?
            .live(Some(&state.form), false)
            .into_owned();
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

    /// Set the dialog's buttons. Returns `self` for builder-style
    /// chaining.
    pub fn with_buttons(mut self, buttons: Vec<DialogButton>) -> Self {
        self.buttons = buttons;
        self
    }

    /// Set the dialog's severity tint. Returns `self` for builder-style
    /// chaining.
    pub fn with_severity(mut self, severity: DialogSeverity) -> Self {
        self.severity = Some(severity);
        self
    }

    /// Stack buttons vertically instead of the default right-aligned
    /// horizontal row. See [`Dialog::vertical_buttons`]. Returns `self`
    /// for builder-style chaining.
    pub fn with_vertical_buttons(mut self, vertical: bool) -> Self {
        self.vertical_buttons = vertical;
        self
    }

    /// Set body text lines, painted above the form (e.g. instructions, or
    /// a status line a host updates after a submit). See [`Dialog::body`]'s
    /// own "title → body → table → input → buttons" layout order. Returns
    /// `self` for builder-style chaining.
    pub fn with_body(mut self, body: Vec<StyledText>) -> Self {
        self.body = body;
        self
    }

    /// The `WidgetId` of the button holding keyboard focus in `state`, or
    /// `None` while focus is on the form.
    pub fn focused_button_id(&self, state: &FormDialogState) -> Option<&WidgetId> {
        state
            .focused_button
            .and_then(|i| self.buttons.get(i))
            .map(|b| &b.id)
    }

    /// The widget as a raw-event host paints it: its form overlaid with
    /// `state` ([`Self::live_form`]), so a `draw_dyn` of it outside the
    /// runtime's widget-state store shows the live caret and focus.
    pub fn painted(&self, state: &FormDialogState) -> FormDialog {
        FormDialog {
            form: self.live_form(state),
            ..self.clone()
        }
    }

    /// The modal's geometry over `backend`'s viewport for the widget as
    /// [`Self::painted`] with `state` — the geometry a host's `draw_dyn`
    /// of it paints, resolved from the backend's own font metrics.
    pub fn dialog_layout(&self, state: &FormDialogState, backend: &dyn Backend) -> DialogLayout {
        backend
            .layout_primitive(backend.viewport().rect(), &self.painted(state))
            .dialog
            .layout
    }

    /// The dialog's full on-screen bounds over `backend`'s viewport — see
    /// [`crate::compose::MessageDialog::bounds`] for why a host tracking a
    /// [`crate::ModalStack`] entry wants this.
    pub fn bounds(&self, state: &FormDialogState, backend: &dyn Backend) -> Rect {
        self.dialog_layout(state, backend).bounds
    }

    /// Resolve one raw [`UiEvent`] for a host that paints
    /// [`Self::painted`] over `backend`'s viewport, through the widget's
    /// own [`Interactive`] against `state`.
    ///
    /// A press outside the modal is [`Handled::Consumed`]: the dialog is
    /// modal. A reported [`FormEvent::FocusChanged`] is applied to
    /// [`Self::form`] here as well as returned, since the dialog is a
    /// self-contained modal.
    pub fn handle_event(
        &mut self,
        state: &mut FormDialogState,
        event: &UiEvent,
        backend: &dyn Backend,
    ) -> Handled<FormDialogAction> {
        let viewport = backend.viewport().rect();
        let layout = backend.layout_primitive(viewport, &self.painted(state));
        let subs = Interactive::sub_regions(self, &layout);
        let Some(routed) = widget_host::routed_event(event, viewport, &subs) else {
            return Handled::Ignored;
        };
        let units = crate::backend::primitive_layout_ctx(backend).units();
        let driven = widget_host::drive(&*self, self.id, state, &routed, &layout, units);
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
            if let Some(form) = &mut self.form {
                form.focused_field = Some(*id);
            }
        }
        handled
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

    /// Park keyboard focus on the form field `id`'s stop.
    fn focus_field(&self, id: WidgetId, cx: &mut EventCx) {
        if let Some(stop) = self.form_stops().iter().position(|s| *s == id) {
            cx.request_focus(Some(SubTarget(stop as u64)));
        }
    }

    /// Move focus from the buttons onto the form's first (`delta > 0`) or
    /// last (`delta < 0`) field, reporting it as the
    /// [`FormEvent::FocusChanged`] the host applies.
    fn enter_form(
        &self,
        state: &mut FormDialogState,
        delta: isize,
        cx: &mut EventCx,
    ) -> Handled<FormDialogAction> {
        state.focused_button = None;
        let stops = self.form_stops();
        let target = if delta > 0 {
            stops.first()
        } else {
            stops.last()
        };
        match target {
            Some(id) => {
                state.form.focused_field = Some(*id);
                self.focus_field(*id, cx);
                Handled::Event(FormDialogAction::FormAction(FormEvent::FocusChanged {
                    id: *id,
                }))
            }
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
                    .live_form(state)
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
                    return self.enter_form(state, delta, cx);
                }
                self.focus_button(state, (i as isize + delta) as usize, cx);
                Handled::Consumed
            }
        }
    }

    /// Hand `event` to the embedded form — its scrollbar, then the form's
    /// own [`Interactive::handle`] — with a pointer position translated
    /// into the form's local space.
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
        let delta = Point::new(form_layout.area.x - origin.x, form_layout.area.y - origin.y);
        let sub = match event {
            RoutedEvent::Pointer { sub, .. } => *sub,
            _ => None,
        };
        let event = widget_host::child_event(event, delta, sub);
        match ScrolledForm::handle_live(form, &mut state.form, &event, form_layout, cx) {
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
    /// the form, at the live state stored for this dialog, is laid out
    /// as a [`ScrolledForm`] in the dialog's form slot.
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> FormDialogLayout {
        let dialog = Primitive::layout(&self.dialog_with(self.form.clone()), ctx, rect);
        let stored = crate::widget_state::with_widget_state_ref(Some(&self.id), |state| {
            state
                .and_then(|s| s.downcast_ref::<FormDialogState>())
                .cloned()
        });
        let state = stored.unwrap_or_default();
        let form = match (self.live_form(&state), dialog.layout.input_bounds) {
            (Some(form), Some(bounds)) => Some(ScrolledForm::layout_live(
                &form,
                state.form.thumb_drag.is_some(),
                ctx,
                bounds,
            )),
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
            let state = state.unwrap_or(&default);
            ScrolledForm::paint_live(form, &state.form, form_layout, surface, env);
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
            let state = state.unwrap_or(&default);
            ScrolledForm::paint_cells_live(form, &state.form, form_layout, cells, env);
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
                        let handled = self.forward_to_form(state, event, layout, cx);
                        // The press lands focus on the clicked field's own
                        // stop, which is how the host addresses it.
                        let focused = match &handled {
                            Handled::Event(FormDialogAction::FormAction(
                                FormEvent::FocusChanged { id },
                            )) => Some(*id),
                            _ => self.form.as_ref().and_then(|f| f.focused_field),
                        };
                        if let Some(id) = focused {
                            self.focus_field(id, cx);
                        }
                        match handled {
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

    /// A raw-event host: the dialog plus the state it keeps between
    /// events, driven through [`FormDialog::handle_event`].
    struct Host {
        widget: FormDialog,
        state: FormDialogState,
    }

    impl Host {
        fn form(&self) -> Option<&Form> {
            self.widget.form.as_ref()
        }

        fn set_form(&mut self, form: Form) {
            self.widget.form = Some(form);
        }

        fn set_body(&mut self, body: Vec<StyledText>) {
            self.widget.body = body;
        }

        fn dialog(&self) -> Dialog {
            self.widget.dialog_with(self.widget.live_form(&self.state))
        }

        fn is_form_focused(&self) -> bool {
            self.state.focused_button.is_none()
        }

        fn focused_button_id(&self) -> Option<&WidgetId> {
            self.widget.focused_button_id(&self.state)
        }

        fn layout(&self, backend: &dyn Backend) -> DialogLayout {
            self.widget.dialog_layout(&self.state, backend)
        }

        fn bounds(&self, backend: &dyn Backend) -> Rect {
            self.widget.bounds(&self.state, backend)
        }

        fn handle_event(
            &mut self,
            event: &UiEvent,
            backend: &dyn Backend,
        ) -> Handled<FormDialogAction> {
            self.widget.handle_event(&mut self.state, event, backend)
        }
    }

    fn host() -> Host {
        let widget = FormDialog::new(WidgetId::new("create_milestone"), "Create milestone")
            .with_buttons(vec![
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
        let mut c = Host {
            widget,
            state: FormDialogState::default(),
        };
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

    fn many_field_dialog(n: usize) -> FormDialog {
        let mut form = name_description_form();
        let template = form.fields[0].clone();
        form.fields = (0..n)
            .map(|i| FormField {
                id: WidgetId::new(format!("field{i}")),
                ..template.clone()
            })
            .collect();
        let mut dialog = FormDialog::new(WidgetId::new("many"), "Many");
        dialog.form = Some(form);
        dialog
    }

    #[test]
    fn pixel_rows_that_overflow_the_form_slot_reserve_a_scrollbar_track() {
        let ctx = LayoutCtx::new(20.0, 8.0, false);
        let dialog = many_field_dialog(10);
        let layout = Primitive::layout(&dialog, &ctx, Rect::new(0.0, 0.0, 800.0, 600.0));
        let bounds = layout.dialog.layout.input_bounds.expect("form slot");
        let form = layout.form.as_ref().expect("form layout");
        let (scrollbar, sb_layout) = form.scrollbar.as_ref().expect("scrollbar track");
        let track = scrollbar.track;
        assert_eq!(track.x + track.width, bounds.x + bounds.width);
        assert_eq!(form.form.area.width, bounds.width - track.width);
        assert_eq!(sb_layout.track, track);
    }

    #[test]
    fn cell_rows_that_fit_the_form_slot_reserve_no_scrollbar_track() {
        let ctx = LayoutCtx::new(1.0, 1.0, false);
        let layout =
            Primitive::layout(&many_field_dialog(2), &ctx, Rect::new(0.0, 0.0, 80.0, 24.0));
        assert!(layout.form.expect("form layout").scrollbar.is_none());
    }

    /// Drive `dialog` with `kind` at surface point `at`, laid out by
    /// `ctx` over `rect` — the pixel-surface counterpart of
    /// [`FormDialog::handle_event`], since a cell-grid dialog grows to
    /// fit every field instead of scrolling.
    fn drive_pointer(
        dialog: &FormDialog,
        state: &mut FormDialogState,
        ctx: &LayoutCtx,
        rect: Rect,
        kind: PointerKind,
        at: Point,
    ) -> Handled<FormDialogAction> {
        let layout = Primitive::layout(dialog, ctx, rect);
        let event = RoutedEvent::Pointer {
            kind,
            sub: None,
            local: Point::new(at.x - rect.x, at.y - rect.y),
            mods: Modifiers::default(),
        };
        widget_host::drive(dialog, dialog.id, state, &event, &layout, ctx.units()).handled
    }

    /// The form's scrollbar thumb follows a drag: a press on the thumb
    /// starts a drag, a move down scrolls the form, and the release ends
    /// the drag.
    #[test]
    fn dragging_the_form_scrollbar_thumb_scrolls_the_form() {
        let ctx = LayoutCtx::new(20.0, 8.0, false);
        let rect = Rect::new(0.0, 0.0, 800.0, 600.0);
        let dialog = many_field_dialog(40);
        let mut state = FormDialogState::default();
        let layout = Primitive::layout(&dialog, &ctx, rect);
        let (sb, sb_layout) = layout
            .form
            .as_ref()
            .and_then(|f| f.scrollbar.clone())
            .expect("forty fields overflow the dialog's form slot");
        let x = sb.track.x + sb.track.width / 2.0;
        let y = sb.track.y + sb_layout.local_thumb_start() + 1.0;
        let press = PointerKind::Down(MouseButton::Left);
        let result = drive_pointer(&dialog, &mut state, &ctx, rect, press, Point::new(x, y));
        assert_eq!(result, Handled::Consumed);
        assert!(
            state.form.thumb_drag.is_some(),
            "the press starts a thumb drag"
        );
        let to = Point::new(x, y + 40.0);
        let result = drive_pointer(&dialog, &mut state, &ctx, rect, PointerKind::Move, to);
        assert_eq!(result, Handled::Consumed);
        let scrolled = state.form.scroll_offset.expect("the drag scrolls the form");
        assert!(
            scrolled > 0,
            "a drag down scrolls the form down, got {scrolled}"
        );
        let release = PointerKind::Up(MouseButton::Left);
        let result = drive_pointer(&dialog, &mut state, &ctx, rect, release, to);
        assert_eq!(result, Handled::Consumed);
        assert!(state.form.thumb_drag.is_none(), "the release ends the drag");
    }

    /// A press on the bare track below the thumb pages the form down by
    /// one viewport, clamped to the last full page.
    #[test]
    fn a_press_below_the_form_scrollbar_thumb_pages_down() {
        let ctx = LayoutCtx::new(20.0, 8.0, false);
        let rect = Rect::new(0.0, 0.0, 800.0, 600.0);
        let dialog = many_field_dialog(40);
        let mut state = FormDialogState::default();
        let layout = Primitive::layout(&dialog, &ctx, rect);
        let form = layout.form.as_ref().expect("form layout");
        let (sb, _) = form.scrollbar.clone().expect("a scrollbar");
        let at = Point::new(sb.track.x + 1.0, sb.track.y + sb.track.height - 1.0);
        let press = PointerKind::Down(MouseButton::Left);
        let result = drive_pointer(&dialog, &mut state, &ctx, rect, press, at);
        assert_eq!(result, Handled::Consumed);
        let rows = form.viewport_rows;
        assert_eq!(state.form.scroll_offset, Some(rows.min(40 - rows)));
    }

    #[test]
    fn dialog_input_is_a_form_with_both_fields() {
        let c = host();
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
        let mut c = host();
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
        let mut c = host();
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
        let mut c = host();
        assert!(c.dialog().body.is_empty());
        c.set_body(vec![StyledText::plain("Created: Alpha")]);
        assert_eq!(c.dialog().body.len(), 1);
    }

    #[test]
    fn starts_with_form_focused() {
        let c = host();
        assert!(c.is_form_focused());
        assert_eq!(c.focused_button_id(), None);
    }

    #[test]
    fn tab_moves_from_name_to_description_field() {
        let mut c = host();
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
        let mut c = host();
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
        let mut c = host();
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
        let mut c = host();
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
        let mut c = host();
        let mut backend = crate::tui::backend::TuiBackend::new();
        let result = c.handle_event(&tab_event(true), &mut backend);
        assert_eq!(result, Handled::Consumed);
        assert!(!c.is_form_focused());
        assert_eq!(c.focused_button_id().unwrap().as_str(), "create");
    }

    #[test]
    fn left_right_cycle_buttons_once_focused() {
        let mut c = host();
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
        let mut c = host();
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
        let mut c = host();
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
        let mut c = host();
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
        let c = host();
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
        let c = host();
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
        let c = host();
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
        let mut c = host();
        let mut backend = crate::tui::backend::TuiBackend::new();
        let _ = c.handle_event(&tab_event(true), &mut backend); // focus -> "create"
        let ev = UiEvent::CharTyped('x');
        assert_eq!(c.handle_event(&ev, &mut backend), Handled::Ignored);
    }

    /// `focus_order` skips `Label`/`ReadOnly`/disabled fields, matching
    /// the form's own Tab traversal order.
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
