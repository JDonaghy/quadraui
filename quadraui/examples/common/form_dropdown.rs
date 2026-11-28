//! Backend-agnostic [`Form`] demo showcasing `FieldKind::Dropdown`
//! hosted in a [`ScrolledForm`].
//!
//! Unlike `FieldKind::ComboBox` (whose open/filtered popup is drawn by
//! the *app*, via `Backend::draw_combo_box` — see
//! `examples/common/form_combo_box.rs`), a `Dropdown`'s popup is drawn
//! by the TUI backend itself the moment the field is focused: this
//! app's `render` is just `ScrolledForm::draw`, nothing extra.
//!
//! The form claims `Up`/`Down` on a focused, enabled `Dropdown`
//! and emits `FormEvent::DropdownChanged` immediately on every move —
//! there's no separate open/commit step, so this app applies the new
//! `selected_idx` as soon as the event arrives.
//!
//! Controls:
//! - `Up` / `Down` → move the selection (and the popup's highlight)
//! - `q` / `Esc`   → quit

use quadraui::DrawsPrimitives;
use quadraui::{
    AppLogic, Backend, Color, FieldKind, Form, FormEvent, FormField, FormState, Handled, Key,
    NamedKey, Reaction, Rect, ScrolledForm, StatusBar, StatusBarSegment, StyledText, UiEvent,
    WidgetId,
};

pub struct FormDropdownApp {
    form_state: FormState,
    options: Vec<String>,
    selected_idx: usize,
    last_action: String,
}

impl FormDropdownApp {
    pub fn new() -> Self {
        Self {
            form_state: FormState::default(),
            options: vec![
                "One Dark".into(),
                "Solarized Light".into(),
                "Gruvbox".into(),
            ],
            selected_idx: 0,
            last_action: "—".into(),
        }
    }

    fn build_form(&self) -> Form {
        Form {
            id: WidgetId::new("theme-form"),
            fields: vec![FormField {
                id: WidgetId::new("theme"),
                label: StyledText::plain("Theme"),
                kind: FieldKind::Dropdown {
                    options: self.options.iter().map(StyledText::plain).collect(),
                    selected_idx: self.selected_idx,
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
            }],
            focused_field: Some(WidgetId::new("theme")),
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

    fn form_rect(backend: &dyn Backend) -> Rect {
        let vp = backend.viewport();
        let status_h = backend.line_height() * 1.5;
        Rect::new(0.0, 0.0, vp.width, (vp.height - status_h).max(0.0))
    }

    fn status_rect(backend: &dyn Backend) -> Rect {
        let vp = backend.viewport();
        let status_h = backend.line_height() * 1.5;
        Rect::new(0.0, (vp.height - status_h).max(0.0), vp.width, status_h)
    }

    fn build_status_bar(&self) -> StatusBar {
        let fg = Color::rgb(220, 220, 220);
        let bg = Color::rgb(40, 40, 60);
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(
                    " theme={} | {} ",
                    self.options[self.selected_idx], self.last_action
                ),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " up / down / q ".into(),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for FormDropdownApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for FormDropdownApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let form_rect = Self::form_rect(backend);
        let status_rect = Self::status_rect(backend);
        ScrolledForm::new(self.build_form()).draw(&self.form_state, backend, form_rect);
        let _hits = backend.draw_dyn(status_rect, &self.build_status_bar(), &());
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match &event {
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape) | Key::Char('q'),
                ..
            } => return Reaction::Exit,
            UiEvent::WindowResized { .. } => return Reaction::Redraw,
            _ => {}
        }

        let rect = Self::form_rect(backend);
        let form = ScrolledForm::new(self.build_form());
        match form.handle_event(&mut self.form_state, &event, backend, rect) {
            Handled::Event(FormEvent::DropdownChanged { selected_idx, .. }) => {
                self.selected_idx = selected_idx;
                self.last_action = format!("selected {}", self.options[selected_idx]);
                Reaction::Redraw
            }
            Handled::Event(other) => {
                self.last_action = format!("{other:?}");
                Reaction::Redraw
            }
            Handled::Consumed => Reaction::Redraw,
            Handled::Ignored => Reaction::Continue,
        }
    }
}
