//! `NumberInput` primitive: a bounded numeric field with stepper
//! buttons — layout, hit-test, keyboard/wheel handling, validation, and
//! its event. Shared logic only; no backend painter lives here (see
//! `docs/PRIMITIVE_RULES.md`'s cross-backend commitment — a rasteriser
//! follows in a later, backend-scoped issue).
//!
//! Additive alongside [`crate::TextInput`], which is untouched — a
//! `NumberInput` isn't a constrained `TextInput`, it's a separate
//! primitive with its own numeric-only editing model: a committed
//! [`NumberInput::value`] plus an optional in-progress
//! [`NumberInput::draft`] string that overrides the display until the
//! host applies a [`NumberInputEvent::Committed`] or
//! [`NumberInputEvent::Cancelled`] event back onto the struct.
//!
//! # Focus and activation
//!
//! Like [`crate::Switch`], a `NumberInput` tracks its own
//! [`NumberInput::focused`] flag — there is one keyboard target, so no
//! wraparound cursor is needed (contrast [`crate::RadioGroup`]).
//! [`NumberInput::handle_key`] is gated on `focused` and not
//! [`NumberInput::disabled`]; the stepper buttons
//! ([`NumberInput::increment`]/[`NumberInput::decrement`], and the
//! [`NumberInput::click`] dispatcher built on top of them) are gated on
//! `disabled` alone, matching [`crate::Switch::click`]'s "a pointer
//! click doesn't require prior keyboard focus" contract.
//! [`NumberInput::handle_wheel`] mirrors
//! [`crate::ScrollView::handle_wheel`]'s shape and is likewise ungated on
//! focus — wheel events route to whatever the pointer is over, not
//! whatever holds keyboard focus.
//!
//! # Validation
//!
//! Typed digits accumulate in [`NumberInput::draft`] via
//! [`NumberInputEvent::DraftChanged`]; `Enter` parses the draft (falling
//! back to the current [`NumberInput::value`] on a parse failure),
//! clamps it to `[min, max]`, rounds it to [`NumberInput::precision`]
//! decimal places, and emits [`NumberInputEvent::Committed`]. `Escape`
//! discards the draft via [`NumberInputEvent::Cancelled`] without
//! touching `value`. Routing app-wide Tab into
//! [`NumberInput::focused`] is the host's job, same division of
//! responsibility as every other primitive here.

use crate::event::{Key, NamedKey, Point, Rect, ScrollDelta};
use crate::types::{Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

/// Width (in the backend's layout units) reserved for the
/// increment/decrement stepper column.
const STEPPER_WIDTH: f32 = 3.0;

/// Declarative description of a bounded numeric field with stepper
/// buttons.
///
/// # Examples
///
/// ```
/// use quadraui::{
///     Key, Modifiers, NamedKey, NumberInput, NumberInputEvent, NumberInputHit, Point, Rect,
///     WidgetId,
/// };
///
/// let mut input = NumberInput::new(WidgetId::new("settings:volume"), 0.0, 100.0);
/// input.focused = true;
///
/// let layout = input.layout(Rect::new(0.0, 0.0, 10.0, 2.0));
/// assert_eq!(layout.hit_test(Point::new(9.0, 0.0)), NumberInputHit::Increment);
///
/// let event = input.handle_key(&Key::Named(NamedKey::Up), &Modifiers::default());
/// assert_eq!(
///     event,
///     Some(NumberInputEvent::ValueChanged { id: input.id.clone(), value: 1.0 })
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NumberInput {
    pub id: WidgetId,
    /// Last committed value. Only changes via
    /// [`NumberInputEvent::ValueChanged`] (stepper/wheel) or
    /// [`NumberInputEvent::Committed`] (typed entry) — the host applies
    /// both the same way every other primitive's events are applied.
    #[serde(default)]
    pub value: f64,
    pub min: f64,
    pub max: f64,
    /// Amount [`NumberInput::increment`]/[`NumberInput::decrement`] and
    /// [`NumberInput::handle_wheel`] step by.
    #[serde(default = "NumberInput::default_step")]
    pub step: f64,
    /// Decimal places `value` is rounded to on commit/step.
    #[serde(default)]
    pub precision: u32,
    /// In-progress typed text overriding the displayed value until
    /// committed or cancelled. `None` means "showing `value` verbatim,
    /// no pending edit."
    #[serde(default)]
    pub draft: Option<String>,
    /// When `true`, every method below returns `None` — same
    /// "disabled means inert, not hidden" contract every other
    /// primitive's `disabled` flag follows.
    #[serde(default)]
    pub disabled: bool,
    /// Whether this field currently holds keyboard focus. Set by the
    /// host's focus-traversal logic; read by
    /// [`NumberInput::handle_key`] to gate typed/arrow-key handling.
    #[serde(default)]
    pub focused: bool,
}

impl NumberInput {
    fn default_step() -> f64 {
        1.0
    }

    /// A field clamped to `[min, max]`, seeded at the nearest in-range
    /// value to zero, enabled, unfocused, with no pending draft. Set
    /// `step`/`precision`/`disabled`/`focused` directly afterwards
    /// (every field is `pub`) rather than via a `with_*` chain — see
    /// `docs/PRIMITIVE_RULES.md`'s rule-8 history of why this crate
    /// stopped adding those.
    pub fn new(id: WidgetId, min: f64, max: f64) -> Self {
        Self {
            id,
            value: 0.0_f64.clamp(min, max),
            min,
            max,
            step: Self::default_step(),
            precision: 0,
            draft: None,
            disabled: false,
            focused: false,
        }
    }

    /// Round `value` to [`NumberInput::precision`] decimal places and
    /// clamp it to `[min, max]`.
    fn clamp_value(&self, value: f64) -> f64 {
        let factor = 10f64.powi(self.precision as i32);
        let rounded = (value * factor).round() / factor;
        rounded.clamp(self.min, self.max)
    }

    /// Resolve the field's hit-testable regions: a text area on the
    /// left, and stacked increment/decrement buttons in a fixed-width
    /// column on the right (split evenly top/bottom, mirroring
    /// [`crate::RadioGroup::layout`]'s multi-region approach rather
    /// than [`crate::Switch::layout`]'s single body).
    pub fn layout(&self, bounds: Rect) -> NumberInputLayout {
        let stepper_width = STEPPER_WIDTH.min(bounds.width);
        let text_width = (bounds.width - stepper_width).max(0.0);
        let text_area = Rect::new(bounds.x, bounds.y, text_width, bounds.height);
        let stepper_x = bounds.x + text_width;
        let half_height = bounds.height / 2.0;
        let increment = Rect::new(stepper_x, bounds.y, stepper_width, half_height);
        let decrement = Rect::new(
            stepper_x,
            bounds.y + half_height,
            stepper_width,
            bounds.height - half_height,
        );
        NumberInputLayout {
            bounds,
            text_area,
            increment,
            decrement,
        }
    }

    /// `Up`/`Down` step the value; `Enter` commits the current
    /// [`NumberInput::draft`] (or re-affirms `value` when there is no
    /// draft); `Escape` cancels a pending draft; digits/`-`/`.` and
    /// `Backspace` edit the draft. Gated on `focused` and not
    /// `disabled` — any other key is unhandled (`None`).
    pub fn handle_key(&self, key: &Key, _modifiers: &Modifiers) -> Option<NumberInputEvent> {
        if self.disabled || !self.focused {
            return None;
        }
        match key {
            Key::Named(NamedKey::Up) => self.increment(),
            Key::Named(NamedKey::Down) => self.decrement(),
            Key::Named(NamedKey::Enter) => Some(self.commit()),
            Key::Named(NamedKey::Escape) => {
                self.draft.is_some().then(|| NumberInputEvent::Cancelled {
                    id: self.id.clone(),
                })
            }
            Key::Named(NamedKey::Backspace) => {
                let mut draft = self.draft.clone().unwrap_or_default();
                draft.pop();
                Some(NumberInputEvent::DraftChanged {
                    id: self.id.clone(),
                    draft,
                })
            }
            Key::Char(c) if is_number_char(*c) => {
                let mut draft = self.draft.clone().unwrap_or_default();
                draft.push(*c);
                Some(NumberInputEvent::DraftChanged {
                    id: self.id.clone(),
                    draft,
                })
            }
            _ => None,
        }
    }

    /// Parse [`NumberInput::draft`] (falling back to the current
    /// `value` on an empty draft or a parse failure — invalid text
    /// reverts rather than commits garbage), clamp/round it, and
    /// produce the [`NumberInputEvent::Committed`] event.
    fn commit(&self) -> NumberInputEvent {
        let parsed = match &self.draft {
            Some(text) if !text.is_empty() => text.parse::<f64>().unwrap_or(self.value),
            _ => self.value,
        };
        NumberInputEvent::Committed {
            id: self.id.clone(),
            value: self.clamp_value(parsed),
        }
    }

    /// Step `value` up by [`NumberInput::step`], clamped/rounded. Gated
    /// on `disabled` alone — stepper buttons don't require prior
    /// keyboard focus, matching [`crate::Switch::click`].
    pub fn increment(&self) -> Option<NumberInputEvent> {
        if self.disabled {
            return None;
        }
        Some(NumberInputEvent::ValueChanged {
            id: self.id.clone(),
            value: self.clamp_value(self.value + self.step),
        })
    }

    /// Step `value` down by [`NumberInput::step`], clamped/rounded.
    pub fn decrement(&self) -> Option<NumberInputEvent> {
        if self.disabled {
            return None;
        }
        Some(NumberInputEvent::ValueChanged {
            id: self.id.clone(),
            value: self.clamp_value(self.value - self.step),
        })
    }

    /// Pointer activation dispatcher: the host calls this after
    /// resolving a `MouseDown`/`MouseUp` position through
    /// [`NumberInputLayout::hit_test`]. Hits on the text area or
    /// outside the control produce no event (focusing the text area is
    /// the host's job, same as elsewhere in this crate).
    pub fn click(&self, hit: NumberInputHit) -> Option<NumberInputEvent> {
        match hit {
            NumberInputHit::Increment => self.increment(),
            NumberInputHit::Decrement => self.decrement(),
            NumberInputHit::TextArea | NumberInputHit::Outside => None,
        }
    }

    /// Wheel-scroll stepping, mirroring
    /// [`crate::ScrollView::handle_wheel`]'s exact shape: the vertical
    /// delta notch count multiplied by `step`. Gated on `disabled`
    /// alone, not `focused` — wheel events route to whatever the
    /// pointer sits over.
    pub fn handle_wheel(&self, delta: ScrollDelta, step: f64) -> Option<NumberInputEvent> {
        if self.disabled || delta.y == 0.0 {
            return None;
        }
        Some(NumberInputEvent::ValueChanged {
            id: self.id.clone(),
            value: self.clamp_value(self.value + (delta.y as f64) * step),
        })
    }
}

fn is_number_char(c: char) -> bool {
    c.is_ascii_digit() || c == '-' || c == '.'
}

/// Fully-resolved number-input layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NumberInputLayout {
    pub bounds: Rect,
    pub text_area: Rect,
    pub increment: Rect,
    pub decrement: Rect,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberInputHit {
    TextArea,
    Increment,
    Decrement,
    Outside,
}

impl NumberInputLayout {
    pub fn hit_test(&self, p: Point) -> NumberInputHit {
        if self.increment.contains(p) {
            NumberInputHit::Increment
        } else if self.decrement.contains(p) {
            NumberInputHit::Decrement
        } else if self.text_area.contains(p) {
            NumberInputHit::TextArea
        } else {
            NumberInputHit::Outside
        }
    }
}

/// Emitted by [`NumberInput`]'s methods. `ValueChanged` carries the new
/// committed value directly (stepper/wheel never go through a draft);
/// `DraftChanged` carries the in-progress typed text; `Committed`
/// carries the parsed, clamped value a typed edit resolves to;
/// `Cancelled` carries only the id — the host clears `draft` and leaves
/// `value` untouched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NumberInputEvent {
    ValueChanged { id: WidgetId, value: f64 },
    DraftChanged { id: WidgetId, draft: String },
    Committed { id: WidgetId, value: f64 },
    Cancelled { id: WidgetId },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> NumberInput {
        NumberInput::new(WidgetId::new("settings:volume"), 0.0, 100.0)
    }

    #[test]
    fn new_clamps_default_value_into_range_and_starts_enabled_unfocused() {
        let n = input();
        assert_eq!(n.value, 0.0);
        assert!(!n.disabled);
        assert!(!n.focused);
        assert_eq!(n.draft, None);

        let negative_range = NumberInput::new(WidgetId::new("x"), 5.0, 10.0);
        assert_eq!(negative_range.value, 5.0);
    }

    #[test]
    fn layout_splits_text_area_and_stacked_stepper_buttons() {
        let n = input();
        let layout = n.layout(Rect::new(0.0, 0.0, 10.0, 2.0));
        assert_eq!(
            layout.hit_test(Point::new(5.0, 0.0)),
            NumberInputHit::TextArea
        );
        assert_eq!(
            layout.hit_test(Point::new(9.0, 0.0)),
            NumberInputHit::Increment
        );
        assert_eq!(
            layout.hit_test(Point::new(9.0, 1.0)),
            NumberInputHit::Decrement
        );
        assert_eq!(
            layout.hit_test(Point::new(-1.0, -1.0)),
            NumberInputHit::Outside
        );
    }

    #[test]
    fn handle_key_up_down_step_value_when_focused() {
        let mut n = input();
        n.focused = true;
        assert_eq!(
            n.handle_key(&Key::Named(NamedKey::Up), &Modifiers::default()),
            Some(NumberInputEvent::ValueChanged {
                id: n.id.clone(),
                value: 1.0
            })
        );
        n.value = 1.0;
        assert_eq!(
            n.handle_key(&Key::Named(NamedKey::Down), &Modifiers::default()),
            Some(NumberInputEvent::ValueChanged {
                id: n.id.clone(),
                value: 0.0
            })
        );
    }

    #[test]
    fn handle_key_clamps_increment_at_max_and_decrement_at_min() {
        let mut n = input();
        n.focused = true;
        n.value = 100.0;
        assert_eq!(
            n.handle_key(&Key::Named(NamedKey::Up), &Modifiers::default()),
            Some(NumberInputEvent::ValueChanged {
                id: n.id.clone(),
                value: 100.0
            })
        );
        n.value = 0.0;
        assert_eq!(
            n.handle_key(&Key::Named(NamedKey::Down), &Modifiers::default()),
            Some(NumberInputEvent::ValueChanged {
                id: n.id.clone(),
                value: 0.0
            })
        );
    }

    #[test]
    fn typed_digits_accumulate_in_draft_and_enter_commits_the_parsed_value() {
        let mut n = input();
        n.focused = true;

        let e1 = n.handle_key(&Key::Char('4'), &Modifiers::default());
        assert_eq!(
            e1,
            Some(NumberInputEvent::DraftChanged {
                id: n.id.clone(),
                draft: "4".into()
            })
        );
        n.draft = Some("4".into());

        let e2 = n.handle_key(&Key::Char('2'), &Modifiers::default());
        assert_eq!(
            e2,
            Some(NumberInputEvent::DraftChanged {
                id: n.id.clone(),
                draft: "42".into()
            })
        );
        n.draft = Some("42".into());

        let commit = n.handle_key(&Key::Named(NamedKey::Enter), &Modifiers::default());
        assert_eq!(
            commit,
            Some(NumberInputEvent::Committed {
                id: n.id.clone(),
                value: 42.0
            })
        );
    }

    #[test]
    fn invalid_draft_reverts_to_the_current_value_on_commit() {
        let mut n = input();
        n.focused = true;
        n.value = 7.0;
        n.draft = Some("--".into());
        assert_eq!(
            n.handle_key(&Key::Named(NamedKey::Enter), &Modifiers::default()),
            Some(NumberInputEvent::Committed {
                id: n.id.clone(),
                value: 7.0
            })
        );
    }

    #[test]
    fn commit_clamps_and_rounds_to_precision() {
        let mut n = NumberInput::new(WidgetId::new("settings:volume"), 0.0, 10.0);
        n.focused = true;
        n.precision = 1;
        n.draft = Some("999.456".into());
        assert_eq!(
            n.handle_key(&Key::Named(NamedKey::Enter), &Modifiers::default()),
            Some(NumberInputEvent::Committed {
                id: n.id.clone(),
                value: 10.0
            })
        );
    }

    #[test]
    fn escape_cancels_a_pending_draft_without_touching_value() {
        let mut n = input();
        n.focused = true;
        n.draft = Some("5".into());
        assert_eq!(
            n.handle_key(&Key::Named(NamedKey::Escape), &Modifiers::default()),
            Some(NumberInputEvent::Cancelled { id: n.id.clone() })
        );
    }

    #[test]
    fn escape_is_inert_when_there_is_no_pending_draft() {
        let mut n = input();
        n.focused = true;
        assert_eq!(
            n.handle_key(&Key::Named(NamedKey::Escape), &Modifiers::default()),
            None
        );
    }

    #[test]
    fn handle_key_is_inert_when_not_focused() {
        let n = input();
        assert_eq!(
            n.handle_key(&Key::Named(NamedKey::Up), &Modifiers::default()),
            None
        );
    }

    #[test]
    fn handle_key_is_inert_when_disabled() {
        let mut n = input();
        n.focused = true;
        n.disabled = true;
        assert_eq!(
            n.handle_key(&Key::Named(NamedKey::Up), &Modifiers::default()),
            None
        );
    }

    #[test]
    fn click_dispatches_to_increment_and_decrement_without_requiring_focus() {
        let n = input();
        assert_eq!(
            n.click(NumberInputHit::Increment),
            Some(NumberInputEvent::ValueChanged {
                id: n.id.clone(),
                value: 1.0
            })
        );
        assert_eq!(n.click(NumberInputHit::TextArea), None);
        assert_eq!(n.click(NumberInputHit::Outside), None);
    }

    #[test]
    fn click_is_inert_when_disabled() {
        let mut n = input();
        n.disabled = true;
        assert_eq!(n.click(NumberInputHit::Increment), None);
    }

    #[test]
    fn handle_wheel_steps_value_without_requiring_focus() {
        let n = input();
        assert_eq!(
            n.handle_wheel(ScrollDelta::new(0.0, 1.0), 2.0),
            Some(NumberInputEvent::ValueChanged {
                id: n.id.clone(),
                value: 2.0
            })
        );
    }

    #[test]
    fn handle_wheel_is_inert_when_disabled_or_zero_delta() {
        let mut n = input();
        n.disabled = true;
        assert_eq!(n.handle_wheel(ScrollDelta::new(0.0, 1.0), 2.0), None);

        let n = input();
        assert_eq!(n.handle_wheel(ScrollDelta::new(0.0, 0.0), 2.0), None);
    }

    #[test]
    fn serde_round_trip() {
        let mut n = input();
        n.value = 42.0;
        n.focused = true;
        n.draft = Some("4".into());
        let json = serde_json::to_string(&n).unwrap();
        let back: NumberInput = serde_json::from_str(&json).unwrap();
        assert_eq!(n, back);
    }

    #[test]
    fn serde_defaults_optional_fields_when_omitted() {
        let json = r#"{"id":"settings:volume","min":0.0,"max":100.0}"#;
        let n: NumberInput = serde_json::from_str(json).unwrap();
        assert_eq!(n.value, 0.0);
        assert_eq!(n.step, 1.0);
        assert_eq!(n.precision, 0);
        assert_eq!(n.draft, None);
        assert!(!n.disabled);
        assert!(!n.focused);
    }
}
