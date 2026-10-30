//! Declarative validation rules for [`Form`] fields.
//!
//! Today [`crate::primitives::form::FormField::validation`] only carries
//! *state* (`Some(ValidationState::Error("..."))`) — something upstream has
//! to compute that string by hand, in host code, every frame. This module
//! adds the other half: a serializable [`Rule`] (a built-in check like
//! `required`/`pattern`/`min_length`, or an arbitrary cross-field
//! [`crate::expr::Expr`]) plus a message and [`Severity`], collected per
//! field in a [`FormRules`]. [`validate`] evaluates a `FormRules` against a
//! `Form`'s current field values and writes the result straight into
//! `FormField.validation` — so declaring the rules once is enough; no host
//! callback runs on every keystroke.
//!
//! Like [`crate::expr`], this is data, not closures, so every language
//! binding (Lua, Python, a future C-ABI) ships the same plain-data rule set
//! and gets identical validation behaviour out of quadraui.
//!
//! # Additive, not a replacement
//!
//! The imperative path — a host setting `FormField.validation` directly —
//! keeps working unmodified. [`validate`] only touches fields that have at
//! least one [`Rule`] declared for them in the `FormRules` it's given; any
//! field absent from `FormRules::fields` is left exactly as the caller set
//! it. A host that never calls [`validate`] (or never populates
//! `FormRules`) sees no behavior change at all.
//!
//! # Cross-field rules
//!
//! [`Validator::Expr`] wraps a [`crate::expr::Expr`] evaluated against
//! every field's current value (not just the rule's own field) — the same
//! mechanism [`crate::expr`]'s module doc describes for conditional
//! visibility. A `confirm_password == password` check, or "`end_date` must
//! be after `start_date`", is expressed the same way a single-field
//! `pattern` rule is: as data, attached to whichever field should carry the
//! resulting message (typically the "second" field in the pair).
//!
//! # Usage
//!
//! ```
//! use quadraui::primitives::form::{FieldKind, Form, FormField, ValidationState};
//! use quadraui::types::{StyledText, WidgetId};
//! use quadraui::validation::{validate, FormRules, Rule, Severity, Validator};
//!
//! let mut form = Form {
//!     id: WidgetId::new("form:signup"),
//!     fields: vec![FormField {
//!         id: WidgetId::new("form:signup:email"),
//!         label: StyledText::plain("Email"),
//!         kind: FieldKind::TextInput {
//!             value: "not-an-email".to_string(),
//!             placeholder: String::new(),
//!             cursor: None,
//!             selection_anchor: None,
//!         },
//!         hint: StyledText::plain(""),
//!         disabled: false,
//!         validation: None,
//!         label_position: None,
//!         label_align: None,
//!         label_width: None,
//!         tab_index: None,
//!     }],
//!     focused_field: None,
//!     scroll_offset: 0,
//!     has_focus: true,
//!     label_position: Default::default(),
//!     label_align: Default::default(),
//!     label_width: None,
//! };
//!
//! let mut rules = FormRules::default();
//! rules.fields.insert(
//!     "form:signup:email".to_string(),
//!     vec![
//!         Rule {
//!             check: Validator::Required,
//!             message: "Email is required".to_string(),
//!             severity: Severity::Error,
//!         },
//!         Rule {
//!             check: Validator::Pattern(r"^[^@]+@[^@]+\.[^@]+$".to_string()),
//!             message: "Enter a valid email address".to_string(),
//!             severity: Severity::Error,
//!         },
//!     ],
//! );
//!
//! validate(&mut form, &rules);
//! assert_eq!(
//!     form.fields[0].validation,
//!     Some(ValidationState::Error("Enter a valid email address".to_string()))
//! );
//! ```

use crate::expr::{Context, Expr, Value};
use crate::primitives::form::{FieldKind, Form, ValidationState};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How a failing [`Rule`] should render — mirrors
/// [`ValidationState`]'s two variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    fn into_state(self, message: String) -> ValidationState {
        match self {
            Severity::Error => ValidationState::Error(message),
            Severity::Warning => ValidationState::Warning(message),
        }
    }
}

/// One check a [`Rule`] can perform. Every built-in variant (everything but
/// [`Validator::Expr`]) tests the *field's own* current value; it is
/// deliberately vacuous (passes) when that value is the wrong shape for the
/// check (e.g. [`Validator::Min`] against a string) — combine with
/// [`Validator::Required`] in the field's rule list when emptiness should
/// also fail, the same way a JSON-Schema `required` array is separate from
/// a property's own `type`/`pattern`/`minimum`.
///
/// Only [`FieldKind`] variants that carry a plain scalar value feed one of
/// these: `Toggle` (bool), `TextInput`/`TextArea`/`PasswordInput` (string),
/// `Slider` (number), `Dropdown`/`SegmentedControl` (number, the selected
/// index), and `ComboBox` (string, the *committed* value — not the
/// in-progress `text` buffer, so a field mid-edit still validates against
/// its last confirmed value). Every other kind (`Label`, `Button`,
/// `ReadOnly`, `ColorPicker`, `ToggleGroup`, `ButtonRow`, `Toolbar`,
/// `DatePicker`) has no value [`validate`] can extract, so it always
/// evaluates as
/// [`crate::expr::Value::Null`] — fine for [`Validator::Expr`] (which can
/// still read every *other* field), but [`Validator::Required`] against one
/// of those kinds always fails and should not be declared.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Validator {
    /// Fails when the field's value is absent (`Value::Null`) or the empty
    /// string.
    Required,
    /// Fails when the field's string value does not match the regex
    /// `Pattern`. An invalid regex (one `regex::Regex::new` rejects)
    /// fails every value, matching [`Expr::Matches`]'s safe-default
    /// posture inverted for "the rule itself is broken, so nothing
    /// passes it" rather than silently accepting bad input.
    Pattern(String),
    /// Fails when the field's numeric value is less than `Min`.
    Min(f64),
    /// Fails when the field's numeric value is greater than `Max`.
    Max(f64),
    /// Fails when the field's string value has fewer than `MinLength`
    /// characters.
    MinLength(usize),
    /// Fails when the field's string value has more than `MaxLength`
    /// characters.
    MaxLength(usize),
    /// Fails when the field's string value is not one of `OneOf`.
    OneOf(Vec<String>),
    /// Cross-field: fails when `expr` evaluates falsy against the whole
    /// form's [`Context`] (every field's current value, keyed by
    /// `WidgetId::as_str()`), not just this rule's own field. See the
    /// module doc's "Cross-field rules" section.
    Expr(Expr),
}

impl Validator {
    /// `true` when this check passes for `value` (this rule's own field)
    /// given the full cross-field `ctx`.
    fn passes(&self, value: &Value, ctx: &Context) -> bool {
        match self {
            Validator::Required => !value.is_empty(),
            Validator::Pattern(pattern) => match value {
                Value::String(s) => regex::Regex::new(pattern)
                    .map(|re| re.is_match(s))
                    .unwrap_or(false),
                _ => true,
            },
            Validator::Min(min) => match value {
                Value::Number(n) => n >= min,
                _ => true,
            },
            Validator::Max(max) => match value {
                Value::Number(n) => n <= max,
                _ => true,
            },
            Validator::MinLength(min) => match value {
                Value::String(s) => s.chars().count() >= *min,
                _ => true,
            },
            Validator::MaxLength(max) => match value {
                Value::String(s) => s.chars().count() <= *max,
                _ => true,
            },
            Validator::OneOf(values) => match value {
                Value::String(s) => values.iter().any(|v| v == s),
                _ => true,
            },
            Validator::Expr(expr) => expr.eval_bool(ctx),
        }
    }
}

/// One validation check attached to a field: a [`Validator`] plus the
/// message and [`Severity`] to apply when it fails.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub check: Validator,
    pub message: String,
    pub severity: Severity,
}

/// Declarative validation rules for a whole [`Form`], keyed by field id
/// (`WidgetId::as_str()`). Serializable so a Lua/Python/C-ABI binding can
/// ship the same rule set quadraui evaluates — see the module doc and
/// `crate::expr`'s for why rules are data, not callbacks.
///
/// A field absent from `fields` has no declarative rules; [`validate`]
/// leaves its `validation` exactly as the caller set it (the imperative
/// escape-hatch path — see the module doc's "Additive" section).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FormRules {
    pub fields: BTreeMap<String, Vec<Rule>>,
}

/// Build the cross-field [`Context`] for `form`: one entry per field whose
/// [`FieldKind`] carries a plain scalar value (see [`Validator`]'s doc for
/// the exact list), keyed by `WidgetId::as_str()`. Fields of any other kind
/// are omitted (so [`Expr::Field`] against one evaluates to
/// `Value::Null`/absent, same as a field that doesn't exist at all).
pub fn form_context(form: &Form) -> Context {
    form.fields
        .iter()
        .map(|field| (field.id.as_str().to_string(), field_value(&field.kind)))
        .collect()
}

fn field_value(kind: &FieldKind) -> Value {
    match kind {
        FieldKind::Toggle { value } => Value::Bool(*value),
        FieldKind::TextInput { value, .. }
        | FieldKind::TextArea { value, .. }
        | FieldKind::PasswordInput { value, .. } => Value::String(value.clone()),
        FieldKind::Slider { value, .. } => Value::Number(*value as f64),
        FieldKind::Dropdown { selected_idx, .. }
        | FieldKind::SegmentedControl { selected_idx, .. } => Value::Number(*selected_idx as f64),
        FieldKind::ComboBox(combo) => Value::String(combo.committed.clone()),
        _ => Value::Null,
    }
}

/// Evaluate `rules` against `form`'s current field values and write the
/// result into each affected field's `FormField.validation`.
///
/// Rules within one field's list are checked in declaration order; the
/// first failure wins (matching Yup/Zod's "first error in the chain stops
/// there" behaviour) and its message/severity become the field's
/// `validation`. A field with a non-empty rule list where every rule
/// passes has its `validation` cleared to `None` — so fixing a field's
/// value clears a previously-shown error without any host logic. A field
/// absent from `rules.fields` is never touched (see the module doc).
pub fn validate(form: &mut Form, rules: &FormRules) {
    if rules.fields.is_empty() {
        return;
    }
    let ctx = form_context(form);
    for field in &mut form.fields {
        let Some(field_rules) = rules.fields.get(field.id.as_str()) else {
            continue;
        };
        let value = field_value(&field.kind);
        field.validation = field_rules.iter().find_map(|rule| {
            if rule.check.passes(&value, &ctx) {
                None
            } else {
                Some(rule.severity.into_state(rule.message.clone()))
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::form::FormField;
    use crate::types::{StyledText, WidgetId};

    fn text_field(id: &str, value: &str) -> FormField {
        FormField {
            id: WidgetId::new(id),
            label: StyledText::plain(id),
            kind: FieldKind::TextInput {
                value: value.to_string(),
                placeholder: String::new(),
                cursor: None,
                selection_anchor: None,
            },
            hint: StyledText::plain(""),
            disabled: false,
            validation: None,
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
        }
    }

    fn combo_field(id: &str, committed: &str) -> FormField {
        let mut combo = crate::primitives::combo_box::ComboBox::new(
            WidgetId::new(id),
            vec![
                "Apple".to_string(),
                "Banana".to_string(),
                "Cherry".to_string(),
            ],
        );
        combo.committed = committed.to_string();
        combo.text = committed.to_string();
        FormField {
            id: WidgetId::new(id),
            label: StyledText::plain(id),
            kind: FieldKind::ComboBox(combo),
            hint: StyledText::plain(""),
            disabled: false,
            validation: None,
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
        }
    }

    fn form_with(fields: Vec<FormField>) -> Form {
        Form {
            id: WidgetId::new("form:test"),
            fields,
            focused_field: None,
            scroll_offset: 0,
            has_focus: true,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
        }
    }

    fn rule(check: Validator, message: &str) -> Rule {
        Rule {
            check,
            message: message.to_string(),
            severity: Severity::Error,
        }
    }

    #[test]
    fn required_fails_on_empty_string() {
        let mut form = form_with(vec![text_field("f:name", "")]);
        let mut rules = FormRules::default();
        rules.fields.insert(
            "f:name".to_string(),
            vec![rule(Validator::Required, "Required")],
        );

        validate(&mut form, &rules);
        assert_eq!(
            form.fields[0].validation,
            Some(ValidationState::Error("Required".to_string()))
        );
    }

    #[test]
    fn required_passes_on_non_empty_and_clears_prior_error() {
        let mut form = form_with(vec![text_field("f:name", "Ada")]);
        form.fields[0].validation = Some(ValidationState::Error("stale".to_string()));
        let mut rules = FormRules::default();
        rules.fields.insert(
            "f:name".to_string(),
            vec![rule(Validator::Required, "Required")],
        );

        validate(&mut form, &rules);
        assert_eq!(form.fields[0].validation, None);
    }

    #[test]
    fn pattern_rejects_non_matching_value() {
        let mut form = form_with(vec![text_field("f:email", "nope")]);
        let mut rules = FormRules::default();
        rules.fields.insert(
            "f:email".to_string(),
            vec![rule(
                Validator::Pattern(r"^[^@]+@[^@]+\.[^@]+$".to_string()),
                "Invalid email",
            )],
        );

        validate(&mut form, &rules);
        assert_eq!(
            form.fields[0].validation,
            Some(ValidationState::Error("Invalid email".to_string()))
        );
    }

    #[test]
    fn min_length_and_max_length() {
        let mut form = form_with(vec![text_field("f:pw", "ab")]);
        let mut rules = FormRules::default();
        rules.fields.insert(
            "f:pw".to_string(),
            vec![rule(Validator::MinLength(8), "Too short")],
        );
        validate(&mut form, &rules);
        assert_eq!(
            form.fields[0].validation,
            Some(ValidationState::Error("Too short".to_string()))
        );

        let mut form = form_with(vec![text_field("f:pw", "a very long password indeed")]);
        let mut rules = FormRules::default();
        rules.fields.insert(
            "f:pw".to_string(),
            vec![rule(Validator::MaxLength(8), "Too long")],
        );
        validate(&mut form, &rules);
        assert_eq!(
            form.fields[0].validation,
            Some(ValidationState::Error("Too long".to_string()))
        );
    }

    #[test]
    fn min_and_max_on_slider() {
        let field = FormField {
            id: WidgetId::new("f:qty"),
            label: StyledText::plain("Qty"),
            kind: FieldKind::Slider {
                value: 0.0,
                min: 0.0,
                max: 100.0,
                step: 1.0,
            },
            hint: StyledText::plain(""),
            disabled: false,
            validation: None,
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
        };
        let mut form = form_with(vec![field]);
        let mut rules = FormRules::default();
        rules.fields.insert(
            "f:qty".to_string(),
            vec![rule(Validator::Min(1.0), "Too low")],
        );
        validate(&mut form, &rules);
        assert_eq!(
            form.fields[0].validation,
            Some(ValidationState::Error("Too low".to_string()))
        );
    }

    #[test]
    fn one_of_rejects_value_outside_allowed_set() {
        let mut form = form_with(vec![text_field("f:role", "wizard")]);
        let mut rules = FormRules::default();
        rules.fields.insert(
            "f:role".to_string(),
            vec![rule(
                Validator::OneOf(vec!["admin".to_string(), "guest".to_string()]),
                "Unknown role",
            )],
        );
        validate(&mut form, &rules);
        assert_eq!(
            form.fields[0].validation,
            Some(ValidationState::Error("Unknown role".to_string()))
        );
    }

    #[test]
    fn cross_field_expr_rule_compares_two_fields() {
        let mut form = form_with(vec![
            text_field("f:password", "secret123"),
            text_field("f:confirm", "different"),
        ]);
        let mut rules = FormRules::default();
        rules.fields.insert(
            "f:confirm".to_string(),
            vec![rule(
                Validator::Expr(Expr::Eq(
                    Box::new(Expr::Field("f:confirm".to_string())),
                    Box::new(Expr::Field("f:password".to_string())),
                )),
                "Passwords must match",
            )],
        );

        validate(&mut form, &rules);
        assert_eq!(
            form.fields[1].validation,
            Some(ValidationState::Error("Passwords must match".to_string()))
        );
        // The field the cross-field rule doesn't target is untouched.
        assert_eq!(form.fields[0].validation, None);

        form.fields[1] = text_field("f:confirm", "secret123");
        validate(&mut form, &rules);
        assert_eq!(form.fields[1].validation, None);
    }

    #[test]
    fn warning_severity_renders_as_warning_state() {
        let mut form = form_with(vec![text_field("f:nickname", "x")]);
        let mut rules = FormRules::default();
        rules.fields.insert(
            "f:nickname".to_string(),
            vec![Rule {
                check: Validator::MinLength(3),
                message: "Short nickname".to_string(),
                severity: Severity::Warning,
            }],
        );
        validate(&mut form, &rules);
        assert_eq!(
            form.fields[0].validation,
            Some(ValidationState::Warning("Short nickname".to_string()))
        );
    }

    #[test]
    fn field_without_declared_rules_is_left_untouched() {
        let mut form = form_with(vec![text_field("f:untouched", "")]);
        form.fields[0].validation = Some(ValidationState::Warning("keep me".to_string()));
        let mut rules = FormRules::default();
        rules.fields.insert(
            "f:other".to_string(),
            vec![rule(Validator::Required, "Required")],
        );

        validate(&mut form, &rules);
        assert_eq!(
            form.fields[0].validation,
            Some(ValidationState::Warning("keep me".to_string()))
        );
    }

    #[test]
    fn first_failing_rule_in_declaration_order_wins() {
        let mut form = form_with(vec![text_field("f:name", "")]);
        let mut rules = FormRules::default();
        rules.fields.insert(
            "f:name".to_string(),
            vec![
                rule(Validator::Required, "Required"),
                rule(Validator::MinLength(3), "Too short"),
            ],
        );
        validate(&mut form, &rules);
        assert_eq!(
            form.fields[0].validation,
            Some(ValidationState::Error("Required".to_string()))
        );
    }

    #[test]
    fn empty_rules_leaves_form_untouched() {
        let mut form = form_with(vec![text_field("f:name", "")]);
        form.fields[0].validation = Some(ValidationState::Error("pre-existing".to_string()));
        validate(&mut form, &FormRules::default());
        assert_eq!(
            form.fields[0].validation,
            Some(ValidationState::Error("pre-existing".to_string()))
        );
    }

    #[test]
    fn combo_box_field_validates_against_its_committed_value() {
        let mut form = form_with(vec![combo_field("f:fruit", "")]);
        let mut rules = FormRules::default();
        rules.fields.insert(
            "f:fruit".to_string(),
            vec![rule(Validator::Required, "Pick a fruit")],
        );

        validate(&mut form, &rules);
        assert_eq!(
            form.fields[0].validation,
            Some(ValidationState::Error("Pick a fruit".to_string()))
        );

        // Committing a value clears the error, even though `text` is what
        // the host mutates live while the user types.
        let FieldKind::ComboBox(combo) = &mut form.fields[0].kind else {
            unreachable!()
        };
        combo.committed = "Banana".to_string();
        validate(&mut form, &rules);
        assert_eq!(form.fields[0].validation, None);
    }

    #[test]
    fn form_rules_json_round_trip() {
        let mut rules = FormRules::default();
        rules.fields.insert(
            "f:email".to_string(),
            vec![rule(Validator::Required, "Required")],
        );
        let json = serde_json::to_string(&rules).unwrap();
        let back: FormRules = serde_json::from_str(&json).unwrap();
        assert_eq!(back, rules);
    }
}
