//! Win32 message → `quadraui::UiEvent` translation.
//!
//! Issue #20. Mirrors the shape of [`crate::gtk::events`] and
//! [`crate::macos::events`]: pure free functions taking primitive input
//! types (already-decoded ints, floats, bools) so unit tests construct
//! synthetic inputs without linking WinAPI. `super::run`'s `wndproc`
//! extracts the relevant fields from each `WPARAM`/`LPARAM` — using
//! [`super::msg`]'s decoders for the arithmetic — and calls into these
//! helpers.
//!
//! # Scope vs #19
//!
//! `WM_SIZE` → [`UiEvent::WindowResized`], `WM_DPICHANGED` →
//! [`UiEvent::DpiChanged`], and `WM_CLOSE` → [`UiEvent::WindowClose`]
//! already landed in #19 (`super::msg`'s decoders, dispatched directly
//! from `super::run`'s `wndproc`) — the window-lifecycle events the
//! message loop itself needs to stay alive. This module covers the rest
//! of the table: mouse buttons/motion/wheel, keyboard, and focus.
//!
//! # Known gaps (follow-up, not this issue's table)
//!
//! - `WM_SYSKEYDOWN`/`WM_SYSKEYUP`/`WM_SYSCHAR` (fired while Alt is held
//!   — Alt+F4, menu mnemonics) aren't dispatched. Plain `WM_KEYDOWN`/
//!   `WM_CHAR` never fire for those combinations on real Windows, so
//!   wiring this up needs an extra `wndproc` arm reusing the same
//!   translators here.
//! - `WM_*BUTTONDBLCLK` isn't dispatched, so [`UiEvent::DoubleClick`]
//!   (which GTK's translator supports via `GestureClick`'s `n_press ==
//!   2`) has no Win-GUI producer yet.
//!
//! # Coordinate convention
//!
//! Mouse positions arrive at the wndproc in **client-area device
//! pixels** (`GET_X_LPARAM`/`GET_Y_LPARAM` on the message's `lparam`,
//! decoded by [`super::msg::point_from_lparam`]). [`crate::Point`]'s
//! documented Win-GUI unit is Direct2D DIPs, matching
//! [`crate::Viewport::scale`] — so every translator here takes the
//! window's current DPI `scale` and divides device pixels by it before
//! building a `Point`. A 200% (scale 2.0) monitor's 100px client-pixel
//! click becomes DIP `x = 50.0`, the same DIP value `WinBackend`'s
//! rasterisers lay primitives out in.
//!
//! # Modifier state
//!
//! Unlike GTK's `GdkModifierType` (carried on every event) or a mouse
//! message's `wparam` (which only ever carries `MK_CONTROL`/`MK_SHIFT`,
//! never Alt or the Windows key), Win32 has no single bitmask with all
//! four modifiers. The caller reads live state via
//! `GetKeyState(VK_CONTROL)` / `VK_SHIFT` / `VK_MENU` (Alt) /
//! `VK_LWIN`+`VK_RWIN` (Win) — high bit set means "down" — for *every*
//! mouse and keyboard message alike, and passes the four booleans to
//! [`win_modifiers`]. The Windows key maps to [`Modifiers::cmd`], same
//! role as macOS's Command key and GTK's Super/Meta.

use crate::{ButtonMask, Key, Modifiers, MouseButton, NamedKey, Point, ScrollDelta, UiEvent};

use super::msg::WHEEL_DELTA;

// ─── Win32 message identifiers ──────────────────────────────────────────
//
// Stable numeric values from `<winuser.h>` — same values the `windows`
// crate's `windows::Win32::UI::WindowsAndMessaging::WM_*` constants carry.
// Defined locally (rather than imported from `windows`) so
// [`win_mouse_button_for_message`] stays reachable — and testable — off
// Windows, the same reasoning `super::msg`'s module docs give for not
// depending on real WinAPI types.

/// `WM_LBUTTONDOWN`.
pub(crate) const WM_LBUTTONDOWN: u32 = 0x0201;
/// `WM_LBUTTONUP`.
pub(crate) const WM_LBUTTONUP: u32 = 0x0202;
/// `WM_RBUTTONDOWN`.
pub(crate) const WM_RBUTTONDOWN: u32 = 0x0204;
/// `WM_RBUTTONUP`.
pub(crate) const WM_RBUTTONUP: u32 = 0x0205;
/// `WM_MBUTTONDOWN`.
pub(crate) const WM_MBUTTONDOWN: u32 = 0x0207;
/// `WM_MBUTTONUP`.
pub(crate) const WM_MBUTTONUP: u32 = 0x0208;
/// `WM_XBUTTONDOWN`.
pub(crate) const WM_XBUTTONDOWN: u32 = 0x020B;
/// `WM_XBUTTONUP`.
pub(crate) const WM_XBUTTONUP: u32 = 0x020C;

// ─── Modifiers ───────────────────────────────────────────────────────────

/// Build [`Modifiers`] from four `GetKeyState`-derived booleans — see this
/// module's docs for why Win32 needs the caller to assemble these rather
/// than decoding one bitmask. `win` is Windows' Super-key equivalent and
/// maps to [`Modifiers::cmd`], mirroring GTK's Super/Meta → `cmd` mapping.
pub fn win_modifiers(ctrl: bool, shift: bool, alt: bool, win: bool) -> Modifiers {
    Modifiers {
        ctrl,
        shift,
        alt,
        cmd: win,
    }
}

// ─── Mouse buttons ───────────────────────────────────────────────────────

/// Map a mouse `WM_*BUTTON{DOWN,UP}` message id (plus its `wparam`, needed
/// only for the `WM_XBUTTON*` pair) to a [`MouseButton`]. Returns `None`
/// for a message id this table doesn't cover.
///
/// `XBUTTON1`/`XBUTTON2` live in `wparam`'s high word
/// (`GET_XBUTTON_WPARAM`) — 1 = X1 (back), 2 = X2 (forward), same
/// hardware convention GTK's `wire_da_events`/`gdk_button_to_quadraui`
/// documents for buttons 8/9.
pub fn win_mouse_button_for_message(msg: u32, wparam: usize) -> Option<MouseButton> {
    Some(match msg {
        WM_LBUTTONDOWN | WM_LBUTTONUP => MouseButton::Left,
        WM_RBUTTONDOWN | WM_RBUTTONUP => MouseButton::Right,
        WM_MBUTTONDOWN | WM_MBUTTONUP => MouseButton::Middle,
        WM_XBUTTONDOWN | WM_XBUTTONUP => match (wparam >> 16) & 0xFFFF {
            1 => MouseButton::X1,
            2 => MouseButton::X2,
            n => MouseButton::Other(n.min(255) as u8),
        },
        _ => return None,
    })
}

/// Convert a client-area device-pixel coordinate to a DIP, dividing by
/// the window's current DPI `scale` (see this module's docs). Guards the
/// same way [`super::msg::dpi_ratio`] does — an (in-practice-impossible)
/// zero scale degrades to unscaled rather than producing `inf`/`NaN`.
fn to_dip(px: i16, scale: f32) -> f32 {
    if scale == 0.0 {
        px as f32
    } else {
        px as f32 / scale
    }
}

/// Translate a `WM_LBUTTONDOWN`/`WM_RBUTTONDOWN`/… into
/// [`UiEvent::MouseDown`]. `x`, `y` are client-area device pixels
/// (already decoded via [`super::msg::point_from_lparam`]); `scale` is
/// the window's current DPI ratio.
pub fn win_button_down(
    button: MouseButton,
    x: i16,
    y: i16,
    scale: f32,
    modifiers: Modifiers,
) -> UiEvent {
    UiEvent::MouseDown {
        widget: None,
        button,
        position: Point::new(to_dip(x, scale), to_dip(y, scale)),
        modifiers,
    }
}

/// Translate a `WM_LBUTTONUP`/`WM_RBUTTONUP`/… into [`UiEvent::MouseUp`].
pub fn win_button_up(button: MouseButton, x: i16, y: i16, scale: f32) -> UiEvent {
    UiEvent::MouseUp {
        widget: None,
        button,
        position: Point::new(to_dip(x, scale), to_dip(y, scale)),
    }
}

/// Translate `WM_MOUSEMOVE` into [`UiEvent::MouseMoved`]. `buttons` is
/// derived by the caller from `wparam`'s `MK_LBUTTON`/`MK_MBUTTON`/
/// `MK_RBUTTON` bits (the one case Win32 *does* carry button-held state
/// directly on the message, unlike Alt/Win — see this module's docs).
pub fn win_mouse_moved(x: i16, y: i16, scale: f32, buttons: ButtonMask) -> UiEvent {
    UiEvent::MouseMoved {
        position: Point::new(to_dip(x, scale), to_dip(y, scale)),
        buttons,
    }
}

// ─── Scroll wheel ────────────────────────────────────────────────────────

/// Translate `WM_MOUSEWHEEL`/`WM_MOUSEHWHEEL` into [`UiEvent::Scroll`].
/// `raw_delta` is [`super::msg::wheel_delta_from_wparam`]'s output,
/// normalised here by [`WHEEL_DELTA`] (120) into quadraui's small-
/// magnitude scroll unit — one full notch is `1.0`, matching the TUI
/// backend's crossterm `ScrollUp`/`ScrollDown` translation.
///
/// `x`, `y` must already be client-area device pixels — `WM_MOUSEWHEEL`'s
/// `lparam` carries **screen** coordinates (unlike every other mouse
/// message), so the caller converts via `ScreenToClient` before calling.
///
/// No sign flip: Win32's positive delta (wheel rotated forward/away from
/// the user, or horizontally to the right) already matches quadraui's
/// "positive `y` = up, positive `x` = right" convention — unlike GTK/
/// macOS, whose native deltas run the opposite way and are negated in
/// their translators.
pub fn win_wheel_to_uievent(
    raw_delta: i16,
    x: i16,
    y: i16,
    scale: f32,
    horizontal: bool,
) -> UiEvent {
    let notches = raw_delta as f32 / WHEEL_DELTA;
    let delta = if horizontal {
        ScrollDelta::new(notches, 0.0)
    } else {
        ScrollDelta::new(0.0, notches)
    };
    UiEvent::Scroll {
        widget: None,
        delta,
        position: Point::new(to_dip(x, scale), to_dip(y, scale)),
    }
}

// ─── Keyboard ────────────────────────────────────────────────────────────

/// Map a Win32 virtual-key code (`VK_*` from `<winuser.h>`) to a
/// [`NamedKey`]. Returns `None` for keys with no quadraui counterpart —
/// notably every printable letter/digit/punctuation `VK_*`, which
/// `WM_CHAR` (via [`wm_char_to_uievent`]) delivers instead, already
/// resolved through the active keyboard layout.
pub fn vk_to_named_key(vk: u32) -> Option<NamedKey> {
    Some(match vk {
        0x1B => NamedKey::Escape,                          // VK_ESCAPE
        0x09 => NamedKey::Tab,                             // VK_TAB
        0x0D => NamedKey::Enter,                           // VK_RETURN
        0x08 => NamedKey::Backspace,                       // VK_BACK
        0x2E => NamedKey::Delete,                          // VK_DELETE
        0x2D => NamedKey::Insert,                          // VK_INSERT
        0x24 => NamedKey::Home,                            // VK_HOME
        0x23 => NamedKey::End,                             // VK_END
        0x21 => NamedKey::PageUp,                          // VK_PRIOR
        0x22 => NamedKey::PageDown,                        // VK_NEXT
        0x25 => NamedKey::Left,                            // VK_LEFT
        0x26 => NamedKey::Up,                              // VK_UP
        0x27 => NamedKey::Right,                           // VK_RIGHT
        0x28 => NamedKey::Down,                            // VK_DOWN
        0x14 => NamedKey::CapsLock,                        // VK_CAPITAL
        0x90 => NamedKey::NumLock,                         // VK_NUMLOCK
        0x91 => NamedKey::ScrollLock,                      // VK_SCROLL
        0x5D => NamedKey::Menu,                            // VK_APPS
        0x70..=0x87 => NamedKey::F((vk - 0x70 + 1) as u8), // VK_F1..VK_F24
        _ => return None,
    })
}

/// Translate `WM_KEYDOWN`/`WM_KEYUP` into [`UiEvent::KeyPressed`] for the
/// non-printable keys [`vk_to_named_key`] maps. Returns `None` for every
/// other virtual-key code (letters, digits, punctuation) — those arrive
/// as [`UiEvent::KeyPressed`] via [`wm_char_to_uievent`] instead, once
/// Windows has resolved them through the keyboard layout.
///
/// Windows has no separate "back-tab" virtual key — Shift+Tab still
/// reports `VK_TAB` with the shift modifier set, same as GTK's
/// `ISO_Left_Tab` case and macOS's Tab-keycode-plus-shift case. Promoted
/// to [`NamedKey::BackTab`] here so backend-neutral consumers see the
/// same variant on every backend.
pub fn wm_keydown_to_uievent(vk: u32, modifiers: Modifiers, repeat: bool) -> Option<UiEvent> {
    let named = vk_to_named_key(vk)?;
    let named = if matches!(named, NamedKey::Tab) && modifiers.shift {
        NamedKey::BackTab
    } else {
        named
    };
    Some(UiEvent::KeyPressed {
        key: Key::Named(named),
        modifiers,
        repeat,
    })
}

/// Translate `WM_CHAR`'s already-decoded character into
/// [`UiEvent::KeyPressed`] for printable text. `c` is the UTF-16 code
/// unit Windows delivered, already resolved through the active keyboard
/// layout — the caller decodes surrogate pairs if it cares about
/// characters outside the BMP; this translator handles one `char`.
///
/// Ctrl+letter produces a C0 control character (`\x01`..`\x1A` for
/// Ctrl+A..Ctrl+Z), the same as every other backend's raw key-event
/// path — recovered back to the base letter so apps see
/// `Key::Char('a')` with `modifiers.ctrl == true` rather than an
/// unprintable control code. Mirrors the GTK/macOS translators'
/// identical recovery step.
///
/// Returns `None` for every other control character — notably `\r`/`\t`/
/// `\x1B`/`\x08` (Enter/Tab/Escape/Backspace all echo their C0 code
/// through `WM_CHAR` too, and Backspace/Enter/Tab's codes (8/13/9) sit
/// inside the same `1..=26` band as real Ctrl+letters). Windows gives no
/// way to tell "Enter" from "Ctrl+M" from the character code alone, so
/// this only takes the Ctrl+letter interpretation when `modifiers.ctrl`
/// is actually set (read via `GetKeyState`, same as every other
/// translator in this module) — plain Enter/Tab/Backspace/Escape arrive
/// with `ctrl == false` and are dropped here, already covered by
/// [`wm_keydown_to_uievent`]'s `VK_RETURN`/`VK_TAB`/`VK_BACK`/`VK_ESCAPE`
/// cases so this avoids double-firing them.
pub fn wm_char_to_uievent(c: char, modifiers: Modifiers, repeat: bool) -> Option<UiEvent> {
    if c.is_control() {
        if modifiers.ctrl && (1..=26).contains(&(c as u32)) {
            let base = (b'a' + (c as u8 - 1)) as char;
            return Some(UiEvent::KeyPressed {
                key: Key::Char(base),
                modifiers,
                repeat,
            });
        }
        return None;
    }
    Some(UiEvent::KeyPressed {
        key: Key::Char(c),
        modifiers,
        repeat,
    })
}

// ─── Focus ───────────────────────────────────────────────────────────────

/// Translate `WM_SETFOCUS`/`WM_KILLFOCUS` into [`UiEvent::WindowFocused`].
pub fn win_focus_to_uievent(focused: bool) -> UiEvent {
    UiEvent::WindowFocused(focused)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Modifiers ───────────────────────────────────────────────────

    #[test]
    fn modifiers_all_false() {
        let m = win_modifiers(false, false, false, false);
        assert!(!m.ctrl && !m.shift && !m.alt && !m.cmd);
    }

    #[test]
    fn modifiers_ctrl_shift() {
        let m = win_modifiers(true, true, false, false);
        assert!(m.ctrl);
        assert!(m.shift);
        assert!(!m.alt);
        assert!(!m.cmd);
    }

    #[test]
    fn modifiers_win_key_maps_to_cmd() {
        let m = win_modifiers(false, false, false, true);
        assert!(m.cmd);
        assert!(!m.ctrl && !m.shift && !m.alt);
    }

    #[test]
    fn modifiers_all_true() {
        let m = win_modifiers(true, true, true, true);
        assert!(m.ctrl && m.shift && m.alt && m.cmd);
    }

    // ── Button translation ──────────────────────────────────────────

    #[test]
    fn button_for_message_left_and_right() {
        assert_eq!(
            win_mouse_button_for_message(WM_LBUTTONDOWN, 0),
            Some(MouseButton::Left)
        );
        assert_eq!(
            win_mouse_button_for_message(WM_LBUTTONUP, 0),
            Some(MouseButton::Left)
        );
        assert_eq!(
            win_mouse_button_for_message(WM_RBUTTONDOWN, 0),
            Some(MouseButton::Right)
        );
        assert_eq!(
            win_mouse_button_for_message(WM_RBUTTONUP, 0),
            Some(MouseButton::Right)
        );
    }

    #[test]
    fn button_for_message_middle() {
        assert_eq!(
            win_mouse_button_for_message(WM_MBUTTONDOWN, 0),
            Some(MouseButton::Middle)
        );
        assert_eq!(
            win_mouse_button_for_message(WM_MBUTTONUP, 0),
            Some(MouseButton::Middle)
        );
    }

    #[test]
    fn button_for_message_xbutton_reads_high_word() {
        let xbutton1 = 1usize << 16;
        let xbutton2 = 2usize << 16;
        assert_eq!(
            win_mouse_button_for_message(WM_XBUTTONDOWN, xbutton1),
            Some(MouseButton::X1)
        );
        assert_eq!(
            win_mouse_button_for_message(WM_XBUTTONUP, xbutton2),
            Some(MouseButton::X2)
        );
    }

    #[test]
    fn button_for_message_unknown_returns_none() {
        assert_eq!(win_mouse_button_for_message(0x9999, 0), None);
    }

    #[test]
    fn mouse_down_carries_button_position_and_modifiers() {
        let ev = win_button_down(
            MouseButton::Left,
            50,
            100,
            1.0,
            win_modifiers(true, false, false, false),
        );
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
            }
            other => panic!("expected MouseDown, got {other:?}"),
        }
    }

    #[test]
    fn mouse_up_translation() {
        let ev = win_button_up(MouseButton::Right, 200, 300, 1.0);
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
        let ev = win_mouse_moved(10, 20, 1.0, buttons);
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

    // ── DIP scaling ─────────────────────────────────────────────────

    #[test]
    fn mouse_position_scales_device_pixels_to_dips() {
        // 200% scale: 100 device px client click -> 50.0 DIP.
        let ev = win_button_down(MouseButton::Left, 100, 100, 2.0, Modifiers::default());
        match ev {
            UiEvent::MouseDown { position, .. } => {
                assert_eq!(position.x, 50.0);
                assert_eq!(position.y, 50.0);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn mouse_position_at_100_percent_scale_is_unchanged() {
        let ev = win_button_up(MouseButton::Left, 77, 33, 1.0);
        match ev {
            UiEvent::MouseUp { position, .. } => {
                assert_eq!(position.x, 77.0);
                assert_eq!(position.y, 33.0);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn mouse_position_never_divides_by_zero_scale() {
        let ev = win_mouse_moved(10, 20, 0.0, ButtonMask::default());
        match ev {
            UiEvent::MouseMoved { position, .. } => {
                assert!(position.x.is_finite());
                assert!(position.y.is_finite());
                assert_eq!(position.x, 10.0);
                assert_eq!(position.y, 20.0);
            }
            _ => panic!(),
        }
    }

    // ── Scroll wheel ────────────────────────────────────────────────

    #[test]
    fn wheel_forward_notch_is_positive_y_no_negation() {
        let ev = win_wheel_to_uievent(120, 10, 20, 1.0, false);
        match ev {
            UiEvent::Scroll {
                delta, position, ..
            } => {
                assert_eq!(delta.y, 1.0);
                assert_eq!(delta.x, 0.0);
                assert_eq!(position.x, 10.0);
                assert_eq!(position.y, 20.0);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn wheel_backward_notch_is_negative_y() {
        let ev = win_wheel_to_uievent(-120, 0, 0, 1.0, false);
        match ev {
            UiEvent::Scroll { delta, .. } => assert_eq!(delta.y, -1.0),
            _ => panic!(),
        }
    }

    #[test]
    fn horizontal_wheel_maps_to_delta_x() {
        let ev = win_wheel_to_uievent(120, 0, 0, 1.0, true);
        match ev {
            UiEvent::Scroll { delta, .. } => {
                assert_eq!(delta.x, 1.0);
                assert_eq!(delta.y, 0.0);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn wheel_fractional_notch_normalises_correctly() {
        // High-resolution wheel/trackpad delta smaller than a full notch.
        let ev = win_wheel_to_uievent(40, 0, 0, 1.0, false);
        match ev {
            UiEvent::Scroll { delta, .. } => {
                assert!((delta.y - (40.0 / 120.0)).abs() < f32::EPSILON)
            }
            _ => panic!(),
        }
    }

    // ── Keyboard: named keys ────────────────────────────────────────

    #[test]
    fn vk_navigation_and_editing_keys() {
        for (vk, expected) in &[
            (0x1B_u32, NamedKey::Escape),
            (0x09, NamedKey::Tab),
            (0x0D, NamedKey::Enter),
            (0x08, NamedKey::Backspace),
            (0x2E, NamedKey::Delete),
            (0x2D, NamedKey::Insert),
            (0x24, NamedKey::Home),
            (0x23, NamedKey::End),
            (0x21, NamedKey::PageUp),
            (0x22, NamedKey::PageDown),
            (0x25, NamedKey::Left),
            (0x26, NamedKey::Up),
            (0x27, NamedKey::Right),
            (0x28, NamedKey::Down),
            (0x14, NamedKey::CapsLock),
            (0x90, NamedKey::NumLock),
            (0x91, NamedKey::ScrollLock),
            (0x5D, NamedKey::Menu),
        ] {
            assert_eq!(vk_to_named_key(*vk), Some(*expected), "for VK 0x{vk:02X}");
        }
    }

    #[test]
    fn vk_function_keys() {
        assert_eq!(vk_to_named_key(0x70), Some(NamedKey::F(1)));
        assert_eq!(vk_to_named_key(0x7B), Some(NamedKey::F(12)));
        assert_eq!(vk_to_named_key(0x87), Some(NamedKey::F(24)));
    }

    #[test]
    fn vk_letters_and_digits_are_not_named_keys() {
        // 'A' = 0x41, '0' = 0x30 — printable, handled via WM_CHAR instead.
        assert_eq!(vk_to_named_key(0x41), None);
        assert_eq!(vk_to_named_key(0x30), None);
    }

    #[test]
    fn vk_unknown_returns_none() {
        assert_eq!(vk_to_named_key(0xFF), None);
    }

    #[test]
    fn keydown_named_key_translates() {
        let ev = wm_keydown_to_uievent(0x1B, Modifiers::default(), false);
        match ev {
            Some(UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                repeat,
                ..
            }) => assert!(!repeat),
            other => panic!("expected KeyPressed(Named(Escape)), got {other:?}"),
        }
    }

    #[test]
    fn keydown_shift_tab_promotes_to_backtab() {
        let mods = win_modifiers(false, true, false, false);
        let ev = wm_keydown_to_uievent(0x09, mods, false);
        match ev {
            Some(UiEvent::KeyPressed {
                key: Key::Named(NamedKey::BackTab),
                modifiers,
                ..
            }) => assert!(modifiers.shift),
            other => panic!("expected KeyPressed(Named(BackTab)), got {other:?}"),
        }
    }

    #[test]
    fn keydown_plain_tab_unchanged() {
        let ev = wm_keydown_to_uievent(0x09, Modifiers::default(), false);
        assert!(matches!(
            ev,
            Some(UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Tab),
                ..
            })
        ));
    }

    #[test]
    fn keydown_unmapped_vk_returns_none() {
        assert_eq!(
            wm_keydown_to_uievent(0x41, Modifiers::default(), false),
            None
        );
    }

    #[test]
    fn keydown_repeat_flag_passes_through() {
        let ev = wm_keydown_to_uievent(0x25, Modifiers::default(), true);
        match ev {
            Some(UiEvent::KeyPressed { repeat, .. }) => assert!(repeat),
            _ => panic!(),
        }
    }

    // ── Keyboard: printable chars ───────────────────────────────────

    #[test]
    fn char_printable_translates() {
        let ev = wm_char_to_uievent('a', Modifiers::default(), false);
        match ev {
            Some(UiEvent::KeyPressed {
                key: Key::Char(c),
                repeat,
                ..
            }) => {
                assert_eq!(c, 'a');
                assert!(!repeat);
            }
            other => panic!("expected KeyPressed(Char('a')), got {other:?}"),
        }
    }

    #[test]
    fn char_ctrl_letter_recovers_base_letter() {
        // Ctrl+C delivers \x03 via WM_CHAR.
        let mods = win_modifiers(true, false, false, false);
        let ev = wm_char_to_uievent('\u{0003}', mods, false);
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
    fn char_ctrl_a_through_z_all_recover() {
        for i in 1u32..=26 {
            let ctrl_char = char::from_u32(i).unwrap();
            let ev = wm_char_to_uievent(ctrl_char, win_modifiers(true, false, false, false), false);
            let expected = (b'a' + (i as u8 - 1)) as char;
            match ev {
                Some(UiEvent::KeyPressed {
                    key: Key::Char(c), ..
                }) => assert_eq!(c, expected, "for control char {i}"),
                other => panic!("expected KeyPressed(Char), got {other:?}"),
            }
        }
    }

    #[test]
    fn char_control_without_ctrl_held_returns_none() {
        // \r (Enter), \t (Tab), \x08 (Backspace) all fall in the same
        // control-code band Ctrl+M/Ctrl+I/Ctrl+H would — without
        // modifiers.ctrl set, these are the named keys' own WM_CHAR
        // echoes and are dropped so they don't double-fire alongside
        // wm_keydown_to_uievent's VK_RETURN/VK_TAB/VK_BACK cases.
        assert_eq!(wm_char_to_uievent('\r', Modifiers::default(), false), None);
        assert_eq!(wm_char_to_uievent('\t', Modifiers::default(), false), None);
        assert_eq!(
            wm_char_to_uievent('\x08', Modifiers::default(), false),
            None
        );
        // \x1B (Escape) is outside the 1..=26 band entirely either way.
        assert_eq!(
            wm_char_to_uievent('\x1B', Modifiers::default(), false),
            None
        );
    }

    #[test]
    fn char_control_with_ctrl_held_recovers_even_for_named_key_codes() {
        // Ctrl+M really does produce the same \r byte Enter does — with
        // modifiers.ctrl set, it's treated as Ctrl+M, matching what the
        // caller's GetKeyState(VK_CONTROL) read actually observed.
        let mods = win_modifiers(true, false, false, false);
        let ev = wm_char_to_uievent('\r', mods, false);
        match ev {
            Some(UiEvent::KeyPressed {
                key: Key::Char(c),
                modifiers,
                ..
            }) => {
                assert_eq!(c, 'm');
                assert!(modifiers.ctrl);
            }
            other => panic!("expected KeyPressed(Char('m'), ctrl), got {other:?}"),
        }
    }

    #[test]
    fn char_repeat_flag_passes_through() {
        let ev = wm_char_to_uievent('k', Modifiers::default(), true);
        match ev {
            Some(UiEvent::KeyPressed { repeat, .. }) => assert!(repeat),
            _ => panic!(),
        }
    }

    // ── Focus ────────────────────────────────────────────────────────

    #[test]
    fn focus_translation() {
        assert_eq!(win_focus_to_uievent(true), UiEvent::WindowFocused(true));
        assert_eq!(win_focus_to_uievent(false), UiEvent::WindowFocused(false));
    }
}
