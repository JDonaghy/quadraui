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
//! ## Single-DA model
//!
//! The runner uses a **single-DrawingArea** model: one DA, one
//! `set_draw_func`, one `app.render(backend, AreaId::default())` per
//! redraw. Zone routing (sidebar, main, status bar, etc.) is handled
//! entirely by `AppShell::compute_layout` + `FrameHitMap` hit-testing
//! — not by multiple GTK DAs.
//!
//! This was a deliberate decision. All vimcode paint paths
//! already go through quadraui primitives, so per-zone
//! DAs add GTK widget-tree complexity without benefit. The
//! `AppLogic::AreaId` associated type remains in the trait as a
//! compatibility seam but is always `()` in practice.
//!
//! ## Shared with the headless test driver
//!
//! [`render_frame`] and [`dispatch_event`] are `pub(crate)` so the
//! in-process [`crate::gtk::testing::GtkDriver`] renders + dispatches
//! through the *exact same* code as the live runner. The driver swaps
//! the `DrawingArea`'s live `cairo::Context` for one backed by a
//! headless `cairo::ImageSurface` and supplies scripted events instead
//! of real GDK signals — but the frame paint and the event
//! pre-processing (ActivityBar keyboard-focus intercept, Tab/Shift+Tab
//! focus cycling, accelerator matching, Ctrl-C/V/A interception,
//! text-selection state) cannot drift, because every GTK signal closure
//! below routes through these same two functions.
//!
//! ## Headless smoke mode
//!
//! [`GtkDriver`][crate::gtk::testing::GtkDriver] (above) is deliberately
//! display-free, so it structurally cannot catch bugs that only exist in
//! a real `Application` + `ApplicationWindow` + `GdkDisplay`. This
//! motivated the headless smoke-test mode:
//!
//! [`run`] honours two environment variables, read once at startup, so
//! any `gtk_*` example is xvfb-run-friendly with zero example-specific
//! code (every example already goes through this runner):
//!
//! - `QUADRAUI_GTK_SMOKE_MS=<u64>` — enables smoke mode. `after_ms`
//!   milliseconds after the window is presented, the runner checks the
//!   `DrawingArea`'s allocated size against a sane floor
//!   ([`smoke_size_ok`]), then closes the window so an unattended
//!   process exits deterministically instead of hanging forever waiting
//!   for a user who isn't there.
//! - `QUADRAUI_GTK_SMOKE_PASTE=<text>` — optional. If set, the same timer
//!   round-trips `<text>` through the **real OS clipboard** (`arboard`,
//!   the same object the live Ctrl-V handler reads —
//!   `backend.services().clipboard()`) and, if that succeeds, dispatches
//!   a synthetic Ctrl-V `KeyPressed` through [`dispatch_event`] — the
//!   exact code path the live key controller calls — so a regression in
//!   the paste-interception wiring itself also fails the smoke, not just
//!   a raw clipboard failure. `arboard` needs a real `DISPLAY` (Xvfb
//!   provides one; the Broadway backend does not), which is why the
//!   operator-run wrapper uses Xvfb.
//!
//! Any assertion failure is printed to stderr and flips [`run`]'s return
//! value to [`std::process::ExitCode::FAILURE`], overriding GLib's own
//! exit code — see the end of [`run`]. Disabled (zero runtime cost)
//! unless `QUADRAUI_GTK_SMOKE_MS` is set, so ordinary interactive
//! launches are unaffected.
//!
//! The size/text assertion *logic* is unit-tested below with no display
//! required.
//!
//! ## Clipboard paste vs. IME/dead-key composition
//!
//! Two distinct input paths. Only the clipboard-paste half — Ctrl-V,
//! Ctrl-Shift-V, and middle-click PRIMARY, all handled below in
//! [`dispatch_event`] — is implemented by this module. IME/dead-key
//! composed input (e.g. a dead-key `´` followed by `e` composing to `é`,
//! or any real IME committing multi-keystroke text) is **not** wired up:
//! `EventControllerKey` here only ever sees raw, already-resolved keysyms
//! via `gdk_key_to_uievent`, with no `gtk4::IMMulticontext` attached to
//! intercept `key-press-event` first and expose its `commit` /
//! `preedit-changed` signals. That's a deliberate scope split, not an
//! oversight: no quadraui backend runs an IME composition pipeline yet
//! (see [`crate::UiEvent::CharTyped`]'s doc comment), and IME composition
//! owns adding one across every backend, not just GTK's terminal example.
//! `examples/common/terminal_app.rs`'s module doc carries the
//! consumer-facing version of this note.
//!
//! ## Re-entrancy: two independent sources, one invariant
//!
//! Every signal closure below takes `backend.borrow_mut()` and
//! `app.borrow_mut()` together across a `dispatch_event` call. Two
//! different things can make that double-borrow — panicking inside a
//! non-unwindable `extern "C"` GLib trampoline frame, which aborts the
//! whole process rather than unwinding:
//!
//! 1. **A nested modal pump** — GTK's async `FileDialog`'s
//!    `MainContext::iteration(true)` wait services every other pending
//!    source, including this runner's own closures, while the code that
//!    started the pump still holds `backend`/`app` borrowed. Guarded by
//!    `pump_depth.is_pumping()`, checked first in every closure below.
//! 2. **A synchronous same-signal re-entry from app code** — app code,
//!    called from inside `dispatch_event`, invokes a GTK method that
//!    synchronously re-emits the very signal a handler below listens for.
//!    The reported case: `AppLogic::handle` calls `window.close()` from
//!    inside its own `WindowClose` handling; `gtk_window_close` emits
//!    `close-request` synchronously, re-entering `connect_close_request`
//!    below while the outer call still holds `backend`/`app`.
//!    `pump_depth` is `0` the entire time this happens — it only ever
//!    models source 1 — so it waves this straight through into a second
//!    `borrow_mut()` and the abort.
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
    gdk_scroll_to_uievent_with_direction, gtk_drop_to_uievent, gtk_open_to_uievent,
};
use crate::backend::Backend;
use crate::desktop::{smoke_clipboard_round_trip_ok, smoke_size_ok, SmokeConfig};
use crate::dispatch::route_pointer;
use crate::runner::{AppLogic, Reaction};
use crate::runtime::{self, ReactionSink, RESIZE_SETTLE};
// Re-exported (not just imported) so `gtk::testing` — and any other
// in-crate caller that historically reached `EventOutcome` through this
// module — keeps working unchanged after the type moved to
// `crate::runtime`.
pub(crate) use crate::runtime::EventOutcome;
use crate::{ButtonMask, Key, Modifiers, MouseButton, Point, UiEvent};

/// Default window size, in DIPs. Matches the `ApplicationWindow`
/// builder's `default_width`/`default_height` below. Also used to seed
/// `GtkBackend`'s viewport *before* `app.setup()` runs, so
/// `Backend::viewport()` returns a sane, non-zero size to apps that read
/// it during setup (e.g. to size an embedded PTY) instead of
/// `GtkBackend::new()`'s zeroed default. `DrawingArea::connect_resize`
/// (wired below) corrects this to the widget's *actual* allocated size
/// as soon as it's realized, so this seed only matters for the brief
/// window between `setup()` and the first resize signal.
const DEFAULT_WINDOW_WIDTH: i32 = 800;
const DEFAULT_WINDOW_HEIGHT: i32 = 600;

/// Minimum sane `DrawingArea` size for headless smoke mode.
/// Comfortably below [`DEFAULT_WINDOW_WIDTH`]/[`DEFAULT_WINDOW_HEIGHT`] so
/// ordinary window-manager chrome/decoration insets don't false-positive.
const SMOKE_MIN_WIDTH: i32 = 200;
const SMOKE_MIN_HEIGHT: i32 = 150;

/// Env var enabling headless smoke mode — see the module doc's
/// "Headless smoke mode" section. Fed to
/// [`crate::desktop::SmokeConfig::from_env`] below.
const SMOKE_MS_VAR: &str = "QUADRAUI_GTK_SMOKE_MS";
/// Env var carrying the optional smoke-mode paste round-trip text — see
/// [`SMOKE_MS_VAR`].
const SMOKE_PASTE_VAR: &str = "QUADRAUI_GTK_SMOKE_PASTE";

// `SmokeConfig` + the size/clipboard pass-fail predicates are defined
// in the backend-neutral `desktop` module.

/// Configuration for [`run_with`] — the GTK application id and window
/// title that [`run`] hardcodes to generic defaults (`"org.quadraui.app"`
/// / `"quadraui app"`).
///
/// Apps that need a stable app id (Wayland/GNOME dock pinning, D-Bus
/// single-instance activation, Flatpak) or a custom window title build
/// this and call [`run_with`] instead of [`run`]. `run` itself is just
/// `run_with(app, RunConfig::default())`.
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
///
/// ## Downstream impact (CLAUDE.md rule 8): `single_instance` field
///
/// `RunConfig` is a plain, all-`pub`-field struct with no
/// `#[non_exhaustive]`, so adding [`Self::single_instance`] is only safe
/// if no consumer builds one with an exhaustive struct literal. Blast
/// radius, per CLAUDE.md's mandatory rule 1:
///
/// ```text
/// $ grep -rn 'RunConfig' ~/src/coord-tui/src ~/src/vimcode/src
/// (no output — zero hits in both)
/// ```
///
/// Both consumers reach this backend exclusively through
/// `run_with_shell(app, ShellConfig)` (`shell_runner`), which builds its
/// own `RunConfig` internally — neither consumer constructs or names
/// `gtk::RunConfig` at all. Adding this field is a no-op for both.
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
    pub icon_name: Option<String>,
    /// Whether a second launch of this app (same `app_id`) is forwarded
    /// to the already-running instance instead of starting a new process.
    /// `true` (the default — see [`Self::default`]) is GTK/GLib's own
    /// out-of-the-box behaviour for any `Application` with a stable `app_id`
    /// and no `ApplicationFlags::NON_UNIQUE`: dedup and D-Bus forwarding come
    /// free, no extra wiring needed beyond `app_id` itself. `run_with` also
    /// always sets `ApplicationFlags::HANDLES_OPEN`, so `argv` file/URL
    /// arguments (at launch, or forwarded from a second launch when this is
    /// `true`) reach the app as [`crate::UiEvent::OpenRequested`] via
    /// `Application::connect_open` instead of being rejected. Set this
    /// `false` (via [`Self::with_single_instance`]) to opt back into
    /// GTK's `ApplicationFlags::NON_UNIQUE` — every launch gets its own
    /// window, useful for running several debug instances side by side.
    pub single_instance: bool,
}

impl RunConfig {
    /// Build a config with the given app id and window title. No icon
    /// override — see [`Self::with_icon_name`]. Single-instance dedup is
    /// on — see [`Self::with_single_instance`] to opt out.
    pub fn new(app_id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            app_id: app_id.into(),
            title: title.into(),
            icon_name: None,
            single_instance: true,
        }
    }

    /// Opt in/out of single-instance dedup + `argv`/OS-open forwarding
    /// — see [`Self::single_instance`]'s doc for the full
    /// contract. Defaults to `true`.
    pub fn with_single_instance(mut self, single_instance: bool) -> Self {
        self.single_instance = single_instance;
        self
    }

    /// Set a themed icon name for the window. Feeds
    /// `gtk4::Window::set_icon_name` in [`run_with`] so themed-icon lookup
    /// works even on window managers that don't resolve `app_id` to an
    /// installed `.desktop` file.
    pub fn with_icon_name(mut self, icon_name: impl Into<String>) -> Self {
        self.icon_name = Some(icon_name.into());
        self
    }
}

impl Default for RunConfig {
    /// Default configuration: matches [`run`]'s previously-hardcoded defaults,
    /// so `run_with(app, RunConfig::default())` and `run(app)` behave identically.
    fn default() -> Self {
        Self {
            app_id: "org.quadraui.app".to_string(),
            title: "quadraui app".to_string(),
            icon_name: None,
            single_instance: true,
        }
    }
}

/// Drive `app` to completion in a basic single-`DrawingArea` GTK
/// environment, using the default [`RunConfig`] (generic app id and
/// window title). See [`run_with`] to set a custom app id / title
/// — needed by any app that isn't quadraui itself, e.g.
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
/// window title) instead of the generic quadraui defaults.
pub fn run_with<A: AppLogic + 'static>(app: A, config: RunConfig) -> std::process::ExitCode {
    let app = Rc::new(RefCell::new(app));
    let backend = Rc::new(RefCell::new(GtkBackend::new()));
    // `None` unless `QUADRAUI_GTK_SMOKE_MS` is set —
    // see the module doc's "Headless smoke mode" section.
    let smoke = SmokeConfig::from_env(SMOKE_MS_VAR, SMOKE_PASTE_VAR);
    let smoke_failed = Rc::new(Cell::new(false));
    let title = config.title;
    let icon_name = config.icon_name;
    // Fetched here (before `gapp` even exists) rather than inside
    // `activate` alone, so `connect_open` below can push
    // `UiEvent::OpenRequested` onto the same queue regardless of whether
    // `activate` has run yet — `GtkBackend::new()` above already set up
    // the underlying `VecDeque`, and `activate`'s idle-drain timer (once
    // it starts) drains whatever is already sitting in it.
    let events_handle = backend.borrow().events_handle();

    // `HANDLES_OPEN` is required for `argv` file/URL positional
    // arguments to route to `connect_open` below instead of GLib
    // rejecting them outright ("This application can not open files.").
    // `NON_UNIQUE` opts *out* of GTK/GLib's default single-instance
    // dedup — see `RunConfig::single_instance`'s doc for why the default
    // (`true`, i.e. this flag absent) reproduces GTK's existing
    // no-flags-set behaviour exactly.
    let mut flags = gtk4::gio::ApplicationFlags::HANDLES_OPEN;
    if !config.single_instance {
        flags |= gtk4::gio::ApplicationFlags::NON_UNIQUE;
    }
    let gapp = Application::builder()
        .application_id(config.app_id)
        .flags(flags)
        .build();

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

    // GLib emits `open` *instead of* `activate` whenever the
    // launch carries file/URL arguments — at first launch (a registered
    // `.myext` file double-clicked in a file manager, a `myapp://` URL
    // activation) or, with `single_instance` on, forwarded here from a
    // second launch via GLib's own D-Bus dedup (no extra wiring needed
    // for that half — see `RunConfig::single_instance`'s doc). Either
    // way `activate` never separately fires for this same launch, so
    // `gapp.windows()` being empty is exactly the "no window built yet"
    // signal: build one via the same `activate` this closure otherwise
    // shares nothing with, so a files-at-launch invocation still gets a
    // window instead of silently pushing an event nothing will ever
    // read.
    {
        let app = app.clone();
        let backend = backend.clone();
        let smoke = smoke.clone();
        let smoke_failed = smoke_failed.clone();
        let title = title.clone();
        let icon_name = icon_name.clone();
        let events_handle = events_handle.clone();
        gapp.connect_open(move |gapp, files, _hint| {
            if gapp.windows().is_empty() {
                activate(
                    gapp,
                    app.clone(),
                    backend.clone(),
                    smoke.clone(),
                    smoke_failed.clone(),
                    title.clone(),
                    icon_name.clone(),
                );
            }
            events_handle
                .borrow_mut()
                .push_back(gtk_open_to_uievent(files));
            // Bring the (new-or-existing) window forward — the point of
            // single-instance forwarding is that the user *sees* their
            // request land, not just that the event reaches `AppLogic`.
            // On macOS this needs the same frontmost hand-off `activate`
            // does on first launch: a second launch can arrive while the
            // process has since lost focus, so re-activate every time,
            // not just when a window had to be built.
            if let Some(window) = gapp.windows().first() {
                window.present();
                activate_macos_process();
            }
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
    std::process::ExitCode::from(glib_code)
}

pub use crate::runner::StepOutcome;

/// Non-blocking / bounded-wait entry points for a host that owns its own
/// event loop — the GTK counterpart of [`crate::tui::run::TuiRunner`].
///
/// [`run`]/[`run_with`] each own the whole process: `Application::run()`
/// blocks the calling thread inside GLib's main loop until the app quits.
/// That's the right shape for a process whose only job is running one
/// quadraui window, but it can't be embedded inside a host that already
/// owns its own loop and needs to interleave quadraui's work with its own.
///
/// [`GtkRunner::new`]/[`GtkRunner::new_with`] build the exact same
/// `Application` + window + `DrawingArea` + event-controller wiring
/// [`run_with`] does — both call the same [`activate`] function directly
/// (rather than `run_with`'s `Application::connect_activate` +
/// `Application::run()`), so nothing about the live window, its signal
/// handlers, or its render path differs between the two entry points.
/// What differs is only how the GLib main loop gets serviced afterwards:
///
/// - [`GtkRunner::step`] — one non-blocking pass:
///   `glib::MainContext::iteration(false)`, GLib's own primitive for
///   "process whatever's immediately ready and return, blocking on nothing".
/// - [`GtkRunner::pump`] — one pass bounded to at most `timeout`: arms a
///   one-shot `glib::timeout_add_local_once` for `timeout`, then calls
///   `glib::MainContext::iteration(true)` — GLib's own blocking-until-
///   something's-ready primitive — so the call returns either once a real
///   GTK/GLib source fires (a key press, a redraw, a background
///   `Backend::waker` wake) or once the bound timer does, whichever is first.
///
/// Both return a [`StepOutcome`], mirroring `TuiRunner`: `Continue` while
/// the window is still open, `Exited` once it's gone. Checked via
/// `Application::windows().is_empty()` rather than a dedicated flag —
/// every exit path in this module (`Reaction::Exit`/`EventOutcome::Exit`
/// via [`GtkSink::request_exit`], the OS close button once the app's
/// `WindowClose` handling allows it through, headless smoke mode's forced
/// timeout) already tears the window down via `window.destroy()` or lets
/// `window.close()` proceed, both of which remove it from that list — see
/// [`GtkSink::request_exit`]'s doc for why `destroy()`, not `close()`, is
/// the one every *programmatic* exit uses.
///
/// A background thread's [`Backend::waker`] call needs no extra wiring to
/// reach a `GtkRunner`-driven loop: `GtkBackend::waker()` posts through
/// `glib::MainContext::invoke`, which schedules its wake callback onto the
/// exact same default `MainContext` `step`/`pump` iterate — unlike TUI,
/// GTK *can* interrupt a blocking `pump(timeout)` wait early this way,
/// since `MainContext::invoke` wakes the context's underlying poll
/// immediately rather than TUI's crossterm read having to wait out a
/// fixed timeout regardless.
///
/// Scope note: unlike `run`/`run_with`, `GtkRunner` doesn't wire
/// `Application::connect_open` (`argv` file/URL open-with activation) —
/// an embedding host already owns its own process's argv handling, so
/// that signal has no obvious owner here. Everything else `activate`
/// wires (keyboard, mouse, scroll, resize, HiDPI, file drop,
/// window-close veto, headless smoke mode) is unchanged.
///
/// `GtkRunner` has no `Drop`-based teardown the way `TuiRunner` does:
/// there's no global terminal mode to restore, and the window/`Application`
/// object graph is kept alive by GTK's own `Rc`-like GObject refcounting
/// (the event-controller closures each hold their own clones), independent
/// of whether anything is still calling `step`/`pump`. A host that stops
/// pumping without driving the app to [`StepOutcome::Exited`] leaves the
/// window exactly as open as it was — nothing leaks beyond what any
/// abandoned GTK `Application` would.
pub struct GtkRunner {
    gapp: Application,
    context: glib::MainContext,
    smoke_failed: Rc<Cell<bool>>,
}

impl GtkRunner {
    /// Build a runner with the default [`RunConfig`] (generic app id and
    /// window title). Builds the `Application`, window, and `DrawingArea`,
    /// and calls `app.setup()` — exactly what [`run`] does before handing
    /// control to `Application::run()`.
    pub fn new<A: AppLogic + 'static>(app: A) -> Self {
        Self::new_with(app, RunConfig::default())
    }

    /// Like [`Self::new`], but with an explicit [`RunConfig`].
    pub fn new_with<A: AppLogic + 'static>(app: A, config: RunConfig) -> Self {
        let app = Rc::new(RefCell::new(app));
        let backend = Rc::new(RefCell::new(GtkBackend::new()));
        // `None` unless `QUADRAUI_GTK_SMOKE_MS` is set —
        // see the module doc's "Headless smoke mode" section. Honoured here
        // too, so `GtkRunner` gets the same xvfb-run-friendly headless
        // smoke behaviour `run_with` does, with no separate opt-in.
        let smoke = SmokeConfig::from_env(SMOKE_MS_VAR, SMOKE_PASTE_VAR);
        let smoke_failed = Rc::new(Cell::new(false));

        let mut flags = gtk4::gio::ApplicationFlags::HANDLES_OPEN;
        if !config.single_instance {
            flags |= gtk4::gio::ApplicationFlags::NON_UNIQUE;
        }
        let gapp = Application::builder()
            .application_id(config.app_id)
            .flags(flags)
            .build();
        // `Application::run()` registers before ever emitting `activate` —
        // mirrored here since this path calls `activate` directly instead
        // of going through `connect_activate`/`run()`. Best-effort: a
        // registration failure (e.g. no D-Bus session) shouldn't prevent
        // an embedded window from working at all — `run_with` doesn't
        // surface this failure mode differently either, since `run()`
        // itself has no separate registration-error return path.
        let _ = gapp.register(gtk4::gio::Cancellable::NONE);

        // Retain a clone here (mirroring `run_with`'s own `smoke_failed`
        // clone) so callers can read whether headless smoke mode failed
        // after driving this runner to `StepOutcome::Exited` — see
        // [`Self::smoke_failed`].
        let smoke_failed_handle = smoke_failed.clone();

        activate(
            &gapp,
            app,
            backend,
            smoke,
            smoke_failed,
            config.title,
            config.icon_name,
        );

        Self {
            gapp,
            context: glib::MainContext::default(),
            smoke_failed: smoke_failed_handle,
        }
    }

    /// Whether the app's window is still open — see the type-level doc's
    /// "Both return a `StepOutcome`" paragraph for why this list, not a
    /// dedicated flag, is the source of truth.
    fn window_open(&self) -> bool {
        !self.gapp.windows().is_empty()
    }

    fn outcome(&self) -> StepOutcome {
        if self.window_open() {
            StepOutcome::Continue
        } else {
            StepOutcome::Exited
        }
    }

    /// Whether headless smoke mode (`QUADRAUI_GTK_SMOKE_MS`) detected a
    /// failure — a tiny-window regression or a clipboard round-trip
    /// mismatch, set by `schedule_smoke_check`. Mirrors `run_with`'s own
    /// post-`gapp.run()` `smoke_failed.get()` check. [`run`]/[`run_with`]
    /// fold this into their returned `std::process::ExitCode` automatically,
    /// but `step`/`pump` return a [`StepOutcome`] with no room for a failure
    /// signal, so an embedding host must poll this explicitly once the runner
    /// reaches [`StepOutcome::Exited`] (headless smoke mode always forces the
    /// window closed after `QUADRAUI_GTK_SMOKE_MS`, so `Exited` is guaranteed
    /// to arrive) and set its own process exit code from it — see
    /// `examples/gtk_embedded.rs` for the pairing with `scripts/gtk_smoke.sh`.
    pub fn smoke_failed(&self) -> bool {
        self.smoke_failed.get()
    }

    /// One non-blocking pass: `glib::MainContext::iteration(false)` —
    /// dispatches whatever's immediately ready (a queued redraw, an
    /// already-delivered input event, an expired timer), never blocks.
    pub fn step(&mut self) -> StepOutcome {
        if self.window_open() {
            self.context.iteration(false);
        }
        self.outcome()
    }

    /// One pass, blocking for up to `timeout` if nothing is ready yet.
    /// Arms a one-shot timer for `timeout` and calls
    /// `glib::MainContext::iteration(true)` exactly once — GLib's own
    /// "block until something's ready" primitive — so this returns as soon
    /// as *either* a real source fires *or* the bound timer does, never
    /// later than `timeout`.
    pub fn pump(&mut self, timeout: Duration) -> StepOutcome {
        if !self.window_open() {
            return StepOutcome::Exited;
        }

        let timed_out = Rc::new(Cell::new(false));
        let timed_out_cb = timed_out.clone();
        let timer_id = glib::source::timeout_add_local_once(timeout, move || {
            timed_out_cb.set(true);
        });

        self.context.iteration(true);

        // If a real source — not our own bound timer — is what woke this
        // call, cancel the still-pending timer rather than leaving it to
        // fire later and wake some future, unrelated `iteration()` call
        // for no reason.
        if !timed_out.get() {
            timer_id.remove();
        }

        self.outcome()
    }
}

/// Hands the process frontmost/active-application status on macOS, right
/// after the first window is presented.
///
/// GTK's quartz backend creates and shows a real `NSWindow`, but
/// `window.present()` is only a GDK-level "raise and request focus"
/// request to the window server — it does not make the owning
/// *process* the frontmost application, and macOS routes keyboard
/// events by frontmost application, not merely by which window is
/// raised. Normally a double-clicked (or `open`ed) `.app` bundle gets
/// handed that status by LaunchServices as part of launching it; a
/// bare, non-bundled binary run directly from a terminal never goes
/// through LaunchServices at all, so it's never activated and every
/// keystroke keeps landing in the launching terminal instead of the
/// GTK window that just appeared, painted, and took a mouse click.
///
/// Two calls, in this order, matching `macos::run`'s own AppKit-bootstrap
/// sequence (`setActivationPolicy` near its top, an activation call right
/// before `ns_app.run()`) — see that function for the native-AppKit-
/// backend equivalent of this GTK-quartz one:
///
/// - `setActivationPolicy(.regular)` first — the `TransformProcessType`-
///   to-foreground step a bare, unbundled binary normally needs, and the
///   same call `macos::run` makes for the identical reason. Measured on
///   real macOS hardware (macmini, direct `cargo run --example gtk_demo
///   --features gtk`, instrumented with an AppKit-state printout at
///   function entry): GDK's quartz backend has *already* set the policy
///   to `.regular` by the time this runs, before this call ever executes,
///   so on this GTK/macOS combination the call observably returns `NO`
///   (already-set, nothing left to do) rather than performing the
///   transform itself. It's kept anyway — cheap, correct, and this repo
///   has exactly one data point, not a guarantee GDK's default never
///   changes — rather than relying on an undocumented GDK behaviour.
/// - `activateIgnoringOtherApps(true)` second. Deprecated in objc2-app-kit
///   in favor of `NSApp.activate` (`NSApplication::activate`/
///   `NSRunningApplication::activateWithOptions`); kept here
///   unconditionally for its simplicity, unlike `macos::run`, which
///   prefers `activate()` and only falls back to this call on macOS
///   versions where `activate()` isn't available. Unlike a purely
///   cosmetic rename, macOS 14 changed activation to be cooperative
///   between apps, so the replacement is not guaranteed to behave
///   identically in every case — this call may need revisiting if that
///   distinction turns out to matter in practice.
///
/// **What hardware testing on macmini could and couldn't confirm:** the
/// two calls above execute for real against a live `NSApplication` and a
/// real, visible `NSWindow` (`lsappinfo list` shows the process as
/// `type="Foreground"`, `(in front)`), and a synthetic `osascript ...
/// System Events keystroke "q"` reached the running example's quit
/// handler and ended the process. What that same hardware session could
/// *not* produce is a trustworthy automated activation-state reading:
/// `NSApplication.isActive` and `NSWorkspace`'s notion of the frontmost
/// process disagreed with `lsappinfo`'s "(in front)" marker in ways that
/// didn't change between a build with this function's body intact and
/// one with the `activateIgnoringOtherApps` call commented out — i.e.
/// this automated session could not establish a clean fixed-vs-unfixed
/// signal either way for the keyboard-routing symptom itself. That is
/// consistent with the issue's own acceptance text: it asks for a person
/// at the physical keyboard of a Terminal.app-launched binary, which is
/// exactly the step an unattended agent session run through automation
/// (rather than a human pressing real keys) cannot substitute for.
///
/// A no-op on every other target: `objc2`/`objc2-app-kit` are declared
/// under `[target.'cfg(target_os = "macos")'.dependencies]` in
/// `Cargo.toml`, so on non-macOS targets those crates don't exist in the
/// dependency graph at all regardless of which feature's `dep:` list
/// names them — this function's `cfg`-gated body is what keeps the
/// `gtk` feature building everywhere else.
#[cfg(target_os = "macos")]
fn activate_macos_process() {
    let Some(mtm) = objc2::MainThreadMarker::new() else {
        // `activate` only ever runs on GTK's main thread in practice —
        // this is cheap insurance against ever being wrong about that,
        // rather than a reachable case.
        return;
    };
    let ns_app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    let _ = ns_app.setActivationPolicy(objc2_app_kit::NSApplicationActivationPolicy::Regular);
    #[allow(deprecated)]
    ns_app.activateIgnoringOtherApps(true);
}

/// Non-macOS targets: no AppKit, nothing to activate. See the `#[cfg(target_os
/// = "macos")]` sibling above for the real implementation and its rationale.
#[cfg(not(target_os = "macos"))]
fn activate_macos_process() {}

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
    // Themed icon lookup, independent of the app-id ↔
    // `.desktop` file match — see `RunConfig::icon_name`'s doc comment.
    if let Some(icon_name) = icon_name {
        window_builder = window_builder.icon_name(icon_name);
    }
    let window = window_builder.build();

    // Stash the window handle so `Backend::begin_window_drag` /
    // `Backend::toggle_window_maximize` have something to drive.
    // Harmless for apps that never call them (default no-op on every
    // other backend, and GTK apps that don't opt into a CSD titlebar).
    backend.borrow_mut().set_window(window.clone());

    // Re-entrancy guard shared with `GtkPlatformServices::pump_until_ready`:
    // `> 0` while a file dialog's nested-mainloop wait is in
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

    // Shared handle to the backend's event queue, fetched once here
    // (same reasoning as `pump_depth` above) and cloned into any closure
    // below that needs to *defer* an event rather than drop it outright when
    // `try_dispatch`/`try_borrow_both` (below) report the backend/app are
    // already borrowed. Pushing here never needs `backend`'s own `RefCell` —
    // `events_handle()` hands back a plain `Rc<RefCell<VecDeque<UiEvent>>>`
    // with independent borrow state — so it works precisely in the situation
    // that motivates it: `backend` mutably borrowed by an outer
    // `dispatch_event` call, still on the stack. See `try_dispatch`'s doc
    // and the `close-request` handler below for the concrete re-entrancy
    // this exists to survive.
    let events_handle = backend.borrow().events_handle();

    let da = DrawingArea::new();
    da.set_hexpand(true);
    da.set_vexpand(true);
    // The DA is the widget every keyboard-driven primitive expects
    // to hold GTK's internal focus (even though `setup_key_controller`
    // below attaches `EventControllerKey` to the *window*, not `da` —
    // bubble-phase propagation still starts at whatever widget holds
    // focus and walks up to the window, so an unfocusable/unfocused `da`
    // leaves nothing in that chain until the user clicks it). Without
    // `focusable(true)` here, `grab_focus()` below (after `present()`)
    // would be a silent no-op — GTK widgets default to
    // `focusable: false`. kubeui-gtk worked around the missing piece by
    // calling `grab_focus()` itself post-construction; this folds that
    // into the runner so every app built on `gtk::run`/`GtkRunner` gets
    // working keyboard input on first launch with no click required.
    da.set_focusable(true);
    da.set_can_focus(true);
    window.set_child(Some(&da));

    // Seed the backend's persistent pango context from the widget so
    // `form_layout()` and other `_layout()` methods can use exact Pango
    // measurement outside the draw callback.
    {
        let pctx = da.pango_context();
        let font_desc = pg::FontDescription::from_string("Sans 11");
        // pango tightened `Context::set_font_description` to no longer accept
        // `Option`, so the wrapper is just dropped.
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
    // size it to ~zero.
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

    // Per-signal controller wiring: each `setup_*` below owns one GTK
    // signal's full closure — cloning whatever shared `Rc<RefCell<_>>`
    // handles it needs and attaching itself to `da`/`window` — so the
    // re-entrancy guards (`pump_depth.is_pumping()`, the `try_dispatch*`
    // backstops) and the shared-state threading (`cursor_pos` between
    // motion/scroll, `events_handle` for the close-request deferral) are
    // unchanged — see the module doc's "Re-entrancy" section, which every
    // `setup_*` function's doc comment below points back to.
    setup_draw_func(&da, &app, &backend, &pump_depth);
    setup_key_controller(&window, &da, &app, &backend, &pump_depth);

    // Shared with `setup_scroll_controller` below so scroll events carry
    // the actual pointer position — GTK's `EventControllerScroll` only
    // delivers `(dx, dy)` in its own callback.
    let cursor_pos = Rc::new(Cell::new((0.0_f64, 0.0_f64)));

    setup_click_controller(&da, &window, &app, &backend, &pump_depth);
    setup_motion_controller(&da, &window, &app, &backend, &pump_depth, &cursor_pos);
    setup_scroll_controller(&da, &window, &app, &backend, &pump_depth, &cursor_pos);
    setup_resize_controller(&da, &window, &app, &backend, &pump_depth);
    setup_hidpi_controller(&da, &window, &app, &backend, &pump_depth);
    setup_file_drop_controller(&da, &window, &app, &backend, &pump_depth);
    setup_close_request(&window, &da, &app, &backend, &pump_depth, &events_handle);
    setup_event_drain(&da, &window, &app, &backend, &pump_depth, &events_handle);

    window.present();
    // On macOS, hand the process frontmost status too — see
    // `activate_macos_process`'s doc for why `present()` alone isn't
    // enough and what the two AppKit calls do.
    activate_macos_process();
    // Grab focus only after `present()` realizes/maps the window —
    // calling this any earlier is a well-documented GTK4 foot-gun (the
    // widget isn't part of a mapped surface yet, so the focus request can
    // be silently dropped on some backends instead of just being queued).
    // See the `set_focusable`/`set_can_focus` call above `window.set_child`
    // for why this is a no-op without that pairing.
    da.grab_focus();

    // Opt-in, zero-cost unless `QUADRAUI_GTK_SMOKE_MS`
    // is set — see the module doc's "Headless smoke mode" section.
    if let Some(cfg) = smoke {
        schedule_smoke_check(cfg, da, backend, app, window, smoke_failed);
    }
}

/// Wires `da`'s `set_draw_func` — enters `render_frame` every repaint,
/// skipping the frame entirely under the re-entrancy guard (a file dialog's
/// nested pump further up the call stack already holds `backend`). See
/// `activate`'s "Re-entrancy" module-doc section.
fn setup_draw_func<A: AppLogic + 'static>(
    da: &DrawingArea,
    app: &Rc<RefCell<A>>,
    backend: &Rc<RefCell<GtkBackend>>,
    pump_depth: &crate::desktop::ModalPumpDepth,
) {
    let app = app.clone();
    let backend = backend.clone();
    let pump_depth = pump_depth.clone();
    da.set_draw_func(move |_da, cr, w, h| {
        // Re-entrancy guard: skip this repaint entirely rather than
        // double-borrow `backend` while a file dialog's nested pump
        // (further up the call stack) already holds it. Worst case this
        // frame stays stale until the dialog closes and a normal redraw fires;
        if pump_depth.is_pumping() {
            return;
        }
        let mut backend_mut = backend.borrow_mut();
        let app_ref = app.borrow();
        render_frame(&mut backend_mut, &*app_ref, cr, w, h);
    });
}

/// Wires the window's keyboard controller. All key-press pre-processing
/// (ActivityBar focus intercept, Tab/Shift+Tab cycling, `Global`
/// accelerators, Ctrl-C/V/A interception) lives in the shared
/// `dispatch_event` (see the module doc's "Shared with the headless
/// test driver" section) so the live GTK path and `GtkDriver` can't
/// drift apart.
fn setup_key_controller<A: AppLogic + 'static>(
    window: &ApplicationWindow,
    da: &DrawingArea,
    app: &Rc<RefCell<A>>,
    backend: &Rc<RefCell<GtkBackend>>,
    pump_depth: &crate::desktop::ModalPumpDepth,
) {
    let key_ctrl = EventControllerKey::new();
    {
        let backend = backend.clone();
        let app = app.clone();
        let da_for_redraw = da.clone();
        let window_for_close = window.clone();
        let pump_depth = pump_depth.clone();
        key_ctrl.connect_key_pressed(move |_ctrl, key, _code, modifier| {
            // Re-entrancy guard: a dialog pump further up the call stack
            // already holds `backend` mutably borrowed — don't re-enter it
            if pump_depth.is_pumping() {
                return glib::Propagation::Proceed;
            }
            let Some(ev) = gdk_key_to_uievent(key, modifier, false) else {
                return glib::Propagation::Proceed;
            };

            // If `backend`/`app` already borrowed by an outer `dispatch_event`
            // further up this stack (not a nested pump, which the guard above
            // already ruled out) — degrade rather than panicking. See the
            // module doc's "Re-entrancy" section.
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
}

/// Wires `da`'s left-click gesture. Routes mouse-down through
/// `route_pointer` so registered text regions receive selection
/// drags and scrollbar clicks begin scrollbar drags, then forwards the
/// resulting events through the shared `dispatch_event` funnel.
/// Mouse-up discards any armed window-drag request that never
/// crossed the move threshold, then forwards through the same funnel.
fn setup_click_controller<A: AppLogic + 'static>(
    da: &DrawingArea,
    window: &ApplicationWindow,
    app: &Rc<RefCell<A>>,
    backend: &Rc<RefCell<GtkBackend>>,
    pump_depth: &crate::desktop::ModalPumpDepth,
) {
    let click = GestureClick::builder().button(0).build();
    setup_click_pressed(&click, da, window, app, backend, pump_depth);
    setup_click_released(&click, da, window, app, backend, pump_depth);
    da.add_controller(click);
}

/// `connect_pressed` half of [`setup_click_controller`]. Routes through
/// `route_pointer` so text-region clicks begin a TextSelection drag and
/// scrollbar clicks begin scrollbar drags. Double-click folding is decided
/// in `dispatch_event`'s shared `fold_double_click` step, applied uniformly
/// to whatever `route_pointer` returns, the same as macOS/Windows.
fn setup_click_pressed<A: AppLogic + 'static>(
    click: &GestureClick,
    da: &DrawingArea,
    window: &ApplicationWindow,
    app: &Rc<RefCell<A>>,
    backend: &Rc<RefCell<GtkBackend>>,
    pump_depth: &crate::desktop::ModalPumpDepth,
) {
    let backend = backend.clone();
    let app = app.clone();
    let da_for_redraw = da.clone();
    let window_for_close = window.clone();
    let pump_depth = pump_depth.clone();
    click.connect_pressed(move |gesture, _n_press, x, y| {
        // Re-entrancy guard — see `setup_key_controller`.
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
        // `Backend::begin_window_drag` can later arm a deferred window-drag
        // request with it. Runs for both single- and double-press events
        // (both fire `connect_pressed`); harmless if the app never calls
        // `begin_window_drag`.
        if let Some(event) = gesture.current_event() {
            if let Some(device) = event.device() {
                // If `backend` is already borrowed, skip the stash. Worst case
                // a subsequent `begin_window_drag` has nothing armed to commit;
                if let Ok(mut backend_mut) = backend.try_borrow_mut() {
                    backend_mut.stash_window_press(device, gdk_button as i32, x, y, event.time());
                }
            }
        }

        // If `backend`/`app` already borrowed by an outer `dispatch_event` —
        let Ok(mut backend_mut) = backend.try_borrow_mut() else {
            return;
        };

        let events = {
            let stack_rc = backend_mut.modal_stack_handle();
            let drag_rc = backend_mut.drag_state_handle();
            let stack = stack_rc.borrow();
            let mut drag = drag_rc.borrow_mut();
            route_pointer(
                &stack,
                &mut drag,
                backend_mut.text_selection_state_mut(),
                UiEvent::MouseDown {
                    widget: None,
                    button,
                    position,
                    modifiers,
                },
            )
        };

        let mut needs_redraw = false;
        for ev in events {
            // `dispatch_event` folds the double-click itself as its first
            // pre-processing step — pass the raw `MouseDown`
            // `dispatch_click` returned straight through.
            //
            // If `app` already borrowed by an outer `dispatch_event` further up
            // this stack — stop draining this batch rather than panicking.
            let Some(outcome) = try_dispatch_borrowed(&mut backend_mut, &app, ev) else {
                break;
            };
            match outcome {
                EventOutcome::Continue => {}
                EventOutcome::Redraw => needs_redraw = true,
                EventOutcome::RedrawAfter(delay) => backend_mut.request_frame_in(delay),
                EventOutcome::Exit => {
                    // Use `destroy()`, not `close()`: this `Exit` came from
                    // the app's own handling of a click/drag event, not from the
                    // OS close-request signal, so re-routing it through `close()`
                    // would emit `close-request` and re-dispatch a synthetic
                    // `WindowClose` the app never asked about — vetoing its own
                    // exit for any app with no `WindowClose` opinion.
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

/// `connect_released` half of [`setup_click_controller`]. Discards any
/// armed window-drag request that never crossed the move
/// threshold — see `GtkBackend::armed_window_drag` — then forwards
/// through the shared `dispatch_event` funnel.
fn setup_click_released<A: AppLogic + 'static>(
    click: &GestureClick,
    da: &DrawingArea,
    window: &ApplicationWindow,
    app: &Rc<RefCell<A>>,
    backend: &Rc<RefCell<GtkBackend>>,
    pump_depth: &crate::desktop::ModalPumpDepth,
) {
    let backend = backend.clone();
    let app = app.clone();
    let da_for_redraw = da.clone();
    let window_for_close = window.clone();
    let pump_depth = pump_depth.clone();
    click.connect_released(move |gesture, _n_press, x, y| {
        // Re-entrancy guard — see `setup_key_controller`.
        if pump_depth.is_pumping() {
            return;
        }
        // If `backend`/`app` already borrowed by an outer `dispatch_event` —
        let Ok(mut backend_mut) = backend.try_borrow_mut() else {
            return;
        };
        // If the button goes up before the pointer ever moved past
        // the drag threshold, this was a plain click (or the first half
        // of a double-click), not a drag. Discard the armed window-drag
        // request rather than leaving it to be accidentally committed by
        // a later, unrelated hover-motion event.
        backend_mut.discard_armed_window_drag();
        let position = Point::new(x as f32, y as f32);
        let button: MouseButton = gdk_button_to_quadraui(gesture.current_button());
        let events = {
            let stack_rc = backend_mut.modal_stack_handle();
            let drag_rc = backend_mut.drag_state_handle();
            let stack = stack_rc.borrow();
            let mut drag = drag_rc.borrow_mut();
            route_pointer(
                &stack,
                &mut drag,
                backend_mut.text_selection_state_mut(),
                UiEvent::MouseUp {
                    widget: None,
                    button,
                    position,
                },
            )
        };
        for ev in events {
            // If `app` already borrowed by an outer `dispatch_event` further up
            // this stack — stop draining this batch rather than panicking.
            let Some(outcome) = try_dispatch_borrowed(&mut backend_mut, &app, ev) else {
                break;
            };
            apply_event_outcome(outcome, &backend_mut, &da_for_redraw, &window_for_close);
        }
    });
}

/// Wires `da`'s motion controller. Routes mouse-move through
/// `route_pointer` so text-selection drags emit `TextSelectionChanged`
/// events, and commits any armed window-drag once the pointer
/// crosses the move threshold. `cursor_pos` is shared with
/// `setup_scroll_controller` so scroll events carry the actual pointer
/// position.
fn setup_motion_controller<A: AppLogic + 'static>(
    da: &DrawingArea,
    window: &ApplicationWindow,
    app: &Rc<RefCell<A>>,
    backend: &Rc<RefCell<GtkBackend>>,
    pump_depth: &crate::desktop::ModalPumpDepth,
    cursor_pos: &Rc<Cell<(f64, f64)>>,
) {
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

            // Re-entrancy guard — see `setup_key_controller`.
            if pump_depth.is_pumping() {
                return;
            }

            // Commit a deferred window-drag once the pointer has moved past
            // the drag threshold since the press that armed it. Mirrors native
            // `gtk4::WindowHandle`, which defers its own move-start to
            // `GestureDrag`'s threshold-gated `drag-begin` signal rather than
            // the raw button press — this keeps a press that turns into a
            // double-click from starting an interactive move grab that would
            // swallow the second press. The threshold check itself lives in the
            // backend-neutral `desktop::WindowDragArm` —
            // `GtkBackend::try_commit_window_drag` is a no-op on every call
            // until the pointer actually crosses it, so this can call it
            // unconditionally on every move. Only returns early when a drag is
            // actually committed (control passes to the compositor's native move
            // at that point); otherwise falls through to the normal motion
            // handling below unaffected. If `backend` is already borrowed,
            // skip the commit check this move event; the next motion event
            // retries it.
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

            // If `backend`/`app` already borrowed by an outer `dispatch_event` —
            let Ok(mut backend_mut) = backend.try_borrow_mut() else {
                return;
            };
            let events = {
                let stack_rc = backend_mut.modal_stack_handle();
                let drag_rc = backend_mut.drag_state_handle();
                let stack = stack_rc.borrow();
                let mut drag = drag_rc.borrow_mut();
                route_pointer(
                    &stack,
                    &mut drag,
                    backend_mut.text_selection_state_mut(),
                    UiEvent::MouseMoved { position, buttons },
                )
            };

            // `TextSelectionChanged` (active-selection state update) and the
            // fallback `app.handle` both live in the shared `dispatch_event`.
            let mut needs_redraw = false;
            for ev in events {
                // If `app` already borrowed by an outer `dispatch_event`
                // further up this stack — stop draining this batch rather than
                let Some(outcome) = try_dispatch_borrowed(&mut backend_mut, &app, ev) else {
                    break;
                };
                match outcome {
                    EventOutcome::Continue => {}
                    EventOutcome::Redraw => needs_redraw = true,
                    EventOutcome::RedrawAfter(delay) => backend_mut.request_frame_in(delay),
                    EventOutcome::Exit => {
                        // Use `destroy()`, not `close()`: this `Exit` came from
                        // the app's own handling of a click/drag event, not from the
                        // OS close-request signal, so re-routing it through `close()`
                        // would emit `close-request` and re-dispatch a synthetic
                        // `WindowClose` the app never asked about — vetoing its own
                        // exit for any app with no `WindowClose` opinion.
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
}

/// Wires `da`'s scroll controller, reading the shared `cursor_pos` so
/// wheel events carry the actual pointer position (GTK's
/// `EventControllerScroll` only ever delivers `(dx, dy)` itself).
fn setup_scroll_controller<A: AppLogic + 'static>(
    da: &DrawingArea,
    window: &ApplicationWindow,
    app: &Rc<RefCell<A>>,
    backend: &Rc<RefCell<GtkBackend>>,
    pump_depth: &crate::desktop::ModalPumpDepth,
    cursor_pos: &Rc<Cell<(f64, f64)>>,
) {
    let scroll = EventControllerScroll::new(EventControllerScrollFlags::BOTH_AXES);
    {
        let backend = backend.clone();
        let app = app.clone();
        let da_for_redraw = da.clone();
        let window_for_close = window.clone();
        let pump_depth = pump_depth.clone();
        let cursor_pos = cursor_pos.clone();
        scroll.connect_scroll(move |_ctrl, dx, dy| {
            // Re-entrancy guard — see `setup_key_controller`.
            if pump_depth.is_pumping() {
                return glib::Propagation::Proceed;
            }
            let (x, y) = cursor_pos.get();
            // If `app` already mutably borrowed further up this stack,
            // fall back to the trait's own default (`false`) rather than risk it;
            // `try_dispatch` below degrades the same way for the dispatch itself.
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
}

/// Wires `da`'s resize signal. `Backend::begin_frame(viewport)` (in
/// `setup_draw_func`) keeps `backend.viewport()` in sync with the DA's
/// allocated size on every *render*, but apps with side effects on
/// resize — not just re-painting — need `UiEvent::WindowResized`
/// dispatched too. The dispatch itself is **debounced**
/// via a cancel-and-reschedule `glib::SourceId` (mirroring the TUI
/// runner's `RESIZE_SETTLE` window) so a live resize drag doesn't spam
/// PTY/SIGWINCH on every intermediate frame; painting stays perfectly
/// live regardless, since `set_draw_func` re-reads the DA's actual
/// allocated size every frame.
fn setup_resize_controller<A: AppLogic + 'static>(
    da: &DrawingArea,
    window: &ApplicationWindow,
    app: &Rc<RefCell<A>>,
    backend: &Rc<RefCell<GtkBackend>>,
    pump_depth: &crate::desktop::ModalPumpDepth,
) {
    let resize_timer: Rc<Cell<Option<glib::SourceId>>> = Rc::new(Cell::new(None));
    let backend = backend.clone();
    let app = app.clone();
    let da_for_redraw = da.clone();
    let window_for_close = window.clone();
    let pump_depth = pump_depth.clone();
    da.connect_resize(move |da, _width, _height| {
        // Re-entrancy guard — see `setup_key_controller`.
        if pump_depth.is_pumping() {
            return;
        }
        // Force a FULL-widget invalidation on every resize edge, not
        // just at the debounced settle below. This is the
        // render-path half of the "stale ~ / > prompt fragments stuck on
        // rows that should be blank after a shrink→expand" ghosting.
        //
        // GTK repaints a growing window with *partial* damage regions —
        // typically only the newly-exposed strip at the right/bottom
        // edge — and reuses the cached render node for the rest. The
        // `set_draw_func` opens every frame by clearing the whole DA to
        // the theme background, but Cairo honours GTK's clip, so that
        // "full" clear only ever covers the damaged strip. The undamaged
        // middle keeps its pre-resize render node, so any glyph the
        // pre-resize grid painted there survives — the ghost. A bare
        // `queue_draw()` (no region) invalidates the entire widget, so
        // the next `set_draw_func` runs unclipped and the whole-DA clear
        // actually clears the whole DA. GTK coalesces repeated per-edge
        // invalidations into one repaint per frame, so this stays cheap
        // during a live drag. The PTY resize / SIGWINCH stays debounced
        // below — only the *paint* is forced eager here.
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
            // The fired timer is no longer "pending" — clear so a future
            // resize doesn't try to cancel a dead source. Re-entrancy guard
            // checked at fire time — a dialog pump may have started after this
            // timer was scheduled.
            resize_timer_inner.set(None);
            // Re-entrancy guard, re-checked at fire time — a dialog pump may
            // have started after this timer was scheduled.
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
            // A resize and a DPI change can arrive in the same GTK event
            let Some(outcome) = try_dispatch_with(
                &backend,
                &app,
                |backend_mut| {
                    // Keep the tracked scale current on every
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

/// Wires `da`'s `notify::scale-factor` signal — the GObject
/// property-change signal GTK fires for a *pure* DPI change (dragging
/// the window to a different-DPI monitor, or a live scaling-setting
/// change) with no accompanying resize, which `setup_resize_controller`
/// alone would never observe. No debounce: unlike a live resize drag, a
/// DPI change doesn't arrive as a rapid-fire burst.
fn setup_hidpi_controller<A: AppLogic + 'static>(
    da: &DrawingArea,
    window: &ApplicationWindow,
    app: &Rc<RefCell<A>>,
    backend: &Rc<RefCell<GtkBackend>>,
    pump_depth: &crate::desktop::ModalPumpDepth,
) {
    let backend = backend.clone();
    let app = app.clone();
    let da_for_dpi = da.clone();
    let window_for_dpi = window.clone();
    let pump_depth = pump_depth.clone();
    da.connect_scale_factor_notify(move |da| {
        if pump_depth.is_pumping() {
            return;
        }
        let scale = da.scale_factor() as f32;
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

/// Wires `da`'s OS file-drop target. `gtk::DropTarget` is a
/// `gtk::EventController` added to `da` the same way the click/motion/
/// scroll controllers are, not a separate widget — GTK4's DnD model
/// routes a drop to whichever controller on the target widget declares
/// it accepts the dragged `GType`. `gdk::FileList` is the type GTK/GNOME
/// apps (file managers, browsers) drop file references as.
fn setup_file_drop_controller<A: AppLogic + 'static>(
    da: &DrawingArea,
    window: &ApplicationWindow,
    app: &Rc<RefCell<A>>,
    backend: &Rc<RefCell<GtkBackend>>,
    pump_depth: &crate::desktop::ModalPumpDepth,
) {
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
        let Some(outcome) = try_dispatch(&backend, &app, ev) else {
            return false;
        };
        apply_event_outcome(outcome, &backend.borrow(), &da_for_drop, &window_for_drop);
        true
    });
    da.add_controller(drop_target);
}

/// Wires the window's `close-request` signal — the OS "×"
/// / Alt-F4 / window-manager close — dispatches `UiEvent::WindowClose`
/// through `dispatch_event`, then only lets the close proceed if the
/// app's own reaction was `Exit`. An app that doesn't return `Exit`
/// implicitly vetoes the close.
fn setup_close_request<A: AppLogic + 'static>(
    window: &ApplicationWindow,
    da: &DrawingArea,
    app: &Rc<RefCell<A>>,
    backend: &Rc<RefCell<GtkBackend>>,
    pump_depth: &crate::desktop::ModalPumpDepth,
    events_handle: &Rc<RefCell<std::collections::VecDeque<UiEvent>>>,
) {
    let backend = backend.clone();
    let app = app.clone();
    let da_for_redraw = da.clone();
    let pump_depth = pump_depth.clone();
    let events_handle = events_handle.clone();
    window.connect_close_request(move |_window| {
        // This handler runs re-entrantly when app code inside `handle` calls
        // `window.close()`: GTK turns that into a synchronous `close-request`
        // re-emission. The `pump_depth` guard covers only nested modal pumps;
        // `try_dispatch` below degrades instead of panicking on a double borrow.
        if pump_depth.is_pumping() {
            return glib::Propagation::Proceed;
        }
        // This is the handler the reported abort actually hit —
        // `pump_depth` only ever sees a *nested modal pump*; it's
        // still 0 here when app code, synchronously inside an outer
        // `dispatch_event` call further up this very stack, calls
        // `window.close()`. GTK turns that into a second, synchronous
        // `close-request` emission that re-enters this closure while
        // `backend`/`app` are still borrowed by the outer call.
        //
        // Unlike every other handler, this one can't just skip the
        // event on a double-borrow: `WindowClose` is exactly what a
        // consumer needs to see to decide whether to actually exit, and
        // dropping it silently would turn an in-dispatch `close()` call
        // into a permanent no-op. So defer instead: push straight onto
        // `events_handle` — a plain `Rc<RefCell<VecDeque<UiEvent>>>`
        // with its own, independent borrow state, so this can't
        // double-borrow no matter how deep the reentrant stack is — and
        // refuse the close *this* turn, same as the ordinary
        // `EventOutcome::Continue` arm below. The queued `WindowClose`
        // reaches the app cleanly a tick later via the idle drain loop,
        // once every outstanding borrow from this stack has unwound; if
        // the app wants to exit it drives that itself through
        // `Reaction::Exit` → `request_exit` → `window.destroy()`, which
        // — unlike `close()` — cannot loop back through this handler.
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

/// Wires the low-rate idle drain of the backend's event queue, plus the
/// periodic `AppLogic::tick` call. Signal handlers dispatch synchronously
/// through `app.handle`; the backend's queue is drained here on idle.
fn setup_event_drain<A: AppLogic + 'static>(
    da: &DrawingArea,
    window: &ApplicationWindow,
    app: &Rc<RefCell<A>>,
    backend: &Rc<RefCell<GtkBackend>>,
    pump_depth: &crate::desktop::ModalPumpDepth,
    events_handle: &Rc<RefCell<std::collections::VecDeque<UiEvent>>>,
) {
    let drain_da = da.clone();
    let drain_window = window.clone();
    let drain_app = app.clone();
    let drain_backend = backend.clone();
    let drain_pump_depth = pump_depth.clone();
    let drain_events_handle = events_handle.clone();
    let drain_and_tick: Rc<dyn Fn()> = Rc::new(move || {
        // This is the callback that produces re-entrancy: a file dialog's
        // nested `pump_until_ready` loop (invoked from `app.handle`, while
        // `backend` is still held mutably borrowed) services this source too.
        // `pump_depth` guards against nested modal pumps; `try_borrow_mut` below
        // degrades instead of panicking on a double borrow. Skip this tick and
        // let the next one (after the dialog closes) pick up any pending events.
        if drain_pump_depth.is_pumping() {
            return;
        }
        // Degrade instead of panicking if some other re-entrant source already
        // holds the backend borrow. The next tick (or the next `waker()` call)
        let Ok(mut drain_backend_mut) = drain_backend.try_borrow_mut() else {
            return;
        };
        let events = drain_backend_mut.poll_events();
        drop(drain_backend_mut);
        let mut events_iter = events.into_iter();
        while let Some(ev) = events_iter.next() {
            // Degrade instead of panicking; requeue the event onto `events_handle`
            // (a separate `RefCell`) so it isn't lost.
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

        // Periodic tick — called after every queue drain, including idle ticks
        // where no events arrived. Degrades instead of panicking if `backend`/
        // `app` are already borrowed.
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

    // Install this closure as `waker()`'s wake target before the timer fires,
    // so a background thread that calls the waker reaches it.
    backend
        .borrow()
        .set_wake_callback(Rc::clone(&drain_and_tick));

    // This fallback idle tick runs at `IDLE_POLL_CEILING` (250ms) as a safety
    // net — see that constant's doc for why a fallback still exists. Apps that
    // need a tighter cadence (a spinner frame, a caret blink) should return
    // `Reaction::RedrawAfter` with the exact interval they need instead.
    glib::timeout_add_local(crate::runtime::IDLE_POLL_CEILING, move || {
        drain_and_tick();
        glib::ControlFlow::Continue
    });
}

/// Schedules the one-shot headless smoke-mode check: `cfg.after_ms`
/// after the window is presented, checks the `DrawingArea`'s allocated
/// size, optionally round-trips `cfg.paste_text` through the real OS
/// clipboard and replays it as a synthetic Ctrl-V, then closes the window
/// so an unattended `xvfb-run` invocation exits deterministically.
/// Exempt from the crate-wide `print_stderr` deny; its failure output is
/// the headless smoke harness's normal output.
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

        // `da.is_focus()` asks GTK's own toplevel-focus bookkeeping
        // "is this the widget holding internal focus", independent of
        // whether the X/Wayland compositor also granted the *window*
        // native input focus (`has_focus()` conflates the two, and
        // `xvfb-run` here runs with no window manager to arbitrate that
        // — see the module doc on this script). That makes it the right
        // predicate for this regression: `activate`'s `set_focusable` +
        // `grab_focus()` calls are a GTK-level widget request, not an
        // X11/Wayland one, so this is exactly what they're responsible
        // for.
        if !da.is_focus() {
            eprintln!(
                "quadraui smoke: DrawingArea never became GTK's focus widget — \
                 this is the quadraui#235 missing-grab_focus regression class"
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

        // `destroy()`, not `close()`: this forced
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
/// pair, plus the `GtkBackend` a `RedrawAfter` outcome arms its scheduled
/// wake against — the redraw/exit/frame-request target every GTK signal
/// closure in this module applies its [`EventOutcome`] (or raw [`Reaction`])
/// to, via [`runtime::apply_outcome`].
///
/// Takes `backend: &'a GtkBackend` (a shared reference, not the
/// `Rc<RefCell<GtkBackend>>` every call site holds) — cheap to obtain even
/// from a live `RefMut` several call sites already hold across the
/// `apply_event_outcome` call: a plain `&*backend_mut` reborrow, not a
/// second `borrow_mut()` that would panic against an already-live borrow.
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
        // `destroy()`, not `close()`: the app has
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
/// the active text-selection highlight. This body is shared by the live
/// runner and [`crate::gtk::testing::GtkDriver`].
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
    // next repaint. Monospace is required because `draw_editor`'s
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
    // short at 500-char scroll). Delegates to
    // `backend::measure_char_width_px` rather than
    // `layout.pixel_size()` directly: that rounds to whole pixels, which
    // silently loses columns once a consumer divides pane width by it.
    let char_w = super::backend::measure_char_width_px(&layout);

    // Seed from the tracked scale factor (kept current by the
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
    // `ui_font` is set once per app at setup; don't re-seed it every frame.
    // However, its *advance* needs re-deriving every frame for
    // `Backend::list_char_width()`, so `set_ui_font` takes effect on the
    // next repaint the same way `set_editor_font` does for `char_width()`.
    backend.refresh_chrome_char_width(&layout);

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

    // Resolve the currently-focused widget's rect (if any) from this frame's
    // tab stops before entering the frame scope.
    let focus_ring_rect = runtime::focused_stop_rect(backend, app, A::AreaId::default());
    backend.enter_frame_scope(cr, &layout, |b| {
        // Single-area runner: pass the default `AreaId`.
        app.render(b, A::AreaId::default());
        // After app.render: paint the focus-ring convention.
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

// `EventOutcome` — what the caller should do after [`dispatch_event`]
// handles one event — is defined once in `crate::runtime` and shared by
// every backend runner; imported at the top of this file.

/// Dispatch one already-translated [`UiEvent`] through the app, applying
/// the shared runner pre-processing pipeline first. This is the single
/// funnel every GTK signal closure above routes through (key press,
/// click press/release, motion, scroll, resize, idle-drain) — see the
/// module doc's "Shared with the headless test driver" section for why
/// that matters.
///
/// The pre-processing itself — ActivityBar keyboard-focus redirect,
/// Tab/Shift+Tab focus cycling,
/// global accelerator rewrite, Ctrl-C copy, Ctrl-V/Ctrl-Shift-V paste,
/// middle-click PRIMARY-selection paste, Ctrl-A select-all, selection-
/// display clearing, `TextSelectionChanged` — lives in
/// [`crate::runtime::preprocess_event`], shared with
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

// ── Re-entrancy backstop ──────────────────────────────────────
//
// A GTK signal handler must never panic — see the module's `close-request`
// handler. `pump_depth.is_pumping()` (checked by every signal closure above)
// guards only nested modal pumps. It doesn't guard app code already inside
// a `dispatch_event` call that synchronously invokes a GTK method re-emitting
// the same signal (`window.close()` → `close-request`). That second source
// leaves `pump_depth` at 0, so it passes through into a second `borrow_mut()`
// on an already-mutably-borrowed `RefCell` — a panic that can't unwind across
// the GLib trampoline frame, aborting the process.
//
// Rather than add a second counter (which would fix only one source), these
// ask the `RefCell`s themselves — they already know the true answer, including
// for sources nobody has enumerated yet. Every caller below must treat a
// `None` return as "degrade, don't dispatch" — most skip the event; the
// `close-request` handler defers it via `events_handle` because silently
// dropping a `WindowClose` would make an in-dispatch `close()` call a
// permanent no-op.

/// Attempt to mutably borrow both `backend` and `app`, returning `None`
/// instead of panicking if either is already borrowed. See the re-entrancy
/// backstop section above.
fn try_borrow_both<'a, A>(
    backend: &'a Rc<RefCell<GtkBackend>>,
    app: &'a Rc<RefCell<A>>,
) -> Option<(std::cell::RefMut<'a, GtkBackend>, std::cell::RefMut<'a, A>)> {
    let backend_mut = backend.try_borrow_mut().ok()?;
    let app_mut = app.try_borrow_mut().ok()?;
    Some((backend_mut, app_mut))
}

/// [`try_borrow_both`] + [`dispatch_event`], running `pre` on the
/// borrowed backend in between (e.g. `set_dpi_scale`) before the event
/// is dispatched. `None` on a double-borrow.
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
/// mutably borrowed across a loop of several events. `None` on a
/// double-borrow of `app`.
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
    //! Coverage for [`RunConfig`] — display-free, since
    //! `RunConfig` itself is a plain data struct with no GTK dependency.
    //! `run_with`'s actual wiring (app id → `Application::builder`, title
    //! → `ApplicationWindow::builder`) can't be exercised without a real
    //! display; that's covered by the operator-run smoke tier same as the
    //! rest of this module (see `smoke_tests` below).
    use super::*;

    #[test]
    fn default_matches_runs_previous_hardcoded_values() {
        // `run(app)` used to hardcode the app id and window title directly.
        // Now that `run` is `run_with(app, RunConfig::default())`, this pins
        // the defaults so they can't silently drift.
        let config = RunConfig::default();
        assert_eq!(config.app_id, "org.quadraui.app");
        assert_eq!(config.title, "quadraui app");
        assert_eq!(config.icon_name, None);
        assert!(config.single_instance, "single-instance dedup defaults on");
    }

    #[test]
    fn new_sets_both_fields() {
        let config = RunConfig::new("io.github.jdonaghy.kubeui-gtk", "kubeui");
        assert_eq!(config.app_id, "io.github.jdonaghy.kubeui-gtk");
        assert_eq!(config.title, "kubeui");
        assert_eq!(config.icon_name, None);
        assert!(config.single_instance);
    }

    /// `with_single_instance(false)` is how an app opts out of
    /// GTK's default single-instance dedup — e.g. to run several debug
    /// instances side by side.
    #[test]
    fn with_single_instance_overrides_the_default() {
        let config = RunConfig::new("a.b.c", "Title").with_single_instance(false);
        assert!(!config.single_instance);
    }

    #[test]
    fn new_accepts_owned_and_borrowed_strings() {
        let owned = RunConfig::new(String::from("a.b.c"), String::from("Title"));
        let borrowed = RunConfig::new("a.b.c", "Title");
        assert_eq!(owned, borrowed);
    }

    /// `with_icon_name` stores the icon name for `activate` to feed
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

#[cfg(target_os = "macos")]
#[cfg(test)]
mod activate_macos_process_tests {
    //! The activation calls themselves need a real `NSApplication` on a
    //! live window server and can't run headlessly (same constraint
    //! `window_close_tests` documents for `close-request`) — what *is*
    //! headlessly testable is the main-thread guard, same shape as
    //! `require_gtk_does_not_panic_off_main_thread_on_macos` in
    //! `gtk/services.rs`: `cargo test` never runs a `#[test]` fn on the
    //! process's real OS main thread, so `MainThreadMarker::new()`
    //! deterministically returns `None` here, and this pins that the
    //! function returns instead of reaching for a marker it doesn't have.
    use super::*;

    #[test]
    fn activate_macos_process_is_a_no_op_off_the_main_thread() {
        // No assertion beyond "doesn't panic" is possible here — the
        // function has no return value and touches no state this test
        // can observe. The guard this covers is the early `return` on
        // `MainThreadMarker::new()` returning `None`; reaching the
        // `NSApplication::sharedApplication` call below it from a
        // `cargo test` thread is what would panic.
        activate_macos_process();
    }
}

#[cfg(test)]
mod smoke_tests {
    //! Unit tests for GTK's use of the headless smoke-mode helpers.
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
        // Content wrapped into an ~8px-wide column.
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

    /// GTK's real env-var names still enable smoke mode through
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
    //! Coverage for GTK clipboard-paste and PRIMARY-selection routing.
    //!
    //! Each test installs an in-memory clipboard via
    //! [`GtkBackend::install_test_clipboard`] before dispatching, so both
    //! "there IS something to paste" and "there is nothing to paste" branches
    //! are covered on any host without depending on the live clipboard.
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
        // Several terminal emulators reserve Ctrl-V for a
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
        // Shift-modifier regression check: `modifiers.cmd` is Super/Meta on GTK,
        // and plain Super+V must never trigger paste on GTK. Passing
        // `PasteModifier::Ctrl` here ensures that stays true.
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
        // Ctrl-Shift-V is the paste shortcut in terminal
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
        // must never fall back to CLIPBOARD.
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

    // ── Double-click folding ──────────────────────────────────────────

    /// Before #813, `GtkDriver::dispatch` had no double-click folding at all —
    /// GTK only folded from native `GdkEventType` inside `gtk/run.rs`'s own
    /// `connect_pressed` closure, which `GtkDriver` never fires. Two scripted
    /// `MouseDown`s at the same spot were always delivered as two separate events.
    /// This test confirms the fold now moves into the shared `dispatch_event` path.
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
    //! Coverage for `UiEvent::WindowClose` dispatch. The live `connect_close_request`
    //! wiring (GDK signal → `dispatch` → `glib::Propagation`) needs a real display and
    //! can't run through `GtkDriver`. What is headless-testable is that `dispatch_event`
    //! doesn't intercept `WindowClose` — it must reach `app.handle` verbatim and
    //! round-trip the app's `Reaction` unchanged, since the app's return value is the
    //! veto mechanism: `activate` only lets the OS close when the app returns `Exit`.
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
    //! Coverage for the structural re-entrancy backstop
    //! (`try_borrow_both` / `try_dispatch` / `try_dispatch_with` /
    //! `try_dispatch_borrowed`) that every guarded signal closure now routes through,
    //! replacing a plain `borrow_mut()` that would panic and abort the process on a
    //! double borrow. The live `close-request` re-entry itself needs a real display;
    //! what's headlessly testable is the degrade path: hold one `RefCell` borrowed —
    //! the state an outer `dispatch_event` call leaves the stack in — and confirm
    //! these helpers return `None` instead of panicking.
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
        // An outer `dispatch_event` call holds `backend` mutably borrowed when a
        // re-entered signal handler runs — app code synchronously triggering the signal
        // again. Not a nested modal pump (which `pump_depth` covers).
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

    /// When `try_dispatch` reports a double-borrow, the `close-request` handler
    /// pushes `WindowClose` onto `events_handle` instead of dropping it. Confirms
    /// both halves: pushing while `backend` is borrowed doesn't panic (the whole point
    /// of deferring through `events_handle`), and the event reaches `poll_events()`
    /// once the outer borrow is released, so it isn't lost.
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
