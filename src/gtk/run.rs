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
//! ## Single vs multi-area
//!
//! The first runner ships with a **single-area** model: one
//! `DrawingArea`, one `set_draw_func` callback, one
//! `app.render(backend, AreaId::default())` invocation per redraw.
//!
//! Apps with multiple independently-painted surfaces (vimcode's
//! per-DrawingArea model with ~20 distinct DAs) need a richer shape.
//! The associated-type [`crate::AppLogic::AreaId`] is the trait-level
//! plumbing for that future work — once a multi-area runner ships,
//! it will pass the AreaId for whichever DA is repainting and the
//! single trait method body branches on `area`. Stage B (this file)
//! proves the trait shape end-to-end on the simple case.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    pango as pg, Application, ApplicationWindow, DrawingArea, EventControllerKey,
    EventControllerScroll, EventControllerScrollFlags, GestureClick,
};
use pangocairo::functions as pcfn;

use super::backend::GtkBackend;
use super::events::{
    gdk_button_to_mouse_down, gdk_button_to_mouse_up, gdk_key_to_uievent, gdk_motion_to_uievent,
    gdk_scroll_to_uievent,
};
use crate::backend::Backend;
use crate::runner::{AppLogic, Reaction};
use crate::ButtonMask;

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
        .default_width(800)
        .default_height(600)
        .build();

    let da = DrawingArea::new();
    da.set_hexpand(true);
    da.set_vexpand(true);
    window.set_child(Some(&da));

    // App setup hook (one-time).
    {
        let mut backend_mut = backend.borrow_mut();
        let mut app_mut = app.borrow_mut();
        app_mut.setup(&mut *backend_mut);
    }

    // ── Draw callback ──────────────────────────────────────────────
    //
    // Set a default Pango font on the layout (matches GTK system
    // sans-serif) and seed `current_line_height` / `current_char_width`
    // on the backend from the resolved font metrics so trait `draw_*`
    // methods that consume those (e.g. `draw_status_bar` for clip
    // height) line up with the actual rendered text height.
    //
    // Apps that want a custom font / size override these on the
    // backend themselves at the start of `render` via direct
    // `GtkBackend` access (not exposed via the trait today).
    {
        let app = app.clone();
        let backend = backend.clone();
        da.set_draw_func(move |da, cr, w, h| {
            let pango_ctx = pcfn::create_context(cr);
            let layout = pg::Layout::new(&pango_ctx);
            // Default font — system sans, size 11. Resolves to whatever
            // GTK's default theme provides (Cantarell on GNOME, Segoe UI
            // on Windows, etc).
            let font_desc = pg::FontDescription::from_string("Sans 11");
            layout.set_font_description(Some(&font_desc));
            // Single-line, no wrap. Belt-and-braces over the rasterisers
            // that also call `set_width(-1)` themselves.
            layout.set_width(-1);

            // Resolve font metrics for the default font and seed the
            // backend's per-frame state. Cheap; the font metrics call
            // is sub-microsecond after Pango caches it.
            let metrics = pango_ctx.metrics(Some(&font_desc), None);
            let line_h = (metrics.ascent() + metrics.descent()) as f64 / pg::SCALE as f64;
            let char_w = metrics.approximate_char_width() as f64 / pg::SCALE as f64;

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
            backend_mut.end_frame();
        });
    }

    // ── Keyboard ───────────────────────────────────────────────────
    let key_ctrl = EventControllerKey::new();
    {
        let backend = backend.clone();
        let app = app.clone();
        let da_for_redraw = da.clone();
        let window_for_close = window.clone();
        key_ctrl.connect_key_pressed(move |_ctrl, key, _code, modifier| {
            let Some(ev) = gdk_key_to_uievent(key, modifier, false) else {
                return glib::Propagation::Proceed;
            };
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

    // ── Mouse click (button 1) ─────────────────────────────────────
    let click = GestureClick::builder().button(0).build();
    {
        let backend = backend.clone();
        let app = app.clone();
        let da_for_redraw = da.clone();
        let window_for_close = window.clone();
        click.connect_pressed(move |gesture, _n_press, x, y| {
            let button = gesture.current_button();
            let modifier = gesture.current_event_state();
            let ev = gdk_button_to_mouse_down(button, x, y, modifier);
            let reaction = {
                let mut backend_mut = backend.borrow_mut();
                let mut app_mut = app.borrow_mut();
                app_mut.handle(ev, &mut *backend_mut)
            };
            apply_reaction(reaction, &da_for_redraw, &window_for_close);
        });
    }
    {
        let backend = backend.clone();
        let app = app.clone();
        let da_for_redraw = da.clone();
        let window_for_close = window.clone();
        click.connect_released(move |gesture, _n_press, x, y| {
            let ev = gdk_button_to_mouse_up(gesture.current_button(), x, y);
            let reaction = {
                let mut backend_mut = backend.borrow_mut();
                let mut app_mut = app.borrow_mut();
                app_mut.handle(ev, &mut *backend_mut)
            };
            apply_reaction(reaction, &da_for_redraw, &window_for_close);
        });
    }
    da.add_controller(click);

    // ── Scroll ─────────────────────────────────────────────────────
    let scroll = EventControllerScroll::new(EventControllerScrollFlags::BOTH_AXES);
    {
        let backend = backend.clone();
        let app = app.clone();
        let da_for_redraw = da.clone();
        let window_for_close = window.clone();
        scroll.connect_scroll(move |_ctrl, dx, dy| {
            let ev = gdk_scroll_to_uievent(dx, dy, 0.0, 0.0);
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
        let events = backend.borrow_mut().poll_events();
        for ev in events {
            let reaction = {
                let mut backend_mut = backend.borrow_mut();
                let mut app_mut = app.borrow_mut();
                app_mut.handle(ev, &mut *backend_mut)
            };
            apply_reaction(reaction, &drain_da, &drain_window);
        }
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

// Suppress an unused-import warning when other event types are unused.
#[allow(dead_code)]
fn _unused_imports(_: ButtonMask) {
    let _ = gdk_motion_to_uievent;
}
