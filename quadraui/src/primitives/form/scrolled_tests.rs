//! Behaviour tests for [`super::ScrolledForm`] and the [`Form`] it hosts,
//! driven the way a raw-event host drives them: the test [`Host`] keeps
//! the form descriptor and its [`FormState`], applies the host's own
//! duties (keyboard-focus gating, declarative validation and
//! visibility), and routes each [`UiEvent`] through the widget's
//! [`Interactive`] against the layout it paints.

use super::*;
use crate::event::{Key, NamedKey};
use crate::primitives::combo_box::ComboBox;
use crate::primitives::date_picker::CalendarDate;
use crate::primitives::form::contract::form_click_event;
use crate::primitives::form::contract::slider_track_rect;
use crate::primitives::form::{FieldKind, FormField, ToggleGroupItem};
use crate::primitives::number_input::NumberInput;
use crate::primitives::radio_group::{RadioGroup, RadioOption};
use crate::types::{Modifiers, StyledText};
use crate::validation::{mark_required, validate, FormRules};
use crate::visibility::{apply_visibility, FormVisibility};
use crate::{ButtonMask, Theme};

/// A raw-event host of one [`ScrolledForm`]: the form the app last
/// built, the widget's live [`FormState`], and the host's own duties
/// around it.
struct Host {
    form: Option<Form>,
    state: FormState,
    /// The backend line height the layout is resolved with when no
    /// backend is in reach; `None` until [`Self::set_backend_info`].
    lh: Option<f32>,
    /// Whether this form is the active panel: keys reach it only then.
    has_focus: bool,
    rules: FormRules,
    visibility: FormVisibility,
}

impl Host {
    fn new(_id: String) -> Self {
        Self {
            form: None,
            state: FormState::default(),
            lh: None,
            has_focus: false,
            rules: FormRules::default(),
            visibility: FormVisibility::default(),
        }
    }

    /// Store the app's freshly built `form`, after the host's declarative
    /// validation and visibility.
    fn set_form(&mut self, mut form: Form) {
        validate(&mut form, &self.rules);
        apply_visibility(&mut form, &self.visibility);
        self.form = Some(form);
    }

    fn set_rules(&mut self, rules: FormRules) {
        self.rules = rules;
    }

    fn rules(&self) -> &FormRules {
        &self.rules
    }

    fn set_visibility(&mut self, visibility: FormVisibility) {
        self.visibility = visibility;
    }

    fn visibility(&self) -> &FormVisibility {
        &self.visibility
    }

    fn form(&self) -> Option<&Form> {
        self.form.as_ref()
    }

    fn field_count(&self) -> usize {
        self.form.as_ref().map_or(0, |f| f.fields.len())
    }

    fn set_has_focus(&mut self, has_focus: bool) {
        self.has_focus = has_focus;
    }

    fn has_focus(&self) -> bool {
        self.has_focus
    }

    fn set_backend_info(&mut self, line_height: f32) {
        self.lh = Some(line_height);
    }

    fn scroll_offset(&self) -> usize {
        self.form
            .as_ref()
            .map_or(0, |f| self.state.live_scroll_offset(f))
    }

    fn set_scroll_offset(&mut self, offset: usize) {
        self.state.scroll_offset = Some(offset);
    }

    fn scroll_by(&mut self, delta: isize, viewport_rows: usize) {
        if let Some(form) = &self.form {
            self.state.scroll_by(form, delta, viewport_rows);
        }
    }

    fn page_scroll(&mut self, delta: isize, viewport_rows: usize) {
        self.scroll_by(delta, viewport_rows);
    }

    fn scroll_to_field(&mut self, field_idx: usize, viewport_rows: usize) {
        if let Some(form) = &self.form {
            self.state.scroll_to_field(form, field_idx, viewport_rows);
        }
    }

    /// The form as the widget paints it: the live state overlaid, the
    /// host's focus, and the required-field markers.
    fn build_form(&self, _rect: Rect, theme: Option<&Theme>) -> Form {
        let mut form = match &self.form {
            Some(f) => f.live(Some(&self.state), false).into_owned(),
            None => Form {
                id: WidgetId::new("form"),
                fields: Vec::new(),
                focused_field: None,
                scroll_offset: 0,
                has_focus: false,
                label_position: Default::default(),
                label_align: Default::default(),
                label_width: None,
                control_width: Default::default(),
                min_control_width: None,
                spacing: Default::default(),
            },
        };
        form.has_focus = self.has_focus;
        mark_required(&mut form, &self.rules, theme.map(|t| t.error_fg));
        form
    }

    fn widget(&self) -> ScrolledForm {
        ScrolledForm::new(self.build_form(Rect::default(), None))
    }

    /// Whether `event` is keyboard input, which reaches the form only
    /// while it is the active panel.
    fn is_keyboard(event: &UiEvent) -> bool {
        matches!(
            event,
            UiEvent::KeyPressed { .. } | UiEvent::CharTyped(_) | UiEvent::ClipboardPaste(_)
        )
    }

    /// Route `event` through the widget laid out at `rect` with the
    /// cached line height — a cell grid at `1.0`, a pixel surface with
    /// `0.6`-line-height glyphs above it.
    fn handle_cached(&mut self, event: &UiEvent, rect: Rect) -> Handled<FormEvent> {
        let Some(lh) = self.lh else {
            return Handled::Ignored;
        };
        if self.form.is_none() || (Self::is_keyboard(event) && !self.has_focus) {
            return Handled::Ignored;
        }
        if let UiEvent::MouseDown { position, .. } = event {
            if !rect.contains(*position) {
                self.state.slider_drag = None;
                self.state.thumb_drag = None;
                return Handled::Ignored;
            }
        }
        let char_w = if lh <= 1.0 { 1.0 } else { lh * 0.6 };
        let ctx = LayoutCtx::new(lh, char_w, false);
        let widget = self.widget();
        let layout =
            ScrolledForm::layout_live(&widget.form, self.state.thumb_drag.is_some(), &ctx, rect);
        let subs = Interactive::sub_regions(&widget, &layout);
        let Some(routed) = widget_host::routed_event(event, rect, &subs) else {
            return Handled::Ignored;
        };
        let id = widget.form.id;
        widget_host::drive(&widget, id, &mut self.state, &routed, &layout, ctx.units()).handled
    }

    /// Route `event` through the widget as `backend` paints it at
    /// `rect`.
    fn handle(
        &mut self,
        event: &UiEvent,
        backend: &mut dyn Backend,
        rect: Rect,
    ) -> Handled<FormEvent> {
        if self.form.is_none() || (Self::is_keyboard(event) && !self.has_focus) {
            return Handled::Ignored;
        }
        self.widget()
            .handle_event(&mut self.state, event, backend, rect)
    }

    fn render(&self, backend: &mut dyn Backend, rect: Rect) {
        self.widget().draw(&self.state, backend, rect);
    }

    fn field_rect(&self, id: WidgetId, backend: &dyn Backend, rect: Rect) -> Option<Rect> {
        self.form.as_ref()?;
        self.widget()
            .layout_at(&self.state, backend, rect)
            .field_rect(&id)
    }
}

fn make_fields(n: usize) -> Vec<FormField> {
    (0..n)
        .map(|i| FormField {
            id: WidgetId::new(format!("field-{i}")),
            label: StyledText::plain(format!("Field {i}")),
            kind: FieldKind::Toggle { value: false },
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
        })
        .collect()
}

fn make_form(n: usize) -> Form {
    Form {
        id: WidgetId::new("test-form"),
        fields: make_fields(n),
        focused_field: None,
        scroll_offset: 0,
        has_focus: false,
        label_position: Default::default(),
        label_align: Default::default(),
        label_width: None,
        control_width: Default::default(),
        min_control_width: None,
        spacing: Default::default(),
    }
}

fn test_controller(field_count: usize) -> Host {
    let mut fc = Host::new("test".into());
    fc.set_form(make_form(field_count));
    fc
}

// ── Accessors ────────────────────────────────────────────────────

#[test]
fn new_starts_empty() {
    let fc = Host::new("fc".into());
    assert_eq!(fc.scroll_offset(), 0);
    assert!(!fc.has_focus());
    assert!(fc.form().is_none());
    assert_eq!(fc.field_count(), 0);
}

#[test]
fn set_form_and_read_back() {
    let mut fc = Host::new("fc".into());
    fc.set_form(make_form(5));
    assert!(fc.form().is_some());
    assert_eq!(fc.field_count(), 5);
}

#[test]
fn set_rules_validates_on_every_set_form_call() {
    use crate::validation::{Rule, Severity, Validator};

    let mut fc = Host::new("fc".into());
    let mut rules = FormRules::default();
    rules.fields.insert(
        "field-0".to_string(),
        vec![Rule {
            check: Validator::Required,
            message: "Required".to_string(),
            severity: Severity::Error,
        }],
    );
    fc.set_rules(rules);
    assert_eq!(fc.rules().fields.len(), 1);

    // Empty text field fails `Required`.
    let mut form = make_form(1);
    form.fields[0].kind = FieldKind::TextInput {
        value: String::new(),
        placeholder: String::new(),
        cursor: None,
        selection_anchor: None,
    };
    fc.set_form(form);
    assert_eq!(
        fc.form().unwrap().fields[0].validation,
        Some(crate::primitives::form::ValidationState::Error(
            "Required".to_string()
        ))
    );

    // Filling it in and pushing the next frame clears the error with no
    // host-side validation call.
    let mut form = make_form(1);
    form.fields[0].kind = FieldKind::TextInput {
        value: "Ada".to_string(),
        placeholder: String::new(),
        cursor: Some(3),
        selection_anchor: None,
    };
    fc.set_form(form);
    assert_eq!(fc.form().unwrap().fields[0].validation, None);

    // A field with no declared rules is left untouched even when the
    // controller has other rules set.
    let mut form = make_form(2);
    form.fields[1].validation = Some(crate::primitives::form::ValidationState::Warning(
        "host-set".to_string(),
    ));
    fc.set_form(form);
    assert_eq!(
        fc.form().unwrap().fields[1].validation,
        Some(crate::primitives::form::ValidationState::Warning(
            "host-set".to_string()
        ))
    );
}

#[test]
fn blurring_a_required_field_populates_validation_without_manual_validate_call() {
    use crate::validation::{Rule, Severity, Validator};

    fn name_email_form(name_value: &str, focused: &str) -> Form {
        Form {
            id: WidgetId::new("form"),
            fields: vec![
                FormField {
                    id: WidgetId::new("name"),
                    label: StyledText::plain("Name"),
                    kind: FieldKind::TextInput {
                        value: name_value.to_string(),
                        placeholder: String::new(),
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
                text_field("email", "", None),
            ],
            focused_field: Some(WidgetId::new(focused)),
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

    let mut fc = Host::new("f".into());
    fc.set_form(name_email_form("", "name"));
    fc.set_has_focus(true);
    fc.set_backend_info(1.0);
    let mut rules = FormRules::default();
    rules.fields.insert(
        "name".to_string(),
        vec![Rule {
            check: Validator::Required,
            message: "Required".to_string(),
            severity: Severity::Error,
        }],
    );
    fc.set_rules(rules);

    // Tab away from "name" — the controller reports the focus move; the
    // host persists it the same way the module doc's "Keyboard editing"
    // section describes, by feeding the new `focused_field` back on the
    // next `set_form` call. Nothing here calls `validate()` directly.
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::FocusChanged {
            id: WidgetId::new("email")
        }),
        "tab should blur \"name\" and focus \"email\""
    );
    fc.set_form(name_email_form("", "email"));
    assert_eq!(
        fc.form().unwrap().fields[0].validation,
        Some(crate::primitives::form::ValidationState::Error(
            "Required".to_string()
        )),
        "blurring the empty required field should populate its \
             validation state without a manual validate() call"
    );

    // Filling the field in and pushing the next frame (still away from
    // "name") clears the error, again with no manual validate() call.
    fc.set_form(name_email_form("Ada", "email"));
    assert_eq!(
        fc.form().unwrap().fields[0].validation,
        None,
        "a now-passing field should have its validation state cleared"
    );

    // A controller with no `FormRules` set never shows a validation
    // state after the same focus/blur sequence — the inert path.
    let mut fc_no_rules = Host::new("f2".into());
    fc_no_rules.set_form(name_email_form("", "name"));
    fc_no_rules.set_has_focus(true);
    fc_no_rules.set_backend_info(1.0);
    let _ = fc_no_rules.handle_cached(
        &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
        RECT,
    );
    fc_no_rules.set_form(name_email_form("", "email"));
    assert_eq!(
        fc_no_rules.form().unwrap().fields[0].validation,
        None,
        "a form with no FormRules shows no validation state after blur"
    );
}

#[test]
fn set_visibility_applies_on_every_set_form_call() {
    use crate::expr::{Expr, Value};
    use crate::visibility::{FieldVisibility, FormVisibility};

    let mut fc = Host::new("fc".into());
    let mut visibility = FormVisibility::default();
    visibility.fields.insert(
        "field-1".to_string(),
        FieldVisibility {
            visible_when: Some(Expr::Eq(
                Box::new(Expr::Field("field-0".into())),
                Box::new(Expr::Lit(Value::Bool(true))),
            )),
            enabled_when: None,
        },
    );
    fc.set_visibility(visibility);
    assert_eq!(fc.visibility().fields.len(), 1);

    // `field-0` toggled off hides `field-1` from the pushed form.
    let form = make_form(2);
    fc.set_form(form);
    assert_eq!(fc.field_count(), 1);

    // Flipping `field-0` back on and pushing the next frame restores
    // `field-1` with no host-side rebuild of the field list.
    let mut form = make_form(2);
    form.fields[0].kind = FieldKind::Toggle { value: true };
    fc.set_form(form);
    assert_eq!(fc.field_count(), 2);
}

#[test]
fn set_has_focus() {
    let mut fc = Host::new("fc".into());
    fc.set_has_focus(true);
    assert!(fc.has_focus());
}

// ── Scroll-by ───────────────────────────────────────────────────

#[test]
fn scroll_by_advances_offset() {
    let mut fc = test_controller(20);
    fc.scroll_by(3, 5);
    assert_eq!(fc.scroll_offset(), 3);
}

#[test]
fn scroll_by_retreats_offset() {
    let mut fc = test_controller(20);
    fc.set_scroll_offset(5);
    fc.scroll_by(-2, 5);
    assert_eq!(fc.scroll_offset(), 3);
}

#[test]
fn scroll_by_clamps_to_bounds() {
    let mut fc = test_controller(10);
    fc.scroll_by(100, 5);
    assert_eq!(fc.scroll_offset(), 5); // 10 - 5
    fc.scroll_by(-100, 5);
    assert_eq!(fc.scroll_offset(), 0);
}

// ── Page scroll ─────────────────────────────────────────────────

#[test]
fn page_scroll_clamps() {
    let mut fc = test_controller(20);
    fc.page_scroll(100, 5);
    assert_eq!(fc.scroll_offset(), 15); // 20 - 5
    fc.page_scroll(-100, 5);
    assert_eq!(fc.scroll_offset(), 0);
}

// ── Scroll-to-field ─────────────────────────────────────────────

#[test]
fn scroll_to_field_scrolls_down() {
    let mut fc = test_controller(20);
    fc.scroll_to_field(8, 5);
    assert!(fc.scroll_offset() + 5 > 8);
    assert!(fc.scroll_offset() <= 8);
}

#[test]
fn scroll_to_field_scrolls_up() {
    let mut fc = test_controller(20);
    fc.set_scroll_offset(10);
    fc.scroll_to_field(3, 5);
    assert_eq!(fc.scroll_offset(), 3);
}

#[test]
fn scroll_to_field_noop_when_visible() {
    let mut fc = test_controller(20);
    fc.set_scroll_offset(5);
    fc.scroll_to_field(7, 5);
    assert_eq!(fc.scroll_offset(), 5);
}

#[test]
fn scroll_to_field_noop_with_zero_viewport() {
    let mut fc = test_controller(20);
    fc.scroll_to_field(10, 0);
    assert_eq!(fc.scroll_offset(), 0);
}

// ── field_rect ───────────────────────────────────────────────────
//
// Drives a real `TuiBackend` (not a hand-rolled mock — see
// `context_menu_controller.rs`'s tests module doc for why): the one
// thing `field_rect` needs from a `Backend` is `line_height` +
// `form_layout`, both cheap and headless on `TuiBackend::new()`.

#[cfg(feature = "tui")]
mod field_rect_tests {
    use super::*;
    use crate::tui::TuiBackend;

    #[test]
    fn resolves_second_field_below_the_first() {
        let fc = test_controller(3);
        let backend = TuiBackend::new();
        let rect = Rect::new(0.0, 0.0, 40.0, 20.0);
        let r0 = fc
            .field_rect(WidgetId::new("field-0"), &backend, rect)
            .unwrap();
        let r1 = fc
            .field_rect(WidgetId::new("field-1"), &backend, rect)
            .unwrap();
        // Same column (and width) as the first field, strictly below
        // it — this is the whole point of the lookup: a multi-field
        // form's second field is *not* at the form's own rect.
        assert_eq!(r0.x, r1.x);
        assert_eq!(r0.width, r1.width);
        assert!(r1.y > r0.y);
    }

    #[test]
    fn offsets_by_the_passed_rect_origin() {
        let fc = test_controller(1);
        let backend = TuiBackend::new();
        let origin_rect = Rect::new(0.0, 0.0, 40.0, 20.0);
        let shifted_rect = Rect::new(5.0, 7.0, 40.0, 20.0);
        let at_origin = fc
            .field_rect(WidgetId::new("field-0"), &backend, origin_rect)
            .unwrap();
        let shifted = fc
            .field_rect(WidgetId::new("field-0"), &backend, shifted_rect)
            .unwrap();
        assert_eq!(shifted.x, at_origin.x + 5.0);
        assert_eq!(shifted.y, at_origin.y + 7.0);
    }

    #[test]
    fn unknown_field_id_returns_none() {
        let fc = test_controller(3);
        let backend = TuiBackend::new();
        let rect = Rect::new(0.0, 0.0, 40.0, 20.0);
        assert_eq!(
            fc.field_rect(WidgetId::new("no-such-field"), &backend, rect),
            None
        );
    }

    // The rect handed back has to be the *same* rect the controller's
    // own hit-testing resolves to that field — a host that anchors a
    // popup to it and then routes the user's next click back through
    // `handle` depends on the two agreeing. Click the centre of every
    // reported rect and assert the click lands on that field rather
    // than a neighbour.
    #[test]
    fn reported_rect_round_trips_through_hit_testing() {
        let rect = Rect::new(3.0, 2.0, 40.0, 20.0);
        let mut backend = TuiBackend::new();
        for i in 0..3 {
            let id = WidgetId::new(format!("field-{i}"));
            let mut fc = test_controller(3);
            let field = fc.field_rect(id, &backend, rect).unwrap();
            let ev = fc.handle(
                &UiEvent::MouseDown {
                    widget: None,
                    button: MouseButton::Left,
                    position: crate::Point {
                        x: field.x + field.width / 2.0,
                        y: field.y + field.height / 2.0,
                    },
                    modifiers: Default::default(),
                },
                &mut backend,
                rect,
            );
            match ev {
                Handled::Event(FormEvent::ToggleChanged { id: hit, .. }) => {
                    assert_eq!(hit, id, "field-{i}'s reported rect hit-tested elsewhere");
                }
                other => panic!("expected ToggleChanged for field-{i}, got {other:?}"),
            }
        }
    }

    #[test]
    fn empty_controller_returns_none() {
        let fc = Host::new("fc".into());
        let backend = TuiBackend::new();
        let rect = Rect::new(0.0, 0.0, 40.0, 20.0);
        assert_eq!(
            fc.field_rect(WidgetId::new("field-0"), &backend, rect),
            None
        );
    }

    #[test]
    fn scrolled_out_of_view_field_returns_none() {
        let mut fc = test_controller(20);
        // One row per field at `TuiBackend`'s line height leaves a
        // 20.0-tall viewport room for only a handful of rows — scroll
        // far enough that field-0 is no longer among the visible ones.
        fc.set_scroll_offset(15);
        let backend = TuiBackend::new();
        let rect = Rect::new(0.0, 0.0, 40.0, 20.0);
        assert_eq!(
            fc.field_rect(WidgetId::new("field-0"), &backend, rect),
            None
        );
    }
}

// ── build_form injects controller state ──────────────────────────

#[test]
fn build_form_injects_scroll_offset() {
    let mut fc = test_controller(10);
    fc.set_scroll_offset(3);
    fc.set_has_focus(true);
    let form = fc.build_form(Rect::new(0.0, 0.0, 40.0, 100.0), None);
    assert_eq!(form.scroll_offset, 3);
    assert!(form.has_focus);
}

#[test]
fn build_form_with_no_data_returns_empty() {
    let fc = Host::new("empty".into());
    let form = fc.build_form(Rect::new(0.0, 0.0, 40.0, 100.0), None);
    assert!(form.fields.is_empty());
}

// ── form_click_event ────────────────────────────────────────────

#[test]
fn click_toggle_flips_value() {
    let form = make_form(3);
    let ev = form_click_event(&form, &WidgetId::new("field-0"));
    assert_eq!(
        ev,
        FormEvent::ToggleChanged {
            id: WidgetId::new("field-0"),
            value: true
        }
    );
}

#[test]
fn click_button_emits_button_clicked() {
    let mut form = make_form(3);
    form.fields[1].kind = FieldKind::Button;
    let ev = form_click_event(&form, &WidgetId::new("field-1"));
    assert_eq!(
        ev,
        FormEvent::ButtonClicked {
            id: WidgetId::new("field-1")
        }
    );
}

#[test]
fn click_text_input_emits_focus_changed() {
    let mut form = make_form(3);
    form.fields[2].kind = FieldKind::TextInput {
        value: "hello".into(),
        placeholder: String::new(),
        cursor: None,
        selection_anchor: None,
    };
    let ev = form_click_event(&form, &WidgetId::new("field-2"));
    assert_eq!(
        ev,
        FormEvent::FocusChanged {
            id: WidgetId::new("field-2")
        }
    );
}

#[test]
fn click_unknown_id_emits_focus_changed() {
    let form = make_form(3);
    let ev = form_click_event(&form, &WidgetId::new("unknown"));
    assert_eq!(
        ev,
        FormEvent::FocusChanged {
            id: WidgetId::new("unknown")
        }
    );
}

#[test]
fn click_toolbar_button_emits_toolbar_button_clicked() {
    use crate::primitives::toolbar::{Toolbar, ToolbarButton};
    let mut form = make_form(1);
    form.fields[0].kind = FieldKind::Toolbar(Toolbar {
        id: WidgetId::new("tb"),
        buttons: vec![
            ToolbarButton::Action {
                spec: None,
                id: WidgetId::new("reset"),
                label: "Reset".into(),
                icon: None,
                key_hint: None,
                enabled: true,
                is_active: false,
                tooltip: String::new(),
            },
            ToolbarButton::Action {
                spec: None,
                id: WidgetId::new("export"),
                label: "Export".into(),
                icon: None,
                key_hint: None,
                enabled: true,
                is_active: false,
                tooltip: String::new(),
            },
        ],
        bg: None,
        focused_index: None,
        ..Default::default()
    });

    // Clicking the first button should emit ToolbarButtonClicked with
    // the owning field_id and the button_id.
    let ev = form_click_event(&form, &WidgetId::new("reset"));
    assert_eq!(
        ev,
        FormEvent::ToolbarButtonClicked {
            field_id: WidgetId::new("field-0"),
            button_id: WidgetId::new("reset"),
        },
        "clicking 'reset' should emit ToolbarButtonClicked"
    );

    // Clicking the second button should emit ToolbarButtonClicked too.
    let ev2 = form_click_event(&form, &WidgetId::new("export"));
    assert_eq!(
        ev2,
        FormEvent::ToolbarButtonClicked {
            field_id: WidgetId::new("field-0"),
            button_id: WidgetId::new("export"),
        },
        "clicking 'export' should emit ToolbarButtonClicked"
    );
}

#[test]
fn click_toolbar_disabled_button_still_emits_toolbar_button_clicked() {
    // A click on a disabled action still routes through form_click_event
    // (the primitive layout marks it non-clickable, so the hit won't
    // fire in practice). But if a caller does pass a disabled action id
    // in, we should still recover the field_id correctly.
    use crate::primitives::toolbar::{Toolbar, ToolbarButton};
    let mut form = make_form(1);
    form.fields[0].kind = FieldKind::Toolbar(Toolbar {
        id: WidgetId::new("tb"),
        buttons: vec![ToolbarButton::Action {
            spec: None,
            id: WidgetId::new("noop"),
            label: "Noop".into(),
            icon: None,
            key_hint: None,
            enabled: false,
            is_active: false,
            tooltip: String::new(),
        }],
        bg: None,
        focused_index: None,
        ..Default::default()
    });

    // form_click_event matches by id, not by enabled state.
    let ev = form_click_event(&form, &WidgetId::new("noop"));
    assert_eq!(
        ev,
        FormEvent::ToolbarButtonClicked {
            field_id: WidgetId::new("field-0"),
            button_id: WidgetId::new("noop"),
        }
    );
}

// ── Empty-form edge cases ────────────────────────────────────────

#[test]
fn scroll_by_on_empty_is_noop() {
    let mut fc = Host::new("fc".into());
    fc.scroll_by(5, 10);
    assert_eq!(fc.scroll_offset(), 0);
}

// ── handle_cached ───────────────────────────────────────────────

#[test]
fn handle_cached_returns_ignored_without_backend_info() {
    let mut fc = test_controller(20);
    let ev = fc.handle_cached(
        &UiEvent::Scroll {
            widget: None,
            delta: crate::ScrollDelta { x: 0.0, y: 1.0 },
            position: crate::Point { x: 5.0, y: 5.0 },
        },
        Rect::new(0.0, 0.0, 40.0, 10.0),
    );
    assert_eq!(ev, Handled::Ignored);
}

#[test]
fn handle_cached_scroll_works_after_set_backend_info() {
    let mut fc = test_controller(20);
    // TUI: lh=1.0, row_h=1.0, rect height=5 → 5 viewport rows
    fc.set_backend_info(1.0);
    let ev = fc.handle_cached(
        &UiEvent::Scroll {
            widget: None,
            delta: crate::ScrollDelta { x: 0.0, y: -1.0 },
            position: crate::Point { x: 5.0, y: 2.0 },
        },
        Rect::new(0.0, 0.0, 40.0, 5.0),
    );
    assert_eq!(ev, Handled::Consumed);
    assert_eq!(fc.scroll_offset(), 1);
}

#[test]
fn handle_cached_scroll_consumes_the_delta_magnitude_not_just_its_sign() {
    let mut fc = test_controller(20);
    fc.set_backend_info(1.0);
    let ev = fc.handle_cached(
        &UiEvent::Scroll {
            widget: None,
            delta: crate::ScrollDelta { x: 0.0, y: -3.0 },
            position: crate::Point { x: 5.0, y: 2.0 },
        },
        Rect::new(0.0, 0.0, 40.0, 5.0),
    );
    assert_eq!(ev, Handled::Consumed);
    assert_eq!(fc.scroll_offset(), 3);
}

#[test]
fn handle_cached_click_uses_fallback_layout() {
    let mut fc = test_controller(3);
    fc.set_backend_info(1.0);
    let ev = fc.handle_cached(
        &UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: crate::Point { x: 5.0, y: 0.5 },
            modifiers: Default::default(),
        },
        Rect::new(0.0, 0.0, 40.0, 5.0),
    );
    match ev {
        Handled::Event(FormEvent::ToggleChanged { id, value }) => {
            assert_eq!(id.as_str(), "field-0");
            assert!(value);
        }
        other => panic!("expected FormAction(ToggleChanged), got {other:?}"),
    }
}

#[test]
fn handle_cached_click_on_second_toolbar_button_resolves_that_button() {
    use crate::primitives::toolbar::{Toolbar, ToolbarButton};

    let mut fc = test_controller(1);
    fc.set_form({
        let mut form = make_form(1);
        form.fields[0].label = StyledText::default();
        form.fields[0].kind = FieldKind::Toolbar(Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![
                ToolbarButton::Action {
                    spec: None,
                    id: WidgetId::new("btn-a"),
                    label: "A".into(),
                    icon: None,
                    key_hint: None,
                    enabled: true,
                    is_active: false,
                    tooltip: String::new(),
                },
                ToolbarButton::Action {
                    spec: None,
                    id: WidgetId::new("btn-b"),
                    label: "BB".into(),
                    icon: None,
                    key_hint: None,
                    enabled: true,
                    is_active: false,
                    tooltip: String::new(),
                },
            ],
            bg: None,
            focused_index: None,
            ..Default::default()
        });
        form
    });
    // lh=10.0 → row_h=14.0, char_w=6.0. An unlabelled toolbar still
    // reserves `char_w` (6.0) for the row inset, so items_start_x =
    // 6.0, not 0.0 (see `form_field_measure`'s `FieldKind::Toolbar`
    // arm): "A" button is 1*6.0 + 2*8.0 = 22.0 wide, spanning x in
    // [6, 28); the "BB" button follows immediately (no item gap),
    // spanning x in [28, 56). x=30 lands inside the second button.
    fc.set_backend_info(10.0);

    // A click left of items_start_x must miss both buttons and fall
    // through to the row's own hit region (FocusChanged on the
    // field), pinning that items_start_x is 6.0 and not 0.0 -
    // the pixel `Form` layout's row inset, not
    // tui_form_layout's flush-left TUI cell convention.
    let miss = fc.handle_cached(
        &UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: crate::Point { x: 3.0, y: 5.0 },
            modifiers: Default::default(),
        },
        Rect::new(0.0, 0.0, 200.0, 14.0),
    );
    assert_eq!(
        miss,
        Handled::Event(FormEvent::FocusChanged {
            id: WidgetId::new("field-0"),
        }),
        "a click left of items_start_x must miss both buttons"
    );

    let ev = fc.handle_cached(
        &UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: crate::Point { x: 30.0, y: 5.0 },
            modifiers: Default::default(),
        },
        Rect::new(0.0, 0.0, 200.0, 14.0),
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::ToolbarButtonClicked {
            field_id: WidgetId::new("field-0"),
            button_id: WidgetId::new("btn-b"),
        }),
        "click on the second toolbar button must resolve to its own id, \
             not fall back to the field's generic FocusChanged"
    );
}

#[test]
fn handle_cached_click_on_toolbar_separator_falls_back_to_field_focus() {
    use crate::primitives::toolbar::{Toolbar, ToolbarButton};

    let mut fc = test_controller(1);
    fc.set_form({
        let mut form = make_form(1);
        form.fields[0].label = StyledText::default();
        form.fields[0].kind = FieldKind::Toolbar(Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![
                ToolbarButton::Action {
                    spec: None,
                    id: WidgetId::new("btn-a"),
                    label: "A".into(),
                    icon: None,
                    key_hint: None,
                    enabled: true,
                    is_active: false,
                    tooltip: String::new(),
                },
                ToolbarButton::Separator,
                ToolbarButton::Action {
                    spec: None,
                    id: WidgetId::new("btn-b"),
                    label: "B".into(),
                    icon: None,
                    key_hint: None,
                    enabled: true,
                    is_active: false,
                    tooltip: String::new(),
                },
            ],
            bg: None,
            focused_index: None,
            ..Default::default()
        });
        form
    });
    // lh=10.0 → row_h=14.0, char_w=6.0, items_start_x=6.0. "A" is
    // 1*6.0 + 2*8.0 = 22.0 wide, spanning [6, 28); the separator is a
    // fixed SEPARATOR_PX=12.0 regardless of char_w, spanning [28, 40);
    // "B" follows at [40, 62). A `Separator` has no id of its own, so
    // `form_field_measure` falls back to `field.id` for its item - a
    // click inside the separator's zone must resolve the same way a
    // click outside every item does: FocusChanged on the field, not a
    // ToolbarButtonClicked for either neighbouring button.
    fc.set_backend_info(10.0);

    let ev = fc.handle_cached(
        &UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: crate::Point { x: 34.0, y: 5.0 },
            modifiers: Default::default(),
        },
        Rect::new(0.0, 0.0, 200.0, 14.0),
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::FocusChanged {
            id: WidgetId::new("field-0"),
        }),
        "a click on a separator's hit-zone must fall back to the field's \
             generic FocusChanged, not resolve to either neighbouring button"
    );
}

#[test]
fn handle_cached_toolbar_items_start_x_accounts_for_the_label() {
    use crate::primitives::toolbar::{Toolbar, ToolbarButton};

    let mut fc = test_controller(1);
    fc.set_form({
        let mut form = make_form(1);
        form.fields[0].label = StyledText::plain("Lbl"); // 3 chars
        form.fields[0].kind = FieldKind::Toolbar(Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![ToolbarButton::Action {
                spec: None,
                id: WidgetId::new("btn-a"),
                label: "A".into(),
                icon: None,
                key_hint: None,
                enabled: true,
                is_active: false,
                tooltip: String::new(),
            }],
            bg: None,
            focused_index: None,
            ..Default::default()
        });
        form
    });
    // lh=10.0 → row_h=14.0, char_w=6.0. A labelled toolbar row uses the
    // same `label_w + 2 * char_w` reservation as every sibling arm:
    // label_w = 3*6.0 = 18.0, so items_start_x = 18.0 + 12.0 = 30.0,
    // and "A" (1*6.0 + 2*8.0 = 22.0 wide) spans [30, 52).
    fc.set_backend_info(10.0);

    let miss = fc.handle_cached(
        &UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: crate::Point { x: 20.0, y: 5.0 },
            modifiers: Default::default(),
        },
        Rect::new(0.0, 0.0, 200.0, 14.0),
    );
    assert_eq!(
        miss,
        Handled::Event(FormEvent::FocusChanged {
            id: WidgetId::new("field-0"),
        }),
        "a click inside the label's reserved space must miss the button"
    );

    let hit = fc.handle_cached(
        &UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: crate::Point { x: 40.0, y: 5.0 },
            modifiers: Default::default(),
        },
        Rect::new(0.0, 0.0, 200.0, 14.0),
    );
    assert_eq!(
        hit,
        Handled::Event(FormEvent::ToolbarButtonClicked {
            field_id: WidgetId::new("field-0"),
            button_id: WidgetId::new("btn-a"),
        }),
        "a click past the label's reserved space must resolve to the button"
    );
}

// ── Pure helper tests ───────────────────────────────────────────

#[test]
fn viewport_rows_lh_tui() {
    // TUI: lh=1.0, row_h=round(1.4)=1.0, height=10 → 10 rows
    assert_eq!(viewport_rows(1.0, Rect::new(0.0, 0.0, 40.0, 10.0)), 10);
}

#[test]
fn viewport_rows_lh_gtk() {
    // GTK: lh=20.0, row_h=round(28.0)=28.0, height=280 → 10 rows
    assert_eq!(viewport_rows(20.0, Rect::new(0.0, 0.0, 400.0, 280.0)), 10);
}

#[test]
fn split_rect_lh_no_scrollbar_when_fits() {
    let (form_rect, sb) = split_rect(5, 1.0, Rect::new(0.0, 0.0, 40.0, 10.0));
    assert!(sb.is_none());
    assert_eq!(form_rect.width, 40.0);
}

#[test]
fn split_rect_lh_scrollbar_when_overflows() {
    let (form_rect, sb) = split_rect(20, 1.0, Rect::new(0.0, 0.0, 40.0, 10.0));
    assert!(sb.is_some());
    assert!(form_rect.width < 40.0);
}

// ── Keyboard editing ───────────────────────────────────────────

fn key_event(key: Key, modifiers: Modifiers) -> UiEvent {
    UiEvent::KeyPressed {
        key,
        modifiers,
        repeat: false,
    }
}

fn shift() -> Modifiers {
    Modifiers {
        shift: true,
        ..Default::default()
    }
}

fn text_field(id: &str, value: &str, cursor: Option<usize>) -> FormField {
    FormField {
        id: WidgetId::new(id),
        label: StyledText::plain("Label"),
        kind: FieldKind::TextInput {
            value: value.to_string(),
            placeholder: String::new(),
            cursor,
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
    }
}

fn label_field(id: &str) -> FormField {
    FormField {
        id: WidgetId::new(id),
        label: StyledText::plain("Header"),
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
    }
}

fn toggle_field(id: &str, disabled: bool) -> FormField {
    FormField {
        id: WidgetId::new(id),
        label: StyledText::plain("On"),
        kind: FieldKind::Toggle { value: false },
        hint: StyledText::default(),
        disabled,
        validation: None,
        label_position: None,
        label_align: None,
        label_width: None,
        tab_index: None,
        control_width: None,
        min_control_width: None,
        row_align: None,
    }
}

fn date_field(id: &str, focused: CalendarDate, disabled: bool) -> FormField {
    FormField {
        id: WidgetId::new(id),
        label: StyledText::plain("Date"),
        kind: FieldKind::DatePicker(crate::primitives::date_picker::DatePicker::new(
            WidgetId::new(id),
            focused,
        )),
        hint: StyledText::default(),
        disabled,
        validation: None,
        label_position: None,
        label_align: None,
        label_width: None,
        tab_index: None,
        control_width: None,
        min_control_width: None,
        row_align: None,
    }
}

fn combo_field(id: &str, options: &[&str], text: &str, open: bool, disabled: bool) -> FormField {
    combo_field_with_ids(id, id, options, text, open, disabled)
}

/// Like [`combo_field`], but lets the `FormField`'s id and the
/// embedded `ComboBox`'s own id differ, so a test can assert which
/// one a `FormEvent::ComboBox*` variant actually carries.
fn combo_field_with_ids(
    field_id: &str,
    combo_id: &str,
    options: &[&str],
    text: &str,
    open: bool,
    disabled: bool,
) -> FormField {
    let mut combo = ComboBox::new(
        WidgetId::new(combo_id),
        options.iter().map(|s| s.to_string()).collect(),
    );
    combo.text = text.to_string();
    combo.open = open;
    FormField {
        id: WidgetId::new(field_id),
        label: StyledText::plain("Combo"),
        kind: FieldKind::ComboBox(combo),
        hint: StyledText::default(),
        disabled,
        validation: None,
        label_position: None,
        label_align: None,
        label_width: None,
        tab_index: None,
        control_width: None,
        min_control_width: None,
        row_align: None,
    }
}

fn number_field(id: &str, number: NumberInput, disabled: bool) -> FormField {
    FormField {
        id: WidgetId::new(id),
        label: StyledText::default(),
        kind: FieldKind::NumberInput(number),
        hint: StyledText::default(),
        disabled,
        validation: None,
        label_position: None,
        label_align: None,
        label_width: None,
        tab_index: None,
        control_width: None,
        min_control_width: None,
        row_align: None,
    }
}

fn slider_field(id: &str, value: f32, min: f32, max: f32, step: f32, disabled: bool) -> FormField {
    FormField {
        id: WidgetId::new(id),
        label: StyledText::default(),
        kind: FieldKind::Slider {
            value,
            min,
            max,
            step,
        },
        hint: StyledText::default(),
        disabled,
        validation: None,
        label_position: None,
        label_align: None,
        label_width: None,
        tab_index: None,
        control_width: None,
        min_control_width: None,
        row_align: None,
    }
}

fn radio_group_field(id: &str, group: RadioGroup, disabled: bool) -> FormField {
    FormField {
        id: WidgetId::new(id),
        label: StyledText::default(),
        kind: FieldKind::RadioGroup(group),
        hint: StyledText::default(),
        disabled,
        validation: None,
        label_position: None,
        label_align: None,
        label_width: None,
        tab_index: None,
        control_width: None,
        min_control_width: None,
        row_align: None,
    }
}

fn dropdown_field(id: &str, options: &[&str], selected_idx: usize, disabled: bool) -> FormField {
    FormField {
        id: WidgetId::new(id),
        label: StyledText::plain("Dropdown"),
        kind: FieldKind::Dropdown {
            options: options.iter().map(|s| StyledText::plain(*s)).collect(),
            selected_idx,
        },
        hint: StyledText::default(),
        disabled,
        validation: None,
        label_position: None,
        label_align: None,
        label_width: None,
        tab_index: None,
        control_width: None,
        min_control_width: None,
        row_align: None,
    }
}

fn controller_with(fields: Vec<FormField>, focused: Option<&str>) -> Host {
    let mut fc = Host::new("f".into());
    fc.set_form(Form {
        id: WidgetId::new("form"),
        fields,
        focused_field: focused.map(WidgetId::new),
        scroll_offset: 0,
        has_focus: true,
        label_position: Default::default(),
        label_align: Default::default(),
        label_width: None,
        control_width: Default::default(),
        min_control_width: None,
        spacing: Default::default(),
    });
    // The controller's own `has_focus` gates `KeyPressed`/`CharTyped`/
    // `ClipboardPaste` (not `Form.has_focus` above, which only
    // affects rendering) — these keyboard-editing tests need it set.
    fc.set_has_focus(true);
    fc.set_backend_info(1.0);
    fc
}

const RECT: Rect = Rect::new(0.0, 0.0, 40.0, 10.0);

#[test]
fn tab_moves_focus_to_first_focusable_field_skipping_label() {
    let mut fc = controller_with(
        vec![
            label_field("hdr"),
            text_field("name", "", None),
            toggle_field("tgl", false),
        ],
        None,
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::FocusChanged {
            id: WidgetId::new("name")
        })
    );
}

#[test]
fn tab_advances_and_wraps_around() {
    let mut fc = controller_with(
        vec![text_field("a", "", None), toggle_field("b", false)],
        Some("b"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::FocusChanged {
            id: WidgetId::new("a")
        }),
        "tab from the last field should wrap to the first"
    );
}

#[test]
fn shift_tab_retreats_and_wraps() {
    let mut fc = controller_with(
        vec![text_field("a", "", None), toggle_field("b", false)],
        Some("a"),
    );
    let ev = fc.handle_cached(&key_event(Key::Named(NamedKey::Tab), shift()), RECT);
    assert_eq!(
        ev,
        Handled::Event(FormEvent::FocusChanged {
            id: WidgetId::new("b")
        }),
        "shift+tab from the first field should wrap to the last"
    );
}

#[test]
fn back_tab_also_retreats() {
    let mut fc = controller_with(
        vec![text_field("a", "", None), toggle_field("b", false)],
        Some("b"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::BackTab), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::FocusChanged {
            id: WidgetId::new("a")
        })
    );
}

#[test]
fn tab_skips_disabled_field() {
    let mut fc = controller_with(
        vec![
            text_field("a", "", None),
            toggle_field("disabled-tgl", true),
            toggle_field("c", false),
        ],
        Some("a"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::FocusChanged {
            id: WidgetId::new("c")
        }),
        "tab should skip the disabled field entirely"
    );
}

#[test]
fn tab_follows_explicit_tab_index_over_declaration_order() {
    // Declared "a", "b", "c" but `tab_index` reorders the visit to
    // "c", "a", "b" — tabbing from "c" must land on "a", not "b".
    let mut a = text_field("a", "", None);
    a.tab_index = Some(1);
    let mut b = text_field("b", "", None);
    b.tab_index = Some(2);
    let mut c = text_field("c", "", None);
    c.tab_index = Some(0);
    let mut fc = controller_with(vec![a, b, c], Some("c"));
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::FocusChanged {
            id: WidgetId::new("a")
        }),
        "tab_index should override declaration order"
    );
}

#[test]
fn typing_char_appends_and_updates_value() {
    let mut fc = controller_with(vec![text_field("name", "ab", Some(2))], Some("name"));
    let ev = fc.handle_cached(&key_event(Key::Char('c'), Modifiers::default()), RECT);
    assert_eq!(
        ev,
        Handled::Event(FormEvent::TextInputChanged {
            id: WidgetId::new("name"),
            value: "abc".to_string(),
        })
    );
}

#[test]
fn cursor_persists_across_keystrokes_even_if_app_never_echoes_it_back() {
    let mut fc = controller_with(vec![text_field("name", "ab", Some(2))], Some("name"));
    let ev1 = fc.handle_cached(&key_event(Key::Char('c'), Modifiers::default()), RECT);
    assert_eq!(
        ev1,
        Handled::Event(FormEvent::TextInputChanged {
            id: WidgetId::new("name"),
            value: "abc".to_string(),
        })
    );

    // The app updates `value` per the emitted event but — as the
    // issue requires — never tracks `cursor` itself; it stays `None`.
    fc.set_form(Form {
        id: WidgetId::new("form"),
        fields: vec![text_field("name", "abc", None)],
        focused_field: Some(WidgetId::new("name")),
        scroll_offset: 0,
        has_focus: true,
        label_position: Default::default(),
        label_align: Default::default(),
        label_width: None,
        control_width: Default::default(),
        min_control_width: None,
        spacing: Default::default(),
    });

    let ev2 = fc.handle_cached(&key_event(Key::Char('d'), Modifiers::default()), RECT);
    assert_eq!(
        ev2,
        Handled::Event(FormEvent::TextInputChanged {
            id: WidgetId::new("name"),
            value: "abcd".to_string(),
        }),
        "the controller's cached cursor (after 'c') should carry over, \
             appending 'd' at the end rather than re-deriving a stale position"
    );
}

#[test]
fn backspace_deletes_char_before_cursor() {
    let mut fc = controller_with(vec![text_field("name", "abc", Some(3))], Some("name"));
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Backspace), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::TextInputChanged {
            id: WidgetId::new("name"),
            value: "ab".to_string(),
        })
    );
}

#[test]
fn enter_commits_text_field_instead_of_inserting_newline() {
    let mut fc = controller_with(vec![text_field("name", "hello", Some(5))], Some("name"));
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::TextInputCommitted {
            id: WidgetId::new("name"),
            value: "hello".to_string(),
        })
    );
}

#[test]
fn char_typed_event_also_inserts_into_the_focused_field() {
    let mut fc = controller_with(vec![text_field("name", "", None)], Some("name"));
    let ev = fc.handle_cached(&UiEvent::CharTyped('x'), RECT);
    assert_eq!(
        ev,
        Handled::Event(FormEvent::TextInputChanged {
            id: WidgetId::new("name"),
            value: "x".to_string(),
        })
    );
}

#[test]
fn disabled_text_field_ignores_keystrokes() {
    let mut fc = controller_with(
        vec![FormField {
            id: WidgetId::new("name"),
            label: StyledText::plain("Name"),
            kind: FieldKind::TextInput {
                value: "ab".to_string(),
                placeholder: String::new(),
                cursor: Some(2),
                selection_anchor: None,
            },
            hint: StyledText::default(),
            disabled: true,
            validation: None,
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width: None,
            min_control_width: None,
            row_align: None,
        }],
        Some("name"),
    );
    let ev = fc.handle_cached(&key_event(Key::Char('c'), Modifiers::default()), RECT);
    assert_eq!(ev, Handled::Ignored);
}

#[test]
fn no_focused_field_ignores_text_keys() {
    let mut fc = controller_with(vec![text_field("name", "ab", Some(2))], None);
    let ev = fc.handle_cached(&key_event(Key::Char('c'), Modifiers::default()), RECT);
    assert_eq!(ev, Handled::Ignored);
}

#[test]
fn shift_left_then_char_replaces_the_selection() {
    // "ab|c" (cursor after 'b') — shift+Left selects "b", typing 'X'
    // replaces the selection (EditOp::InsertChar's documented
    // selection-replace behaviour), landing on "aXc".
    let mut fc = controller_with(vec![text_field("name", "abc", Some(2))], Some("name"));
    let ev1 = fc.handle_cached(&key_event(Key::Named(NamedKey::Left), shift()), RECT);
    assert_eq!(
        ev1,
        Handled::Consumed,
        "pure cursor/selection movement changes no text, so it's Consumed not a FormEvent"
    );

    let ev2 = fc.handle_cached(&key_event(Key::Char('X'), Modifiers::default()), RECT);
    assert_eq!(
        ev2,
        Handled::Event(FormEvent::TextInputChanged {
            id: WidgetId::new("name"),
            value: "aXc".to_string(),
        })
    );
}

#[test]
fn build_form_overlays_the_cached_cursor_for_rendering() {
    let mut fc = controller_with(vec![text_field("name", "ab", Some(2))], Some("name"));
    let _ = fc.handle_cached(&key_event(Key::Char('c'), Modifiers::default()), RECT);

    // Realistic round-trip: the app persists `value` from the
    // emitted `TextInputChanged` (same as it already does for a
    // toggle flip) but — per `primitives::form::contract`'s "Keyboard editing" doc
    // section — never has to track `cursor` itself, so it stays
    // `None` here.
    fc.set_form(Form {
        id: WidgetId::new("form"),
        fields: vec![text_field("name", "abc", None)],
        focused_field: Some(WidgetId::new("name")),
        scroll_offset: 0,
        has_focus: true,
        label_position: Default::default(),
        label_align: Default::default(),
        label_width: None,
        control_width: Default::default(),
        min_control_width: None,
        spacing: Default::default(),
    });

    let rendered = fc.build_form(RECT, None);
    match &rendered.fields[0].kind {
        FieldKind::TextInput { value, cursor, .. } => {
            assert_eq!(value, "abc");
            assert_eq!(
                *cursor,
                Some(3),
                "controller-cached cursor should overlay the app's \
                     None, sitting after the just-typed 'c'"
            );
        }
        other => panic!("expected TextInput, got {other:?}"),
    }
}

#[test]
fn text_area_newline_round_trips_through_byte_offset_conversion() {
    // TextArea's `value` embeds '\n' directly — exercise the
    // multi-line byte_offset_to_line_col / line_col_to_byte_offset
    // conversion, not just the single-line TextInput path.
    let field = FormField {
        id: WidgetId::new("notes"),
        label: StyledText::plain("Notes"),
        kind: FieldKind::TextArea {
            value: "line one\nline two".to_string(),
            placeholder: String::new(),
            cursor: Some("line one\n".len()), // start of "line two"
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
    };
    let mut fc = controller_with(vec![field], Some("notes"));
    let ev = fc.handle_cached(&key_event(Key::Char('X'), Modifiers::default()), RECT);
    assert_eq!(
        ev,
        Handled::Event(FormEvent::TextInputChanged {
            id: WidgetId::new("notes"),
            value: "line one\nXline two".to_string(),
        })
    );
}

fn ctrl() -> Modifiers {
    Modifiers {
        ctrl: true,
        ..Default::default()
    }
}

fn cmd() -> Modifiers {
    Modifiers {
        cmd: true,
        ..Default::default()
    }
}

#[test]
fn ctrl_z_on_a_focused_text_field_is_ignored_not_typed() {
    // Every backend delivers Ctrl+Z as `Key::Char('z')` with
    // `modifiers.ctrl == true`; typing it would insert a literal 'z'
    // into the field and report `TextInputChanged` instead of falling
    // through to an app-level Undo accelerator.
    let mut fc = controller_with(vec![text_field("name", "ab", Some(2))], Some("name"));
    let ev = fc.handle_cached(&key_event(Key::Char('z'), ctrl()), RECT);
    assert_eq!(ev, Handled::Ignored);
    // The value is genuinely untouched, not just the return value.
    match &fc.form().unwrap().fields[0].kind {
        FieldKind::TextInput { value, .. } => assert_eq!(value, "ab"),
        other => panic!("expected TextInput, got {other:?}"),
    }
}

#[test]
fn cmd_s_on_a_focused_text_field_is_ignored_not_typed() {
    // Same guard, macOS's modifier.
    let mut fc = controller_with(vec![text_field("name", "ab", Some(2))], Some("name"));
    let ev = fc.handle_cached(&key_event(Key::Char('s'), cmd()), RECT);
    assert_eq!(ev, Handled::Ignored);
}

#[test]
fn selection_anchor_mid_multibyte_char_snaps_instead_of_panicking() {
    // `selection_anchor` is app-owned public data (`primitives/form.rs`
    // documents it as "a byte offset into `value`") and isn't
    // guaranteed to land on a char boundary — e.g. an app that
    // accidentally computes it in char counts instead of bytes.
    // `byte_offset_to_line_col` slices `value[..byte_offset]`, which
    // would panic on a non-boundary offset into "h\u{e9}llo" (byte 2
    // is the second byte of the two-byte 'é').
    let field = FormField {
        id: WidgetId::new("name"),
        label: StyledText::plain("Name"),
        kind: FieldKind::TextInput {
            value: "h\u{e9}llo".to_string(),
            placeholder: String::new(),
            cursor: Some(5),
            selection_anchor: Some(2), // mid-char: not a valid boundary
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
    };
    let mut fc = controller_with(vec![field], Some("name"));
    // Must not panic. `EditOp::MoveRight` with `extend: false` drops
    // the selection, so the exact resulting event only needs to be
    // some non-panicking outcome — the regression is the absence of
    // a panic, not this particular value.
    let _ = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
        RECT,
    );
}

// ── DatePicker routing ──────────────────────────────────────────

#[test]
fn right_arrow_moves_date_picker_focus_and_is_consumed_not_a_form_action() {
    let start = CalendarDate::new(2024, 3, 15);
    let mut fc = controller_with(vec![date_field("d", start, false)], Some("d"));
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Consumed,
        "pure focus-day movement changes no persisted value, so it's \
             Consumed not a FormEvent, mirroring text cursor movement"
    );
    let form = fc.build_form(RECT, None);
    match &form.fields[0].kind {
        FieldKind::DatePicker(picker) => {
            assert_eq!(picker.focused, start.add_days(1));
        }
        other => panic!("expected DatePicker, got {other:?}"),
    }
}

#[test]
fn cached_focused_date_persists_across_keystrokes_even_if_app_never_echoes_it_back() {
    let start = CalendarDate::new(2024, 3, 15);
    let mut fc = controller_with(vec![date_field("d", start, false)], Some("d"));
    let _ = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
        RECT,
    );

    // The app never tracks `focused` itself (per `DateEditState`'s
    // doc) — it stays at `start` here, exactly as the field was
    // first pushed.
    fc.set_form(Form {
        id: WidgetId::new("form"),
        fields: vec![date_field("d", start, false)],
        focused_field: Some(WidgetId::new("d")),
        scroll_offset: 0,
        has_focus: true,
        label_position: Default::default(),
        label_align: Default::default(),
        label_width: None,
        control_width: Default::default(),
        min_control_width: None,
        spacing: Default::default(),
    });

    let ev2 = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
        RECT,
    );
    assert_eq!(ev2, Handled::Consumed);
    let form = fc.build_form(RECT, None);
    match &form.fields[0].kind {
        FieldKind::DatePicker(picker) => assert_eq!(
            picker.focused,
            start.add_days(2),
            "the controller's cached focused day should carry over, \
                 landing two days forward rather than re-deriving a stale position"
        ),
        other => panic!("expected DatePicker, got {other:?}"),
    }
}

#[test]
fn enter_confirms_the_cached_focused_date_and_clears_the_cache() {
    let start = CalendarDate::new(2024, 3, 15);
    let mut fc = controller_with(vec![date_field("d", start, false)], Some("d"));
    let _ = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
        RECT,
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::DateChanged {
            id: WidgetId::new("d"),
            value: start.add_days(1),
        })
    );

    // The cache clears on confirm, so re-rendering the app's
    // untouched field (still `start`) shows `start` again, not the
    // confirmed day — the app is expected to push the new value
    // back via `set_form` itself.
    let form = fc.build_form(RECT, None);
    match &form.fields[0].kind {
        FieldKind::DatePicker(picker) => assert_eq!(picker.focused, start),
        other => panic!("expected DatePicker, got {other:?}"),
    }
}

#[test]
fn disabled_date_picker_field_ignores_navigation_keys() {
    let start = CalendarDate::new(2024, 3, 15);
    let mut fc = controller_with(vec![date_field("d", start, true)], Some("d"));
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
        RECT,
    );
    assert_eq!(ev, Handled::Ignored);
}

#[test]
fn tab_moves_focus_away_from_a_focused_date_picker_field_normally() {
    let start = CalendarDate::new(2024, 3, 15);
    let mut fc = controller_with(
        vec![date_field("d", start, false), toggle_field("t", false)],
        Some("d"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::FocusChanged {
            id: WidgetId::new("t")
        }),
        "Tab isn't one of DatePicker's claimed keys, so focus traversal \
             off a focused DatePicker field still works"
    );
}

#[test]
fn date_edit_cache_resets_when_focus_moves_to_a_different_date_picker_field() {
    let d1_start = CalendarDate::new(2024, 3, 15);
    let d2_start = CalendarDate::new(2024, 6, 1);
    let mut fc = controller_with(
        vec![
            date_field("d1", d1_start, false),
            date_field("d2", d2_start, false),
        ],
        Some("d1"),
    );
    let _ = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
        RECT,
    );
    let _ = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
        RECT,
    );

    // `move_focus` only emits `FocusChanged` — per this module's
    // "Keyboard editing" doc section, the app persists the new
    // `focused_field` itself, same as it would for a toggle flip.
    fc.set_form(Form {
        id: WidgetId::new("form"),
        fields: vec![
            date_field("d1", d1_start, false),
            date_field("d2", d2_start, false),
        ],
        focused_field: Some(WidgetId::new("d2")),
        scroll_offset: 0,
        has_focus: true,
        label_position: Default::default(),
        label_align: Default::default(),
        label_width: None,
        control_width: Default::default(),
        min_control_width: None,
        spacing: Default::default(),
    });

    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
        RECT,
    );
    assert_eq!(ev, Handled::Consumed);
    let form = fc.build_form(RECT, None);
    match &form.fields[1].kind {
        FieldKind::DatePicker(picker) => assert_eq!(
            picker.focused,
            d2_start.add_days(1),
            "moving focus to d2 should seed its own focused day, not \
                 carry over d1's cached scrubbing position"
        ),
        other => panic!("expected DatePicker, got {other:?}"),
    }
}

#[test]
fn build_form_shows_live_scrub_position_over_a_confirmed_selected_date() {
    let start = CalendarDate::new(2024, 3, 15);
    let field_with_selected = |selected: CalendarDate| {
        let mut f = date_field("d", start, false);
        if let FieldKind::DatePicker(picker) = &mut f.kind {
            picker.selected = Some(selected);
        }
        f
    };
    let mut fc = controller_with(vec![field_with_selected(start)], Some("d"));

    // Scrub past the already-confirmed date. The renderer (see the
    // "Date picker fields" doc section above) picks `focused` over
    // `selected` while the row is focused, so this must advance
    // `focused` without touching `selected`.
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
        RECT,
    );
    assert_eq!(ev, Handled::Consumed);

    let form = fc.build_form(RECT, None);
    match &form.fields[0].kind {
        FieldKind::DatePicker(picker) => {
            assert_eq!(
                picker.selected,
                Some(start),
                "scrubbing must not touch the app's confirmed value"
            );
            assert_eq!(
                picker.focused,
                start.add_days(1),
                "the live scrub position must be exposed via `focused` \
                     even once a date has already been confirmed, so the \
                     renderer doesn't show a frozen `selected` instead"
            );
        }
        other => panic!("expected DatePicker, got {other:?}"),
    }
}

#[test]
fn ctrl_modified_date_picker_key_falls_through_instead_of_navigating() {
    let start = CalendarDate::new(2024, 3, 15);
    let mut fc = controller_with(vec![date_field("d", start, false)], Some("d"));
    let ev = fc.handle_cached(
        &key_event(
            Key::Named(NamedKey::Right),
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
        ),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Ignored,
        "Ctrl+Right is a host accelerator candidate, not calendar \
             navigation, so it must not be swallowed by the DatePicker"
    );
    let form = fc.build_form(RECT, None);
    match &form.fields[0].kind {
        FieldKind::DatePicker(picker) => {
            assert_eq!(picker.focused, start, "the calendar must not have moved")
        }
        other => panic!("expected DatePicker, got {other:?}"),
    }
}

#[test]
fn build_form_does_not_overlay_a_stale_cached_scrub_onto_an_unfocused_date_picker() {
    let d1_start = CalendarDate::new(2024, 3, 15);
    let d2_start = CalendarDate::new(2024, 6, 1);
    let mut fc = controller_with(
        vec![
            date_field("d1", d1_start, false),
            date_field("d2", d2_start, false),
        ],
        Some("d1"),
    );
    let _ = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
        RECT,
    );

    // Move focus to "d2" by a click rather than Tab — unlike
    // `move_focus`, `click_inner`'s path to `FormEvent::FocusChanged`
    // doesn't reseed/clear the cached scrub on "d1", so this is the
    // scenario where a stale `date_edit` could otherwise leak.
    fc.set_form(Form {
        id: WidgetId::new("form"),
        fields: vec![
            date_field("d1", d1_start, false),
            date_field("d2", d2_start, false),
        ],
        focused_field: Some(WidgetId::new("d2")),
        scroll_offset: 0,
        has_focus: true,
        label_position: Default::default(),
        label_align: Default::default(),
        label_width: None,
        control_width: Default::default(),
        min_control_width: None,
        spacing: Default::default(),
    });

    let form = fc.build_form(RECT, None);
    match &form.fields[0].kind {
        FieldKind::DatePicker(picker) => assert_eq!(
            picker.focused, d1_start,
            "the stale cached scrub for \"d1\" must not be overlaid \
                 once focus has moved to \"d2\""
        ),
        other => panic!("expected DatePicker, got {other:?}"),
    }
}

// ── ComboBox routing ──────────────────────────────────────────

#[test]
fn typed_char_on_a_focused_combo_box_updates_its_text() {
    let mut fc = controller_with(
        vec![combo_field(
            "c",
            &["Apple", "Banana", "Cherry"],
            "",
            false,
            false,
        )],
        Some("c"),
    );
    let ev = fc.handle_cached(&key_event(Key::Char('b'), Modifiers::default()), RECT);
    assert_eq!(
        ev,
        Handled::Event(FormEvent::ComboBoxTextChanged {
            id: WidgetId::new("c"),
            value: "b".to_string(),
        })
    );
}

#[test]
fn down_arrow_moves_combo_box_highlight() {
    let mut fc = controller_with(
        vec![combo_field(
            "c",
            &["Apple", "Banana", "Cherry"],
            "",
            true,
            false,
        )],
        Some("c"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Down), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::ComboBoxHighlightChanged {
            id: WidgetId::new("c"),
            index: 0,
        })
    );
}

#[test]
fn down_arrow_over_an_empty_filtered_list_falls_through_instead_of_navigating() {
    // "xyz" matches none of these options, so `move_highlight` returns
    // `None` and `handle_combo_box_key` must fall through to the
    // generic dispatch (which has nothing else to do with a bare Down
    // either, so this resolves as `Ignored`) rather than returning
    // some synthetic event.
    let mut fc = controller_with(
        vec![combo_field("c", &["Apple", "Banana"], "xyz", true, false)],
        Some("c"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Down), Modifiers::default()),
        RECT,
    );
    assert_eq!(ev, Handled::Ignored);
}

#[test]
fn enter_commits_the_focused_combo_box_text() {
    let mut fc = controller_with(
        vec![combo_field(
            "c",
            &["Apple", "Banana", "Cherry"],
            "Ban",
            false,
            false,
        )],
        Some("c"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::ComboBoxCommitted {
            id: WidgetId::new("c"),
            value: "Ban".to_string(),
        })
    );
}

#[test]
fn escape_while_open_cancels_the_combo_box() {
    let mut fc = controller_with(
        vec![combo_field("c", &["Apple", "Banana"], "a", true, false)],
        Some("c"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Escape), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::ComboBoxCancelled {
            id: WidgetId::new("c"),
        })
    );
}

#[test]
fn escape_while_closed_falls_through_instead_of_cancelling() {
    let mut fc = controller_with(
        vec![combo_field("c", &["Apple", "Banana"], "a", false, false)],
        Some("c"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Escape), Modifiers::default()),
        RECT,
    );
    assert_eq!(ev, Handled::Ignored);
}

#[test]
fn disabled_combo_box_field_ignores_its_keys() {
    let mut fc = controller_with(
        vec![combo_field("c", &["Apple", "Banana"], "", false, true)],
        Some("c"),
    );
    let ev = fc.handle_cached(&key_event(Key::Char('a'), Modifiers::default()), RECT);
    assert_eq!(ev, Handled::Ignored);
}

#[test]
fn tab_moves_focus_away_from_a_focused_combo_box_field_normally() {
    let mut fc = controller_with(
        vec![
            combo_field("c", &["Apple", "Banana"], "", false, false),
            toggle_field("t", false),
        ],
        Some("c"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::FocusChanged {
            id: WidgetId::new("t")
        }),
        "Tab isn't one of ComboBox's claimed keys, so focus traversal \
             off a focused ComboBox field still works"
    );
}

#[test]
fn ctrl_modified_combo_box_key_falls_through_instead_of_editing() {
    let mut fc = controller_with(
        vec![combo_field("c", &["Apple", "Banana"], "ab", false, false)],
        Some("c"),
    );
    let ev = fc.handle_cached(&key_event(Key::Char('a'), ctrl()), RECT);
    assert_eq!(
        ev,
        Handled::Ignored,
        "Ctrl+A is a host accelerator candidate, not combo-box text \
             entry, so it must not be swallowed by the ComboBox"
    );
    let form = fc.build_form(RECT, None);
    match &form.fields[0].kind {
        FieldKind::ComboBox(combo) => {
            assert_eq!(combo.text, "ab", "the text must not have changed")
        }
        other => panic!("expected ComboBox, got {other:?}"),
    }
}

#[test]
fn combo_box_events_carry_the_field_id_not_the_embedded_combo_box_id() {
    // The `FormField` and its embedded `ComboBox` are given distinct
    // ids here, mirroring `examples/common/form_all_fields.rs`'s
    // "fruit" field / "fruit-combo" primitive split. Every
    // `FormEvent::ComboBox*` below must carry the *field* id
    // ("field"), not the primitive's own id ("combo") — otherwise a
    // consumer resolving the event back to its `FormField` via
    // `form.fields.iter().find(|f| f.id == id)` would silently find
    // nothing.
    let mut fc = controller_with(
        vec![combo_field_with_ids(
            "field",
            "combo",
            &["Apple", "Banana", "Cherry"],
            "Ban",
            false,
            false,
        )],
        Some("field"),
    );
    let ev = fc.handle_cached(&key_event(Key::Char('b'), Modifiers::default()), RECT);
    assert_eq!(
        ev,
        Handled::Event(FormEvent::ComboBoxTextChanged {
            id: WidgetId::new("field"),
            value: "Banb".to_string(),
        })
    );

    let mut fc = controller_with(
        vec![combo_field_with_ids(
            "field",
            "combo",
            &["Apple", "Banana", "Cherry"],
            "",
            true,
            false,
        )],
        Some("field"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Down), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::ComboBoxHighlightChanged {
            id: WidgetId::new("field"),
            index: 0,
        })
    );

    let mut fc = controller_with(
        vec![combo_field_with_ids(
            "field",
            "combo",
            &["Apple", "Banana", "Cherry"],
            "Ban",
            false,
            false,
        )],
        Some("field"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::ComboBoxCommitted {
            id: WidgetId::new("field"),
            value: "Ban".to_string(),
        })
    );

    let mut fc = controller_with(
        vec![combo_field_with_ids(
            "field",
            "combo",
            &["Apple", "Banana"],
            "a",
            true,
            false,
        )],
        Some("field"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Escape), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::ComboBoxCancelled {
            id: WidgetId::new("field"),
        })
    );
}

// ── NumberInput routing ─────────────────────────────────────────

#[test]
fn up_arrow_steps_a_focused_number_input_value_up() {
    let mut fc = controller_with(
        vec![number_field(
            "n",
            NumberInput::new(WidgetId::new("n"), 0.0, 10.0),
            false,
        )],
        Some("n"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Up), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::NumberChanged {
            id: WidgetId::new("n"),
            value: 1.0,
        })
    );
}

#[test]
fn down_arrow_steps_a_focused_number_input_value_down_clamped_at_min() {
    let mut fc = controller_with(
        vec![number_field(
            "n",
            NumberInput::new(WidgetId::new("n"), 0.0, 10.0),
            false,
        )],
        Some("n"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Down), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::NumberChanged {
            id: WidgetId::new("n"),
            value: 0.0,
        }),
        "decrementing below `min` must clamp, not go negative"
    );
}

#[test]
fn up_arrow_on_a_number_input_already_at_max_clamps_instead_of_overshooting() {
    let mut n = NumberInput::new(WidgetId::new("n"), 0.0, 10.0);
    n.value = 9.0;
    n.step = 5.0;
    let mut fc = controller_with(vec![number_field("n", n, false)], Some("n"));
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Up), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::NumberChanged {
            id: WidgetId::new("n"),
            value: 10.0,
        }),
        "a step past `max` must clamp to `max`, not overshoot to 14.0"
    );
}

#[test]
fn typed_digit_on_a_focused_number_input_updates_its_draft() {
    let mut fc = controller_with(
        vec![number_field(
            "n",
            NumberInput::new(WidgetId::new("n"), 0.0, 100.0),
            false,
        )],
        Some("n"),
    );
    let ev = fc.handle_cached(&key_event(Key::Char('5'), Modifiers::default()), RECT);
    assert_eq!(
        ev,
        Handled::Event(FormEvent::NumberDraftChanged {
            id: WidgetId::new("n"),
            draft: "5".to_string(),
        })
    );
}

#[test]
fn enter_commits_the_focused_number_inputs_draft_clamped_and_rounded() {
    let mut n = NumberInput::new(WidgetId::new("n"), 0.0, 10.0);
    n.draft = Some("999.456".into());
    n.precision = 1;
    let mut fc = controller_with(vec![number_field("n", n, false)], Some("n"));
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::NumberCommitted {
            id: WidgetId::new("n"),
            value: 10.0,
        }),
        "the draft must clamp to `max` on commit"
    );
}

#[test]
fn escape_cancels_a_focused_number_inputs_pending_draft() {
    let mut n = NumberInput::new(WidgetId::new("n"), 0.0, 10.0);
    n.draft = Some("3".into());
    let mut fc = controller_with(vec![number_field("n", n, false)], Some("n"));
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Escape), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::NumberCancelled {
            id: WidgetId::new("n"),
        })
    );
}

#[test]
fn disabled_number_input_field_ignores_its_keys() {
    let mut fc = controller_with(
        vec![number_field(
            "n",
            NumberInput::new(WidgetId::new("n"), 0.0, 10.0),
            true,
        )],
        Some("n"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Up), Modifiers::default()),
        RECT,
    );
    assert_eq!(ev, Handled::Ignored);
}

#[test]
fn tab_moves_focus_away_from_a_focused_number_input_field_normally() {
    let mut fc = controller_with(
        vec![
            number_field("n", NumberInput::new(WidgetId::new("n"), 0.0, 10.0), false),
            toggle_field("t", false),
        ],
        Some("n"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::FocusChanged {
            id: WidgetId::new("t")
        }),
        "Tab isn't one of NumberInput's claimed keys, so focus traversal \
             off a focused NumberInput field still works"
    );
}

/// `lh=10.0` → `row_h=14.0`, `char_w=6.0`; an unlabelled `NumberInput`
/// (no label, so `items_start_x = char_w = 6.0`) showing "0" sizes its
/// single item to `(1 + 5) * 6.0 = 36.0` wide — see
/// `form_field_measure`'s `FieldKind::NumberInput` arm — so its
/// control rect spans x in `[6, 42)`. `NumberInput::layout` then
/// splits off a 3-wide stepper column at the right: increment is
/// `[39, 42) x [0, 7)`, decrement is `[39, 42) x [7, 14)`.
/// The centre of number field `id`'s increment (`up`) or decrement
/// stepper, as the harness's pixel layout (`lh = 10.0`) at `rect`
/// places it.
fn stepper_center(fc: &Host, id: &str, rect: Rect, up: bool) -> crate::Point {
    let ctx = LayoutCtx::new(10.0, 6.0, false);
    let widget = fc.widget();
    let layout = ScrolledForm::layout_live(&widget.form, false, &ctx, rect);
    let id = WidgetId::new(id);
    let vf = layout
        .form
        .visible_field(&id)
        .expect("number field painted");
    let (_, control) = vf.item_bounds.first().expect("a control rect");
    let FieldKind::NumberInput(number) = &widget.form.fields[vf.field_idx].kind else {
        panic!("{id:?} is not a number field");
    };
    let stepper = number.layout(*control);
    let r = if up {
        stepper.increment
    } else {
        stepper.decrement
    };
    crate::Point::new(r.x + r.width / 2.0, r.y + r.height / 2.0)
}

#[test]
fn stepper_click_increments_a_number_inputs_value() {
    let mut fc = controller_with(
        vec![number_field(
            "n",
            NumberInput::new(WidgetId::new("n"), 0.0, 10.0),
            false,
        )],
        None,
    );
    fc.set_backend_info(10.0);
    let rect = Rect::new(0.0, 0.0, 200.0, 14.0);
    let ev = fc.handle_cached(
        &UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: stepper_center(&fc, "n", rect, true),
            modifiers: Default::default(),
        },
        rect,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::NumberChanged {
            id: WidgetId::new("n"),
            value: 1.0,
        })
    );
}

#[test]
fn stepper_click_decrements_a_number_inputs_value() {
    let mut n = NumberInput::new(WidgetId::new("n"), 0.0, 10.0);
    n.value = 5.0;
    let mut fc = controller_with(vec![number_field("n", n, false)], None);
    fc.set_backend_info(10.0);
    let rect = Rect::new(0.0, 0.0, 200.0, 14.0);
    let ev = fc.handle_cached(
        &UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: stepper_center(&fc, "n", rect, false),
            modifiers: Default::default(),
        },
        rect,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::NumberChanged {
            id: WidgetId::new("n"),
            value: 4.0,
        })
    );
}

#[test]
fn stepper_click_on_a_disabled_number_input_just_focuses_the_field() {
    let mut fc = controller_with(
        vec![number_field(
            "n",
            NumberInput::new(WidgetId::new("n"), 0.0, 10.0),
            true,
        )],
        None,
    );
    fc.set_backend_info(10.0);
    let rect = Rect::new(0.0, 0.0, 200.0, 14.0);
    let ev = fc.handle_cached(
        &UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: stepper_center(&fc, "n", rect, true),
            modifiers: Default::default(),
        },
        rect,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::FocusChanged {
            id: WidgetId::new("n"),
        }),
        "a disabled NumberInput's stepper click must not step the value"
    );
}

// ── Slider routing ────────────────────────────────────────────

#[test]
fn left_arrow_steps_a_focused_sliders_value_down() {
    let mut fc = controller_with(
        vec![slider_field("s", 5.0, 0.0, 10.0, 1.0, false)],
        Some("s"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Left), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::SliderChanged {
            id: WidgetId::new("s"),
            value: 4.0,
        })
    );
}

#[test]
fn right_arrow_steps_a_focused_sliders_value_up() {
    let mut fc = controller_with(
        vec![slider_field("s", 5.0, 0.0, 10.0, 1.0, false)],
        Some("s"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::SliderChanged {
            id: WidgetId::new("s"),
            value: 6.0,
        })
    );
}

#[test]
fn up_and_down_arrows_mirror_right_and_left_for_a_focused_slider() {
    let mut fc = controller_with(
        vec![slider_field("s", 5.0, 0.0, 10.0, 2.0, false)],
        Some("s"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Up), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::SliderChanged {
            id: WidgetId::new("s"),
            value: 7.0,
        })
    );

    let mut fc = controller_with(
        vec![slider_field("s", 5.0, 0.0, 10.0, 2.0, false)],
        Some("s"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Down), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::SliderChanged {
            id: WidgetId::new("s"),
            value: 3.0,
        })
    );
}

#[test]
fn right_arrow_past_max_clamps_instead_of_overshooting() {
    let mut fc = controller_with(
        vec![slider_field("s", 9.0, 0.0, 10.0, 5.0, false)],
        Some("s"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::SliderChanged {
            id: WidgetId::new("s"),
            value: 10.0,
        }),
        "a step past `max` must clamp to `max`, not overshoot to 14.0"
    );
}

#[test]
fn home_and_end_jump_a_focused_slider_to_its_bounds() {
    let mut fc = controller_with(
        vec![slider_field("s", 5.0, 0.0, 10.0, 1.0, false)],
        Some("s"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Home), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::SliderChanged {
            id: WidgetId::new("s"),
            value: 0.0,
        })
    );

    let mut fc = controller_with(
        vec![slider_field("s", 5.0, 0.0, 10.0, 1.0, false)],
        Some("s"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::End), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::SliderChanged {
            id: WidgetId::new("s"),
            value: 10.0,
        })
    );
}

/// Rect every slider click/drag test below aims inside of. Wide
/// enough that one field needs no scrollbar, so `split_rect_lh`
/// leaves it whole.
const SLIDER_RECT: Rect = Rect {
    x: 0.0,
    y: 0.0,
    width: 200.0,
    height: 14.0,
};

/// The painted track rect the harness's pixel layout (`lh = 10.0`,
/// the `set_backend_info` these tests use) reports for slider field
/// `id`.
///
/// Asked for rather than hardcoded, so a test aims at a fraction of
/// the track the painter actually draws — the whole point of the
/// `item_bounds` contract — instead of pinning arithmetic that would
/// still "pass" if click and paint drifted apart.
fn fallback_track_rect(fc: &Host, id: &str) -> Rect {
    let ctx = LayoutCtx::new(10.0, 6.0, false);
    let widget = fc.widget();
    let layout = ScrolledForm::layout_live(&widget.form, false, &ctx, SLIDER_RECT);
    slider_track_rect(&widget.form, &layout.form.layout, &WidgetId::new(id))
        .expect("an enabled Slider field has a painted track rect")
}

fn mouse_down_at(fc: &mut Host, x: f32) -> Handled<FormEvent> {
    fc.handle_cached(
        &UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: crate::Point { x, y: 7.0 },
            modifiers: Default::default(),
        },
        SLIDER_RECT,
    )
}

fn left_drag_to(fc: &mut Host, x: f32) -> Handled<FormEvent> {
    fc.handle_cached(
        &UiEvent::MouseMoved {
            position: crate::Point { x, y: 7.0 },
            buttons: ButtonMask {
                left: true,
                middle: false,
                right: false,
            },
        },
        SLIDER_RECT,
    )
}

fn labeled_slider_controller(value: f32) -> Host {
    let mut field = slider_field("s", value, 0.0, 10.0, 1.0, false);
    field.label = StyledText::plain("Volume");
    let mut fc = controller_with(vec![field], None);
    fc.set_backend_info(10.0);
    fc
}

/// An unlabeled `Slider`: a press at the midpoint of the track the
/// layout reports lands at fraction `0.5` of `[min, max] = [0, 10]`.
#[test]
fn click_on_a_sliders_track_maps_the_click_position_to_a_value() {
    let mut fc = controller_with(vec![slider_field("s", 0.0, 0.0, 10.0, 1.0, false)], None);
    fc.set_backend_info(10.0);
    let track = fallback_track_rect(&fc, "s");
    let ev = mouse_down_at(&mut fc, track.x + track.width / 2.0);
    assert_eq!(
        ev,
        Handled::Event(FormEvent::SliderChanged {
            id: WidgetId::new("s"),
            value: 5.0,
        })
    );
}

/// The same press on a *labeled* `Slider` — `LabelPosition::Left` is
/// the form-wide default (see `Form::label_position`'s doc), so this
/// is the layout any ordinary labeled slider field gets. The track
/// sits past the label column, and a press at each of its quarter
/// positions resolves to that fraction of `[min, max]`, not to a
/// fraction of the whole row.
#[test]
fn click_on_a_labeled_sliders_track_maps_the_click_position_to_a_value() {
    // Binary-exact fractions, so the expected value needs no float
    // tolerance — the mapping under test is a plain multiply.
    for (fraction, expected) in [(0.0, 0.0), (0.25, 2.5), (0.5, 5.0), (0.75, 7.5)] {
        let mut fc = labeled_slider_controller(10.0);
        let track = fallback_track_rect(&fc, "s");
        assert!(
            track.x > 0.0,
            "a labeled slider's track must start past its label column"
        );
        let ev = mouse_down_at(&mut fc, track.x + track.width * fraction);
        assert_eq!(
            ev,
            Handled::Event(FormEvent::SliderChanged {
                id: WidgetId::new("s"),
                value: expected,
            }),
            "a press at fraction {fraction} of a labeled slider's track"
        );
    }
}

/// A press square on a labeled slider's label text is not a track
/// interaction, so it must not resolve to `fraction == 0.0` → `min`.
/// It falls through to `form_click_event`'s generic `FocusChanged`
/// instead, the same outcome a click on any other labeled field's
/// label already gets.
#[test]
fn click_on_a_labeled_sliders_label_column_only_focuses_it() {
    let mut fc = labeled_slider_controller(5.0);
    let track = fallback_track_rect(&fc, "s");
    let ev = mouse_down_at(&mut fc, track.x / 2.0);
    assert_eq!(
        ev,
        Handled::Event(FormEvent::FocusChanged {
            id: WidgetId::new("s"),
        }),
        "a click on a labeled slider's label column must only focus it, never move its value"
    );
}

/// Nor is a press on the value text painted after the track: the row
/// extends well past the track's right edge, and mapping an `x`
/// there would snap the value to `max`.
#[test]
fn click_right_of_a_sliders_track_only_focuses_it() {
    let mut fc = labeled_slider_controller(5.0);
    let track = fallback_track_rect(&fc, "s");
    let past_end = track.x + track.width + 1.0;
    assert!(
        past_end < SLIDER_RECT.width,
        "the probe must still land inside the field's own row"
    );
    let ev = mouse_down_at(&mut fc, past_end);
    assert_eq!(
        ev,
        Handled::Event(FormEvent::FocusChanged {
            id: WidgetId::new("s"),
        }),
        "a click past a slider's track must only focus it, never snap its value to max"
    );
}

/// A `MouseMoved` with the left button still held, after a `MouseDown`
/// armed `slider_drag` on the same field, keeps mapping the pointer's
/// x position to a value the same way the initial click did — and a
/// drag the track already captured pins to `max` past its right edge
/// rather than dropping out, unlike a fresh press there.
#[test]
fn drag_after_a_sliders_click_continues_mapping_the_pointer_to_a_value() {
    let mut fc = labeled_slider_controller(0.0);
    let track = fallback_track_rect(&fc, "s");
    let down = mouse_down_at(&mut fc, track.x + track.width / 2.0);
    assert_eq!(
        down,
        Handled::Event(FormEvent::SliderChanged {
            id: WidgetId::new("s"),
            value: 5.0,
        })
    );

    let moved = left_drag_to(&mut fc, track.x + track.width * 0.75);
    assert_eq!(
        moved,
        Handled::Event(FormEvent::SliderChanged {
            id: WidgetId::new("s"),
            value: 7.5,
        }),
        "a drag after the click should keep mapping the pointer's x to a value"
    );

    let past_end = left_drag_to(&mut fc, track.x + track.width + 50.0);
    assert_eq!(
        past_end,
        Handled::Event(FormEvent::SliderChanged {
            id: WidgetId::new("s"),
            value: 10.0,
        }),
        "dragging past the track's right edge pins the value to max"
    );

    let up = fc.handle_cached(
        &UiEvent::MouseUp {
            widget: None,
            button: MouseButton::Left,
            position: crate::Point { x: 150.0, y: 7.0 },
        },
        SLIDER_RECT,
    );
    assert_eq!(up, Handled::Consumed);

    // `MouseUp` cleared `slider_drag` — a further move with the left
    // button held (as if a fresh, unrelated drag started elsewhere)
    // must not still be attributed to this slider.
    let moved_after_release = left_drag_to(&mut fc, track.x + 1.0);
    assert_eq!(moved_after_release, Handled::Ignored);
}

/// A press on a labeled slider's label column focuses the row
/// without capturing the pointer, so the drag that follows it is not
/// attributed to the slider either — otherwise a press meant to
/// focus the field would start rewriting its value the moment the
/// pointer moved.
#[test]
fn a_label_column_press_does_not_arm_a_sliders_drag() {
    let mut fc = labeled_slider_controller(5.0);
    let track = fallback_track_rect(&fc, "s");
    let _ = mouse_down_at(&mut fc, track.x / 2.0);
    assert_eq!(
        left_drag_to(&mut fc, track.x + track.width),
        Handled::Ignored,
        "a drag from a label-column press must not move the slider's value"
    );
}

#[test]
fn a_disabled_sliders_click_and_keys_produce_focus_changed_or_ignored_only() {
    let mut fc = controller_with(vec![slider_field("s", 5.0, 0.0, 10.0, 1.0, true)], None);
    fc.set_backend_info(10.0);
    let click = mouse_down_at(&mut fc, 100.0);
    assert_eq!(
        click,
        Handled::Event(FormEvent::FocusChanged {
            id: WidgetId::new("s"),
        }),
        "a disabled Slider's click must only focus it, never move its value"
    );

    fc.set_has_focus(true);
    if let Some(form) = &mut fc.form {
        form.focused_field = Some(WidgetId::new("s"));
    }
    let key = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        key,
        Handled::Ignored,
        "a disabled Slider's keys must not intercept at all"
    );
}

// ── RadioGroup routing ──────────────────────────────────────────

fn radio_group(selected: Option<usize>) -> RadioGroup {
    let mut group = RadioGroup::new(
        WidgetId::new("theme"),
        vec![
            RadioOption::new(WidgetId::new("theme:light"), "Light"),
            RadioOption::new(WidgetId::new("theme:dark"), "Dark"),
        ],
    );
    group.selected = selected;
    group
}

#[test]
fn down_arrow_moves_a_focused_radio_groups_selection() {
    let mut fc = controller_with(
        vec![radio_group_field("g", radio_group(Some(0)), false)],
        Some("g"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Down), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::RadioChanged {
            id: WidgetId::new("g"),
            selected: 1,
        })
    );
}

#[test]
fn left_arrow_moves_a_focused_radio_groups_selection_back() {
    let mut fc = controller_with(
        vec![radio_group_field("g", radio_group(Some(1)), false)],
        Some("g"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Left), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::RadioChanged {
            id: WidgetId::new("g"),
            selected: 0,
        })
    );
}

#[test]
fn disabled_radio_group_field_ignores_its_keys() {
    let mut fc = controller_with(
        vec![radio_group_field("g", radio_group(Some(0)), true)],
        Some("g"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Down), Modifiers::default()),
        RECT,
    );
    assert_eq!(ev, Handled::Ignored);
}

#[test]
fn tab_moves_focus_away_from_a_focused_radio_group_field_normally() {
    let mut fc = controller_with(
        vec![
            radio_group_field("g", radio_group(Some(0)), false),
            toggle_field("t", false),
        ],
        Some("g"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::FocusChanged {
            id: WidgetId::new("t")
        }),
        "Tab isn't one of RadioGroup's claimed keys, so focus traversal \
             off a focused RadioGroup field still works"
    );
}

#[test]
fn click_radio_group_option_emits_radio_changed_with_the_field_id() {
    let mut form = make_form(1);
    form.fields[0] = radio_group_field("g", radio_group(Some(0)), false);
    let ev = form_click_event(&form, &WidgetId::new("theme:dark"));
    assert_eq!(
        ev,
        FormEvent::RadioChanged {
            id: WidgetId::new("g"),
            selected: 1,
        },
        "the event must carry the *field's* id, not the clicked option's"
    );
}

#[test]
fn click_disabled_radio_group_option_focuses_the_field_instead_of_selecting() {
    let mut group = radio_group(Some(0));
    group.options[1].disabled = true;
    let mut form = make_form(1);
    form.fields[0] = radio_group_field("g", group, false);
    let ev = form_click_event(&form, &WidgetId::new("theme:dark"));
    assert_eq!(
        ev,
        FormEvent::FocusChanged {
            id: WidgetId::new("g"),
        }
    );
}

/// A real `MouseDown`, routed through `Form::layout`/`hit_test` via
/// `TuiBackend` (not `form_click_event` called directly, which
/// bypasses layout entirely) — a `LabelPosition::Top`-resolved
/// `RadioGroup`'s painted "Light" row must select "Light", not the
/// option one row below it (see `tui_form_layout_with_label_width`'s
/// `FieldKind::RadioGroup` arm and `items_start_y` on
/// `FormFieldMeasure`).
#[test]
#[cfg(feature = "tui")]
fn click_on_a_top_labelled_radio_groups_painted_row_selects_the_right_option() {
    use crate::primitives::form::LabelPosition;
    use crate::tui::TuiBackend;

    let mut field = radio_group_field("g", radio_group(Some(0)), false);
    field.label = StyledText::plain("Theme");
    field.label_position = Some(LabelPosition::Top);
    let mut fc = controller_with(vec![field], None);
    let mut backend = TuiBackend::new();
    let rect = Rect::new(0.0, 0.0, 40.0, 10.0);

    // Row 0 of the field's own bounds is the label ("Theme"); row 1
    // is "Light" (the first, currently-selected option); row 2 is
    // "Dark".
    let field_rect = fc.field_rect(WidgetId::new("g"), &backend, rect).unwrap();
    let light_y = field_rect.y + 1.0;
    let ev = fc.handle(
        &UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: crate::Point {
                x: field_rect.x + 1.0,
                y: light_y + 0.5,
            },
            modifiers: Default::default(),
        },
        &mut backend,
        rect,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::RadioChanged {
            id: WidgetId::new("g"),
            selected: 0,
        }),
        "a click on the row painted \"Light\" must select \"Light\" (index 0), not \"Dark\""
    );
}

// ── Dropdown routing ───────────────────────────────────────────

#[test]
fn down_arrow_moves_and_commits_dropdown_selection() {
    let mut fc = controller_with(
        vec![dropdown_field(
            "d",
            &["Apple", "Banana", "Cherry"],
            0,
            false,
        )],
        Some("d"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Down), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::DropdownChanged {
            id: WidgetId::new("d"),
            selected_idx: 1,
        }),
        "Down must move AND commit in the same keystroke — Dropdown \
             has no separate open/highlight step"
    );
}

#[test]
fn up_arrow_moves_dropdown_selection_back() {
    let mut fc = controller_with(
        vec![dropdown_field(
            "d",
            &["Apple", "Banana", "Cherry"],
            2,
            false,
        )],
        Some("d"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Up), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::DropdownChanged {
            id: WidgetId::new("d"),
            selected_idx: 1,
        })
    );
}

#[test]
fn up_arrow_at_first_option_falls_through_instead_of_wrapping() {
    let mut fc = controller_with(
        vec![dropdown_field("d", &["Apple", "Banana"], 0, false)],
        Some("d"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Up), Modifiers::default()),
        RECT,
    );
    assert_eq!(ev, Handled::Ignored);
}

#[test]
fn down_arrow_at_last_option_falls_through_instead_of_wrapping() {
    let mut fc = controller_with(
        vec![dropdown_field("d", &["Apple", "Banana"], 1, false)],
        Some("d"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Down), Modifiers::default()),
        RECT,
    );
    assert_eq!(ev, Handled::Ignored);
}

#[test]
fn disabled_dropdown_field_ignores_arrow_keys() {
    let mut fc = controller_with(
        vec![dropdown_field("d", &["Apple", "Banana"], 0, true)],
        Some("d"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Down), Modifiers::default()),
        RECT,
    );
    assert_eq!(ev, Handled::Ignored);
}

#[test]
fn tab_moves_focus_away_from_a_focused_dropdown_field_normally() {
    let mut fc = controller_with(
        vec![
            dropdown_field("d", &["Apple", "Banana"], 0, false),
            toggle_field("t", false),
        ],
        Some("d"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::FocusChanged {
            id: WidgetId::new("t")
        }),
        "Tab isn't one of Dropdown's claimed keys, so focus traversal \
             off a focused Dropdown field still works"
    );
}

#[test]
fn ctrl_modified_dropdown_key_falls_through_instead_of_navigating() {
    let mut fc = controller_with(
        vec![dropdown_field("d", &["Apple", "Banana"], 0, false)],
        Some("d"),
    );
    let ev = fc.handle_cached(&key_event(Key::Named(NamedKey::Down), ctrl()), RECT);
    assert_eq!(
        ev,
        Handled::Ignored,
        "Ctrl+Down is a host accelerator candidate, not dropdown \
             navigation, so it must not be swallowed"
    );
    let form = fc.build_form(RECT, None);
    match &form.fields[0].kind {
        FieldKind::Dropdown { selected_idx, .. } => {
            assert_eq!(*selected_idx, 0, "selection must not have changed")
        }
        other => panic!("expected Dropdown, got {other:?}"),
    }
}

// ── Item-level keyboard focus ──────────────────────────────────────
//
// See `primitives::form::contract`'s "Item-level keyboard focus"
// doc section.

fn button_row_field(id: &str, buttons: &[(&str, bool)]) -> FormField {
    FormField {
        id: WidgetId::new(id),
        label: StyledText::default(),
        kind: FieldKind::ButtonRow {
            buttons: buttons
                .iter()
                .map(|(bid, disabled)| crate::primitives::form::ButtonRowItem {
                    id: WidgetId::new(*bid),
                    label: (*bid).to_string(),
                    disabled: *disabled,
                    icon: None,
                    spec: None,
                })
                .collect(),
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
    }
}

fn toggle_group_field(id: &str, toggles: &[(&str, bool)]) -> FormField {
    FormField {
        id: WidgetId::new(id),
        label: StyledText::default(),
        kind: FieldKind::ToggleGroup {
            toggles: toggles
                .iter()
                .map(|(tid, value)| ToggleGroupItem {
                    id: WidgetId::new(*tid),
                    label: (*tid).to_string(),
                    value: *value,
                })
                .collect(),
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
    }
}

fn toolbar_field(id: &str, buttons: &[(&str, bool)]) -> FormField {
    use crate::primitives::toolbar::{Toolbar, ToolbarButton};
    FormField {
        id: WidgetId::new(id),
        label: StyledText::default(),
        kind: FieldKind::Toolbar(Toolbar {
            id: WidgetId::new(format!("{id}-toolbar")),
            buttons: buttons
                .iter()
                .map(|(bid, enabled)| ToolbarButton::Action {
                    spec: None,
                    id: WidgetId::new(*bid),
                    label: (*bid).to_string(),
                    icon: None,
                    key_hint: None,
                    enabled: *enabled,
                    is_active: false,
                    tooltip: String::new(),
                })
                .collect(),
            bg: None,
            focused_index: None,
            ..Default::default()
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
    }
}

#[test]
fn right_then_enter_activates_the_second_button_in_a_button_row() {
    let mut fc = controller_with(
        vec![button_row_field("row", &[("a", false), ("b", false)])],
        Some("row"),
    );
    let moved = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
        RECT,
    );
    assert_eq!(moved, Handled::Consumed);
    let activated = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        activated,
        Handled::Event(FormEvent::ButtonClicked {
            id: WidgetId::new("b")
        })
    );
}

#[test]
fn space_also_activates_the_focused_button() {
    let mut fc = controller_with(
        vec![button_row_field("row", &[("a", false), ("b", false)])],
        Some("row"),
    );
    let ev = fc.handle_cached(&key_event(Key::Char(' '), Modifiers::default()), RECT);
    assert_eq!(
        ev,
        Handled::Event(FormEvent::ButtonClicked {
            id: WidgetId::new("a")
        }),
        "Space activates the first (default-focused) item without a prior Right"
    );
}

#[test]
fn space_and_enter_flip_a_focused_plain_toggle() {
    let mut fc = controller_with(vec![toggle_field("flag", false)], Some("flag"));
    let ev = fc.handle_cached(&key_event(Key::Char(' '), Modifiers::default()), RECT);
    assert_eq!(
        ev,
        Handled::Event(FormEvent::ToggleChanged {
            id: WidgetId::new("flag"),
            value: true,
        }),
        "Space must flip a focused plain Toggle field, not just a mouse click"
    );

    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::ToggleChanged {
            id: WidgetId::new("flag"),
            value: true,
        }),
        "Enter must flip a focused plain Toggle field too"
    );
}

#[test]
fn disabled_toggle_ignores_space_and_enter() {
    let mut fc = controller_with(vec![toggle_field("flag", true)], Some("flag"));
    let ev = fc.handle_cached(&key_event(Key::Char(' '), Modifiers::default()), RECT);
    assert_eq!(ev, Handled::Ignored);
}

#[test]
fn char_typed_space_flips_a_focused_toggle_the_same_as_a_keypressed_space() {
    // A space delivered as a raw `CharTyped` (some backends don't
    // route a printable key through `KeyPressed` at all) must
    // activate the toggle too, not fall through to
    // `handle_text_op`'s no-op for a field with no text value.
    let mut fc = controller_with(vec![toggle_field("flag", false)], Some("flag"));
    let ev = fc.handle_cached(&UiEvent::CharTyped(' '), RECT);
    assert_eq!(
        ev,
        Handled::Event(FormEvent::ToggleChanged {
            id: WidgetId::new("flag"),
            value: true,
        })
    );
}

#[test]
fn char_typed_space_activates_a_focused_button_row_item_too() {
    let mut fc = controller_with(
        vec![button_row_field("row", &[("a", false), ("b", false)])],
        Some("row"),
    );
    let ev = fc.handle_cached(&UiEvent::CharTyped(' '), RECT);
    assert_eq!(
        ev,
        Handled::Event(FormEvent::ButtonClicked {
            id: WidgetId::new("a")
        })
    );
}

#[test]
fn char_typed_space_still_inserts_into_a_focused_text_field() {
    // `activate_focused_on_space` must return `None` for a field
    // kind it has no activation story for, so a text field's own
    // space character keeps inserting normally.
    let mut fc = controller_with(vec![text_field("name", "ab", Some(2))], Some("name"));
    let ev = fc.handle_cached(&UiEvent::CharTyped(' '), RECT);
    assert_eq!(
        ev,
        Handled::Event(FormEvent::TextInputChanged {
            id: WidgetId::new("name"),
            value: "ab ".to_string(),
        })
    );
}

#[test]
fn disabled_first_button_item_focus_starts_on_the_second() {
    let mut fc = controller_with(
        vec![button_row_field("row", &[("a", true), ("b", false)])],
        Some("row"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::ButtonClicked {
            id: WidgetId::new("b")
        }),
        "item-focus must skip the disabled first button and default to \
             the second, with no prior Right keypress needed"
    );
}

#[test]
fn right_skips_a_disabled_middle_button() {
    let mut fc = controller_with(
        vec![button_row_field(
            "row",
            &[("a", false), ("b", true), ("c", false)],
        )],
        Some("row"),
    );
    let _ = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
        RECT,
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::ButtonClicked {
            id: WidgetId::new("c")
        }),
        "Right from 'a' must land on 'c', skipping disabled 'b'"
    );
}

#[test]
fn right_at_the_last_enabled_item_is_a_noop() {
    let mut fc = controller_with(
        vec![button_row_field("row", &[("a", false), ("b", false)])],
        Some("row"),
    );
    let _ = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
        RECT,
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Ignored,
        "no wrap: Right at the last item stays put"
    );
    let activated = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        activated,
        Handled::Event(FormEvent::ButtonClicked {
            id: WidgetId::new("b")
        }),
        "item-focus is still on 'b', not pushed past it"
    );
}

#[test]
fn left_at_the_first_enabled_item_is_a_noop() {
    let mut fc = controller_with(
        vec![button_row_field("row", &[("a", false), ("b", false)])],
        Some("row"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Left), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Ignored,
        "no wrap: Left at the first item stays put"
    );
}

#[test]
fn ctrl_modified_right_falls_through_instead_of_moving_item_focus() {
    let mut fc = controller_with(
        vec![button_row_field("row", &[("a", false), ("b", false)])],
        Some("row"),
    );
    let ev = fc.handle_cached(&key_event(Key::Named(NamedKey::Right), ctrl()), RECT);
    assert_eq!(
        ev,
        Handled::Ignored,
        "Ctrl+Right is a host accelerator candidate, not item navigation"
    );
    let activated = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        activated,
        Handled::Event(FormEvent::ButtonClicked {
            id: WidgetId::new("a")
        }),
        "item-focus must still be on the first item — Ctrl+Right never moved it"
    );
}

#[test]
fn disabled_button_row_field_ignores_item_navigation_keys() {
    let mut fc = controller_with(
        vec![FormField {
            disabled: true,
            ..button_row_field("row", &[("a", false), ("b", false)])
        }],
        Some("row"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
        RECT,
    );
    assert_eq!(ev, Handled::Ignored);
}

#[test]
fn tab_moves_focus_away_from_a_button_row_normally() {
    let mut fc = controller_with(
        vec![
            button_row_field("row", &[("a", false), ("b", false)]),
            toggle_field("t", false),
        ],
        Some("row"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::FocusChanged {
            id: WidgetId::new("t")
        }),
        "Tab moves between fields, never into an item"
    );
}

#[test]
fn tab_into_a_button_row_seeds_item_focus_on_the_first_enabled_item() {
    let mut fc = controller_with(
        vec![
            toggle_field("t", false),
            button_row_field("row", &[("a", true), ("b", false)]),
        ],
        Some("t"),
    );
    let _ = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
        RECT,
    );
    // `move_focus` only emits `FocusChanged` — the app persists the new
    // `focused_field` itself (`primitives::form::contract`'s "Keyboard editing" doc
    // section) — so push it back before the next keystroke.
    fc.set_form(Form {
        focused_field: Some(WidgetId::new("row")),
        ..make_form_with(vec![
            toggle_field("t", false),
            button_row_field("row", &[("a", true), ("b", false)]),
        ])
    });
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::ButtonClicked {
            id: WidgetId::new("b")
        }),
        "Tab onto the row must seed item-focus on 'b', skipping disabled 'a'"
    );
}

#[test]
fn item_focus_resets_when_leaving_and_returning_to_a_toggle_group() {
    let fields = || {
        vec![
            toggle_group_field("toggles", &[("x", false), ("y", false)]),
            toggle_field("t", false),
        ]
    };
    let mut fc = controller_with(fields(), Some("toggles"));
    let _ = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
        RECT,
    );
    // Tab away, then Tab back (BackTab), round-tripping through the app
    // persisting `focused_field` the same way `tab_into_a_button_row_...`
    // above does.
    let _ = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Tab), Modifiers::default()),
        RECT,
    );
    fc.set_form(Form {
        focused_field: Some(WidgetId::new("t")),
        ..make_form_with(fields())
    });
    let _ = fc.handle_cached(
        &key_event(Key::Named(NamedKey::BackTab), Modifiers::default()),
        RECT,
    );
    fc.set_form(Form {
        focused_field: Some(WidgetId::new("toggles")),
        ..make_form_with(fields())
    });

    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::ToggleChanged {
            id: WidgetId::new("x"),
            value: true,
        }),
        "returning to the toggle group must reset item-focus to the \
             first item, not resume at 'y' from before Tab left"
    );
}

#[test]
fn right_then_enter_activates_the_second_toggle_in_a_toggle_group() {
    let mut fc = controller_with(
        vec![toggle_group_field("toggles", &[("x", true), ("y", false)])],
        Some("toggles"),
    );
    let _ = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
        RECT,
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::ToggleChanged {
            id: WidgetId::new("y"),
            value: true,
        }),
        "activating 'y' must flip its own value, not 'x's"
    );
}

#[test]
fn enter_activates_the_focused_toolbar_button() {
    let mut fc = controller_with(
        vec![toolbar_field("tb", &[("reset", true), ("export", true)])],
        Some("tb"),
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::ToolbarButtonClicked {
            field_id: WidgetId::new("tb"),
            button_id: WidgetId::new("reset"),
        })
    );
}

#[test]
fn right_skips_a_disabled_toolbar_button() {
    let mut fc = controller_with(
        vec![toolbar_field(
            "tb",
            &[("reset", true), ("broken", false), ("export", true)],
        )],
        Some("tb"),
    );
    let _ = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Right), Modifiers::default()),
        RECT,
    );
    let ev = fc.handle_cached(
        &key_event(Key::Named(NamedKey::Enter), Modifiers::default()),
        RECT,
    );
    assert_eq!(
        ev,
        Handled::Event(FormEvent::ToolbarButtonClicked {
            field_id: WidgetId::new("tb"),
            button_id: WidgetId::new("export"),
        }),
        "Right from 'reset' must land on 'export', skipping the \
             disabled ('enabled: false') 'broken' button"
    );
}

#[test]
fn mouse_click_on_an_item_does_not_touch_item_focus_state() {
    // Mouse behaviour is unchanged: `form_click_event` resolves a click
    // straight to the clicked item's event without going through
    // `item_focus` at all.
    let form = make_form_with(vec![button_row_field("row", &[("a", false), ("b", false)])]);
    let ev = form_click_event(&form, &WidgetId::new("b"));
    assert_eq!(
        ev,
        FormEvent::ButtonClicked {
            id: WidgetId::new("b")
        }
    );
}

fn make_form_with(fields: Vec<FormField>) -> Form {
    Form {
        id: WidgetId::new("form"),
        fields,
        focused_field: None,
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

// ── Item-level keyboard focus: painted focus ring ───────────────────
//
// Drives a real backend through a minimal `AppLogic` wrapper — see
// `field_rect_tests`'s doc comment above for why a hand-rolled
// `Backend` mock won't do: the ring's rect (resolved via the
// `item_bounds` of `DrawsPrimitives::layout_primitive`, same as
// `Self::draw_item_focus_ring`'s doc describes) needs a backend with
// a real layout algorithm. `ItemFocusRingApp` is backend-agnostic
// (it only ever sees `&mut dyn Backend`), shared by the TUI and GTK
// driver tests below it — the two backends disagree on what a
// "reasonable" rect looks like (cells vs. device pixels), so each
// picks its own.
#[cfg(any(feature = "tui", feature = "gtk"))]
struct ItemFocusRingApp {
    fc: Host,
    rect: Rect,
}

#[cfg(any(feature = "tui", feature = "gtk"))]
impl ItemFocusRingApp {
    fn new(buttons: &[(&str, bool)], rect: Rect) -> Self {
        Self::with_field(button_row_field("row", buttons), rect)
    }

    fn new_toolbar(buttons: &[(&str, bool)], rect: Rect) -> Self {
        Self::with_field(toolbar_field("row", buttons), rect)
    }

    fn with_field(field: FormField, rect: Rect) -> Self {
        let mut fc = Host::new("row-fc".into());
        fc.set_form(Form {
            focused_field: Some(WidgetId::new("row")),
            ..make_form_with(vec![field])
        });
        fc.set_has_focus(true);
        Self { fc, rect }
    }
}

#[cfg(any(feature = "tui", feature = "gtk"))]
impl crate::runner::AppLogic for ItemFocusRingApp {
    type AreaId = ();
    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        self.fc.render(backend, self.rect);
    }
    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> crate::runner::Reaction {
        let _ = self.fc.handle(&event, backend, self.rect);
        crate::runner::Reaction::Redraw
    }
}

#[cfg(feature = "tui")]
mod item_focus_ring_tui_tests {
    use super::*;
    use crate::tui::testing::{Modifier, TuiDriver};

    const ITEM_ROW_RECT: Rect = Rect::new(0.0, 0.0, 40.0, 3.0);

    fn driver(buttons: &[(&str, bool)]) -> TuiDriver<ItemFocusRingApp> {
        TuiDriver::new(ItemFocusRingApp::new(buttons, ITEM_ROW_RECT), 40, 3)
    }

    fn underlined_at(driver: &TuiDriver<ItemFocusRingApp>, needle: &str) -> bool {
        let bounds = driver
            .find_bounds(needle)
            .unwrap_or_else(|| panic!("{needle:?} not painted on screen"));
        driver
            .style_at(bounds.x as u16, bounds.y as u16)
            .is_some_and(|s| s.modifiers.contains(Modifier::UNDERLINED))
    }

    #[test]
    fn the_ring_starts_on_the_first_enabled_button() {
        let driver = driver(&[("Connect", false), ("Cancel", false)]);
        assert!(underlined_at(&driver, "Connect"));
        assert!(!underlined_at(&driver, "Cancel"));
    }

    #[test]
    fn right_moves_the_ring_to_the_second_button() {
        let mut driver = driver(&[("Connect", false), ("Cancel", false)]);
        driver.press_named(NamedKey::Right);
        assert!(
            !underlined_at(&driver, "Connect"),
            "the ring must leave the first button"
        );
        assert!(
            underlined_at(&driver, "Cancel"),
            "the ring must land on the second button"
        );
    }

    #[test]
    fn disabled_first_button_the_ring_starts_on_the_second() {
        let driver = driver(&[("Connect", true), ("Cancel", false)]);
        assert!(!underlined_at(&driver, "Connect"));
        assert!(underlined_at(&driver, "Cancel"));
    }

    #[test]
    fn right_moves_the_ring_between_toolbar_buttons_too() {
        let mut driver = TuiDriver::new(
            ItemFocusRingApp::new_toolbar(&[("Reset", true), ("Export", true)], ITEM_ROW_RECT),
            40,
            3,
        );
        assert!(underlined_at(&driver, "Reset"));
        driver.press_named(NamedKey::Right);
        assert!(
            !underlined_at(&driver, "Reset"),
            "the ring must leave the first toolbar button"
        );
        assert!(
            underlined_at(&driver, "Export"),
            "the ring must land on the second toolbar button"
        );
    }
}

// GTK: `Backend::draw_focus_ring` strokes the exact rect it's given
// (`GtkBackend::draw_focus_ring`, via `surface_stroke_rect`) rather
// than TUI's border-box/underline special case, so — unlike the TUI
// tests above — this samples pixel colour instead of a cell
// modifier: `theme.accent_fg` at the stroke's edge when the ring
// paints there, the row's own background otherwise.
#[cfg(feature = "gtk")]
mod item_focus_ring_gtk_tests {
    use super::*;
    use crate::gtk::testing::GtkDriver;

    const ITEM_ROW_RECT: Rect = Rect::new(0.0, 0.0, 400.0, 120.0);

    fn driver(buttons: &[(&str, bool)]) -> GtkDriver<ItemFocusRingApp> {
        GtkDriver::new(ItemFocusRingApp::new(buttons, ITEM_ROW_RECT), 400, 120)
    }

    /// `needle`'s item's own rect (not its text glyph's, which sits
    /// well inside the item's padded background) — the same
    /// `DrawsPrimitives::layout_primitive` lookup
    /// `ScrolledForm`'s own item focus ring uses, so the sample
    /// point agrees with what the ring actually outlines.
    fn item_rect(driver: &GtkDriver<ItemFocusRingApp>, item_id: &str) -> Rect {
        let app = driver.app();
        let form = app.fc.form().expect("form set").clone();
        let layout =
            crate::DrawsPrimitives::layout_primitive(driver.backend(), app.rect, &form).layout;
        let vf = layout
            .visible_fields
            .iter()
            .find(|vf| form.fields[vf.field_idx].id == WidgetId::new("row"))
            .expect("row field painted");
        let (_, r) = vf
            .item_bounds
            .iter()
            .find(|(id, _)| id.as_str() == item_id)
            .unwrap_or_else(|| panic!("item {item_id:?} not in item_bounds"));
        Rect::new(app.rect.x + r.x, app.rect.y + r.y, r.width, r.height)
    }

    /// Sample one pixel on the stroke along `item_id`'s left edge —
    /// `focus_ring_width` defaults to 2.0px (`Style::default`), and
    /// `GtkBackend::draw_focus_ring` strokes centred on the rect's
    /// own edge, so the rect's left-edge column, vertically centred,
    /// lands solidly inside the stroke band when the ring painted
    /// there.
    fn ring_painted_at(driver: &mut GtkDriver<ItemFocusRingApp>, item_id: &str) -> bool {
        let r = item_rect(driver, item_id);
        let theme = driver.backend().theme();
        let accent = (theme.accent_fg.r, theme.accent_fg.g, theme.accent_fg.b);
        let x = r.x.round().max(0.0) as i32;
        let y = (r.y + r.height / 2.0).round() as i32;
        driver.pixel(x, y) == accent
    }

    #[test]
    fn the_ring_starts_on_the_first_enabled_button() {
        let mut driver = driver(&[("Connect", false), ("Cancel", false)]);
        assert!(ring_painted_at(&mut driver, "Connect"));
        assert!(!ring_painted_at(&mut driver, "Cancel"));
    }

    #[test]
    fn right_moves_the_ring_to_the_second_button() {
        let mut driver = driver(&[("Connect", false), ("Cancel", false)]);
        driver.press_named(NamedKey::Right);
        assert!(
            !ring_painted_at(&mut driver, "Connect"),
            "the ring must leave the first button"
        );
        assert!(
            ring_painted_at(&mut driver, "Cancel"),
            "the ring must land on the second button"
        );
    }

    #[test]
    fn disabled_first_button_the_ring_starts_on_the_second() {
        let mut driver = driver(&[("Connect", true), ("Cancel", false)]);
        assert!(!ring_painted_at(&mut driver, "Connect"));
        assert!(ring_painted_at(&mut driver, "Cancel"));
    }

    #[test]
    fn right_moves_the_ring_between_toolbar_buttons_too() {
        let mut driver = GtkDriver::new(
            ItemFocusRingApp::new_toolbar(&[("Reset", true), ("Export", true)], ITEM_ROW_RECT),
            400,
            120,
        );
        assert!(ring_painted_at(&mut driver, "Reset"));
        driver.press_named(NamedKey::Right);
        assert!(
            !ring_painted_at(&mut driver, "Reset"),
            "the ring must leave the first toolbar button"
        );
        assert!(
            ring_painted_at(&mut driver, "Export"),
            "the ring must land on the second toolbar button"
        );
    }
}
