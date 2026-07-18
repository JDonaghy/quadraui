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

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

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

    let gapp = Application::builder()
        .application_id("org.quadraui.app")
        .build();

    {
        let app = app.clone();
        let backend = backend.clone();
        gapp.connect_activate(move |gapp| {
            activate(gapp, app.clone(), backend.clone());
        });
    }

    let glib_code = gapp.run();
    std::process::ExitCode::from(glib_code.value() as u8)
}

fn activate<A: AppLogic + 'static>(
    gapp: &Application,
    app: Rc<RefCell<A>>,
    backend: Rc<RefCell<GtkBackend>>,
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
        da.set_draw_func(move |da, cr, w, h| {
            // #427 re-entrancy guard: skip this repaint entirely rather
            // than double-borrow `backend` while a file dialog's nested
            // pump (further up the call stack) already holds it. Worst
            // case this frame stays stale until the dialog closes and a
            // normal redraw fires; that beats aborting the process.
            if pump_depth.get() > 0 {
                return;
            }
            let pango_ctx = pcfn::create_context(cr);
            let layout = pg::Layout::new(&pango_ctx);
            // Editor font — defaults to system monospace, size 11, but
            // is app-configurable via `Backend::set_editor_font`
            // (`ShellConfig::with_editor_font` for `ShellApp` consumers).
            // Read fresh every frame so a runtime font change takes
            // effect on the next repaint (#422). Monospace is required
            // because `draw_editor`'s scroll formula
            // (`scroll_left * char_width`) assumes uniform glyph width;
            // the untouched default resolves to the fontconfig monospace
            // alias (DejaVu Sans Mono, JetBrains Mono, etc).
            let font_desc_str = backend.borrow().editor_font_pango_string();
            let font_desc = pg::FontDescription::from_string(&font_desc_str);
            layout.set_font_description(Some(&font_desc));
            // Single-line, no wrap. Belt-and-braces over the rasterisers
            // that also call `set_width(-1)` themselves.
            layout.set_width(-1);

            // Resolve font metrics for the default font and seed the
            // backend's per-frame state.
            let metrics = pango_ctx.metrics(Some(&font_desc), None);
            let line_h = (metrics.ascent() + metrics.descent()) as f64 / pg::SCALE as f64;
            // Measure actual laid-out character width instead of
            // `approximate_char_width()` — the approximate value
            // doesn't account for font hinting and drifts over long
            // lines (e.g. 9 chars short at 500-char scroll).
            layout.set_text("0");
            let (char_w_px, _) = layout.pixel_size();
            let char_w = char_w_px as f64;

            let mut backend_mut = backend.borrow_mut();
            backend_mut.begin_frame(crate::Viewport::new(w as f32, h as f32, 1.0));
            backend_mut.set_current_line_height(line_h);
            backend_mut.set_current_char_width(char_w);
            backend_mut.set_ui_font("Sans 11");

            // Clear the whole DA with the backend's current theme bg
            // before the app's `render` runs. Without this, GTK's
            // default light-theme white shows through anywhere the
            // app doesn't explicitly paint, which clashes with the
            // primitive surface colours. Vimcode does the same as
            // step 1 of every draw flow.
            let bg = backend_mut.current_theme().background;
            cr.set_source_rgb(
                bg.r as f64 / 255.0,
                bg.g as f64 / 255.0,
                bg.b as f64 / 255.0,
            );
            cr.paint().ok();

            backend_mut.enter_frame_scope(cr, &layout, |b| {
                let _ = da; // suppress unused
                let app_ref = app.borrow();
                // Single-area runner: pass the default `AreaId`.
                app_ref.render(b, A::AreaId::default());
            });

            // After app.render: overlay selection highlight on top of the
            // rendered content (mirrors TUI's apply_selection_highlight call
            // in the terminal.draw closure).
            backend_mut.apply_selection_highlight(cr);

            backend_mut.end_frame();
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

            // ── ActivityBar keyboard focus intercept ────────────────
            //
            // When an `ActivityBar` declared `is_keyboard_focused = true`
            // during the last render pass, redirect every `KeyPressed`
            // event to it as `UiEvent::ActivityBar(id, KeyPressed { … })`
            // so the app receives typed key events through a stable channel.
            //
            // This must run *before* the accelerator dispatch below,
            // mirroring TUI's `apply_dispatch` (ActivityBar redirect) →
            // `apply_accelerators` ordering (`src/tui/backend.rs:548-549`,
            // `:583-594`) — a focused bar must be able to intercept any
            // key, even one that also happens to match a registered
            // `Global` accelerator. Reversing this priority would silently
            // diverge GTK from TUI for the same `AppLogic` (#445 review).
            //
            // This runs at the window-level `EventControllerKey` (already
            // attached to the window above), so it does NOT call
            // `grab_focus()` on any `DrawingArea` and cannot silence other
            // key controllers — the root cause of the vimcode#494 failure.
            if let UiEvent::KeyPressed {
                ref key, modifiers, ..
            } = ev
            {
                let focused_bar = backend.borrow().focused_activity_bar_id().cloned();
                if let Some(bar_id) = focused_bar {
                    let key_str = crate::primitives::activity_bar::key_to_activity_bar_string(key);
                    let bar_ev = UiEvent::ActivityBar(
                        bar_id,
                        crate::ActivityBarEvent::KeyPressed {
                            key: key_str,
                            modifiers,
                        },
                    );
                    let reaction = {
                        let mut backend_mut = backend.borrow_mut();
                        let mut app_mut = app.borrow_mut();
                        app_mut.handle(bar_ev, &mut *backend_mut)
                    };
                    apply_reaction(reaction, &da_for_redraw, &window_for_close);
                    return glib::Propagation::Stop;
                }
            }

            // ── Global accelerator dispatch (#445) ───────────────────
            //
            // Registered `Global`-scope accelerators
            // (`Backend::register_accelerator()`) must fire on this,
            // the real GTK key path — `poll_events()` /
            // `apply_accelerators()` is a dormant idle-drain seam that
            // nothing pushes real keypresses into on the
            // `run`/`run_with_shell` path. Mirror
            // `GtkBackend::apply_accelerators` here: rewrite a matching
            // `KeyPressed` into `UiEvent::Accelerator`, after the
            // ActivityBar focus intercept above (so a focused bar still
            // wins) but before the runner's own special-cased key
            // interceptions below (Ctrl-C/V/A) get a chance to consume
            // the raw `KeyPressed` instead.
            let ev = if let UiEvent::KeyPressed { key, modifiers, .. } = &ev {
                match backend.borrow().match_keypress(key, *modifiers) {
                    Some(id) => UiEvent::Accelerator(id, *modifiers),
                    None => ev,
                }
            } else {
                ev
            };

            // ── Ctrl-C interception (text selection) ────────────────
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
            } = &ev
            {
                let mut backend_mut = backend.borrow_mut();
                if backend_mut.active_text_selection().is_some() {
                    let text = backend_mut.extract_selection_text();
                    backend_mut.services().clipboard().write_text(&text);
                    backend_mut.clear_text_selection();
                    // Deliver TextCopied so the app can confirm
                    // (e.g. update a status bar message). Mirrors TUI runner.
                    let copy_ev = UiEvent::TextCopied(text);
                    let mut app_mut = app.borrow_mut();
                    let reaction = app_mut.handle(copy_ev, &mut *backend_mut);
                    drop(backend_mut);
                    apply_reaction(reaction, &da_for_redraw, &window_for_close);
                    // Suppress original Ctrl-C from reaching the app.
                    return glib::Propagation::Stop;
                }
            }

            // ── Ctrl-V interception (paste) ──────────────────────────
            //
            // GTK has no native paste signal on a bespoke `DrawingArea`
            // canvas the way a real `gtk4::Entry`/`TextView` would —
            // unlike TUI, which gets bracketed paste from crossterm for
            // free. Without this, `UiEvent::ClipboardPaste` was never
            // constructed on GTK at all, so paste silently did nothing
            // for every text-accepting primitive, including the
            // embedded terminal (quadraui#437). Reads the system
            // clipboard via the same `Clipboard` service used for the
            // Ctrl-C copy path above and delivers `ClipboardPaste` —
            // routing to the focused input is the app's job, same as
            // TUI's bracketed paste.
            if let UiEvent::KeyPressed {
                key: Key::Char('v') | Key::Char('V'),
                modifiers:
                    Modifiers {
                        ctrl: true,
                        shift: false,
                        alt: false,
                        cmd: false,
                    },
                ..
            } = &ev
            {
                let mut backend_mut = backend.borrow_mut();
                if let Some(text) = backend_mut.services().clipboard().read_text() {
                    let paste_ev = UiEvent::ClipboardPaste(text);
                    let mut app_mut = app.borrow_mut();
                    let reaction = app_mut.handle(paste_ev, &mut *backend_mut);
                    drop(backend_mut);
                    apply_reaction(reaction, &da_for_redraw, &window_for_close);
                }
                return glib::Propagation::Stop;
            }

            // ── Ctrl-A interception (select-all for text regions) ────
            //
            // Accepts 'A' (CapsLock). Guards on !shift to avoid
            // intercepting Ctrl-Shift-A. Falls through to the app when
            // no TextRegion resolves so app-level Ctrl-A handlers
            // (e.g. tree-node inline-edit select-all) are unaffected.
            //
            // Priority note: when a `TextRegion` is registered the runner
            // takes Ctrl-A; apps that register a `TextRegion` and also
            // want their own Ctrl-A handler should clear the region first.
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
            } = &ev
            {
                let handled = backend.borrow_mut().select_all_text_region();
                if handled {
                    da_for_redraw.queue_draw();
                    return glib::Propagation::Stop;
                }
            }

            let reaction = {
                let mut backend_mut = backend.borrow_mut();
                let mut app_mut = app.borrow_mut();
                app_mut.handle(ev, &mut *backend_mut)
            };
            apply_reaction(reaction, &da_for_redraw, &window_for_close);
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
                let mut backend_mut = backend.borrow_mut();
                backend_mut.clear_selection_display();
                let ev = UiEvent::DoubleClick {
                    widget: None,
                    position,
                };
                let reaction = {
                    let mut app_mut = app.borrow_mut();
                    app_mut.handle(ev, &mut *backend_mut)
                };
                apply_reaction(reaction, &da_for_redraw, &window_for_close);
                return;
            }

            let mut backend_mut = backend.borrow_mut();
            // Clear the previous selection highlight before dispatch so the
            // old highlight doesn't flicker while the new drag is starting.
            backend_mut.clear_selection_display();

            // Route through dispatch_click so text-region clicks begin a
            // TextSelection drag and scrollbar clicks begin scrollbar drags.
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
                let reaction = {
                    let mut app_mut = app.borrow_mut();
                    app_mut.handle(ev, &mut *backend_mut)
                };
                match reaction {
                    Reaction::Continue => {}
                    Reaction::Redraw => needs_redraw = true,
                    Reaction::Exit => {
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
                let reaction = {
                    let mut app_mut = app.borrow_mut();
                    app_mut.handle(ev, &mut *backend_mut)
                };
                apply_reaction(reaction, &da_for_redraw, &window_for_close);
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

            let mut needs_redraw = false;
            for ev in events {
                // Pre-process: update backend selection state so
                // `apply_selection_highlight` paints the updated range.
                if let UiEvent::TextSelectionChanged {
                    region,
                    anchor,
                    focus,
                } = &ev
                {
                    backend_mut.set_active_text_selection(region.clone(), *anchor, *focus);
                    needs_redraw = true;
                }
                let reaction = {
                    let mut app_mut = app.borrow_mut();
                    app_mut.handle(ev, &mut *backend_mut)
                };
                match reaction {
                    Reaction::Continue => {}
                    Reaction::Redraw => needs_redraw = true,
                    Reaction::Exit => {
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
            let reaction = {
                let mut backend_mut = backend.borrow_mut();
                let mut app_mut = app.borrow_mut();
                app_mut.handle(ev, &mut *backend_mut)
            };
            apply_reaction(reaction, &da_for_redraw, &window_for_close);
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
        da.connect_resize(move |_da, _width, _height| {
            // #427 re-entrancy guard — see the key-press handler above.
            if pump_depth.get() > 0 {
                return;
            }
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
                let reaction = {
                    let mut backend_mut = backend.borrow_mut();
                    let mut app_mut = app.borrow_mut();
                    app_mut.handle(ev, &mut *backend_mut)
                };
                apply_reaction(reaction, &da_for_redraw, &window_for_close);
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
        let events = backend.borrow_mut().poll_events();
        for ev in events {
            let reaction = {
                let mut backend_mut = backend.borrow_mut();
                let mut app_mut = app.borrow_mut();
                app_mut.handle(ev, &mut *backend_mut)
            };
            apply_reaction(reaction, &drain_da, &drain_window);
        }

        // Periodic tick — called after every queue drain, including
        // idle ticks where no events arrived. Lets apps drive timer
        // logic without synthetic event injection.
        let tick_reaction = {
            let mut backend_mut = backend.borrow_mut();
            let mut app_mut = app.borrow_mut();
            app_mut.tick(&mut *backend_mut)
        };
        apply_reaction(tick_reaction, &drain_da, &drain_window);

        glib::ControlFlow::Continue
    });

    window.present();
}

fn apply_reaction(reaction: Reaction, da: &DrawingArea, window: &ApplicationWindow) {
    match reaction {
        Reaction::Continue => {}
        Reaction::Redraw => da.queue_draw(),
        Reaction::Exit => window.close(),
    }
}
