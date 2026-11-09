//! `FormDialogController` — drives a [`Dialog`] hosting a multi-field
//! [`Form`] ([`DialogInput::Form`]).
//!
//! Composes the [`Dialog`] primitive with a [`FormController`], the same
//! controller every other form host (`examples/common/form_scroll.rs`,
//! `FolderPickerController`) uses — so a dialog's embedded form gets
//! keystroke editing, validation and visibility for free, exactly like a
//! full-page form does. This controller's own job is the piece neither
//! primitive has on its own: handing keyboard focus back and forth
//! between the form's fields and the dialog's buttons.
//!
//! # Usage pattern
//!
//! ```rust,ignore
//! let mut dialog = FormDialogController::new("create_milestone", "Create milestone")
//!     .with_buttons(vec![
//!         DialogButton { id: WidgetId::new("cancel"), label: "Cancel".into(), is_cancel: true, is_default: false, tint: None, spec: None },
//!         DialogButton { id: WidgetId::new("create"), label: "Create".into(), is_default: true, is_cancel: false, tint: None, spec: None },
//!     ]);
//! dialog.set_form(my_form); // rebuilt every frame from app-owned field values, like `FormController::set_form`
//!
//! // AppLogic::render:
//! dialog.render(backend);
//!
//! // AppLogic::handle:
//! match dialog.handle(&event, backend) {
//!     FormDialogEvent::ButtonClicked(id) if id.as_str() == "create" => {
//!         // Read the app's own canonical field values (already synced via
//!         // FormAction(FormEvent::TextInputChanged { .. }) below) — same
//!         // "app owns the data" contract `Form`'s own module doc states.
//!     }
//!     FormDialogEvent::FormAction(event) => { /* apply to app state, same as any FormController consumer */ }
//!     _ => {}
//! }
//! ```
//!
//! # Keyboard model
//!
//! Tab/Shift-Tab traverse the form's focusable fields first (delegated to
//! [`FormController`], which also keeps typing/cursor/selection working);
//! stepping past the last field moves focus onto the first button, and
//! stepping back past the first field moves focus onto the last button —
//! a single cyclic Tab order across both widgets. Left/Right additionally
//! cycle among the buttons once they have focus, mirroring
//! [`crate::compose::MessageDialogController`]'s own button navigation.
//! Enter activates the focused button when one has focus, otherwise it's
//! forwarded to the form (commits a `TextInput`, inserts a newline in a
//! `TextArea`, etc. — see [`FormController`]'s own "Keyboard editing" doc
//! section). Escape resolves to [`Dialog::cancel_button_id`], falling
//! back to [`Dialog::default_button_id`], regardless of where button
//! focus currently is — *except* while the focused field is an open
//! `ComboBox` popup, where Escape is forwarded into the form first so it
//! closes the popup instead of the whole dialog (mirroring
//! [`FormController`]'s own combo-box Escape handling).
//!
//! Every button-activating path above (Enter while a button has focus,
//! Escape) resolves the id regardless of which button currently holds
//! *keyboard* focus — Escape always targets the cancel/default button,
//! not whichever one Tab last landed on.
//!
//! Unlike a bare [`FormController`] consumer (which must re-apply a
//! `FormEvent::FocusChanged` onto its own canonical `Form` and call
//! `set_form` again before the next frame — see
//! `examples/common/form_scroll.rs`), this controller applies that
//! hand-off to its own stored form itself, the same way it already owns
//! `focused_button`: a dialog is a self-contained modal, not a page the
//! host app otherwise tracks field-by-field focus for. `FormAction`
//! still bubbles the event up (including when focus moves onto the form
//! from a button — see [`Self::enter_form`]) so a consumer that *does*
//! want to mirror focus elsewhere (e.g. a status line, or its own
//! canonical `focused_field` it feeds back via [`Self::set_form`]) can,
//! and must: [`Self::dialog`] always reports the stored form's *current*
//! `focused_field`, so a consumer whose own mirror goes stale here would
//! otherwise stomp it back to the wrong field on its next `set_form`.
//!
//! # No visual keyboard-focus ring (known limitation)
//!
//! Like [`crate::compose::MessageDialogController`] (see its own doc
//! section of the same name), [`DialogButton`] carries no
//! "keyboard-focused" field, so a button holding keyboard focus renders
//! no differently from an unfocused one beyond `is_default`'s existing
//! styling. It's worse here: moving focus onto a button
//! ([`Self::enter_buttons`]) also clears the form's own focus highlight
//! (`FormController::set_has_focus(false)`), so once focus leaves the
//! form nothing on screen is highlighted at all until it returns. Enter/
//! Escape/click routing is fully correct regardless; this only affects
//! what a sighted user sees highlighted while tabbing.

use crate::backend::Backend;
use crate::event::{Point, Rect, UiEvent};
use crate::primitives::dialog::{
    Dialog, DialogButton, DialogHit, DialogInput, DialogLayout, DialogMeasure, DialogSeverity,
};
use crate::primitives::form::{FieldKind, Form, FormEvent};
use crate::primitives::toolbar::ToolbarItemMeasure;
use crate::types::{StyledText, WidgetId};
use crate::{Key, Modifiers, NamedKey};

use super::form_controller::{focus_order, FormController, FormControllerEvent};

/// What happened after [`FormDialogController::handle`] processed an event.
#[derive(Debug, Clone, PartialEq)]
pub enum FormDialogEvent {
    /// A button was clicked, activated via Enter while it had keyboard
    /// focus, or resolved via Escape (which always targets
    /// [`Dialog::cancel_button_id`]/[`Dialog::default_button_id`]
    /// regardless of keyboard focus — see this module's "Keyboard
    /// model" doc section). The consumer should check `id` against the
    /// [`DialogButton::id`] values it built the dialog from — e.g. a
    /// click on the "Create" button is the submit signal; the app reads
    /// its own canonical field values (already kept in sync via
    /// `FormAction` below) rather than this event carrying them.
    ButtonClicked(WidgetId),
    /// A field-level event occurred inside the form (text typed, a
    /// toggle flipped, focus moved between fields, …). Apply it the same
    /// way a direct [`FormController`] consumer would.
    FormAction(FormEvent),
    /// Event consumed — internal state (keyboard/button focus, scroll)
    /// changed, caller should redraw.
    Consumed,
    /// Event not relevant to this controller.
    Ignored,
}

/// Cross-backend compose controller for a [`Dialog`] hosting a
/// multi-field [`Form`] in its [`DialogInput::Form`] slot.
///
/// See the [module-level documentation](self) for a usage example.
pub struct FormDialogController {
    id: WidgetId,
    title: StyledText,
    body: Vec<StyledText>,
    severity: Option<DialogSeverity>,
    vertical_buttons: bool,
    buttons: Vec<DialogButton>,
    form: FormController,
    /// `None` = keyboard focus is on the form; `Some(i)` = focus is on
    /// `buttons[i]`.
    focused_button: Option<usize>,
}

impl FormDialogController {
    /// Build an empty controller — no form, no buttons. Call
    /// [`Self::set_form`] and [`Self::with_buttons`] before using it.
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        let id = id.into();
        let mut form = FormController::new(format!("{id}-form"));
        form.set_has_focus(true);
        Self {
            id: WidgetId::new(id),
            title: StyledText::plain(title.into()),
            body: Vec::new(),
            severity: None,
            vertical_buttons: false,
            buttons: Vec::new(),
            form,
            focused_button: None,
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

    /// Set body text lines, rendered above the form (e.g. instructions,
    /// or a status line a consumer updates after a submit). See
    /// [`Dialog::body`]'s own "title → body → table → input → buttons"
    /// layout order. Returns `self` for builder-style chaining.
    pub fn with_body(mut self, body: Vec<StyledText>) -> Self {
        self.body = body;
        self
    }

    /// Replace the body text lines set via [`Self::with_body`] — unlike
    /// that builder, callable after construction, for a status line a
    /// consumer wants to update in response to its own `FormDialogEvent`
    /// handling (e.g. "Created: …" after a submit).
    pub fn set_body(&mut self, body: Vec<StyledText>) {
        self.body = body;
    }

    /// Push a fresh `form` into the controller — call every frame with
    /// the app's own canonical field values, exactly like
    /// [`FormController::set_form`] (this delegates straight to it).
    pub fn set_form(&mut self, form: Form) {
        self.form.set_form(form);
    }

    /// The form most recently pushed via [`Self::set_form`].
    pub fn form(&self) -> Option<&Form> {
        self.form.form()
    }

    /// Whether keyboard focus is currently on the form (as opposed to
    /// one of the buttons).
    pub fn is_form_focused(&self) -> bool {
        self.focused_button.is_none()
    }

    /// The `WidgetId` of the currently keyboard-focused button, or
    /// `None` when focus is on the form.
    pub fn focused_button_id(&self) -> Option<&WidgetId> {
        self.focused_button
            .and_then(|i| self.buttons.get(i))
            .map(|b| &b.id)
    }

    /// [`Dialog::cancel_button_id`], falling back to
    /// [`Dialog::default_button_id`] — computed directly against
    /// `self.buttons` (rather than through [`Self::dialog`], whose
    /// return value is an owned temporary a caller can't borrow from)
    /// for Escape's "there's always something to resolve to" fallback.
    fn cancel_or_default_button_id(&self) -> Option<WidgetId> {
        self.buttons
            .iter()
            .find(|b| b.is_cancel)
            .or_else(|| self.buttons.iter().find(|b| b.is_default))
            .or_else(|| self.buttons.last())
            .map(|b| b.id)
    }

    /// The underlying [`Dialog`] descriptor, built fresh from this
    /// controller's current state.
    pub fn dialog(&self) -> Dialog {
        Dialog {
            id: self.id,
            title: self.title.clone(),
            body: self.body.clone(),
            buttons: self.buttons.clone(),
            severity: self.severity,
            vertical_buttons: self.vertical_buttons,
            table: None,
            // `live_form` (not `form`) so the descriptor used for both
            // painting and a11y chrome (`a11y::chrome`'s
            // `DialogInput::Form` child) reports the form's *actual*
            // on-screen state — in particular `has_focus`, which
            // `self.form`'s raw stored descriptor doesn't track (a
            // consumer's own mirror typically always pushes
            // `has_focus: true`; this controller is the one that knows
            // whether focus is really on the form or on a button).
            input: self.form.live_form().map(DialogInput::Form),
        }
    }

    /// The dialog's full on-screen bounds for this backend's current
    /// viewport — see [`crate::compose::MessageDialogController::bounds`]'s
    /// own doc for why a caller tracking a [`crate::ModalStack`] entry
    /// wants this.
    pub fn bounds(&self, backend: &dyn Backend) -> Rect {
        self.layout(backend).bounds
    }

    // ── Layout ────────────────────────────────────────────────────────

    fn layout(&self, backend: &dyn Backend) -> DialogLayout {
        let m = backend.measure();
        let lh = m.line_height;
        let char_w = m.char_width;
        let viewport = backend.viewport();
        let dialog = self.dialog();
        let has_title = dialog.title.spans.iter().any(|s| !s.text.is_empty());
        let button_width = self
            .buttons
            .iter()
            .map(|b| b.resolved_button_spec().min_cell_width() * char_w)
            .fold(char_w * 10.0, f32::max);
        let width = (viewport.width * 0.5).clamp(char_w * 30.0, char_w * 64.0);
        let padding = lh;
        let input_height = self.measure_input_height(backend, &dialog, width, padding);
        let measure = DialogMeasure {
            width,
            title_height: if has_title { lh } else { 0.0 },
            body_height: lh * dialog.body.len() as f32,
            table_height: 0.0,
            input_height,
            button_row_height: lh,
            button_width,
            button_gap: char_w * 2.0,
            padding,
        };
        let viewport_rect = Rect::new(0.0, 0.0, viewport.width, viewport.height);
        dialog.layout(viewport_rect, measure, |_| ToolbarItemMeasure::new(0.0))
    }

    /// The input slot's real height for `dialog.input`, measured
    /// per-field through [`Backend::form_layout`] rather than the
    /// one-row-per-field estimate every other `DialogInput` variant can
    /// get away with — a `FieldKind::TextArea` is `visible_rows` tall, a
    /// `Top`-labelled field takes an extra row for its label line, and a
    /// focused `Dropdown` grows to fit its open option list (see
    /// `tui::form::tui_form_layout`'s own per-kind measure). Probes with
    /// a generously tall rect so every field lands at its full,
    /// unclipped height — [`Form::layout`]'s "last row squeezed to fit
    /// the viewport" clipping never kicks in here, since this *is* how
    /// the viewport height gets decided.
    fn measure_input_height(
        &self,
        backend: &dyn Backend,
        dialog: &Dialog,
        dialog_width: f32,
        padding: f32,
    ) -> f32 {
        let Some(DialogInput::Form(form)) = dialog.input.as_ref() else {
            return 0.0;
        };
        if form.fields.is_empty() {
            return 0.0;
        }
        let content_w = (dialog_width - padding * 2.0).max(0.0);
        let probe_rect = Rect::new(0.0, 0.0, content_w, f32::MAX / 4.0);
        let probe = backend.form_layout(probe_rect, form);
        let content_h = probe
            .visible_fields
            .iter()
            .map(|f| f.bounds.y + f.bounds.height)
            .fold(0.0_f32, f32::max);
        content_h + form.spacing.padding
    }

    /// The on-screen rect the embedded form renders/hit-tests within —
    /// `layout(backend).input_bounds`, or a zero-sized rect when the
    /// controller has no form yet.
    fn form_rect(&self, backend: &dyn Backend) -> Rect {
        self.layout(backend)
            .input_bounds
            .unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0))
    }

    // ── Render ────────────────────────────────────────────────────────

    /// Paint the dialog, centered in the backend's current viewport.
    ///
    /// Call this from `AppLogic::render` whenever the dialog is open.
    pub fn render(&self, backend: &mut dyn Backend) {
        let layout = self.layout(backend);
        let dialog = self.dialog();
        backend.draw_dialog(&dialog, &layout);
        if let Some(input_bounds) = layout.input_bounds {
            self.form.render(backend, input_bounds);
        }
    }

    // ── Handle ────────────────────────────────────────────────────────

    /// Drive the state machine with a backend-neutral [`UiEvent`].
    pub fn handle(&mut self, event: &UiEvent, backend: &mut dyn Backend) -> FormDialogEvent {
        match event {
            UiEvent::KeyPressed { key, modifiers, .. } => {
                self.handle_key(key, modifiers, event, backend)
            }
            UiEvent::MouseDown { position, .. } => self.handle_click(event, *position, backend),
            _ if self.focused_button.is_none() => {
                let rect = self.form_rect(backend);
                let result = self.form.handle(event, backend, rect);
                self.apply_form_result(result)
            }
            _ => FormDialogEvent::Ignored,
        }
    }

    fn handle_key(
        &mut self,
        key: &Key,
        modifiers: &Modifiers,
        event: &UiEvent,
        backend: &mut dyn Backend,
    ) -> FormDialogEvent {
        match key {
            // An open `ComboBox` popup claims Escape first — closing the
            // popup, not the dialog — the same precedence
            // `FormController::handle_key` already gives its own
            // combo-box Escape path; this controller otherwise never
            // forwards Escape, so it would be unreachable without this
            // check.
            Key::Named(NamedKey::Escape)
                if self.focused_button.is_none() && self.form_has_open_combo_box() =>
            {
                self.forward_to_form(event, backend)
            }
            Key::Named(NamedKey::Escape) => match self.cancel_or_default_button_id() {
                Some(id) => FormDialogEvent::ButtonClicked(id),
                None => FormDialogEvent::Ignored,
            },
            Key::Named(NamedKey::Enter) => match self.focused_button {
                Some(i) => match self.buttons.get(i) {
                    Some(b) => FormDialogEvent::ButtonClicked(b.id),
                    None => FormDialogEvent::Ignored,
                },
                None => self.forward_to_form(event, backend),
            },
            Key::Named(NamedKey::Tab) if !modifiers.ctrl && !modifiers.alt && !modifiers.cmd => {
                self.navigate(if modifiers.shift { -1 } else { 1 }, backend)
            }
            Key::Named(NamedKey::BackTab) => self.navigate(-1, backend),
            Key::Named(NamedKey::Left) if self.focused_button.is_some() => self.cycle_button(-1),
            Key::Named(NamedKey::Right) if self.focused_button.is_some() => self.cycle_button(1),
            _ if self.focused_button.is_none() => self.forward_to_form(event, backend),
            _ => FormDialogEvent::Ignored,
        }
    }

    /// Forward `event` (the original [`UiEvent::KeyPressed`] `handle`
    /// received) into the form unchanged — preserving its `repeat` flag
    /// rather than reconstructing a new event that always hardcodes
    /// `repeat: false`.
    fn forward_to_form(&mut self, event: &UiEvent, backend: &mut dyn Backend) -> FormDialogEvent {
        let rect = self.form_rect(backend);
        let result = self.form.handle(event, backend, rect);
        self.apply_form_result(result)
    }

    /// Whether the focused field is an enabled, currently-open
    /// `ComboBox` — see this module's "Keyboard model" doc section for
    /// why Escape checks this before resolving to a button.
    fn form_has_open_combo_box(&self) -> bool {
        let Some(form) = self.form.form() else {
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

    /// Move keyboard focus by `delta` (`1` = forward, `-1` = backward)
    /// across the single cyclic order: form fields, then buttons, then
    /// back to form fields — see this module's "Keyboard model" doc
    /// section.
    fn navigate(&mut self, delta: isize, backend: &mut dyn Backend) -> FormDialogEvent {
        match self.focused_button {
            None => {
                let stops = self.form.form().map(focus_order).unwrap_or_default();
                let at_boundary = match &stops {
                    empty if empty.is_empty() => true,
                    stops => {
                        let cur = self
                            .form
                            .form()
                            .and_then(|f| f.focused_field)
                            .and_then(|id| stops.iter().position(|s| *s == id));
                        match cur {
                            Some(i) if delta > 0 => i + 1 >= stops.len(),
                            Some(0) if delta < 0 => true,
                            None if delta < 0 => true,
                            _ => false,
                        }
                    }
                };
                if at_boundary {
                    return self.enter_buttons(delta);
                }
                let rect = self.form_rect(backend);
                let event = UiEvent::KeyPressed {
                    key: if delta > 0 {
                        Key::Named(NamedKey::Tab)
                    } else {
                        Key::Named(NamedKey::BackTab)
                    },
                    modifiers: Modifiers::default(),
                    repeat: false,
                };
                let result = self.form.handle(&event, backend, rect);
                self.apply_form_result(result)
            }
            Some(i) => {
                let n = self.buttons.len();
                if n == 0 {
                    return FormDialogEvent::Ignored;
                }
                let at_boundary = (delta > 0 && i + 1 >= n) || (delta < 0 && i == 0);
                if at_boundary {
                    return self.enter_form(delta);
                }
                self.focused_button = Some((i as isize + delta) as usize);
                FormDialogEvent::Consumed
            }
        }
    }

    fn enter_buttons(&mut self, delta: isize) -> FormDialogEvent {
        if self.buttons.is_empty() {
            return FormDialogEvent::Ignored;
        }
        self.focused_button = Some(if delta > 0 { 0 } else { self.buttons.len() - 1 });
        self.form.set_has_focus(false);
        FormDialogEvent::Consumed
    }

    /// Move focus from the button row onto the form's first (`delta >
    /// 0`) or last (`delta < 0`) focusable field, self-applying the new
    /// `focused_field` onto the stored form (same as every other
    /// focus-moving path in this controller) **and** surfacing it as
    /// [`FormDialogEvent::FormAction(FormEvent::FocusChanged)`] — a
    /// consumer that mirrors `focused_field` onto its own canonical
    /// `Form` (the documented `set_form`-every-frame contract; see this
    /// module's doc) needs this event the same way it needs the one
    /// `FormController::handle` emits for an in-form Tab, or its mirror
    /// goes stale the moment focus re-enters the form from a button.
    fn enter_form(&mut self, delta: isize) -> FormDialogEvent {
        self.focused_button = None;
        self.form.set_has_focus(true);
        let Some(mut form) = self.form.form().cloned() else {
            return FormDialogEvent::Consumed;
        };
        let stops = focus_order(&form);
        let target = if delta > 0 {
            stops.first()
        } else {
            stops.last()
        };
        match target {
            Some(id) => {
                let id = *id;
                form.focused_field = Some(id);
                self.form.set_form(form);
                FormDialogEvent::FormAction(FormEvent::FocusChanged { id })
            }
            None => FormDialogEvent::Consumed,
        }
    }

    fn cycle_button(&mut self, delta: isize) -> FormDialogEvent {
        let Some(i) = self.focused_button else {
            return FormDialogEvent::Ignored;
        };
        let n = self.buttons.len();
        if n == 0 {
            return FormDialogEvent::Ignored;
        }
        self.focused_button = Some(crate::focus::cycle_index(i, delta, n));
        FormDialogEvent::Consumed
    }

    fn handle_click(
        &mut self,
        event: &UiEvent,
        position: Point,
        backend: &mut dyn Backend,
    ) -> FormDialogEvent {
        let layout = self.layout(backend);
        match layout.hit_test(position.x, position.y) {
            DialogHit::Button(id) => {
                if let Some(i) = self.buttons.iter().position(|b| b.id == id) {
                    self.focused_button = Some(i);
                    self.form.set_has_focus(false);
                }
                FormDialogEvent::ButtonClicked(id)
            }
            DialogHit::Outside => FormDialogEvent::Consumed,
            // Body (or an impossible `BodyToolbarButton` — this
            // controller never sets `body_toolbar_layout`): delegate to
            // the form's own click routing so clicking a field moves
            // keyboard focus onto it.
            _ => {
                let Some(input_bounds) = layout.input_bounds else {
                    return FormDialogEvent::Consumed;
                };
                self.focused_button = None;
                self.form.set_has_focus(true);
                let result = self.form.handle(event, backend, input_bounds);
                self.apply_form_result(result)
            }
        }
    }

    /// Map a [`FormControllerEvent`] from `self.form.handle`/`handle_cached`
    /// to a [`FormDialogEvent`], self-applying a `FocusChanged` onto the
    /// stored form first — see this module's "Keyboard model" doc section
    /// for why this controller, unlike a bare [`FormController`] consumer,
    /// doesn't leave that hand-off to the caller.
    fn apply_form_result(&mut self, result: FormControllerEvent) -> FormDialogEvent {
        if let FormControllerEvent::FormAction(FormEvent::FocusChanged { id }) = &result {
            if let Some(mut form) = self.form.form().cloned() {
                form.focused_field = Some(*id);
                self.form.set_form(form);
            }
        }
        map_form_event(result)
    }
}

fn map_form_event(event: FormControllerEvent) -> FormDialogEvent {
    match event {
        FormControllerEvent::FormAction(e) => FormDialogEvent::FormAction(e),
        FormControllerEvent::ScrollChanged | FormControllerEvent::Consumed => {
            FormDialogEvent::Consumed
        }
        FormControllerEvent::Ignored => FormDialogEvent::Ignored,
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
        c.handle(&tab_event(false), &mut backend); // name -> description
        c.handle(&tab_event(false), &mut backend); // description -> cancel
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
        let result = c.handle(&esc, &mut backend);
        // The combo box's own Escape path cancels the edit back to the
        // committed value and closes the popup — not a `ButtonClicked`.
        assert!(
            matches!(result, FormDialogEvent::FormAction(_)),
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
        let result = c.handle(&ev, &mut backend);
        assert!(matches!(
            result,
            FormDialogEvent::FormAction(FormEvent::FocusChanged { .. })
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
        c.handle(&tab_event(false), &mut backend);
        let result = c.handle(&tab_event(false), &mut backend);
        assert_eq!(result, FormDialogEvent::Consumed);
        assert!(!c.is_form_focused());
        assert_eq!(c.focused_button_id().unwrap().as_str(), "cancel");
    }

    #[test]
    fn tab_past_the_last_button_wraps_to_the_first_field() {
        let mut c = controller();
        let mut backend = crate::tui::backend::TuiBackend::new();
        c.handle(&tab_event(false), &mut backend); // name -> description
        c.handle(&tab_event(false), &mut backend); // description -> cancel
        c.handle(&tab_event(false), &mut backend); // cancel -> create
        let result = c.handle(&tab_event(false), &mut backend); // create -> name
                                                                // `enter_form` surfaces `FocusChanged` (not a bare `Consumed`)
                                                                // so a consumer mirroring `focused_field` onto its own
                                                                // canonical `Form` (the documented `set_form`-every-frame
                                                                // contract) learns about the hand-off too — see this module's
                                                                // "Keyboard model" doc section.
        assert_eq!(
            result,
            FormDialogEvent::FormAction(FormEvent::FocusChanged {
                id: WidgetId::new("name")
            })
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
        c.handle(&tab_event(false), &mut backend); // name -> description
        c.handle(&tab_event(false), &mut backend); // description -> cancel
        c.handle(&tab_event(false), &mut backend); // cancel -> create
        c.handle(&tab_event(false), &mut backend); // create -> (wraps) name
        let result = c.handle(&UiEvent::CharTyped('z'), &mut backend);
        assert_eq!(
            result,
            FormDialogEvent::FormAction(FormEvent::TextInputChanged {
                id: WidgetId::new("name"),
                value: "z".into(),
            })
        );
    }

    #[test]
    fn shift_tab_from_the_first_field_moves_to_the_last_button() {
        let mut c = controller();
        let mut backend = crate::tui::backend::TuiBackend::new();
        let result = c.handle(&tab_event(true), &mut backend);
        assert_eq!(result, FormDialogEvent::Consumed);
        assert!(!c.is_form_focused());
        assert_eq!(c.focused_button_id().unwrap().as_str(), "create");
    }

    #[test]
    fn left_right_cycle_buttons_once_focused() {
        let mut c = controller();
        let mut backend = crate::tui::backend::TuiBackend::new();
        c.handle(&tab_event(false), &mut backend);
        c.handle(&tab_event(false), &mut backend); // now on "cancel"
        let right = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Right),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        c.handle(&right, &mut backend);
        assert_eq!(c.focused_button_id().unwrap().as_str(), "create");
    }

    #[test]
    fn typing_while_name_focused_updates_the_form_value() {
        let mut c = controller();
        let mut backend = crate::tui::backend::TuiBackend::new();
        let ev = UiEvent::CharTyped('x');
        let result = c.handle(&ev, &mut backend);
        match result {
            FormDialogEvent::FormAction(FormEvent::TextInputChanged { id, value }) => {
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
        c.handle(&tab_event(true), &mut backend); // shift-tab: focus -> "create"
        let enter = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let result = c.handle(&enter, &mut backend);
        assert_eq!(
            result,
            FormDialogEvent::ButtonClicked(WidgetId::new("create"))
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
        let result = c.handle(&esc, &mut backend);
        assert_eq!(
            result,
            FormDialogEvent::ButtonClicked(WidgetId::new("cancel"))
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
        let result = c.handle(&ev, &mut backend);
        assert_eq!(
            result,
            FormDialogEvent::ButtonClicked(WidgetId::new("create"))
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
        c.handle(&tab_event(true), &mut backend); // focus -> "create"
        let ev = UiEvent::CharTyped('x');
        assert_eq!(c.handle(&ev, &mut backend), FormDialogEvent::Ignored);
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
