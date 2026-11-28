//! Backend-agnostic app code for the `Dialog` + `Form` demo
//! ([`tui_form_dialog`]).
//!
//! [`FormDialogApp`] shows a "Create milestone" dialog with a required
//! **Name** (`TextInput`) and an optional **Description** (`TextArea`) —
//! the two-field shape `DialogInput::Form` exists for — hosted through
//! the [`quadraui::FormDialog`] widget. The demo exercises:
//! - `DialogInput::Form` rendering both fields inside the dialog box,
//!   with the button row below them.
//! - Tab/Shift-Tab moving focus Name → Description → Cancel → Create and
//!   back around (see `FormDialog`'s own "Focus model" doc section).
//! - Submitting via "Create" (or Enter while a button has focus) reads
//!   back this app's own canonical `name`/`description` state and shows
//!   it as a status line above the form via `FormDialog::body`.
//!
//! Controls:
//! - `Tab` / `Shift+Tab`  move focus: Name → Description → Cancel → Create → …
//! - typing (field focused) edit the focused field
//! - `Enter`              commit text, or activate the focused button
//! - `Esc`                cancel (same as clicking Cancel) — quits
//! - `q` (no field focused) quit

use quadraui::{
    AppLogic, Backend, DialogButton, DrawsPrimitives, FieldKind, Form, FormDialog,
    FormDialogAction, FormDialogState, FormEvent, Handled, Key, Reaction, StyledText, UiEvent,
    WidgetId,
};

pub struct FormDialogApp {
    dialog: FormDialog,
    state: FormDialogState,
    name: String,
    description: String,
    focused_field: Option<WidgetId>,
}

impl FormDialogApp {
    pub fn new() -> Self {
        let dialog = FormDialog::new(WidgetId::new("create_milestone"), "Create milestone")
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
        let mut app = Self {
            dialog,
            state: FormDialogState::default(),
            name: String::new(),
            description: String::new(),
            focused_field: Some(WidgetId::new("name")),
        };
        app.sync_form();
        app
    }

    fn build_form(&self) -> Form {
        use quadraui::FormField;
        Form {
            id: WidgetId::new("form:create_milestone"),
            fields: vec![
                FormField {
                    id: WidgetId::new("name"),
                    label: StyledText::plain("Name"),
                    kind: FieldKind::TextInput {
                        value: self.name.clone(),
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
                        value: self.description.clone(),
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
            focused_field: self.focused_field,
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

    /// Push the current `name`/`description`/`focused_field` into the
    /// dialog — call after every mutation, the "rebuild the `Form` from
    /// the app's own canonical state" contract `Form`'s own module doc
    /// states.
    fn sync_form(&mut self) {
        let form = self.build_form();
        self.dialog.form = Some(form);
    }

    /// The button holding keyboard focus, or `None` while focus is on the
    /// form — exposed for `tests/tui_example_driver.rs` to assert on
    /// keyboard-focus state that has no on-screen rendering of its own to
    /// `screen_contains` against. Dead in the `tui_form_dialog` example
    /// binary itself — the standard cost of a shared example module
    /// `#[path]`-included by a test binary too (see
    /// `examples/common/appshell_demo.rs`'s own `ActivityProbe` note).
    #[allow(dead_code)]
    pub fn focused_button_id(&self) -> Option<&WidgetId> {
        self.dialog.focused_button_id(&self.state)
    }

    /// Whether keyboard focus is on the form rather than a button.
    pub fn form_focused(&self) -> bool {
        self.state.focused_button.is_none()
    }

    /// The form the dialog currently hosts — see
    /// [`Self::focused_button_id`] for why the test binary reads it.
    #[allow(dead_code)]
    pub fn form(&self) -> Option<&Form> {
        self.dialog.form.as_ref()
    }
}

impl Default for FormDialogApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for FormDialogApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        backend.draw_dyn(
            backend.viewport().rect(),
            &self.dialog.painted(&self.state),
            &(),
        );
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        // `q` quits — unless a field currently has keyboard focus, in
        // which case it's a character the user is typing into it.
        if let UiEvent::KeyPressed {
            key: Key::Char('q'),
            ..
        } = &event
        {
            if !self.form_focused() {
                return Reaction::Exit;
            }
        }
        if let UiEvent::WindowResized { .. } = &event {
            return Reaction::Redraw;
        }

        match self.dialog.handle_event(&mut self.state, &event, backend) {
            Handled::Event(FormDialogAction::ButtonClicked(id)) if id.as_str() == "cancel" => {
                Reaction::Exit
            }
            Handled::Event(FormDialogAction::ButtonClicked(id)) if id.as_str() == "create" => {
                self.dialog.body = vec![StyledText::plain(format!(
                    "Created: {:?} / {:?}",
                    self.name, self.description
                ))];
                Reaction::Redraw
            }
            Handled::Event(FormDialogAction::ButtonClicked(_)) => Reaction::Redraw,
            Handled::Event(FormDialogAction::FormAction(fe)) => {
                match fe {
                    FormEvent::TextInputChanged { id, value }
                    | FormEvent::TextInputCommitted { id, value }
                        if id.as_str() == "name" =>
                    {
                        self.name = value;
                    }
                    FormEvent::TextInputChanged { id, value }
                    | FormEvent::TextInputCommitted { id, value }
                        if id.as_str() == "description" =>
                    {
                        self.description = value;
                    }
                    FormEvent::FocusChanged { id } => {
                        self.focused_field = Some(id);
                    }
                    _ => {}
                }
                self.sync_form();
                Reaction::Redraw
            }
            Handled::Consumed => Reaction::Redraw,
            // Escape always resolves to the cancel button inside
            // `FormDialog::handle_event` (see its "Focus model" doc
            // section), so this arm never actually sees Escape.
            Handled::Ignored => Reaction::Continue,
        }
    }
}
