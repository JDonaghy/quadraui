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
//! `app`/`backend` as a closure. [`RunState`] is heap-allocated once via
//! `Box::into_raw` *before* `CreateWindowExW` runs, and its address is
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

use crate::runner::AppLogic;

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

        let state_ptr: *mut RefCell<RunState<A>> =
            Box::into_raw(Box::new(RefCell::new(RunState { app, backend })));

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
            let state: &RefCell<RunState<A>> = unsafe { &*state_ptr };
            state.borrow_mut().backend.attach_surface(hwnd)
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

    /// Dispatch `event` through `app.handle`, then honour the returned
    /// [`Reaction`]: [`Reaction::Redraw`] invalidates the whole client
    /// area so the next message-loop iteration repaints via `WM_PAINT`.
    /// [`Reaction::Exit`] is the caller's responsibility (each call site
    /// below decides what "exit" means for its own message: `WM_CLOSE`
    /// destroys the window, letting `WM_DESTROY` post the quit message
    /// that actually ends `run_inner`'s loop).
    ///
    /// Holds `state.borrow_mut()` for the duration of `app.handle`. No
    /// bootstrap-era `AppLogic` (#19) does anything that pumps messages
    /// synchronously, but a future impl that shows a native modal or
    /// `SendMessage`s its own `hwnd` from inside `handle` would re-enter
    /// `wndproc` on this same thread while this borrow is still live —
    /// a `RefCell` double-borrow panic. Worth revisiting once #20 lands
    /// real input handling and third-party `AppLogic` impls get more
    /// latitude.
    fn dispatch<A: AppLogic>(state: &RefCell<RunState<A>>, hwnd: HWND, event: UiEvent) -> Reaction {
        let reaction = {
            let mut state = state.borrow_mut();
            let RunState { app, backend } = &mut *state;
            app.handle(event, backend)
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
    /// `*const RefCell<RunState<A>>` that outlives every message this
    /// function will ever receive for this `hwnd` (guaranteed by
    /// `run_inner` freeing it only after the message loop — driven by
    /// this same `hwnd` — has already exited).
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

        let state_ptr =
            unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RefCell<RunState<A>>;
        if state_ptr.is_null() {
            // Messages Windows can send before `WM_NCCREATE` populates
            // `GWLP_USERDATA` (rare, but the contract allows it) — no
            // app/backend to dispatch to yet.
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        }
        // SAFETY: see this function's contract above.
        let state: &RefCell<RunState<A>> = unsafe { &*state_ptr };

        match msg {
            WM_SIZE => {
                // `lparam`'s low/high words are the new client width/height
                // in pixels — the standard `WM_SIZE` payload shape.
                let (width, height) = size_from_lparam(lparam.0);
                let viewport = {
                    let mut s = state.borrow_mut();
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
                dispatch(state, hwnd, UiEvent::WindowResized { viewport });
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
                    let mut s = state.borrow_mut();
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
                dispatch(state, hwnd, UiEvent::DpiChanged(scale));
                LRESULT(0)
            }
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN => {
                if let Some(button) = win_mouse_button_for_message(msg, wparam.0) {
                    let (x, y) = point_from_lparam(lparam.0);
                    let scale = state.borrow().backend.viewport().scale;
                    let modifiers = win_key_modifiers();
                    dispatch(state, hwnd, win_button_down(button, x, y, scale, modifiers));
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
                    let scale = state.borrow().backend.viewport().scale;
                    dispatch(state, hwnd, win_button_up(button, x, y, scale));
                }
                if msg == WM_XBUTTONUP {
                    LRESULT(1)
                } else {
                    LRESULT(0)
                }
            }
            WM_MOUSEMOVE => {
                let (x, y) = point_from_lparam(lparam.0);
                let scale = state.borrow().backend.viewport().scale;
                let buttons = ButtonMask {
                    left: wparam.0 & MK_LBUTTON != 0,
                    right: wparam.0 & MK_RBUTTON != 0,
                    middle: wparam.0 & MK_MBUTTON != 0,
                };
                dispatch(state, hwnd, win_mouse_moved(x, y, scale, buttons));
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
                let scale = state.borrow().backend.viewport().scale;
                let event = win_wheel_to_uievent(
                    raw_delta,
                    pt.x as i16,
                    pt.y as i16,
                    scale,
                    msg == WM_MOUSEHWHEEL,
                );
                dispatch(state, hwnd, event);
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
                    dispatch(state, hwnd, event);
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
                        dispatch(state, hwnd, event);
                    }
                }
                LRESULT(0)
            }
            WM_SETFOCUS => {
                dispatch(state, hwnd, win_focus_to_uievent(true));
                LRESULT(0)
            }
            WM_KILLFOCUS => {
                dispatch(state, hwnd, win_focus_to_uievent(false));
                LRESULT(0)
            }
            WM_PAINT => {
                {
                    let mut s = state.borrow_mut();
                    // Recreate the render target if a prior `EndDraw`
                    // failure dropped it (device lost, RDP session
                    // change — see `backend.rs`'s `end_frame` docs).
                    // Best-effort: if recreation fails again (surface
                    // still unavailable), `begin_frame`/`end_frame`
                    // below are no-ops while `surface` stays `None`, and
                    // the next `WM_PAINT`/`WM_SIZE` tries again.
                    let _ = s.backend.ensure_surface();
                    let viewport = s.backend.viewport();
                    s.backend.begin_frame(viewport);
                    let RunState { app, backend } = &mut *s;
                    app.render(backend, Default::default());
                    backend.end_frame();
                }
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
                if dispatch(state, hwnd, UiEvent::WindowClose) == Reaction::Exit {
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
    use super::RunConfig;

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
}
