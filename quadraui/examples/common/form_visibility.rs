//! Backend-agnostic Form demo showcasing `FormController::set_visibility`:
//! declarative `visible_when` / `enabled_when` conditions
//! (`quadraui::visibility`) drive which fields are shown and which are
//! enabled, re-evaluated automatically on every `FormController::set_form`
//! call — no app-side rebuild of the field list per keystroke.
//!
//! Single [`AppLogic`] impl drives both the TUI runner and the GTK
//! runner. The thin shells in `examples/{tui,gtk}_form_visibility.rs` are
//! each ~10 lines.
//!
//! Shape: a tiny checkout panel —
//! - Toggle `ship`: "Ship to a different address"
//! - TextInput `address`: "Shipping address" — a member of `Form::fields`
//!   only while `ship` is on (`visible_when: ship == true`); otherwise
//!   `apply_visibility` drops it from the list entirely, so it is not
//!   painted, not a Tab stop, and not hit-testable.
//! - Toggle `agree`: "I agree to the terms"
//! - Button `submit`: "Submit" — rendered with `disabled: true` (muted
//!   style, see `tui::form::draw_field_button`) whenever `agree` is off
//!   (`enabled_when: agree == true`). `FormController`'s click routing
//!   still emits `FormEvent::ButtonClicked` for a disabled field (the
//!   same posture a disabled toolbar button takes — see
//!   `form_controller`'s `click_toolbar_disabled_button_still_emits_toolbar_button_clicked`
//!   test), so `handle` below checks `self.agree` itself before honoring
//!   the click, rather than relying on the framework to swallow it.
//!
//! Controls:
//! - mouse click on a toggle → flip value; visibility/enablement
//!   re-evaluates against the new value before the next render
//! - mouse click on Submit  → logged to the status bar only while
//!   enabled; ignored (and logged as ignored) while disabled
//! - `Tab` / `Shift+Tab`    → move focus between the currently visible
//!   fields only
//! - `q` / `Esc`            → quit

use quadraui::{
    expr::{Expr, Value},
    visibility::{FieldVisibility, FormVisibility},
    AppLogic, Backend, Color, FieldKind, Form, FormController, FormControllerEvent, FormEvent,
    FormField, InteractionState, Reaction, Rect, StatusBar, StatusBarSegment, StyledText, UiEvent,
    WidgetId,
};

pub struct FormVisibilityApp {
    fc: FormController,
    ship: bool,
    address: String,
    agree: bool,
    submit_count: u32,
    focused: Option<WidgetId>,
    last_action: String,
}

impl FormVisibilityApp {
    pub fn new() -> Self {
        let mut fc = FormController::new("checkout".into());
        fc.set_has_focus(true);

        let mut rules = FormVisibility::default();
        rules.fields.insert(
            "address".to_string(),
            FieldVisibility {
                visible_when: Some(Expr::Eq(
                    Box::new(Expr::Field("ship".into())),
                    Box::new(Expr::Lit(Value::Bool(true))),
                )),
                enabled_when: None,
            },
        );
        rules.fields.insert(
            "submit".to_string(),
            FieldVisibility {
                visible_when: None,
                enabled_when: Some(Expr::Eq(
                    Box::new(Expr::Field("agree".into())),
                    Box::new(Expr::Lit(Value::Bool(true))),
                )),
            },
        );
        // Declared once, up front: `fc.set_form` re-evaluates this same
        // `FormVisibility` against whatever `self.ship`/`self.agree` are
        // at the time, on every call — see `compose::form_controller`'s
        // "Declarative visibility" doc section.
        fc.set_visibility(rules);

        let mut app = Self {
            fc,
            ship: false,
            address: String::new(),
            agree: false,
            submit_count: 0,
            focused: Some(WidgetId::new("ship")),
            last_action: "—".into(),
        };
        // Seed the controller's cached form immediately: the runner's
        // first paint happens before any `UiEvent` reaches `handle`, so
        // without this the first frame would render an empty form area.
        app.fc.set_form(app.build_form());
        app
    }

    fn build_form(&self) -> Form {
        let fields = vec![
            FormField {
                id: WidgetId::new("ship"),
                label: StyledText::plain("Ship to a different address"),
                kind: FieldKind::Toggle { value: self.ship },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
            },
            FormField {
                id: WidgetId::new("address"),
                label: StyledText::plain("Shipping address"),
                kind: FieldKind::TextInput {
                    value: self.address.clone(),
                    placeholder: "123 Main St…".into(),
                    cursor: None,
                    selection_anchor: None,
                },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
            },
            FormField {
                id: WidgetId::new("agree"),
                label: StyledText::plain("I agree to the terms"),
                kind: FieldKind::Toggle { value: self.agree },
                hint: StyledText::default(),
                disabled: false,
                validation: None,
            },
            FormField {
                id: WidgetId::new("submit"),
                label: StyledText::plain("Submit"),
                kind: FieldKind::Button,
                hint: StyledText::default(),
                // Always built as enabled: `FormController::set_form`
                // runs `apply_visibility` right after this, which writes
                // the real value from `enabled_when`.
                disabled: false,
                validation: None,
            },
        ];

        Form {
            id: WidgetId::new("checkout-form"),
            fields,
            focused_field: self.focused,
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
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" submits={} | {} ", self.submit_count, self.last_action),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " Tab focus / click / q ".into(),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for FormVisibilityApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for FormVisibilityApp {
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
                    FormEvent::ToggleChanged { ref id, value } if id.as_str() == "ship" => {
                        self.ship = value;
                        self.last_action = format!("ship = {value}");
                    }
                    FormEvent::ToggleChanged { ref id, value } if id.as_str() == "agree" => {
                        self.agree = value;
                        self.last_action = format!("agree = {value}");
                    }
                    FormEvent::TextInputChanged { id, value } if id.as_str() == "address" => {
                        self.address = value;
                        self.last_action = format!("address = {:?}", self.address);
                    }
                    FormEvent::ButtonClicked { ref id } if id.as_str() == "submit" => {
                        // `FormController` routes the click regardless of
                        // `disabled` (same posture a disabled toolbar
                        // button takes); it is this app's job to honor
                        // `enabled_when`'s outcome, which it already knows
                        // via `self.agree`.
                        if self.agree {
                            self.submit_count += 1;
                            self.last_action = "submitted".into();
                        } else {
                            self.last_action = "submit ignored (disabled)".into();
                        }
                    }
                    FormEvent::FocusChanged { id } => {
                        self.last_action = format!("focus → {}", id.as_str());
                        self.focused = Some(id);
                    }
                    _ => {
                        self.last_action = format!("{fe:?}");
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
