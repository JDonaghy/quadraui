//! Serializable expression AST for form rules (validation, conditional
//! visibility, and anything else that needs a small boolean test against a
//! form's current field values).
//!
//! # Binding-neutral by construction
//!
//! quadraui's plugin/extension UI is driven from multiple language
//! bindings (Lua, Python, and a planned C-ABI). An expression model built
//! out of host callbacks (closures, trait objects) cannot cross a C-ABI and
//! does not marshal to Python — so the condition logic has to be *data*.
//! [`Expr`] is that data: every binding builds (or deserializes) the same
//! [`Expr`] tree, and quadraui is the only thing that ever evaluates it,
//! once, against the form's current field values.
//!
//! `Expr` and [`Value`] derive `serde::Serialize`/`Deserialize` with no
//! extra configuration, so the wire form *is* whatever `serde_json` (or any
//! other serde format) produces for them — see the module-level example
//! below for the exact JSON shape binding authors should emit.
//!
//! # Scope
//!
//! Deliberately small, non-Turing-complete, and safe to re-evaluate on
//! every keystroke:
//!
//! - [`Expr::Field`] — a reference to another field's current value by
//!   path.
//! - [`Expr::Lit`] — a literal [`Value`] (string, number, bool, or null).
//! - Comparisons: [`Expr::Eq`], [`Expr::Ne`], [`Expr::Lt`], [`Expr::Le`],
//!   [`Expr::Gt`], [`Expr::Ge`].
//! - [`Expr::In`] — membership in a literal list.
//! - [`Expr::Present`] / [`Expr::Empty`] — field-has-a-value tests.
//! - [`Expr::And`] / [`Expr::Or`] / [`Expr::Not`] — boolean combinators.
//! - [`Expr::Matches`] — regex match against a string value.
//!
//! There are deliberately no function calls, loops, or side effects: a rule
//! is a pure, total function from field values to a [`Value`], and
//! [`Expr::eval`] never panics and never errors — a reference to a field
//! that doesn't exist (or a comparison between mismatched types) evaluates
//! to [`Value::Null`] / `false` rather than failing, so a rule authored
//! against a field that hasn't been added to the form yet degrades safely
//! instead of crashing the host application.
//!
//! # Wire shape
//!
//! `Expr` and `Value` serialize with serde's default (externally tagged)
//! enum representation, `snake_case`d. A rule that says "show this field
//! when `country` is `\"US\"` and `zip` is non-empty" is:
//!
//! ```
//! use quadraui::expr::{Expr, Value};
//!
//! let rule = Expr::And(vec![
//!     Expr::Eq(
//!         Box::new(Expr::Field("country".into())),
//!         Box::new(Expr::Lit(Value::String("US".into()))),
//!     ),
//!     Expr::Present(Box::new(Expr::Field("zip".into()))),
//! ]);
//!
//! let json = serde_json::to_string(&rule).unwrap();
//! assert_eq!(
//!     json,
//!     r#"{"and":[{"eq":[{"field":"country"},{"lit":{"string":"US"}}]},{"present":{"field":"zip"}}]}"#
//! );
//!
//! // Round-trips losslessly — this is what a Lua/Python/C-ABI binding
//! // author hand-builds (or serializes from their own host-language
//! // structure) to ship a rule into quadraui.
//! let back: Expr = serde_json::from_str(&json).unwrap();
//! assert_eq!(back, rule);
//! ```
//!
//! Evaluate a parsed rule against a form's current values with
//! [`Expr::eval`] or the boolean convenience [`Expr::eval_bool`]:
//!
//! ```
//! use quadraui::expr::{Expr, Value};
//! use std::collections::BTreeMap;
//!
//! let rule = Expr::Eq(
//!     Box::new(Expr::Field("country".into())),
//!     Box::new(Expr::Lit(Value::String("US".into()))),
//! );
//!
//! let mut fields = BTreeMap::new();
//! fields.insert("country".to_string(), Value::String("US".into()));
//!
//! assert!(rule.eval_bool(&fields));
//! ```

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A field value as the expression evaluator understands it.
///
/// Mirrors the literal kinds the scope section of the module doc allows:
/// string, number (always `f64` — there is no separate integer kind,
/// matching every other primitive field value in this crate, e.g.
/// `FieldKind::Slider`'s `f32`), bool, and null (an absent/unset value).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
}

impl Value {
    /// JSONLogic/CEL-style truthiness: `null`, `false`, `0`, and `""` are
    /// falsy; everything else is truthy. Used by [`Expr::eval_bool`] to
    /// coerce a non-boolean rule root (e.g. a bare `Expr::Field`) to a
    /// bool instead of panicking or erroring.
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => *n != 0.0,
            Value::String(s) => !s.is_empty(),
        }
    }

    /// `true` for anything except `Null` — the field has *a* value, not
    /// necessarily a non-empty one. Pairs with [`Value::is_empty`], which
    /// additionally treats the empty string as absent.
    fn is_present(&self) -> bool {
        !matches!(self, Value::Null)
    }

    /// `true` for `Null` and the empty string — the two shapes a
    /// `TextInput`/optional field takes before the user has filled it in.
    fn is_empty(&self) -> bool {
        matches!(self, Value::Null) || matches!(self, Value::String(s) if s.is_empty())
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

impl From<f64> for Value {
    fn from(n: f64) -> Self {
        Value::Number(n)
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::String(s)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::String(s.to_string())
    }
}

/// A form's current field values, keyed by field path (e.g. a
/// `FormField::id`'s string form, or a dotted path into a nested group).
///
/// Plain type alias over `BTreeMap` rather than a newtype: callers build
/// this fresh from whatever they already hold (a `Form`'s fields, a Lua
/// table, a Python dict) every time they re-evaluate a rule, so there is no
/// state for a wrapper type to usefully own.
pub type Context = BTreeMap<String, Value>;

/// Serializable boolean/value expression — see the module docs for scope,
/// wire shape, and evaluation semantics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expr {
    /// A literal value.
    Lit(Value),
    /// A reference to another field's current value, by path. Evaluates
    /// to [`Value::Null`] if `path` is not present in the [`Context`].
    Field(String),
    Eq(Box<Expr>, Box<Expr>),
    Ne(Box<Expr>, Box<Expr>),
    Lt(Box<Expr>, Box<Expr>),
    Le(Box<Expr>, Box<Expr>),
    Gt(Box<Expr>, Box<Expr>),
    Ge(Box<Expr>, Box<Expr>),
    /// `true` if evaluating `needle` equals evaluating any element of
    /// `haystack`.
    In {
        needle: Box<Expr>,
        haystack: Vec<Expr>,
    },
    /// `true` if evaluating the inner expression is not `Value::Null`.
    Present(Box<Expr>),
    /// `true` if evaluating the inner expression is `Value::Null` or the
    /// empty string.
    Empty(Box<Expr>),
    /// `true` if every operand evaluates truthy. `And(vec![])` is `true`
    /// (the identity for `and`), matching `all()`'s empty-iterator result.
    And(Vec<Expr>),
    /// `true` if any operand evaluates truthy. `Or(vec![])` is `false`
    /// (the identity for `or`), matching `any()`'s empty-iterator result.
    Or(Vec<Expr>),
    Not(Box<Expr>),
    /// `true` if evaluating `value` is a string matched by the regex
    /// `pattern`. `pattern` is a plain string, not a nested `Expr` — a
    /// regex is parsed once, not re-derived per evaluation, and binding
    /// authors always know the pattern at rule-authoring time. An
    /// invalid `pattern` (one `regex::Regex::new` rejects) evaluates to
    /// `false` rather than panicking, keeping `eval` total.
    Matches {
        value: Box<Expr>,
        pattern: String,
    },
}

impl Expr {
    /// Evaluate against `ctx`, producing a [`Value`]. Total: never panics,
    /// never errors. A dangling [`Expr::Field`] reference, a
    /// mismatched-type comparison, or an invalid [`Expr::Matches`] pattern
    /// all degrade to a safe default (`Value::Null` / `false`) rather than
    /// failing — see the module doc's Scope section.
    pub fn eval(&self, ctx: &Context) -> Value {
        match self {
            Expr::Lit(v) => v.clone(),
            Expr::Field(path) => ctx.get(path).cloned().unwrap_or(Value::Null),
            Expr::Eq(a, b) => Value::Bool(a.eval(ctx) == b.eval(ctx)),
            Expr::Ne(a, b) => Value::Bool(a.eval(ctx) != b.eval(ctx)),
            Expr::Lt(a, b) => {
                Value::Bool(compare(&a.eval(ctx), &b.eval(ctx)) == Some(std::cmp::Ordering::Less))
            }
            Expr::Le(a, b) => Value::Bool(matches!(
                compare(&a.eval(ctx), &b.eval(ctx)),
                Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
            )),
            Expr::Gt(a, b) => Value::Bool(
                compare(&a.eval(ctx), &b.eval(ctx)) == Some(std::cmp::Ordering::Greater),
            ),
            Expr::Ge(a, b) => Value::Bool(matches!(
                compare(&a.eval(ctx), &b.eval(ctx)),
                Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
            )),
            Expr::In { needle, haystack } => {
                let needle = needle.eval(ctx);
                Value::Bool(haystack.iter().any(|e| e.eval(ctx) == needle))
            }
            Expr::Present(e) => Value::Bool(e.eval(ctx).is_present()),
            Expr::Empty(e) => Value::Bool(e.eval(ctx).is_empty()),
            Expr::And(exprs) => Value::Bool(exprs.iter().all(|e| e.eval(ctx).is_truthy())),
            Expr::Or(exprs) => Value::Bool(exprs.iter().any(|e| e.eval(ctx).is_truthy())),
            Expr::Not(e) => Value::Bool(!e.eval(ctx).is_truthy()),
            Expr::Matches { value, pattern } => {
                let v = value.eval(ctx);
                let text = match &v {
                    Value::String(s) => s.as_str(),
                    _ => return Value::Bool(false),
                };
                match regex::Regex::new(pattern) {
                    Ok(re) => Value::Bool(re.is_match(text)),
                    Err(_) => Value::Bool(false),
                }
            }
        }
    }

    /// Evaluate against `ctx` and coerce the result to `bool` via
    /// [`Value::is_truthy`]. The convenience entry point for validation /
    /// conditional-visibility callers, which only ever need a yes/no
    /// answer, not the intermediate `Value`.
    pub fn eval_bool(&self, ctx: &Context) -> bool {
        self.eval(ctx).is_truthy()
    }
}

/// Ordering between two values of the *same* kind. `None` for a
/// cross-kind comparison (e.g. a string against a number) or any
/// comparison involving `Null`/`Bool` — those have no natural order, so
/// `<`/`<=`/`>`/`>=` against them is always `false` rather than an
/// arbitrary pick, per the module doc's "safe default" rule.
fn compare(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a.partial_cmp(b),
        (Value::String(a), Value::String(b)) => a.partial_cmp(b),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(pairs: &[(&str, Value)]) -> Context {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn field_reference_reads_current_value() {
        let c = ctx(&[("name", Value::String("Ada".into()))]);
        assert_eq!(
            Expr::Field("name".into()).eval(&c),
            Value::String("Ada".into())
        );
    }

    #[test]
    fn missing_field_evaluates_to_null_not_panic() {
        let c = ctx(&[]);
        assert_eq!(Expr::Field("missing".into()).eval(&c), Value::Null);
    }

    #[test]
    fn eq_and_ne() {
        let c = ctx(&[("a", Value::Number(1.0))]);
        let lit = |n: f64| Box::new(Expr::Lit(Value::Number(n)));
        assert!(Expr::Eq(Box::new(Expr::Field("a".into())), lit(1.0)).eval_bool(&c));
        assert!(!Expr::Eq(Box::new(Expr::Field("a".into())), lit(2.0)).eval_bool(&c));
        assert!(Expr::Ne(Box::new(Expr::Field("a".into())), lit(2.0)).eval_bool(&c));
    }

    #[test]
    fn ordering_comparisons_numbers_and_strings() {
        let c: Context = Context::new();
        let n = |n: f64| Box::new(Expr::Lit(Value::Number(n)));
        assert!(Expr::Lt(n(1.0), n(2.0)).eval_bool(&c));
        assert!(Expr::Le(n(2.0), n(2.0)).eval_bool(&c));
        assert!(Expr::Gt(n(3.0), n(2.0)).eval_bool(&c));
        assert!(Expr::Ge(n(2.0), n(2.0)).eval_bool(&c));
        assert!(!Expr::Lt(n(2.0), n(1.0)).eval_bool(&c));

        let s = |s: &str| Box::new(Expr::Lit(Value::String(s.into())));
        assert!(Expr::Lt(s("a"), s("b")).eval_bool(&c));
    }

    #[test]
    fn ordering_across_mismatched_types_is_false_not_panic() {
        let c = Context::new();
        let lhs = Box::new(Expr::Lit(Value::String("1".into())));
        let rhs = Box::new(Expr::Lit(Value::Number(1.0)));
        assert!(!Expr::Lt(lhs.clone(), rhs.clone()).eval_bool(&c));
        assert!(!Expr::Gt(lhs, rhs).eval_bool(&c));
    }

    #[test]
    fn membership_in() {
        let c = ctx(&[("role", Value::String("admin".into()))]);
        let rule = Expr::In {
            needle: Box::new(Expr::Field("role".into())),
            haystack: vec![
                Expr::Lit(Value::String("admin".into())),
                Expr::Lit(Value::String("owner".into())),
            ],
        };
        assert!(rule.eval_bool(&c));

        let c2 = ctx(&[("role", Value::String("guest".into()))]);
        assert!(!rule.eval_bool(&c2));
    }

    #[test]
    fn present_and_empty() {
        let c = ctx(&[
            ("zip", Value::String(String::new())),
            ("city", Value::String("NYC".into())),
        ]);
        // "zip" is in the context (present — it has *a* value, `Null` or
        // not) but that value is the empty string (empty).
        assert!(Expr::Present(Box::new(Expr::Field("zip".into()))).eval_bool(&c));
        assert!(Expr::Empty(Box::new(Expr::Field("zip".into()))).eval_bool(&c));
        assert!(Expr::Present(Box::new(Expr::Field("city".into()))).eval_bool(&c));
        assert!(!Expr::Empty(Box::new(Expr::Field("city".into()))).eval_bool(&c));
        // A field that isn't in the context at all evaluates to `Null`,
        // which is both "not present" and "empty".
        assert!(Expr::Empty(Box::new(Expr::Field("ghost".into()))).eval_bool(&c));
        assert!(!Expr::Present(Box::new(Expr::Field("ghost".into()))).eval_bool(&c));
    }

    #[test]
    fn boolean_combinators() {
        let t = Expr::Lit(Value::Bool(true));
        let f = Expr::Lit(Value::Bool(false));
        let c = Context::new();

        assert!(Expr::And(vec![t.clone(), t.clone()]).eval_bool(&c));
        assert!(!Expr::And(vec![t.clone(), f.clone()]).eval_bool(&c));
        assert!(
            Expr::And(vec![]).eval_bool(&c),
            "empty And is vacuously true"
        );

        assert!(Expr::Or(vec![f.clone(), t.clone()]).eval_bool(&c));
        assert!(!Expr::Or(vec![f.clone(), f.clone()]).eval_bool(&c));
        assert!(
            !Expr::Or(vec![]).eval_bool(&c),
            "empty Or is vacuously false"
        );

        assert!(Expr::Not(Box::new(f)).eval_bool(&c));
        assert!(!Expr::Not(Box::new(t)).eval_bool(&c));
    }

    #[test]
    fn matches_regex() {
        let c = ctx(&[("email", Value::String("a@example.com".into()))]);
        let rule = Expr::Matches {
            value: Box::new(Expr::Field("email".into())),
            pattern: r"^[^@]+@[^@]+\.[^@]+$".to_string(),
        };
        assert!(rule.eval_bool(&c));

        let c2 = ctx(&[("email", Value::String("nope".into()))]);
        assert!(!rule.eval_bool(&c2));
    }

    #[test]
    fn matches_against_non_string_is_false() {
        let c = ctx(&[("age", Value::Number(30.0))]);
        let rule = Expr::Matches {
            value: Box::new(Expr::Field("age".into())),
            pattern: r"\d+".to_string(),
        };
        assert!(!rule.eval_bool(&c));
    }

    #[test]
    fn invalid_pattern_is_false_not_panic() {
        let c = ctx(&[("name", Value::String("Ada".into()))]);
        let rule = Expr::Matches {
            value: Box::new(Expr::Field("name".into())),
            pattern: "(unclosed".to_string(),
        };
        assert!(!rule.eval_bool(&c));
    }

    #[test]
    fn truthiness_of_non_bool_values() {
        assert!(!Value::Null.is_truthy());
        assert!(!Value::Number(0.0).is_truthy());
        assert!(Value::Number(1.0).is_truthy());
        assert!(!Value::String(String::new()).is_truthy());
        assert!(Value::String("x".into()).is_truthy());
    }

    #[test]
    fn eval_bool_coerces_non_bool_root() {
        let c = ctx(&[("name", Value::String("Ada".into()))]);
        assert!(Expr::Field("name".into()).eval_bool(&c));
        assert!(!Expr::Field("missing".into()).eval_bool(&c));
    }

    #[test]
    fn json_round_trip_matches_documented_wire_shape() {
        let rule = Expr::And(vec![
            Expr::Eq(
                Box::new(Expr::Field("country".into())),
                Box::new(Expr::Lit(Value::String("US".into()))),
            ),
            Expr::Present(Box::new(Expr::Field("zip".into()))),
        ]);
        let json = serde_json::to_string(&rule).unwrap();
        assert_eq!(
            json,
            r#"{"and":[{"eq":[{"field":"country"},{"lit":{"string":"US"}}]},{"present":{"field":"zip"}}]}"#
        );
        let back: Expr = serde_json::from_str(&json).unwrap();
        assert_eq!(back, rule);
    }

    #[test]
    fn value_from_impls() {
        assert_eq!(Value::from(true), Value::Bool(true));
        assert_eq!(Value::from(1.5f64), Value::Number(1.5));
        assert_eq!(Value::from("x"), Value::String("x".into()));
        assert_eq!(Value::from(String::from("y")), Value::String("y".into()));
    }
}
