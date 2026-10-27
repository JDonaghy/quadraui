//! Backend-agnostic [`Form`] demo showcasing `FieldKind::ComboBox`
//! routed through `FormController`.
//!
//! `Form`/`FormController` paint only the compact one-row field (live
//! text + a chevron); the open, filtered dropdown is this app's own
//! popup overlay, mirroring `FieldKind::ColorPicker`'s contract. This
//! demo's `render` is the reference implementation for that overlay:
//! it recomputes `ComboBox::layout` itself and calls
//! `Backend::draw_combo_box` to paint the dropdown on top of the field
//! `FormController` already rendered.
//!
//! `FormController` forwards every key straight into the embedded
//! combo box and translates the result into one of the four
//! `FormEvent::ComboBox*` events — this app never decodes a key
//! itself, it only *applies* the resulting event per `ComboBox`'s
//! apply contract (see `FormEvent`'s doc comments).
//!
//! Controls:
//! - type any letter → filters the dropdown, opens it
//! - `Up` / `Down`    → move the highlighted option
//! - `Enter`          → commit the highlighted (or typed) value
//! - `Esc`            → cancel the edit, revert to the committed value
//! - `q`              → quit

use quadraui::{
    AppLogic, Backend, Color, ComboBox, CompletionItemMeasure, FieldKind, Form, FormController,
    FormControllerEvent, FormEvent, FormField, InteractionState, Key, NamedKey, Reaction, Rect,
    StatusBar, StatusBarSegment, StyledText, UiEvent, WidgetId,
};

pub struct FormComboBoxApp {
    fc: FormController,
    combo: ComboBox,
    last_action: String,
}

impl FormComboBoxApp {
    pub fn new() -> Self {
        let mut fc = FormController::new("fruit-form".into());
        fc.set_has_focus(true);
        let combo = ComboBox::new(
            WidgetId::new("fruit-combo"),
            vec![
                "Apple".into(),
                "Apricot".into(),
                "Banana".into(),
                "Cherry".into(),
                "Cranberry".into(),
            ],
        );
        let mut app = Self {
            fc,
            combo,
            last_action: "—".into(),
        };
        app.fc.set_form(app.build_form());
        app
    }

    fn build_form(&self) -> Form {
        Form {
            id: WidgetId::new("fruit-form"),
            fields: vec![FormField {
                id: WidgetId::new("fruit"),
                label: StyledText::plain("Fruit"),
                kind: FieldKind::ComboBox(self.combo.clone()),
                hint: StyledText::default(),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
            }],
            focused_field: Some(WidgetId::new("fruit")),
            scroll_offset: 0,
            has_focus: true,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
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

    /// The combo box's own field row, one row below the form origin —
    /// matches the single-field `Form`'s own layout exactly, since
    /// `FormController` never exposes a per-field rect accessor.
    fn field_bounds(backend: &dyn Backend) -> Rect {
        let form_rect = Self::form_rect(backend);
        Rect::new(
            form_rect.x,
            form_rect.y,
            form_rect.width,
            backend.line_height(),
        )
    }

    fn build_status_bar(&self) -> StatusBar {
        let fg = Color::rgb(220, 220, 220);
        let bg = Color::rgb(40, 40, 60);
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" fruit={} | {} ", self.combo.committed, self.last_action),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " type / up / down / Enter / Esc / q ".into(),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for FormComboBoxApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for FormComboBoxApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let form_rect = Self::form_rect(backend);
        let status_rect = Self::status_rect(backend);
        self.fc.render(backend, form_rect);

        if self.combo.open {
            let field_bounds = Self::field_bounds(backend);
            let vp = backend.viewport();
            let viewport = Rect::new(0.0, 0.0, vp.width, vp.height);
            let line_height = backend.line_height();
            let layout = self.combo.layout(
                field_bounds,
                viewport,
                field_bounds.width,
                line_height * 8.0,
                |_idx| CompletionItemMeasure {
                    height: line_height,
                },
            );
            backend.draw_combo_box(&self.combo, &layout);
        }

        let _hits = backend.draw_status_bar_interactive(
            status_rect,
            &self.build_status_bar(),
            &InteractionState::new(),
        );
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match &event {
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } if !self.combo.open => return Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            } if !self.combo.open => return Reaction::Exit,
            UiEvent::WindowResized { .. } => return Reaction::Redraw,
            _ => {}
        }

        self.fc.set_form(self.build_form());
        self.fc.set_backend_info(backend.line_height());
        let rect = Self::form_rect(backend);
        match self.fc.handle_cached(&event, rect) {
            FormControllerEvent::FormAction(fe) => {
                match fe {
                    FormEvent::ComboBoxTextChanged { value, .. } => {
                        self.combo.text = value.clone();
                        self.combo.open = true;
                        self.last_action = format!("typing {value:?}");
                    }
                    FormEvent::ComboBoxHighlightChanged { index, .. } => {
                        self.combo.highlighted = Some(index);
                        self.last_action = format!("highlight {index}");
                    }
                    FormEvent::ComboBoxCommitted { value, .. } => {
                        self.combo.committed = value.clone();
                        self.combo.text = value.clone();
                        self.combo.open = false;
                        self.last_action = format!("committed {value:?}");
                    }
                    FormEvent::ComboBoxCancelled { .. } => {
                        self.combo.text = self.combo.committed.clone();
                        self.combo.open = false;
                        self.last_action = "cancelled".into();
                    }
                    other => {
                        self.last_action = format!("{other:?}");
                    }
                }
                self.fc.set_form(self.build_form());
                Reaction::Redraw
            }
            FormControllerEvent::ScrollChanged | FormControllerEvent::Consumed => Reaction::Redraw,
            FormControllerEvent::Ignored => Reaction::Continue,
        }
    }
}
