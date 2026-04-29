//! Crossterm → `crate::UiEvent` translation.
//!
//! Phase B.4 Stage 4. This module is the boundary between crossterm's
//! native event types (`KeyEvent`, `MouseEvent`, `Event::Resize`,
//! `Event::Paste`, `Event::FocusGained`/`FocusLost`) and quadraui's
//! backend-agnostic [`UiEvent`] enum. Once the event loop migrates
//! (Stage 5), every native event reaches the engine via this layer
//! instead of being decoded inline.
//!
//! # What this does
//!
//! - **Keys** ([`crossterm_key_to_uievent`]): map `KeyCode` → `Key`,
//!   `KeyModifiers` → `Modifiers`. Press events emit
//!   `UiEvent::KeyPressed`; releases are dropped (the engine doesn't
//!   distinguish them today).
//! - **Mouse** ([`crossterm_mouse_to_uievent`]): map `MouseEventKind`
//!   to `MouseDown` / `MouseUp` / `MouseMoved` / `Scroll`. The
//!   `widget` field stays `None` — actual hit-testing happens in
//!   [`crate::dispatch`] / [`crate::dispatch::dispatch_mouse_down`].
//! - **Window resize** → `UiEvent::WindowResized { viewport }`.
//! - **Bracketed paste** → `UiEvent::ClipboardPaste(text)`.
//! - **Focus gained / lost** (kitty protocol) → `UiEvent::WindowFocused(true/false)`.
//!
//! # What this does NOT do
//!
//! - Vim-style key-name strings (`"Escape"`, `"Shift_Up"`, `"Return"`)
//!   that the engine's `handle_key` expects — those still come from
//!   the existing [`the app`] function.
//!   Stage 5 either keeps that as a separate adapter or refactors it
//!   to consume `UiEvent::KeyPressed` directly.
//! - Accelerator matching. Stage 6's [`Backend::register_accelerator`]
//!   path will compare the registered list against the translated
//!   `Key + Modifiers` and emit `UiEvent::Accelerator(id, mods)`.
//! - Hit-testing. Mouse events come out with `widget: None`; the
//!   trait-side dispatch fills that in via the modal stack and per-
//!   primitive layout hit_test.

use crate::{
    ButtonMask, Key, Modifiers, MouseButton, NamedKey, Point, ScrollDelta, UiEvent, Viewport,
};
use ratatui::crossterm::event::{
    Event as CtEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton as CtMouseButton,
    MouseEvent, MouseEventKind,
};

/// Translate one crossterm event to zero or more quadraui events.
///
/// Single events translate one-to-one (Vec of length 1); some
/// crossterm events have no quadraui equivalent and translate to
/// an empty Vec (e.g. `KeyEventKind::Release`). Returning `Vec`
/// instead of `Option` keeps the door open for future
/// composite events without a breaking change.
pub fn crossterm_to_uievents(event: CtEvent) -> Vec<UiEvent> {
    match event {
        CtEvent::Key(k) => crossterm_key_to_uievent(k).into_iter().collect(),
        CtEvent::Mouse(m) => crossterm_mouse_to_uievent(m).into_iter().collect(),
        CtEvent::Resize(w, h) => vec![UiEvent::WindowResized {
            viewport: Viewport::new(w as f32, h as f32, 1.0),
        }],
        CtEvent::Paste(text) => vec![UiEvent::ClipboardPaste(text)],
        CtEvent::FocusGained => vec![UiEvent::WindowFocused(true)],
        CtEvent::FocusLost => vec![UiEvent::WindowFocused(false)],
    }
}

/// Translate one crossterm key event. `None` means the event has no
/// quadraui counterpart (releases, repeats not surfaced as
/// `KeyPressed`, etc.).
pub fn crossterm_key_to_uievent(event: KeyEvent) -> Option<UiEvent> {
    if event.kind == KeyEventKind::Release {
        return None;
    }
    let key = crossterm_keycode_to_key(event.code)?;
    let modifiers = crossterm_modifiers_to_quadraui(event.modifiers);
    let repeat = event.kind == KeyEventKind::Repeat;
    Some(UiEvent::KeyPressed {
        key,
        modifiers,
        repeat,
    })
}

/// Translate one crossterm mouse event.
pub fn crossterm_mouse_to_uievent(event: MouseEvent) -> Option<UiEvent> {
    let position = Point::new(event.column as f32, event.row as f32);
    let modifiers = crossterm_modifiers_to_quadraui(event.modifiers);
    match event.kind {
        MouseEventKind::Down(b) => Some(UiEvent::MouseDown {
            widget: None,
            button: crossterm_mouse_button_to_quadraui(b),
            position,
            modifiers,
        }),
        MouseEventKind::Up(b) => Some(UiEvent::MouseUp {
            widget: None,
            button: crossterm_mouse_button_to_quadraui(b),
            position,
        }),
        MouseEventKind::Drag(b) => Some(UiEvent::MouseMoved {
            position,
            buttons: button_mask_with_held(b),
        }),
        MouseEventKind::Moved => Some(UiEvent::MouseMoved {
            position,
            buttons: ButtonMask::default(),
        }),
        MouseEventKind::ScrollUp => Some(UiEvent::Scroll {
            widget: None,
            delta: ScrollDelta { x: 0.0, y: -1.0 },
            position,
        }),
        MouseEventKind::ScrollDown => Some(UiEvent::Scroll {
            widget: None,
            delta: ScrollDelta { x: 0.0, y: 1.0 },
            position,
        }),
        MouseEventKind::ScrollLeft => Some(UiEvent::Scroll {
            widget: None,
            delta: ScrollDelta { x: -1.0, y: 0.0 },
            position,
        }),
        MouseEventKind::ScrollRight => Some(UiEvent::Scroll {
            widget: None,
            delta: ScrollDelta { x: 1.0, y: 0.0 },
            position,
        }),
    }
}

fn crossterm_keycode_to_key(code: KeyCode) -> Option<Key> {
    let named = match code {
        KeyCode::Char(c) => return Some(Key::Char(c)),
        KeyCode::Esc => NamedKey::Escape,
        KeyCode::Tab => NamedKey::Tab,
        KeyCode::BackTab => NamedKey::BackTab,
        KeyCode::Enter => NamedKey::Enter,
        KeyCode::Backspace => NamedKey::Backspace,
        KeyCode::Delete => NamedKey::Delete,
        KeyCode::Insert => NamedKey::Insert,
        KeyCode::Home => NamedKey::Home,
        KeyCode::End => NamedKey::End,
        KeyCode::PageUp => NamedKey::PageUp,
        KeyCode::PageDown => NamedKey::PageDown,
        KeyCode::Up => NamedKey::Up,
        KeyCode::Down => NamedKey::Down,
        KeyCode::Left => NamedKey::Left,
        KeyCode::Right => NamedKey::Right,
        KeyCode::F(n) => NamedKey::F(n),
        KeyCode::CapsLock => NamedKey::CapsLock,
        KeyCode::NumLock => NamedKey::NumLock,
        KeyCode::ScrollLock => NamedKey::ScrollLock,
        KeyCode::Menu => NamedKey::Menu,
        // Crossterm has Null, Modifier, Pause, PrintScreen, Media,
        // KeypadBegin etc. that quadraui's NamedKey doesn't yet model.
        // Drop them — the existing engine doesn't act on these either.
        _ => return None,
    };
    Some(Key::Named(named))
}

fn crossterm_modifiers_to_quadraui(m: KeyModifiers) -> Modifiers {
    Modifiers {
        shift: m.contains(KeyModifiers::SHIFT),
        ctrl: m.contains(KeyModifiers::CONTROL),
        alt: m.contains(KeyModifiers::ALT),
        cmd: m.contains(KeyModifiers::SUPER) | m.contains(KeyModifiers::META),
    }
}

fn crossterm_mouse_button_to_quadraui(b: CtMouseButton) -> MouseButton {
    match b {
        CtMouseButton::Left => MouseButton::Left,
        CtMouseButton::Right => MouseButton::Right,
        CtMouseButton::Middle => MouseButton::Middle,
    }
}

/// Synthesise a crossterm `Event` from any single `UiEvent` that has
/// a crossterm equivalent. The migrated event loop in
/// [`the runner loop`] uses this to feed `UiEvent`s back
/// into the existing match-on-`Event::*` dispatch without rewriting
/// every legacy handler. Returns `None` for `UiEvent` variants that
/// don't round-trip (`Accelerator`, `Tree(...)`, etc.).
pub fn uievent_to_crossterm(ev: UiEvent) -> Option<CtEvent> {
    match ev {
        UiEvent::KeyPressed {
            key,
            modifiers,
            repeat,
        } => synth_keyevent(&key, modifiers, repeat).map(CtEvent::Key),
        UiEvent::CharTyped(_) => None, // crossterm doesn't have a separate variant
        UiEvent::MouseDown { .. }
        | UiEvent::MouseUp { .. }
        | UiEvent::MouseMoved { .. }
        | UiEvent::Scroll { .. } => synth_mouseevent(&ev).map(CtEvent::Mouse),
        UiEvent::WindowResized { viewport } => Some(CtEvent::Resize(
            viewport.width as u16,
            viewport.height as u16,
        )),
        UiEvent::ClipboardPaste(text) => Some(CtEvent::Paste(text)),
        UiEvent::WindowFocused(true) => Some(CtEvent::FocusGained),
        UiEvent::WindowFocused(false) => Some(CtEvent::FocusLost),
        // No equivalent: WindowClose, DpiChanged, FilesDropped, primitive
        // events (Tree/List/...), Accelerator, etc.
        _ => None,
    }
}

// ─── Inverse: UiEvent → crossterm::Event ────────────────────────────────────
//
// Stage 5b's loop migration consumes `UiEvent` from `backend.wait_events`
// but the existing handlers (translate_key, handle_mouse, …) take
// crossterm types. Rather than duplicate all that decode logic, the
// migrated loop synthesises a crossterm event from each `UiEvent` and
// feeds the legacy handler unchanged. The translation is lossy in
// principle (we don't preserve `KeyEventState` or wide-mouse-button
// indices) but the existing handlers only consult the fields we
// faithfully round-trip.

/// Synthesise a crossterm `KeyEvent` from a `UiEvent::KeyPressed`'s
/// payload. Returns `None` if the `Key` doesn't have a crossterm
/// equivalent (won't happen for events we translated ourselves —
/// every variant we emit round-trips).
pub fn synth_keyevent(key: &Key, modifiers: Modifiers, repeat: bool) -> Option<KeyEvent> {
    use ratatui::crossterm::event::KeyEventState;
    let code = match key {
        Key::Char(c) => KeyCode::Char(*c),
        Key::Named(n) => match n {
            NamedKey::Escape => KeyCode::Esc,
            NamedKey::Tab => KeyCode::Tab,
            NamedKey::BackTab => KeyCode::BackTab,
            NamedKey::Enter => KeyCode::Enter,
            NamedKey::Backspace => KeyCode::Backspace,
            NamedKey::Delete => KeyCode::Delete,
            NamedKey::Insert => KeyCode::Insert,
            NamedKey::Home => KeyCode::Home,
            NamedKey::End => KeyCode::End,
            NamedKey::PageUp => KeyCode::PageUp,
            NamedKey::PageDown => KeyCode::PageDown,
            NamedKey::Up => KeyCode::Up,
            NamedKey::Down => KeyCode::Down,
            NamedKey::Left => KeyCode::Left,
            NamedKey::Right => KeyCode::Right,
            NamedKey::F(n) => KeyCode::F(*n),
            NamedKey::CapsLock => KeyCode::CapsLock,
            NamedKey::NumLock => KeyCode::NumLock,
            NamedKey::ScrollLock => KeyCode::ScrollLock,
            NamedKey::Menu => KeyCode::Menu,
        },
    };
    let mut mods = KeyModifiers::empty();
    if modifiers.shift {
        mods |= KeyModifiers::SHIFT;
    }
    if modifiers.ctrl {
        mods |= KeyModifiers::CONTROL;
    }
    if modifiers.alt {
        mods |= KeyModifiers::ALT;
    }
    if modifiers.cmd {
        mods |= KeyModifiers::SUPER;
    }
    let kind = if repeat {
        KeyEventKind::Repeat
    } else {
        KeyEventKind::Press
    };
    Some(KeyEvent {
        code,
        modifiers: mods,
        kind,
        state: KeyEventState::empty(),
    })
}

/// Synthesise a crossterm `MouseEvent` from any `UiEvent::Mouse*`
/// variant. Returns `None` for `UiEvent`s that aren't mouse events
/// (or for `MouseEntered`/`Left`, which crossterm doesn't surface
/// natively).
pub fn synth_mouseevent(ev: &UiEvent) -> Option<MouseEvent> {
    let (kind, position, modifiers) = match ev {
        UiEvent::MouseDown {
            button,
            position,
            modifiers,
            ..
        } => (
            MouseEventKind::Down(quadraui_button_to_crossterm(*button)?),
            *position,
            *modifiers,
        ),
        UiEvent::MouseUp {
            button, position, ..
        } => (
            MouseEventKind::Up(quadraui_button_to_crossterm(*button)?),
            *position,
            Modifiers::default(),
        ),
        UiEvent::MouseMoved { position, buttons } => {
            let kind = if buttons.left {
                MouseEventKind::Drag(CtMouseButton::Left)
            } else if buttons.right {
                MouseEventKind::Drag(CtMouseButton::Right)
            } else if buttons.middle {
                MouseEventKind::Drag(CtMouseButton::Middle)
            } else {
                MouseEventKind::Moved
            };
            (kind, *position, Modifiers::default())
        }
        UiEvent::Scroll {
            delta, position, ..
        } => {
            let kind = if delta.y < 0.0 {
                MouseEventKind::ScrollUp
            } else if delta.y > 0.0 {
                MouseEventKind::ScrollDown
            } else if delta.x < 0.0 {
                MouseEventKind::ScrollLeft
            } else {
                MouseEventKind::ScrollRight
            };
            (kind, *position, Modifiers::default())
        }
        _ => return None,
    };
    Some(MouseEvent {
        kind,
        column: position.x.max(0.0) as u16,
        row: position.y.max(0.0) as u16,
        modifiers: quadraui_modifiers_to_crossterm(modifiers),
    })
}

fn quadraui_button_to_crossterm(b: MouseButton) -> Option<CtMouseButton> {
    match b {
        MouseButton::Left => Some(CtMouseButton::Left),
        MouseButton::Right => Some(CtMouseButton::Right),
        MouseButton::Middle => Some(CtMouseButton::Middle),
        // X1/X2/Other don't have crossterm equivalents.
        _ => None,
    }
}

fn quadraui_modifiers_to_crossterm(m: Modifiers) -> KeyModifiers {
    let mut out = KeyModifiers::empty();
    if m.shift {
        out |= KeyModifiers::SHIFT;
    }
    if m.ctrl {
        out |= KeyModifiers::CONTROL;
    }
    if m.alt {
        out |= KeyModifiers::ALT;
    }
    if m.cmd {
        out |= KeyModifiers::SUPER;
    }
    out
}

/// Crossterm's `MouseEventKind::Drag(b)` carries the held button as
/// the variant payload; the quadraui `ButtonMask` reflects which
/// buttons are down at that moment. We only know the *one* button
/// crossterm reports, so the mask has just that bit set.
fn button_mask_with_held(b: CtMouseButton) -> ButtonMask {
    match b {
        CtMouseButton::Left => ButtonMask {
            left: true,
            right: false,
            middle: false,
        },
        CtMouseButton::Right => ButtonMask {
            left: false,
            right: true,
            middle: false,
        },
        CtMouseButton::Middle => ButtonMask {
            left: false,
            right: false,
            middle: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{
        KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton as CtMouseButton,
        MouseEvent, MouseEventKind,
    };

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        }
    }

    fn mouse(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: col,
            row,
            modifiers: KeyModifiers::empty(),
        }
    }

    #[test]
    fn char_keypress_translates_to_keypressed_char() {
        let ev = crossterm_key_to_uievent(key(KeyCode::Char('a'), KeyModifiers::empty()));
        assert_eq!(
            ev,
            Some(UiEvent::KeyPressed {
                key: Key::Char('a'),
                modifiers: Modifiers::default(),
                repeat: false,
            })
        );
    }

    #[test]
    fn ctrl_p_translates_with_ctrl_modifier() {
        let ev = crossterm_key_to_uievent(key(KeyCode::Char('p'), KeyModifiers::CONTROL)).unwrap();
        match ev {
            UiEvent::KeyPressed { key, modifiers, .. } => {
                assert_eq!(key, Key::Char('p'));
                assert!(modifiers.ctrl);
                assert!(!modifiers.shift);
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn named_keys_translate() {
        for (code, named) in &[
            (KeyCode::Esc, NamedKey::Escape),
            (KeyCode::Enter, NamedKey::Enter),
            (KeyCode::Backspace, NamedKey::Backspace),
            (KeyCode::Tab, NamedKey::Tab),
            (KeyCode::BackTab, NamedKey::BackTab),
            (KeyCode::Up, NamedKey::Up),
            (KeyCode::Down, NamedKey::Down),
            (KeyCode::Left, NamedKey::Left),
            (KeyCode::Right, NamedKey::Right),
            (KeyCode::PageUp, NamedKey::PageUp),
            (KeyCode::PageDown, NamedKey::PageDown),
            (KeyCode::Home, NamedKey::Home),
            (KeyCode::End, NamedKey::End),
            (KeyCode::Delete, NamedKey::Delete),
            (KeyCode::Insert, NamedKey::Insert),
            (KeyCode::F(1), NamedKey::F(1)),
            (KeyCode::F(12), NamedKey::F(12)),
        ] {
            let ev = crossterm_key_to_uievent(key(*code, KeyModifiers::empty())).unwrap();
            match ev {
                UiEvent::KeyPressed { key, .. } => {
                    assert_eq!(key, Key::Named(*named), "for code {:?}", code);
                }
                other => panic!("unexpected variant: {:?}", other),
            }
        }
    }

    #[test]
    fn key_release_drops() {
        let mut ev = key(KeyCode::Char('a'), KeyModifiers::empty());
        ev.kind = KeyEventKind::Release;
        assert!(crossterm_key_to_uievent(ev).is_none());
    }

    #[test]
    fn key_repeat_marks_repeat_true() {
        let mut ev = key(KeyCode::Char('a'), KeyModifiers::empty());
        ev.kind = KeyEventKind::Repeat;
        let translated = crossterm_key_to_uievent(ev).unwrap();
        match translated {
            UiEvent::KeyPressed { repeat, .. } => assert!(repeat),
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn mouse_down_left_translates() {
        let ev =
            crossterm_mouse_to_uievent(mouse(MouseEventKind::Down(CtMouseButton::Left), 10, 5))
                .unwrap();
        assert!(matches!(
            ev,
            UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                ..
            }
        ));
        if let UiEvent::MouseDown { position, .. } = ev {
            assert_eq!(position.x, 10.0);
            assert_eq!(position.y, 5.0);
        }
    }

    #[test]
    fn mouse_drag_carries_held_button() {
        let ev =
            crossterm_mouse_to_uievent(mouse(MouseEventKind::Drag(CtMouseButton::Right), 12, 8))
                .unwrap();
        match ev {
            UiEvent::MouseMoved { buttons, .. } => {
                assert!(!buttons.left);
                assert!(buttons.right);
                assert!(!buttons.middle);
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn mouse_scroll_up_translates_to_negative_y() {
        let ev = crossterm_mouse_to_uievent(mouse(MouseEventKind::ScrollUp, 0, 0)).unwrap();
        match ev {
            UiEvent::Scroll { delta, .. } => {
                assert_eq!(delta.y, -1.0);
                assert_eq!(delta.x, 0.0);
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn mouse_scroll_down_translates_to_positive_y() {
        let ev = crossterm_mouse_to_uievent(mouse(MouseEventKind::ScrollDown, 0, 0)).unwrap();
        match ev {
            UiEvent::Scroll { delta, .. } => assert_eq!(delta.y, 1.0),
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn resize_translates_to_window_resized() {
        let evs = crossterm_to_uievents(CtEvent::Resize(120, 40));
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            UiEvent::WindowResized { viewport } => {
                assert_eq!(viewport.width, 120.0);
                assert_eq!(viewport.height, 40.0);
                assert_eq!(viewport.scale, 1.0);
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn paste_translates_to_clipboard_paste() {
        let evs = crossterm_to_uievents(CtEvent::Paste("hello".into()));
        assert_eq!(evs, vec![UiEvent::ClipboardPaste("hello".into())]);
    }

    #[test]
    fn focus_gained_translates() {
        let evs = crossterm_to_uievents(CtEvent::FocusGained);
        assert_eq!(evs, vec![UiEvent::WindowFocused(true)]);
    }

    #[test]
    fn focus_lost_translates() {
        let evs = crossterm_to_uievents(CtEvent::FocusLost);
        assert_eq!(evs, vec![UiEvent::WindowFocused(false)]);
    }

    #[test]
    fn shift_ctrl_modifiers_combine() {
        let mods = crossterm_modifiers_to_quadraui(KeyModifiers::SHIFT | KeyModifiers::CONTROL);
        assert!(mods.shift);
        assert!(mods.ctrl);
        assert!(!mods.alt);
        assert!(!mods.cmd);
    }

    #[test]
    fn super_modifier_maps_to_cmd() {
        let mods = crossterm_modifiers_to_quadraui(KeyModifiers::SUPER);
        assert!(mods.cmd);
    }

    // ─── Inverse: round-trip tests ──────────────────────────────────────────
    //
    // Stage 5b's loop migration relies on `synth_keyevent` /
    // `synth_mouseevent` giving back a crossterm event the existing
    // legacy handlers will accept. The round-trip through
    // `crossterm → UiEvent → synth_*` must preserve the fields
    // those handlers consult.

    #[test]
    fn key_roundtrips_through_synth() {
        for code in &[
            KeyCode::Char('a'),
            KeyCode::Char('Z'),
            KeyCode::Esc,
            KeyCode::Enter,
            KeyCode::Tab,
            KeyCode::BackTab,
            KeyCode::Backspace,
            KeyCode::Delete,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Home,
            KeyCode::End,
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::Insert,
            KeyCode::F(7),
        ] {
            let original = key(*code, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
            let ui = crossterm_key_to_uievent(original).unwrap();
            let UiEvent::KeyPressed {
                key: k,
                modifiers,
                repeat,
            } = ui
            else {
                panic!("not a key press for {:?}", code);
            };
            let round = synth_keyevent(&k, modifiers, repeat).unwrap();
            assert_eq!(round.code, *code, "code mismatch for {:?}", code);
            assert_eq!(round.modifiers, original.modifiers);
            assert_eq!(round.kind, KeyEventKind::Press);
        }
    }

    #[test]
    fn mouse_down_roundtrips() {
        let original = mouse(MouseEventKind::Down(CtMouseButton::Left), 7, 12);
        let ui = crossterm_mouse_to_uievent(original).unwrap();
        let round = synth_mouseevent(&ui).unwrap();
        assert!(matches!(
            round.kind,
            MouseEventKind::Down(CtMouseButton::Left)
        ));
        assert_eq!(round.column, 7);
        assert_eq!(round.row, 12);
    }

    #[test]
    fn mouse_drag_roundtrips() {
        let original = mouse(MouseEventKind::Drag(CtMouseButton::Right), 30, 5);
        let ui = crossterm_mouse_to_uievent(original).unwrap();
        let round = synth_mouseevent(&ui).unwrap();
        assert!(matches!(
            round.kind,
            MouseEventKind::Drag(CtMouseButton::Right)
        ));
    }

    #[test]
    fn scroll_up_roundtrips() {
        let original = mouse(MouseEventKind::ScrollUp, 0, 0);
        let ui = crossterm_mouse_to_uievent(original).unwrap();
        let round = synth_mouseevent(&ui).unwrap();
        assert!(matches!(round.kind, MouseEventKind::ScrollUp));
    }
}
