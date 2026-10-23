//! Backend-agnostic Form demo showcasing `FieldKind::DatePicker` routed
//! through `FormController`.
//!
//! Single [`AppLogic`] impl drives both the TUI runner and the GTK
//! runner. The thin shells in `examples/{tui,gtk}_form_date_picker.rs`
//! are each ~10 lines.
//!
//! Shape: a one-field form holding a `DatePicker`. `FormController`
//! forwards arrow/page keys straight into the embedded picker's own
//! `navigate` method (see `compose::form_controller`'s "Date picker
//! fields" doc section) — the app never decodes a key itself. Moving
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

use quadraui::{
    AppLogic, Backend, CalendarDate, Color, FieldKind, Form, FormController, FormControllerEvent,
    FormField, InteractionState, Reaction, Rect, StatusBar, StatusBarSegment, StyledText, UiEvent,
    WidgetId,
};

pub struct FormDatePickerApp {
    fc: FormController,
    focused: CalendarDate,
    confirmed: Option<CalendarDate>,
    last_action: String,
}

impl FormDatePickerApp {
    pub fn new() -> Self {
        let mut fc = FormController::new("due-date-form".into());
        // This demo has exactly one panel, so the form always has
        // keyboard focus — `set_has_focus` is what lets
        // `FormController::handle_cached` accept `KeyPressed` at all.
        fc.set_has_focus(true);
        let mut app = Self {
            fc,
            focused: CalendarDate::new(2024, 3, 15),
            confirmed: None,
            last_action: "—".into(),
        };
        // Seed `fc` with the initial form so the very first `render()`
        // (before any key is ever pressed) already paints the picker,
        // rather than relying on `handle()` to populate it.
        app.fc.set_form(app.build_form());
        app
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
            }],
            focused_field: Some(WidgetId::new("due-date")),
            scroll_offset: 0,
            has_focus: true,
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
        self.fc.render(backend, form_rect);
        let _hits = backend.draw_status_bar_interactive(
            status_rect,
            &self.build_status_bar(),
            &InteractionState::new(),
        );
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

        self.fc.set_form(self.build_form());
        self.fc.set_backend_info(backend.line_height());
        let rect = Self::form_rect(backend);
        match self.fc.handle_cached(&event, rect) {
            FormControllerEvent::FormAction(fe) => {
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
                // Re-sync `fc`'s cached form with the just-updated
                // `self.focused`/`self.confirmed` before the runner's
                // next `render()` call. Without this, `render` would
                // paint the field as it was *before* this event
                // (`fc.set_form` above ran with the pre-event state),
                // leaving the field one keystroke behind what the
                // status bar already reports.
                self.fc.set_form(self.build_form());
                Reaction::Redraw
            }
            FormControllerEvent::ScrollChanged | FormControllerEvent::Consumed => Reaction::Redraw,
            FormControllerEvent::Ignored => Reaction::Continue,
        }
    }
}
