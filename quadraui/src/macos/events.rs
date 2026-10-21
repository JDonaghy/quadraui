//! AppKit / Cocoa → `quadraui::UiEvent` translation.
//!
//! Mirrors the shape of [`crate::gtk::events`]: pure free functions taking
//! primitive input types so unit tests construct synthetic inputs without
//! linking AppKit. The QuadraView responder overrides in [`super::run`]
//! extract the relevant fields from each `NSEvent` and call into these
//! helpers.
//!
//! ## Coordinate convention
//!
//! Mouse positions arrive here in **view-local points** — the caller
//! is responsible for `[view convertPoint:event.locationInWindow
//! fromView:nil]` before calling. Because `QuadraView` overrides
//! `isFlipped` to return `true`, top-left is `(0, 0)` and y grows
//! downward — matching TUI / GTK / quadraui's [`crate::Point`] convention.
//!
//! ## Scroll-sign convention
//!
//! NSEvent's `scrollingDeltaY` is positive when the user requests
//! content to move **down** (scroll forward through the document).
//! [`crate::UiEvent::Scroll::delta`] follows the opposite convention
//! (positive y = up toward the top of content), so we negate. This
//! matches the GTK translator's behaviour — see
//! [`crate::gtk::events::gdk_scroll_to_uievent`].
//!
//! ## Precise (trackpad) vs. discrete (wheel) deltas
//!
//! [`ns_scroll`] passes `scrollingDeltaX`/`scrollingDeltaY` through at
//! full `f32` precision regardless of source device, so trackpad
//! scrolling is already proportional and sub-row. The two device
//! classes still need different *delivery* treatment, decided by
//! [`route_wheel_event`] (called from [`super::run`] with
//! `NSEvent.hasPreciseScrollingDeltas()`):
//!
//! - **Trackpad** (precise): AppKit itself delivers a decaying stream
//!   of real `scrollWheel:` calls during a momentum-phase flick, so
//!   [`super::run::QuadraView::objc_scroll_wheel`] dispatches each one
//!   straight through ([`WheelRouting::Immediate`]) — no synthetic
//!   animation needed.
//! - **Discrete wheel** (imprecise): a plain wheel notch arrives as one
//!   `scrollWheel:` call with no native momentum, so `route_wheel_event`
//!   returns [`WheelRouting::Glide`] and `objc_scroll_wheel` wires it
//!   through [`crate::transition::Transition`] to glide the notch's
//!   delta out over [`WHEEL_MOMENTUM_DURATION`] instead of dispatching
//!   it as a single lump. Because each tick's dispatched delta must
//!   stay a whole multiple of the underlying row/line unit (row-index
//!   consumers like [`crate::primitives::scrollbar::wheel_row_delta`]
//!   round any nonzero fractional delta up to at least one row), the
//!   glide is quantised: [`wheel_momentum_quantized_target`] computes
//!   the cumulative whole-unit amount that should have been delivered
//!   by a given eased progress, and the caller dispatches only the
//!   incremental difference from the previous tick. [`ease_out_cubic`]
//!   and [`fold_wheel_momentum_remaining`] are the other pure pieces of
//!   that math, kept here (rather than in `super::run`) so they're
//!   unit-testable without AppKit.

use std::time::Duration;

use crate::{ButtonMask, Key, Modifiers, MouseButton, NamedKey, UiEvent};

// ─── NSEventModifierFlags bits ──────────────────────────────────────────────
//
// Documented constants from `<AppKit/NSEvent.h>`. We deliberately accept the
// flag set as a raw `usize` (matching `NSEvent.modifierFlags()`'s storage
// width) rather than the typed `objc2_app_kit::NSEventModifierFlags` — tests
// can then construct flag combinations as plain integers without linking
// AppKit. The high bits we ignore (capslock, numeric-pad, function-key,
// help) don't map to a `quadraui::Modifiers` field.

const NS_FLAG_SHIFT: usize = 1 << 17;
const NS_FLAG_CONTROL: usize = 1 << 18;
const NS_FLAG_OPTION: usize = 1 << 19; // Mac Option / Alt key
const NS_FLAG_COMMAND: usize = 1 << 20;

/// Translate `NSEvent.modifierFlags()` (as `usize` bits) into
/// [`Modifiers`]. Maps Shift / Control / Option / Command directly.
/// Other flag bits (CapsLock, Numeric Pad, Function key, Help) are
/// dropped — they don't have a [`Modifiers`] counterpart.
pub fn ns_modifier_flags_to_quadraui(flags: usize) -> Modifiers {
    Modifiers {
        shift: flags & NS_FLAG_SHIFT != 0,
        ctrl: flags & NS_FLAG_CONTROL != 0,
        alt: flags & NS_FLAG_OPTION != 0,
        cmd: flags & NS_FLAG_COMMAND != 0,
    }
}

/// Translate Cocoa's `NSEvent.buttonNumber()` into [`MouseButton`].
///
/// Cocoa convention is 0-based (unlike GTK's 1-based scheme):
/// `0 = primary (left), 1 = secondary (right), 2 = middle,
///  3 = back / X1, 4 = forward / X2, n = Other(n)`.
pub fn ns_button_number_to_quadraui(n: i64) -> MouseButton {
    match n {
        0 => MouseButton::Left,
        1 => MouseButton::Right,
        2 => MouseButton::Middle,
        3 => MouseButton::X1,
        4 => MouseButton::X2,
        n if (0..=255).contains(&n) => MouseButton::Other(n as u8),
        _ => MouseButton::Other(255),
    }
}

/// Build a [`UiEvent::MouseDown`] from a translated `NSEvent`.
/// `x`, `y` are view-local points; `flags` is `NSEvent.modifierFlags()`.
pub fn ns_mouse_down(button: i64, x: f64, y: f64, flags: usize) -> UiEvent {
    crate::event::mouse_down(
        ns_button_number_to_quadraui(button),
        x as f32,
        y as f32,
        ns_modifier_flags_to_quadraui(flags),
    )
}

/// Build a [`UiEvent::MouseUp`] from a translated `NSEvent`.
pub fn ns_mouse_up(button: i64, x: f64, y: f64) -> UiEvent {
    crate::event::mouse_up(ns_button_number_to_quadraui(button), x as f32, y as f32)
}

/// Build a [`UiEvent::MouseMoved`] from `mouseMoved:` / `mouseDragged:`.
/// `buttons` is the current button-held state inferred by the caller from
/// the event type (`mouseMoved:` → all-false; `mouseDragged:` → the dragged
/// button bit set).
pub fn ns_mouse_moved(x: f64, y: f64, buttons: ButtonMask) -> UiEvent {
    crate::event::mouse_moved(x as f32, y as f32, buttons)
}

/// Build a [`UiEvent::Scroll`] from `scrollWheel:` deltas. Negates
/// `dy` so the result follows quadraui's "positive y = up" convention.
pub fn ns_scroll(dx: f64, dy: f64, x: f64, y: f64) -> UiEvent {
    crate::event::scroll(dx as f32, dy as f32, x as f32, y as f32)
}

/// Total duration of one synthetic discrete-wheel momentum glide.
///
/// Short enough that a quick run of repeated notches still feels
/// responsive (each notch folds into the in-flight ramp rather than
/// queuing behind it — see [`fold_wheel_momentum_remaining`]), long
/// enough to read as a glide rather than an instant jump.
pub(crate) const WHEEL_MOMENTUM_DURATION: Duration = Duration::from_millis(220);

/// Tick interval for the repeating timer that drives a discrete-wheel
/// momentum glide (~60 Hz).
pub(crate) const WHEEL_MOMENTUM_TICK: Duration = Duration::from_millis(16);

/// Cubic ease-out: fast start, gentle settle. `t` is clamped to
/// `0.0..=1.0` first, so `ease_out_cubic(0.0) == 0.0` and
/// `ease_out_cubic(1.0) == 1.0` exactly — this is what makes
/// [`wheel_momentum_quantized_target`] conserve the total notch distance
/// (see that function's doc).
pub(crate) fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Combine a still-undelivered momentum total with a freshly arrived
/// wheel notch, for when a new `scrollWheel:` call lands while a
/// previous notch's glide is still animating.
///
/// `old_total` is the total delta the in-flight
/// [`crate::transition::Transition`] was asked to deliver; `old_delivered`
/// is the whole-unit amount already dispatched out of it (the previous
/// tick's [`wheel_momentum_quantized_target`] result).
/// `old_total - old_delivered` is therefore the portion *not yet*
/// dispatched, which gets folded into `new_notch` so the next glide
/// starts from a correct cumulative baseline — no already-delivered
/// distance is replayed, and no in-flight distance is silently dropped.
pub(crate) fn fold_wheel_momentum_remaining(
    old_total: f32,
    old_delivered: f32,
    new_notch: f32,
) -> f32 {
    (old_total - old_delivered) + new_notch
}

/// The cumulative whole-unit amount a momentum glide should have
/// delivered by a given eased progress.
///
/// `total` is the total distance this glide must deliver; `eased` is
/// the eased progress ([`ease_out_cubic`] applied to
/// [`crate::transition::Transition::progress_at`]) at the current tick.
/// The result is always `(total * eased).round()` — a whole multiple of
/// the caller's underlying unit, never a fraction. Callers dispatch
/// only the *difference* from the previous tick's target, so the
/// dispatched stream never contains a tiny nonzero fractional delta
/// that a row-index consumer (e.g.
/// [`crate::primitives::scrollbar::wheel_row_delta`], which floors any
/// nonzero delta up to at least one row) would inflate past the
/// notch's true magnitude. Because `ease_out_cubic(0.0) == 0.0` and
/// `ease_out_cubic(1.0) == 1.0`, the final tick's target is always
/// `total.round()`, so the sum of dispatched increments across a full
/// glide equals exactly one un-split notch.
pub(crate) fn wheel_momentum_quantized_target(total: f32, eased: f32) -> f32 {
    (total * eased).round()
}

/// How a translated `scrollWheel:` event should be delivered, decided
/// by [`route_wheel_event`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum WheelRouting {
    /// Dispatch this event immediately; cancel any in-flight discrete
    /// wheel momentum glide first.
    Immediate(UiEvent),
    /// Fold this delta into (or start) a synthetic discrete-wheel
    /// momentum glide instead of dispatching it directly.
    Glide { dx: f32, dy: f32, x: f32, y: f32 },
}

/// Decide how a `scrollWheel:` event should be delivered.
///
/// `precise` is `NSEvent.hasPreciseScrollingDeltas()`; `dx`/`dy` are
/// `scrollingDeltaX`/`scrollingDeltaY`; `x`/`y` are the view-local
/// point. Precise (trackpad) deltas are delivered immediately, since
/// AppKit already provides native momentum-phase events for those.
/// Imprecise (discrete wheel) deltas are routed into a synthetic
/// momentum glide instead. This is a pure function so the routing
/// decision is unit-testable without AppKit.
pub(crate) fn route_wheel_event(precise: bool, dx: f64, dy: f64, x: f64, y: f64) -> WheelRouting {
    let event = ns_scroll(dx, dy, x, y);
    if precise {
        return WheelRouting::Immediate(event);
    }
    match event {
        UiEvent::Scroll {
            delta, position, ..
        } => WheelRouting::Glide {
            dx: delta.x,
            dy: delta.y,
            x: position.x,
            y: position.y,
        },
        other => WheelRouting::Immediate(other),
    }
}

/// Translate `keyDown:` into a [`UiEvent::KeyPressed`].
///
/// `characters` is the IME-resolved string from `NSEvent.characters()` —
/// `Some("a")` for a printable key, often `None` or a non-printable
/// control character for arrows / function keys. The translator
/// prefers `characters` for printable keys (already layout-aware) and
/// falls back to `key_code` for navigation / function / control keys
/// via [`ns_keycode_to_named_key`].
///
/// Returns `None` for keys with no quadraui counterpart (modifier-only
/// presses, dead keys, unknown function keys).
pub fn ns_key_to_uievent(
    characters: Option<&str>,
    key_code: u16,
    flags: usize,
    repeat: bool,
) -> Option<UiEvent> {
    let modifiers = ns_modifier_flags_to_quadraui(flags);

    // Try the keycode → NamedKey lookup first. macOS reports
    // arrows / function / nav keys with both a (non-printable)
    // `characters` string and a stable `key_code`; the keycode is
    // the only reliable source.
    if let Some(named) = ns_keycode_to_named_key(key_code) {
        // Shift+Tab on macOS arrives as the same Tab keycode (0x30)
        // with the shift modifier set; AppKit doesn't have a separate
        // back-tab keycode the way GTK's `ISO_Left_Tab` and crossterm's
        // `BackTab` do. Translate to `NamedKey::BackTab` here so
        // backend-neutral consumers (`SidebarSystem`, `FocusRing`,
        // etc.) match the same variant on every backend.
        let named = if matches!(named, NamedKey::Tab) && modifiers.shift {
            NamedKey::BackTab
        } else {
            named
        };
        return Some(UiEvent::KeyPressed {
            key: Key::Named(named),
            modifiers,
            repeat,
        });
    }

    // Fall back to the IME-resolved printable character.
    let first = characters?.chars().next()?;
    if first.is_control() {
        // Ctrl+letter produces \x01..\x1A; map back to the base letter
        // so apps see `Key::Char('c')` with `modifiers.ctrl == true`.
        // Mirrors the GTK translator's Ctrl+letter recovery path.
        if (1..=26).contains(&(first as u32)) {
            let base = (b'a' + (first as u8 - 1)) as char;
            return Some(UiEvent::KeyPressed {
                key: Key::Char(base),
                modifiers,
                repeat,
            });
        }
        return None;
    }
    Some(UiEvent::KeyPressed {
        key: Key::Char(first),
        modifiers,
        repeat,
    })
}

/// Map a macOS hardware key code (`kVK_*` constants from
/// `<Carbon/HIToolbox/Events.h>`) to a [`NamedKey`].
///
/// Returns `None` for keys with no quadraui counterpart — modifiers,
/// numeric-pad letters, eject, brightness, etc. The full table:
///
/// | Mac code | Key | Mac code | Key |
/// |---|---|---|---|
/// | `0x24` | Enter | `0x7B` | Left |
/// | `0x30` | Tab | `0x7C` | Right |
/// | `0x33` | Backspace | `0x7D` | Down |
/// | `0x35` | Escape | `0x7E` | Up |
/// | `0x39` | CapsLock | `0x74` | PageUp |
/// | `0x47` | NumLock | `0x79` | PageDown |
/// | `0x4C` | Enter (keypad) | `0x73` | Home |
/// | `0x72` | Insert | `0x77` | End |
/// | `0x75` | Delete (forward) | `0x7A`..`0x6F` | F1–F12 |
pub fn ns_keycode_to_named_key(key_code: u16) -> Option<NamedKey> {
    Some(match key_code {
        0x24 => NamedKey::Enter,     // Return
        0x4C => NamedKey::Enter,     // Keypad Enter
        0x30 => NamedKey::Tab,       // Tab
        0x33 => NamedKey::Backspace, // Delete (Mac's main delete-left key)
        0x75 => NamedKey::Delete,    // Forward delete (fn+Delete)
        0x72 => NamedKey::Insert,    // Help key (the closest equivalent)
        0x35 => NamedKey::Escape,
        0x39 => NamedKey::CapsLock,
        0x47 => NamedKey::NumLock, // Clear key on full keyboards
        0x73 => NamedKey::Home,
        0x77 => NamedKey::End,
        0x74 => NamedKey::PageUp,
        0x79 => NamedKey::PageDown,
        0x7B => NamedKey::Left,
        0x7C => NamedKey::Right,
        0x7D => NamedKey::Down,
        0x7E => NamedKey::Up,
        0x7A => NamedKey::F(1),
        0x78 => NamedKey::F(2),
        0x63 => NamedKey::F(3),
        0x76 => NamedKey::F(4),
        0x60 => NamedKey::F(5),
        0x61 => NamedKey::F(6),
        0x62 => NamedKey::F(7),
        0x64 => NamedKey::F(8),
        0x65 => NamedKey::F(9),
        0x6D => NamedKey::F(10),
        0x67 => NamedKey::F(11),
        0x6F => NamedKey::F(12),
        0x69 => NamedKey::F(13),
        0x6B => NamedKey::F(14),
        0x71 => NamedKey::F(15),
        0x6A => NamedKey::F(16),
        0x40 => NamedKey::F(17),
        0x4F => NamedKey::F(18),
        0x50 => NamedKey::F(19),
        0x5A => NamedKey::F(20),
        _ => return None,
    })
}

/// Build a [`UiEvent::WindowResized`] from a resize notification.
/// `width` / `height` are window content size in points; `scale` is the
/// `backingScaleFactor`. Called from `macos::run`'s
/// `viewFrameDidChange:` observer, which dispatches the result
/// synchronously alongside mouse/keyboard events.
pub fn ns_resize_to_uievent(width: f64, height: f64, scale: f32) -> UiEvent {
    crate::event::window_resized(width as f32, height as f32, scale)
}

/// Build a [`UiEvent::WindowFocused`] from `windowDidBecomeKey:` /
/// `windowDidResignKey:` notifications.
pub fn ns_focus_to_uievent(focused: bool) -> UiEvent {
    UiEvent::WindowFocused(focused)
}

/// Build a [`UiEvent::FilesDropped`] from an
/// `NSDraggingInfo`'s already-decoded file paths and view-local drop
/// point. `super::run`'s `performDragOperation:` override (on
/// `QuadraView`) is the real caller — it reads the file URLs off
/// `sender.draggingPasteboard()` (real AppKit calls, so that decode step
/// stays in `super::run` rather than this cross-platform-testable
/// module) and converts `sender.draggingLocation()` via
/// `convertPoint:fromView:` before calling this.
pub fn ns_files_dropped(paths: Vec<std::path::PathBuf>, x: f64, y: f64) -> UiEvent {
    crate::event::files_dropped(paths, x as f32, y as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Modifier translation ──────────────────────────────────────

    #[test]
    fn modifier_shift_alone() {
        let m = ns_modifier_flags_to_quadraui(NS_FLAG_SHIFT);
        assert!(m.shift);
        assert!(!m.ctrl);
        assert!(!m.alt);
        assert!(!m.cmd);
    }

    #[test]
    fn modifier_option_maps_to_alt() {
        let m = ns_modifier_flags_to_quadraui(NS_FLAG_OPTION);
        assert!(m.alt);
        assert!(!m.shift);
    }

    #[test]
    fn modifier_command_maps_to_cmd() {
        let m = ns_modifier_flags_to_quadraui(NS_FLAG_COMMAND);
        assert!(m.cmd);
        assert!(!m.ctrl);
    }

    #[test]
    fn modifier_all_four_combined() {
        let m = ns_modifier_flags_to_quadraui(
            NS_FLAG_SHIFT | NS_FLAG_CONTROL | NS_FLAG_OPTION | NS_FLAG_COMMAND,
        );
        assert!(m.shift && m.ctrl && m.alt && m.cmd);
    }

    #[test]
    fn modifier_ignores_unrelated_bits() {
        // Bit 16 = capslock, bit 21 = numeric pad — quadraui doesn't model these.
        let m = ns_modifier_flags_to_quadraui((1 << 16) | (1 << 21) | NS_FLAG_SHIFT);
        assert!(m.shift);
        assert!(!m.ctrl && !m.alt && !m.cmd);
    }

    // ── Button translation ────────────────────────────────────────

    #[test]
    fn button_zero_to_left() {
        assert_eq!(ns_button_number_to_quadraui(0), MouseButton::Left);
    }

    #[test]
    fn button_one_to_right() {
        assert_eq!(ns_button_number_to_quadraui(1), MouseButton::Right);
    }

    #[test]
    fn button_two_to_middle() {
        assert_eq!(ns_button_number_to_quadraui(2), MouseButton::Middle);
    }

    #[test]
    fn button_three_four_to_x1_x2() {
        assert_eq!(ns_button_number_to_quadraui(3), MouseButton::X1);
        assert_eq!(ns_button_number_to_quadraui(4), MouseButton::X2);
    }

    #[test]
    fn button_other_in_byte_range() {
        assert_eq!(ns_button_number_to_quadraui(7), MouseButton::Other(7));
        assert_eq!(ns_button_number_to_quadraui(200), MouseButton::Other(200));
    }

    #[test]
    fn button_out_of_byte_range_clamps_to_255() {
        assert_eq!(
            ns_button_number_to_quadraui(99_999),
            MouseButton::Other(255)
        );
    }

    // ── Mouse event variants ──────────────────────────────────────

    #[test]
    fn mouse_down_carries_coords_and_modifiers() {
        let ev = ns_mouse_down(0, 50.0, 100.0, NS_FLAG_CONTROL);
        match ev {
            UiEvent::MouseDown {
                widget,
                button,
                position,
                modifiers,
            } => {
                assert!(widget.is_none());
                assert_eq!(button, MouseButton::Left);
                assert_eq!(position.x, 50.0);
                assert_eq!(position.y, 100.0);
                assert!(modifiers.ctrl);
                assert!(!modifiers.shift);
            }
            other => panic!("expected MouseDown, got {other:?}"),
        }
    }

    #[test]
    fn mouse_up_translation() {
        let ev = ns_mouse_up(1, 200.0, 300.0);
        match ev {
            UiEvent::MouseUp {
                button, position, ..
            } => {
                assert_eq!(button, MouseButton::Right);
                assert_eq!(position.x, 200.0);
                assert_eq!(position.y, 300.0);
            }
            other => panic!("expected MouseUp, got {other:?}"),
        }
    }

    #[test]
    fn mouse_moved_carries_button_mask() {
        let buttons = ButtonMask {
            left: true,
            ..Default::default()
        };
        let ev = ns_mouse_moved(10.0, 20.0, buttons);
        match ev {
            UiEvent::MouseMoved { position, buttons } => {
                assert_eq!(position.x, 10.0);
                assert_eq!(position.y, 20.0);
                assert!(buttons.left);
                assert!(!buttons.middle && !buttons.right);
            }
            _ => panic!(),
        }
    }

    // ── Scroll sign convention ────────────────────────────────────

    #[test]
    fn scroll_negates_dy_for_quadraui_convention() {
        // NSEvent's positive dy = user wants to scroll forward (down)
        // through content. quadraui's `delta.y > 0` = up. So a Cocoa
        // dy of +1 should produce a quadraui delta.y of -1.
        let ev = ns_scroll(0.0, 1.0, 10.0, 20.0);
        match ev {
            UiEvent::Scroll {
                delta, position, ..
            } => {
                assert_eq!(delta.y, -1.0);
                assert_eq!(delta.x, 0.0);
                assert_eq!(position.x, 10.0);
                assert_eq!(position.y, 20.0);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn scroll_dx_is_not_negated() {
        let ev = ns_scroll(2.5, 0.0, 0.0, 0.0);
        match ev {
            UiEvent::Scroll { delta, .. } => {
                assert_eq!(delta.x, 2.5);
                assert_eq!(delta.y, 0.0);
            }
            _ => panic!(),
        }
    }

    // ── Discrete-wheel momentum math ──────────────────────────────

    #[test]
    fn ease_out_cubic_endpoints_are_exact() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
    }

    #[test]
    fn ease_out_cubic_clamps_out_of_range_input() {
        assert_eq!(ease_out_cubic(-1.0), 0.0);
        assert_eq!(ease_out_cubic(2.0), 1.0);
    }

    #[test]
    fn ease_out_cubic_is_monotonically_non_decreasing() {
        let mut prev = ease_out_cubic(0.0);
        for i in 1..=20 {
            let t = i as f32 / 20.0;
            let next = ease_out_cubic(t);
            assert!(next >= prev, "eased value decreased at t={t}");
            prev = next;
        }
    }

    #[test]
    fn ease_out_cubic_front_loads_progress() {
        // "Ease out" means most of the distance is covered early.
        assert!(ease_out_cubic(0.25) > 0.25);
    }

    #[test]
    fn quantized_targets_sum_to_the_rounded_total_over_a_full_sweep() {
        for total in [1.0_f32, -3.5, 42.0] {
            let mut delivered = 0.0_f32;
            for i in 1..=10 {
                let t = i as f32 / 10.0;
                let eased = ease_out_cubic(t);
                let target = wheel_momentum_quantized_target(total, eased);
                let tick_delta = target - delivered;
                delivered += tick_delta;
            }
            assert_eq!(
                delivered,
                total.round(),
                "expected {}, got {delivered}",
                total.round()
            );
        }
    }

    #[test]
    fn quantized_target_every_tick_is_a_whole_unit() {
        // A fractional tick delta would get floor-inflated by a
        // row-index consumer like `wheel_row_delta`; the quantized
        // target must never produce one.
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let eased = ease_out_cubic(t);
            let target = wheel_momentum_quantized_target(1.0, eased);
            assert_eq!(target, target.round());
        }
    }

    #[test]
    fn quantized_target_is_stable_once_fully_eased() {
        let first = wheel_momentum_quantized_target(10.0, 1.0);
        let second = wheel_momentum_quantized_target(10.0, 1.0);
        assert_eq!(first, second);
        assert_eq!(first - second, 0.0);
    }

    #[test]
    fn fold_remaining_adds_undelivered_portion_to_new_notch() {
        // 5.0 of a 10.0 total is still undelivered; a fresh notch of
        // 2.0 should fold to 7.0.
        let folded = fold_wheel_momentum_remaining(10.0, 5.0, 2.0);
        assert_eq!(folded, 7.0);
    }

    #[test]
    fn fold_remaining_with_finished_transition_is_just_the_new_notch() {
        // A fully-delivered prior transition (delivered == total)
        // contributes nothing — folding degrades to the new notch.
        let folded = fold_wheel_momentum_remaining(10.0, 10.0, 3.0);
        assert_eq!(folded, 3.0);
    }

    #[test]
    fn fold_remaining_handles_negative_deltas() {
        let folded = fold_wheel_momentum_remaining(-10.0, -5.0, -2.0);
        assert_eq!(folded, -7.0);
    }

    // ── Wheel routing decision ────────────────────────────────────

    #[test]
    fn precise_delta_routes_immediate() {
        match route_wheel_event(true, 1.5, -2.25, 10.0, 20.0) {
            WheelRouting::Immediate(UiEvent::Scroll {
                delta, position, ..
            }) => {
                assert_eq!(delta.x, 1.5);
                assert_eq!(delta.y, 2.25);
                assert_eq!(position.x, 10.0);
                assert_eq!(position.y, 20.0);
            }
            other => panic!("expected Immediate(Scroll), got {other:?}"),
        }
    }

    #[test]
    fn discrete_delta_routes_glide() {
        match route_wheel_event(false, 0.0, 1.0, 3.0, 4.0) {
            WheelRouting::Glide { dx, dy, x, y } => {
                assert_eq!(dx, 0.0);
                assert_eq!(dy, -1.0); // negated per quadraui convention
                assert_eq!(x, 3.0);
                assert_eq!(y, 4.0);
            }
            other => panic!("expected Glide, got {other:?}"),
        }
    }

    #[test]
    fn a_full_discrete_glide_applies_exactly_one_row_per_axis() {
        // This is the regression test for the over-scroll bug: a
        // single discrete wheel notch, split across the momentum
        // glide's ticks and quantized, must sum to the same row delta
        // a consumer would get from one un-split dispatch of the same
        // notch — never more.
        use crate::primitives::scrollbar::wheel_row_delta;

        for rows_per_notch in [1.0_f32, 3.0] {
            let dy = -1.0_f32; // one notch, already sign-adjusted
            let expected_rows = wheel_row_delta(dy, rows_per_notch);

            let mut delivered = 0.0_f32;
            let mut applied_rows = 0_isize;
            for i in 1..=14 {
                let t = i as f32 / 14.0;
                let eased = ease_out_cubic(t);
                let target = wheel_momentum_quantized_target(dy, eased);
                let tick_delta = target - delivered;
                delivered = target;
                if tick_delta != 0.0 {
                    applied_rows += wheel_row_delta(tick_delta, rows_per_notch);
                }
            }

            assert_eq!(
                applied_rows, expected_rows,
                "rows_per_notch={rows_per_notch}: expected {expected_rows}, got {applied_rows}"
            );
        }
    }

    // ── Key translation ───────────────────────────────────────────

    #[test]
    fn keycode_navigation_to_named() {
        for (code, expected) in &[
            (0x35_u16, NamedKey::Escape),
            (0x30, NamedKey::Tab),
            (0x24, NamedKey::Enter),
            (0x4C, NamedKey::Enter), // keypad enter
            (0x33, NamedKey::Backspace),
            (0x75, NamedKey::Delete),
            (0x7B, NamedKey::Left),
            (0x7C, NamedKey::Right),
            (0x7D, NamedKey::Down),
            (0x7E, NamedKey::Up),
            (0x74, NamedKey::PageUp),
            (0x79, NamedKey::PageDown),
            (0x73, NamedKey::Home),
            (0x77, NamedKey::End),
            (0x39, NamedKey::CapsLock),
            (0x72, NamedKey::Insert),
        ] {
            assert_eq!(
                ns_keycode_to_named_key(*code),
                Some(*expected),
                "for 0x{code:X}",
            );
        }
    }

    #[test]
    fn keycode_function_keys() {
        for (code, n) in &[
            (0x7A_u16, 1_u8),
            (0x78, 2),
            (0x63, 3),
            (0x76, 4),
            (0x60, 5),
            (0x61, 6),
            (0x62, 7),
            (0x64, 8),
            (0x65, 9),
            (0x6D, 10),
            (0x67, 11),
            (0x6F, 12),
            (0x5A, 20),
        ] {
            assert_eq!(
                ns_keycode_to_named_key(*code),
                Some(NamedKey::F(*n)),
                "for 0x{code:X}",
            );
        }
    }

    #[test]
    fn keycode_unknown_returns_none() {
        // 0x0A is no valid keycode mapping we expose.
        assert_eq!(ns_keycode_to_named_key(0x0A), None);
        assert_eq!(ns_keycode_to_named_key(0xFFFF), None);
    }

    #[test]
    fn key_event_printable_via_characters() {
        // `a` keycode (0x00) — keycode lookup fails, characters wins.
        let ev = ns_key_to_uievent(Some("a"), 0x00, 0, false);
        match ev {
            Some(UiEvent::KeyPressed {
                key: Key::Char(c),
                modifiers,
                repeat,
            }) => {
                assert_eq!(c, 'a');
                assert!(!modifiers.shift && !modifiers.cmd);
                assert!(!repeat);
            }
            other => panic!("expected KeyPressed(Char), got {other:?}"),
        }
    }

    #[test]
    fn shift_tab_translates_to_backtab() {
        // AppKit reports Shift+Tab as Tab keycode (0x30) + shift flag,
        // not a separate back-tab keycode. The translator promotes it
        // to NamedKey::BackTab so SidebarSystem / FocusRing see the
        // same variant as the GTK + TUI translators emit.
        let ev = ns_key_to_uievent(Some("\u{0019}"), 0x30, NS_FLAG_SHIFT, false);
        match ev {
            Some(UiEvent::KeyPressed {
                key: Key::Named(NamedKey::BackTab),
                modifiers,
                ..
            }) => {
                assert!(modifiers.shift, "shift modifier should still be set");
            }
            other => panic!("expected KeyPressed(Named(BackTab)), got {other:?}"),
        }
    }

    #[test]
    fn plain_tab_unchanged() {
        // Sanity: plain Tab without shift stays Tab.
        let ev = ns_key_to_uievent(Some("\t"), 0x30, 0, false);
        match ev {
            Some(UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Tab),
                modifiers,
                ..
            }) => {
                assert!(!modifiers.shift);
            }
            other => panic!("expected KeyPressed(Named(Tab)), got {other:?}"),
        }
    }

    #[test]
    fn key_event_arrow_via_keycode() {
        // Left arrow + Shift — characters would be a private-use Unicode
        // glyph, but keycode lookup should win first.
        let ev = ns_key_to_uievent(Some("\u{F702}"), 0x7B, NS_FLAG_SHIFT, false);
        match ev {
            Some(UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Left),
                modifiers,
                ..
            }) => {
                assert!(modifiers.shift);
            }
            other => panic!("expected KeyPressed(Named(Left)), got {other:?}"),
        }
    }

    #[test]
    fn key_event_ctrl_letter_recovers_base() {
        // Ctrl+C: characters is "\x03" (control), keycode is 0x08
        // (the 'c' key — not in our named-key table).
        let ev = ns_key_to_uievent(Some("\u{0003}"), 0x08, NS_FLAG_CONTROL, false);
        match ev {
            Some(UiEvent::KeyPressed {
                key: Key::Char(c),
                modifiers,
                ..
            }) => {
                assert_eq!(c, 'c');
                assert!(modifiers.ctrl);
            }
            other => panic!("expected KeyPressed(Char('c'), ctrl), got {other:?}"),
        }
    }

    #[test]
    fn key_event_empty_characters_returns_none() {
        // No characters, unknown keycode — nothing to translate.
        let ev = ns_key_to_uievent(None, 0x00, 0, false);
        assert!(ev.is_none());
    }

    #[test]
    fn key_event_repeat_flag_passes_through() {
        let ev = ns_key_to_uievent(Some("k"), 0x00, 0, true);
        match ev {
            Some(UiEvent::KeyPressed { repeat, .. }) => assert!(repeat),
            _ => panic!(),
        }
    }

    // ── Window event translators ──────────────────────────────────

    #[test]
    fn resize_translation() {
        let ev = ns_resize_to_uievent(1920.0, 1080.0, 2.0);
        match ev {
            UiEvent::WindowResized { viewport } => {
                assert_eq!(viewport.width, 1920.0);
                assert_eq!(viewport.height, 1080.0);
                assert_eq!(viewport.scale, 2.0);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn focus_translation() {
        assert_eq!(ns_focus_to_uievent(true), UiEvent::WindowFocused(true));
        assert_eq!(ns_focus_to_uievent(false), UiEvent::WindowFocused(false));
    }

    /// Issue #834: before this issue nothing in `src/macos` constructed
    /// `UiEvent::FilesDropped` — `grep -rn FilesDropped quadraui/src/macos`
    /// had zero hits outside this new function and its test.
    #[test]
    fn files_dropped_translation() {
        let paths = vec![
            std::path::PathBuf::from("/tmp/a.txt"),
            std::path::PathBuf::from("/tmp/b.txt"),
        ];
        let ev = ns_files_dropped(paths.clone(), 12.0, 34.0);
        match ev {
            UiEvent::FilesDropped {
                paths: got,
                position,
            } => {
                assert_eq!(got, paths);
                assert_eq!((position.x, position.y), (12.0, 34.0));
            }
            other => panic!("expected UiEvent::FilesDropped, got {other:?}"),
        }
    }
}
