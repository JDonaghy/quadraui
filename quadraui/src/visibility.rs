//! Declarative visibility and enablement rules for [`Form`] fields.
//!
//! Today the only way to hide or disable a field in response to another
//! field's value is imperative: a host rebuilds `Form.fields` (or flips
//! `FormField::disabled`) by hand, every time any field it depends on
//! changes. This module adds the declarative half — a serializable
//! [`FieldVisibility`] (`visible_when` / `enabled_when`, each a
//! [`crate::expr::Expr`]) collected per field in a [`FormVisibility`] —
//! so declaring the dependency once is enough; [`apply_visibility`]
//! re-evaluates it against the form's current field values on every
//! [`crate::compose::FormController::set_form`] call, with no host
//! round-trip per keystroke.
//!
//! Like [`crate::expr`] and [`crate::validation`], this is data, not
//! closures, so every language binding (Lua, Python, a future C-ABI) ships
//! the same plain-data rule set and gets identical show/hide and
//! enable/disable behaviour out of quadraui.
//!
//! # Additive, not a replacement
//!
//! The imperative path — a host rebuilding `Form.fields`, or setting
//! `FormField::disabled` directly — keeps working unmodified.
//! [`apply_visibility`] only touches fields that have at least one
//! condition declared for them in the [`FormVisibility`] it's given; any
//! field absent from `FormVisibility::fields` is left exactly as the
//! caller set it. A host that never calls [`apply_visibility`] (or never
//! populates `FormVisibility`) sees no behaviour change at all.
//!
//! # Per field, and per section
//!
//! `primitives::form` has no separate "section" type — a `FieldKind::Label`
//! row is the existing stand-in for a section header. Since
//! `FormVisibility` keys rules by `WidgetId::as_str()` like
//! [`crate::validation::FormRules`] does, a `Label` field's own id takes a
//! `visible_when` exactly like any other field's: hide the heading and
//! hide its member fields (each with their own `visible_when`, or sharing
//! the same condition) to make a whole section disappear.
//!
//! # Evaluation
//!
//! - `visible_when` gates membership in [`Form::fields`] itself: a field
//!   whose `visible_when` evaluates to `false` is removed from the list
//!   entirely (mirroring the imperative baseline of rebuilding the field
//!   list), not just skipped during painting — so it does not occupy
//!   layout space, is not a Tab stop, and the `WidgetId` space it would
//!   have claimed in a click hit-test is free. A field with no
//!   `visible_when` declared is always a member.
//! - `enabled_when` writes `FormField::disabled`: `true` when the
//!   condition evaluates to `false`, `false` when it evaluates to `true`.
//!   Declaring `enabled_when` therefore takes over `disabled` for that
//!   field unconditionally on every [`apply_visibility`] call, the same
//!   posture [`crate::validation::validate`] takes for `validation`. A
//!   field with no `enabled_when` declared keeps whatever `disabled` the
//!   caller (or the imperative path) set. `enabled_when` is visual and
//!   Tab-order only: [`crate::compose::FormController`]'s click routing
//!   still emits `FormEvent::ButtonClicked` / `ToggleChanged` for a
//!   `disabled` field exactly as it does for an imperatively disabled one
//!   (the same posture a disabled toolbar button takes), so a host that
//!   needs pointer input actually blocked must re-check the condition
//!   itself before honoring the event.
//! - Both conditions are evaluated against the same cross-field
//!   [`crate::validation::form_context`] `validate` uses, built from the
//!   form's values *before* any field is removed by `visible_when` — so a
//!   condition may reference a field that this same call is about to hide.
//! - If `Form::focused_field` names a field that `visible_when` just
//!   removed, it is cleared to `None` rather than left dangling; the next
//!   Tab press (or a fresh click) picks a new focus the same way it would
//!   after the last field is removed by any other means. No
//!   `FormEvent::FocusChanged` is emitted for this drop — a host that
//!   mirrors `focused_field` in its own state (rather than reading it back
//!   off `Form`) keeps a dangling id until the next event moves focus.
//! - [`crate::compose::FormController`]'s own `scroll_offset` is not
//!   re-clamped when `visible_when` shrinks the field list; it is only
//!   re-clamped on the next scroll event. A form scrolled near the bottom
//!   that then hides most of its fields can render blank until then.
//! - `FormVisibility` keys only a top-level `FormField::id`, the same
//!   limitation [`crate::validation::FormRules`] has: a condition cannot
//!   target a `ToggleGroupItem`, `ButtonRowItem` or `ToolbarButton` nested
//!   inside a field.
//!
//! # Wire shape
//!
//! `FieldVisibility` and `FormVisibility` derive `serde::Serialize` /
//! `Deserialize` with no extra configuration beyond omitting an absent
//! condition, so a binding emits exactly this shape. "Show `shipping_name`
//! only when `needs_shipping` is `true`; disable it whenever `locked` is
//! `true`" is:
//!
//! ```
//! use quadraui::expr::{Expr, Value};
//! use quadraui::visibility::{FieldVisibility, FormVisibility};
//!
//! let mut rules = FormVisibility::default();
//! rules.fields.insert(
//!     "shipping_name".to_string(),
//!     FieldVisibility {
//!         visible_when: Some(Expr::Eq(
//!             Box::new(Expr::Field("needs_shipping".into())),
//!             Box::new(Expr::Lit(Value::Bool(true))),
//!         )),
//!         enabled_when: Some(Expr::Not(Box::new(Expr::Field("locked".into())))),
//!     },
//! );
//!
//! let json = serde_json::to_string(&rules).unwrap();
//! assert!(json.contains("\"visible_when\""));
//! assert!(json.contains("\"enabled_when\""));
//! ```
//!
//! which any binding can build directly as data — `{"fields": {"shipping_name":
//! {"visible_when": {"eq": [...]}, "enabled_when": {"not": [...]}}}}` — with
//! no host callback on either side.

use crate::primitives::form::Form;
use crate::validation::form_context;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::expr::Expr;

/// Conditional visibility/enablement for one field. Either or both of
/// `visible_when` / `enabled_when` may be set; an absent condition leaves
/// the corresponding aspect (membership in `Form::fields`, or
/// `FormField::disabled`) untouched by [`apply_visibility`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FieldVisibility {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_when: Option<Expr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_when: Option<Expr>,
}

/// Declarative visibility/enablement rules for a whole [`Form`], keyed by
/// field id (`WidgetId::as_str()`) — the same keying
/// [`crate::validation::FormRules`] uses. Serializable so a Lua/Python/
/// C-ABI binding can ship the same rule set quadraui evaluates — see the
/// module doc and `crate::expr`'s for why rules are data, not callbacks.
///
/// A field absent from `fields` has no declarative visibility rules;
/// [`apply_visibility`] leaves it exactly as the caller set it (the
/// imperative escape-hatch path — see the module doc's "Additive"
/// section).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FormVisibility {
    pub fields: BTreeMap<String, FieldVisibility>,
}

/// Evaluate `rules` against `form`'s current field values, then:
///
/// - drop every field whose `visible_when` evaluates to `false` from
///   `form.fields`;
/// - set `form.fields[..].disabled` for every field with an `enabled_when`
///   declared;
/// - clear `form.focused_field` to `None` if the field it named was just
///   dropped.
///
/// See the module doc's "Evaluation" section for the exact semantics and
/// evaluation order. A `rules` with an empty `fields` map is a no-op.
pub fn apply_visibility(form: &mut Form, rules: &FormVisibility) {
    if rules.fields.is_empty() {
        return;
    }
    let ctx = form_context(form);
    for field in &mut form.fields {
        if let Some(enabled_when) = rules
            .fields
            .get(field.id.as_str())
            .and_then(|fv| fv.enabled_when.as_ref())
        {
            field.disabled = !enabled_when.eval_bool(&ctx);
        }
    }
    form.fields.retain(|field| {
        rules
            .fields
            .get(field.id.as_str())
            .and_then(|fv| fv.visible_when.as_ref())
            .map(|cond| cond.eval_bool(&ctx))
            .unwrap_or(true)
    });
    if let Some(focused) = &form.focused_field {
        if !form.fields.iter().any(|f| &f.id == focused) {
            form.focused_field = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::Value;
    use crate::primitives::form::FieldKind;
    use crate::types::{StyledText, WidgetId};

    fn toggle_field(id: &str, value: bool) -> crate::primitives::form::FormField {
        crate::primitives::form::FormField {
            id: WidgetId::new(id),
            label: StyledText::plain(id),
            kind: FieldKind::Toggle { value },
            hint: StyledText::plain(""),
            disabled: false,
            validation: None,
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
        }
    }

    fn form_with(fields: Vec<crate::primitives::form::FormField>) -> Form {
        Form {
            id: WidgetId::new("form:test"),
            fields,
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
        }
    }

    #[test]
    fn empty_rules_are_a_no_op() {
        let mut form = form_with(vec![toggle_field("a", true)]);
        apply_visibility(&mut form, &FormVisibility::default());
        assert_eq!(form.fields.len(), 1);
    }

    #[test]
    fn field_with_no_declared_rule_is_untouched() {
        let mut form = form_with(vec![toggle_field("a", true), toggle_field("b", false)]);
        let mut rules = FormVisibility::default();
        rules.fields.insert(
            "a".to_string(),
            FieldVisibility {
                visible_when: Some(Expr::Lit(Value::Bool(false))),
                enabled_when: None,
            },
        );
        apply_visibility(&mut form, &rules);
        assert_eq!(form.fields.len(), 1);
        assert_eq!(form.fields[0].id, WidgetId::new("b"));
        assert!(!form.fields[0].disabled);
    }

    #[test]
    fn visible_when_false_removes_the_field() {
        let mut form = form_with(vec![
            toggle_field("needs_shipping", false),
            toggle_field("shipping_name", true),
        ]);
        let mut rules = FormVisibility::default();
        rules.fields.insert(
            "shipping_name".to_string(),
            FieldVisibility {
                visible_when: Some(Expr::Eq(
                    Box::new(Expr::Field("needs_shipping".into())),
                    Box::new(Expr::Lit(Value::Bool(true))),
                )),
                enabled_when: None,
            },
        );
        apply_visibility(&mut form, &rules);
        assert_eq!(form.fields.len(), 1);
        assert_eq!(form.fields[0].id, WidgetId::new("needs_shipping"));
    }

    #[test]
    fn visible_when_true_keeps_the_field() {
        let mut form = form_with(vec![
            toggle_field("needs_shipping", true),
            toggle_field("shipping_name", true),
        ]);
        let mut rules = FormVisibility::default();
        rules.fields.insert(
            "shipping_name".to_string(),
            FieldVisibility {
                visible_when: Some(Expr::Eq(
                    Box::new(Expr::Field("needs_shipping".into())),
                    Box::new(Expr::Lit(Value::Bool(true))),
                )),
                enabled_when: None,
            },
        );
        apply_visibility(&mut form, &rules);
        assert_eq!(form.fields.len(), 2);
    }

    #[test]
    fn enabled_when_false_disables_without_removing() {
        let mut form = form_with(vec![
            toggle_field("locked", true),
            toggle_field("shipping_name", true),
        ]);
        let mut rules = FormVisibility::default();
        rules.fields.insert(
            "shipping_name".to_string(),
            FieldVisibility {
                visible_when: None,
                enabled_when: Some(Expr::Not(Box::new(Expr::Field("locked".into())))),
            },
        );
        apply_visibility(&mut form, &rules);
        assert_eq!(form.fields.len(), 2);
        assert!(form.fields[1].disabled);
    }

    #[test]
    fn enabled_when_true_re_enables_a_previously_disabled_field() {
        let mut field = toggle_field("shipping_name", true);
        field.disabled = true;
        let mut form = form_with(vec![toggle_field("locked", false), field]);
        let mut rules = FormVisibility::default();
        rules.fields.insert(
            "shipping_name".to_string(),
            FieldVisibility {
                visible_when: None,
                enabled_when: Some(Expr::Not(Box::new(Expr::Field("locked".into())))),
            },
        );
        apply_visibility(&mut form, &rules);
        assert!(!form.fields[1].disabled);
    }

    #[test]
    fn focused_field_is_cleared_when_hidden() {
        let mut form = form_with(vec![
            toggle_field("needs_shipping", false),
            toggle_field("shipping_name", true),
        ]);
        form.focused_field = Some(WidgetId::new("shipping_name"));
        let mut rules = FormVisibility::default();
        rules.fields.insert(
            "shipping_name".to_string(),
            FieldVisibility {
                visible_when: Some(Expr::Field("needs_shipping".into())),
                enabled_when: None,
            },
        );
        apply_visibility(&mut form, &rules);
        assert_eq!(form.focused_field, None);
    }

    #[test]
    fn focused_field_on_a_visible_field_is_left_alone() {
        let mut form = form_with(vec![
            toggle_field("needs_shipping", true),
            toggle_field("shipping_name", true),
        ]);
        form.focused_field = Some(WidgetId::new("shipping_name"));
        let mut rules = FormVisibility::default();
        rules.fields.insert(
            "shipping_name".to_string(),
            FieldVisibility {
                visible_when: Some(Expr::Field("needs_shipping".into())),
                enabled_when: None,
            },
        );
        apply_visibility(&mut form, &rules);
        assert_eq!(form.focused_field, Some(WidgetId::new("shipping_name")));
    }

    #[test]
    fn hidden_field_still_gets_its_disabled_write_before_being_removed() {
        // Both conditions declared on the same field, `visible_when`
        // false: the `enabled_when` write happens before the `retain`
        // drops the field, which is harmless since the field is gone by
        // the time `apply_visibility` returns either way.
        let mut form = form_with(vec![
            toggle_field("needs_shipping", false),
            toggle_field("locked", true),
            toggle_field("shipping_name", true),
        ]);
        let mut rules = FormVisibility::default();
        rules.fields.insert(
            "shipping_name".to_string(),
            FieldVisibility {
                visible_when: Some(Expr::Field("needs_shipping".into())),
                enabled_when: Some(Expr::Not(Box::new(Expr::Field("locked".into())))),
            },
        );
        apply_visibility(&mut form, &rules);
        assert!(
            !form
                .fields
                .iter()
                .any(|f| f.id == WidgetId::new("shipping_name")),
            "visible_when false should remove the field regardless of enabled_when"
        );
    }

    #[test]
    fn wire_shape_round_trips_through_json() {
        let mut rules = FormVisibility::default();
        rules.fields.insert(
            "shipping_name".to_string(),
            FieldVisibility {
                visible_when: Some(Expr::Lit(Value::Bool(true))),
                enabled_when: None,
            },
        );
        let json = serde_json::to_string(&rules).unwrap();
        assert!(!json.contains("enabled_when"));
        let round_tripped: FormVisibility = serde_json::from_str(&json).unwrap();
        assert_eq!(round_tripped, rules);
    }
}
