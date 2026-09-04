//! Win-GUI runner: Win32 message loop driving an [`AppLogic`] impl.
//!
//! The runner creates a Win32 window (`RegisterClassEx` + `CreateWindowEx`),
//! initialises a Direct2D render target via [`WinBackend::attach_surface`],
//! enters the message loop, and translates window-lifecycle `WM_*`
//! messages → [`UiEvent`] → `app.handle()`, redrawing via `app.render()`
//! on `WM_PAINT`.
//!
//! Mirrors `quadraui::gtk::run` and `quadraui::tui::run` — consumers
//! call `quadraui::win::run(MyApp::new())` and the same `AppLogic`
//! impl drives every backend. [`run_with`] takes a [`RunConfig`] for the
//! one value a `ShellConfig`-driven consumer needs that [`run`] hardcodes
//! (the window title) — see `win::shell_runner::run_with_shell` (#707),
//! mirroring `gtk::run::RunConfig`.
//!
//! # Window chrome: standard frame, not CSD — `desktop::WindowDragArm` N/A
//!
//! `run_inner` creates its window with `WS_OVERLAPPEDWINDOW` (below) —
//! the standard Windows title bar, system menu, and non-client resize
//! border, not a client-side-decorated one quadraui paints itself.
//! Title-bar drag-to-move and edge-resize are therefore already handled
//! entirely by Windows' own non-client (`WM_NC*`) message processing
//! before this `wndproc` ever sees them, the same way GTK's `gtk4::
//! WindowHandle`/edge-resize gesture needs [`crate::desktop::WindowDragArm`]
//! specifically *because* GTK's `DrawingArea`-filled window has no native
//! frame to delegate to. #702's audit considered adopting
//! `WindowDragArm` here and concluded it doesn't apply for exactly this
//! reason — see that type's doc comment for the fuller rationale. This
//! is a deliberate "not applicable", not an unfinished gap: adopt it only
//! if a future issue gives `win::run` a custom (CSD-style) frame.
//!
//! # Scope
//!
//! Issue #19 landed the window + render-target *bootstrap* and exactly
//! the three window-lifecycle events the message loop itself needs to
//! stay alive and responsive: `WM_SIZE` → [`UiEvent::WindowResized`],
//! `WM_DPICHANGED` → [`UiEvent::DpiChanged`], `WM_CLOSE` →
//! [`UiEvent::WindowClose`]. Issue #20 adds the rest of the input table
//! this `wndproc` dispatches: mouse buttons/motion/wheel, `WM_KEYDOWN` +
//! `WM_CHAR`, and focus — decoded by [`super::msg`], translated by
//! [`super::events`]. `WM_KEYUP` and the `WM_SYSKEY*`/double-click
//! families remain unhandled (no quadraui `UiEvent` counterpart exists
//! yet for the former; see `super::events`' module docs for the latter
//! two).
//!
//! # Dispatch model: direct, not `poll_events`/`wait_events`
//!
//! A Win32 message loop is callback-driven (`DispatchMessageW` calls
//! `WndProc` synchronously), not poll-driven like the TUI runner's
//! `wait_events(timeout)`. This mirrors the GTK runner: GTK's live
//! signal handlers call `dispatch_event`/`app.handle` directly from
//! each signal callback rather than routing through
//! `Backend::poll_events` (see `gtk::run`'s module docs, "single-DA
//! model"); `Backend::poll_events`/`wait_events` exist as a
//! forward-compat seam for future headless-driver parity, not as this
//! runner's hot path. `WinBackend`'s versions stay `todo!()` stubs
//! until something actually needs them.
//!
//! # Per-window state without closures
//!
//! `WndProc` must be a plain `extern "system" fn` — it cannot capture
//! `app`/`backend` as a closure. `WindowState` (wrapping `RunState` in a
//! `RefCell`, alongside a reentrancy-guard counter — see "Reentrancy
//! guard" below) is heap-allocated once via `Box::into_raw` *before*
//! `CreateWindowExW` runs, and its address is
//! threaded through as `CreateWindowExW`'s last parameter
//! (`lpCreateParams`), which Windows hands back inside `WM_NCCREATE`'s
//! `CREATESTRUCTW`. `wndproc::<A>` stashes that pointer in
//! `GWLP_USERDATA` on `WM_NCCREATE` and reads it back on every
//! subsequent message — the same raw-pointer-in-`GWLP_USERDATA` pattern
//! every Win32 Rust binding uses, since the OS API predates closures by
//! three decades.
//!
//! `wndproc::<A>` is a generic function monomorphized once per
//! concrete `A: AppLogic` — each instantiation is a distinct
//! `extern "system" fn` with its own address, so `RegisterClassExW`
//! (which needs one concrete function pointer) can register the right
//! one for whatever app type `run::<A>` was called with.
//!
//! # Reentrancy guard
//!
//! `wndproc` dispatches `app.handle`/`app.render` while holding
//! `WindowState::state`'s `RefCell` borrow. A `wndproc` invoked by
//! `DispatchMessageW` never nests directly, but a *synchronous* native
//! re-entry onto this same thread is possible — a future `AppLogic` that
//! shows a native modal (which internally pumps messages) or
//! `SendMessage`s its own `hwnd` from inside `handle`/`render` — and
//! would otherwise double-borrow the same `RefCell` and panic (quadraui
//! #702, following up on #498/#427, which hit the identical hazard in
//! `gtk::services`'s async-dialog pump first). [`guarded_call`] plus the
//! `ws.pump_depth.is_pumping()` check at the top of `wndproc` close it:
//! [`crate::desktop::ModalPumpDepth`]/[`crate::desktop::ModalPumpGuard`]
//! (extracted backend-neutral by #498) track whether a guarded call is
//! already in flight, and a reentrant message is ceded to
//! `DefWindowProcW` rather than ever touching `WindowState::state`.
//! [`guarded_call`] itself has no WinAPI dependency, so it — and the
//! double-borrow scenario it prevents — are unit-tested off Windows; see
//! the `tests` module below.
//!
//! # Headless smoke mode (#702, adopting `desktop::SmokeConfig`)
//!
//! Mirrors `gtk::run`'s "Headless smoke mode" (quadraui#450, GD-5): two
//! environment variables, read once at startup via
//! [`crate::desktop::SmokeConfig::from_env`], let any `win_*` example run
//! unattended and exit with a deterministic, checkable status instead of
//! sitting in the message loop forever waiting for a user who isn't
//! there.
//!
//! - `QUADRAUI_WIN_SMOKE_MS=<u64>` — enables smoke mode. `after_ms`
//!   milliseconds after the window is shown, a one-shot `WM_TIMER`
//!   (`win32::SMOKE_TIMER_ID`) fires `win32::run_smoke_check`, which
//!   checks the client area's size against a sane floor
//!   ([`crate::desktop::smoke_size_ok`] — the same #437 tiny-window
//!   regression class GTK's smoke lane guards against), then closes the
//!   window.
//! - `QUADRAUI_WIN_SMOKE_PASTE=<text>` — optional. If set, the same timer
//!   round-trips `<text>` through the real OS clipboard
//!   (`WinPlatformServices::clipboard()`, the same object the live Ctrl-V
//!   handler below reads) and checks it byte-for-byte via
//!   [`crate::desktop::smoke_clipboard_round_trip_ok`], then — since
//!   #728 gave `dispatch_event` a real Ctrl-V → `ClipboardPaste`
//!   interception path — dispatches a synthetic Ctrl-V `KeyPressed`
//!   through it, mirroring `gtk::run::schedule_smoke_check`'s identical
//!   round-trip-then-replay sequence. Note this replay's `dispatch`
//!   return value is never inspected here (same as GTK's equivalent
//!   fire-and-forget call) — it exercises the interception code path so
//!   a panic there would fail the smoke, but on its own it cannot tell
//!   "recognised as paste" apart from "fell through to `app.handle`
//!   unchanged," since nothing asserts on the outcome. Real coverage of
//!   `is_paste_keypress` matching is `crate::desktop`'s pure-predicate
//!   unit tests, not this smoke check.
//!
//! Any assertion failure is printed to stderr (`win32::run_smoke_check`
//! is exempted from the crate-wide `print_stderr` deny for the same
//! reason `gtk::run`'s equivalent is) and flips [`run`]'s
//! [`std::process::ExitCode`] to [`std::process::ExitCode::FAILURE`].
//! Disabled (zero runtime cost — no timer is ever armed) unless
//! `QUADRAUI_WIN_SMOKE_MS` is set.

use crate::backend::Backend;
use crate::desktop::{is_paste_keypress, PasteModifier};
use crate::event::Viewport;
use crate::runner::AppLogic;
// `EventOutcome` — what the caller should do after `dispatch_event`
// handles one event — is defined once in `crate::runtime` and shared by
// every backend runner (quadraui#496); re-exported (not just imported)
// so `win::testing` reaches it through this path, mirroring
// `tui::run`/`macos::run`.
pub(crate) use crate::runtime::EventOutcome;
use crate::win::backend::WinBackend;
use crate::{ActivityBarEvent, UiEvent};

/// Dispatch one already-translated [`UiEvent`] through the app, applying
/// the runner's built-in pre-processing first. This is the funnel both
/// the live `wndproc`'s `dispatch` helper (`mod win32`, below) and
/// [`super::testing::WinDriver`] (quadraui#707) route through, so a test
/// exercises the exact pre-processing a real keypress gets — mirrors
/// [`crate::gtk::run::dispatch_event`] / [`crate::macos::run::dispatch_event`].
///
/// Pre-processing handled here, in priority order:
/// - `MouseDown` ([`WinBackend::fold_double_click`], #729): folds into
///   `UiEvent::DoubleClick` when it lands within the shared
///   `dispatch::DoubleClickDetector`'s time/position window of the
///   previous click. Independent of every other arm below (those only
///   ever match `KeyPressed`), so its position in the list doesn't
///   interact with them — listed first because it's the first check this
///   function performs.
/// - `KeyPressed` while an `ActivityBar` declared
///   `is_keyboard_focused = true` (tracked by
///   [`WinBackend::draw_activity_bar`] into
///   `WinBackend::focused_activity_bar_id`): redirect to
///   `UiEvent::ActivityBar(id, ActivityBarEvent::KeyPressed { … })`
///   instead of the app's normal `handle`. `ShellAdapter`'s built-in
///   activity-bar keyboard cursor (#409) depends on this — without it,
///   every `ShellApp` on Win-GUI would silently lose keyboard navigation
///   the other three backends already have.
/// - `KeyPressed` matching a registered `Global`-scope accelerator
///   ([`WinBackend::match_keypress`]): rewrite to `UiEvent::Accelerator`.
///   Ordered *after* the activity-bar intercept above (same priority
///   `gtk::run::dispatch_event` / `macos::run::dispatch_event` use), so a
///   bound accelerator never steals a navigation key out from under a
///   keyboard-focused activity bar.
/// - Ctrl-V / Ctrl-Shift-V ([`is_paste_keypress`], shared with
///   `gtk::run`/`macos::run` since #728): reads the system clipboard via
///   [`WinBackend::services`] and delivers `UiEvent::ClipboardPaste`
///   instead of forwarding the raw key press — Win-GUI's first paste
///   support at all (#728; Win32 has no native paste signal on a bespoke
///   `HWND` client area, same reasoning `gtk::run`'s doc comment gives
///   for GTK's bespoke `DrawingArea`). See `docs/DECISIONS.md` D-011 for
///   the shift-tolerance contract this predicate settles once for every
///   backend.
///
/// Anything not matched above falls through to `app.handle` unchanged.
///
/// Not `target_os`-gated: neither `WinBackend::focused_activity_bar_id`
/// nor `WinBackend::match_keypress` touch Direct2D/Win32 directly (they
/// read plain `Option`/`Vec` fields populated by `register_accelerator`
/// and `draw_activity_bar`), so this compiles and behaves identically on
/// every host — same "compiles everywhere" posture as [`RunConfig`]
/// above. The new paste arm keeps that posture too:
/// `WinPlatformServices::clipboard()`'s `read_text()` is a real Win32
/// clipboard read under `target_os = "windows"` and an unconditional
/// `None` everywhere else (see `win::services`'s `Clipboard` impl), so
/// off-Windows this arm always falls through to `EventOutcome::Continue`
/// with nothing forwarded to the app — never a compile-time or
/// behavioral surprise on the `ubuntu-latest` `--features win` leg.
///
/// `#[allow(dead_code)]`: this function's only callers — `mod win32`'s
/// live `wndproc`/`dispatch` and [`super::testing::WinDriver`] — are both
/// `#[cfg(target_os = "windows")]`-gated (the former directly, the
/// latter because `mod testing` only exists on Windows — see
/// `win::mod`'s doc). On a non-Windows host (`cargo check`/`cargo test
/// --features win` on the `ubuntu-latest` CI leg) neither caller exists,
/// so without this it — and everything it calls in turn
/// (`WinBackend::focused_activity_bar_id`/`match_keypress`,
/// `key_to_activity_bar_string`, `EventOutcome`) — would trip `-D
/// warnings`' dead-code lint on that leg despite being genuinely used on
/// the `windows-latest` leg where it matters.
#[allow(dead_code)]
pub(crate) fn dispatch_event<A: AppLogic>(
    event: UiEvent,
    backend: &mut WinBackend,
    app: &mut A,
) -> EventOutcome {
    // ── Double-click folding (#729) ───────────────────────────────────
    //
    // Folds a `MouseDown` into `DoubleClick` when it lands within the
    // shared `dispatch::DoubleClickDetector`'s time/position window of the
    // previous click — the same synthesis-from-`MouseDown`-stream pattern
    // `MacBackend::fold_double_click` uses, not a new `WM_*BUTTONDBLCLK`
    // translator. Every other variant (including plain `MouseDown` that
    // didn't fold) passes through unchanged.
    let event = backend.fold_double_click(event);

    // ── ActivityBar keyboard focus intercept (#707) ──────────────────
    if let UiEvent::KeyPressed {
        ref key, modifiers, ..
    } = event
    {
        if let Some(bar_id) = backend.focused_activity_bar_id().cloned() {
            let key_str = crate::primitives::activity_bar::key_to_activity_bar_string(key);
            let bar_ev = UiEvent::ActivityBar(
                bar_id,
                ActivityBarEvent::KeyPressed {
                    key: key_str,
                    modifiers,
                },
            );
            return app.handle(bar_ev, backend).into();
        }
    }

    // ── Global accelerator dispatch (#707) ───────────────────────────
    let event = if let UiEvent::KeyPressed { key, modifiers, .. } = &event {
        match backend.match_keypress(key, *modifiers) {
            Some(id) => UiEvent::Accelerator(id, *modifiers),
            None => event,
        }
    } else {
        event
    };

    // ── Ctrl-V / Ctrl-Shift-V interception (paste, #728) ─────────────
    if let UiEvent::KeyPressed { key, modifiers, .. } = &event {
        if is_paste_keypress(key, modifiers, PasteModifier::Ctrl) {
            return if let Some(text) = backend.services().clipboard().read_text() {
                app.handle(UiEvent::ClipboardPaste(text), backend).into()
            } else {
                EventOutcome::Continue
            };
        }
    }

    app.handle(event, backend).into()
}

/// Render one frame: `begin_frame` + `app.render` + `end_frame` — the
/// exact body `mod win32`'s `WM_PAINT` handler used to run inline,
/// extracted so it never depends on a live `HWND`, only a [`WinBackend`]
/// with *some* surface attached (a real one via
/// [`WinBackend::attach_surface`], or a headless one via
/// [`WinBackend::attach_headless`]). Shared by the live runner and
/// [`super::testing::WinDriver`] (quadraui#707) — mirrors
/// [`crate::gtk::run::render_frame`] / [`crate::macos::run::render_frame`].
///
/// `#[allow(dead_code)]`: same reasoning as [`dispatch_event`]'s doc —
/// both its callers only exist on `target_os = "windows"`.
#[allow(dead_code)]
pub(crate) fn render_frame<A: AppLogic>(backend: &mut WinBackend, app: &A, viewport: Viewport) {
    backend.begin_frame(viewport);
    app.render(backend, Default::default());
    backend.end_frame();
}

/// Configuration for [`run_with`]: the window title [`run`] hardcodes to
/// a generic default (`"quadraui"`).
///
/// A `ShellConfig`-driven consumer (`win::shell_runner::run_with_shell`)
/// needs its own window title to reach the real Win32 window instead of
/// always showing `"quadraui"` — mirrors `gtk::run::RunConfig`, minus the
/// GTK-specific `app_id`/`icon_name` fields Win32 has no equivalent
/// concept for.
///
/// Deliberately *not* `#[cfg(target_os = "windows")]`: it must type-check
/// under a plain `cargo check --features win` on Linux, since
/// `win::shell_runner::run_with_shell` constructs one unconditionally
/// (same "compiles everywhere, only *works* on Windows" posture as the
/// rest of `src/win/` — see `Cargo.toml`'s `win`-example comments).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunConfig {
    /// Window title shown by the title bar, taskbar, and Alt-Tab switcher.
    ///
    /// Encoded as a `\0`-terminated wide string before reaching Win32
    /// (see `run_inner`'s `title_nul`) — an embedded `'\0'` character in
    /// this string silently truncates the displayed title at that point
    /// rather than erroring, since Win32 treats the first NUL as the
    /// terminator. Not a concern for any title a real app would set, but
    /// worth knowing if this is ever built from untrusted input.
    pub title: String,
}

impl RunConfig {
    /// Build a config with the given window title.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
        }
    }
}

impl Default for RunConfig {
    /// Mirrors [`run`]'s previously-hardcoded window title, so
    /// `run_with(app, RunConfig::default())` and `run(app)` behave
    /// identically.
    fn default() -> Self {
        Self {
            title: "quadraui".to_string(),
        }
    }
}

// `ModalPumpDepth`/`ModalPumpGuard`/`RefCell` are only reachable from
// `guarded_call` and its test module below — both `#[cfg(any(target_os
// = "windows", test))]`, matching `win32`'s own `target_os = "windows"`
// gate plus an off-Windows `test`-only carve-out (see `guarded_call`'s
// doc for why it, unlike `win32`, is unit-testable off Windows). Gating
// these imports identically avoids an unresolved-import error on a
// plain, non-test `cargo check --features win` build on a non-Windows
// host, where neither `guarded_call` nor `win32::dispatch` exist to
// consume them.
#[cfg(any(target_os = "windows", test))]
use crate::desktop::{ModalPumpDepth, ModalPumpGuard};
#[cfg(any(target_os = "windows", test))]
use std::cell::RefCell;

/// Runs `f` against a mutable borrow of `*state`, guarded by
/// `pump_depth` against the `wndproc` reentrancy hazard this module used
/// to just document rather than close (#702, following up on #498): if
/// `pump_depth` already shows a guarded call in flight on this thread —
/// i.e. something reachable from `f` re-enters this same call path
/// synchronously, the way `win32::dispatch`'s doc comment describes a
/// future `AppLogic` pumping a native modal or `SendMessage`ing its own
/// `hwnd` from inside `handle` — this returns `None` *without ever
/// taking `state`'s `RefCell` borrow*, instead of panicking on a double
/// `borrow_mut()`.
///
/// Pure `RefCell`/[`ModalPumpDepth`] logic with no WinAPI dependency of
/// its own, so — unlike its production caller, `win32::dispatch`
/// (Windows-only, since `wndproc` itself is) — this is unit-testable off
/// Windows; see the `tests` module below for a from-scratch reproduction
/// of the double-borrow panic this guards against.
#[cfg(any(target_os = "windows", test))]
fn guarded_call<T, R>(
    state: &RefCell<T>,
    pump_depth: &ModalPumpDepth,
    f: impl FnOnce(&mut T) -> R,
) -> Option<R> {
    if pump_depth.is_pumping() {
        return None;
    }
    let _guard = ModalPumpGuard::new(pump_depth);
    Some(f(&mut state.borrow_mut()))
}

#[cfg(target_os = "windows")]
mod win32 {
    use std::cell::{Cell, RefCell};
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::rc::Rc;

    use windows::core::{Error as WinError, PCWSTR};
    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        InvalidateRect, ScreenToClient, UpdateWindow, ValidateRect,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyState, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
        GetMessageW, GetWindowLongPtrW, KillTimer, LoadCursorW, PostQuitMessage, RegisterClassExW,
        SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage, CREATESTRUCTW,
        CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, GWLP_USERDATA, HTCLIENT, IDC_ARROW, MSG,
        SWP_NOACTIVATE, SWP_NOZORDER, SW_SHOW, WINDOW_EX_STYLE, WM_CHAR, WM_CLOSE, WM_DESTROY,
        WM_DPICHANGED, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN,
        WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCREATE, WM_PAINT,
        WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETCURSOR, WM_SETFOCUS, WM_SIZE, WM_TIMER, WM_XBUTTONDOWN,
        WM_XBUTTONUP, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
    };

    use crate::backend::Backend;
    use crate::desktop::{
        smoke_clipboard_round_trip_ok, smoke_size_ok, ModalPumpDepth, SmokeConfig,
    };
    use crate::event::UiEvent;
    use crate::runner::{AppLogic, Reaction};
    use crate::win::backend::WinBackend;
    // Message → `UiEvent` translation lives in `super::events` so it can
    // be unit-tested off Windows (pure functions over already-decoded
    // ints/floats/bools) — see that module's docs.
    use crate::win::events::{
        win_button_down, win_button_up, win_focus_to_uievent, win_modifiers,
        win_mouse_button_for_message, win_mouse_moved, win_wheel_to_uievent, wm_char_to_uievent,
        wm_keydown_to_uievent,
    };
    // Payload decoding lives in `super::msg` so it can be unit-tested off
    // Windows — see that module's docs for why the shifts aren't inlined
    // here.
    use crate::win::msg::{
        dpi_scale_from_wparam, is_repeat_from_lparam, point_from_lparam, size_from_lparam,
        wheel_delta_from_wparam,
    };
    use crate::{ButtonMask, Key, Modifiers};

    /// Window-class name. Null-terminated up front — every `PCWSTR` this
    /// module builds from a Rust string does the same, since Win32 wide
    /// strings have no length field.
    const CLASS_NAME: &str = "QuadrauiWin32WindowClass\0";

    /// Seed window size in DIPs, matching the GTK runner's
    /// `DEFAULT_WINDOW_WIDTH`/`HEIGHT`. `WM_SIZE` (fired synchronously
    /// from inside `CreateWindowExW`, then again on every real resize)
    /// immediately corrects `WinBackend`'s viewport to the window's
    /// actual client size.
    const DEFAULT_WIDTH: i32 = 800;
    const DEFAULT_HEIGHT: i32 = 600;

    // ─── Headless smoke mode (#702) ─────────────────────────────────────
    //
    // See the module doc's "Headless smoke mode" section. Env var names
    // mirror GTK's `QUADRAUI_GTK_SMOKE_MS`/`_PASTE` with a `WIN` infix —
    // `SmokeConfig::from_env` is parameterised over the name specifically
    // so a second backend's adoption never collides with GTK's.
    const SMOKE_MS_VAR: &str = "QUADRAUI_WIN_SMOKE_MS";
    const SMOKE_PASTE_VAR: &str = "QUADRAUI_WIN_SMOKE_PASTE";
    /// Comfortably below [`DEFAULT_WIDTH`]/[`DEFAULT_HEIGHT`] so a normal
    /// (even heavily letterboxed) window passes, but still catches the
    /// quadraui#437 tiny/wrapped-window regression class — same floor
    /// GTK's `SMOKE_MIN_WIDTH`/`SMOKE_MIN_HEIGHT` use.
    const SMOKE_MIN_WIDTH: i32 = 200;
    const SMOKE_MIN_HEIGHT: i32 = 150;
    /// `SetTimer`/`KillTimer`'s `nIDEvent`. Only ever one timer live on a
    /// given `hwnd` in this runner, so any nonzero id is fine — Win32
    /// scopes timer ids per-window, not per-process.
    const SMOKE_TIMER_ID: usize = 1;

    /// Everything a live window needs, reachable from `wndproc::<A>` via
    /// `GWLP_USERDATA` (see module docs).
    struct RunState<A: AppLogic> {
        app: A,
        backend: WinBackend,
    }

    /// Per-window state stashed in `GWLP_USERDATA` (see module docs).
    /// `pump_depth` deliberately lives *outside* `state`'s `RefCell`
    /// rather than as a `RunState` field: `super::guarded_call` (used by
    /// [`dispatch`] and the `WM_PAINT` handler below) must be able to
    /// read it without itself needing a borrow of the very `RefCell` a
    /// reentrant `wndproc` call would already be fighting over — see
    /// `super::guarded_call`'s docs (#702).
    struct WindowState<A: AppLogic> {
        state: RefCell<RunState<A>>,
        pump_depth: ModalPumpDepth,
        /// `Some` only when `QUADRAUI_WIN_SMOKE_MS` is set (#702) — see
        /// the module doc's "Headless smoke mode" section. Read once by
        /// the `WM_TIMER` handler in [`wndproc`]; never mutated.
        smoke: Option<SmokeConfig>,
        /// `Rc`, not a plain `Cell`, so [`run_inner`] can hold a clone
        /// that survives past `WindowState` itself being freed (its
        /// `Box::from_raw` happens right before the message loop's final
        /// exit-code decision needs this value) — same reason
        /// `gtk::run::run_with`'s `smoke_failed` is an `Rc<Cell<bool>>`
        /// shared with the `Application`, not a plain local.
        smoke_failed: Rc<Cell<bool>>,
    }

    /// Encode a Rust `&str` (already `\0`-terminated by its caller) as
    /// UTF-16 for a Win32 wide-string API.
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    // `WM_MOUSEMOVE`'s `wparam` key-state flags (`<winuser.h>`'s
    // `MK_LBUTTON`/`MK_RBUTTON`/`MK_MBUTTON`). Defined locally rather than
    // pulled from the `windows` crate — same reasoning `super::events`'
    // module docs give for its local `WM_*` consts: only the arithmetic
    // mask is needed, and it's one less binding-crate detail to get right.
    const MK_LBUTTON: usize = 0x0001;
    const MK_RBUTTON: usize = 0x0002;
    const MK_MBUTTON: usize = 0x0010;

    /// Live modifier state for the message currently being dispatched,
    /// via `GetKeyState` — see `super::events`' module docs on why Win32
    /// needs a per-message read here rather than a bitmask carried on the
    /// message itself (the one exception, `WM_MOUSEMOVE`'s `MK_*` button
    /// flags, is handled separately above).
    fn win_key_modifiers() -> Modifiers {
        win_modifiers(
            key_is_down(VK_CONTROL),
            key_is_down(VK_SHIFT),
            key_is_down(VK_MENU),
            key_is_down(VK_LWIN) || key_is_down(VK_RWIN),
        )
    }

    /// `GetKeyState`'s return value has its high bit set when the key is
    /// currently down — reading it as a signed `i16` and comparing `< 0`
    /// is the standard idiom (matches `IS_KEY_DOWN`-style macros other
    /// Win32 bindings define for this).
    fn key_is_down(vk: VIRTUAL_KEY) -> bool {
        unsafe { GetKeyState(vk.0 as i32) < 0 }
    }

    pub(super) fn run<A: AppLogic + 'static>(
        app: A,
        config: super::RunConfig,
    ) -> std::process::ExitCode {
        // #19 acceptance: "DPI scale factor plumbed to Viewport::scale".
        // Without per-monitor-v2 awareness, Windows silently bitmap-scales
        // the whole window on HiDPI displays and `GetDpiForWindow` always
        // reports 96 (100%) — this must run before the window (and its
        // DPI-aware render target) is created. Best-effort: an older
        // Windows build lacking this API, or a manifest that already
        // declared DPI awareness (the acceptance criteria's other listed
        // option), both make this call fail harmlessly — either way the
        // process keeps running at whatever awareness level is already
        // active rather than aborting startup over it.
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }

        match unsafe { run_inner(app, &config.title) } {
            // `Ok(smoke_ok)`: `smoke_ok` is `true` unless
            // `QUADRAUI_WIN_SMOKE_MS` was set *and* `run_smoke_check`
            // found a failure (#702) — see the module doc's "Headless
            // smoke mode" section. Always `true` when smoke mode is off.
            Ok(smoke_ok) => {
                if smoke_ok {
                    std::process::ExitCode::SUCCESS
                } else {
                    std::process::ExitCode::FAILURE
                }
            }
            Err(_) => {
                // `Backend`'s frame/window lifecycle has no error
                // channel (docs/SMELL_AUDIT_2026-07.md #93) and this
                // crate denies `clippy::print_stderr` crate-wide, so the
                // `windows::core::Error`'s message has nowhere to go
                // beyond the process exit code.
                std::process::ExitCode::FAILURE
            }
        }
    }

    /// # Safety
    ///
    /// Must run on the thread that will pump the returned message loop
    /// to completion — Win32 windows are thread-affine, and `wndproc`
    /// below assumes it's only ever invoked by `DispatchMessageW` on
    /// this same thread (no synchronization guards `RunState`'s
    /// `RefCell`).
    ///
    /// Returns `Ok(smoke_ok)` rather than plain `Ok(())` (#702):
    /// `smoke_ok` is always `true` unless `QUADRAUI_WIN_SMOKE_MS` armed
    /// the headless smoke check and it found a failure — see the module
    /// doc's "Headless smoke mode" section and [`run_smoke_check`].
    unsafe fn run_inner<A: AppLogic + 'static>(
        mut app: A,
        title: &str,
    ) -> windows::core::Result<bool> {
        let hinstance: HINSTANCE = unsafe { GetModuleHandleW(PCWSTR::null())?.into() };

        let class_name = wide(CLASS_NAME);
        // `wide()` expects an already-`\0`-terminated string (see its doc
        // comment) — `title` (from `RunConfig::title`) carries no
        // terminator of its own, unlike the `\0`-suffixed string literals
        // this module builds everywhere else.
        let title_nul = format!("{title}\0");
        let window_title = wide(&title_nul);

        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc::<A>),
            hInstance: hinstance,
            hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        // `RegisterClassExW` returns 0 (a Win32 `ATOM` is never zero on
        // success) on failure — there's no `Result`-returning wrapper for
        // it in `windows-rs`, so this reaches for `GetLastError()` via
        // `Error::from_thread()` the same way the crate's own `Result`
        // wrappers do internally.
        if unsafe { RegisterClassExW(&wc) } == 0 {
            return Err(WinError::from_thread());
        }

        // Backend + app setup happens *before* the window exists — same
        // ordering as `gtk::run` (`GtkBackend::new()` then `app.setup()`
        // before the `ApplicationWindow` is realized) and `tui::run`.
        // `backend.viewport()` reads `WinBackend::new`'s zeroed seed
        // until `WM_SIZE`/`attach_surface` populate it below; apps
        // reading it from `setup()` for immediate sizing decisions won't
        // see the real window size yet, matching the other two runners'
        // same limitation.
        let mut backend = WinBackend::new();
        app.setup(&mut backend);

        // #702: `None` (zero runtime cost — no timer ever armed below)
        // unless `QUADRAUI_WIN_SMOKE_MS` is set. See the module doc's
        // "Headless smoke mode" section.
        let smoke = SmokeConfig::from_env(SMOKE_MS_VAR, SMOKE_PASTE_VAR);
        let smoke_failed = Rc::new(Cell::new(false));

        let state_ptr: *mut WindowState<A> = Box::into_raw(Box::new(WindowState {
            state: RefCell::new(RunState { app, backend }),
            pump_depth: ModalPumpDepth::new(),
            smoke,
            smoke_failed: smoke_failed.clone(),
        }));

        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                PCWSTR(class_name.as_ptr()),
                PCWSTR(window_title.as_ptr()),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                DEFAULT_WIDTH,
                DEFAULT_HEIGHT,
                None,
                None,
                Some(hinstance),
                Some(state_ptr as *const c_void),
            )
        };
        let hwnd = match hwnd {
            Ok(hwnd) => hwnd,
            Err(e) => {
                // SAFETY: `state_ptr` came from `Box::into_raw` above and
                // hasn't been freed yet — `CreateWindowExW` failing means
                // `wndproc` never got far enough to need it again.
                drop(unsafe { Box::from_raw(state_ptr) });
                return Err(e);
            }
        };

        // The window exists now, so `WinBackend` can create its
        // `ID2D1HwndRenderTarget` sized to the real client rect (#19's
        // "Direct2D `ID2D1HwndRenderTarget`" + "DPI scale factor plumbed
        // to `Viewport::scale`" criteria). `WM_SIZE` fired synchronously
        // during `CreateWindowExW` above already ran with `surface: None`
        // (see `WinBackend::resize_surface`'s docs) — this is the first
        // point a surface can exist at all.
        let attach_result = {
            // SAFETY: `state_ptr` is valid and no other reference to it
            // exists yet — `wndproc` only reads it starting with the
            // `WM_NCCREATE` this same `CreateWindowExW` call already
            // dispatched (synchronously, before returning), and that
            // path only stores the pointer in `GWLP_USERDATA`, never
            // dereferences it.
            let ws: &WindowState<A> = unsafe { &*state_ptr };
            ws.state.borrow_mut().backend.attach_surface(hwnd)
        };
        if let Err(e) = attach_result {
            // SAFETY: `state_ptr` must stay valid until `DestroyWindow`
            // returns. `DestroyWindow` synchronously re-enters `wndproc`
            // on this thread (at minimum for `WM_DESTROY`/`WM_NCDESTROY`,
            // potentially others), and `wndproc` unconditionally
            // dereferences `GWLP_USERDATA` before matching on `msg` — so
            // freeing the box first would leave that pointer dangling
            // for every message `DestroyWindow` generates, not just the
            // ones that read `state`. `DestroyWindow` completes its
            // entire synchronous dispatch before returning — the same
            // guarantee the normal-shutdown path below relies on when it
            // frees `state_ptr` only after the message loop has exited —
            // so freeing it here, after `DestroyWindow` returns, is safe.
            let _ = unsafe { DestroyWindow(hwnd) };
            drop(unsafe { Box::from_raw(state_ptr) });
            return Err(e);
        }

        let _ = unsafe { ShowWindow(hwnd, SW_SHOW) };
        // Forces the first `WM_PAINT` synchronously rather than waiting
        // for it to reach the front of the message queue, so "opens a
        // Win32 window with a cleared Direct2D surface" (#19's second
        // acceptance criterion) is true by the time this function
        // returns control to the message loop below, not just
        // eventually.
        let _ = unsafe { UpdateWindow(hwnd) };

        // #702: arm the one-shot smoke-check timer, if enabled. See the
        // module doc's "Headless smoke mode" section and
        // `run_smoke_check`'s `WM_TIMER` handler in `wndproc` below.
        {
            // SAFETY: same as the `attach_surface` borrow above —
            // `state_ptr` is valid and not concurrently referenced here.
            let ws: &WindowState<A> = unsafe { &*state_ptr };
            if let Some(cfg) = &ws.smoke {
                unsafe {
                    SetTimer(Some(hwnd), SMOKE_TIMER_ID, cfg.after_ms as u32, None);
                }
            }
        }

        let mut msg = MSG::default();
        loop {
            // `GetMessageW` returns `-1` on error, `0` on `WM_QUIT`,
            // nonzero otherwise — `BOOL` isn't `bool` here, hence the
            // explicit `.0` comparison instead of a truthiness check.
            let ret = unsafe { GetMessageW(&mut msg, None, 0, 0) }.0;
            if ret <= 0 {
                break;
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        // SAFETY: `WM_DESTROY`'s handler (below) is the only other place
        // that could still be running, and it doesn't touch `state_ptr`
        // — by the time `GetMessageW` returns `WM_QUIT` the window is
        // already gone, so there is no `wndproc` invocation left that
        // could observe this free.
        drop(unsafe { Box::from_raw(state_ptr) });
        // #702: read *after* freeing `state_ptr` — `smoke_failed` is the
        // `Rc<Cell<bool>>` clone taken before `Box::into_raw` above, so
        // it's still valid (the `Rc`'s count just dropped by one, not to
        // zero) even though `WindowState` itself is gone.
        Ok(!smoke_failed.get())
    }

    /// Dispatch `event` through [`super::dispatch_event`] (the shared
    /// pre-processing funnel — ActivityBar keyboard-focus redirect,
    /// global accelerator matching, then `app.handle`, quadraui#707),
    /// then honour the returned outcome: a redraw invalidates the whole
    /// client area so the next message-loop iteration repaints via
    /// `WM_PAINT`. Exit is the caller's responsibility (each call site
    /// below decides what "exit" means for its own message: `WM_CLOSE`
    /// destroys the window, letting `WM_DESTROY` post the quit message
    /// that actually ends `run_inner`'s loop).
    ///
    /// Routes the `state.borrow_mut()` that spans `super::dispatch_event`
    /// (the shared pre-processing funnel, #707) through
    /// `super::guarded_call(&ws.state, &ws.pump_depth, ...)` (#702,
    /// following up on #498, closing the hazard this comment used to
    /// only document): no bootstrap-era `AppLogic` (#19) pumps messages
    /// synchronously, but a future impl that shows a native modal or
    /// `SendMessage`s its own `hwnd` from inside `handle` would
    /// synchronously re-enter `wndproc` on this same thread while this
    /// borrow is still live. `guarded_call` makes that reentrant call
    /// see `ws.pump_depth.is_pumping()` and no-op — via `wndproc`'s own
    /// `is_pumping()` check before it ever reaches here — instead of
    /// panicking on a double `borrow_mut()`. When that happens, `dispatch`
    /// returns `Reaction::Continue` for the dropped message: no redraw,
    /// no exit, matching every other message this `wndproc` doesn't
    /// otherwise handle.
    fn dispatch<A: AppLogic>(ws: &WindowState<A>, hwnd: HWND, event: UiEvent) -> Reaction {
        let outcome = super::guarded_call(&ws.state, &ws.pump_depth, |run_state| {
            let RunState { app, backend } = run_state;
            super::dispatch_event(event, backend, app)
        });
        let Some(outcome) = outcome else {
            return Reaction::Continue;
        };
        let reaction = match outcome {
            super::EventOutcome::Continue => Reaction::Continue,
            super::EventOutcome::Redraw => Reaction::Redraw,
            super::EventOutcome::Exit => Reaction::Exit,
        };
        if reaction == Reaction::Redraw {
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
        reaction
    }

    /// #702's headless-smoke check, fired once by [`wndproc`]'s
    /// `WM_TIMER` handler `cfg.after_ms` after the window is shown — see
    /// the module doc's "Headless smoke mode" section. No-ops (does
    /// nothing, sets nothing) if `ws.smoke` is `None`, though in practice
    /// the `WM_TIMER` handler never fires without it (no timer is ever
    /// armed unless `ws.smoke.is_some()`).
    ///
    /// #619-style exemption (mirrors `gtk::run::schedule_smoke_check`'s
    /// identical one): this *is* the headless smoke harness's own
    /// diagnostic output, opt-in via `QUADRAUI_WIN_SMOKE_MS`, never
    /// reached by a host embedding a live quadraui backend — so printing
    /// here doesn't fight the crate-wide `print_stderr` deny's actual
    /// purpose (keeping a *library* silent by default).
    #[allow(clippy::print_stderr)]
    fn run_smoke_check<A: AppLogic>(ws: &WindowState<A>, hwnd: HWND) {
        let Some(cfg) = ws.smoke.as_ref() else {
            return;
        };

        let mut rect = RECT::default();
        let (width, height) = unsafe {
            let _ = GetClientRect(hwnd, &mut rect);
            (rect.right - rect.left, rect.bottom - rect.top)
        };
        if !smoke_size_ok(width, height, SMOKE_MIN_WIDTH, SMOKE_MIN_HEIGHT) {
            eprintln!(
                "quadraui smoke: client area size looks broken ({width}x{height}px, expected \
                 at least {SMOKE_MIN_WIDTH}x{SMOKE_MIN_HEIGHT}px) — this is the quadraui#437 \
                 tiny-window regression class"
            );
            ws.smoke_failed.set(true);
        }

        if let Some(text) = &cfg.paste_text {
            let read_back = {
                let state = ws.state.borrow();
                let clipboard = state.backend.services().clipboard();
                clipboard.write_text(text);
                clipboard.read_text()
            };
            if !smoke_clipboard_round_trip_ok(text, read_back.as_deref()) {
                eprintln!(
                    "quadraui smoke: OS clipboard round-trip failed — wrote {text:?}, read back \
                     {read_back:?}"
                );
                ws.smoke_failed.set(true);
            } else {
                // #728: also exercise the real Ctrl-V interception path
                // (the exact code `WM_CHAR`'s live dispatch calls) —
                // mirrors `gtk::run::schedule_smoke_check`'s identical
                // follow-up dispatch. Note: the return value below isn't
                // inspected, so this alone can't distinguish "recognised
                // as paste" from "fell through to `app.handle`
                // unchanged" — it only guards against a panic in the
                // interception path itself. The predicate's actual
                // matching behavior is covered by `crate::desktop`'s
                // pure-predicate unit tests, not here.
                dispatch(
                    ws,
                    hwnd,
                    UiEvent::KeyPressed {
                        key: Key::Char('v'),
                        modifiers: Modifiers {
                            ctrl: true,
                            shift: false,
                            alt: false,
                            cmd: false,
                        },
                        repeat: false,
                    },
                );
            }
        }
    }

    /// The Win32 window procedure. Monomorphized once per concrete `A`
    /// (see module docs) so `RegisterClassExW` gets a real function
    /// pointer despite `run` being generic.
    ///
    /// # Safety
    ///
    /// Called only by Windows via `DispatchMessageW`/directly during
    /// `CreateWindowExW` (`WM_NCCREATE`), per the standard `WNDPROC`
    /// contract. `GWLP_USERDATA` is trusted to hold either `0` (no state
    /// yet — every message before `WM_NCCREATE` sets it) or a valid
    /// `*const WindowState<A>` that outlives every message this function
    /// will ever receive for this `hwnd` (guaranteed by `run_inner`
    /// freeing it only after the message loop — driven by this same
    /// `hwnd` — has already exited).
    unsafe extern "system" fn wndproc<A: AppLogic + 'static>(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_NCCREATE {
            // SAFETY: `WM_NCCREATE`'s `lparam` always points at a live
            // `CREATESTRUCTW` for the duration of this call — that's the
            // OS's own contract for this message.
            let cs = lparam.0 as *const CREATESTRUCTW;
            if !cs.is_null() {
                let create_params = unsafe { (*cs).lpCreateParams };
                unsafe {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, create_params as isize);
                }
            }
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        }

        let state_ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const WindowState<A>;
        if state_ptr.is_null() {
            // Messages Windows can send before `WM_NCCREATE` populates
            // `GWLP_USERDATA` (rare, but the contract allows it) — no
            // app/backend to dispatch to yet.
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        }
        // SAFETY: see this function's contract above.
        let ws: &WindowState<A> = unsafe { &*state_ptr };

        // #702: a message re-entering `wndproc` while a `super::
        // guarded_call` borrow (`dispatch`'s `app.handle`, or `WM_PAINT`'s
        // `app.render`, below) is already live on this same thread — see
        // `super::guarded_call`'s and `dispatch`'s docs for the hazard
        // this closes. Cede to the default window proc rather than ever
        // touching `ws.state` while that's a live possibility; the next
        // non-reentrant message (once the outer guarded call returns)
        // handles normally.
        if ws.pump_depth.is_pumping() {
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        }

        match msg {
            WM_SIZE => {
                // `lparam`'s low/high words are the new client width/height
                // in pixels — the standard `WM_SIZE` payload shape.
                let (width, height) = size_from_lparam(lparam.0);
                let viewport = {
                    let mut s = ws.state.borrow_mut();
                    // Recreate the render target first if a prior
                    // `EndDraw` failure (see `backend.rs`'s `end_frame`)
                    // dropped it — `resize_surface` alone is a no-op on
                    // the render-target side while `surface` is `None`.
                    let _ = s.backend.ensure_surface();
                    // Best-effort: a failed resize (device lost mid-drag)
                    // leaves the old-sized target in place for this
                    // frame; the next `WM_PAINT`/`WM_SIZE` tries
                    // `ensure_surface` again.
                    let _ = s.backend.resize_surface(width, height);
                    s.backend.viewport()
                };
                dispatch(ws, hwnd, UiEvent::WindowResized { viewport });
                unsafe {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_DPICHANGED => {
                // `wparam`'s low word is the new DPI (`x` and `y` are
                // always equal on Windows) — see `WM_DPICHANGED`'s docs.
                let scale = dpi_scale_from_wparam(wparam.0);
                {
                    let mut s = ws.state.borrow_mut();
                    s.backend.set_dpi_scale(scale);
                }
                // `lparam` points at Windows' suggested new window rect
                // for the new DPI — applying it keeps the window's
                // *physical* on-screen size roughly constant across the
                // monitor change instead of staying pinned to its old
                // pixel dimensions (which would now be the wrong physical
                // size on the new monitor).
                let suggested = lparam.0 as *const RECT;
                if !suggested.is_null() {
                    let r = unsafe { *suggested };
                    unsafe {
                        let _ = SetWindowPos(
                            hwnd,
                            None,
                            r.left,
                            r.top,
                            r.right - r.left,
                            r.bottom - r.top,
                            SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                    }
                }
                dispatch(ws, hwnd, UiEvent::DpiChanged(scale));
                LRESULT(0)
            }
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN => {
                if let Some(button) = win_mouse_button_for_message(msg, wparam.0) {
                    let (x, y) = point_from_lparam(lparam.0);
                    let scale = ws.state.borrow().backend.viewport().scale;
                    let modifiers = win_key_modifiers();
                    dispatch(ws, hwnd, win_button_down(button, x, y, scale, modifiers));
                }
                // `WM_XBUTTONDOWN`/`WM_XBUTTONUP` are the one pair in this
                // group whose docs require returning `TRUE` when handled —
                // every other message here (and everywhere else in this
                // `wndproc`) wants `0`.
                if msg == WM_XBUTTONDOWN {
                    LRESULT(1)
                } else {
                    LRESULT(0)
                }
            }
            WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP | WM_XBUTTONUP => {
                if let Some(button) = win_mouse_button_for_message(msg, wparam.0) {
                    let (x, y) = point_from_lparam(lparam.0);
                    let scale = ws.state.borrow().backend.viewport().scale;
                    dispatch(ws, hwnd, win_button_up(button, x, y, scale));
                }
                if msg == WM_XBUTTONUP {
                    LRESULT(1)
                } else {
                    LRESULT(0)
                }
            }
            WM_MOUSEMOVE => {
                let (x, y) = point_from_lparam(lparam.0);
                let scale = ws.state.borrow().backend.viewport().scale;
                let buttons = ButtonMask {
                    left: wparam.0 & MK_LBUTTON != 0,
                    right: wparam.0 & MK_RBUTTON != 0,
                    middle: wparam.0 & MK_MBUTTON != 0,
                };
                dispatch(ws, hwnd, win_mouse_moved(x, y, scale, buttons));
                LRESULT(0)
            }
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                // Unlike every other mouse message, `WM_MOUSEWHEEL`'s
                // `lparam` carries **screen** coordinates — `ScreenToClient`
                // converts before handing off to `win_wheel_to_uievent`,
                // which (like every other translator here) expects
                // client-area pixels. See `super::events`' module docs.
                let raw_delta = wheel_delta_from_wparam(wparam.0);
                let (screen_x, screen_y) = point_from_lparam(lparam.0);
                let mut pt = POINT {
                    x: screen_x as i32,
                    y: screen_y as i32,
                };
                unsafe {
                    let _ = ScreenToClient(hwnd, &mut pt);
                }
                let scale = ws.state.borrow().backend.viewport().scale;
                let event = win_wheel_to_uievent(
                    raw_delta,
                    pt.x as i16,
                    pt.y as i16,
                    scale,
                    msg == WM_MOUSEHWHEEL,
                );
                dispatch(ws, hwnd, event);
                LRESULT(0)
            }
            WM_KEYDOWN => {
                // `WM_KEYUP` isn't separately dispatched: quadraui's
                // `UiEvent` has no key-release variant (mirroring the GTK
                // translator, which only wires `key-press-event`) — a
                // future release-tracking need should add that variant
                // rather than inventing one here. `TranslateMessage` in
                // `run_inner`'s message loop reads the raw `WM_KEYDOWN`
                // from the queue regardless of what this arm returns, so
                // intercepting it here doesn't suppress the `WM_CHAR` it
                // generates for printable keys.
                // Truncate rather than trust the upper `WPARAM` bits are
                // zero — a virtual-key code is always a single byte, same
                // masking discipline `super::msg`'s decoders use for
                // `LPARAM`.
                let vk = (wparam.0 & 0xFF) as u32;
                let repeat = is_repeat_from_lparam(lparam.0);
                let modifiers = win_key_modifiers();
                if let Some(event) = wm_keydown_to_uievent(vk, modifiers, repeat) {
                    dispatch(ws, hwnd, event);
                }
                LRESULT(0)
            }
            WM_CHAR => {
                // `wparam`'s low word is the UTF-16 code unit
                // `TranslateMessage` resolved through the active keyboard
                // layout. Surrogate-pair characters (outside the BMP) are
                // dropped here — `char::from_u32` returns `None` for a
                // lone surrogate half — same scope boundary
                // `wm_char_to_uievent`'s docs describe.
                let repeat = is_repeat_from_lparam(lparam.0);
                // Truncate to the 16-bit code unit before widening — same
                // reasoning as `WM_KEYDOWN`'s `vk` above.
                if let Some(c) = char::from_u32((wparam.0 & 0xFFFF) as u32) {
                    let modifiers = win_key_modifiers();
                    if let Some(event) = wm_char_to_uievent(c, modifiers, repeat) {
                        dispatch(ws, hwnd, event);
                    }
                }
                LRESULT(0)
            }
            WM_SETFOCUS => {
                dispatch(ws, hwnd, win_focus_to_uievent(true));
                LRESULT(0)
            }
            WM_KILLFOCUS => {
                dispatch(ws, hwnd, win_focus_to_uievent(false));
                LRESULT(0)
            }
            WM_PAINT => {
                // Routed through `super::guarded_call` for the same
                // reason as `dispatch` (#702): `app.render` can hit the
                // same synchronous-reentrancy hazard `app.handle` can.
                // The top-of-`wndproc` `is_pumping()` check above already
                // guarantees `WM_PAINT` itself is never the *reentrant*
                // call, but `guarded_call` still has to be the one
                // holding `ws.pump_depth`'s guard for the duration of
                // `app.render` — otherwise a nested pump triggered *from
                // inside* `render` wouldn't be caught by that same check.
                let _ = super::guarded_call(&ws.state, &ws.pump_depth, |run_state| {
                    // Recreate the render target if a prior `EndDraw`
                    // failure dropped it (device lost, RDP session
                    // change — see `backend.rs`'s `end_frame` docs).
                    // Best-effort: if recreation fails again (surface
                    // still unavailable), `begin_frame`/`end_frame`
                    // below are no-ops while `surface` stays `None`, and
                    // the next `WM_PAINT`/`WM_SIZE` tries again.
                    let _ = run_state.backend.ensure_surface();
                    let viewport = run_state.backend.viewport();
                    let RunState { app, backend } = run_state;
                    super::render_frame(backend, app, viewport);
                });
                // Direct2D draws straight to the swap chain via the
                // `ID2D1HwndRenderTarget` — no GDI `HDC`/`PAINTSTRUCT`
                // involved, so this validates the update region directly
                // instead of the usual `BeginPaint`/`EndPaint` pair (the
                // standard pattern for D2D-only `WM_PAINT` handlers).
                unsafe {
                    let _ = ValidateRect(Some(hwnd), None);
                }
                LRESULT(0)
            }
            WM_SETCURSOR => {
                // #702: Windows asks "what cursor belongs here?" via this
                // message on every mouse move over the window, including
                // moves that never reach `WM_MOUSEMOVE` (e.g. the pointer
                // re-entering after a click elsewhere). The low word of
                // `lparam` is the hit-test code from the preceding
                // `WM_NCHITTEST`; only handle the client area
                // (`HTCLIENT`) ourselves and re-apply
                // `WinBackend::current_pointer_shape` there — everywhere
                // else (the resize border, titlebar, …) is native
                // `WS_OVERLAPPEDWINDOW` chrome, so `DefWindowProcW`
                // already draws the right cursor for it and must stay in
                // control.
                let hit_test = (lparam.0 as usize) & 0xFFFF;
                if hit_test == HTCLIENT as usize {
                    ws.state.borrow().backend.apply_current_cursor();
                    LRESULT(1)
                } else {
                    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
                }
            }
            WM_TIMER => {
                // #702: the one-shot headless-smoke timer armed by
                // `run_inner`, if `QUADRAUI_WIN_SMOKE_MS` was set — see
                // the module doc's "Headless smoke mode" section. Any
                // other timer id is none of this runner's business (no
                // `AppLogic` today owns one, but a future one might) and
                // falls through to `DefWindowProcW` untouched.
                if wparam.0 == SMOKE_TIMER_ID {
                    unsafe {
                        let _ = KillTimer(Some(hwnd), SMOKE_TIMER_ID);
                    }
                    run_smoke_check(ws, hwnd);
                    unsafe {
                        let _ = DestroyWindow(hwnd);
                    }
                    LRESULT(0)
                } else {
                    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
                }
            }
            WM_CLOSE => {
                if dispatch(ws, hwnd, UiEvent::WindowClose) == Reaction::Exit {
                    unsafe {
                        let _ = DestroyWindow(hwnd);
                    }
                }
                // An app returning `Reaction::Continue`/`Redraw` here
                // vetoes the close, matching the GTK runner's
                // `Reaction::Exit => window.close()` — every other
                // reaction leaves the window open.
                LRESULT(0)
            }
            WM_DESTROY => {
                unsafe {
                    PostQuitMessage(0);
                }
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }
}

#[cfg(target_os = "windows")]
pub fn run<A: AppLogic + 'static>(app: A) -> std::process::ExitCode {
    win32::run(app, RunConfig::default())
}

/// Like [`run`], but with a custom window title via [`RunConfig`] instead
/// of the hardcoded `"quadraui"` default. `run(app)` is equivalent to
/// `run_with(app, RunConfig::default())`.
#[cfg(target_os = "windows")]
pub fn run_with<A: AppLogic + 'static>(app: A, config: RunConfig) -> std::process::ExitCode {
    win32::run(app, config)
}

#[cfg(not(target_os = "windows"))]
pub fn run<A: AppLogic + 'static>(_app: A) -> std::process::ExitCode {
    todo!(
        "Win32 message loop: RegisterClassEx, CreateWindowEx, \
         Direct2D render target, translate WM_* → UiEvent, \
         dispatch to app.handle(), redraw via app.render()"
    )
}

/// Like [`run`], but with a custom window title via [`RunConfig`] — see
/// [`run`]'s non-Windows stub for why this is `todo!()` off Windows too.
#[cfg(not(target_os = "windows"))]
pub fn run_with<A: AppLogic + 'static>(_app: A, _config: RunConfig) -> std::process::ExitCode {
    todo!(
        "Win32 message loop: RegisterClassEx, CreateWindowEx, \
         Direct2D render target, translate WM_* → UiEvent, \
         dispatch to app.handle(), redraw via app.render() — with a \
         custom window title from RunConfig"
    )
}

// `RunConfig` itself is deliberately not `target_os = "windows"`-gated
// (see its doc comment), so these run on every host — including plain
// `cargo test -p quadraui --features win` on Linux, same as the rest of
// this crate's `win` compile/test gate. Mirrors `gtk::run`'s
// `run_config_tests` module.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_sets_the_title() {
        let config = RunConfig::new("kubeui");
        assert_eq!(config.title, "kubeui");
    }

    #[test]
    fn new_accepts_owned_and_borrowed_strings() {
        assert_eq!(RunConfig::new("borrowed").title, "borrowed");
        assert_eq!(RunConfig::new(String::from("owned")).title, "owned");
    }

    #[test]
    fn default_matches_runs_previously_hardcoded_title() {
        // `run(app)` used to always show a `"quadraui\0"`-derived window
        // title; `RunConfig::default()` must reproduce that exact value
        // so `run(app)` staying `run_with(app, RunConfig::default())`
        // (see both functions above) doesn't change existing behaviour.
        assert_eq!(RunConfig::default().title, "quadraui");
    }

    /// Reproduces the exact hazard `win32::dispatch`'s doc comment used
    /// to only describe as a future risk: a synchronous reentrant call
    /// into [`guarded_call`] while the outer call's `state.borrow_mut()`
    /// is still live. Without the `pump_depth.is_pumping()` check, the
    /// inner `state.borrow_mut()` would panic ("already mutably
    /// borrowed"); with it, the inner call is skipped and the outer one
    /// completes normally — this is the "test that re-enters wndproc ...
    /// and does not panic" #702 asks for, at the nearest seam that's
    /// testable without a live Win32 message loop (`wndproc` itself only
    /// compiles under `target_os = "windows"`; this doesn't).
    #[test]
    fn reentrant_call_is_skipped_not_double_borrowed() {
        let state = RefCell::new(0i32);
        let depth = ModalPumpDepth::new();
        let outer = guarded_call(&state, &depth, |v| {
            *v += 1;
            // Simulate `wndproc` re-entering while this closure — the
            // stand-in for `app.handle`/`app.render` — is still running
            // with `state`'s `RefCell` mutably borrowed.
            let inner = guarded_call(&state, &depth, |v2| {
                *v2 += 100;
                *v2
            });
            assert_eq!(
                inner, None,
                "a reentrant guarded_call must be skipped, not run"
            );
            *v
        });
        assert_eq!(
            outer,
            Some(1),
            "the outer call must still complete normally"
        );
        assert_eq!(
            *state.borrow(),
            1,
            "the skipped reentrant call must not have mutated state"
        );
    }

    #[test]
    fn sequential_non_reentrant_calls_both_run() {
        let state = RefCell::new(0i32);
        let depth = ModalPumpDepth::new();
        assert_eq!(
            guarded_call(&state, &depth, |v| {
                *v += 1;
                *v
            }),
            Some(1)
        );
        assert_eq!(
            guarded_call(&state, &depth, |v| {
                *v += 1;
                *v
            }),
            Some(2)
        );
        assert!(
            !depth.is_pumping(),
            "depth must return to 0 once each guarded call completes"
        );
    }
}

/// Coverage for #728 — Ctrl-V/Ctrl-Shift-V clipboard-paste interception in
/// [`dispatch_event`]. `dispatch_event` itself is deliberately not
/// `target_os`-gated (see its doc comment), so this runs on every host,
/// including plain `cargo test --features win` on Linux (`win::testing`'s
/// `WinDriver`, by contrast, only exists under `target_os = "windows"` —
/// see that module's doc — so it can't be used here).
///
/// **Which assertions are host-independent, and why that matters here.**
/// `WinPlatformServices::clipboard()` has no test seam (unlike
/// `gtk::testing::GtkDriver::new`'s in-memory fake): `read_text()` is an
/// unconditional `None` off Windows and a *real, uncontrolled* Win32
/// `CF_UNICODETEXT` read on Windows — the machine's live systemwide
/// clipboard, whatever some other process happened to leave on it. So
/// "the app received nothing at all" is a claim only the non-Windows
/// hosts can make, and asserting it unconditionally makes the outcome of
/// this test depend on CI-runner clipboard state nothing in this repo
/// owns. The tests below are split accordingly:
///
/// - `ctrl_v_is_never_forwarded_to_the_app_as_a_raw_keypress` runs on
///   every host and asserts the invariant #728 actually promises and
///   that holds for *both* clipboard branches: once `is_paste_keypress`
///   recognises the chord, the raw `KeyPressed` is consumed — anything
///   the app does see is a `ClipboardPaste`, never the key event.
/// - `ctrl_v_with_nothing_to_paste_is_consumed_not_forwarded` is gated
///   to non-Windows, where "nothing to paste" is a compile-time property
///   of `WinClipboard::read_text` rather than a guess about the host.
/// - `ctrl_v_delivers_real_clipboard_text_as_clipboard_paste` is gated to
///   Windows and seeds the clipboard first, so it covers the "reads real
///   clipboard text and delivers `ClipboardPaste`" branch for real on the
///   one CI leg that can — see `CLAUDE.md`'s "Win-GUI: building and
///   testing for real".
#[cfg(test)]
mod paste_dispatch_tests {
    use super::*;
    use crate::runner::Reaction;
    use crate::{Key, Modifiers, UiEvent};

    /// Payload for the Windows-only round-trip below. Distinctive enough
    /// that a stale clipboard entry can't be mistaken for it.
    #[cfg(target_os = "windows")]
    const PASTE_PAYLOAD: &str = "quadraui#728 ctrl-v round trip";

    /// Records every event `handle` receives, so tests can assert an
    /// intercepted paste trigger does NOT also forward the raw key event
    /// underneath it — mirrors `gtk::run::paste_tests::RecordingApp`.
    #[derive(Default)]
    struct RecordingApp {
        events: Vec<UiEvent>,
    }

    impl AppLogic for RecordingApp {
        type AreaId = ();

        fn render(&self, _backend: &mut dyn Backend, _area: ()) {}

        fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            self.events.push(event);
            Reaction::Continue
        }
    }

    fn ctrl_v() -> UiEvent {
        UiEvent::KeyPressed {
            key: Key::Char('v'),
            modifiers: Modifiers {
                ctrl: true,
                shift: false,
                alt: false,
                cmd: false,
            },
            repeat: false,
        }
    }

    /// The one paste-interception claim that is true on every host, with
    /// any clipboard contents: `dispatch_event` swallows the recognised
    /// chord. Whether the clipboard had text (Windows, maybe) or not
    /// (everywhere else, always) only changes *what else* the app sees —
    /// a `ClipboardPaste`, or nothing — never whether the raw key event
    /// leaks through underneath it.
    #[test]
    fn ctrl_v_is_never_forwarded_to_the_app_as_a_raw_keypress() {
        let mut backend = WinBackend::new();
        let mut app = RecordingApp::default();
        let _ = dispatch_event(ctrl_v(), &mut backend, &mut app);
        assert!(
            app.events
                .iter()
                .all(|e| matches!(e, UiEvent::ClipboardPaste(_))),
            "once is_paste_keypress recognises the chord, the raw Ctrl-V \
             KeyPressed must never reach the app — the only thing that may \
             is a ClipboardPaste; got {:?}",
            app.events
        );
    }

    /// The "nothing to paste" branch, asserted only where it is a
    /// compile-time property rather than a guess about the host:
    /// `WinClipboard::read_text` is an unconditional `None` off Windows
    /// (`win::services`'s `Clipboard` impl), so the app must see nothing
    /// at all and the outcome must be `Continue`.
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn ctrl_v_with_nothing_to_paste_is_consumed_not_forwarded() {
        let mut backend = WinBackend::new();
        let mut app = RecordingApp::default();
        let outcome = dispatch_event(ctrl_v(), &mut backend, &mut app);
        assert!(
            matches!(outcome, EventOutcome::Continue),
            "an intercepted paste keypress with an empty clipboard must \
             resolve to Continue, not fall through to app.handle"
        );
        assert!(
            app.events.is_empty(),
            "the raw Ctrl-V KeyPressed must never reach the app once \
             is_paste_keypress recognises it — got {:?}",
            app.events
        );
    }

    /// The branch that only a live Windows host can reach: a real
    /// `CF_UNICODETEXT` clipboard read feeding `UiEvent::ClipboardPaste`.
    /// Seeds the clipboard itself rather than assuming what is on it, so
    /// this asserts on state the test owns.
    ///
    /// Self-guarding: if the round-trip write→read doesn't come back
    /// intact, this host's clipboard is unavailable to the process
    /// (`OpenClipboard` fails on a non-interactive window station, for
    /// one), which is a property of the environment and not of
    /// `dispatch_event` — there is nothing left for the assertions below
    /// to prove, so say so on stderr and stop rather than reporting an
    /// interception bug that isn't there. The
    /// `ctrl_v_is_never_forwarded_to_the_app_as_a_raw_keypress` test
    /// above still covers the interception path unconditionally on this
    /// same host.
    ///
    /// `#[allow(clippy::print_stderr)]`: same #619-style exemption
    /// `run_smoke_check` above carries — this is test-harness diagnostic
    /// output, never reached by a host embedding a live quadraui backend,
    /// so it doesn't fight the crate-wide deny's purpose of keeping the
    /// *library* silent.
    #[cfg(target_os = "windows")]
    #[test]
    #[allow(clippy::print_stderr)]
    fn ctrl_v_delivers_real_clipboard_text_as_clipboard_paste() {
        let mut backend = WinBackend::new();
        backend.services().clipboard().write_text(PASTE_PAYLOAD);
        let seeded = backend.services().clipboard().read_text();
        if seeded.as_deref() != Some(PASTE_PAYLOAD) {
            eprintln!(
                "quadraui: skipping ctrl_v_delivers_real_clipboard_text_as_clipboard_paste — \
                 this host's clipboard is not usable from the test process (wrote \
                 {PASTE_PAYLOAD:?}, read back {seeded:?})"
            );
            return;
        }

        let mut app = RecordingApp::default();
        let _ = dispatch_event(ctrl_v(), &mut backend, &mut app);
        assert_eq!(
            app.events,
            vec![UiEvent::ClipboardPaste(PASTE_PAYLOAD.to_string())],
            "Ctrl-V with text on the clipboard must deliver exactly one \
             ClipboardPaste carrying that text, and no raw KeyPressed"
        );
    }

    #[test]
    fn non_paste_keypress_still_reaches_the_app() {
        let mut backend = WinBackend::new();
        let mut app = RecordingApp::default();
        let ev = UiEvent::KeyPressed {
            key: Key::Char('x'),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let _ = dispatch_event(ev.clone(), &mut backend, &mut app);
        assert_eq!(
            app.events,
            vec![ev],
            "a non-paste keypress must fall through to app.handle unchanged"
        );
    }
}
