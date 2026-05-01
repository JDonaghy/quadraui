//! GDK → `quadraui::UiEvent` translation.
//!
//! Phase B.5 Stage 4. This module is the boundary between GTK's
//! native event types (`gdk::Key`, `gdk::ModifierType`,
//! `GestureClick` button presses, drawing-area resize signals) and
//! quadraui's backend-agnostic [`UiEvent`] enum.
//!
//! GTK is callback-driven: signals fire from inside the GTK main
//! loop via Relm4 widget templates. The architectural pattern (B.5
//! option A — the event-queue adapter) is:
//!
//! 1. Each signal callback closure captures
//!    `events_handle: Rc<RefCell<VecDeque<UiEvent>>>` cloned from
//!    `GtkBackend::events_handle()`.
//! 2. Inside the callback, translate the native event to a `UiEvent`
//!    via the helpers in this module.
//! 3. `events_handle.borrow_mut().push_back(ui_event)`.
//! 4. The driver code (Stage 5+) drains the queue via
//!    `Backend::wait_events` / `poll_events`.
//!
//! Stage 4 ships the translation helpers + unit tests; Stage 5 wires
//! actual signal callbacks to use them. The functions are
//! standalone (no Rc/RefCell), so they're trivially testable.

use gtk4::gdk;

use crate::{Key, Modifiers, MouseButton, NamedKey, Point, UiEvent};

/// Translate a GDK key event from `EventControllerKey::connect_key_pressed`
/// into a [`UiEvent::KeyPressed`]. Returns `None` for keysyms that
/// don't map to a `Key` value (modifier-only presses, dead keys, etc.).
///
/// `repeat` should be the value the controller's repeat flag reports —
/// today GTK doesn't surface a stable repeat flag through the
/// gtk4-rs binding, so callers pass `false`. The trait honours the
/// field but vimcode's engine doesn't currently distinguish first-press
/// from repeat at this layer.
pub fn gdk_key_to_uievent(
    key: gdk::Key,
    modifiers: gdk::ModifierType,
    repeat: bool,
) -> Option<UiEvent> {
    let key = gdk_key_to_quadraui_key(key)?;
    let modifiers = gdk_modifiers_to_quadraui(modifiers);
    Some(UiEvent::KeyPressed {
        key,
        modifiers,
        repeat,
    })
}

/// Translate a GDK button press from `GestureClick::connect_pressed`
/// into a [`UiEvent::MouseDown`]. The button index follows GTK's
/// convention (1 = primary / left, 2 = middle, 3 = secondary / right);
/// `n_press` is GTK's click count (used by callers to distinguish
/// double-click via `UiEvent::DoubleClick` if desired). The translator
/// here always emits `MouseDown` — caller handles double-click
/// detection if needed.
pub fn gdk_button_to_mouse_down(
    button: u32,
    x: f64,
    y: f64,
    modifiers: gdk::ModifierType,
) -> UiEvent {
    UiEvent::MouseDown {
        widget: None,
        button: gdk_button_to_quadraui(button),
        position: Point::new(x as f32, y as f32),
        modifiers: gdk_modifiers_to_quadraui(modifiers),
    }
}

/// Translate a GDK button release into [`UiEvent::MouseUp`].
pub fn gdk_button_to_mouse_up(button: u32, x: f64, y: f64) -> UiEvent {
    UiEvent::MouseUp {
        widget: None,
        button: gdk_button_to_quadraui(button),
        position: Point::new(x as f32, y as f32),
    }
}

/// Translate a GDK motion event into [`UiEvent::MouseMoved`]. GTK
/// doesn't surface which buttons are held in the basic motion
/// signal — callers tracking drag state pass an explicit `buttons`
/// mask (today: filled by `GestureDrag` consumers from gesture state).
pub fn gdk_motion_to_uievent(x: f64, y: f64, buttons: crate::ButtonMask) -> UiEvent {
    UiEvent::MouseMoved {
        position: Point::new(x as f32, y as f32),
        buttons,
    }
}

/// Translate a GDK scroll event into [`UiEvent::Scroll`]. `dx`/`dy`
/// are GTK's `EventControllerScroll` deltas (positive `dy` = down).
/// We negate `dy` for `UiEvent::Scroll`'s convention (positive y =
/// up toward the top of content) — same as the TUI translator does
/// for crossterm scroll events.
pub fn gdk_scroll_to_uievent(dx: f64, dy: f64, x: f64, y: f64) -> UiEvent {
    UiEvent::Scroll {
        widget: None,
        delta: crate::ScrollDelta::new(dx as f32, -dy as f32),
        position: Point::new(x as f32, y as f32),
    }
}

/// Translate a `GtkDrawingArea` resize into [`UiEvent::WindowResized`].
/// `scale` is the surface scale factor (HiDPI multiplier) — typically
/// `widget.scale_factor() as f32`.
///
/// Currently unused — GTK doesn't surface resize through this queue
/// because `Backend::begin_frame(viewport)` already updates the
/// viewport from the active DrawingArea each frame. Kept here as a
/// reference translator for the day a non-DrawingArea-driven resize
/// path needs to push into the queue.
#[allow(dead_code)]
pub fn gdk_resize_to_uievent(width: i32, height: i32, scale: f32) -> UiEvent {
    UiEvent::WindowResized {
        viewport: crate::Viewport::new(width as f32, height as f32, scale),
    }
}

/// Translate `gdk::ModifierType` to `quadraui::Modifiers`. Maps the
/// four standard modifiers — Ctrl, Shift, Alt, Super (Cmd on macOS;
/// Win/Meta key on X11). The other GDK bits (Lock, Hyper, Mod1–5)
/// don't have quadraui equivalents and are dropped.
pub fn gdk_modifiers_to_quadraui(m: gdk::ModifierType) -> Modifiers {
    Modifiers {
        shift: m.contains(gdk::ModifierType::SHIFT_MASK),
        ctrl: m.contains(gdk::ModifierType::CONTROL_MASK),
        alt: m.contains(gdk::ModifierType::ALT_MASK),
        cmd: m.contains(gdk::ModifierType::SUPER_MASK) || m.contains(gdk::ModifierType::META_MASK),
    }
}

/// Translate a `gdk::Key` keysym to a `quadraui::Key`. Unicode
/// keysyms map to `Key::Char`; named non-printables map to
/// `Key::Named`. Returns `None` for keysyms with no quadraui
/// counterpart (modifier keys, function keys above F24, dead keys,
/// etc.).
pub fn gdk_key_to_quadraui_key(key: gdk::Key) -> Option<Key> {
    // First pass: printable Unicode characters.
    if let Some(c) = key.to_unicode() {
        if !c.is_control() {
            return Some(Key::Char(c));
        }
    }
    // Second pass: named non-printable keys. Match on the keysym
    // name string for simplicity — gdk::Key constants would also
    // work but the name match keeps the translator readable.
    let name = key.name()?;
    let named = gdk_keyname_to_named_key(&name)?;
    Some(Key::Named(named))
}

/// Map a GDK keysym name to a `quadraui::NamedKey`. Mirrors the
/// `crossterm_keycode_to_key` shape in `tui_main/events.rs`.
pub fn gdk_keyname_to_named_key(name: &str) -> Option<NamedKey> {
    let named = match name {
        "Escape" => NamedKey::Escape,
        "Tab" => NamedKey::Tab,
        "ISO_Left_Tab" => NamedKey::BackTab,
        "Return" | "KP_Enter" => NamedKey::Enter,
        "BackSpace" => NamedKey::Backspace,
        "Delete" | "KP_Delete" => NamedKey::Delete,
        "Insert" | "KP_Insert" => NamedKey::Insert,
        "Home" | "KP_Home" => NamedKey::Home,
        "End" | "KP_End" => NamedKey::End,
        "Page_Up" | "KP_Page_Up" => NamedKey::PageUp,
        "Page_Down" | "KP_Page_Down" => NamedKey::PageDown,
        "Up" | "KP_Up" => NamedKey::Up,
        "Down" | "KP_Down" => NamedKey::Down,
        "Left" | "KP_Left" => NamedKey::Left,
        "Right" | "KP_Right" => NamedKey::Right,
        "Caps_Lock" => NamedKey::CapsLock,
        "Num_Lock" => NamedKey::NumLock,
        "Scroll_Lock" => NamedKey::ScrollLock,
        "Menu" => NamedKey::Menu,
        s if s.starts_with('F') && s[1..].chars().all(|c| c.is_ascii_digit()) => {
            let n: u8 = s[1..].parse().ok()?;
            if (1..=24).contains(&n) {
                NamedKey::F(n)
            } else {
                return None;
            }
        }
        _ => return None,
    };
    Some(named)
}

/// Map a GDK button index to a `quadraui::MouseButton`.
/// GTK convention: 1 = primary (left), 2 = middle, 3 = secondary
/// (right), 8 / 9 = back / forward (X1 / X2 on most mice).
pub fn gdk_button_to_quadraui(button: u32) -> MouseButton {
    match button {
        1 => MouseButton::Left,
        2 => MouseButton::Middle,
        3 => MouseButton::Right,
        8 => MouseButton::X1,
        9 => MouseButton::X2,
        n => MouseButton::Other(n.min(255) as u8),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifiers_translate() {
        let m = gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK;
        let q = gdk_modifiers_to_quadraui(m);
        assert!(q.ctrl);
        assert!(q.shift);
        assert!(!q.alt);
        assert!(!q.cmd);
    }

    #[test]
    fn alt_modifier_translates() {
        let q = gdk_modifiers_to_quadraui(gdk::ModifierType::ALT_MASK);
        assert!(q.alt);
        assert!(!q.ctrl);
    }

    #[test]
    fn super_or_meta_maps_to_cmd() {
        assert!(gdk_modifiers_to_quadraui(gdk::ModifierType::SUPER_MASK).cmd);
        assert!(gdk_modifiers_to_quadraui(gdk::ModifierType::META_MASK).cmd);
    }

    #[test]
    fn keyname_named_keys() {
        for (name, expected) in &[
            ("Escape", NamedKey::Escape),
            ("Tab", NamedKey::Tab),
            ("ISO_Left_Tab", NamedKey::BackTab),
            ("Return", NamedKey::Enter),
            ("KP_Enter", NamedKey::Enter),
            ("BackSpace", NamedKey::Backspace),
            ("Delete", NamedKey::Delete),
            ("Up", NamedKey::Up),
            ("Down", NamedKey::Down),
            ("Left", NamedKey::Left),
            ("Right", NamedKey::Right),
            ("Page_Up", NamedKey::PageUp),
            ("Page_Down", NamedKey::PageDown),
            ("Home", NamedKey::Home),
            ("End", NamedKey::End),
            ("F1", NamedKey::F(1)),
            ("F12", NamedKey::F(12)),
        ] {
            assert_eq!(
                gdk_keyname_to_named_key(name),
                Some(*expected),
                "for {name}"
            );
        }
    }

    #[test]
    fn keyname_function_out_of_range() {
        assert_eq!(gdk_keyname_to_named_key("F0"), None);
        assert_eq!(gdk_keyname_to_named_key("F25"), None);
        assert_eq!(gdk_keyname_to_named_key("F99"), None);
    }

    #[test]
    fn keyname_unknown() {
        assert_eq!(gdk_keyname_to_named_key("Unknown_Foo"), None);
        assert_eq!(gdk_keyname_to_named_key(""), None);
    }

    #[test]
    fn button_translates() {
        assert_eq!(gdk_button_to_quadraui(1), MouseButton::Left);
        assert_eq!(gdk_button_to_quadraui(2), MouseButton::Middle);
        assert_eq!(gdk_button_to_quadraui(3), MouseButton::Right);
        assert_eq!(gdk_button_to_quadraui(8), MouseButton::X1);
        assert_eq!(gdk_button_to_quadraui(9), MouseButton::X2);
        assert_eq!(gdk_button_to_quadraui(7), MouseButton::Other(7));
    }

    #[test]
    fn mouse_down_translation() {
        let ev = gdk_button_to_mouse_down(1, 50.0, 100.0, gdk::ModifierType::CONTROL_MASK);
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
    fn scroll_negates_dy_for_quadraui_convention() {
        // GTK reports positive dy = scroll down. quadraui's
        // `Scroll.delta.y` follows the convention positive y = up.
        let ev = gdk_scroll_to_uievent(0.0, 1.0, 10.0, 20.0);
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
    fn resize_translation() {
        let ev = gdk_resize_to_uievent(1920, 1080, 2.0);
        match ev {
            UiEvent::WindowResized { viewport } => {
                assert_eq!(viewport.width, 1920.0);
                assert_eq!(viewport.height, 1080.0);
                assert_eq!(viewport.scale, 2.0);
            }
            _ => panic!(),
        }
    }
}
