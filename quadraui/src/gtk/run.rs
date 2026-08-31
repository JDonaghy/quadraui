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
//! (ActivityBar keyboard-focus intercept, accelerator matching, Ctrl-C/
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

use std::cell::{Cell, RefCell};
use std::env;
use std::rc::Rc;
use std::time::Duration;

use gtk4::cairo::Context;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    pango as pg, Application, ApplicationWindow, DrawingArea, EventControllerKey,
    EventControllerMotion, EventControllerScroll, EventControllerScrollFlags, GestureClick,
};
use pangocairo::functions as pcfn;

use super::backend::GtkBackend;
use super::events::{
    gdk_button_to_quadraui, gdk_key_to_uievent, gdk_modifiers_to_quadraui, gdk_resize_to_uievent,
    gdk_scroll_to_uievent,
};
use crate::backend::Backend;
use crate::dispatch::{dispatch_click, dispatch_mouse_drag, dispatch_mouse_up};
use crate::runner::{AppLogic, Reaction};
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

/// Headless smoke-mode config (quadraui#450, GD-5) — see the module doc's
/// "Headless smoke mode" section. `None` unless `QUADRAUI_GTK_SMOKE_MS` is
/// set.
#[derive(Clone)]
struct SmokeConfig {
    /// Delay after `window.present()` before the one-shot check fires and
    /// the window is closed.
    after_ms: u64,
    /// `QUADRAUI_GTK_SMOKE_PASTE`, if set — round-tripped through the real
    /// OS clipboard and then replayed as a synthetic Ctrl-V.
    paste_text: Option<String>,
}

impl SmokeConfig {
    /// Reads the smoke-mode env vars once. Returns `None` (the default —
    /// zero behavioral change) unless `QUADRAUI_GTK_SMOKE_MS` parses as a
    /// `u64`.
    fn from_env() -> Option<Self> {
        let after_ms = env::var("QUADRAUI_GTK_SMOKE_MS").ok()?.parse().ok()?;
        let paste_text = env::var("QUADRAUI_GTK_SMOKE_PASTE").ok();
        Some(Self {
            after_ms,
            paste_text,
        })
    }
}

/// Is `width`x`height` a plausible, non-broken `DrawingArea` allocation?
/// The direct regression check for the quadraui#437 tiny/wrapped-window
/// bug class. Pure and display-free so it's covered by an ordinary unit
/// test (see `tests` below) with no Xvfb required.
fn smoke_size_ok(width: i32, height: i32) -> bool {
    width >= SMOKE_MIN_WIDTH && height >= SMOKE_MIN_HEIGHT
}

/// Did the OS clipboard round-trip `written` back byte-for-byte? Pure
/// comparison, factored out so the pass/fail rule is unit-testable
/// without a real clipboard.
fn smoke_clipboard_round_trip_ok(written: &str, read_back: Option<&str>) -> bool {
    read_back == Some(written)
}

/// Drive `app` to completion in a basic single-`DrawingArea` GTK
/// environment.
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
///
/// ## Window title + app id
///
/// Both default to a generic `"quadraui app"`. Apps that need a
/// custom title or a stable app id (Flatpak, dock-pinning) build the
/// runner via lower-level pieces in `quadraui::gtk::backend` /
/// `events`. A future stage may add a builder API.
pub fn run<A: AppLogic + 'static>(app: A) -> std::process::ExitCode {
    let app = Rc::new(RefCell::new(app));
    let backend = Rc::new(RefCell::new(GtkBackend::new()));
    // quadraui#450 (GD-5): `None` unless `QUADRAUI_GTK_SMOKE_MS` is set —
    // see the module doc's "Headless smoke mode" section.
    let smoke = SmokeConfig::from_env();
    let smoke_failed = Rc::new(Cell::new(false));

    let gapp = Application::builder()
        .application_id("org.quadraui.app")
        .build();

    {
        let app = app.clone();
        let backend = backend.clone();
        let smoke = smoke.clone();
        let smoke_failed = smoke_failed.clone();
        gapp.connect_activate(move |gapp| {
            activate(
                gapp,
                app.clone(),
                backend.clone(),
                smoke.clone(),
                smoke_failed.clone(),
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
    std::process::ExitCode::from(glib_code.value() as u8)
}

fn activate<A: AppLogic + 'static>(
    gapp: &Application,
    app: Rc<RefCell<A>>,
    backend: Rc<RefCell<GtkBackend>>,
    smoke: Option<SmokeConfig>,
    smoke_failed: Rc<Cell<bool>>,
) {
    let window = ApplicationWindow::builder()
        .application(gapp)
        .title("quadraui app")
        .default_width(DEFAULT_WINDOW_WIDTH)
        .default_height(DEFAULT_WINDOW_HEIGHT)
        .build();

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
        pctx.set_font_description(Some(&font_desc));
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
            if pump_depth.get() > 0 {
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
            if pump_depth.get() > 0 {
                return glib::Propagation::Proceed;
            }
            let Some(ev) = gdk_key_to_uievent(key, modifier, false) else {
                return glib::Propagation::Proceed;
            };

            // All key-press pre-processing — ActivityBar keyboard-focus
            // intercept, `Global` accelerator matching (#445), Ctrl-C/V/A
            // interception — lives in the shared `dispatch_event` (see the
            // module doc's "Shared with the headless test driver" section)
            // so the live GTK path and `GtkDriver::press`/`type_char`
            // (quadraui#446) can't drift apart.
            let outcome = {
                let mut backend_mut = backend.borrow_mut();
                let mut app_mut = app.borrow_mut();
                dispatch_event(ev, &mut backend_mut, &mut *app_mut)
            };
            apply_event_outcome(outcome, &da_for_redraw, &window_for_close);
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
        click.connect_pressed(move |gesture, n_press, x, y| {
            // #427 re-entrancy guard — see the key-press handler above.
            if pump_depth.get() > 0 {
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
                    backend.borrow_mut().stash_window_press(
                        device,
                        gdk_button as i32,
                        x,
                        y,
                        event.time(),
                    );
                }
            }

            if n_press == 2 {
                // Double-click: clear selection and deliver DoubleClick directly.
                // (Not a `MouseDown`, so `dispatch_event`'s selection-display
                // clear doesn't fire for it — do it explicitly here, same as
                // before the refactor.)
                let mut backend_mut = backend.borrow_mut();
                backend_mut.clear_selection_display();
                let ev = UiEvent::DoubleClick {
                    widget: None,
                    position,
                };
                let outcome = {
                    let mut app_mut = app.borrow_mut();
                    dispatch_event(ev, &mut backend_mut, &mut *app_mut)
                };
                drop(backend_mut);
                apply_event_outcome(outcome, &da_for_redraw, &window_for_close);
                return;
            }

            let mut backend_mut = backend.borrow_mut();

            // Route through dispatch_click so text-region clicks begin a
            // TextSelection drag and scrollbar clicks begin scrollbar drags.
            // The resulting `MouseDown` event(s) go through `dispatch_event`
            // below, which clears the previous selection-highlight display
            // (mirrors the TUI runner's own `MouseDown` pre-processing).
            let events = {
                let stack_rc = backend_mut.modal_stack_handle();
                let drag_rc = backend_mut.drag_state_handle();
                let stack = stack_rc.borrow();
                let mut drag = drag_rc.borrow_mut();
                let evs = dispatch_click(
                    &stack,
                    &[], // scroll surfaces not tracked in the runner
                    &backend_mut.text_regions,
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
                // Only `MouseDown` events are emitted by dispatch_click;
                // pass them to the app.
                let outcome = {
                    let mut app_mut = app.borrow_mut();
                    dispatch_event(ev, &mut backend_mut, &mut *app_mut)
                };
                match outcome {
                    EventOutcome::Continue => {}
                    EventOutcome::Redraw => needs_redraw = true,
                    EventOutcome::Exit => {
                        window_for_close.close();
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
            if pump_depth.get() > 0 {
                return;
            }
            let mut backend_mut = backend.borrow_mut();
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
                let outcome = {
                    let mut app_mut = app.borrow_mut();
                    dispatch_event(ev, &mut backend_mut, &mut *app_mut)
                };
                apply_event_outcome(outcome, &da_for_redraw, &window_for_close);
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
            if pump_depth.get() > 0 {
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
            // Only returns early when a drag is actually committed
            // (control passes to the compositor's native move at that
            // point); otherwise falls through to the normal motion
            // handling below unaffected.
            {
                let mut backend_mut = backend.borrow_mut();
                if let Some((origin_x, origin_y)) = backend_mut.armed_window_drag_origin() {
                    let dx = x - origin_x;
                    let dy = y - origin_y;
                    let threshold = backend_mut.window_drag_threshold_px();
                    if (dx * dx + dy * dy).sqrt() >= threshold {
                        backend_mut.commit_armed_window_drag();
                        return;
                    }
                }
            }

            let modifier = ctrl.current_event_state();
            let buttons = ButtonMask {
                left: modifier.contains(gtk4::gdk::ModifierType::BUTTON1_MASK),
                middle: modifier.contains(gtk4::gdk::ModifierType::BUTTON2_MASK),
                right: modifier.contains(gtk4::gdk::ModifierType::BUTTON3_MASK),
            };
            let position = Point::new(x as f32, y as f32);

            let mut backend_mut = backend.borrow_mut();
            let events = {
                let drag_rc = backend_mut.drag_state_handle();
                let drag = drag_rc.borrow();
                dispatch_mouse_drag(&drag, position, buttons)
            };

            // `TextSelectionChanged` (active-selection state update) and the
            // fallback `app.handle` both live in the shared `dispatch_event`.
            let mut needs_redraw = false;
            for ev in events {
                let outcome = {
                    let mut app_mut = app.borrow_mut();
                    dispatch_event(ev, &mut backend_mut, &mut *app_mut)
                };
                match outcome {
                    EventOutcome::Continue => {}
                    EventOutcome::Redraw => needs_redraw = true,
                    EventOutcome::Exit => {
                        window_for_close.close();
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
            if pump_depth.get() > 0 {
                return glib::Propagation::Proceed;
            }
            let (x, y) = cursor_pos.get();
            let ev = gdk_scroll_to_uievent(dx, dy, x, y);
            let outcome = {
                let mut backend_mut = backend.borrow_mut();
                let mut app_mut = app.borrow_mut();
                dispatch_event(ev, &mut backend_mut, &mut *app_mut)
            };
            apply_event_outcome(outcome, &da_for_redraw, &window_for_close);
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
    // (quadraui#437 follow-up): a live edge-drag fires `connect_resize`
    // dozens of times per second, and apps with PTY-backed side effects
    // (`TerminalApp::handle` → `TerminalSession::resize` → SIGWINCH) were
    // resizing the child shell on every single intermediate frame. A
    // shell's line-editor (readline/zle) redraws its prompt on SIGWINCH;
    // firing another SIGWINCH before that redraw finishes interleaves two
    // half-written escape sequences and leaves the display permanently
    // garbled — no amount of further resizing, scrolling, or typing
    // recovers it, because the line-editor's internal notion of the
    // screen is now out of sync with what's actually on screen. Real
    // terminal emulators avoid this by resizing the PTY only once the
    // drag settles, not on every intermediate frame; `resize_timer` below
    // does the same via a short trailing-edge debounce. Painting itself
    // is unaffected and stays perfectly live — `set_draw_func` re-reads
    // the DA's actual allocated size every frame regardless of whether
    // the debounced event has fired yet.
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
            if pump_depth.get() > 0 {
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
            let id = glib::source::timeout_add_local_once(Duration::from_millis(120), move || {
                // The fired timer is no longer "pending" — clear so a
                // future resize doesn't try to cancel a dead source.
                resize_timer_inner.set(None);
                // #427 re-entrancy guard, re-checked at fire time —
                // a dialog pump may have started after this timer was
                // scheduled.
                if pump_depth.get() > 0 {
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
                let outcome = {
                    let mut backend_mut = backend.borrow_mut();
                    let mut app_mut = app.borrow_mut();
                    dispatch_event(ev, &mut backend_mut, &mut *app_mut)
                };
                apply_event_outcome(outcome, &da_for_redraw, &window_for_close);
            });
            resize_timer.set(Some(id));
        });
    }

    // ── Backend event-queue drain (low-rate idle) ─────────────────
    //
    // Producer-side event controllers above already dispatch
    // synchronously through `app.handle` and trigger redraws. The
    // backend's queue exists as a forward-compat seam — any future
    // signal handlers that push directly to `events_handle()` get
    // drained here on each idle tick.
    let drain_da = da.clone();
    let drain_window = window.clone();
    // Cloned (rather than moved) so `app`/`backend` stay available below
    // for the quadraui#450 headless smoke-mode hook.
    let drain_app = app.clone();
    let drain_backend = backend.clone();
    glib::timeout_add_local(Duration::from_millis(33), move || {
        // #427 re-entrancy guard: this is the callback that produced the
        // original crash report. A file dialog's nested `pump_until_ready`
        // loop (invoked from `app.handle` above, while `backend` is still
        // held mutably borrowed by that call) services *this* GLib timer
        // source too — without the guard, `backend.borrow_mut()` below
        // double-borrows and panics inside a non-unwindable GLib callback
        // frame, aborting the process. Skip this tick entirely and let the
        // next one (after the dialog closes) pick up any pending events.
        if pump_depth.get() > 0 {
            return glib::ControlFlow::Continue;
        }
        let events = drain_backend.borrow_mut().poll_events();
        for ev in events {
            let outcome = {
                let mut backend_mut = drain_backend.borrow_mut();
                let mut app_mut = drain_app.borrow_mut();
                dispatch_event(ev, &mut backend_mut, &mut *app_mut)
            };
            apply_event_outcome(outcome, &drain_da, &drain_window);
        }

        // Periodic tick — called after every queue drain, including
        // idle ticks where no events arrived. Lets apps drive timer
        // logic without synthetic event injection.
        let tick_reaction = {
            let mut backend_mut = drain_backend.borrow_mut();
            let mut app_mut = drain_app.borrow_mut();
            app_mut.tick(&mut *backend_mut)
        };
        apply_reaction(tick_reaction, &drain_da, &drain_window);

        glib::ControlFlow::Continue
    });

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
        if !smoke_size_ok(width, height) {
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

        window.close();
    });
}

fn apply_reaction(reaction: Reaction, da: &DrawingArea, window: &ApplicationWindow) {
    match reaction {
        Reaction::Continue => {}
        Reaction::Redraw => da.queue_draw(),
        Reaction::Exit => window.close(),
    }
}

/// Same as [`apply_reaction`] but for the [`EventOutcome`] that
/// [`dispatch_event`] returns.
fn apply_event_outcome(outcome: EventOutcome, da: &DrawingArea, window: &ApplicationWindow) {
    match outcome {
        EventOutcome::Continue => {}
        EventOutcome::Redraw => da.queue_draw(),
        EventOutcome::Exit => window.close(),
    }
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

    backend.begin_frame(crate::Viewport::new(width as f32, height as f32, 1.0));
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

    backend.enter_frame_scope(cr, &layout, |b| {
        // Single-area runner: pass the default `AreaId`.
        app.render(b, A::AreaId::default());
    });

    // After app.render: overlay selection highlight on top of the
    // rendered content (mirrors TUI's apply_selection_highlight call in
    // the terminal.draw closure).
    backend.apply_selection_highlight(cr);

    backend.end_frame();
}

/// Whether a key press should trigger clipboard paste: plain Ctrl-V or
/// Ctrl-Shift-V, with Alt/Cmd unheld (quadraui#415). `Shift` is
/// deliberately not checked — some terminal emulators reserve Ctrl-V
/// for a literal control byte and use Ctrl-Shift-V as the paste
/// shortcut instead, and quadraui treats the two identically since a
/// terminal grid has no "paste without formatting" distinction.
fn is_paste_keypress(key: &Key, modifiers: &Modifiers) -> bool {
    matches!(key, Key::Char('v') | Key::Char('V'))
        && modifiers.ctrl
        && !modifiers.alt
        && !modifiers.cmd
}

/// What the caller should do after [`dispatch_event`] handles one event.
/// Mirrors [`crate::tui::run::EventOutcome`].
pub(crate) enum EventOutcome {
    /// No redraw needed; keep going.
    Continue,
    /// State changed; schedule a redraw.
    Redraw,
    /// The app requested exit.
    Exit,
}

/// Dispatch one already-translated [`UiEvent`] through the app, applying
/// the runner's built-in pre-processing first. This is the single funnel
/// every GTK signal closure above routes through (key press, click
/// press/release, motion, scroll, resize, idle-drain) — see the module
/// doc's "Shared with the headless test driver" section for why that
/// matters.
///
/// Pre-processing handled here (before — or instead of — the app's
/// `handle`), in priority order:
/// - `KeyPressed` while an `ActivityBar` declared
///   `is_keyboard_focused = true`: redirect to
///   `UiEvent::ActivityBar(id, KeyPressed { … })` instead of the app's
///   normal `handle` (#445 review — must win over accelerators below).
/// - `KeyPressed` matching a registered `Global` accelerator: rewrite to
///   `UiEvent::Accelerator`.
/// - Ctrl-C with an active text selection: copy to the clipboard and
///   deliver `TextCopied` instead of forwarding the raw key press.
/// - Ctrl-V or Ctrl-Shift-V: read the system clipboard and deliver
///   `ClipboardPaste` (GTK has no native paste signal on a bespoke
///   `DrawingArea`, unlike TUI's crossterm bracketed paste). Ctrl-Shift-V
///   is accepted alongside plain Ctrl-V because several terminal
///   emulators reserve Ctrl-V for a control byte and use Shift as the
///   paste-disambiguator (quadraui#415) — quadraui treats them
///   identically since there's no "paste without formatting" distinction
///   for a terminal grid.
/// - Middle-click (`MouseDown` with `MouseButton::Middle`): read the
///   X11/Wayland PRIMARY selection and deliver `ClipboardPaste` — the
///   platform convention for "paste what was last selected", distinct
///   from the CLIPBOARD selection Ctrl-V reads (quadraui#415).
/// - Ctrl-A: select the entire content of the most-recently focused
///   `TextRegion`, if one is registered.
/// - `MouseDown`: clear the displayed selection highlight (a fresh drag
///   may be starting).
/// - `TextSelectionChanged`: update the backend's active selection and
///   force a redraw.
///
/// Anything not matched above falls through to `app.handle` unchanged.
pub(crate) fn dispatch_event<A: AppLogic>(
    event: UiEvent,
    backend: &mut GtkBackend,
    app: &mut A,
) -> EventOutcome {
    // ── ActivityBar keyboard focus intercept ────────────────────────
    if let UiEvent::KeyPressed {
        ref key, modifiers, ..
    } = event
    {
        let focused_bar = backend.focused_activity_bar_id().cloned();
        if let Some(bar_id) = focused_bar {
            let key_str = crate::primitives::activity_bar::key_to_activity_bar_string(key);
            let bar_ev = UiEvent::ActivityBar(
                bar_id,
                crate::ActivityBarEvent::KeyPressed {
                    key: key_str,
                    modifiers,
                },
            );
            return match app.handle(bar_ev, backend) {
                Reaction::Continue => EventOutcome::Continue,
                Reaction::Redraw => EventOutcome::Redraw,
                Reaction::Exit => EventOutcome::Exit,
            };
        }
    }

    // ── Global accelerator dispatch (#445) ───────────────────────────
    let event = if let UiEvent::KeyPressed { key, modifiers, .. } = &event {
        match backend.match_keypress(key, *modifiers) {
            Some(id) => UiEvent::Accelerator(id, *modifiers),
            None => event,
        }
    } else {
        event
    };

    // ── Ctrl-C interception (text selection) ─────────────────────────
    if let UiEvent::KeyPressed {
        key: Key::Char('c'),
        modifiers:
            Modifiers {
                ctrl: true,
                shift: false,
                alt: false,
                cmd: false,
            },
        ..
    } = &event
    {
        if backend.active_text_selection().is_some() {
            let text = backend.extract_selection_text();
            backend.services().clipboard().write_text(&text);
            backend.clear_text_selection();
            // Deliver TextCopied so the app can confirm (e.g. update a
            // status bar message). Mirrors the TUI runner.
            return match app.handle(UiEvent::TextCopied(text), backend) {
                Reaction::Continue => EventOutcome::Continue,
                Reaction::Redraw => EventOutcome::Redraw,
                Reaction::Exit => EventOutcome::Exit,
            };
        }
    }

    // ── Ctrl-V / Ctrl-Shift-V interception (paste) ────────────────────
    if let UiEvent::KeyPressed { key, modifiers, .. } = &event {
        if is_paste_keypress(key, modifiers) {
            if let Some(text) = backend.services().clipboard().read_text() {
                return match app.handle(UiEvent::ClipboardPaste(text), backend) {
                    Reaction::Continue => EventOutcome::Continue,
                    Reaction::Redraw => EventOutcome::Redraw,
                    Reaction::Exit => EventOutcome::Exit,
                };
            }
            return EventOutcome::Continue;
        }
    }

    // ── Middle-click interception (PRIMARY-selection paste) ───────────
    //
    // X11/Wayland convention: middle-click pastes the PRIMARY selection
    // (whatever text was last selected anywhere), independent of the
    // CLIPBOARD selection Ctrl-V reads. Falls through to ordinary
    // `MouseDown` handling (scrollbar drags, text-selection start, …)
    // when there's no primary selection to paste (quadraui#415).
    if let UiEvent::MouseDown {
        button: MouseButton::Middle,
        ..
    } = &event
    {
        if let Some(text) = backend.services().clipboard().read_primary_selection() {
            return match app.handle(UiEvent::ClipboardPaste(text), backend) {
                Reaction::Continue => EventOutcome::Continue,
                Reaction::Redraw => EventOutcome::Redraw,
                Reaction::Exit => EventOutcome::Exit,
            };
        }
    }

    // ── Ctrl-A interception (select-all for text regions) ────────────
    if let UiEvent::KeyPressed {
        key: Key::Char('a') | Key::Char('A'),
        modifiers:
            Modifiers {
                ctrl: true,
                shift: false,
                alt: false,
                cmd: false,
            },
        ..
    } = &event
    {
        if backend.select_all_text_region() {
            return EventOutcome::Redraw;
        }
    }

    // ── MouseDown: clear the displayed selection highlight ────────────
    if let UiEvent::MouseDown { .. } = &event {
        backend.clear_selection_display();
    }

    // ── TextSelectionChanged: update active selection while dragging ──
    let mut force_redraw = false;
    if let UiEvent::TextSelectionChanged {
        region,
        anchor,
        focus,
    } = &event
    {
        backend.set_active_text_selection(region.clone(), *anchor, *focus);
        force_redraw = true;
    }

    // ── Normal app dispatch ────────────────────────────────────────────
    match app.handle(event, backend) {
        Reaction::Continue => {
            if force_redraw {
                EventOutcome::Redraw
            } else {
                EventOutcome::Continue
            }
        }
        Reaction::Redraw => EventOutcome::Redraw,
        Reaction::Exit => EventOutcome::Exit,
    }
}

#[cfg(test)]
mod smoke_tests {
    //! Unit tests for the headless smoke-mode helpers (quadraui#450,
    //! GD-5). These are pure/display-free by design — the live
    //! Xvfb/Broadway run itself is an operator-run tier documented in
    //! `quadraui/docs/TESTING.md`, not something CI (no Xvfb — see
    //! `ci.yml`) or this in-process test can exercise.
    use super::*;

    #[test]
    fn smoke_size_ok_accepts_the_default_window_size() {
        assert!(smoke_size_ok(DEFAULT_WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT));
    }

    #[test]
    fn smoke_size_ok_accepts_exactly_the_floor() {
        assert!(smoke_size_ok(SMOKE_MIN_WIDTH, SMOKE_MIN_HEIGHT));
    }

    #[test]
    fn smoke_size_ok_rejects_the_437_tiny_window_class() {
        // quadraui#437: content wrapped into an ~8px-wide column.
        assert!(!smoke_size_ok(8, DEFAULT_WINDOW_HEIGHT));
        assert!(!smoke_size_ok(DEFAULT_WINDOW_WIDTH, 8));
        assert!(!smoke_size_ok(8, 8));
    }

    #[test]
    fn smoke_size_ok_rejects_just_under_the_floor() {
        assert!(!smoke_size_ok(SMOKE_MIN_WIDTH - 1, SMOKE_MIN_HEIGHT));
        assert!(!smoke_size_ok(SMOKE_MIN_WIDTH, SMOKE_MIN_HEIGHT - 1));
    }

    #[test]
    fn clipboard_round_trip_ok_when_read_back_matches() {
        assert!(smoke_clipboard_round_trip_ok(
            "quadraui smoke",
            Some("quadraui smoke")
        ));
    }

    #[test]
    fn clipboard_round_trip_rejects_a_missing_read() {
        // The failure mode a headless box with no OS clipboard access
        // actually produces (e.g. Broadway, no real `DISPLAY`).
        assert!(!smoke_clipboard_round_trip_ok("quadraui smoke", None));
    }

    #[test]
    fn clipboard_round_trip_rejects_a_mismatched_read() {
        assert!(!smoke_clipboard_round_trip_ok(
            "quadraui smoke",
            Some("something else")
        ));
    }
}

#[cfg(test)]
mod paste_tests {
    //! Coverage for quadraui#415 — GTK clipboard-paste and PRIMARY-
    //! selection routing added to [`dispatch_event`].
    //!
    //! `is_paste_keypress` is a pure predicate, tested directly with no
    //! display required. The `GtkDriver`-based tests below exercise the
    //! actual [`dispatch_event`] wiring end to end.
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
        assert!(is_paste_keypress(&Key::Char('v'), &mods));
        assert!(is_paste_keypress(&Key::Char('V'), &mods));
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
        assert!(is_paste_keypress(&Key::Char('v'), &mods));
    }

    #[test]
    fn ctrl_alt_v_is_not_a_paste_keypress() {
        let mods = Modifiers {
            ctrl: true,
            alt: true,
            ..Modifiers::default()
        };
        assert!(!is_paste_keypress(&Key::Char('v'), &mods));
    }

    #[test]
    fn ctrl_cmd_v_is_not_a_paste_keypress() {
        let mods = Modifiers {
            ctrl: true,
            cmd: true,
            ..Modifiers::default()
        };
        assert!(!is_paste_keypress(&Key::Char('v'), &mods));
    }

    #[test]
    fn shift_v_without_ctrl_is_not_a_paste_keypress() {
        let mods = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        assert!(!is_paste_keypress(&Key::Char('v'), &mods));
    }

    #[test]
    fn plain_v_is_not_a_paste_keypress() {
        assert!(!is_paste_keypress(&Key::Char('v'), &Modifiers::default()));
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
}
