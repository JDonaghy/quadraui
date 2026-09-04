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
//! different (crossterm vs. GDK vs. raw `WM_*` message params);
//! `tui_case`/`gtk_case`/`win_case` below are independent per-backend
//! case tables instead, joined by row *name* into one printed matrix
//! (`c2_event_parity` in `conformance.rs`), mirroring `c0.rs`'s report
//! shape without forcing a shared driver trait that doesn't fit this
//! layer.
//!
//! **Win** (quadraui#742) needs no `target_os = "windows"` host: unlike
//! C0/C1, which drive a live `Backend` (a real `ID2D1DCRenderTarget` for
//! Win), C2 only calls `win::events`'s free functions — pure `WM_*`
//! message + already-decoded params → `UiEvent`, no WinAPI call in
//! sight (see `win::events`'s module doc) — so `win_case` runs on the
//! plain `ubuntu-latest --features win` leg, same as `tui_case`/
//! `gtk_case` run wherever their own feature is enabled.
//!
//! **Scope of this pass**: the mouse/key/scroll/resize core, plus
//! `WindowClose` on the two backends with a real OS window (GTK — this
//! issue's own wiring, see `gtk::run`'s `connect_close_request`; Win —
//! pre-existing `WM_CLOSE` handling, see `win::run`'s `wndproc`).
//! `DoubleClick`, `Accelerator`, `ClipboardPaste`, and `TextCopied` are
//! required too (per the matrix) but need dispatch-level fixtures
//! (`DoubleClickDetector`, an accelerator registry, a backend +
//! clipboard) rather than a bare translation-function call, and are
//! tracked as D-010 follow-up rather than added here — true for Win as
//! much as TUI/GTK: `WinBackend::match_keypress`/`dispatch_event` are
//! `pub(crate)` and need a live `WinBackend` + `AppLogic`, not a bare
//! native-input-in/`UiEvent`-out call, even though the underlying
//! wiring itself has landed (`WinBackend::match_keypress`, #707;
//! `WinBackend::fold_double_click`, #729; Ctrl-V paste, #728) — see
//! `docs/BACKEND.md`'s emission matrix for the production-wiring status
//! and this file's `WINDOWED_ROWS` doc for why `window_close` is the one
//! exception that *can* still appear as a (placeholder) row.

/// Outcome of one C2 case.
pub struct CaseOutcome {
    pub pass: bool,
    pub detail: String,
    /// Set only by [`CaseOutcome::placeholder`]. A placeholder row
    /// records a real pass/fail bit like any other row, but the bit
    /// isn't backed by a native-injection assertion the way every other
    /// row's is — see `window_close`'s use below. `conformance.rs`'s
    /// table printer marks these rows distinctly (`pass*` + a footnote)
    /// so a reader of the matrix can't mistake this cell for a verified
    /// pass the way a plain `ok()` would otherwise read.
    pub placeholder: bool,
}

impl CaseOutcome {
    fn ok() -> Self {
        Self {
            pass: true,
            detail: String::new(),
            placeholder: false,
        }
    }

    /// A row that always reports "pass" because the thing it names
    /// genuinely can't be asserted at this tier (see call site comment
    /// for why) — recorded so the row appears in the matrix at all,
    /// distinctly marked so it isn't read as equivalent to a verified
    /// `ok()`.
    ///
    /// The only placeholder row today is `window_close`, on GTK and Win
    /// (quadraui#742) — the two backends with a real OS window whose
    /// close veto only round-trips through a live window/`wndproc`, not
    /// a bare translation call — so on a `--features tui` build (neither
    /// `gtk` nor `win`) this constructor has no caller. CI sets
    /// `RUSTFLAGS: -D warnings` workflow-wide, which promotes that to a
    /// hard `dead_code` build failure on the tui leg — the same shape as
    /// this file's crate-root `cfg_attr(not(any(tui, gtk)), allow(dead_code))`
    /// in `conformance.rs`, and allowed for the same reason: the item is
    /// unreachable *from this feature set*, not unused. Scoped to
    /// `not(any(gtk, win))` rather than blanket-allowed so that if every
    /// `window_close` call below ever goes away, a `tui`-only leg still
    /// reports this as genuinely dead.
    #[cfg_attr(not(any(feature = "gtk", feature = "win")), allow(dead_code))]
    fn placeholder(detail: impl Into<String>) -> Self {
        Self {
            pass: true,
            detail: detail.into(),
            placeholder: true,
        }
    }

    fn fail(detail: impl Into<String>) -> Self {
        Self {
            pass: false,
            detail: detail.into(),
            placeholder: false,
        }
    }
}

/// Row labels for the mouse/key/scroll/resize core, in print order.
/// `window_close` is only applicable to backends with a real OS window
/// (TUI has none — D-010) so it's kept out of this shared list and
/// appended separately by `conformance.rs`, via [`WINDOWED_ROWS`].
pub const CORE_ROWS: &[&str] = &[
    "key_char",
    "key_named",
    "mouse_down",
    "mouse_up",
    "mouse_moved_drag",
    "scroll",
    "window_resized",
];

/// Required row(s) for backends with a real OS window — GTK and Win
/// (quadraui#742) — but N/A on TUI (D-010: TUI's "window" is the
/// terminal viewport, which the process doesn't close independently of
/// itself).
pub const WINDOWED_ROWS: &[&str] = &["window_close"];

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
            other => CaseOutcome::fail(format!(
                "expected KeyPressed{{Named(Enter)}}, got {other:?}"
            )),
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
            match crossterm_mouse_to_uievent(mouse(MouseEventKind::Up(CtMouseButton::Left), 3, 4)) {
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
                other => {
                    CaseOutcome::fail(format!("expected [WindowResized{{120x40}}], got {other:?}"))
                }
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
                other => CaseOutcome::fail(format!(
                    "expected KeyPressed{{Named(Enter)}}, got {other:?}"
                )),
            }
        }
        "mouse_down" => match gdk_button_to_mouse_down(1, 3.0, 4.0, gdk::ModifierType::empty()) {
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } if position.x == 3.0 && position.y == 4.0 => CaseOutcome::ok(),
            other => CaseOutcome::fail(format!("expected MouseDown at (3,4), got {other:?}")),
        },
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
            other => CaseOutcome::fail(format!(
                "expected WindowResized{{1920x1080}}, got {other:?}"
            )),
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
            // as a `placeholder()`, not `ok()` (quadraui#501 review), so
            // this row appears in the matrix rather than silently having
            // no C2 entry at all, but the printed table still marks it
            // as unverified-at-this-tier rather than reading identically
            // to a real assertion.
            CaseOutcome::placeholder(
                "no live-window assertion at this tier — see gtk::run::window_close_tests \
                 (dispatch_event pass-through) and the C4 live-window smoke tier for the \
                 real close-request/veto coverage",
            )
        }
        other => CaseOutcome::fail(format!("unknown C2 row {other:?}")),
    }
}

/// Win-GUI (quadraui#742). Unlike `tui_case`/`gtk_case`, none of the
/// `win::events` functions this calls need `target_os = "windows"` — see
/// this file's module doc and `win::events`'s own doc for why the
/// translators are plain host-independent functions, so this whole
/// function (and therefore the `win` column in `c2_event_parity`) runs
/// on the `ubuntu-latest --features win` leg with no live `HWND`/
/// Direct2D anywhere in sight.
#[cfg(feature = "win")]
pub fn win_case(row: &str) -> CaseOutcome {
    use quadraui::win::events::{
        win_button_down, win_button_up, win_mouse_moved, win_resize_to_uievent,
        win_wheel_to_uievent, wm_char_to_uievent, wm_keydown_to_uievent,
    };
    use quadraui::{ButtonMask, Key, Modifiers, MouseButton, NamedKey, UiEvent};

    // `VK_RETURN` (0x0D) — `<winuser.h>`'s stable numeric value, same
    // convention `win::events::vk_to_named_key`'s own match arms use.
    const VK_RETURN: u32 = 0x0D;

    match row {
        // `WM_CHAR`'s printable-text path (`wm_char_to_uievent`) — the
        // production caller only reaches this arm for a character
        // `vk_to_named_key` doesn't map, same split `wm_keydown_to_uievent`'s
        // doc describes.
        "key_char" => match wm_char_to_uievent('q', Modifiers::default(), false) {
            Some(UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            }) => CaseOutcome::ok(),
            other => CaseOutcome::fail(format!("expected KeyPressed{{Char('q')}}, got {other:?}")),
        },
        // `WM_KEYDOWN`'s named-key path (`wm_keydown_to_uievent`).
        "key_named" => match wm_keydown_to_uievent(VK_RETURN, Modifiers::default(), false) {
            Some(UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Enter),
                ..
            }) => CaseOutcome::ok(),
            other => CaseOutcome::fail(format!(
                "expected KeyPressed{{Named(Enter)}}, got {other:?}"
            )),
        },
        "mouse_down" => match win_button_down(MouseButton::Left, 3, 4, 1.0, Modifiers::default()) {
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } if position.x == 3.0 && position.y == 4.0 => CaseOutcome::ok(),
            other => CaseOutcome::fail(format!("expected MouseDown at (3,4), got {other:?}")),
        },
        "mouse_up" => match win_button_up(MouseButton::Left, 3, 4, 1.0) {
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
            match win_mouse_moved(5, 6, 1.0, buttons) {
                UiEvent::MouseMoved { buttons, .. } if buttons.left => CaseOutcome::ok(),
                other => {
                    CaseOutcome::fail(format!("expected MouseMoved with left held, got {other:?}"))
                }
            }
        }
        // Win32's wheel delta needs no sign flip (`win_wheel_to_uievent`'s
        // doc) — a backward/negative notch is already "down" in
        // quadraui's convention, so -120 (one notch back) is the "down"
        // input the other two backends reach via an explicit ScrollDown/
        // negate step.
        "scroll" => match win_wheel_to_uievent(-120, 1, 1, 1.0, false) {
            UiEvent::Scroll { delta, .. } if delta.y < 0.0 => CaseOutcome::ok(),
            other => CaseOutcome::fail(format!(
                "expected Scroll with delta.y < 0.0 (down), got {other:?}"
            )),
        },
        "window_resized" => match win_resize_to_uievent(1920, 1080, 2.0) {
            UiEvent::WindowResized { viewport }
                if viewport.width == 1920.0 && viewport.height == 1080.0 =>
            {
                CaseOutcome::ok()
            }
            other => CaseOutcome::fail(format!(
                "expected WindowResized{{1920x1080}}, got {other:?}"
            )),
        },
        "window_close" => {
            // Same shape as GTK's `window_close` placeholder above: the
            // real veto decision (`WM_CLOSE => if dispatch(...) ==
            // Reaction::Exit { DestroyWindow }`) lives in `win::run`'s
            // `wndproc`, which is `pub(crate)` and needs a live `HWND` —
            // unreachable from this external test crate and unreachable
            // headlessly either way. What *is* true and host-independent:
            // Win32 decodes no payload for `WM_CLOSE` at all (unlike
            // every other row here), so there is no native-injection
            // recipe to run in the first place — `UiEvent::WindowClose`
            // is the whole translation. Recorded as a `placeholder()`,
            // not `ok()`, so the row appears in the matrix without
            // reading as a verified assertion — the real coverage is
            // Win-GUI's own live-window smoke tier (C4) once it exists,
            // mirroring `gtk::run::window_close_tests` for GTK.
            CaseOutcome::placeholder(
                "no live-window assertion at this tier — WM_CLOSE carries no payload to \
                 translate (UiEvent::WindowClose has no fields), and the real close veto \
                 (win::run's wndproc: Reaction::Exit -> DestroyWindow) needs a live HWND, \
                 unreachable from this external test crate — see the C4 live-window smoke \
                 tier for the real coverage",
            )
        }
        other => CaseOutcome::fail(format!("unknown C2 row {other:?}")),
    }
}
