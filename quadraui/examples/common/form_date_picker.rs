//! TUI Form demo showcasing `FieldKind::DatePicker` hosted in a
//! [`ScrolledForm`].
//!
//! [`AppLogic`] impl driving the TUI runner; the thin shell in
//! `examples/tui_form_date_picker.rs` is ~10 lines. TUI-only for now;
//! a GTK twin can follow once GTK's `DatePicker` rasteriser exists.
//!
//! Shape: a one-field form holding a `DatePicker`. The form forwards
//! arrow/page keys straight into the embedded picker's own `navigate`
//! method (see `primitives::form::contract`'s "Date picker fields" doc
//! section) — the app never decodes a key itself. Moving
//! the focused day only scrubs an in-flight cursor; `Enter` confirms it,
//! which is when `FormEvent::DateChanged` fires and the app persists the
//! new value.
//!
//! Controls:
//! - `Left` / `Right`  → move focused day by one
//! - `Up` / `Down`     → move focused day by one week
//! - `PageUp`/`PageDn` → move focused day by one month
//! - `Enter`           → confirm the focused day
//! - `Esc` / `q`       → quit

use quadraui::DrawsPrimitives;
use quadraui::{
    AppLogic, Backend, CalendarDate, Color, FieldKind, Form, FormField, FormState, Handled,
    Reaction, Rect, ScrolledForm, StatusBar, StatusBarSegment, StyledText, UiEvent, WidgetId,
};

pub struct FormDatePickerApp {
    form_state: FormState,
    focused: CalendarDate,
    confirmed: Option<CalendarDate>,
    last_action: String,
}

impl FormDatePickerApp {
    pub fn new() -> Self {
        // This demo has exactly one panel, so every key goes to the form.
        Self {
            form_state: FormState::default(),
            focused: CalendarDate::new(2024, 3, 15),
            confirmed: None,
            last_action: "—".into(),
        }
    }

    fn build_form(&self) -> Form {
        let mut picker = quadraui::DatePicker::new(WidgetId::new("due-date"), self.focused);
        picker.selected = self.confirmed;
        picker.has_focus = true;

        Form {
            id: WidgetId::new("due-date-form"),
            fields: vec![FormField {
                id: WidgetId::new("due-date"),
                label: StyledText::plain("Due date"),
                kind: FieldKind::DatePicker(picker),
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
            focused_field: Some(WidgetId::new("due-date")),
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
        let confirmed = self
            .confirmed
            .map(|d| d.format(quadraui::DateFormat::Iso8601))
            .unwrap_or_else(|| "none".into());
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" confirmed={} | {} ", confirmed, self.last_action),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " arrows / PageUp / PageDown / Enter / q ".into(),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for FormDatePickerApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for FormDatePickerApp {
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
                key: quadraui::Key::Named(quadraui::NamedKey::Escape),
                ..
            } => return Reaction::Exit,
            UiEvent::KeyPressed {
                key: quadraui::Key::Char('q'),
                ..
            } => return Reaction::Exit,
            UiEvent::WindowResized { .. } => return Reaction::Redraw,
            _ => {}
        }

        let rect = Self::form_rect(backend);
        let form = ScrolledForm::new(self.build_form());
        match form.handle_event(&mut self.form_state, &event, backend, rect) {
            Handled::Event(fe) => {
                match fe {
                    quadraui::FormEvent::DateChanged { value, .. } => {
                        self.focused = value;
                        self.confirmed = Some(value);
                        self.last_action =
                            format!("confirmed {}", value.format(quadraui::DateFormat::Iso8601));
                    }
                    _ => {
                        self.last_action = format!("{:?}", fe);
                    }
                }
                Reaction::Redraw
            }
            Handled::Consumed => Reaction::Redraw,
            Handled::Ignored => Reaction::Continue,
        }
    }
}
