//! `Button` primitive: a standalone, clickable action control — primary,
//! secondary, and icon variants, with keyboard focus/activation, plus
//! its disabled state and event. Shared logic only; no backend painter
//! lives here (see `docs/PRIMITIVE_RULES.md`'s cross-backend commitment
//! — a rasteriser follows in a later, backend-scoped issue).
//!
//! Today `Form`'s `Button`/`ButtonRow` field kinds cover the same shape
//! embedded inside a form. `Button` is additive: a standalone control
//! for a host that wants a button outside a form (a toolbar action, a
//! dialog's own footer, a palette entry) without pulling in the whole
//! `Form`/`FormController` machinery. `Form`'s own field kinds are
//! untouched.
//!
//! # Focus and activation
//!
//! A `Button` tracks its own [`Button::focused`] flag, the same
//! self-contained pattern [`crate::PipelineView`] and [`crate::Toolbar`]
//! already use for single-widget keyboard state — there is exactly one
//! focusable target per `Button`. [`Button::handle_key`] activates on
//! `Enter` or `Space` while focused and not [`Button::disabled`];
//! [`Button::click`] does the same for a pointer click, gated on
//! `disabled` alone. Both return the same [`ButtonEvent`] so a host
//! wires one event type regardless of input modality.
//!
//! Routing app-wide Tab into `Button::focused` is the host's job — same
//! division of responsibility as every other primitive here: this type
//! owns what happens once it has focus, not how focus arrives (see
//! [`crate::focus::FocusManager`] for the app-wide owner of "what has
//! focus").

use crate::event::{Key, NamedKey, Point, Rect};
use crate::types::{Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

/// Visual emphasis variant. Purely descriptive — `Button` carries
/// no backend painting, so a rasteriser reads this field to choose its
/// own chrome (filled vs. outlined vs. icon-only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ButtonVariant {
    /// The call-to-action for its surface (e.g. a dialog's "OK").
    #[default]
    Primary,
    /// A lower-emphasis action alongside a primary one (e.g. "Cancel").
    Secondary,
    /// Icon-only, no label chrome (e.g. a toolbar glyph button).
    Icon,
}

/// Declarative description of a standalone button.
///
/// # Examples
///
/// ```
/// use quadraui::{Button, ButtonEvent, ButtonHit, Key, Modifiers, NamedKey, Point, Rect, WidgetId};
///
/// let mut button = Button::new(WidgetId::new("dialog:ok"), "OK");
/// button.focused = true;
///
/// let layout = button.layout(Rect::new(0.0, 0.0, 6.0, 1.0));
/// assert_eq!(layout.hit_test(Point::new(1.0, 0.0)), ButtonHit::Body);
///
/// let event = button.handle_key(&Key::Named(NamedKey::Enter), &Modifiers::default());
/// assert_eq!(event, Some(ButtonEvent::Activated { id: button.id.clone() }));
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Button {
    pub id: WidgetId,
    /// Visible label. Ignored (but still present) for
    /// [`ButtonVariant::Icon`] — a host typically pairs an icon button
    /// with an [`crate::Icon`] carried alongside the button itself rather
    /// than through it, same as [`crate::Toolbar`]'s items.
    pub label: String,
    #[serde(default)]
    pub variant: ButtonVariant,
    /// When `true`, [`Button::handle_key`] and [`Button::click`] both
    /// return `None` — same "disabled means inert, not hidden" contract
    /// every other primitive's `disabled` flag follows.
    #[serde(default)]
    pub disabled: bool,
    /// Whether this button currently holds keyboard focus. Set by the
    /// host's focus-traversal logic; read by [`Button::handle_key`] to
    /// gate activation.
    #[serde(default)]
    pub focused: bool,
}

impl Button {
    /// A primary, enabled, unfocused button with `label`. Set
    /// `variant`/`disabled`/`focused` directly afterwards (every field
    /// is `pub`) rather than via a `with_*` chain — see
    /// `docs/PRIMITIVE_RULES.md`'s rule-8 history of why this crate
    /// stopped adding those.
    pub fn new(id: WidgetId, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
            variant: ButtonVariant::Primary,
            disabled: false,
            focused: false,
        }
    }

    /// Resolve the button's hit-testable bounds. A `Button` has no
    /// internal sub-regions — the whole box is one hit target.
    pub fn layout(&self, bounds: Rect) -> ButtonLayout {
        ButtonLayout { bounds }
    }

    /// `Enter`/`Space` activates when focused and enabled. Any other
    /// key is unhandled (`None`) — the same "return `None` for a key
    /// this control doesn't own" contract [`crate::Toolbar`] and
    /// [`crate::PipelineView`] already follow.
    pub fn handle_key(&self, key: &Key, _modifiers: &Modifiers) -> Option<ButtonEvent> {
        if self.disabled || !self.focused {
            return None;
        }
        match key {
            Key::Named(NamedKey::Enter) | Key::Char(' ') => Some(ButtonEvent::Activated {
                id: self.id.clone(),
            }),
            _ => None,
        }
    }

    /// Pointer activation: fires when enabled, regardless of
    /// [`Button::focused`] (a click doesn't require prior keyboard
    /// focus the way `Enter`/`Space` does). The host calls this after
    /// confirming a `MouseDown`/`MouseUp` landed on [`ButtonHit::Body`]
    /// via [`ButtonLayout::hit_test`].
    pub fn click(&self) -> Option<ButtonEvent> {
        if self.disabled {
            return None;
        }
        Some(ButtonEvent::Activated {
            id: self.id.clone(),
        })
    }
}

/// Fully-resolved button layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ButtonLayout {
    pub bounds: Rect,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonHit {
    Body,
    Outside,
}

impl ButtonLayout {
    pub fn hit_test(&self, p: Point) -> ButtonHit {
        if self.bounds.contains(p) {
            ButtonHit::Body
        } else {
            ButtonHit::Outside
        }
    }
}

/// Emitted by [`Button::handle_key`]/[`Button::click`] when the button
/// activates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ButtonEvent {
    Activated { id: WidgetId },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn button() -> Button {
        Button::new(WidgetId::new("dialog:ok"), "OK")
    }

    #[test]
    fn new_defaults_to_primary_enabled_and_unfocused() {
        let b = button();
        assert_eq!(b.variant, ButtonVariant::Primary);
        assert!(!b.disabled);
        assert!(!b.focused);
        assert_eq!(b.label, "OK");
    }

    #[test]
    fn hit_test_inside_and_outside() {
        let b = button();
        let layout = b.layout(Rect::new(2.0, 2.0, 6.0, 1.0));
        assert_eq!(layout.hit_test(Point::new(3.0, 2.0)), ButtonHit::Body);
        assert_eq!(layout.hit_test(Point::new(0.0, 0.0)), ButtonHit::Outside);
    }

    #[test]
    fn handle_key_activates_on_enter_when_focused_and_enabled() {
        let mut b = button();
        b.focused = true;
        let event = b.handle_key(&Key::Named(NamedKey::Enter), &Modifiers::default());
        assert_eq!(event, Some(ButtonEvent::Activated { id: b.id.clone() }));
    }

    #[test]
    fn handle_key_activates_on_space() {
        let mut b = button();
        b.focused = true;
        let event = b.handle_key(&Key::Char(' '), &Modifiers::default());
        assert_eq!(event, Some(ButtonEvent::Activated { id: b.id.clone() }));
    }

    #[test]
    fn handle_key_ignores_unrelated_keys() {
        let mut b = button();
        b.focused = true;
        assert_eq!(
            b.handle_key(&Key::Named(NamedKey::Tab), &Modifiers::default()),
            None
        );
    }

    #[test]
    fn handle_key_is_inert_when_not_focused() {
        let b = button();
        assert_eq!(
            b.handle_key(&Key::Named(NamedKey::Enter), &Modifiers::default()),
            None
        );
    }

    #[test]
    fn handle_key_is_inert_when_disabled() {
        let mut b = button();
        b.focused = true;
        b.disabled = true;
        assert_eq!(
            b.handle_key(&Key::Named(NamedKey::Enter), &Modifiers::default()),
            None
        );
    }

    #[test]
    fn click_activates_without_requiring_focus() {
        let b = button();
        assert_eq!(b.click(), Some(ButtonEvent::Activated { id: b.id.clone() }));
    }

    #[test]
    fn click_is_inert_when_disabled() {
        let mut b = button();
        b.disabled = true;
        assert_eq!(b.click(), None);
    }

    #[test]
    fn serde_round_trip() {
        let mut b = button();
        b.variant = ButtonVariant::Secondary;
        b.focused = true;
        let json = serde_json::to_string(&b).unwrap();
        let back: Button = serde_json::from_str(&json).unwrap();
        assert_eq!(b, back);
    }

    #[test]
    fn serde_defaults_variant_primary_and_flags_false_when_omitted() {
        let json = r#"{"id":"dialog:ok","label":"OK"}"#;
        let b: Button = serde_json::from_str(json).unwrap();
        assert_eq!(b.variant, ButtonVariant::Primary);
        assert!(!b.disabled);
        assert!(!b.focused);
    }
}
