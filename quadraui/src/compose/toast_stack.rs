//! `ToastStackController` — VS Code-style keyboard-focus driver for the
//! in-canvas [`ToastStack`] primitive (#1185).
//!
//! Toasts are non-modal (see [`crate::primitives::toast`]'s module doc)
//! — unlike [`crate::compose::MessageDialogController`], this controller
//! never owns the stack or blocks input while unfocused. It is a pure
//! keyboard-focus cursor: the app builds its own `ToastStack` each frame
//! (as it always has), and *optionally* hands this controller keyboard
//! focus — with its own command or keybinding, e.g. a "focus
//! notifications" action — at which point Tab/Shift+Tab/Left/Right cycle
//! through the focused toast's buttons (dismiss `×` included), Up/Down
//! move between toasts, Enter activates the focused button, and Escape
//! dismisses the focused toast and returns focus to the app.
//!
//! # Usage pattern
//!
//! ```rust,ignore
//! // App-owned state:
//! let mut toasts: Vec<ToastItem> = vec![/* ... */];
//! let mut controller = ToastStackController::new();
//!
//! // Some app keybinding gives the stack focus:
//! controller.give_focus(&stack);
//!
//! // In AppLogic::render, whenever there are toasts to show:
//! let mut stack = ToastStack { id, corner, toasts: toasts.clone(), focus: None };
//! stack.focus = controller.focus(); // attaches the focus ring for painting
//! backend.draw_toast_stack(rect, &stack);
//!
//! // In AppLogic::handle, before any other keyboard routing (so the
//! // controller can claim navigation keys while it has focus):
//! match controller.handle(&event, &stack) {
//!     ToastStackEvent::Action(id) => { /* run the action named by `id` */ }
//!     ToastStackEvent::Dismiss(id) => { toasts.retain(|t| t.id != id); }
//!     ToastStackEvent::FocusReturned => { /* nothing to do — focus already cleared */ }
//!     ToastStackEvent::Consumed => { /* redraw */ }
//!     ToastStackEvent::Ignored => { /* fall through to normal app key routing */ }
//! }
//! ```
//!
//! # Keyboard model
//!
//! - Tab / Shift+Tab and Left / Right cycle keyboard focus among the
//!   focused toast's controls — dismiss `×` first, then each of
//!   [`ToastItem::actions`] in order — wrapping at either end.
//! - Up / Down move focus to the previous / next toast in
//!   [`ToastStack::toasts`] (temporal, oldest-first) order, clamping at
//!   either end rather than wrapping, and reset the in-toast cursor back
//!   to the dismiss button. (A corner-aware "visual stacking order"
//!   would invert this for bottom corners; this controller intentionally
//!   keeps the simpler array-order contract — see this module's tests.)
//! - Enter activates the focused control: [`ToastStackEvent::Dismiss`]
//!   for `×`, [`ToastStackEvent::Action`] (carrying that action's own
//!   [`crate::types::WidgetId`]) otherwise.
//! - Escape dismisses the focused toast *and* clears focus — both
//!   "dismiss the toast" and "return focus to the app" in one keystroke,
//!   since there is nothing left to keep focused once its toast is gone.
//! - Every other key, and every key while nothing is focused, is
//!   [`ToastStackEvent::Ignored`] — the controller never claims focus for
//!   itself, and never consumes a key it isn't the target of (#1185
//!   acceptance: "no key consumed while unfocused").
//!
//! # Mouse clicks are unaffected
//!
//! This controller only ever inspects [`UiEvent::KeyPressed`]. Mouse
//! clicks keep resolving exactly as before, via
//! [`crate::Backend::toast_stack_layout`]/[`crate::primitives::toast::ToastStackLayout::hit_test`]
//! — see `examples/common/toast_app.rs`'s `MouseDown` arm for the
//! existing pattern, unchanged by this controller's existence.
//!
//! # Visible focus indicator
//!
//! [`Self::focus`] returns the current [`ToastFocus`] (or `None`); the
//! caller attaches it to [`ToastStack::focus`] before painting so every
//! backend's rasteriser draws a `theme.link_fg` ring around the focused
//! control — see `primitives::toast::native_surface_paint::paint_toast`
//! and `tui::toast::paint_toast`.

use crate::primitives::toast::{ToastFocus, ToastFocusTarget, ToastStack};
use crate::types::WidgetId;
use crate::{Key, NamedKey, UiEvent};

/// What happened after [`ToastStackController::handle`] processed an
/// event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToastStackEvent {
    /// Enter activated the focused action button — carries that
    /// action's own [`WidgetId`] (matching [`crate::primitives::toast::ToastHit::Action`]'s
    /// shape, so the app dispatches identically whether the action was
    /// reached by mouse or keyboard).
    Action(WidgetId),
    /// Enter activated the focused dismiss `×`, or Escape dismissed the
    /// focused toast — carries the toast's own `WidgetId` (matching
    /// [`crate::primitives::toast::ToastHit::Dismiss`]'s shape).
    Dismiss(WidgetId),
    /// Escape cleared focus with no toast to dismiss (defensive —
    /// reachable if `handle` is called after the focused toast already
    /// vanished from `stack` by some other path; see [`Self::handle`]'s
    /// stale-focus resync).
    FocusReturned,
    /// Event consumed — internal state (which control has focus)
    /// changed; caller should redraw.
    Consumed,
    /// Event not relevant to this controller — either it isn't a key
    /// event, the stack has no focus, or the key isn't one of this
    /// controller's navigation keys. The caller should route it to
    /// normal app key handling.
    Ignored,
}

/// Cross-backend keyboard-focus cursor for a non-modal [`ToastStack`].
///
/// See the [module-level documentation](self) for the full usage
/// pattern and keyboard model.
#[derive(Debug, Default)]
pub struct ToastStackController {
    focus: Option<ToastFocus>,
}

impl ToastStackController {
    pub fn new() -> Self {
        Self { focus: None }
    }

    /// The current focus target, if any — attach to [`ToastStack::focus`]
    /// before painting so the focused control gets a visible ring.
    pub fn focus(&self) -> Option<ToastFocus> {
        self.focus.clone()
    }

    /// Whether the stack currently has keyboard focus at all.
    pub fn is_focused(&self) -> bool {
        self.focus.is_some()
    }

    /// Give the stack keyboard focus, seeding it on the newest toast's
    /// dismiss button (`stack.toasts.last()` — the temporally most
    /// recent, matching every corner's "newest nearest the user"
    /// convention per [`crate::primitives::toast::ToastStack::layout`]'s
    /// own doc). Returns `false` (and leaves focus untouched) if `stack`
    /// has no toasts to focus.
    ///
    /// Call this from the app's own keybinding/command that means
    /// "focus notifications" — this controller never claims focus on
    /// its own initiative (#1185: "the app decides how focus arrives").
    pub fn give_focus(&mut self, stack: &ToastStack) -> bool {
        match stack.toasts.last() {
            Some(t) => {
                self.focus = Some(ToastFocus {
                    toast_id: t.id.clone(),
                    target: ToastFocusTarget::Dismiss,
                });
                true
            }
            None => false,
        }
    }

    /// Clear focus, returning it to the app — the same end state
    /// [`Self::handle`]'s Escape handling reaches, exposed directly for
    /// an app that wants to drop focus programmatically (e.g. the last
    /// toast was dismissed by a mouse click elsewhere).
    pub fn take_focus(&mut self) {
        self.focus = None;
    }

    /// Drive the state machine with a backend-neutral [`UiEvent`]. Only
    /// [`UiEvent::KeyPressed`] is ever consumed — every other event
    /// variant, and every key while unfocused, is
    /// [`ToastStackEvent::Ignored`].
    pub fn handle(&mut self, event: &UiEvent, stack: &ToastStack) -> ToastStackEvent {
        let UiEvent::KeyPressed { key, modifiers, .. } = event else {
            return ToastStackEvent::Ignored;
        };
        self.resync(stack);
        let Some(focus) = self.focus.clone() else {
            return ToastStackEvent::Ignored;
        };
        let Some(toast_idx) = stack.toasts.iter().position(|t| t.id == focus.toast_id) else {
            // Resync above should have already cleared this; defensive.
            self.focus = None;
            return ToastStackEvent::Ignored;
        };
        let control_count = 1 + stack.toasts[toast_idx].actions.len();
        let cur = target_to_index(focus.target);

        match key {
            Key::Named(NamedKey::Escape) => {
                self.focus = None;
                ToastStackEvent::Dismiss(focus.toast_id)
            }
            Key::Named(NamedKey::Enter) => match focus.target {
                ToastFocusTarget::Dismiss => ToastStackEvent::Dismiss(focus.toast_id),
                ToastFocusTarget::Action(i) => match stack.toasts[toast_idx].actions.get(i) {
                    Some(a) => ToastStackEvent::Action(a.id.clone()),
                    None => ToastStackEvent::Consumed,
                },
            },
            Key::Named(NamedKey::Left) => {
                self.set_index(&focus.toast_id, (cur + control_count - 1) % control_count);
                ToastStackEvent::Consumed
            }
            Key::Named(NamedKey::Right) => {
                self.set_index(&focus.toast_id, (cur + 1) % control_count);
                ToastStackEvent::Consumed
            }
            Key::Named(NamedKey::Tab) => {
                let next = if modifiers.shift {
                    (cur + control_count - 1) % control_count
                } else {
                    (cur + 1) % control_count
                };
                self.set_index(&focus.toast_id, next);
                ToastStackEvent::Consumed
            }
            Key::Named(NamedKey::Up) => {
                if toast_idx > 0 {
                    self.focus_toast_at(stack, toast_idx - 1);
                }
                ToastStackEvent::Consumed
            }
            Key::Named(NamedKey::Down) => {
                if toast_idx + 1 < stack.toasts.len() {
                    self.focus_toast_at(stack, toast_idx + 1);
                }
                ToastStackEvent::Consumed
            }
            _ => ToastStackEvent::Ignored,
        }
    }

    /// Drop focus if it names a toast no longer present in `stack` (e.g.
    /// dismissed by a mouse click, or auto-expired) — an app that keeps
    /// calling `handle` after that shouldn't get stale
    /// `ToastStackEvent::Action`/`Dismiss` results for a toast that no
    /// longer exists.
    fn resync(&mut self, stack: &ToastStack) {
        if let Some(f) = &self.focus {
            if !stack.toasts.iter().any(|t| t.id == f.toast_id) {
                self.focus = None;
            }
        }
    }

    fn set_index(&mut self, toast_id: &WidgetId, index: usize) {
        self.focus = Some(ToastFocus {
            toast_id: toast_id.clone(),
            target: index_to_target(index),
        });
    }

    fn focus_toast_at(&mut self, stack: &ToastStack, idx: usize) {
        if let Some(t) = stack.toasts.get(idx) {
            self.focus = Some(ToastFocus {
                toast_id: t.id.clone(),
                target: ToastFocusTarget::Dismiss,
            });
        }
    }
}

fn target_to_index(target: ToastFocusTarget) -> usize {
    match target {
        ToastFocusTarget::Dismiss => 0,
        ToastFocusTarget::Action(i) => i + 1,
    }
}

fn index_to_target(index: usize) -> ToastFocusTarget {
    if index == 0 {
        ToastFocusTarget::Dismiss
    } else {
        ToastFocusTarget::Action(index - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::toast::{ToastAction, ToastCorner, ToastItem, ToastSeverity};
    use crate::Modifiers;

    fn toast(id: &str) -> ToastItem {
        ToastItem {
            id: WidgetId::new(id),
            title: id.to_string(),
            body: String::new(),
            severity: ToastSeverity::Info,
            actions: Vec::new(),
            accent: None,
        }
    }

    fn toast_with_actions(id: &str, action_ids: &[&str]) -> ToastItem {
        ToastItem {
            actions: action_ids
                .iter()
                .map(|a| ToastAction {
                    id: WidgetId::new(*a),
                    label: (*a).to_string(),
                    primary: false,
                })
                .collect(),
            ..toast(id)
        }
    }

    fn stack(toasts: Vec<ToastItem>) -> ToastStack {
        ToastStack {
            id: WidgetId::new("toasts"),
            corner: ToastCorner::BottomRight,
            toasts,
            focus: None,
        }
    }

    fn key(named: NamedKey) -> UiEvent {
        UiEvent::KeyPressed {
            key: Key::Named(named),
            modifiers: Modifiers::default(),
            repeat: false,
        }
    }

    fn shift_key(named: NamedKey) -> UiEvent {
        UiEvent::KeyPressed {
            key: Key::Named(named),
            modifiers: Modifiers {
                shift: true,
                ..Modifiers::default()
            },
            repeat: false,
        }
    }

    #[test]
    fn new_controller_has_no_focus() {
        let c = ToastStackController::new();
        assert!(!c.is_focused());
        assert_eq!(c.focus(), None);
    }

    #[test]
    fn unfocused_controller_ignores_every_key() {
        let mut c = ToastStackController::new();
        let s = stack(vec![toast("t1")]);
        for k in [
            NamedKey::Tab,
            NamedKey::Left,
            NamedKey::Right,
            NamedKey::Up,
            NamedKey::Down,
            NamedKey::Enter,
            NamedKey::Escape,
        ] {
            assert_eq!(c.handle(&key(k), &s), ToastStackEvent::Ignored);
        }
        assert!(!c.is_focused());
    }

    #[test]
    fn give_focus_seeds_newest_toast_dismiss() {
        let mut c = ToastStackController::new();
        let s = stack(vec![toast("t1"), toast("t2")]);
        assert!(c.give_focus(&s));
        assert_eq!(
            c.focus(),
            Some(ToastFocus {
                toast_id: WidgetId::new("t2"),
                target: ToastFocusTarget::Dismiss,
            })
        );
    }

    #[test]
    fn give_focus_on_empty_stack_returns_false() {
        let mut c = ToastStackController::new();
        let s = stack(vec![]);
        assert!(!c.give_focus(&s));
        assert!(!c.is_focused());
    }

    #[test]
    fn tab_cycles_dismiss_then_actions_then_wraps() {
        let mut c = ToastStackController::new();
        let s = stack(vec![toast_with_actions("t1", &["a", "b"])]);
        c.give_focus(&s);
        assert_eq!(c.focus().unwrap().target, ToastFocusTarget::Dismiss);

        c.handle(&key(NamedKey::Tab), &s);
        assert_eq!(c.focus().unwrap().target, ToastFocusTarget::Action(0));

        c.handle(&key(NamedKey::Tab), &s);
        assert_eq!(c.focus().unwrap().target, ToastFocusTarget::Action(1));

        c.handle(&key(NamedKey::Tab), &s);
        assert_eq!(
            c.focus().unwrap().target,
            ToastFocusTarget::Dismiss,
            "wraps back to dismiss"
        );
    }

    #[test]
    fn shift_tab_cycles_backward() {
        let mut c = ToastStackController::new();
        let s = stack(vec![toast_with_actions("t1", &["a"])]);
        c.give_focus(&s);
        c.handle(&shift_key(NamedKey::Tab), &s);
        assert_eq!(
            c.focus().unwrap().target,
            ToastFocusTarget::Action(0),
            "shift-tab from dismiss wraps to the last control"
        );
    }

    #[test]
    fn left_right_cycle_the_same_as_tab() {
        let mut c = ToastStackController::new();
        let s = stack(vec![toast_with_actions("t1", &["a"])]);
        c.give_focus(&s);
        c.handle(&key(NamedKey::Right), &s);
        assert_eq!(c.focus().unwrap().target, ToastFocusTarget::Action(0));
        c.handle(&key(NamedKey::Left), &s);
        assert_eq!(c.focus().unwrap().target, ToastFocusTarget::Dismiss);
    }

    #[test]
    fn up_down_move_between_toasts_and_reset_to_dismiss() {
        let mut c = ToastStackController::new();
        let s = stack(vec![toast("t1"), toast_with_actions("t2", &["a"])]);
        c.give_focus(&s); // seeds t2 (newest)
        c.handle(&key(NamedKey::Right), &s); // t2/Action(0)
        assert_eq!(c.focus().unwrap().toast_id, WidgetId::new("t2"));

        c.handle(&key(NamedKey::Up), &s);
        assert_eq!(
            c.focus(),
            Some(ToastFocus {
                toast_id: WidgetId::new("t1"),
                target: ToastFocusTarget::Dismiss,
            })
        );

        // Up again at the first toast is a no-op (clamped, not wrapped).
        c.handle(&key(NamedKey::Up), &s);
        assert_eq!(c.focus().unwrap().toast_id, WidgetId::new("t1"));

        c.handle(&key(NamedKey::Down), &s);
        assert_eq!(c.focus().unwrap().toast_id, WidgetId::new("t2"));
        assert_eq!(c.focus().unwrap().target, ToastFocusTarget::Dismiss);
    }

    #[test]
    fn enter_on_dismiss_resolves_dismiss() {
        let mut c = ToastStackController::new();
        let s = stack(vec![toast("t1")]);
        c.give_focus(&s);
        assert_eq!(
            c.handle(&key(NamedKey::Enter), &s),
            ToastStackEvent::Dismiss(WidgetId::new("t1"))
        );
    }

    #[test]
    fn enter_on_action_resolves_that_actions_id() {
        let mut c = ToastStackController::new();
        let s = stack(vec![toast_with_actions("t1", &["install", "skip"])]);
        c.give_focus(&s);
        c.handle(&key(NamedKey::Tab), &s); // -> Action(0) = "install"
        assert_eq!(
            c.handle(&key(NamedKey::Enter), &s),
            ToastStackEvent::Action(WidgetId::new("install"))
        );
    }

    #[test]
    fn escape_dismisses_and_clears_focus() {
        let mut c = ToastStackController::new();
        let s = stack(vec![toast("t1")]);
        c.give_focus(&s);
        assert_eq!(
            c.handle(&key(NamedKey::Escape), &s),
            ToastStackEvent::Dismiss(WidgetId::new("t1"))
        );
        assert!(!c.is_focused());
    }

    #[test]
    fn take_focus_clears_without_an_event() {
        let mut c = ToastStackController::new();
        let s = stack(vec![toast("t1")]);
        c.give_focus(&s);
        c.take_focus();
        assert!(!c.is_focused());
        assert_eq!(
            c.handle(&key(NamedKey::Enter), &s),
            ToastStackEvent::Ignored
        );
    }

    /// #1185 acceptance: a toast removed by some other path (e.g. a
    /// mouse click dismissing it) drops stale focus instead of letting
    /// `handle` keep resolving events against a toast that's gone.
    #[test]
    fn handle_resyncs_when_focused_toast_vanished() {
        let mut c = ToastStackController::new();
        let s1 = stack(vec![toast("t1")]);
        c.give_focus(&s1);
        let s2 = stack(vec![]); // t1 dismissed by some other path
        assert_eq!(
            c.handle(&key(NamedKey::Enter), &s2),
            ToastStackEvent::Ignored
        );
        assert!(!c.is_focused());
    }

    #[test]
    fn non_key_events_are_ignored() {
        let mut c = ToastStackController::new();
        let s = stack(vec![toast("t1")]);
        c.give_focus(&s);
        assert_eq!(
            c.handle(&UiEvent::CharTyped('x'), &s),
            ToastStackEvent::Ignored
        );
        // Focus untouched by an irrelevant event.
        assert!(c.is_focused());
    }

    #[test]
    fn unrelated_key_is_ignored_even_while_focused() {
        let mut c = ToastStackController::new();
        let s = stack(vec![toast("t1")]);
        c.give_focus(&s);
        let ev = UiEvent::KeyPressed {
            key: Key::Char('a'),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        assert_eq!(c.handle(&ev, &s), ToastStackEvent::Ignored);
        assert!(c.is_focused());
    }
}
