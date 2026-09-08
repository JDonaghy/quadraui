//! GTK runner — drives a [`crate::AppLogic`] implementation against
//! [`GtkBackend`].
//!
//! The runner absorbs every per-app-but-not-app-logic boilerplate
//! piece for a basic single-`DrawingArea` GTK app:
//! - `Application` + `ApplicationWindow` + single `DrawingArea`
//!   construction.
//! - GTK main loop.
//! - `set_draw_func` wiring: enters [`GtkBackend::enter_frame_scope`]
//!   with the cairo context + pango layout and calls
//!   [`crate::AppLogic::render`] with the app's default
//!   [`AppLogic::AreaId`][crate::AppLogic::AreaId].
//! - Key / mouse / scroll / resize → [`crate::UiEvent`] translation
//!   pushed onto the backend's event queue, drained on each
//!   subsequent frame.
//! - [`crate::Reaction`] dispatch (Continue / Redraw / Exit).
//!
//! ## Single-DA model (decided: #217 Stage 1)
//!
//! The runner uses a **single-DrawingArea** model: one DA, one
//! `set_draw_func`, one `app.render(backend, AreaId::default())` per
//! redraw. Zone routing (sidebar, main, status bar, etc.) is handled
//! entirely by `AppShell::compute_layout` + `FrameHitMap` hit-testing
//! — not by multiple GTK DAs.
//!
//! This was a deliberate decision (#217). All vimcode paint paths
//! already go through quadraui primitives (vimcode#446), so per-zone
//! DAs add GTK widget-tree complexity without benefit. The
//! `AppLogic::AreaId` associated type remains in the trait as a
//! compatibility seam but is always `()` in practice.
//!
//! ## Shared with the headless test driver
//!
//! [`render_frame`] and [`dispatch_event`] are `pub(crate)` so the
//! in-process [`crate::gtk::testing::GtkDriver`] (quadraui#446, mirroring
//! quadraui#300's TUI split) renders + dispatches through the *exact
//! same* code as the live runner. The driver swaps the `DrawingArea`'s
//! live `cairo::Context` for one backed by a headless
//! `cairo::ImageSurface` and supplies scripted events instead of real
//! GDK signals — but the frame paint and the event pre-processing
//! (ActivityBar keyboard-focus intercept, Tab/Shift+Tab focus cycling
//! (#830), accelerator matching, Ctrl-C/
//! V/A interception, text-selection state) cannot drift, because every
//! GTK signal closure below routes through these same two functions.
//!
//! ## Headless smoke mode (quadraui#450, GD-5)
//!
//! [`GtkDriver`][crate::gtk::testing::GtkDriver] (above) is deliberately
//! display-free, so it structurally cannot catch bugs that only exist in
//! a real `Application` + `ApplicationWindow` + `GdkDisplay` — the exact
//! class that motivated this: quadraui#437 (`gtk_terminal` opening with a
//! tiny/garbled window, paste not working at all) only reproduced against
//! a live window.
//!
//! [`run`] honours two environment variables, read once at startup, so
//! any `gtk_*` example is xvfb-run-friendly with zero example-specific
//! code (every example already goes through this runner):
//!
//! - `QUADRAUI_GTK_SMOKE_MS=<u64>` — enables smoke mode. `after_ms`
//!   milliseconds after the window is presented, the runner checks the
//!   `DrawingArea`'s allocated size against a sane floor
//!   ([`smoke_size_ok`] — the direct #437 tiny-window regression check),
//!   then closes the window so an unattended process exits deterministically
//!   instead of hanging forever waiting for a user who isn't there.
//! - `QUADRAUI_GTK_SMOKE_PASTE=<text>` — optional. If set, the same timer
//!   round-trips `<text>` through the **real OS clipboard** (`arboard`,
//!   the same object the live Ctrl-V handler reads —
//!   `backend.services().clipboard()`) and, if that succeeds, dispatches
//!   a synthetic Ctrl-V `KeyPressed` through [`dispatch_event`] — the
//!   exact code path the live key controller calls — so a regression in
//!   the paste-interception wiring itself also fails the smoke, not just
//!   a raw clipboard failure. `arboard` needs a real `DISPLAY` (Xvfb
//!   provides one; the Broadway backend does not), which is why the
//!   operator-run wrapper (`quadraui/scripts/gtk_smoke.sh`) uses Xvfb.
//!
//! Any assertion failure is printed to stderr and flips [`run`]'s return
//! value to [`std::process::ExitCode::FAILURE`], overriding GLib's own
//! exit code — see the end of [`run`]. Disabled (zero runtime cost)
//! unless `QUADRAUI_GTK_SMOKE_MS` is set, so ordinary interactive
//! launches are unaffected.
//!
//! This mechanism can't be exercised in CI (the `gtk` CI job is
//! deliberately Xvfb-free — see `ci.yml`); it's the operator-run tier
//! `quadraui/docs/TESTING.md` documents as "live-app headless smoke".
//! The size/text assertion *logic* is unit-tested below with no display
//! required.
//!
//! ## Clipboard paste vs. IME/dead-key composition (quadraui#415)
//!
//! quadraui#415 ("route clipboard paste + IME/dead-key composition into
//! the focused terminal PTY") is two distinct input paths. Only the
//! clipboard-paste half — Ctrl-V, Ctrl-Shift-V, and middle-click PRIMARY,
//! all handled below in [`dispatch_event`] — is implemented by this
//! module. IME/dead-key composed input (e.g. a dead-key `´` followed by
//! `e` composing to `é`, or any real IME committing multi-keystroke text)
//! is **not** wired up: `EventControllerKey` here only ever sees raw,
//! already-resolved keysyms via `gdk_key_to_uievent`, with no
//! `gtk4::IMMulticontext` attached to intercept `key-press-event` first
//! and expose its `commit` / `preedit-changed` signals. That's a
//! deliberate scope split, not an oversight: no quadraui backend runs an
//! IME composition pipeline yet (see [`crate::UiEvent::CharTyped`]'s doc
//! comment), and epic quadraui#481 owns adding one across every backend,
//! not just GTK's terminal example. `examples/common/terminal_app.rs`'s
//! module doc carries the consumer-facing version of this note.
//!
//! ## Re-entrancy: two independent sources, one invariant (quadraui#902)
//!
//! Every signal closure below takes `backend.borrow_mut()` and
//! `app.borrow_mut()` together across a `dispatch_event` call. Two
//! different things can make that double-borrow — panicking inside a
//! non-unwindable `extern "C"` GLib trampoline frame, which aborts the
//! whole process rather than unwinding:
//!
//! 1. **A nested modal pump** (quadraui#427) — GTK's async
//!    `FileDialog`'s `MainContext::iteration(true)` wait services every
//!    other pending source, including this runner's own closures, while
//!    the code that started the pump still holds `backend`/`app`
//!    borrowed. Guarded by `pump_depth.is_pumping()`, checked first in
//!    every closure below.
//! 2. **A synchronous same-signal re-entry from app code** (quadraui#902)
//!    — app code, called from inside `dispatch_event`, invokes a GTK
//!    method that synchronously re-emits the very signal a handler below
//!    listens for. The reported case: `AppLogic::handle` calls
//!    `window.close()` from inside its own `WindowClose` handling;
//!    `gtk_window_close` emits `close-request` synchronously, re-entering
//!    `connect_close_request` below while the outer call still holds
//!    `backend`/`app`. `pump_depth` is `0` the entire time this happens —
//!    it only ever models source 1 — so it waves this straight through
//!    into a second `borrow_mut()` and the abort.
//!
//! Nothing about source 2 is specific to `close-request`: any handler
//! below aborts the same way if app code, mid-dispatch, triggers the
//! signal it listens for. The fix is structural rather than enumerative
//! — `try_borrow_both`/`try_dispatch`/`try_dispatch_with`/
//! `try_dispatch_borrowed` (defined near [`dispatch_event`], used
//! throughout) ask the `RefCell`s themselves whether it's safe to
//! proceed, which catches source 2 *and* any future re-entrancy source
//! nobody has enumerated yet, without needing a second depth counter.
//! Most handlers degrade by skipping the event, the same trade
//! `pump_depth.is_pumping()` already makes. `close-request` additionally
//! *defers* its event onto `events_handle` (a plain
//! `Rc<RefCell<VecDeque<UiEvent>>>` with borrow state independent of
//! `backend`'s, safe to push to even while `backend` is held) rather than
//! dropping it, because a dropped `WindowClose` would turn an
//! in-dispatch `close()` call into a silent, permanent no-op — see that
//! handler for the full rationale.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk4::cairo::Context;
use gtk4::glib;
use gtk4::glib::prelude::StaticType;
use gtk4::prelude::*;
use gtk4::{
    pango as pg, Application, ApplicationWindow, DrawingArea, EventControllerKey,
    EventControllerMotion, EventControllerScroll, EventControllerScrollFlags, GestureClick,
};
use pangocairo::functions as pcfn;

use super::backend::GtkBackend;
use super::events::{
    gdk_button_to_quadraui, gdk_key_to_uievent, gdk_modifiers_to_quadraui, gdk_resize_to_uievent,
    gdk_scroll_to_uievent_with_direction, gtk_drop_to_uievent,
};
use crate::backend::Backend;
use crate::desktop::{smoke_clipboard_round_trip_ok, smoke_size_ok, SmokeConfig};
use crate::dispatch::{dispatch_click, dispatch_mouse_drag, dispatch_mouse_up};
use crate::runner::{AppLogic, Reaction};
use crate::runtime::{self, ReactionSink, RESIZE_SETTLE};
// Re-exported (not just imported) so `gtk::testing` — and any other
// in-crate caller that historically reached `EventOutcome` through this
// module — keeps working unchanged after the type moved to
// `crate::runtime` (quadraui#496).
pub(crate) use crate::runtime::EventOutcome;
use crate::{ButtonMask, Key, Modifiers, MouseButton, Point, UiEvent};

/// Default window size, in DIPs. Matches the `ApplicationWindow`
/// builder's `default_width`/`default_height` below. Also used to seed
/// `GtkBackend`'s viewport *before* `app.setup()` runs, so
/// `Backend::viewport()` returns a sane, non-zero size to apps that read
/// it during setup (e.g. to size an embedded PTY — quadraui#437) instead
/// of `GtkBackend::new()`'s zeroed default. `DrawingArea::connect_resize`
/// (wired below) corrects this to the widget's *actual* allocated size
/// as soon as it's realized, so this seed only matters for the brief
/// window between `setup()` and the first resize signal.
const DEFAULT_WINDOW_WIDTH: i32 = 800;
const DEFAULT_WINDOW_HEIGHT: i32 = 600;

/// Minimum sane `DrawingArea` size for headless smoke mode (quadraui#450).
/// Comfortably below [`DEFAULT_WINDOW_WIDTH`]/[`DEFAULT_WINDOW_HEIGHT`] so
/// ordinary window-manager chrome/decoration insets don't false-positive,
/// but well above the ~8-character-wide wrapped column quadraui#437
/// actually produced.
const SMOKE_MIN_WIDTH: i32 = 200;
const SMOKE_MIN_HEIGHT: i32 = 150;

/// Env var enabling headless smoke mode (quadraui#450, GD-5) — see the
/// module doc's "Headless smoke mode" section. Fed to
/// [`crate::desktop::SmokeConfig::from_env`] below.
const SMOKE_MS_VAR: &str = "QUADRAUI_GTK_SMOKE_MS";
/// Env var carrying the optional smoke-mode paste round-trip text — see
/// [`SMOKE_MS_VAR`].
const SMOKE_PASTE_VAR: &str = "QUADRAUI_GTK_SMOKE_PASTE";

// `SmokeConfig` + the size/clipboard pass-fail predicates used to be
// defined here; extracted to the backend-neutral `desktop` module (#498)
// — see that module's doc for why (and `smoke_tests` below, which now
// covers `SmokeConfig::from_env` itself in addition to the predicates).

/// Configuration for [`run_with`] — the GTK application id and window
/// title that [`run`] hardcodes to generic defaults (`"org.quadraui.app"`
/// / `"quadraui app"`).
///
/// Apps that need a stable app id (Wayland/GNOME dock pinning, D-Bus
/// single-instance activation, Flatpak) or a custom window title build
/// this and call [`run_with`] instead of [`run`]. `run` itself is just
/// `run_with(app, RunConfig::default())` — see quadraui#234.
///
/// ```no_run
/// # struct MyApp;
/// # impl quadraui::AppLogic for MyApp {
/// #     type AreaId = ();
/// #     fn render(&self, _b: &mut dyn quadraui::Backend, _a: ()) {}
/// #     fn handle(&mut self, _e: quadraui::UiEvent, _b: &mut dyn quadraui::Backend) -> quadraui::Reaction { quadraui::Reaction::Continue }
/// # }
/// use quadraui::gtk::RunConfig;
///
/// let config = RunConfig::new("io.github.jdonaghy.kubeui-gtk", "kubeui");
/// quadraui::gtk::run_with(MyApp, config);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunConfig {
    /// GTK application id, e.g. `"io.github.jdonaghy.kubeui-gtk"`. Used by
    /// GTK/GLib for Wayland/GNOME dock integration, D-Bus activation, and
    /// desktop-file matching. Should be a valid reverse-DNS app id — see
    /// the [GNOME application ID guidelines](https://developer.gnome.org/documentation/tutorials/application-id.html).
    pub app_id: String,
    /// Window title shown by the window manager (titlebar, taskbar,
    /// Alt-Tab switcher).
    pub title: String,
    /// Themed icon name (`gtk4::Window::set_icon_name`), e.g.
    /// `"io.github.jdonaghy.vimcode"`. `None` (the default) leaves GTK's
    /// own fallback icon in place. Set via [`Self::with_icon_name`] — see
    /// quadraui#656.
    pub icon_name: Option<String>,
}

impl RunConfig {
    /// Build a config with the given app id and window title. No icon
    /// override — see [`Self::with_icon_name`].
    pub fn new(app_id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            app_id: app_id.into(),
            title: title.into(),
            icon_name: None,
        }
    }

    /// Set a themed icon name for the window (quadraui#656). Feeds
    /// `gtk4::Window::set_icon_name` in [`run_with`] so themed-icon lookup
    /// works even on window managers that don't resolve `app_id` to an
    /// installed `.desktop` file.
    pub fn with_icon_name(mut self, icon_name: impl Into<String>) -> Self {
        self.icon_name = Some(icon_name.into());
        self
    }
}

impl Default for RunConfig {
    /// Mirrors [`run`]'s previously-hardcoded defaults, so `run_with(app,
    /// RunConfig::default())` and `run(app)` behave identically.
    fn default() -> Self {
        Self {
            app_id: "org.quadraui.app".to_string(),
            title: "quadraui app".to_string(),
            icon_name: None,
        }
    }
}

/// Drive `app` to completion in a basic single-`DrawingArea` GTK
/// environment, using the default [`RunConfig`] (generic app id and
/// window title). See [`run_with`] to set a custom app id / title
/// (quadraui#234) — needed by any app that isn't quadraui itself, e.g.
/// for Wayland/GNOME dock integration.
///
/// Creates an `Application`, a single window, and a single
/// `DrawingArea` filling the window. Wires `set_draw_func`,
/// keyboard, mouse-click, mouse-motion, and scroll event controllers
/// to push `UiEvent`s through `GtkBackend`'s event queue. The frame
/// loop polls the queue and dispatches via
/// [`AppLogic::handle`][crate::AppLogic::handle].
///
/// Returns [`std::process::ExitCode`] so apps can `fn main() ->
/// std::process::ExitCode { quadraui::gtk::run(app) }` without
/// translating between `glib::ExitCode` and the stdlib type. Mirrors
/// the ergonomic shape of `quadraui::tui::run` (which returns
/// `std::io::Result<()>` so apps' `main` is similarly trivial).
pub fn run<A: AppLogic + 'static>(app: A) -> std::process::ExitCode {
    run_with(app, RunConfig::default())
}

/// Same as [`run`], but with a caller-supplied [`RunConfig`] (app id +
/// window title) instead of the generic quadraui defaults (quadraui#234).
pub fn run_with<A: AppLogic + 'static>(app: A, config: RunConfig) -> std::process::ExitCode {
    let app = Rc::new(RefCell::new(app));
    let backend = Rc::new(RefCell::new(GtkBackend::new()));
    // quadraui#450 (GD-5): `None` unless `QUADRAUI_GTK_SMOKE_MS` is set —
    // see the module doc's "Headless smoke mode" section.
    let smoke = SmokeConfig::from_env(SMOKE_MS_VAR, SMOKE_PASTE_VAR);
    let smoke_failed = Rc::new(Cell::new(false));
    let title = config.title;
    let icon_name = config.icon_name;

    let gapp = Application::builder().application_id(config.app_id).build();

    {
        let app = app.clone();
        let backend = backend.clone();
        let smoke = smoke.clone();
        let smoke_failed = smoke_failed.clone();
        let title = title.clone();
        let icon_name = icon_name.clone();
        gapp.connect_activate(move |gapp| {
            activate(
                gapp,
                app.clone(),
                backend.clone(),
                smoke.clone(),
                smoke_failed.clone(),
                title.clone(),
                icon_name.clone(),
            );
        });
    }

    let glib_code = gapp.run();
    // Smoke-mode failures (bad window size, clipboard round-trip mismatch
    // — see `schedule_smoke_check`) override GLib's own exit code so an
    // `xvfb-run` caller sees a non-zero status even though the app itself
    // exited "cleanly" (a closed window, not a crash).
    if smoke_failed.get() {
        return std::process::ExitCode::FAILURE;
    }
    // glib 0.22 (pulled in by the gtk4 0.11 bump, #796) renamed
    // `ExitCode::value()` to `get()` and added a direct
    // `From<ExitCode> for std::process::ExitCode`, so the `as u8`
    // round-trip through `ExitCode::from(u8)` is no longer needed either.
    std::process::ExitCode::from(glib_code)
}

fn activate<A: AppLogic + 'static>(
    gapp: &Application,
    app: Rc<RefCell<A>>,
    backend: Rc<RefCell<GtkBackend>>,
    smoke: Option<SmokeConfig>,
    smoke_failed: Rc<Cell<bool>>,
    title: String,
    icon_name: Option<String>,
) {
    let mut window_builder = ApplicationWindow::builder()
        .application(gapp)
        .title(title)
        .default_width(DEFAULT_WINDOW_WIDTH)
        .default_height(DEFAULT_WINDOW_HEIGHT);
    // quadraui#656: themed icon lookup, independent of the app-id ↔
    // `.desktop` file match — see `RunConfig::icon_name`'s doc comment.
    if let Some(icon_name) = icon_name {
        window_builder = window_builder.icon_name(icon_name);
    }
    let window = window_builder.build();

    // Stash the window handle so `Backend::begin_window_drag` /
    // `Backend::toggle_window_maximize` (#400) have something to drive.
    // Harmless for apps that never call them (default no-op on every
    // other backend, and GTK apps that don't opt into a CSD titlebar).
    backend.borrow_mut().set_window(window.clone());

    // Re-entrancy guard shared with `GtkPlatformServices::pump_until_ready`
    // (#427): `> 0` while a file dialog's nested-mainloop wait is in
    // flight. Fetched once here, before any event controller is
    // installed, and cloned (not re-derived via `backend.borrow()`) into
    // every closure below that calls `backend.borrow_mut()` — those
    // closures check it first and no-op while a dialog pump further up
    // the call stack is already holding the backend's `RefCell`
    // mutably borrowed. Without this, the 33ms idle-drain timer (or any
    // input controller) re-enters via `MainContext::iteration(true)`
    // and double-borrows, panicking inside a non-unwindable GLib
    // callback frame and aborting the process.
    let pump_depth = backend.borrow().pump_depth();

    // #902 backstop: shared handle to the backend's event queue, fetched
    // once here (same reasoning as `pump_depth` above) and cloned into
    // any closure below that needs to *defer* an event rather than drop
    // it outright when `try_dispatch`/`try_borrow_both` (below) report the
    // backend/app are already borrowed. Pushing here never needs
    // `backend`'s own `RefCell` — `events_handle()` hands back a plain
    // `Rc<RefCell<VecDeque<UiEvent>>>` with independent borrow state — so
    // it works precisely in the situation that motivates it: `backend`
    // mutably borrowed by an outer `dispatch_event` call, still on the
    // stack. See `try_dispatch`'s doc and the `close-request` handler
    // below for the concrete re-entrancy this exists to survive.
    let events_handle = backend.borrow().events_handle();

    let da = DrawingArea::new();
    da.set_hexpand(true);
    da.set_vexpand(true);
    window.set_child(Some(&da));

    // Seed the backend's persistent pango context from the widget so
    // `form_layout()` and other `_layout()` methods can use exact Pango
    // measurement outside the draw callback.
    {
        let pctx = da.pango_context();
        let font_desc = pg::FontDescription::from_string("Sans 11");
        // pango 0.22 (gtk4 0.11 bump, #796) tightened
        // `Context::set_font_description` from `Option<&FontDescription>`
        // to `&FontDescription` — there's no longer a "clear description"
        // call to represent, so the `Some(...)` wrapper is just dropped.
        pctx.set_font_description(&font_desc);
        backend.borrow_mut().set_pango_context(pctx);
    }

    // App setup hook (one-time).
    //
    // Seed the viewport with the window's default size first (see
    // `DEFAULT_WINDOW_WIDTH`/`HEIGHT` above) — the `DrawingArea` has no
    // allocation yet at this point, so without this `backend.viewport()`
    // would return `GtkBackend::new()`'s zeroed default and any app that
    // sizes something (e.g. a PTY) from the setup-time viewport would
    // size it to ~zero (quadraui#437).
    {
        let mut backend_mut = backend.borrow_mut();
        backend_mut.begin_frame(crate::Viewport::new(
            DEFAULT_WINDOW_WIDTH as f32,
            DEFAULT_WINDOW_HEIGHT as f32,
            1.0,
        ));
        let mut app_mut = app.borrow_mut();
        app_mut.setup(&mut *backend_mut);
    }

    // ── Draw callback ──────────────────────────────────────────────
    //
    // Set the editor's Pango font on the layout (app-configurable via
    // `Backend::set_editor_font` — see #422) and seed
    // `current_line_height` / `current_char_width` on the backend from
    // the resolved font metrics so trait `draw_*` methods that consume
    // those (e.g. `draw_status_bar` for clip height) line up with the
    // actual rendered text height.
    //
    // Apps that want a custom editor font call `backend.set_editor_font`
    // (from `AppLogic::setup` for a static font, or any time their
    // preference changes) via the trait — no direct `GtkBackend` access
    // required. `ShellApp` consumers set it declaratively via
    // `ShellConfig::with_editor_font`.
    {
        let app = app.clone();
        let backend = backend.clone();
        let pump_depth = pump_depth.clone();
        da.set_draw_func(move |_da, cr, w, h| {
            // #427 re-entrancy guard: skip this repaint entirely rather
            // than double-borrow `backend` while a file dialog's nested
            // pump (further up the call stack) already holds it. Worst
            // case this frame stays stale until the dialog closes and a
            // normal redraw fires; that beats aborting the process.
            if pump_depth.is_pumping() {
                return;
            }
            let mut backend_mut = backend.borrow_mut();
            let app_ref = app.borrow();
            render_frame(&mut backend_mut, &*app_ref, cr, w, h);
        });
    }

    // ── Keyboard ───────────────────────────────────────────────────
    //
    // Intercepts Ctrl-C when a text selection is active: copies the
    // selected text to the clipboard via arboard and delivers a
    // `TextCopied` event so apps can confirm (mirrors the TUI runner).
    let key_ctrl = EventControllerKey::new();
    {
        let backend = backend.clone();
        let app = app.clone();
        let da_for_redraw = da.clone();
        let window_for_close = window.clone();
        let pump_depth = pump_depth.clone();
        key_ctrl.connect_key_pressed(move |_ctrl, key, _code, modifier| {
            // #427 re-entrancy guard: a dialog pump further up the call
            // stack already holds `backend` mutably borrowed — don't
            // re-enter it (and don't dispatch input to the app while a
            // modal-ish dialog is up).
            if pump_depth.is_pumping() {
                return glib::Propagation::Proceed;
            }
            let Some(ev) = gdk_key_to_uievent(key, modifier, false) else {
                return glib::Propagation::Proceed;
            };

            // All key-press pre-processing — ActivityBar keyboard-focus
            // intercept, Tab/Shift+Tab focus cycling (#830), `Global`
            // accelerator matching (#445), Ctrl-C/V/A
            // interception — lives in the shared `dispatch_event` (see the
            // module doc's "Shared with the headless test driver" section)
            // so the live GTK path and `GtkDriver::press`/`type_char`
            // (quadraui#446) can't drift apart.
            // #902 backstop: `backend`/`app` already borrowed by an outer
            // `dispatch_event` further up this stack (not a nested pump,
            // which `pump_depth` above already ruled out) — degrade like
            // that guard instead of panicking. See the `#902` section
            // above `dispatch_event`.
            let Some(outcome) = try_dispatch(&backend, &app, ev) else {
                return glib::Propagation::Proceed;
            };
            apply_event_outcome(
                outcome,
                &backend.borrow(),
                &da_for_redraw,
                &window_for_close,
            );
            glib::Propagation::Stop
        });
    }
    window.add_controller(key_ctrl);

    // ── Mouse click ────────────────────────────────────────────────
    //
    // Routes mouse-down through `dispatch_click` so registered text
    // regions receive selection drags. Pre-processes the returned events
    // for selection-state management before forwarding to the app, mirroring
    // the TUI runner's text-selection pre-processing.
    let click = GestureClick::builder().button(0).build();
    {
        let backend = backend.clone();
        let app = app.clone();
        let da_for_redraw = da.clone();
        let window_for_close = window.clone();
        let pump_depth = pump_depth.clone();
        click.connect_pressed(move |gesture, _n_press, x, y| {
            // #427 re-entrancy guard — see the key-press handler above.
            if pump_depth.is_pumping() {
                return;
            }
            let gdk_button = gesture.current_button();
            let modifier = gesture.current_event_state();
            let button = gdk_button_to_quadraui(gdk_button);
            let modifiers = gdk_modifiers_to_quadraui(modifier);
            let position = Point::new(x as f32, y as f32);

            // Stash the raw GDK press context (device + button + timestamp)
            // before this press gets translated to a portable `UiEvent`, so
            // `Backend::begin_window_drag` can later arm a deferred
            // window-drag request with it (#400; see
            // `GtkBackend::armed_window_drag` for why it's deferred rather
            // than calling GDK's native window-drag immediately). Runs for
            // both single- and double-press events (both fire
            // `connect_pressed`); harmless if the app never calls
            // `begin_window_drag`.
            if let Some(event) = gesture.current_event() {
                if let Some(device) = event.device() {
                    // #902 backstop: best-effort — if `backend` is already
                    // borrowed (see the `#902` section above
                    // `dispatch_event`), skip the stash. Worst case a
                    // subsequent `begin_window_drag` has nothing armed to
                    // commit; strictly better than aborting the process.
                    if let Ok(mut backend_mut) = backend.try_borrow_mut() {
                        backend_mut.stash_window_press(
                            device,
                            gdk_button as i32,
                            x,
                            y,
                            event.time(),
                        );
                    }
                }
            }

            // #902 backstop — see the `#902` section above `dispatch_event`.
            let Ok(mut backend_mut) = backend.try_borrow_mut() else {
                return;
            };

            // Route through dispatch_click so text-region clicks begin a
            // TextSelection drag and scrollbar clicks begin scrollbar drags —
            // regardless of GDK's own press count. Double-click folding used
            // to be decided right here from `n_press == 2`, bypassing
            // `dispatch_click` (and so `TextRegion`/modal routing) for the
            // second press entirely; #813 moves it into `dispatch_event`'s
            // shared `fold_double_click` step below, applied uniformly to
            // whatever `dispatch_click` returns, the same as macOS/Windows.
            let events = {
                let stack_rc = backend_mut.modal_stack_handle();
                let drag_rc = backend_mut.drag_state_handle();
                let stack = stack_rc.borrow();
                let mut drag = drag_rc.borrow_mut();
                let evs = dispatch_click(
                    &stack,
                    &[], // scroll surfaces not tracked in the runner
                    backend_mut.text_regions(),
                    &mut drag,
                    position,
                    button,
                    modifiers,
                );
                // Track which region was clicked so Ctrl-A can target
                // the right region even before the first drag move.
                if let Some(crate::dispatch::DragTarget::TextSelection { region, .. }) =
                    drag.target()
                {
                    backend_mut.track_focused_text_region(region.clone());
                }
                evs
            };

            let mut needs_redraw = false;
            for ev in events {
                // `dispatch_event` folds the double-click itself (as its
                // first pre-processing step) — pass the raw `MouseDown`
                // dispatch_click returned straight through.
                //
                // #902 backstop: `app` already borrowed by an outer
                // `dispatch_event` further up this stack — stop draining
                // this batch rather than panicking; see the `#902` section
                // above `dispatch_event`.
                let Some(outcome) = try_dispatch_borrowed(&mut backend_mut, &app, ev) else {
                    break;
                };
                match outcome {
                    EventOutcome::Continue => {}
                    EventOutcome::Redraw => needs_redraw = true,
                    EventOutcome::RedrawAfter(delay) => backend_mut.request_frame_in(delay),
                    EventOutcome::Exit => {
                        // `destroy()`, not `close()` (quadraui#501
                        // review — same fix as `GtkSink::request_exit`):
                        // this `Exit` came from the app's own handling
                        // of a click/drag event, not from the OS
                        // close-request signal, so re-routing it through
                        // `close()` would emit `close-request` and
                        // re-dispatch a synthetic `WindowClose` the app
                        // never asked about — vetoing its own exit for
                        // any app with no `WindowClose` opinion (the
                        // unhandled catch-all default).
                        window_for_close.destroy();
                        return;
                    }
                }
            }
            if needs_redraw {
                da_for_redraw.queue_draw();
            }
        });
    }
    {
        let backend = backend.clone();
        let app = app.clone();
        let da_for_redraw = da.clone();
        let window_for_close = window.clone();
        let pump_depth = pump_depth.clone();
        click.connect_released(move |gesture, _n_press, x, y| {
            // #427 re-entrancy guard — see the key-press handler above.
            if pump_depth.is_pumping() {
                return;
            }
            // #902 backstop — see the `#902` section above `dispatch_event`.
            let Ok(mut backend_mut) = backend.try_borrow_mut() else {
                return;
            };
            // #400: if the button goes up before the pointer ever moved
            // past the drag threshold, this was a plain click (or the
            // first half of a double-click), not a drag. Discard the
            // armed window-drag request rather than leaving it to be
            // accidentally committed by a later, unrelated hover-motion
            // event. See `GtkBackend::armed_window_drag` for the full
            // rationale.
            backend_mut.discard_armed_window_drag();
            let position = Point::new(x as f32, y as f32);
            let button: MouseButton = gdk_button_to_quadraui(gesture.current_button());
            let events = {
                let stack_rc = backend_mut.modal_stack_handle();
                let drag_rc = backend_mut.drag_state_handle();
                let stack = stack_rc.borrow();
                let mut drag = drag_rc.borrow_mut();
                dispatch_mouse_up(&stack, &mut drag, position, button)
            };
            for ev in events {
                // #902 backstop — see the `#902` section above
                // `dispatch_event`.
                let Some(outcome) = try_dispatch_borrowed(&mut backend_mut, &app, ev) else {
                    break;
                };
                apply_event_outcome(outcome, &backend_mut, &da_for_redraw, &window_for_close);
            }
        });
    }
    da.add_controller(click);

    // ── Motion ─────────────────────────────────────────────────────
    //
    // Routes mouse-move through `dispatch_mouse_drag` so text-selection
    // drags emit `TextSelectionChanged` events. The runner pre-processes
    // `TextSelectionChanged` to update backend selection state before
    // forwarding to the app.
    //
    // `cursor_pos` is shared with the scroll controller below so that
    // scroll events carry the actual pointer position. GTK's
    // `EventControllerScroll` only delivers (dx, dy) in its callback.
    let cursor_pos = Rc::new(Cell::new((0.0_f64, 0.0_f64)));
    let motion = EventControllerMotion::new();
    {
        let backend = backend.clone();
        let app = app.clone();
        let da_for_redraw = da.clone();
        let window_for_close = window.clone();
        let cursor_pos = cursor_pos.clone();
        let pump_depth = pump_depth.clone();
        motion.connect_motion(move |ctrl, x, y| {
            cursor_pos.set((x, y));

            // #427 re-entrancy guard — see the key-press handler above.
            if pump_depth.is_pumping() {
                return;
            }

            // #400: commit a deferred window-drag once the pointer has
            // moved past the drag threshold since the press that armed
            // it (`Backend::begin_window_drag`). Mirrors native
            // `gtk4::WindowHandle`, which defers its own move-start to
            // `GestureDrag`'s threshold-gated `drag-begin` signal rather
            // than the raw button press — this is what keeps a press
            // that turns into a double-click from starting an
            // interactive move grab that would swallow the second press.
            // The threshold check itself lives in the backend-neutral
            // `desktop::WindowDragArm` (#498) —
            // `GtkBackend::try_commit_window_drag` is a no-op on every
            // call until the pointer actually crosses it (whether
            // because nothing is armed or the request is still under
            // threshold), so this can call it unconditionally on every
            // move. Only returns early when a drag is actually committed
            // (control passes to the compositor's native move at that
            // point); otherwise falls through to the normal motion
            // handling below unaffected.
            // #902 backstop: best-effort — if `backend` is already
            // borrowed, skip the commit check this move event; the next
            // motion event retries it. See the `#902` section above
            // `dispatch_event`.
            let Ok(mut probe_backend_mut) = backend.try_borrow_mut() else {
                return;
            };
            if probe_backend_mut.try_commit_window_drag(x, y) {
                return;
            }
            drop(probe_backend_mut);

            let modifier = ctrl.current_event_state();
            let buttons = ButtonMask {
                left: modifier.contains(gtk4::gdk::ModifierType::BUTTON1_MASK),
                middle: modifier.contains(gtk4::gdk::ModifierType::BUTTON2_MASK),
                right: modifier.contains(gtk4::gdk::ModifierType::BUTTON3_MASK),
            };
            let position = Point::new(x as f32, y as f32);

            // #902 backstop — see the `#902` section above `dispatch_event`.
            let Ok(mut backend_mut) = backend.try_borrow_mut() else {
                return;
            };
            let events = {
                let drag_rc = backend_mut.drag_state_handle();
                let drag = drag_rc.borrow();
                dispatch_mouse_drag(&drag, position, buttons)
            };

            // `TextSelectionChanged` (active-selection state update) and the
            // fallback `app.handle` both live in the shared `dispatch_event`.
            let mut needs_redraw = false;
            for ev in events {
                // #902 backstop — see the `#902` section above
                // `dispatch_event`.
                let Some(outcome) = try_dispatch_borrowed(&mut backend_mut, &app, ev) else {
                    break;
                };
                match outcome {
                    EventOutcome::Continue => {}
                    EventOutcome::Redraw => needs_redraw = true,
                    EventOutcome::RedrawAfter(delay) => backend_mut.request_frame_in(delay),
                    EventOutcome::Exit => {
                        // `destroy()`, not `close()` (quadraui#501
                        // review — same fix as `GtkSink::request_exit`):
                        // this `Exit` came from the app's own handling
                        // of a click/drag event, not from the OS
                        // close-request signal, so re-routing it through
                        // `close()` would emit `close-request` and
                        // re-dispatch a synthetic `WindowClose` the app
                        // never asked about — vetoing its own exit for
                        // any app with no `WindowClose` opinion (the
                        // unhandled catch-all default).
                        window_for_close.destroy();
                        return;
                    }
                }
            }
            if needs_redraw {
                da_for_redraw.queue_draw();
            }
        });
    }
    da.add_controller(motion);

    // ── Scroll ─────────────────────────────────────────────────────
    let scroll = EventControllerScroll::new(EventControllerScrollFlags::BOTH_AXES);
    {
        let backend = backend.clone();
        let app = app.clone();
        let da_for_redraw = da.clone();
        let window_for_close = window.clone();
        let pump_depth = pump_depth.clone();
        scroll.connect_scroll(move |_ctrl, dx, dy| {
            // #427 re-entrancy guard — see the key-press handler above.
            if pump_depth.is_pumping() {
                return glib::Propagation::Proceed;
            }
            let (x, y) = cursor_pos.get();
            // #902 backstop: `app` already mutably borrowed further up
            // this stack would make even this shared `.borrow()` panic —
            // fall back to the trait's own default (`false`) rather than
            // risk it; `try_dispatch` below degrades the same way for the
            // dispatch itself. See the `#902` section above
            // `dispatch_event`.
            let natural_scroll = app
                .try_borrow()
                .map(|a| a.natural_scroll())
                .unwrap_or(false);
            let ev = gdk_scroll_to_uievent_with_direction(dx, dy, x, y, natural_scroll);
            let Some(outcome) = try_dispatch(&backend, &app, ev) else {
                return glib::Propagation::Stop;
            };
            apply_event_outcome(
                outcome,
                &backend.borrow(),
                &da_for_redraw,
                &window_for_close,
            );
            glib::Propagation::Stop
        });
    }
    da.add_controller(scroll);

    // ── Resize ─────────────────────────────────────────────────────
    //
    // `Backend::begin_frame(viewport)` (in `set_draw_func` above) keeps
    // `backend.viewport()` in sync with the DA's allocated size on every
    // *render*, but apps with side effects on resize — not just
    // re-painting — never learned about it: GTK didn't deliver
    // `UiEvent::WindowResized` at all (quadraui#437; see
    // `gdk_resize_to_uievent`'s doc comment). `DrawingArea::connect_resize`
    // fires with the widget's real allocated pixel size both on first
    // realization (correcting the `DEFAULT_WINDOW_WIDTH`/`HEIGHT` seed
    // above) and on every subsequent resize, mirroring how the TUI
    // runner delivers `WindowResized` from crossterm's `Resize` event.
    //
    // The dispatch of `UiEvent::WindowResized` itself is **debounced**
    // (quadraui#437 follow-up) using the same `RESIZE_SETTLE` window the
    // TUI runner's poll-loop debounce uses — see
    // `crate::runtime::RESIZE_SETTLE`'s doc for the full rationale
    // (PTY/SIGWINCH corruption from resizing on every intermediate
    // frame of a drag). `resize_timer` below is GTK's own mechanism for
    // it: cancel-and-reschedule a `glib::SourceId` per `connect_resize`
    // signal, rather than TUI's `ResizeDebouncer` + `Instant`-poll (GTK's
    // timer already gives exactly-once-after-settle semantics natively,
    // and re-reads the DA's live size at fire time instead of storing a
    // pending value). Painting itself is unaffected and stays perfectly
    // live — `set_draw_func` re-reads the DA's actual allocated size
    // every frame regardless of whether the debounced event has fired
    // yet.
    let resize_timer: Rc<Cell<Option<glib::SourceId>>> = Rc::new(Cell::new(None));
    {
        let backend = backend.clone();
        let app = app.clone();
        let da_for_redraw = da.clone();
        let window_for_close = window.clone();
        let pump_depth = pump_depth.clone();
        let resize_timer = resize_timer.clone();
        da.connect_resize(move |da, _width, _height| {
            // #427 re-entrancy guard — see the key-press handler above.
            if pump_depth.is_pumping() {
                return;
            }
            // Force a FULL-widget invalidation on every resize edge, not
            // just at the debounced settle below (quadraui#437). This is the
            // render-path half of the "stale ~ / > prompt fragments stuck on
            // rows that should be blank after a shrink→expand" ghosting.
            //
            // GTK repaints a growing window with *partial* damage regions —
            // typically only the newly-exposed strip at the right/bottom
            // edge — and reuses the cached render node for the rest. The
            // `set_draw_func` above opens every frame by clearing the whole
            // DA to the theme background, but Cairo honours GTK's clip, so
            // that "full" clear only ever covers the damaged strip. The
            // undamaged middle keeps its pre-resize render node, so any
            // glyph the pre-resize grid painted there survives — the ghost.
            // A bare `queue_draw()` (no region) invalidates the entire
            // widget, so the next `set_draw_func` runs unclipped and the
            // whole-DA clear actually clears the whole DA. GTK coalesces
            // repeated per-edge invalidations into one repaint per frame, so
            // this stays cheap during a live drag. The PTY resize / SIGWINCH
            // stays debounced below — only the *paint* is forced eager here.
            da.queue_draw();
            // Cancel any pending debounced dispatch — this signal
            // supersedes it. `remove()` is a no-op-safe consume; the
            // source may have already fired (Cell held `None`).
            if let Some(id) = resize_timer.take() {
                id.remove();
            }
            let backend = backend.clone();
            let app = app.clone();
            let da_for_redraw = da_for_redraw.clone();
            let window_for_close = window_for_close.clone();
            let pump_depth = pump_depth.clone();
            let resize_timer_inner = resize_timer.clone();
            let id = glib::source::timeout_add_local_once(RESIZE_SETTLE, move || {
                // The fired timer is no longer "pending" — clear so a
                // future resize doesn't try to cancel a dead source.
                resize_timer_inner.set(None);
                // #427 re-entrancy guard, re-checked at fire time —
                // a dialog pump may have started after this timer was
                // scheduled.
                if pump_depth.is_pumping() {
                    return;
                }
                // Query the DA's *current* (settled) allocated size
                // rather than replaying the size captured when this
                // timer was scheduled — later `connect_resize` calls
                // during the same drag reschedule (see above), so by
                // the time this fires the widget has already reached
                // its final size.
                let width = da_for_redraw.width();
                let height = da_for_redraw.height();
                let scale = da_for_redraw.scale_factor() as f32;
                let ev = gdk_resize_to_uievent(width, height, scale);
                // #902 backstop — see the `#902` section above
                // `dispatch_event`. Skipping here means the tracked scale
                // (`set_dpi_scale`, run as `try_dispatch_with`'s `pre`
                // step) doesn't update this tick either; the next resize
                // or `notify::scale-factor` firing corrects it.
                let Some(outcome) = try_dispatch_with(
                    &backend,
                    &app,
                    |backend_mut| {
                        // #834: keep the tracked scale current on every
                        // resize too, not just the dedicated
                        // `notify::scale-factor` handler below — a resize
                        // and a DPI change can arrive in the same GTK
                        // signal burst (dragging a window across a
                        // monitor boundary resizes it too on some
                        // compositors).
                        backend_mut.set_dpi_scale(scale);
                    },
                    ev,
                ) else {
                    return;
                };
                apply_event_outcome(
                    outcome,
                    &backend.borrow(),
                    &da_for_redraw,
                    &window_for_close,
                );
            });
            resize_timer.set(Some(id));
        });
    }

    // ── HiDPI runtime change (issue #834) ───────────────────────────
    //
    // `DrawingArea::scale_factor()` is only ever read once, at
    // resize-settle time, before this issue — a pure DPI change with no
    // accompanying resize (dragging the window to a different-DPI
    // monitor without resizing it, or an external monitor's scaling
    // setting changing live) never fired `connect_resize` at all, so
    // `UiEvent::DpiChanged` was never dispatched and `GtkBackend`'s
    // tracked scale went stale. `notify::scale-factor` is the GObject
    // property-change signal GTK fires specifically for this case
    // (`gtk4::Widget::connect_scale_factor_notify`), independent of any
    // size change. No debounce: unlike a live resize drag, a DPI change
    // doesn't arrive as a rapid-fire burst.
    {
        let backend = backend.clone();
        let app = app.clone();
        let da_for_dpi = da.clone();
        let window_for_dpi = window.clone();
        let pump_depth = pump_depth.clone();
        da.connect_scale_factor_notify(move |da| {
            // #427 re-entrancy guard — see the key-press handler above.
            if pump_depth.is_pumping() {
                return;
            }
            let scale = da.scale_factor() as f32;
            // #902 backstop — see the `#902` section above `dispatch_event`.
            let Some(outcome) = try_dispatch_with(
                &backend,
                &app,
                |backend_mut| backend_mut.set_dpi_scale(scale),
                UiEvent::DpiChanged(scale),
            ) else {
                return;
            };
            apply_event_outcome(outcome, &backend.borrow(), &da_for_dpi, &window_for_dpi);
        });
    }

    // ── OS file drop (issue #834) ────────────────────────────────────
    //
    // `gtk::DropTarget` is a `gtk::EventController` (added to the `da`
    // the same way the click/motion/scroll controllers above are), not
    // a separate widget — GTK4's drag-and-drop model routes a drop to
    // whichever controller on the target widget declares it accepts the
    // dragged `GType`. `gdk::FileList` is the type GTK/GNOME apps (file
    // managers, browsers) drop file references as; `NSFilenamesPboardType`
    // and `WM_DROPFILES`'s `HDROP` are the equivalent native shapes the
    // macOS/Win wiring decodes.
    {
        let backend = backend.clone();
        let app = app.clone();
        let da_for_drop = da.clone();
        let window_for_drop = window.clone();
        let pump_depth = pump_depth.clone();
        let drop_target = gtk4::DropTarget::new(
            gtk4::gdk::FileList::static_type(),
            gtk4::gdk::DragAction::COPY,
        );
        drop_target.connect_drop(move |_target, value, x, y| {
            // #427 re-entrancy guard — see the key-press handler above.
            if pump_depth.is_pumping() {
                return false;
            }
            let Ok(file_list) = value.get::<gtk4::gdk::FileList>() else {
                return false;
            };
            let paths: Vec<std::path::PathBuf> =
                file_list.files().iter().filter_map(|f| f.path()).collect();
            if paths.is_empty() {
                return false;
            }
            let ev = gtk_drop_to_uievent(paths, x, y);
            // #902 backstop — see the `#902` section above `dispatch_event`.
            let Some(outcome) = try_dispatch(&backend, &app, ev) else {
                return false;
            };
            apply_event_outcome(outcome, &backend.borrow(), &da_for_drop, &window_for_drop);
            true
        });
        da.add_controller(drop_target);
    }

    // ── Window close (quadraui#501) ───────────────────────────────
    //
    // Until now GTK never asked the app before tearing the window down:
    // the OS "×" button / Alt-F4 / window-manager close went straight to
    // GLib's default `close-request` handling with no `UiEvent` in
    // between, so an app had no way to veto an unsaved-changes close or
    // even *know* the window was closing. `WinBackend::wnd_proc`'s
    // `WM_CLOSE` arm (`win/run.rs`) already documents the intended
    // contract — "matching the GTK runner's `Reaction::Exit =>
    // window.close()`" — this wiring is the GTK half that comment
    // assumed already existed. `connect_close_request` fires before the
    // window is destroyed and lets the handler return
    // `glib::Propagation::Stop` to cancel it.
    //
    // Dispatch `UiEvent::WindowClose` through the same `dispatch_event`
    // funnel every other native signal uses, then only let the close
    // proceed if the app's own reaction was `Exit` — same rule Win uses.
    // An app that doesn't return `Exit` (the default for an unhandled
    // event, and any app that wants to show a confirmation dialog first)
    // implicitly vetoes the close. Never route this outcome through
    // `apply_event_outcome`/`request_exit`: that calls `window.close()`,
    // which would re-enter this same `close-request` handler.
    //
    // That re-entrancy hazard cuts the other way too, and *did* bite
    // (quadraui#501 review): `GtkSink::request_exit` — the target of
    // every *other* `Reaction::Exit`/`EventOutcome::Exit` in this file,
    // e.g. an app's own "press q to quit" key handler — used to call
    // `window.close()` as well. Once this handler existed, that
    // `close()` re-entered it as a *second*, synthetic `WindowClose`
    // dispatch the app never asked for; an app with no `WindowClose`
    // opinion (the unhandled-catch-all default, true of every existing
    // example) would veto its own already-decided exit. Same trap
    // caught `schedule_smoke_check`'s forced-close-after-timeout, which
    // needs to close the window unconditionally regardless of what the
    // app returns. Both now call `window.destroy()` instead of
    // `window.close()` — `destroy()` tears the window down directly
    // without emitting `close-request`, so it cannot loop back through
    // this veto. Only a real external close request (OS "×" / Alt-F4 /
    // window manager) should ever reach this handler.
    {
        let backend = backend.clone();
        let app = app.clone();
        let da_for_redraw = da.clone();
        let pump_depth = pump_depth.clone();
        let events_handle = events_handle.clone();
        window.connect_close_request(move |_window| {
            // #427 re-entrancy guard — see the key-press handler above.
            if pump_depth.is_pumping() {
                return glib::Propagation::Proceed;
            }
            // #902: this is the handler the reported abort actually hit —
            // `pump_depth` only ever sees a *nested modal pump* (#427);
            // it's still 0 here when app code, synchronously inside an
            // outer `dispatch_event` call further up this very stack,
            // calls `window.close()`. GTK turns that into a second,
            // synchronous `close-request` emission that re-enters this
            // closure while `backend`/`app` are still borrowed by the
            // outer call — see the module doc's "Re-entrancy: two
            // independent sources, one invariant (quadraui#902)" section
            // for the full call chain.
            //
            // Unlike every other handler above, this one can't just skip
            // the event on a double-borrow: `WindowClose` is exactly what
            // a consumer needs to see to decide whether to actually exit,
            // and dropping it silently would turn an in-dispatch
            // `close()` call into a permanent no-op. So defer instead:
            // push straight onto `events_handle` — a plain
            // `Rc<RefCell<VecDeque<UiEvent>>>` with its own, independent
            // borrow state, so this can't double-borrow no matter how
            // deep the reentrant stack is (see `events_handle`'s capture
            // above `activate`'s `pump_depth`) — and refuse the close
            // *this* turn, same as the ordinary `EventOutcome::Continue`
            // arm below. The queued `WindowClose` reaches the app
            // cleanly a tick later via the 33ms drain loop, once every
            // outstanding borrow from this stack has unwound; if the app
            // wants to exit it drives that itself through
            // `Reaction::Exit` → `request_exit` → `window.destroy()`,
            // which — unlike `close()` — cannot loop back through this
            // handler.
            let outcome = match try_dispatch(&backend, &app, UiEvent::WindowClose) {
                Some(outcome) => outcome,
                None => {
                    events_handle.borrow_mut().push_back(UiEvent::WindowClose);
                    return glib::Propagation::Stop;
                }
            };
            match outcome {
                EventOutcome::Exit => glib::Propagation::Proceed,
                EventOutcome::Redraw => {
                    da_for_redraw.queue_draw();
                    glib::Propagation::Stop
                }
                EventOutcome::RedrawAfter(delay) => {
                    backend.borrow_mut().request_frame_in(delay);
                    glib::Propagation::Stop
                }
                EventOutcome::Continue => glib::Propagation::Stop,
            }
        });
    }

    // ── Backend event-queue drain (low-rate idle) ─────────────────
    //
    // Producer-side event controllers above already dispatch
    // synchronously through `app.handle` and trigger redraws. The
    // backend's queue exists as a forward-compat seam — any future
    // signal handlers that push directly to `events_handle()` get
    // drained here on each idle tick.
    //
    // Issue #831: this closure is also `GtkBackend::waker`'s wake target
    // (via `set_wake_callback` below), not just the periodic timer's body
    // — pulled into a named `Rc<dyn Fn()>` so a background-thread wake and
    // the ordinary 33ms poll funnel through one dispatch path instead of
    // two that could drift. See `GtkBackend::waker`'s doc for why the
    // invoked-on-wake path can't just reimplement this inline: it only has
    // `&self` on the backend, no handle to `app`/`da`/`window` of its own.
    let drain_da = da.clone();
    let drain_window = window.clone();
    // Cloned (rather than moved) so `app`/`backend` stay available below
    // for the quadraui#450 headless smoke-mode hook.
    let drain_app = app.clone();
    let drain_backend = backend.clone();
    let drain_pump_depth = pump_depth.clone();
    let drain_events_handle = events_handle.clone();
    let drain_and_tick: Rc<dyn Fn()> = Rc::new(move || {
        // #427 re-entrancy guard: this is the callback that produced the
        // original crash report. A file dialog's nested `pump_until_ready`
        // loop (invoked from `app.handle` above, while `backend` is still
        // held mutably borrowed by that call) services *this* source too
        // — without the guard, `backend.borrow_mut()` below double-borrows
        // and panics inside a non-unwindable GLib callback frame, aborting
        // the process. Skip this tick entirely and let the next one
        // (after the dialog closes, or the next `waker()` call) pick up
        // any pending events.
        if drain_pump_depth.is_pumping() {
            return;
        }
        // #902 backstop: `poll_events` needs `&mut GtkBackend` same as
        // every dispatch below — degrade instead of panicking if some
        // other re-entrant source (see the `#902` section above
        // `dispatch_event`) already holds it. The next tick (33ms later,
        // or the next `waker()` call) retries.
        let Ok(mut drain_backend_mut) = drain_backend.try_borrow_mut() else {
            return;
        };
        let events = drain_backend_mut.poll_events();
        drop(drain_backend_mut);
        let mut events_iter = events.into_iter();
        while let Some(ev) = events_iter.next() {
            // #902 backstop — see the `#902` section above
            // `dispatch_event`. Stop draining this tick rather than
            // panicking; `ev` and anything left in `events_iter` were
            // already drained from the backend's own queue above, so
            // requeue them onto `events_handle` (a separate `RefCell`,
            // safe to push to here — see `events_handle`'s capture above
            // `pump_depth`) rather than losing them outright.
            let Some((mut backend_mut, mut app_mut)) = try_borrow_both(&drain_backend, &drain_app)
            else {
                let mut q = drain_events_handle.borrow_mut();
                q.push_back(ev);
                q.extend(events_iter);
                break;
            };
            let outcome = dispatch_event(ev, &mut backend_mut, &mut *app_mut);
            drop(backend_mut);
            drop(app_mut);
            apply_event_outcome(outcome, &drain_backend.borrow(), &drain_da, &drain_window);
        }

        // Periodic tick — called after every queue drain, including
        // idle ticks where no events arrived. Lets apps drive timer
        // logic without synthetic event injection.
        //
        // #902 backstop: skip the tick, rather than panic, if `backend`/
        // `app` are already borrowed — see the `#902` section above
        // `dispatch_event`.
        if let Some((mut backend_mut, mut app_mut)) = try_borrow_both(&drain_backend, &drain_app) {
            let tick_reaction = app_mut.tick(&mut *backend_mut);
            drop(backend_mut);
            drop(app_mut);
            apply_reaction(
                tick_reaction,
                &drain_backend.borrow(),
                &drain_da,
                &drain_window,
            );
        }
    });

    // Issue #831: install this closure as `waker()`'s wake target before
    // the timer below ever fires, so a background thread that calls the
    // waker between `run_with` starting and the first idle tick still
    // reaches it.
    backend
        .borrow()
        .set_wake_callback(Rc::clone(&drain_and_tick));

    // quadraui#832: this fallback idle tick used to be the *only* wake
    // source and ran every 33ms unconditionally, burning CPU on a fully
    // idle app. It's now a coarse safety net at `IDLE_POLL_CEILING`
    // (250ms) — see that constant's doc for why a fallback still exists
    // at all rather than being removed outright (an embedded terminal's
    // PTY-output poll, in particular, still relies on periodic `tick`
    // calls with no explicit `RedrawAfter` opt-in). Apps that need a
    // tighter cadence than this — a spinner frame, a caret blink — should
    // return `Reaction::RedrawAfter` with the exact interval they need
    // instead of relying on this fallback's coarseness; that path arms a
    // real one-shot timer via `GtkBackend::request_frame_in`, independent
    // of this one.
    {
        let drain_and_tick = Rc::clone(&drain_and_tick);
        glib::timeout_add_local(crate::runtime::IDLE_POLL_CEILING, move || {
            drain_and_tick();
            glib::ControlFlow::Continue
        });
    }

    window.present();

    // quadraui#450 (GD-5): opt-in, zero-cost unless `QUADRAUI_GTK_SMOKE_MS`
    // is set — see the module doc's "Headless smoke mode" section.
    if let Some(cfg) = smoke {
        schedule_smoke_check(cfg, da, backend, app, window, smoke_failed);
    }
}

/// Schedules the one-shot headless smoke-mode check (quadraui#450, GD-5;
/// see the module doc's "Headless smoke mode" section). `cfg.after_ms`
/// after the window is presented: checks the `DrawingArea`'s allocated
/// size ([`smoke_size_ok`] — the #437 tiny-window regression check),
/// optionally round-trips `cfg.paste_text` through the real OS clipboard
/// and replays it as a synthetic Ctrl-V through [`dispatch_event`], then
/// always closes the window so an unattended `xvfb-run` invocation exits
/// deterministically instead of hanging.
///
/// #619: exempt from the crate-wide `print_stderr` deny. This is the
/// headless smoke harness itself — opt-in via `QUADRAUI_GTK_SMOKE_MS`,
/// invoked directly by `xvfb-run`/CI, never by a host embedding a live
/// quadraui backend — so its failure output *is* the tool's normal
/// output, the same way a CLI's own diagnostics aren't routed through
/// `diagnostics::emit`.
#[allow(clippy::print_stderr)]
fn schedule_smoke_check<A: AppLogic + 'static>(
    cfg: SmokeConfig,
    da: DrawingArea,
    backend: Rc<RefCell<GtkBackend>>,
    app: Rc<RefCell<A>>,
    window: ApplicationWindow,
    smoke_failed: Rc<Cell<bool>>,
) {
    glib::source::timeout_add_local_once(Duration::from_millis(cfg.after_ms), move || {
        let width = da.width();
        let height = da.height();
        if !smoke_size_ok(width, height, SMOKE_MIN_WIDTH, SMOKE_MIN_HEIGHT) {
            eprintln!(
                "quadraui smoke: DrawingArea size looks broken ({width}x{height}px, \
                 expected at least {SMOKE_MIN_WIDTH}x{SMOKE_MIN_HEIGHT}px) — \
                 this is the quadraui#437 tiny-window regression class"
            );
            smoke_failed.set(true);
        }

        if let Some(text) = &cfg.paste_text {
            let read_back = {
                let backend_ref = backend.borrow();
                let clipboard = backend_ref.services().clipboard();
                clipboard.write_text(text);
                clipboard.read_text()
            };
            if !smoke_clipboard_round_trip_ok(text, read_back.as_deref()) {
                eprintln!(
                    "quadraui smoke: OS clipboard round-trip failed — wrote {text:?}, \
                     read back {read_back:?} (needs a real DISPLAY, e.g. Xvfb — \
                     the Broadway GDK backend has no OS clipboard to round-trip through)"
                );
                smoke_failed.set(true);
            } else {
                // Also exercise the real Ctrl-V interception path (the
                // exact code the live key controller calls), so a
                // regression there — not just in the raw OS clipboard —
                // fails the smoke too.
                let ev = UiEvent::KeyPressed {
                    key: Key::Char('v'),
                    modifiers: Modifiers {
                        ctrl: true,
                        shift: false,
                        alt: false,
                        cmd: false,
                    },
                    repeat: false,
                };
                let mut backend_mut = backend.borrow_mut();
                let mut app_mut = app.borrow_mut();
                dispatch_event(ev, &mut backend_mut, &mut *app_mut);
            }
        }

        // `destroy()`, not `close()` (quadraui#501 review): this forced
        // close must happen unconditionally, regardless of what (if
        // anything) the app's `WindowClose` handler returns — that's
        // the whole point of a headless timeout. `close()` would emit
        // `close-request` and run straight into the veto handler
        // installed above in `activate`; an unattended `xvfb-run`
        // invocation against any example with no `WindowClose` opinion
        // (the unhandled-catch-all default, true of every example in
        // this repo today) would then hang forever instead of exiting
        // — exactly the failure mode this smoke check exists to
        // prevent. `destroy()` tears the window down directly without
        // emitting `close-request`, so the timeout always actually
        // fires.
        window.destroy();
    });
}

/// [`ReactionSink`] over a borrowed `DrawingArea` + `ApplicationWindow`
/// pair, plus (quadraui#832) the `GtkBackend` a `RedrawAfter` outcome
/// arms its scheduled wake against — the redraw/exit/frame-request
/// target every GTK signal closure in this module applies its
/// [`EventOutcome`] (or raw [`Reaction`]) to, via
/// [`runtime::apply_outcome`]. Replaces what used to be two near-
/// identical hand-rolled `match` functions (quadraui#496).
///
/// Takes `backend: &'a GtkBackend` (a shared reference, not the
/// `Rc<RefCell<GtkBackend>>` every call site already holds) — cheap to
/// obtain even from a live `RefMut` several call sites already hold
/// across the `apply_event_outcome` call (e.g. the mouse-release
/// handler below, which processes a `Vec<UiEvent>` in a loop): a plain
/// `&*backend_mut` reborrow, not a second `borrow_mut()`/`borrow()` on
/// the `RefCell` that would panic against an already-live borrow.
/// `GtkBackend::request_frame_in` only ever needs `&self` (see its doc)
/// so this never needs more than that.
struct GtkSink<'a> {
    backend: &'a GtkBackend,
    da: &'a DrawingArea,
    window: &'a ApplicationWindow,
}

impl ReactionSink for GtkSink<'_> {
    fn request_redraw(&self) {
        self.da.queue_draw();
    }
    fn request_exit(&self) {
        // `destroy()`, not `close()` (quadraui#501 review): the app has
        // already decided to exit — via whatever event produced this
        // `Reaction::Exit`/`EventOutcome::Exit`, e.g. a "press q to
        // quit" key handler, not necessarily `WindowClose` at all. If
        // this called `close()` it would emit GTK's `close-request`
        // signal, re-entering the veto handler installed in `activate`
        // (see its "Window close" comment block) as a brand-new,
        // synthetic `WindowClose` dispatch. An app with no opinion on
        // `WindowClose` — the unhandled catch-all default, true of
        // every example in this repo — would then veto the exit it
        // itself just asked for. `destroy()` tears the window down
        // directly without emitting `close-request`, so a
        // programmatic exit always actually exits.
        self.window.destroy();
    }
    fn request_frame_in(&self, delay: Duration) {
        self.backend.request_frame_in(delay);
    }
}

fn apply_reaction(
    reaction: Reaction,
    backend: &GtkBackend,
    da: &DrawingArea,
    window: &ApplicationWindow,
) {
    runtime::apply_outcome(
        reaction,
        &GtkSink {
            backend,
            da,
            window,
        },
    );
}

/// Same as [`apply_reaction`] but for the [`EventOutcome`] that
/// [`dispatch_event`] returns.
fn apply_event_outcome(
    outcome: EventOutcome,
    backend: &GtkBackend,
    da: &DrawingArea,
    window: &ApplicationWindow,
) {
    runtime::apply_outcome(
        outcome,
        &GtkSink {
            backend,
            da,
            window,
        },
    );
}

/// Render one frame into `cr` at `width`×`height` pixels.
///
/// Builds a fresh Pango context + layout from `cr` (Cairo per-surface
/// font metrics), seeds the backend's per-frame font/metric state,
/// clears the surface to the current theme background, runs
/// `app.render` inside [`GtkBackend::enter_frame_scope`], and overlays
/// the active text-selection highlight. This is the exact body the live
/// `set_draw_func` used to run inline — extracted so it never depends on
/// a real `DrawingArea` widget, only a `Context` + pixel size. Shared by
/// the live runner and [`crate::gtk::testing::GtkDriver`] (quadraui#446)
/// — see the module doc's "Shared with the headless test driver"
/// section.
pub(crate) fn render_frame<A: AppLogic>(
    backend: &mut GtkBackend,
    app: &A,
    cr: &Context,
    width: i32,
    height: i32,
) {
    let pango_ctx = pcfn::create_context(cr);
    let layout = pg::Layout::new(&pango_ctx);
    // Editor font — defaults to system monospace, size 11, but is
    // app-configurable via `Backend::set_editor_font`
    // (`ShellConfig::with_editor_font` for `ShellApp` consumers). Read
    // fresh every frame so a runtime font change takes effect on the
    // next repaint (#422). Monospace is required because `draw_editor`'s
    // scroll formula (`scroll_left * char_width`) assumes uniform glyph
    // width; the untouched default resolves to the fontconfig monospace
    // alias (DejaVu Sans Mono, JetBrains Mono, etc).
    let font_desc_str = backend.editor_font_pango_string();
    let font_desc = pg::FontDescription::from_string(&font_desc_str);
    layout.set_font_description(Some(&font_desc));
    // Single-line, no wrap. Belt-and-braces over the rasterisers that
    // also call `set_width(-1)` themselves.
    layout.set_width(-1);

    // Resolve font metrics for the default font and seed the backend's
    // per-frame state.
    let metrics = pango_ctx.metrics(Some(&font_desc), None);
    let line_h = (metrics.ascent() + metrics.descent()) as f64 / pg::SCALE as f64;
    // Measure actual laid-out character width instead of
    // `approximate_char_width()` — the approximate value doesn't
    // account for font hinting and drifts over long lines (e.g. 9 chars
    // short at 500-char scroll).
    layout.set_text("0");
    let (char_w_px, _) = layout.pixel_size();
    let char_w = char_w_px as f64;

    // #834: seed from the tracked scale factor (kept current by the
    // `notify::scale-factor` handler and the debounced resize handler
    // below) rather than a hardcoded `1.0`, so `Backend::viewport().scale`
    // reflects the real backing scale even between `WindowResized`
    // dispatches.
    backend.begin_frame(crate::Viewport::new(
        width as f32,
        height as f32,
        backend.dpi_scale(),
    ));
    backend.set_current_line_height(line_h);
    backend.set_current_char_width(char_w);
    // Deliberately *not* re-seeding `ui_font` here every frame the way
    // `current_line_height`/`current_char_width` are: those are metrics
    // re-derived from the editor font each repaint, but `ui_font` is a
    // static app-level chrome-font preference (`Backend::set_ui_font`,
    // #624) that `setup()` sets once. Stomping it back to the struct's
    // default here would silently undo that call on the very next frame.

    // Clear the whole surface with the backend's current theme bg before
    // the app's `render` runs. Without this, GTK's default light-theme
    // white shows through anywhere the app doesn't explicitly paint,
    // which clashes with the primitive surface colours. Vimcode does the
    // same as step 1 of every draw flow.
    let bg = backend.current_theme().background;
    cr.set_source_rgb(
        bg.r as f64 / 255.0,
        bg.g as f64 / 255.0,
        bg.b as f64 / 255.0,
    );
    cr.paint().ok();

    // Issue #830: resolve the currently-focused widget's rect (if any)
    // from this frame's tab stops before entering the frame scope —
    // mirrors `tui::run::paint_frame`.
    let focus_ring_rect = backend.focus_manager().focused().cloned().and_then(|id| {
        app.tab_stops(A::AreaId::default())
            .into_iter()
            .find(|(stop_id, _)| *stop_id == id)
            .map(|(_, rect)| rect)
    });
    backend.enter_frame_scope(cr, &layout, |b| {
        // Single-area runner: pass the default `AreaId`.
        app.render(b, A::AreaId::default());
        // After app.render: paint the focus-ring convention (#830).
        if let Some(rect) = focus_ring_rect {
            b.draw_focus_ring(rect);
        }
    });

    // After app.render: overlay selection highlight on top of the
    // rendered content (mirrors TUI's apply_selection_highlight call in
    // the terminal.draw closure).
    backend.apply_selection_highlight(cr);

    backend.end_frame();
}

// `is_paste_keypress` (plain Ctrl-V / Ctrl-Shift-V, quadraui#415) used to
// be defined here; lifted to the backend-neutral `desktop` module (#728)
// so `macos::run` and `win::run` share the exact same predicate instead
// of each reimplementing (or, for `win`, never implementing) it — see
// that module's doc and `docs/decisions/DECISIONS.md` D-011 for the shift-
// tolerance contract this settles once for every adopter.

// `EventOutcome` — what the caller should do after [`dispatch_event`]
// handles one event — is defined once in `crate::runtime` and shared by
// every backend runner (quadraui#496); imported at the top of this file.

/// Dispatch one already-translated [`UiEvent`] through the app, applying
/// the shared runner pre-processing pipeline first. This is the single
/// funnel every GTK signal closure above routes through (key press,
/// click press/release, motion, scroll, resize, idle-drain) — see the
/// module doc's "Shared with the headless test driver" section for why
/// that matters.
///
/// The pre-processing itself — ActivityBar keyboard-focus redirect,
/// Tab/Shift+Tab focus cycling (#830),
/// global accelerator rewrite, Ctrl-C copy, Ctrl-V/Ctrl-Shift-V paste,
/// middle-click PRIMARY-selection paste, Ctrl-A select-all, selection-
/// display clearing, `TextSelectionChanged` — lives in
/// [`crate::runtime::preprocess_event`] (quadraui#813), shared with
/// TUI/macOS/Windows; see that function's doc for the exact priority
/// order. This wrapper exists only because `preprocess_event` is
/// generic and GTK's signal closures call a concretely-typed
/// `dispatch_event(UiEvent, &mut GtkBackend, &mut A)`.
pub(crate) fn dispatch_event<A: AppLogic>(
    event: UiEvent,
    backend: &mut GtkBackend,
    app: &mut A,
) -> EventOutcome {
    crate::runtime::preprocess_event(event, backend, app)
}

// ── #902: re-entrancy backstop ──────────────────────────────────────
//
// A GTK trampoline handler must never panic — see the long comment above
// `pump_depth`'s definition (`#427`) and the module's `close-request`
// handler for the full incident this class of helper exists to prevent.
// `pump_depth.is_pumping()` (checked by every signal closure above,
// *before* reaching one of these) guards exactly one re-entrancy source:
// a nested modal pump. It says nothing about a second, independent
// source — app code that's already inside a `dispatch_event` call
// synchronously invoking a GTK method that re-emits the very signal a
// handler below is listening for (`window.close()` → `close-request`
// being the reported case; nothing about the hazard is specific to
// that one signal). `pump_depth` is depth 0 the whole time that happens,
// so it waves the re-entry straight through into a second `borrow_mut()`
// on an already-mutably-borrowed `RefCell` — a panic that can't unwind
// across the `extern "C"` GLib trampoline frame above it, aborting the
// process.
//
// Rather than add a second enumerated counter (which would fix only the
// source that has already been found), these ask the `RefCell`s
// themselves — they already know the true answer, including for sources
// nobody has enumerated yet. Every caller below must treat a `None`
// return as "degrade, don't dispatch" — most just skip the event, the
// same trade the `pump_depth` guard next to them already makes; the
// `close-request` handler additionally *defers* it via `events_handle`
// (see that handler) because silently dropping a `WindowClose` would
// make an in-dispatch `close()` call a permanent, silent no-op.

/// Attempt to mutably borrow both `backend` and `app`, returning `None`
/// instead of panicking if either is already borrowed. See the `#902`
/// section above.
fn try_borrow_both<'a, A>(
    backend: &'a Rc<RefCell<GtkBackend>>,
    app: &'a Rc<RefCell<A>>,
) -> Option<(std::cell::RefMut<'a, GtkBackend>, std::cell::RefMut<'a, A>)> {
    let backend_mut = backend.try_borrow_mut().ok()?;
    let app_mut = app.try_borrow_mut().ok()?;
    Some((backend_mut, app_mut))
}

/// [`try_borrow_both`] + [`dispatch_event`], running `pre` on the
/// borrowed backend in between (e.g. `set_dpi_scale`) before `app` is
/// borrowed and the event dispatched. `None` on a double-borrow — see
/// the `#902` section above.
fn try_dispatch_with<A: AppLogic>(
    backend: &Rc<RefCell<GtkBackend>>,
    app: &Rc<RefCell<A>>,
    pre: impl FnOnce(&mut GtkBackend),
    ev: UiEvent,
) -> Option<EventOutcome> {
    let (mut backend_mut, mut app_mut) = try_borrow_both(backend, app)?;
    pre(&mut backend_mut);
    Some(dispatch_event(ev, &mut backend_mut, &mut *app_mut))
}

/// [`try_dispatch_with`] with no pre-dispatch step — the common case.
fn try_dispatch<A: AppLogic>(
    backend: &Rc<RefCell<GtkBackend>>,
    app: &Rc<RefCell<A>>,
    ev: UiEvent,
) -> Option<EventOutcome> {
    try_dispatch_with(backend, app, |_| {}, ev)
}

/// Like [`try_dispatch`] but for callers that already hold `backend`
/// mutably borrowed across a loop of several events (click press/release,
/// motion) and only need `app` borrowed per-event. `None` on a
/// double-borrow of `app` — see the `#902` section above.
fn try_dispatch_borrowed<A: AppLogic>(
    backend_mut: &mut GtkBackend,
    app: &Rc<RefCell<A>>,
    ev: UiEvent,
) -> Option<EventOutcome> {
    let mut app_mut = app.try_borrow_mut().ok()?;
    Some(dispatch_event(ev, backend_mut, &mut *app_mut))
}

#[cfg(test)]
mod run_config_tests {
    //! Coverage for [`RunConfig`] (quadraui#234) — display-free, since
    //! `RunConfig` itself is a plain data struct with no GTK dependency.
    //! `run_with`'s actual wiring (app id → `Application::builder`, title
    //! → `ApplicationWindow::builder`) can't be exercised without a real
    //! display; that's covered by the operator-run smoke tier same as the
    //! rest of this module (see `smoke_tests` below).
    use super::*;

    #[test]
    fn default_matches_runs_previous_hardcoded_values() {
        // `run(app)` used to hardcode these two literals directly into
        // `Application::builder()` / `ApplicationWindow::builder()`. Now
        // that `run` is `run_with(app, RunConfig::default())`, this pins
        // the default so it can't silently drift and change every
        // existing caller's app id / window title.
        let config = RunConfig::default();
        assert_eq!(config.app_id, "org.quadraui.app");
        assert_eq!(config.title, "quadraui app");
        assert_eq!(config.icon_name, None);
    }

    #[test]
    fn new_sets_both_fields() {
        let config = RunConfig::new("io.github.jdonaghy.kubeui-gtk", "kubeui");
        assert_eq!(config.app_id, "io.github.jdonaghy.kubeui-gtk");
        assert_eq!(config.title, "kubeui");
        assert_eq!(config.icon_name, None);
    }

    #[test]
    fn new_accepts_owned_and_borrowed_strings() {
        let owned = RunConfig::new(String::from("a.b.c"), String::from("Title"));
        let borrowed = RunConfig::new("a.b.c", "Title");
        assert_eq!(owned, borrowed);
    }

    /// #656: `with_icon_name` stores the icon name for `activate` to feed
    /// into `ApplicationWindow::builder().icon_name(..)`.
    #[test]
    fn with_icon_name_sets_the_icon() {
        let config = RunConfig::new("a.b.c", "Title").with_icon_name("io.github.jdonaghy.vimcode");
        assert_eq!(
            config.icon_name,
            Some("io.github.jdonaghy.vimcode".to_string())
        );
    }
}

#[cfg(test)]
mod smoke_tests {
    //! Unit tests for GTK's use of the headless smoke-mode helpers
    //! (quadraui#450, GD-5). The predicates themselves (`smoke_size_ok`,
    //! `smoke_clipboard_round_trip_ok`) and `SmokeConfig::from_env`'s
    //! generic parsing are covered once, backend-neutrally, in
    //! `crate::desktop`'s own tests (#498) — these pin GTK's specific
    //! wiring on top: the real `DEFAULT_WINDOW_WIDTH`/`HEIGHT` and
    //! `SMOKE_MIN_WIDTH`/`HEIGHT` floor values, and the real
    //! `QUADRAUI_GTK_SMOKE_MS`/`_PASTE` env var names. Pure/display-free
    //! by design — the live Xvfb/Broadway run itself is an operator-run
    //! tier documented in `quadraui/docs/TESTING.md`, not something CI
    //! (no Xvfb — see `ci.yml`) or this in-process test can exercise.
    use super::*;

    #[test]
    fn smoke_size_ok_accepts_the_default_window_size() {
        assert!(smoke_size_ok(
            DEFAULT_WINDOW_WIDTH,
            DEFAULT_WINDOW_HEIGHT,
            SMOKE_MIN_WIDTH,
            SMOKE_MIN_HEIGHT
        ));
    }

    #[test]
    fn smoke_size_ok_accepts_exactly_the_floor() {
        assert!(smoke_size_ok(
            SMOKE_MIN_WIDTH,
            SMOKE_MIN_HEIGHT,
            SMOKE_MIN_WIDTH,
            SMOKE_MIN_HEIGHT
        ));
    }

    #[test]
    fn smoke_size_ok_rejects_the_437_tiny_window_class() {
        // quadraui#437: content wrapped into an ~8px-wide column.
        assert!(!smoke_size_ok(
            8,
            DEFAULT_WINDOW_HEIGHT,
            SMOKE_MIN_WIDTH,
            SMOKE_MIN_HEIGHT
        ));
        assert!(!smoke_size_ok(
            DEFAULT_WINDOW_WIDTH,
            8,
            SMOKE_MIN_WIDTH,
            SMOKE_MIN_HEIGHT
        ));
        assert!(!smoke_size_ok(8, 8, SMOKE_MIN_WIDTH, SMOKE_MIN_HEIGHT));
    }

    #[test]
    fn smoke_size_ok_rejects_just_under_the_floor() {
        assert!(!smoke_size_ok(
            SMOKE_MIN_WIDTH - 1,
            SMOKE_MIN_HEIGHT,
            SMOKE_MIN_WIDTH,
            SMOKE_MIN_HEIGHT
        ));
        assert!(!smoke_size_ok(
            SMOKE_MIN_WIDTH,
            SMOKE_MIN_HEIGHT - 1,
            SMOKE_MIN_WIDTH,
            SMOKE_MIN_HEIGHT
        ));
    }

    #[test]
    fn clipboard_round_trip_ok_when_read_back_matches() {
        assert!(smoke_clipboard_round_trip_ok(
            "quadraui smoke",
            Some("quadraui smoke")
        ));
    }

    #[test]
    fn clipboard_round_trip_rejects_a_missing_or_mismatched_read() {
        // The missing-read failure mode a headless box with no OS
        // clipboard access actually produces (e.g. Broadway, no real
        // `DISPLAY`); the mismatched-read case rounds out the branch.
        assert!(!smoke_clipboard_round_trip_ok("quadraui smoke", None));
        assert!(!smoke_clipboard_round_trip_ok(
            "quadraui smoke",
            Some("something else")
        ));
    }

    /// #498: GTK's real env-var names still enable smoke mode through
    /// the now-shared `SmokeConfig::from_env` — a rename regression here
    /// would silently disable the `xvfb-run`/CI-adjacent smoke wrapper
    /// (`quadraui/scripts/gtk_smoke.sh`) without any test going red.
    #[test]
    fn smoke_config_from_env_reads_gtks_own_var_names() {
        // Isolated from other env-touching tests only by using var names
        // no other test in this crate reads or writes.
        std::env::remove_var(SMOKE_MS_VAR);
        std::env::remove_var(SMOKE_PASTE_VAR);
        assert_eq!(SmokeConfig::from_env(SMOKE_MS_VAR, SMOKE_PASTE_VAR), None);

        std::env::set_var(SMOKE_MS_VAR, "500");
        let cfg = SmokeConfig::from_env(SMOKE_MS_VAR, SMOKE_PASTE_VAR)
            .expect("QUADRAUI_GTK_SMOKE_MS set and parseable");
        assert_eq!(cfg.after_ms, 500);
        assert_eq!(cfg.paste_text, None);
        std::env::remove_var(SMOKE_MS_VAR);
    }
}

#[cfg(test)]
mod paste_tests {
    //! Coverage for quadraui#415 — GTK clipboard-paste and PRIMARY-
    //! selection routing added to [`dispatch_event`].
    //!
    //! `is_paste_keypress` (shared with `macos::run`/`win::run` since
    //! #728 — see `crate::desktop`) is a pure predicate, tested directly
    //! with no display required; its full cross-backend contract
    //! (Shift/Cmd tolerance, D-011 in `docs/decisions/DECISIONS.md`) has its own
    //! coverage in `crate::desktop`'s test module, so the pure-predicate
    //! tests below stick to GTK's own Ctrl-based cases. The
    //! `GtkDriver`-based tests below exercise the actual
    //! [`dispatch_event`] wiring end to end.
    //!
    //! Each of those installs an in-memory clipboard via
    //! [`GtkBackend::install_test_clipboard`] before dispatching, so both
    //! the "there IS something to paste" and "there is nothing to paste"
    //! branches are covered on *any* host. Reading the host's real
    //! clipboard instead would make these assertions environment-
    //! dependent — green on a headless box where
    //! `arboard::Clipboard::new()` fails, red on a developer machine or a
    //! CI runner with a live display and a non-empty clipboard.
    use super::*;
    use crate::desktop::{is_paste_keypress, PasteModifier};
    use crate::gtk::services::TestClipboardContents;
    use crate::gtk::testing::GtkDriver;

    /// Minimal [`AppLogic`] that records every event `handle` receives,
    /// so tests can assert on exactly what reached the app — in
    /// particular, that an intercepted paste trigger does NOT also
    /// forward the raw key/mouse event underneath it.
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

    // ── `is_paste_keypress` — pure predicate ──────────────────────────

    #[test]
    fn plain_ctrl_v_is_a_paste_keypress() {
        let mods = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        assert!(is_paste_keypress(
            &Key::Char('v'),
            &mods,
            PasteModifier::Ctrl
        ));
        assert!(is_paste_keypress(
            &Key::Char('V'),
            &mods,
            PasteModifier::Ctrl
        ));
    }

    #[test]
    fn ctrl_shift_v_is_a_paste_keypress() {
        // quadraui#415: several terminal emulators reserve Ctrl-V for a
        // control byte and use Ctrl-Shift-V for paste instead.
        let mods = Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::default()
        };
        assert!(is_paste_keypress(
            &Key::Char('v'),
            &mods,
            PasteModifier::Ctrl
        ));
    }

    #[test]
    fn ctrl_alt_v_is_not_a_paste_keypress() {
        let mods = Modifiers {
            ctrl: true,
            alt: true,
            ..Modifiers::default()
        };
        assert!(!is_paste_keypress(
            &Key::Char('v'),
            &mods,
            PasteModifier::Ctrl
        ));
    }

    #[test]
    fn ctrl_cmd_v_is_not_a_paste_keypress() {
        let mods = Modifiers {
            ctrl: true,
            cmd: true,
            ..Modifiers::default()
        };
        assert!(!is_paste_keypress(
            &Key::Char('v'),
            &mods,
            PasteModifier::Ctrl
        ));
    }

    #[test]
    fn plain_super_v_is_not_a_paste_keypress() {
        // D-011 §4 regression check (#728 fix iteration 1): `modifiers.cmd`
        // is Super/Meta on GTK (`gdk_modifiers_to_quadraui`), and plain
        // Super+V must never trigger paste on GTK — that was true before
        // `is_paste_keypress` was lifted into `desktop`, and passing
        // `PasteModifier::Ctrl` here is what keeps it true afterward.
        let mods = Modifiers {
            cmd: true,
            ..Modifiers::default()
        };
        assert!(!is_paste_keypress(
            &Key::Char('v'),
            &mods,
            PasteModifier::Ctrl
        ));
    }

    #[test]
    fn shift_v_without_ctrl_is_not_a_paste_keypress() {
        let mods = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        assert!(!is_paste_keypress(
            &Key::Char('v'),
            &mods,
            PasteModifier::Ctrl
        ));
    }

    #[test]
    fn plain_v_is_not_a_paste_keypress() {
        assert!(!is_paste_keypress(
            &Key::Char('v'),
            &Modifiers::default(),
            PasteModifier::Ctrl
        ));
    }

    // ── `dispatch_event` wiring (GtkDriver, no display) ───────────────

    /// Build a driver whose clipboard is an in-memory fake seeded with
    /// `clipboard` (the CLIPBOARD selection Ctrl-V reads) and `primary`
    /// (the PRIMARY selection middle-click reads). Nothing here touches
    /// the host's real clipboard, so every assertion below holds on a
    /// headless CI runner and on a developer desktop alike.
    fn driver_with_clipboard(
        clipboard: Option<&str>,
        primary: Option<&str>,
    ) -> GtkDriver<RecordingApp> {
        let driver = GtkDriver::new(RecordingApp::default(), 100, 30);
        driver
            .backend()
            .install_test_clipboard(TestClipboardContents {
                clipboard: clipboard.map(str::to_string),
                primary: primary.map(str::to_string),
            });
        driver
    }

    /// Dispatch `v` with the given modifiers.
    fn press_v(driver: &mut GtkDriver<RecordingApp>, modifiers: Modifiers) {
        driver.dispatch(UiEvent::KeyPressed {
            key: Key::Char('v'),
            modifiers,
            repeat: false,
        });
    }

    fn middle_click(driver: &mut GtkDriver<RecordingApp>) {
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Middle,
            position: Point::new(5.0, 5.0),
            modifiers: Modifiers::default(),
        });
    }

    /// The single event the app received, or a panic naming what it got.
    fn only_event(driver: &GtkDriver<RecordingApp>) -> &UiEvent {
        let events = &driver.app().events;
        assert_eq!(
            events.len(),
            1,
            "expected exactly one event to reach app.handle, got {events:?}"
        );
        &events[0]
    }

    #[test]
    fn ctrl_v_delivers_the_clipboard_selection_as_a_paste() {
        let mut driver = driver_with_clipboard(Some("copied text"), None);
        press_v(
            &mut driver,
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
        );
        assert!(
            matches!(only_event(&driver), UiEvent::ClipboardPaste(t) if t == "copied text"),
            "Ctrl-V should deliver ClipboardPaste with the CLIPBOARD contents, got {:?}",
            driver.app().events
        );
    }

    #[test]
    fn ctrl_shift_v_delivers_the_clipboard_selection_as_a_paste() {
        // quadraui#415: Ctrl-Shift-V is the paste shortcut in terminal
        // emulators that reserve Ctrl-V for a literal control byte.
        let mut driver = driver_with_clipboard(Some("copied text"), None);
        press_v(
            &mut driver,
            Modifiers {
                ctrl: true,
                shift: true,
                ..Modifiers::default()
            },
        );
        assert!(
            matches!(only_event(&driver), UiEvent::ClipboardPaste(t) if t == "copied text"),
            "Ctrl-Shift-V should deliver ClipboardPaste, got {:?}",
            driver.app().events
        );
    }

    #[test]
    fn ctrl_shift_v_is_intercepted_not_forwarded_as_raw_v() {
        let mut driver = driver_with_clipboard(None, None);
        press_v(
            &mut driver,
            Modifiers {
                ctrl: true,
                shift: true,
                ..Modifiers::default()
            },
        );
        // Empty clipboard → nothing to paste, but the keypress must
        // still be swallowed here rather than falling through to
        // `app.handle` as a literal 'v' character (which would be wrong
        // for a focused terminal / text input).
        assert!(
            driver.app().events.is_empty(),
            "Ctrl-Shift-V should be intercepted as a paste attempt, not \
             forwarded to app.handle as a raw keypress: {:?}",
            driver.app().events
        );
    }

    #[test]
    fn ctrl_alt_v_falls_through_to_app_unmodified() {
        // Clipboard deliberately non-empty: proves the fall-through is
        // driven by the modifier combination, not by an empty clipboard.
        let mut driver = driver_with_clipboard(Some("copied text"), None);
        press_v(
            &mut driver,
            Modifiers {
                ctrl: true,
                alt: true,
                ..Modifiers::default()
            },
        );
        assert!(
            matches!(
                only_event(&driver),
                UiEvent::KeyPressed {
                    key: Key::Char('v'),
                    ..
                }
            ),
            "Ctrl-Alt-V is not a paste trigger and should reach app.handle unmodified, got {:?}",
            driver.app().events
        );
    }

    #[test]
    fn middle_click_delivers_the_primary_selection_as_a_paste() {
        let mut driver = driver_with_clipboard(None, Some("selected text"));
        middle_click(&mut driver);
        assert!(
            matches!(only_event(&driver), UiEvent::ClipboardPaste(t) if t == "selected text"),
            "middle-click should deliver ClipboardPaste with the PRIMARY selection, got {:?}",
            driver.app().events
        );
    }

    #[test]
    fn middle_click_reads_primary_selection_not_the_clipboard() {
        // The two selections are distinct on X11/Wayland; middle-click
        // must never fall back to CLIPBOARD (quadraui#415).
        let mut driver = driver_with_clipboard(Some("CLIPBOARD"), Some("PRIMARY"));
        middle_click(&mut driver);
        assert!(
            matches!(only_event(&driver), UiEvent::ClipboardPaste(t) if t == "PRIMARY"),
            "middle-click pasted the wrong selection: {:?}",
            driver.app().events
        );
    }

    #[test]
    fn middle_click_without_primary_selection_falls_through_to_app() {
        // Empty PRIMARY but a non-empty CLIPBOARD: the intercept must
        // not swallow the click (and must not substitute CLIPBOARD); it
        // should reach app.handle as an ordinary MouseDown so apps
        // without terminal focus still see it.
        let mut driver = driver_with_clipboard(Some("CLIPBOARD"), None);
        middle_click(&mut driver);
        assert!(
            matches!(
                only_event(&driver),
                UiEvent::MouseDown {
                    button: MouseButton::Middle,
                    ..
                }
            ),
            "middle-click with no PRIMARY selection should fall through, got {:?}",
            driver.app().events
        );
    }

    // ── Double-click folding (quadraui#813) ───────────────────────────

    /// Before #813, `GtkDriver::dispatch` (which calls `dispatch_event`
    /// directly, the same path `GtkDriver::mouse_down`/click helpers use)
    /// had **no** double-click folding at all — GTK only folded from
    /// native `GdkEventType`/`GestureClick` press-count inside
    /// `gtk/run.rs`'s own `connect_pressed` closure, a real GDK signal
    /// `GtkDriver` never fires. Two scripted `MouseDown`s at the same
    /// spot were always delivered as two separate `MouseDown`s to the
    /// app — this is the RED #813's acceptance bar calls for: GTK could
    /// not be driver-tested for double-click before this function moved
    /// the fold into the shared `dispatch_event`/`preprocess_event` path
    /// every backend now uses.
    #[test]
    fn second_mouse_down_at_same_spot_folds_into_double_click() {
        let mut driver = driver_with_clipboard(None, None);
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(5.0, 5.0),
            modifiers: Modifiers::default(),
        });
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(5.0, 5.0),
            modifiers: Modifiers::default(),
        });

        assert_eq!(
            driver.app().events.len(),
            2,
            "expected exactly two events (MouseDown, DoubleClick), got {:?}",
            driver.app().events
        );
        assert!(
            matches!(driver.app().events[0], UiEvent::MouseDown { .. }),
            "first press should reach the app as a plain MouseDown, got {:?}",
            driver.app().events[0]
        );
        assert!(
            matches!(driver.app().events[1], UiEvent::DoubleClick { .. }),
            "second press at the same spot should fold to DoubleClick, got {:?}",
            driver.app().events[1]
        );
    }

    /// A second click far from the first must stay two plain
    /// `MouseDown`s — the position-tolerance half of the same contract.
    #[test]
    fn second_mouse_down_far_away_stays_two_mouse_downs() {
        let mut driver = driver_with_clipboard(None, None);
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(5.0, 5.0),
            modifiers: Modifiers::default(),
        });
        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(80.0, 40.0),
            modifiers: Modifiers::default(),
        });

        assert_eq!(driver.app().events.len(), 2);
        assert!(
            driver
                .app()
                .events
                .iter()
                .all(|e| matches!(e, UiEvent::MouseDown { .. })),
            "clicks far apart must never fold into a DoubleClick, got {:?}",
            driver.app().events
        );
    }
}

#[cfg(test)]
mod window_close_tests {
    //! Coverage for quadraui#501 — `UiEvent::WindowClose` dispatch.
    //!
    //! `connect_close_request` itself (the live GDK signal → `dispatch`
    //! → `glib::Propagation` wiring in `activate`) needs a real
    //! `ApplicationWindow`/display and can't run through `GtkDriver`
    //! (deliberately display-free — see its module doc); that half is
    //! covered by the GTK live-window smoke tier instead. What *is*
    //! headless-testable, and what actually matters for app authors, is
    //! that `dispatch_event` doesn't silently intercept or rewrite
    //! `WindowClose` the way it does `KeyPressed`/`MouseDown` for
    //! accelerators, paste, etc. — it must reach `app.handle` verbatim
    //! and round-trip the app's `Reaction` through `EventOutcome`
    //! unchanged, since that's the veto mechanism: `activate` only lets
    //! the OS proceed with the close when the app returns `Exit`.
    use super::*;
    use crate::gtk::testing::GtkDriver;

    #[derive(Default)]
    struct RecordingApp {
        events: Vec<UiEvent>,
        reaction: Option<Reaction>,
    }

    impl AppLogic for RecordingApp {
        type AreaId = ();

        fn render(&self, _backend: &mut dyn Backend, _area: ()) {}

        fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            self.events.push(event);
            self.reaction.unwrap_or(Reaction::Continue)
        }
    }

    #[test]
    fn window_close_reaches_app_handle_unmodified() {
        let mut driver = GtkDriver::new(RecordingApp::default(), 400, 300);
        let reaction = driver.dispatch(UiEvent::WindowClose);
        assert_eq!(reaction, Reaction::Continue, "no veto ⇒ Continue, not Exit");
        assert_eq!(
            driver.app().events,
            vec![UiEvent::WindowClose],
            "WindowClose must not be rewritten (e.g. into Accelerator) or swallowed \
             by any of dispatch_event's pre-processing branches"
        );
    }

    /// An app returning `Reaction::Exit` from its `WindowClose` handler
    /// is the "don't veto, let the OS close the window" path —
    /// `activate`'s `close-request` handler maps this outcome to
    /// `glib::Propagation::Proceed`.
    #[test]
    fn window_close_can_be_accepted() {
        let mut driver = GtkDriver::new(
            RecordingApp {
                reaction: Some(Reaction::Exit),
                ..Default::default()
            },
            400,
            300,
        );
        assert_eq!(driver.dispatch(UiEvent::WindowClose), Reaction::Exit);
    }

    /// An app returning anything other than `Exit` (here: `Redraw`, e.g.
    /// to show an "unsaved changes" prompt) is the veto path —
    /// `activate`'s handler maps every non-`Exit` outcome to
    /// `glib::Propagation::Stop` and keeps the window open.
    #[test]
    fn window_close_can_be_vetoed() {
        let mut driver = GtkDriver::new(
            RecordingApp {
                reaction: Some(Reaction::Redraw),
                ..Default::default()
            },
            400,
            300,
        );
        assert_eq!(driver.dispatch(UiEvent::WindowClose), Reaction::Redraw);
    }
}

#[cfg(test)]
mod reentrancy_backstop_tests {
    //! Coverage for quadraui#902 — the structural re-entrancy backstop
    //! (`try_borrow_both` / `try_dispatch` / `try_dispatch_with` /
    //! `try_dispatch_borrowed`, all defined just above [`dispatch_event`])
    //! that every guarded signal closure in `activate` now routes
    //! through, replacing a plain `borrow_mut()` that would panic — and,
    //! because the panic crosses a non-unwindable GLib trampoline frame,
    //! abort the whole process — on a double borrow.
    //!
    //! The live `close-request` re-entry itself (`window.close()` called
    //! from inside `AppLogic::handle`, which GTK turns into a synchronous
    //! `close-request` re-emission) needs a real `ApplicationWindow`/
    //! display and can't run through `GtkDriver` — see
    //! `window_close_tests`'s module doc for the identical constraint on
    //! `WindowClose` dispatch generally. What *is* headlessly testable,
    //! and what the issue's own Verification section asks for, is the
    //! degrade path: hold one of `backend`/`app`'s `RefCell`s borrowed —
    //! exactly the state an outer `dispatch_event` call leaves the stack
    //! in — and confirm these helpers return `None` (never panic), and
    //! that a caller can safely defer the event onto `events_handle`
    //! while that borrow is still held.
    use super::*;

    #[derive(Default)]
    struct NoopApp;

    impl AppLogic for NoopApp {
        type AreaId = ();
        fn render(&self, _backend: &mut dyn Backend, _area: ()) {}
        fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            Reaction::Continue
        }
    }

    #[test]
    fn try_borrow_both_succeeds_when_neither_is_borrowed() {
        let backend = Rc::new(RefCell::new(GtkBackend::new()));
        let app = Rc::new(RefCell::new(NoopApp));
        assert!(try_borrow_both(&backend, &app).is_some());
    }

    #[test]
    fn try_borrow_both_degrades_instead_of_panicking_when_backend_is_borrowed() {
        let backend = Rc::new(RefCell::new(GtkBackend::new()));
        let app = Rc::new(RefCell::new(NoopApp));
        // Mirrors the #902 call chain: an outer `dispatch_event` still
        // holds `backend` mutably borrowed on the stack when a re-entered
        // signal handler runs — not a nested modal pump (`pump_depth`
        // already covers that), just app code synchronously triggering
        // the same signal again.
        let _outer_borrow = backend.borrow_mut();
        assert!(
            try_borrow_both(&backend, &app).is_none(),
            "a double borrow_mut() on backend must degrade to None, not panic"
        );
    }

    #[test]
    fn try_borrow_both_degrades_instead_of_panicking_when_app_is_borrowed() {
        let backend = Rc::new(RefCell::new(GtkBackend::new()));
        let app = Rc::new(RefCell::new(NoopApp));
        let _outer_borrow = app.borrow_mut();
        assert!(try_borrow_both(&backend, &app).is_none());
    }

    #[test]
    fn try_dispatch_degrades_when_backend_already_borrowed() {
        let backend = Rc::new(RefCell::new(GtkBackend::new()));
        let app = Rc::new(RefCell::new(NoopApp));
        let _outer_borrow = backend.borrow_mut();
        assert!(try_dispatch(&backend, &app, UiEvent::WindowClose).is_none());
    }

    #[test]
    fn try_dispatch_with_degrades_when_app_already_borrowed() {
        let backend = Rc::new(RefCell::new(GtkBackend::new()));
        let app = Rc::new(RefCell::new(NoopApp));
        let _outer_borrow = app.borrow_mut();
        let mut pre_ran = false;
        let outcome = try_dispatch_with(
            &backend,
            &app,
            |_backend_mut| pre_ran = true,
            UiEvent::WindowClose,
        );
        assert!(outcome.is_none());
        assert!(
            !pre_ran,
            "try_borrow_both borrows backend *and* app before `pre` ever runs, so a \
             failed app borrow must short-circuit before any backend mutation \
             (e.g. set_dpi_scale) happens — no partial side effect from a dispatch \
             that never completes"
        );
    }

    #[test]
    fn try_dispatch_with_runs_pre_then_dispatches_when_neither_is_borrowed() {
        let backend = Rc::new(RefCell::new(GtkBackend::new()));
        let app = Rc::new(RefCell::new(NoopApp));
        let mut pre_ran = false;
        let outcome = try_dispatch_with(
            &backend,
            &app,
            |_backend_mut| pre_ran = true,
            UiEvent::WindowClose,
        );
        assert!(outcome.is_some());
        assert!(pre_ran);
    }

    #[test]
    fn try_dispatch_borrowed_degrades_when_app_already_borrowed() {
        let mut backend = GtkBackend::new();
        let app = Rc::new(RefCell::new(NoopApp));
        let _outer_borrow = app.borrow_mut();
        assert!(try_dispatch_borrowed(&mut backend, &app, UiEvent::WindowClose).is_none());
    }

    /// This is `close-request`'s own degrade path (#902), exercised
    /// directly: when `try_dispatch` reports a double-borrow, the handler
    /// pushes `WindowClose` onto `events_handle` instead of dropping it —
    /// the fix for the reported abort. Confirms both halves: pushing
    /// while `backend` is still held mutably borrowed doesn't panic (the
    /// whole point of deferring through `events_handle` rather than
    /// `backend.push_event` — it's an independent `RefCell`), and the
    /// event actually reaches `poll_events()` once the outer borrow is
    /// released, so it isn't silently lost the way a plain `skip` would
    /// lose it.
    #[test]
    fn close_request_degrade_path_defers_the_event_instead_of_dropping_it() {
        let backend = Rc::new(RefCell::new(GtkBackend::new()));
        let app = Rc::new(RefCell::new(NoopApp));
        let events_handle = backend.borrow().events_handle();

        {
            // Simulate the outer `dispatch_event` call still holding
            // `backend` borrowed when `close-request` re-enters.
            let _outer_borrow = backend.borrow_mut();
            let outcome = try_dispatch(&backend, &app, UiEvent::WindowClose);
            assert!(
                outcome.is_none(),
                "must degrade, not panic, on the double borrow"
            );
            // The exact push `connect_close_request`'s handler does on
            // `None` — must not panic even though `backend` is still
            // borrowed above, because `events_handle` has independent
            // borrow state.
            events_handle.borrow_mut().push_back(UiEvent::WindowClose);
        }

        // Once the outer borrow is released (the stack has unwound back
        // out of the original dispatch), the drain loop's `poll_events`
        // picks the deferred event up like any other.
        let drained = backend.borrow_mut().poll_events();
        assert_eq!(
            drained,
            vec![UiEvent::WindowClose],
            "the deferred WindowClose must reach the app on the next drain tick, not be lost"
        );
    }
}
