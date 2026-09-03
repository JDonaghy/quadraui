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

use crate::backend::Backend;
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
///
/// Anything not matched above falls through to `app.handle` unchanged.
///
/// Not `target_os`-gated: neither `WinBackend::focused_activity_bar_id`
/// nor `WinBackend::match_keypress` touch Direct2D/Win32 directly (they
/// read plain `Option`/`Vec` fields populated by `register_accelerator`
/// and `draw_activity_bar`), so this compiles and behaves identically on
/// every host — same "compiles everywhere" posture as [`RunConfig`]
/// above.
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
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::mem::size_of;

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
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
        GetWindowLongPtrW, LoadCursorW, PostQuitMessage, RegisterClassExW, SetWindowLongPtrW,
        SetWindowPos, ShowWindow, TranslateMessage, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW,
        CW_USEDEFAULT, GWLP_USERDATA, IDC_ARROW, MSG, SWP_NOACTIVATE, SWP_NOZORDER, SW_SHOW,
        WINDOW_EX_STYLE, WM_CHAR, WM_CLOSE, WM_DESTROY, WM_DPICHANGED, WM_KEYDOWN, WM_KILLFOCUS,
        WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE,
        WM_MOUSEWHEEL, WM_NCCREATE, WM_PAINT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETFOCUS, WM_SIZE,
        WM_XBUTTONDOWN, WM_XBUTTONUP, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
    };

    use crate::backend::Backend;
    use crate::desktop::ModalPumpDepth;
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
    use crate::{ButtonMask, Modifiers};

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
            Ok(()) => std::process::ExitCode::SUCCESS,
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
    unsafe fn run_inner<A: AppLogic + 'static>(
        mut app: A,
        title: &str,
    ) -> windows::core::Result<()> {
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

        let state_ptr: *mut WindowState<A> = Box::into_raw(Box::new(WindowState {
            state: RefCell::new(RunState { app, backend }),
            pump_depth: ModalPumpDepth::new(),
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
        Ok(())
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
