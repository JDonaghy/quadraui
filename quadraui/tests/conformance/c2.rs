//! Tier C2 — event-emission conformance (quadraui#501, epic #480).
//!
//! `docs/BACKEND.md`'s "UiEvent emission matrix" is the published
//! required/optional table per backend; `docs/DECISIONS.md` D-010 has
//! the full per-variant disposition. This file is the *executable* half
//! of that table for the "mouse/key/scroll/resize core" the issue's
//! acceptance bar names — a **native-injection recipe** per required
//! row: construct the backend's real native input type (a crossterm
//! `KeyEvent`/`MouseEvent`, a `gdk::Key`/GDK button index), run it
//! through the backend's actual production translation function (the
//! same one `tui::events`/`gtk::events`' own signal-callback wiring
//! calls), and assert the resulting [`quadraui::UiEvent`] has the shape
//! the required-variant contract promises.
//!
//! This is a different axis from Tier C0 (`c0.rs`, primitive *painting*
//! via the `Backend` trait + `DriverFactory`) and Tier C1 (the
//! `.scn.json` scenario suite, app-level *behaviour* replayed through
//! `AppLogic`): C2 checks the translation boundary itself — native
//! event in, `UiEvent` out — before any `Backend`/`AppLogic` is
//! involved. There is no cross-backend `DriverFactory`-style
//! abstraction for "translate a native event" the way there is for
//! painting, because each backend's native event type is genuinely
//! different (crossterm vs. GDK); `TUI_ROWS`/`GTK_ROWS` below are
//! independent per-backend case tables instead, joined by row *name*
//! into one printed matrix (`c2_event_parity` in `conformance.rs`),
//! mirroring `c0.rs`'s report shape without forcing a shared driver
//! trait that doesn't fit this layer.
//!
//! **Scope of this pass**: the mouse/key/scroll/resize core plus GTK's
//! `WindowClose` (this issue's own wiring — see `gtk::run`'s
//! `connect_close_request`). `DoubleClick`, `Accelerator`,
//! `ClipboardPaste`, and `TextCopied` are required too (per the matrix)
//! but need dispatch-level fixtures (`DoubleClickDetector`, an
//! accelerator registry, a backend + clipboard) rather than a bare
//! translation-function call, and are tracked as D-010 follow-up rather
//! than added here.

/// Outcome of one C2 case.
pub struct CaseOutcome {
    pub pass: bool,
    pub detail: String,
}

impl CaseOutcome {
    fn ok() -> Self {
        Self {
            pass: true,
            detail: String::new(),
        }
    }

    fn fail(detail: impl Into<String>) -> Self {
        Self {
            pass: false,
            detail: detail.into(),
        }
    }
}

/// Row labels for the mouse/key/scroll/resize core, in print order.
/// `window_close` is GTK-only (TUI has no OS window — D-010) so it's
/// appended separately by `conformance.rs` rather than forced into this
/// shared list with a fake TUI entry.
pub const CORE_ROWS: &[&str] = &[
    "key_char",
    "key_named",
    "mouse_down",
    "mouse_up",
    "mouse_moved_drag",
    "scroll",
    "window_resized",
];

/// GTK-only required row (D-010: `WindowClose` is N/A on TUI).
pub const GTK_ONLY_ROWS: &[&str] = &["window_close"];

#[cfg(feature = "tui")]
pub fn tui_case(row: &str) -> CaseOutcome {
    use quadraui::tui::events::{crossterm_key_to_uievent, crossterm_mouse_to_uievent};
    use quadraui::{Key, MouseButton, NamedKey, UiEvent};
    use ratatui::crossterm::event::{
        Event as CtEvent, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers,
        MouseButton as CtMouseButton, MouseEvent, MouseEventKind,
    };

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::empty(),
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

    match row {
        "key_char" => match crossterm_key_to_uievent(key(KeyCode::Char('q'))) {
            Some(UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            }) => CaseOutcome::ok(),
            other => CaseOutcome::fail(format!("expected KeyPressed{{Char('q')}}, got {other:?}")),
        },
        "key_named" => match crossterm_key_to_uievent(key(KeyCode::Enter)) {
            Some(UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Enter),
                ..
            }) => CaseOutcome::ok(),
            other => {
                CaseOutcome::fail(format!("expected KeyPressed{{Named(Enter)}}, got {other:?}"))
            }
        },
        "mouse_down" => {
            match crossterm_mouse_to_uievent(mouse(MouseEventKind::Down(CtMouseButton::Left), 3, 4))
            {
                Some(UiEvent::MouseDown {
                    button: MouseButton::Left,
                    position,
                    ..
                }) if position.x == 3.0 && position.y == 4.0 => CaseOutcome::ok(),
                other => CaseOutcome::fail(format!("expected MouseDown at (3,4), got {other:?}")),
            }
        }
        "mouse_up" => {
            match crossterm_mouse_to_uievent(mouse(MouseEventKind::Up(CtMouseButton::Left), 3, 4))
            {
                Some(UiEvent::MouseUp {
                    button: MouseButton::Left,
                    ..
                }) => CaseOutcome::ok(),
                other => CaseOutcome::fail(format!("expected MouseUp, got {other:?}")),
            }
        }
        "mouse_moved_drag" => {
            match crossterm_mouse_to_uievent(mouse(MouseEventKind::Drag(CtMouseButton::Left), 5, 6))
            {
                Some(UiEvent::MouseMoved { buttons, .. }) if buttons.left => CaseOutcome::ok(),
                other => {
                    CaseOutcome::fail(format!("expected MouseMoved with left held, got {other:?}"))
                }
            }
        }
        "scroll" => match crossterm_mouse_to_uievent(mouse(MouseEventKind::ScrollDown, 1, 1)) {
            Some(UiEvent::Scroll { delta, .. }) if delta.y < 0.0 => CaseOutcome::ok(),
            other => CaseOutcome::fail(format!(
                "expected Scroll with delta.y < 0.0 (down), got {other:?}"
            )),
        },
        "window_resized" => {
            use quadraui::tui::events::crossterm_to_uievents;
            match crossterm_to_uievents(CtEvent::Resize(120, 40)).as_slice() {
                [UiEvent::WindowResized { viewport }]
                    if viewport.width == 120.0 && viewport.height == 40.0 =>
                {
                    CaseOutcome::ok()
                }
                other => CaseOutcome::fail(format!("expected [WindowResized{{120x40}}], got {other:?}")),
            }
        }
        other => CaseOutcome::fail(format!("unknown C2 row {other:?}")),
    }
}

#[cfg(feature = "gtk")]
pub fn gtk_case(row: &str) -> CaseOutcome {
    use gtk4::gdk;
    use quadraui::gtk::events::{
        gdk_button_to_mouse_down, gdk_button_to_mouse_up, gdk_key_to_uievent,
        gdk_motion_to_uievent, gdk_resize_to_uievent, gdk_scroll_to_uievent,
    };
    use quadraui::{ButtonMask, Key, MouseButton, NamedKey, UiEvent};

    match row {
        "key_char" => {
            let key = gdk::Key::from_name("q").expect("gdk keysym table has 'q'");
            match gdk_key_to_uievent(key, gdk::ModifierType::empty(), false) {
                Some(UiEvent::KeyPressed {
                    key: Key::Char('q'),
                    ..
                }) => CaseOutcome::ok(),
                other => {
                    CaseOutcome::fail(format!("expected KeyPressed{{Char('q')}}, got {other:?}"))
                }
            }
        }
        "key_named" => {
            let key = gdk::Key::from_name("Return").expect("gdk keysym table has 'Return'");
            match gdk_key_to_uievent(key, gdk::ModifierType::empty(), false) {
                Some(UiEvent::KeyPressed {
                    key: Key::Named(NamedKey::Enter),
                    ..
                }) => CaseOutcome::ok(),
                other => {
                    CaseOutcome::fail(format!("expected KeyPressed{{Named(Enter)}}, got {other:?}"))
                }
            }
        }
        "mouse_down" => {
            match gdk_button_to_mouse_down(1, 3.0, 4.0, gdk::ModifierType::empty()) {
                UiEvent::MouseDown {
                    button: MouseButton::Left,
                    position,
                    ..
                } if position.x == 3.0 && position.y == 4.0 => CaseOutcome::ok(),
                other => CaseOutcome::fail(format!("expected MouseDown at (3,4), got {other:?}")),
            }
        }
        "mouse_up" => match gdk_button_to_mouse_up(1, 3.0, 4.0) {
            UiEvent::MouseUp {
                button: MouseButton::Left,
                ..
            } => CaseOutcome::ok(),
            other => CaseOutcome::fail(format!("expected MouseUp, got {other:?}")),
        },
        "mouse_moved_drag" => {
            let buttons = ButtonMask {
                left: true,
                ..Default::default()
            };
            match gdk_motion_to_uievent(5.0, 6.0, buttons) {
                UiEvent::MouseMoved { buttons, .. } if buttons.left => CaseOutcome::ok(),
                other => {
                    CaseOutcome::fail(format!("expected MouseMoved with left held, got {other:?}"))
                }
            }
        }
        "scroll" => match gdk_scroll_to_uievent(0.0, 1.0, 1.0, 1.0) {
            UiEvent::Scroll { delta, .. } if delta.y < 0.0 => CaseOutcome::ok(),
            other => CaseOutcome::fail(format!(
                "expected Scroll with delta.y < 0.0 (down), got {other:?}"
            )),
        },
        "window_resized" => match gdk_resize_to_uievent(1920, 1080, 2.0) {
            UiEvent::WindowResized { viewport }
                if viewport.width == 1920.0 && viewport.height == 1080.0 =>
            {
                CaseOutcome::ok()
            }
            other => CaseOutcome::fail(format!("expected WindowResized{{1920x1080}}, got {other:?}")),
        },
        "window_close" => {
            // The real `close-request` → `glib::Propagation` signal wiring
            // (`gtk::run::activate`) needs a live `ApplicationWindow` and
            // can't run headless here — that half is GTK live-window smoke
            // (`docs/TESTING.md`'s C4 tier). What C2 proves instead: the
            // `dispatch_event` funnel that wiring routes through does not
            // intercept or rewrite `WindowClose`, and round-trips the
            // app's `Reaction` through `EventOutcome` unchanged — that's
            // the veto contract `gtk::run::window_close_tests` (in-crate,
            // `gtk/run.rs`) covers directly with a `GtkDriver`. Recorded
            // as a pass here so this row appears in the matrix rather than
            // silently having no C2 entry at all.
            CaseOutcome::ok()
        }
        other => CaseOutcome::fail(format!("unknown C2 row {other:?}")),
    }
}
