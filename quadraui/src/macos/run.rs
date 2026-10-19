//! macOS runner — boots `NSApplication`, opens an `NSWindow`, installs
//! a custom `NSView` (`QuadraView`) that bridges AppKit's
//! responder-chain + `drawRect:` model into [`crate::AppLogic`]'s
//! `setup` / `render` / `handle` shape.
//!
//! ## Type-erasure shape
//!
//! `QuadraView` is declared via [`objc2::define_class!`], which
//! doesn't accept generic parameters. So we type-erase `A` through
//! two closures stored on the view's ivars:
//!
//! - `paint: Box<dyn Fn(Viewport, CGContextRef) + 'static>` — invoked
//!   from `drawRect:` after we resolve the viewport + grab the CG
//!   context. The closure captures `Rc<RefCell<A>>` +
//!   `Rc<RefCell<MacBackend>>`, runs
//!   `backend.enter_frame_scope(ctx, |b| app.render(b, area))`, and
//!   manages `begin_frame` / `end_frame`.
//!
//! - `handle: Rc<dyn Fn(UiEvent) -> Reaction + 'static>` — invoked by
//!   every responder override (mouse, scroll, key) after the
//!   [`super::events`] translator produces a `UiEvent`. Calls
//!   `app.handle(ev, &mut *backend)` and returns the reaction. The
//!   responder dispatches `Reaction` synchronously through
//!   [`QuadraView::apply_reaction`] — `Redraw` → `setNeedsDisplay`,
//!   `Exit` → `[NSApp terminate:]`. `Rc`, not `Box`: the
//!   same closure is also handed to [`QuadraAppDelegate`], so
//!   `applicationShouldTerminate:` (Cmd-Q / the app-menu Quit item) can
//!   run the exact `UiEvent::WindowClose` veto decision
//!   `QuadraView::windowShouldClose:` runs for the traffic-light button —
//!   see that method's doc.
//!
//!   These two hooks are **not** disjoint paths. *Two* separate routes
//!   land back in `applicationShouldTerminate:` after the app has already
//!   granted an exit:
//!
//!   1. Closing the app's one window via the traffic light returns `true`
//!      from `windowShouldClose:`, and
//!      `applicationShouldTerminateAfterLastWindowClosed: → true` carries
//!      that last-window close straight into AppKit's ordinary termination
//!      sequence.
//!   2. *Any* event whose handler returns `Reaction::Exit` (the `q`/Escape
//!      idiom nearly every `examples/common/*.rs` uses) reaches
//!      [`QuadraView::apply_reaction`] → `ReactionSink::request_exit` →
//!      `[NSApp terminate:]`, and `terminate:` is documented to always ask
//!      the delegate `applicationShouldTerminate:` first.
//!
//!   Both would otherwise dispatch `UiEvent::WindowClose` to the app for
//!   something it already decided (route 2 for an event that isn't a
//!   window close at all — and since those handlers have no `WindowClose`
//!   arm, the synthetic re-dispatch answers `Reaction::Continue` →
//!   `TerminateCancel` and the app silently refuses to quit). A shared
//!   [`ExitGate`] (constructed once in [`run_with`] alongside `handle`
//!   itself, held by both the view and the delegate) closes both routes:
//!   whoever grants the exit records it, and the delegate skips
//!   re-dispatching when it sees that record. GTK and Win avoid the same
//!   hazard structurally — `GtkSink::request_exit` uses
//!   `window.destroy()` rather than `close()`, and Win's uses
//!   `DestroyWindow` rather than resending `WM_CLOSE` — but AppKit offers
//!   no "terminate without asking the delegate" entry point, so macOS
//!   guards it with state instead. See [`ExitGate`]'s doc.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::sys::CGContextRef;
// objc2 0.6 removed `declare_class!` in favour of `define_class!`
// (superclass/mutability move from a `ClassType` impl block onto struct
// attributes), renamed `DeclaredClass` to `DefinedClass`, and deprecated
// `msg_send_id!` in favour of `msg_send!` (which now performs the same
// `Retained` conversion itself — see that macro's own doc).
use objc2::encode::{Encoding, RefEncode};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, sel, ClassType, DefinedClass, MainThreadOnly, Message};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate,
    NSApplicationTerminateReply, NSBackingStoreType, NSDragOperation, NSDraggingInfo, NSEvent,
    NSEventMask, NSGraphicsContext, NSPasteboardTypeFileURL, NSView,
    NSViewFrameDidChangeNotification, NSWindow, NSWindowDelegate,
    NSWindowDidChangeBackingPropertiesNotification, NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSDate, NSDefaultRunLoopMode, NSNotification, NSNotificationCenter,
    NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSTimer, NSURL,
};

use super::backend::MacBackend;
use super::events::{
    ns_files_dropped, ns_key_to_uievent, ns_mouse_down, ns_mouse_moved, ns_mouse_up, ns_scroll,
};
use super::text::make_font;
// Reuses the tray icon's AppKit-decoder for the Dock/app-switcher
// icon — see `decode_ns_image`'s doc for why this is a shared pipeline
// rather than a second decoder.
use super::tray::decode_ns_image;
use crate::backend::Backend;
use crate::event::Viewport;
use crate::primitives::image::ImageSource;
use crate::runner::{AppLogic, Reaction};
use crate::runtime::{self, ReactionSink, ResizeDebouncer, RESIZE_SETTLE};
// Re-exported (not just imported) so `macos::testing` — and any other
// in-crate caller that historically reached `EventOutcome` through this
// module — keeps working unchanged after the type moved to
// `crate::runtime`.
pub(crate) use crate::runtime::EventOutcome;
use crate::{ButtonMask, UiEvent};

/// Opaque stand-in for the C type `CGContext`. We only ever hold a
/// `*mut OpaqueCGContext`, which we then cast to `core-graphics`'
/// `CGContextRef`. The custom `RefEncode` impl is what makes
/// `msg_send![gctx, CGContext]` accept the return type — objc2's
/// debug-mode encoding check matches `^{CGContext=}` exactly, so
/// `*mut c_void` (encoded as `^v`) panics at runtime.
#[repr(C)]
struct OpaqueCGContext {
    _unused: [u8; 0],
}

unsafe impl RefEncode for OpaqueCGContext {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("CGContext", &[]));
}

// Direct CoreGraphics bindings. `core-graphics::context` exposes safe
// wrappers that take ownership of a `CGContextRef`; we need to *borrow*
// the pointer AppKit hands us inside `drawRect:` so we drop to FFI.
// The CoreGraphics framework is linked by the `core-graphics` crate so
// no `#[link]` attribute is needed here.
extern "C" {
    fn CGContextSetRGBFillColor(
        c: CGContextRef,
        red: core_graphics::base::CGFloat,
        green: core_graphics::base::CGFloat,
        blue: core_graphics::base::CGFloat,
        alpha: core_graphics::base::CGFloat,
    );
    fn CGContextFillRect(c: CGContextRef, rect: CGRect);
}

/// Fill `rect` with `bg` — the whole-frame backdrop [`QuadraView::draw_rect`]
/// paints before delegating to the app's own render closure.
/// Extracted so it's exercised by a headless [`super::headless::BitmapSurface`]
/// test with no live `NSView`/`NSWindow`.
///
/// # Safety
///
/// `cg_ref` must be a non-null `CGContextRef` borrowed for the duration
/// of this call.
pub(crate) unsafe fn fill_frame_background(
    cg_ref: CGContextRef,
    rect: CGRect,
    bg: crate::types::Color,
) {
    let (r, g, b, a) = super::cg::color_to_cg(bg);
    CGContextSetRGBFillColor(cg_ref, r, g, b, a);
    CGContextFillRect(cg_ref, rect);
}

/// Type-erased closures the view invokes from its responder + draw
/// callbacks. Built once per [`run`] call from the concrete `A:
/// AppLogic`; from `define_class!`'s perspective they're just two
/// `Box<dyn Fn>` smart pointers.
type PaintFn = Box<dyn Fn(Viewport, CGContextRef) + 'static>;
/// `Rc`, not `Box` — the `handle` closure is shared between
/// `QuadraView` (every responder-method dispatch) and `QuadraAppDelegate`
/// (`applicationShouldTerminate:`), so both can run the same
/// `UiEvent::WindowClose` veto decision. See this file's module doc.
type HandleFn = Rc<dyn Fn(UiEvent) -> Reaction + 'static>;
/// Bridges `QuadraView::dispatch_tick` to `AppLogic::tick`
/// — the third leg of the type-erased-closure trio, alongside `paint`/
/// `handle` above. Fired by [`MacBackend::request_frame_in`]'s
/// scheduled-wake timer via [`MacBackend::set_tick_callback`] *and* by
/// [`run_with`]'s repeating `idlePollTick:` timer (quadraui#940 — see
/// that method's doc for why the wake-only path alone left deferred
/// host work stranded).
type TickFn = Box<dyn Fn() -> Reaction + 'static>;

// `EventOutcome` — what the caller should do after [`dispatch_event`]
// handles one event — is defined once in `crate::runtime` and shared by
// every backend runner; imported at the top of this file
// (re-exported so `macos::testing` keeps reaching it through this path).

/// Dispatch one already-translated [`UiEvent`] through the app, applying
/// the shared runner pre-processing pipeline first — same funnel both the
/// live `QuadraView` responder methods (via [`run`]'s `handle` closure)
/// and [`super::testing::MacDriver`] route through, so a
/// test exercises the exact pre-processing a real keypress/click gets.
///
/// Pre-processing handled here, in order:
/// - Caret-blink bump: any `KeyPressed`/`CharTyped` makes the caret solid
///   for ~500ms (`caret_visible`/`caret_pause`), matching AppKit's
///   text-field typing convention. macOS-only — no other backend paints
///   a blinking caret through this mechanism.
/// - Modal click dispatch: `MouseDown` routes through
///   [`crate::dispatch::dispatch_click`] so the backend's `ModalStack`
///   arbitrates first — a click inside an open modal is tagged with the
///   modal's `WidgetId`, and a click outside every modal dismisses the
///   topmost instead of falling through to the widget underneath.
///   Scroll surfaces are not tracked by `MacBackend` yet (see its
///   `capabilities` doc), hence the empty slice. Mirrors
///   `TuiBackend::apply_dispatch` and `gtk::run`'s `connect_pressed`
///   closure. Each resulting event (e.g. dismiss emits `MouseDown` +
///   `Palette(Closed)`) is routed through [`crate::runtime::preprocess_event`]
///   (quadraui#813) — the same shared pipeline every other backend's
///   `MouseDown` routing uses — and the outcomes are folded the way
///   `gtk::run`'s click loop folds them: `Exit` wins immediately, else
///   `Redraw` wins over `Continue`.
/// - `MouseMoved`: routes through
///   [`crate::dispatch::dispatch_mouse_drag`] so an in-progress
///   `TextSelection`/scrollbar/split-divider drag (armed by the
///   `MouseDown` branch above) emits its synthetic event alongside the
///   plain `MouseMoved`, mirroring `gtk::run`'s motion controller /
///   `win::run::route_mouse_move`.
/// - `MouseUp`: routes through [`crate::dispatch::dispatch_mouse_up`]
///   so an in-progress drag ends cleanly.
///
/// Everything else — double-click folding, ActivityBar keyboard-focus
/// redirect, Tab/Shift+Tab focus cycling, global accelerator
/// rewrite, Ctrl-C copy, Cmd-V/Cmd-Shift-V
/// paste, middle-click PRIMARY-selection paste (a no-op on macOS — see
/// [`crate::backend::Clipboard::read_primary_selection`]'s default),
/// Ctrl-A select-all, selection-display clearing, `TextSelectionChanged`
/// — lives in [`crate::runtime::preprocess_event`], shared
/// with TUI/GTK/Windows; see that function's doc for the exact priority
/// order and `MacBackend`'s `PreprocessBackend` impl for macOS's one
/// documented difference: pasting on Cmd-V rather than Ctrl-V, the
/// platform convention. Ctrl-C stays literal Ctrl everywhere (not
/// remapped to Cmd the way [`macos_universal_binding_modifiers`] rewrites
/// *registered* accelerators) — a deliberate cross-platform choice per
/// #803's brief, not an oversight.
///
/// Anything not matched above falls through to `app.handle` unchanged.
pub(crate) fn dispatch_event<A: AppLogic>(
    event: UiEvent,
    backend: &mut MacBackend,
    app: &mut A,
    caret_visible: &Rc<Cell<bool>>,
    caret_pause: &Rc<Cell<std::time::Instant>>,
) -> EventOutcome {
    if matches!(event, UiEvent::KeyPressed { .. } | UiEvent::CharTyped(_)) {
        caret_visible.set(true);
        caret_pause.set(std::time::Instant::now() + std::time::Duration::from_millis(500));
    }

    // Modal click dispatch: every mouse-down consults the
    // `ModalStack` via the shared dispatch layer before the app sees it,
    // so a click inside an open dialog stays inside the dialog and a
    // click outside dismisses it — instead of falling straight through
    // to whatever widget is underneath. `route_pointer` may
    // return 0..n events (e.g. dismiss emits `MouseDown` +
    // `Palette(Closed)`); each goes through the shared `preprocess_event`
    // (which folds double-clicks, clears the selection display, etc. —
    // see this function's doc), and the outcomes are folded the way
    // `gtk::run`'s click loop folds them: `Exit` wins immediately, else
    // `Redraw` wins over `Continue`.
    if let UiEvent::MouseDown { .. } = &event {
        let dispatched = {
            let stack_rc = backend.modal_stack_handle();
            let drag_rc = backend.drag_state_handle();
            let stack = stack_rc.borrow();
            let mut drag = drag_rc.borrow_mut();
            crate::dispatch::route_pointer(
                &stack,
                &mut drag,
                backend.text_selection_state_mut(),
                event,
            )
        };
        let mut outcome = EventOutcome::Continue;
        for ev in dispatched {
            let step = runtime::preprocess_event(ev, backend, app);
            if matches!(step, EventOutcome::Exit) {
                return EventOutcome::Exit;
            }
            // Keep the *earliest* `RedrawAfter` deadline seen
            // in this batch rather than the first non-`Continue` one — see
            // `EventOutcome::merge`'s doc for why that distinction matters.
            outcome = outcome.merge(step);
        }
        return outcome;
    }

    // Route a `MouseMoved` through `route_pointer` so
    // an in-progress `TextSelection`/scrollbar/split-divider drag (armed
    // by the `MouseDown` branch above) emits its synthetic event
    // (`TextSelectionChanged`/`ScrollOffsetChanged`/`SplitDividerDragged`)
    // alongside the plain `MouseMoved` — mirrors gtk::run's motion
    // controller / win::run::route_mouse_move. The plain `MouseMoved`
    // goes straight to `app.handle` (nothing in the shared pipeline
    // matches it); the extra event, if any, recurses into `dispatch_event`
    // so its own pre-processing (`TextSelectionChanged`, via
    // `preprocess_event`) applies uniformly — safe because
    // `dispatch_mouse_drag` never emits a second `MouseMoved`, so this
    // can't loop.
    if let UiEvent::MouseMoved { .. } = &event {
        let events = {
            let stack_rc = backend.modal_stack_handle();
            let drag_rc = backend.drag_state_handle();
            let stack = stack_rc.borrow();
            let mut drag = drag_rc.borrow_mut();
            crate::dispatch::route_pointer(
                &stack,
                &mut drag,
                backend.text_selection_state_mut(),
                event,
            )
        };
        let mut outcome = EventOutcome::Continue;
        for ev in events {
            let step = if matches!(ev, UiEvent::MouseMoved { .. }) {
                app.handle(ev, backend).into()
            } else {
                dispatch_event(ev, backend, app, caret_visible, caret_pause)
            };
            if matches!(step, EventOutcome::Exit) {
                return EventOutcome::Exit;
            }
            // Earliest `RedrawAfter` wins — see
            // `EventOutcome::merge`'s doc.
            outcome = outcome.merge(step);
        }
        return outcome;
    }

    // Route a `MouseUp` through `route_pointer` so an
    // in-progress drag ends cleanly — mirrors gtk::run's
    // `connect_released` / win::run::route_mouse_up. `dispatch_mouse_up`
    // always returns exactly one `MouseUp` (see its doc), rewritten with
    // `widget` when the release lands inside an open modal.
    if let UiEvent::MouseUp { .. } = &event {
        let ev = {
            let stack_rc = backend.modal_stack_handle();
            let drag_rc = backend.drag_state_handle();
            let stack = stack_rc.borrow();
            let mut drag = drag_rc.borrow_mut();
            crate::dispatch::route_pointer(
                &stack,
                &mut drag,
                backend.text_selection_state_mut(),
                event,
            )
            .into_iter()
            .next()
            .expect("dispatch_mouse_up always returns exactly one MouseUp")
        };
        return app.handle(ev, backend).into();
    }

    // Everything else — double-click folding, ActivityBar redirect,
    // Tab/Shift+Tab focus cycling, accelerators,
    // Ctrl-C/Cmd-V/middle-click/Ctrl-A, selection-display
    // clearing, TextSelectionChanged — is the shared pipeline. See this
    // function's doc.
    runtime::preprocess_event(event, backend, app)
}

/// Render one frame: `begin_frame` + [`MacBackend::enter_frame_scope`] +
/// `end_frame` — the exact body `run`'s `paint` closure used to run
/// inline, extracted so it never depends on a live `NSView`, only a
/// [`Viewport`] + a borrowed `CGContextRef`. Shared by the live runner and
/// [`super::testing::MacDriver`] — mirrors
/// [`crate::gtk::run::render_frame`].
///
/// # Panic safety
///
/// `drawRect:` (below) runs this from inside objc2's `define_class!`
/// dispatch trampoline — a C ABI callback that cannot unwind — so
/// `app.render` (arbitrary consumer code, or a `MacBackend` rasteriser's
/// reachable `todo!()`, the exact shape of the vimcode#896 incident that
/// prompted quadraui#922) is wrapped in its own `catch_unwind` instead of
/// being allowed to unwind through that boundary into `abort()`.
///
/// This catches specifically *inside* the closure passed to
/// [`MacBackend::enter_frame_scope`], not around the call to
/// `enter_frame_scope` itself: `enter_frame_scope`'s own cleanup (restoring
/// `current_cg_ptr`, stopping text-run recording) runs as plain code after
/// its closure argument returns, with no `Drop` guard backing it — if the
/// closure unwound instead of returning, that cleanup would never run and
/// `current_cg_ptr` would stay stuck non-null past this frame. Catching
/// here means the closure always returns normally (the panic is already
/// resolved to `Err` before the closure's body ends), so
/// `enter_frame_scope` never sees an unwind and its cleanup runs exactly
/// as it would for a successful frame — this is the "frame scope opened
/// by `enter_frame_scope` must be closed" half of #922's requirements.
/// `draw_rect`'s own outer `catch_unwind` around this whole function is
/// separate defense-in-depth for the code *outside* this closure (the
/// `tab_stops` call below, `begin_frame`/`end_frame` themselves) — see
/// that method's doc.
pub(crate) fn render_frame<A: AppLogic>(
    backend: &mut MacBackend,
    app: &A,
    viewport: Viewport,
    ctx: CGContextRef,
) {
    backend.begin_frame(viewport);
    // Resolve the currently-focused widget's rect (if any)
    // from this frame's tab stops before entering the frame scope.
    let focus_ring_rect =
        runtime::focused_stop_rect(backend, app, <A as AppLogic>::AreaId::default());
    backend.enter_frame_scope(ctx, |b| {
        let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            app.render(&mut *b, <A as AppLogic>::AreaId::default());
        }));
        if let Err(payload) = rendered {
            crate::desktop::report_caught_panic_once(
                "macos::run::render_frame (app.render)",
                payload.as_ref(),
            );
            // Skip the rest of this frame — the focus ring / selection
            // highlight below both read state a panicking `render` may
            // have left inconsistent.
            return;
        }
        // After app.render: paint the focus-ring convention, same
        // reasoning as `apply_selection_highlight` below for staying
        // inside this closure — `MacBackend::current_cg` is only
        // non-null here.
        if let Some(rect) = focus_ring_rect {
            b.draw_focus_ring(rect);
        }
        // After app.render: overlay the text-selection highlight on top of
        // the rendered content — mirrors gtk::run::render_frame's
        // `apply_selection_highlight(cr)` call and win::run::render_frame's
        // `apply_selection_highlight()` call, both made at this same point
        // in the frame. Stays inside this closure (unlike GTK/Win, which
        // call it just after their own render_frame's paint step) because
        // `MacBackend::current_cg` — which the highlight paint needs — is
        // only non-null inside `enter_frame_scope`.
        b.apply_selection_highlight();
    });
    backend.end_frame();
}

/// `QuadraView`'s per-instance state. `last_viewport` is retained for
/// diagnostics + future paint↔click harness work; `paint` / `handle`
/// are the AppLogic bridge.
pub(crate) struct QuadraViewIvars {
    last_viewport: Cell<Viewport>,
    /// Concrete (not type-erased) — unlike `paint`/`handle`, `MacBackend`
    /// doesn't depend on the app's `A: AppLogic`, so responder methods
    /// can reach it directly. Used by `mouseDown:` to stash the
    /// raw press `NSEvent` for `Backend::begin_window_drag` before the
    /// press is translated to a portable `UiEvent`.
    backend: Rc<RefCell<MacBackend>>,
    paint: PaintFn,
    handle: HandleFn,
    /// See [`TickFn`]'s doc.
    tick: TickFn,
    /// Resize-settle debounce state — see
    /// [`crate::runtime::ResizeDebouncer`]'s doc. `view_frame_did_change`
    /// calls `note()` on every AppKit frame-change notification;
    /// `resize_debounce_fired` calls `take()` once `resize_timer` proves
    /// the drag has settled.
    resize_debouncer: RefCell<ResizeDebouncer>,
    /// The in-flight one-shot resize-settle timer, if any — invalidated
    /// and replaced on every `view_frame_did_change` so a live drag keeps
    /// pushing the deadline out, mirroring `SetTimer`'s
    /// replace-if-already-armed semantics on Windows (`win::run`'s
    /// `WM_SIZE`/`WM_TIMER` arms) and GTK's cancel-and-reschedule
    /// `glib::SourceId`.
    resize_timer: RefCell<Option<Retained<NSTimer>>>,
    /// One-shot double-dispatch guard — shared
    /// with [`QuadraAppDelegateIvars::exit_gate`] via the same `Rc`
    /// (constructed once in [`run_with`]). Marked granted by
    /// [`QuadraView::evaluate_window_should_close`] the moment it resolves
    /// a close to `Reaction::Exit`, and by this view's
    /// [`ReactionSink::request_exit`] before it calls `[NSApp terminate:]`;
    /// checked by [`QuadraAppDelegate::evaluate_should_terminate`] so
    /// neither route re-dispatches `UiEvent::WindowClose` for an exit the
    /// app already granted. See [`ExitGate`]'s doc for both sequences.
    exit_gate: Rc<ExitGate>,
}

define_class!(
    /// Quadraui's custom `NSView`. `drawRect:` resolves viewport +
    /// CG context, paints the frame background,
    /// then delegates to the stored `paint` closure so the active
    /// [`AppLogic`] can render on top. Responder methods translate
    /// `NSEvent` → [`UiEvent`] and route through the stored `handle`
    /// closure for `AppLogic::handle`.
    //
    // SAFETY:
    // - NSView is documented to be subclassable for custom drawing.
    // - MainThreadOnly: AppKit views must be created + used on the
    //   main thread; `QuadraView::new` enforces this via the
    //   `MainThreadMarker` argument.
    // - `QuadraView` doesn't implement Drop — its ivars hold owned
    //   `Box<dyn Fn>` smart pointers that drop cleanly when the
    //   class instance is finalized by the Obj-C runtime.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "QuadraUiView"]
    #[ivars = QuadraViewIvars]
    pub(crate) struct QuadraView;

    impl QuadraView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let bounds = self.bounds();
            let scale = self
                .window()
                .map(|w| w.backingScaleFactor())
                .unwrap_or(1.0);

            let viewport = Viewport::new(
                bounds.size.width as f32,
                bounds.size.height as f32,
                scale as f32,
            );
            self.ivars().last_viewport.set(viewport);

            // `currentContext` is a safe method. `drawRect:` is always
            // invoked inside a valid graphics scope, so it always returns
            // `Some` here.
            let Some(gctx) = NSGraphicsContext::currentContext() else {
                return;
            };
            // Custom opaque return type makes objc2's encoding check
            // accept the `CGContext` selector — see the long-form
            // explanation on [`OpaqueCGContext`].
            let cg_opaque: *mut OpaqueCGContext = unsafe { msg_send![&*gctx, CGContext] };
            if cg_opaque.is_null() {
                return;
            }
            let cg_ref: CGContextRef = cg_opaque.cast();

            // Convert NSRect → core_graphics::CGRect (layout-compatible
            // but distinct Rust types).
            let rect = CGRect::new(
                &CGPoint::new(bounds.origin.x, bounds.origin.y),
                &CGSize::new(bounds.size.width, bounds.size.height),
            );

            // Background fill so the area between rasterised chrome
            // (e.g. tab_bar at the top + status_bar at the bottom) has
            // a consistent backdrop until all content is rasterised.
            // Use the current theme's background instead of a fixed shade
            // so unpainted areas match the theme.
            let bg = self.ivars().backend.borrow().current_theme().background;
            // SAFETY: `cg_ref` is a non-null `CGContextRef` borrowed
            // for the duration of this call.
            unsafe {
                fill_frame_background(cg_ref, rect, bg);
            }

            // Now run the app's render via the stored closure.
            // `paint` is responsible for `begin_frame` / frame-scope /
            // `end_frame` orchestration so this method doesn't need
            // to know anything about the concrete `A`.
            //
            // `drawRect:` is objc2's `define_class!`-generated
            // dispatch trampoline invoking this method directly — a C ABI
            // callback that cannot unwind. `catch_unwind` here is the
            // outermost guard for everything `paint` does (the primary
            // guard, around `app.render` specifically, lives inside
            // `render_frame` itself — see its doc for why that inner
            // placement is what keeps `MacBackend::enter_frame_scope`'s
            // frame-scope bracket balanced); this one is defense-in-depth
            // for anything else `paint` runs (queued-event draining,
            // `tab_stops`, `begin_frame`/`end_frame`) so a panic there
            // degrades to "this frame is blank" instead of aborting the
            // host process.
            if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (self.ivars().paint)(viewport, cg_ref);
            })) {
                crate::desktop::report_caught_panic_once(
                    "macos::run::QuadraView::draw_rect (paint)",
                    payload.as_ref(),
                );
            }
        }

        /// Top-left origin to match TUI + GTK conventions.
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        /// Required so AppKit routes `keyDown:` here.
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        // ── Mouse press / release ────────────────────────────────

        #[unsafe(method(mouseDown:))]
        fn objc_mouse_down(&self, event: &NSEvent) {
            // Stash the raw press before it's translated to a
            // portable `UiEvent`, so `Backend::begin_window_drag` (called
            // later, from `AppLogic::handle`, if the app decides this
            // press landed in its CSD titlebar band) has the
            // *originating* `NSEvent` `performWindowDragWithEvent:`
            // requires — mirrors `gtk::run`'s `GestureClick::connect_pressed`
            // stashing the raw GDK press context the same way.
            //
            // `ClassType::retain` (safe: `NSEvent` is immutable, so it
            // satisfies `IsRetainable`) bumps the refcount and hands back
            // an owned `Retained<NSEvent>` that can outlive this call,
            // unlike the borrowed `event: &NSEvent` parameter.
            let retained: Retained<NSEvent> = event.retain();
            self.ivars().backend.borrow_mut().stash_window_press(retained);

            let (x, y, flags) = self.locate(event);
            let button = event.buttonNumber() as i64;
            self.dispatch(ns_mouse_down(button, x, y, flags));
        }

        #[unsafe(method(rightMouseDown:))]
        fn objc_right_mouse_down(&self, event: &NSEvent) {
            let (x, y, flags) = self.locate(event);
            let button = event.buttonNumber() as i64;
            self.dispatch(ns_mouse_down(button, x, y, flags));
        }

        #[unsafe(method(otherMouseDown:))]
        fn objc_other_mouse_down(&self, event: &NSEvent) {
            let (x, y, flags) = self.locate(event);
            let button = event.buttonNumber() as i64;
            self.dispatch(ns_mouse_down(button, x, y, flags));
        }

        #[unsafe(method(mouseUp:))]
        fn objc_mouse_up(&self, event: &NSEvent) {
            let (x, y, _flags) = self.locate(event);
            let button = event.buttonNumber() as i64;
            self.dispatch(ns_mouse_up(button, x, y));
        }

        #[unsafe(method(rightMouseUp:))]
        fn objc_right_mouse_up(&self, event: &NSEvent) {
            let (x, y, _flags) = self.locate(event);
            let button = event.buttonNumber() as i64;
            self.dispatch(ns_mouse_up(button, x, y));
        }

        #[unsafe(method(otherMouseUp:))]
        fn objc_other_mouse_up(&self, event: &NSEvent) {
            let (x, y, _flags) = self.locate(event);
            let button = event.buttonNumber() as i64;
            self.dispatch(ns_mouse_up(button, x, y));
        }

        // ── Mouse move / drag ────────────────────────────────────

        #[unsafe(method(mouseMoved:))]
        fn objc_mouse_moved(&self, event: &NSEvent) {
            let (x, y, _flags) = self.locate(event);
            self.dispatch(ns_mouse_moved(x, y, ButtonMask::default()));
        }

        #[unsafe(method(mouseDragged:))]
        fn objc_mouse_dragged(&self, event: &NSEvent) {
            let (x, y, _flags) = self.locate(event);
            self.dispatch(ns_mouse_moved(
                x,
                y,
                ButtonMask {
                    left: true,
                    ..Default::default()
                },
            ));
        }

        #[unsafe(method(rightMouseDragged:))]
        fn objc_right_mouse_dragged(&self, event: &NSEvent) {
            let (x, y, _flags) = self.locate(event);
            self.dispatch(ns_mouse_moved(
                x,
                y,
                ButtonMask {
                    right: true,
                    ..Default::default()
                },
            ));
        }

        #[unsafe(method(otherMouseDragged:))]
        fn objc_other_mouse_dragged(&self, event: &NSEvent) {
            let (x, y, _flags) = self.locate(event);
            self.dispatch(ns_mouse_moved(
                x,
                y,
                ButtonMask {
                    middle: true,
                    ..Default::default()
                },
            ));
        }

        // ── Scroll wheel + key down ──────────────────────────────

        #[unsafe(method(scrollWheel:))]
        fn objc_scroll_wheel(&self, event: &NSEvent) {
            let (x, y, _flags) = self.locate(event);
            // SAFETY: scrollingDeltaX/Y are safe on a scroll event.
            let dx = event.scrollingDeltaX();
            let dy = event.scrollingDeltaY();
            self.dispatch(ns_scroll(dx, dy, x, y));
        }

        #[unsafe(method(keyDown:))]
        fn objc_key_down(&self, event: &NSEvent) {
            let flags = event.modifierFlags().0;
            let key_code = event.keyCode();
            let repeat = event.isARepeat();
            let chars_ns = event.characters();
            let chars_str = chars_ns.as_ref().map(|s| s.to_string());
            if let Some(ev) = ns_key_to_uievent(chars_str.as_deref(), key_code, flags, repeat) {
                self.dispatch(ev);
            }
        }

        // ── Window resize ────────────────────────────────────────

        /// Registered (in [`run`]) as the observer for
        /// `NSViewFrameDidChangeNotification`, which fires whenever this
        /// view's `frame` changes — live window resize, programmatic
        /// resize, and the split-second the window first opens. Compares
        /// against `last_viewport` (also updated here) so a notification
        /// that doesn't actually change the size — AppKit can post one
        /// on origin-only moves — doesn't spam the debouncer with a
        /// same-size note.
        ///
        /// The `WindowResized` *dispatch* itself is debounced
        /// (quadraui#780, mirroring TUI/GTK's `RESIZE_SETTLE` — see
        /// `crate::runtime::RESIZE_SETTLE`'s doc for the full
        /// rationale): a live window drag delivers a burst of these
        /// notifications, so this stashes the latest size in
        /// `resize_debouncer` and (re)arms a one-shot `NSTimer` rather
        /// than dispatching immediately. Painting stays live regardless
        /// — `drawRect:` re-resolves the view's real `bounds` every
        /// frame independent of whether the debounced event has fired.
        #[unsafe(method(viewFrameDidChange:))]
        fn view_frame_did_change(&self, _note: &NSNotification) {
            let bounds = self.bounds();
            let scale = self
                .window()
                .map(|w| w.backingScaleFactor())
                .unwrap_or(1.0);
            let viewport = Viewport::new(
                bounds.size.width as f32,
                bounds.size.height as f32,
                scale as f32,
            );
            if self.ivars().last_viewport.get() == viewport {
                return;
            }
            self.ivars().last_viewport.set(viewport);
            self.ivars().resize_debouncer.borrow_mut().note(viewport);

            // Cancel any not-yet-fired settle timer from an earlier
            // notification in this same burst, then arm a fresh one —
            // the drag keeps pushing the deadline out until it settles.
            if let Some(old) = self.ivars().resize_timer.borrow_mut().take() {
                old.invalidate();
            }
            // SAFETY: `self` is a valid, live `QuadraView` (an `NSObject`
            // subclass via `NSView`/`NSResponder`) for at least as long as
            // this method call — same no-op upcast [`run`] uses to
            // register the `viewFrameDidChange:` observer itself.
            let self_obj: &AnyObject = unsafe { &*(self as *const Self as *const AnyObject) };
            let timer = unsafe {
                NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                    RESIZE_SETTLE.as_secs_f64(),
                    self_obj,
                    sel!(resizeDebounceFired:),
                    None,
                    false,
                )
            };
            self.ivars().resize_timer.borrow_mut().replace(timer);
        }

        /// Fires once `RESIZE_SETTLE` after the last `viewFrameDidChange:`
        /// in a burst — see that method's doc. Dispatches the coalesced
        /// size, if any, as a single `WindowResized`.
        #[unsafe(method(resizeDebounceFired:))]
        fn resize_debounce_fired(&self, _timer: &NSTimer) {
            self.ivars().resize_timer.borrow_mut().take();
            if let Some(viewport) = self.ivars().resize_debouncer.borrow_mut().take() {
                self.dispatch(UiEvent::WindowResized { viewport });
            }
        }

        // ── Idle-poll fallback tick ──────────────────────────────────
        //
        // Target of [`run_with`]'s repeating `NSTimer`, armed at
        // `crate::runtime::IDLE_POLL_CEILING` (250ms) — mirrors GTK's
        // `glib::timeout_add_local(IDLE_POLL_CEILING, ...)` idle safety
        // net (`gtk::run::run_with`'s `drain_and_tick` install site).
        // Before this, macOS only ever called `AppLogic::tick` off a
        // native event or an explicit `Backend::request_frame_in`
        // deadline — a host that queued deferred work (a file dialog
        // request, a dirty-frame flag) with no further input and no
        // explicit re-arm had that work stranded forever. This timer is
        // the coarse backstop: whatever the app didn't ask to be woken
        // for gets picked up within a quarter second regardless.
        #[unsafe(method(idlePollTick:))]
        fn idle_poll_tick(&self, _timer: &NSTimer) {
            self.dispatch_tick();
        }

        // ── HiDPI runtime change ────────────────────────────────────
        //
        // Registered (in [`run`]) as the observer for
        // `NSWindowDidChangeBackingPropertiesNotification`, which AppKit
        // posts on the *window* whenever its `backingScaleFactor`
        // changes — a monitor drag across a DPI boundary, or the
        // system/external-display scaling setting changing live. Unlike
        // `viewFrameDidChange:` this needs no debounce: a DPI change
        // doesn't arrive as a rapid-fire burst the way a live resize
        // drag does. `drawRect:` already re-reads `backingScaleFactor()`
        // fresh on every paint (see its doc), so dispatching this and
        // letting the shared `crate::runtime::preprocess_event` force a
        // redraw is sufficient to re-measure — no separate
        // cached-scale field to update here the way `WinBackend`/
        // `GtkBackend` need.
        #[unsafe(method(windowDidChangeBackingProperties:))]
        fn window_did_change_backing_properties(&self, _note: &NSNotification) {
            let scale = self
                .window()
                .map(|w| w.backingScaleFactor())
                .unwrap_or(1.0);
            self.dispatch(UiEvent::DpiChanged(scale as f32));
        }

        // ── OS file drop ─────────────────────────────────────────────
        //
        // `NSDraggingDestination`'s methods are informal/optional
        // protocol overrides on `NSView` — AppKit dispatches them by
        // selector once `registerForDraggedTypes:` (called in [`run`])
        // opts this view in, the same way `mouseDown:`/`scrollWheel:`
        // above are plain `NSResponder` overrides with no separate
        // protocol `impl` block needed.

        /// Accept a drag iff it carries at least one file-URL pasteboard
        /// item — see [`Self::drag_operation`].
        ///
        /// The answer itself lives in the plain `impl QuadraView` block
        /// below rather than here, because `define_class!` rewrites the
        /// signatures it generates (an ObjC `_cmd: Sel` is threaded in as
        /// the first argument, and a `-> bool` becomes `-> Bool`), so a
        /// method defined *inside* the macro is awkward to call from a
        /// sibling method inside it. Shared logic goes in the ordinary
        /// impl; the macro arms stay thin selector adapters.
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(
            &self,
            sender: &ProtocolObject<dyn NSDraggingInfo>,
        ) -> NSDragOperation {
            self.drag_operation(sender)
        }

        /// Same accept/reject answer as `draggingEntered:`, re-evaluated
        /// as AppKit calls this continuously while the drag hovers.
        #[unsafe(method(draggingUpdated:))]
        fn dragging_updated(
            &self,
            sender: &ProtocolObject<dyn NSDraggingInfo>,
        ) -> NSDragOperation {
            self.drag_operation(sender)
        }

        /// The user released the mouse button over this view — decode
        /// the dropped paths + drop location and dispatch
        /// `UiEvent::FilesDropped`. Returns `false` (declining the
        /// operation) if the drag turns out to carry no file paths after
        /// all, mirroring `draggingEntered:`'s reject case.
        ///
        /// Written as a single tail `if`/`else` rather than an early
        /// `return false`: `define_class!` declares the generated method
        /// as returning ObjC's `Bool` and converts the *tail* expression
        /// for us, so an early `return` of a Rust `bool` would not
        /// type-check.
        #[unsafe(method(performDragOperation:))]
        fn perform_drag_operation(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            let paths = self.dropped_paths_guarded(sender);
            if paths.is_empty() {
                false
            } else {
                let loc = sender.draggingLocation();
                let view_pt = self.convertPoint_fromView(loc, None);
                self.dispatch(ns_files_dropped(paths, view_pt.x, view_pt.y));
                true
            }
        }
    }

    // Needed to adopt `NSWindowDelegate` below — `QuadraView` inherits
    // from `NSView` (which already conforms), but that conformance
    // doesn't carry over to this distinct Rust type without saying so
    // explicitly (the same requirement `QuadraAppDelegate` — a bare
    // `NSObject` subclass — already states below).
    unsafe impl NSObjectProtocol for QuadraView {}

    // ── Window close ─────────────────────────────────────────────────
    //
    // Before this, the red traffic-light button, Cmd-W, and the
    // AppKit-native "Close" menu item all closed the window directly —
    // no `UiEvent` in between — so an app's unsaved-changes guard,
    // written against `UiEvent::WindowClose`'s documented "required on
    // every windowed backend" contract (D-010), simply never ran on
    // macOS. `windowShouldClose:` fires before the window is torn down
    // and lets the delegate veto it by returning `false` — the same
    // "ask first" hook GTK's `connect_close_request` and Win's
    // `WM_CLOSE` already use (see `gtk::run`'s window-close wiring and
    // `win::run::wnd_proc`'s `WM_CLOSE` arm).
    //
    // `run_with` makes the view its own window delegate
    // (`window.setDelegate`), so this reuses the exact `handle` closure
    // (now `Rc`-shared, see `HandleFn`'s doc) every mouse/key/scroll
    // responder above dispatches through — a real `AppLogic::handle`
    // call, not a synthetic stand-in.
    //
    // When this is the app's last open window, returning `true` here
    // does not end the story: AppKit carries the close straight into
    // `applicationShouldTerminate:` next (see this file's module doc and
    // `QuadraAppDelegate::evaluate_should_terminate`'s doc for the
    // one-shot guard that stops that from re-dispatching `WindowClose`).
    unsafe impl NSWindowDelegate for QuadraView {
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, _sender: &NSWindow) -> bool {
            self.evaluate_window_should_close()
        }
    }
);

impl QuadraView {
    fn new(
        mtm: MainThreadMarker,
        backend: Rc<RefCell<MacBackend>>,
        paint: PaintFn,
        handle: HandleFn,
        tick: TickFn,
        exit_gate: Rc<ExitGate>,
    ) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(QuadraViewIvars {
            last_viewport: Cell::new(Viewport::default()),
            backend,
            paint,
            handle,
            tick,
            resize_debouncer: RefCell::new(ResizeDebouncer::new()),
            resize_timer: RefCell::new(None),
            exit_gate,
        });
        unsafe { msg_send![super(this), init] }
    }

    /// Convert `NSEvent.locationInWindow` into view-local coordinates
    /// and return `(x, y, modifier_flags)`. Top-left origin matches
    /// the rest of quadraui because `isFlipped` is true.
    fn locate(&self, event: &NSEvent) -> (f64, f64, usize) {
        // SAFETY: NSResponder callbacks run on the main thread inside
        // an active event scope.
        let win_pt = event.locationInWindow();
        let view_pt = self.convertPoint_fromView(win_pt, None);
        let flags = event.modifierFlags().0;
        (view_pt.x, view_pt.y, flags)
    }

    /// The accept/reject answer both `draggingEntered:` and
    /// `draggingUpdated:` give AppKit: `Copy` for a drag
    /// carrying at least one file URL, `None` for anything else — a
    /// text or colour drag gets no drop cursor and never reaches
    /// `performDragOperation:`.
    ///
    /// Lives here, outside `define_class!`, so both selector arms can
    /// share it — see `draggingEntered:`'s doc for why one macro-defined
    /// method can't simply call another.
    fn drag_operation(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
        if self.dropped_paths_guarded(sender).is_empty() {
            NSDragOperation::None
        } else {
            NSDragOperation::Copy
        }
    }

    /// Panic-guarded wrapper around [`Self::dropped_paths`].
    ///
    /// `draggingEntered:`/`draggingUpdated:` (via [`Self::drag_operation`])
    /// and `performDragOperation:` are, like `drawRect:` and the responder
    /// methods `dispatch` guards, C-ABI entry points objc2's
    /// `define_class!` trampoline invokes directly off AppKit's
    /// drag-tracking loop — a boundary that cannot unwind. `dropped_paths`
    /// is written defensively today (all `?`-chained `Option`s, no
    /// `unwrap`/indexing), but that can silently stop being true after an
    /// innocuous future edit, so it gets the same `catch_unwind` treatment
    /// as `draw_rect`'s `paint` call and `dispatch`'s `handle` call rather
    /// than relying on that property holding forever.
    ///
    /// On a caught panic, report it once (deduped by call site, so a
    /// hovering drag re-entering `draggingUpdated:` every frame doesn't
    /// spam the log) and answer as if the drag carried no files — the
    /// same "reject this drag" outcome `dropped_paths` itself returns for
    /// a drag with no file-URL pasteboard item.
    fn dropped_paths_guarded(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> Vec<PathBuf> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.dropped_paths(sender)))
        {
            Ok(paths) => paths,
            Err(payload) => {
                crate::desktop::report_caught_panic_once(
                    "macos::run::QuadraView::dropped_paths",
                    payload.as_ref(),
                );
                Vec::new()
            }
        }
    }

    /// Decode a drag's file-URL pasteboard items into filesystem paths.
    /// Empty (not an error) for a drag that carries no
    /// `NSPasteboardTypeFileURL` item — e.g. a text or color drag —
    /// which `draggingEntered:`/`performDragOperation:` both treat as
    /// "reject this drag." Never call this directly from a
    /// `define_class!` selector arm — go through
    /// [`Self::dropped_paths_guarded`] (or [`Self::drag_operation`],
    /// which already does) so a panic here can't escape the C ABI
    /// boundary.
    fn dropped_paths(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> Vec<PathBuf> {
        let pasteboard = sender.draggingPasteboard();
        let Some(items) = pasteboard.pasteboardItems() else {
            return Vec::new();
        };
        items
            .iter()
            .filter_map(|item| {
                // SAFETY: `NSPasteboardTypeFileURL` is a valid extern
                // global AppKit initialises before any drag can reach
                // this callback.
                let file_url_type = unsafe { NSPasteboardTypeFileURL };
                let url_string = item.stringForType(file_url_type)?;
                let url = NSURL::URLWithString(&url_string)?;
                let path = url.path()?;
                Some(PathBuf::from(path.to_string()))
            })
            .collect()
    }

    /// Route a translated [`UiEvent`] through `AppLogic::handle` and
    /// act on the returned [`Reaction`].
    ///
    /// # Panic safety
    ///
    /// Every responder override in the `define_class!` block above that
    /// ends up feeding `AppLogic::handle` (`mouseDown:`, `keyDown:`,
    /// `scrollWheel:`, the resize/DPI notification handlers, and
    /// `performDragOperation:`'s `FilesDropped` dispatch) funnels through
    /// this one method, itself called directly from objc2's dispatch
    /// trampoline — a C ABI boundary that cannot unwind. A single
    /// `catch_unwind` here guards every one of those call sites at once,
    /// including every branch inside `handle` → `dispatch_event`'s own
    /// internal `app.handle` calls, rather than needing a guard
    /// duplicated into each of `dispatch_event`'s several match arms. On
    /// a caught panic this falls back to `Reaction::Continue` — no
    /// redraw, no exit — the same safe default `win::run::dispatch_event`
    /// uses for the identical case.
    ///
    /// `draggingEntered:`/`draggingUpdated:` are the exception: they
    /// answer AppKit synchronously with an `NSDragOperation` and never
    /// call `handle` at all, so they can't route through here. They (and
    /// `performDragOperation:`'s own `dropped_paths` call, before it
    /// reaches the `dispatch` this method serves) are guarded separately
    /// by [`Self::dropped_paths_guarded`] — see its doc.
    fn dispatch(&self, ev: UiEvent) {
        let reaction = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (self.ivars().handle)(ev)
        })) {
            Ok(reaction) => reaction,
            Err(payload) => {
                crate::desktop::report_caught_panic_once(
                    "macos::run::QuadraView::dispatch (app.handle)",
                    payload.as_ref(),
                );
                Reaction::Continue
            }
        };
        self.apply_reaction(reaction);
    }

    /// Backing logic for `windowShouldClose:` — the
    /// `NSWindowDelegate` override above. Deliberately separate from
    /// [`Self::dispatch`]: that method always runs [`Self::apply_reaction`]
    /// and returns nothing, but `windowShouldClose:` must hand AppKit a
    /// `bool` instead, and must *not* run `apply_reaction`'s
    /// `Reaction::Exit` branch — that calls `request_exit` →
    /// `[NSApp terminate:]`, which is redundant here (returning `true`
    /// already lets AppKit's own close sequence proceed) and would risk
    /// re-entering this same delegate callback via `terminate:`'s own
    /// window-closing machinery.
    ///
    /// Only `Reaction::Exit` answers `true` (let the close proceed) —
    /// every other reaction, including the `Reaction::Continue` an app
    /// with no `WindowClose` opinion returns, vetoes it. Matches the veto
    /// rule `gtk::run`'s `connect_close_request` documents: an app must
    /// explicitly return `Reaction::Exit` from its `WindowClose` handler
    /// to let the window close at all.
    ///
    /// # Panic safety
    ///
    /// Same C-ABI-boundary guard as [`Self::dispatch`], with one
    /// difference in the fallback: a caught panic here vetoes the close
    /// (`false`) rather than defaulting to `Reaction::Continue`'s usual
    /// "apply nothing" behaviour, because `Reaction::Continue` already
    /// means "veto" for this method — closing the window anyway after
    /// `handle` panicked would silently skip the very unsaved-changes
    /// guard this method exists to make reliable.
    ///
    /// # One-shot guard against a second dispatch
    ///
    /// When this returns `true` and this is the app's last open window,
    /// AppKit doesn't stop here — `applicationShouldTerminateAfterLastWindowClosed:`
    /// (unconditionally `true`, see [`QuadraAppDelegate::should_terminate_after_last_window`])
    /// carries the close straight into its ordinary termination sequence,
    /// which calls `applicationShouldTerminate:` on the very same app
    /// delegate before the process actually exits. Left alone, that would
    /// dispatch `UiEvent::WindowClose` to `app.handle` a *second* time for
    /// what the user experienced as one action — showing a synchronous
    /// unsaved-changes dialog twice, or worse, answering
    /// `NSApplicationTerminateReply::TerminateCancel` after the window has
    /// already visually closed if the handler's second answer differs
    /// from its first. Calling [`ExitGate::note_exit_granted`] here, the
    /// moment this method itself resolves the close to `Reaction::Exit`,
    /// lets [`QuadraAppDelegate::evaluate_should_terminate`] recognise
    /// that this exact close was already fully decided moments earlier in
    /// the same call chain and skip re-dispatching entirely. A veto
    /// (`false`) never marks the gate: the window stays open, so no
    /// last-window termination sequence follows, and a *later*,
    /// independent Cmd-Q must still get its own fresh dispatch.
    fn evaluate_window_should_close(&self) -> bool {
        let reaction = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (self.ivars().handle)(UiEvent::WindowClose)
        })) {
            Ok(reaction) => reaction,
            Err(payload) => {
                crate::desktop::report_caught_panic_once(
                    "macos::run::QuadraView::evaluate_window_should_close (app.handle)",
                    payload.as_ref(),
                );
                return false;
            }
        };
        // Non-`Exit` reactions still need their usual effect (a `Redraw`
        // shouldn't be silently dropped just because it arrived via
        // `WindowClose`) — `apply_reaction` on `Reaction::Continue` is a
        // documented no-op (`runtime::apply_outcome`), so this is safe to
        // call unconditionally for every reaction except `Exit`, which
        // must NOT go through `apply_reaction`'s `request_exit` path (see
        // this method's doc for the re-entrancy hazard that would cause).
        if window_should_close_for_reaction(reaction) {
            // See "One-shot guard" above: mark this close resolved before
            // AppKit's last-window-close path can reach
            // `applicationShouldTerminate:` and ask again.
            self.ivars().exit_gate.note_exit_granted();
            true
        } else {
            self.apply_reaction(reaction);
            false
        }
    }

    /// Fire `AppLogic::tick` and act on the returned [`Reaction`].
    /// The [`MacBackend::set_tick_callback`] target a
    /// [`MacBackend::request_frame_in`] timer invokes once its delay
    /// elapses, *and* the target of `run_with`'s own
    /// repeating `idlePollTick:` timer — see that timer's doc for why a
    /// wake-only path alone wasn't enough. Mirrors [`Self::dispatch`]'s
    /// "translate, then apply" shape for the tick path instead of an
    /// event.
    ///
    /// Guarded the same way as [`Self::dispatch`] — this is also
    /// invoked directly from an objc2/AppKit callback (the `NSTimer`
    /// target set up in [`run`]), the same non-unwindable boundary.
    fn dispatch_tick(&self) {
        let reaction = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (self.ivars().tick)()
        })) {
            Ok(reaction) => reaction,
            Err(payload) => {
                crate::desktop::report_caught_panic_once(
                    "macos::run::QuadraView::dispatch_tick (app.tick)",
                    payload.as_ref(),
                );
                Reaction::Continue
            }
        };
        self.apply_reaction(reaction);
    }

    /// Apply a [`Reaction`] — delegates to the shared
    /// [`runtime::apply_outcome`] via this view's
    /// [`ReactionSink`] impl below.
    fn apply_reaction(&self, reaction: Reaction) {
        runtime::apply_outcome(reaction, self);
    }
}

impl ReactionSink for QuadraView {
    fn request_redraw(&self) {
        // SAFETY: `setNeedsDisplay:` on the main thread is the documented
        // way to schedule a repaint.
        self.setNeedsDisplay(true)
    }

    /// The app asked to exit — every `Reaction::Exit` returned from any
    /// event or tick funnels here through [`runtime::apply_outcome`].
    ///
    /// # Why the gate is marked before `terminate:` (issue #951 review fix)
    ///
    /// `[NSApp terminate:]` is documented to always ask the delegate
    /// `applicationShouldTerminate:` first, and since #951 quadraui
    /// *implements* that hook — so without this line, the overwhelmingly
    /// common `Key::Char('q') => Reaction::Exit` idiom used by nearly
    /// every `examples/common/*.rs` would take this path:
    ///
    /// ```text
    /// handle(KeyPressed('q')) -> Exit -> request_exit -> terminate:
    ///   -> applicationShouldTerminate: -> handle(WindowClose)   // never happened!
    ///   -> Continue (no WindowClose arm) -> TerminateCancel     // app refuses to quit
    /// ```
    ///
    /// i.e. the app would silently fail to quit on `q`, and an app that
    /// *does* handle `WindowClose` would run that handler a second,
    /// spurious time (duplicate confirmation dialog, double state
    /// mutation) for an event the user never triggered.
    ///
    /// [`ExitGate::note_exit_granted`] records that the app has *already*
    /// granted this exit, so [`QuadraAppDelegate::evaluate_should_terminate`]
    /// answers `TerminateNow` without asking again. GTK and Win dodge the
    /// same hazard structurally (`window.destroy()` / `DestroyWindow`,
    /// neither of which re-enters their close-veto hook); AppKit has no
    /// equivalent "terminate without consulting the delegate" call, so
    /// macOS records the grant instead. Marking it *before* the call
    /// matters: `terminate:` runs the delegate callback synchronously.
    fn request_exit(&self) {
        self.ivars().exit_gate.note_exit_granted();
        let mtm = MainThreadMarker::from(self);
        let app = NSApplication::sharedApplication(mtm);
        // SAFETY: `terminate:` on NSApp on the main thread is the
        // documented exit path.
        app.terminate(None);
    }

    fn request_frame_in(&self, delay: std::time::Duration) {
        self.ivars().backend.borrow().request_frame_in(delay);
    }
}

/// `QuadraAppDelegate`'s ivars — the shared `handle` closure,
/// so `applicationShouldTerminate:` can run the exact same
/// `UiEvent::WindowClose` veto decision `QuadraView::windowShouldClose:`
/// runs. See [`HandleFn`]'s doc for why this is an `Rc`, not a `Box`.
pub(crate) struct QuadraAppDelegateIvars {
    handle: HandleFn,
    /// The other half of [`QuadraViewIvars::exit_gate`] — the same `Rc`,
    /// shared so this delegate can tell whether the app already granted
    /// this exit moments earlier in the same call chain (either from the
    /// window's own `windowShouldClose:` or from a `Reaction::Exit` that
    /// reached `ReactionSink::request_exit`). See [`ExitGate`]'s doc.
    exit_gate: Rc<ExitGate>,
}

define_class!(
    /// `NSApplicationDelegate` — terminates the process when the last
    /// window closes (red traffic-light → exit), and asks
    /// the app before honouring Cmd-Q / the auto-prepended app-menu Quit
    /// item / a Dock "Quit".
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "QuadraUiAppDelegate"]
    #[ivars = QuadraAppDelegateIvars]
    pub(crate) struct QuadraAppDelegate;

    unsafe impl NSObjectProtocol for QuadraAppDelegate {}

    unsafe impl NSApplicationDelegate for QuadraAppDelegate {
        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn should_terminate_after_last_window(&self, _sender: &NSApplication) -> bool {
            true
        }

        // ── Cmd-Q / menu Quit, and the last-window-closed follow-up ──
        //
        // `[NSApp terminate:]` — which Cmd-Q and the auto-prepended app
        // menu's Quit item both send directly — asks the delegate this
        // one question and, absent an override, defaults to
        // `NSTerminateNow`: it does NOT walk open windows and ask each
        // one `windowShouldClose:` the way an ordinary window-manager
        // close does. Left unhandled, an app's unsaved-changes guard
        // (written against `UiEvent::WindowClose`) would run for the
        // traffic-light button but not for Cmd-Q — a gap this method
        // closes by dispatching the same event here and vetoing on
        // anything but `Reaction::Exit`.
        //
        // This method is also reached two other ways, both *after* the
        // app has already granted an exit: (1) closing the app's last
        // window via the traffic light returns `true` from
        // `windowShouldClose:`, and because
        // `should_terminate_after_last_window` above is unconditionally
        // `true`, AppKit carries that straight into its normal
        // termination sequence, landing right back here; (2) any
        // `Reaction::Exit` from any event reaches
        // `ReactionSink::request_exit` → `[NSApp terminate:]`, which by
        // contract always asks this method first. See `ExitGate`'s doc
        // for the guard that stops either from asking the app twice.
        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _sender: &NSApplication) -> NSApplicationTerminateReply {
            self.evaluate_should_terminate()
        }

        // ── Deep links / file associations ─────────────────────────
        //
        // Fires for any URL this app is asked to open — a registered
        // `CFBundleURLTypes` scheme (`myapp://...`, an OAuth-callback
        // redirect) or a `CFBundleDocumentTypes` document with no
        // associated `NSDocument` subclass — both at launch (Launch
        // Services starts the process *with* the URL already queued)
        // and at runtime (the app is already running and gets
        // reactivated with a new one; macOS's own single-instance
        // activation for a bundled `.app` is what delivers that
        // reactivation, not any mechanism quadraui installs itself).
        // Per Apple's own doc on this method,
        // implementing it means `application:openFiles:` below is
        // never actually called — kept anyway as a defensive fallback.
        //
        // **Known gap**: this only fires once AppKit has finished
        // enough of its own launch sequence to deliver the delegate
        // callback. A URL that arrives *before* that point (a cold
        // launch racing Launch Services) needs a raw
        // `NSAppleEventManager`/`kAEGetURL` handler registered even
        // earlier — that requires the `objc2-core-services` crate,
        // which was not available at the time of this implementation.
        #[unsafe(method(application:openURLs:))]
        fn application_open_urls(&self, _application: &NSApplication, urls: &NSArray<NSURL>) {
            self.dispatch_open_requested(ns_urls_to_open_requested(urls));
        }

        /// See `application_open_urls`'s doc — per Apple's documented
        /// contract this is never actually invoked while that method is
        /// implemented, but it is wired the same way as a defensive fallback.
        #[unsafe(method(application:openFiles:))]
        fn application_open_files(&self, _sender: &NSApplication, filenames: &NSArray<NSString>) {
            self.dispatch_open_requested(ns_filenames_to_open_requested(filenames));
        }
    }
);

impl QuadraAppDelegate {
    fn new(mtm: MainThreadMarker, handle: HandleFn, exit_gate: Rc<ExitGate>) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(QuadraAppDelegateIvars { handle, exit_gate });
        unsafe { msg_send![super(this), init] }
    }

    /// Backing logic for `applicationShouldTerminate:` — see that
    /// method's doc for why Cmd-Q needs its own hook at all rather than
    /// relying on `QuadraView::windowShouldClose:`, and for the second
    /// path (last-window-closed) that also reaches this method.
    ///
    /// # One-shot guard against a second dispatch
    ///
    /// Delegates the whole decision to [`ExitGate::terminate_reply`],
    /// which consults [`QuadraAppDelegateIvars::exit_gate`] before
    /// running the `app.handle` closure passed in below. If the app
    /// already granted this exit — either
    /// [`QuadraView::evaluate_window_should_close`] resolved a close to
    /// `Reaction::Exit` (traffic light → last window closed → here), or a
    /// `Reaction::Exit` from *any* event reached
    /// [`ReactionSink::request_exit`] → `[NSApp terminate:]` (the `q`/
    /// Escape idiom) — the closure is never called and the reply is
    /// `TerminateNow`. A standalone Cmd-Q/menu-Quit request is preceded
    /// by neither (see `should_terminate`'s doc), so the gate is still
    /// unmarked for that gesture and it dispatches fresh exactly as
    /// before.
    ///
    /// # Panic safety
    ///
    /// Same C-ABI-boundary guard as [`QuadraView::dispatch`]: this is
    /// invoked directly from objc2's dispatch trampoline off AppKit's
    /// termination machinery, which cannot unwind. A caught panic
    /// answers `TerminateCancel` — same "veto, don't lose work" fallback
    /// [`QuadraView::evaluate_window_should_close`] uses for the
    /// identical case.
    ///
    /// # Known asymmetry: non-`Exit` side effects
    ///
    /// For a *fresh* dispatch (the guard above didn't short-circuit — a
    /// standalone Cmd-Q/menu-Quit, unrelated to any window close), this
    /// method only ever returns [`terminate_reply_for_reaction`]'s verdict
    /// and never calls [`QuadraView::apply_reaction`]: unlike
    /// `evaluate_window_should_close`, this delegate has no view reference
    /// to call it on. So a `Reaction::Redraw`/`RedrawAfter` returned from a
    /// `WindowClose` handler is honoured (`setNeedsDisplay`/timer
    /// scheduling) when the veto came from the traffic light, but not when
    /// it came from Cmd-Q. This is unrelated to the double-dispatch guard
    /// above (the guarded path never calls `app.handle` a second time at
    /// all, so it has no side effects to apply either way) — it's a
    /// pre-existing, independent gap for the *first* dispatch of a
    /// standalone Cmd-Q, considered low-risk (most `WindowClose` handlers
    /// only care about `Exit` vs. not) and left as a documented follow-up
    /// rather than plumbing a `QuadraView` reference into the delegate for
    /// this one case.
    fn evaluate_should_terminate(&self) -> NSApplicationTerminateReply {
        self.ivars().exit_gate.terminate_reply(|| {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (self.ivars().handle)(UiEvent::WindowClose)
            })) {
                Ok(reaction) => reaction,
                Err(payload) => {
                    crate::desktop::report_caught_panic_once(
                        "macos::run::QuadraAppDelegate::evaluate_should_terminate (app.handle)",
                        payload.as_ref(),
                    );
                    // `Reaction::Continue` maps to `TerminateCancel` —
                    // the same "veto, don't lose work" fallback
                    // `evaluate_window_should_close` uses (see the
                    // "Panic safety" section above).
                    Reaction::Continue
                }
            }
        })
    }

    /// Dispatch a [`UiEvent::OpenRequested`] through the
    /// shared `handle` closure — the delegate-side counterpart of
    /// [`QuadraView::dispatch`], for the two entry points
    /// (`application:openURLs:`/`application:openFiles:`) that reach
    /// this delegate directly instead of through the view.
    ///
    /// # Panic safety
    ///
    /// Entered directly from AppKit's delegate dispatch trampoline — a
    /// C ABI boundary that cannot unwind — same posture as
    /// [`Self::evaluate_should_terminate`]. Unlike that method, both
    /// Objective-C selectors this backs return `void`: there is no
    /// veto/reply value to degrade to, so a caught panic is reported
    /// and otherwise silently swallowed rather than answered with
    /// anything.
    fn dispatch_open_requested(&self, event: UiEvent) {
        if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (self.ivars().handle)(event)
        })) {
            crate::desktop::report_caught_panic_once(
                "macos::run::QuadraAppDelegate::dispatch_open_requested (app.handle)",
                payload.as_ref(),
            );
        }
    }
}

/// Translate `application:openURLs:`'s `NSArray<NSURL>` into
/// [`UiEvent::OpenRequested`]. `NSURL::isFileURL` splits
/// each entry the same way `gtk::run`'s `connect_open` splits
/// `gio::File::path()` vs `File::uri()`: a local file URL becomes a
/// `files` entry (`NSURL::path`); anything else (a custom app scheme, an
/// `https://` URL) becomes a `urls` entry (`NSURL::absoluteString`). A
/// URL that reports `isFileURL() == true` but has no decodable `path()`
/// (shouldn't happen in practice) is dropped rather than guessed at.
fn ns_urls_to_open_requested(urls: &NSArray<NSURL>) -> UiEvent {
    let mut open_urls = Vec::new();
    let mut files = Vec::new();
    for url in urls.iter() {
        if url.isFileURL() {
            if let Some(path) = url.path() {
                files.push(PathBuf::from(path.to_string()));
            }
        } else if let Some(absolute) = url.absoluteString() {
            open_urls.push(absolute.to_string());
        }
    }
    UiEvent::OpenRequested {
        urls: open_urls,
        files,
    }
}

/// Translate `application:openFiles:`'s `NSArray<NSString>` (plain
/// filesystem paths, not URLs — see that method's doc) into
/// [`UiEvent::OpenRequested`] with an empty `urls` list. See
/// `application_open_urls`'s doc for why this path is only ever a
/// defensive fallback in practice.
fn ns_filenames_to_open_requested(filenames: &NSArray<NSString>) -> UiEvent {
    UiEvent::OpenRequested {
        urls: Vec::new(),
        files: filenames
            .iter()
            .map(|name| PathBuf::from(name.to_string()))
            .collect(),
    }
}

/// The veto rule `QuadraView::windowShouldClose:` applies to whatever
/// [`Reaction`] the app's `UiEvent::WindowClose` handler returns: only
/// `Reaction::Exit` lets AppKit's close proceed. Pulled out as a pure
/// function so the actual veto decision is unit-testable without a live
/// `NSWindow`/`NSView`, which can't be constructed outside a real host.
fn window_should_close_for_reaction(reaction: Reaction) -> bool {
    matches!(reaction, Reaction::Exit)
}

/// Same rule as [`window_should_close_for_reaction`], answered in the
/// shape `applicationShouldTerminate:` needs instead of a bare `bool`
/// — the Cmd-Q / menu-Quit twin of the traffic-light veto.
/// Deliberately never returns `NSApplicationTerminateReply::TerminateLater`:
/// that variant exists for a delegate that needs to run an async sheet
/// before answering (see its doc), which no quadraui backend does today —
/// an app that wants a confirmation dialog first shows it synchronously
/// inside its own `WindowClose` handler and returns `Reaction::Continue`
/// (implicit veto) or `Reaction::Exit` (proceed) once it's decided.
fn terminate_reply_for_reaction(reaction: Reaction) -> NSApplicationTerminateReply {
    if window_should_close_for_reaction(reaction) {
        NSApplicationTerminateReply::TerminateNow
    } else {
        NSApplicationTerminateReply::TerminateCancel
    }
}

/// The one-shot double-dispatch guard shared by [`QuadraView`] and
/// [`QuadraAppDelegate`].
///
/// # The problem
///
/// `applicationShouldTerminate:` is the *only* hook AppKit offers for
/// Cmd-Q / the app-menu Quit item, so this backend must implement it to
/// make `UiEvent::WindowClose` reliable. But once implemented, it is
/// reached by three routes, and only one of them is a fresh user request
/// the app hasn't answered yet:
///
/// | Route | App already decided? |
/// |---|---|
/// | Cmd-Q / menu Quit / Dock Quit | **no** — dispatch `WindowClose` |
/// | traffic light → last window closed → `applicationShouldTerminateAfterLastWindowClosed: → true` | yes, in `windowShouldClose:` |
/// | any `Reaction::Exit` → [`ReactionSink::request_exit`] → `[NSApp terminate:]` | yes, in the handler that returned `Exit` |
///
/// Re-dispatching on either of the last two rows asks the app about a
/// `UiEvent::WindowClose` it already answered — or, for the third row,
/// one that never happened at all. The third is the worse of the two and
/// by far the more common: `Key::Char('q') | Key::Escape =>
/// Reaction::Exit` with no `WindowClose` arm is the exit idiom in nearly
/// every `examples/common/*.rs`, so the synthetic re-dispatch hits their
/// catch-all `_ => Reaction::Continue`, [`terminate_reply_for_reaction`]
/// turns that into `TerminateCancel`, and **the app silently refuses to
/// quit** despite its own handler having just granted the exit.
///
/// # The fix
///
/// Whoever grants an exit says so ([`Self::note_exit_granted`]);
/// [`Self::terminate_reply`] — the entire `applicationShouldTerminate:`
/// decision — skips the dispatch and answers `TerminateNow` when it sees
/// a grant already on record. One `Rc<ExitGate>` is built in [`run_with`]
/// and shared by the view and the app delegate.
///
/// GTK and Win avoid this hazard structurally rather than with state
/// (`GtkSink::request_exit` calls `window.destroy()`, not `close()`;
/// Win's calls `DestroyWindow` rather than resending `WM_CLOSE` — neither
/// re-enters that backend's close-veto hook). AppKit has no "terminate
/// without consulting the delegate" entry point, so macOS records the
/// grant instead.
///
/// # Lifetime and multi-window
///
/// The flag is set once and never reset. That is correct *only* because
/// [`run_with`] builds exactly one `NSWindow`/[`QuadraView`]: every route
/// that marks the gate is already on its way out of the process, so
/// there is no later, independent termination request left to suppress.
/// **If this backend ever grows multi-window support, this must become
/// per-window / reset-aware** — otherwise closing one non-last window
/// would mark the gate permanently and silently swallow the app's
/// `WindowClose` opinion on a genuinely unrelated Cmd-Q later on.
///
/// Kept as a plain non-`objc2` struct (like [`window_should_close_for_reaction`]
/// and [`window_title`]/[`window_style_mask`]) so the guard protocol
/// is unit-testable without a live `NSWindow`/`NSApplication`.
#[derive(Debug, Default)]
pub(crate) struct ExitGate {
    /// `true` once the app has granted an exit through *any* route (see
    /// the table above). Never reset — see "Lifetime and multi-window".
    exit_granted: Cell<bool>,
}

impl ExitGate {
    /// Record that the app has granted an exit, so a subsequent
    /// `applicationShouldTerminate:` doesn't ask it again. Idempotent.
    fn note_exit_granted(&self) {
        self.exit_granted.set(true);
    }

    /// `true` when some route already recorded the app's grant.
    fn exit_already_granted(&self) -> bool {
        self.exit_granted.get()
    }

    /// The whole `applicationShouldTerminate:` decision
    /// ([`QuadraAppDelegate::evaluate_should_terminate`]'s body).
    ///
    /// `dispatch_window_close` performs the real `app.handle(
    /// UiEvent::WindowClose)` call and is invoked **only** for a fresh
    /// request — a genuine Cmd-Q / menu-Quit the app hasn't answered.
    /// Taking it as a closure (rather than a `Reaction` computed by the
    /// caller) is what lets the guard prove, in a unit test, that no
    /// second dispatch happens at all, instead of merely that its result
    /// was discarded.
    fn terminate_reply(
        &self,
        dispatch_window_close: impl FnOnce() -> Reaction,
    ) -> NSApplicationTerminateReply {
        if self.exit_already_granted() {
            return NSApplicationTerminateReply::TerminateNow;
        }
        terminate_reply_for_reaction(dispatch_window_close())
    }
}

/// Configuration for [`run_with`]: the window title [`run`] hardcodes to
/// a generic default (`"quadraui (macos)"`).
///
/// A `ShellConfig`-driven consumer (`macos::shell_runner::run_with_shell`)
/// needs its own window title to reach the real AppKit window instead of
/// always showing `"quadraui (macos)"` — mirrors `gtk::run::RunConfig` /
/// `win::run::RunConfig`, minus the GTK-specific `app_id`/`icon_name`
/// fields AppKit has no equivalent concept for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunConfig {
    /// Window title shown by the titlebar, Mission Control, and the Dock.
    pub title: String,
    /// Mirrors [`crate::shell::ShellConfig::client_side_titlebar`]
    /// — `false` (the default) reproduces the exact style mask this
    /// module hardcoded before. See [`window_style_mask`] for what
    /// setting it changes.
    pub client_side_titlebar: bool,
    /// App icon applied via `NSApplication::setApplicationIconImage`
    /// right after `setActivationPolicy` in [`run_with`].
    /// `None` (the default) reproduces every pre-#1142 launch: no call
    /// at all, so a bare unbundled binary keeps showing the generic
    /// executable icon in the Dock and Cmd-Tab switcher. Set via
    /// [`Self::with_app_icon`]; a `ShellConfig`-driven consumer reaches
    /// this through [`crate::shell::ShellConfig::with_app_icon`] via
    /// `macos::shell_runner::run_with_shell`.
    pub app_icon: Option<ImageSource>,
}

impl RunConfig {
    /// Build a config with the given window title.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            client_side_titlebar: false,
            app_icon: None,
        }
    }

    /// Opt into the client-side-titlebar style mask — see
    /// [`crate::shell::ShellConfig::client_side_titlebar`] for the
    /// mechanism and why the native traffic lights stay put on macOS.
    pub fn with_client_side_titlebar(mut self, enabled: bool) -> Self {
        self.client_side_titlebar = enabled;
        self
    }

    /// Set the Dock/app-switcher icon applied via
    /// `NSApplication::setApplicationIconImage` — see [`Self::app_icon`].
    pub fn with_app_icon(mut self, icon: ImageSource) -> Self {
        self.app_icon = Some(icon);
        self
    }
}

impl Default for RunConfig {
    /// Default title reproduces the previously-hardcoded window title,
    /// so `run_with(app, RunConfig::default())` and `run(app)` behave
    /// identically.
    fn default() -> Self {
        Self {
            title: "quadraui (macos)".to_string(),
            client_side_titlebar: false,
            app_icon: None,
        }
    }
}

/// The window title [`run_with`] sets via `NSWindow::setTitle` — pulled
/// out of the AppKit call site so the config→window-creation seam is
/// testable off the main thread. Window creation itself
/// still needs a live `NSApplication` on the real main thread and can't be
/// exercised by a plain `#[test]`; this function is the pure "which string
/// do we hand AppKit" decision, mirroring the split `macos::backend`'s
/// `mac_cursor_kind` / `mac_cursor_for_shape` uses for the same reason.
fn window_title(config: &RunConfig) -> String {
    config.title.clone()
}

/// The `NSWindow` style mask [`run_with`]'s window-creation call site
/// applies — pulled out for the same off-main-thread testability reason as
/// [`window_title`]: `NSWindowStyleMask` is a plain
/// `NSUInteger` bitflags wrapper (see its `objc2_app_kit` definition), so
/// comparing bit patterns needs no live `NSApplication`, unlike window
/// *construction* itself.
///
/// `config.client_side_titlebar` unset (the default) reproduces the exact
/// mask this call site hardcoded before #947 — a regression guard for
/// every existing consumer. Set, it additionally carries
/// `FullSizeContentView`: the content view then extends under the full
/// titlebar region while the native traffic lights float on top of it, per
/// #947's "keep the native controls, don't emulate GTK's undecorated
/// model" design. [`run_with`]'s call site pairs this with
/// `setTitlebarAppearsTransparent(true)` + `NSWindowTitleVisibility::Hidden`
/// to complete the look — those two aren't part of the style mask itself,
/// so they can't be covered by this pure function; they're exercised by
/// the same operator-run smoke tier as the rest of live AppKit window
/// creation (see [`window_title`]'s doc).
fn window_style_mask(config: &RunConfig) -> NSWindowStyleMask {
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Resizable
        | NSWindowStyleMask::Miniaturizable;
    if config.client_side_titlebar {
        style | NSWindowStyleMask::FullSizeContentView
    } else {
        style
    }
}

/// Open an AppKit window, install a [`MacBackend`], and drive `app`
/// against it, using the default [`RunConfig`] (a generic window title).
/// See [`run_with`] to set a custom title — needed by any
/// app that isn't quadraui itself. Returns when the user closes the
/// window (red traffic-light) or `app.handle` returns [`Reaction::Exit`].
///
/// **Must be called from the main thread** — enforced by the
/// [`MainThreadMarker::new`] check at entry.
///
/// # Example
///
/// ```ignore
/// use quadraui::runner::{AppLogic, Reaction};
/// use quadraui::{Backend, UiEvent};
///
/// struct Hello;
/// impl AppLogic for Hello {
///     type AreaId = ();
///     fn render(&self, _b: &mut dyn Backend, _: ()) {}
///     fn handle(&mut self, _ev: UiEvent, _b: &mut dyn Backend) -> Reaction {
///         Reaction::Continue
///     }
/// }
///
/// fn main() -> std::process::ExitCode {
///     quadraui::macos::run(Hello)
/// }
/// ```
pub fn run<A: AppLogic + 'static>(app: A) -> std::process::ExitCode {
    run_with(app, RunConfig::default())
}

/// Same as [`run`], but with a caller-supplied [`RunConfig`] (window
/// title) instead of the generic quadraui default.
///
/// **Must be called from the main thread** — enforced by the
/// [`MainThreadMarker::new`] check at entry.
pub fn run_with<A: AppLogic + 'static>(app: A, config: RunConfig) -> std::process::ExitCode {
    let mtm =
        MainThreadMarker::new().expect("quadraui::macos::run must be called from the main thread");
    let boot = bootstrap(app, config, mtm);

    // SAFETY: blocks on AppKit run loop; returns when the last
    // window closes or `[NSApp terminate:]` is invoked.
    boot.ns_app.run();

    std::process::ExitCode::SUCCESS
}

/// Everything [`run_with`] builds before it hands control to AppKit's own
/// run loop: the window, the view, the app delegate, and every timer/
/// observer wired against them. Shared verbatim with [`MacRunner::new_with`]
/// — the only difference between the two entry points is what drives the
/// run loop afterwards (`ns_app.run()` blocking the whole process here vs.
/// `MacRunner::step`/[`MacRunner::pump`] servicing it non-blockingly, one
/// pass at a time, from inside a host that owns its own loop).
///
/// Every field but `ns_app` is held only to stay alive for as long as the
/// app runs — AppKit's object graph keeps working references to the
/// window/view/delegate/timers via the responder chain, the notification
/// center, and the run loop's own timer list, but nothing *here* reaches
/// back into them after bootstrap finishes, so a leading `_` on most fields
/// (matching the local-variable convention `run_with` already used for the
/// same values) says so.
struct MacBootstrap {
    ns_app: Retained<NSApplication>,
    _window: Retained<NSWindow>,
    _view: Retained<QuadraView>,
    _delegate: Retained<QuadraAppDelegate>,
    _idle_poll_timer: Retained<NSTimer>,
    _blink_target: Retained<super::caret_blink::QuadraBlinkTarget>,
    _blink_timer: Retained<NSTimer>,
}

fn bootstrap<A: AppLogic + 'static>(
    app: A,
    config: RunConfig,
    mtm: MainThreadMarker,
) -> MacBootstrap {
    let app = Rc::new(RefCell::new(app));
    let backend = Rc::new(RefCell::new(MacBackend::new()));

    // ── Default font ──────────────────────────────────────────────
    // Install Menlo 14pt before `setup` so backend-trait calls inside
    // the app's setup or first render frame find a font to measure
    // against. Apps that want a different family / size override via
    // `MacBackend::set_current_font` from their own `setup` hook —
    // but the shared backend-agnostic examples (`MiniApp`, `AppState`,
    // etc.) work with no per-app font wiring this way, matching the
    // ergonomics of `tui::run` and `gtk::run`.
    if let Some(font) = make_font("Menlo", 14.0) {
        backend.borrow_mut().set_current_font(font);
    }

    // ── App setup hook ────────────────────────────────────────────
    // Run `AppLogic::setup` once before the window opens. Accelerator
    // registration / cache warming happens here.
    {
        let mut backend_mut = backend.borrow_mut();
        let mut app_mut = app.borrow_mut();
        app_mut.setup(&mut *backend_mut);
    }

    // ── Build the type-erased paint + handle closures ────────────
    let paint: PaintFn = {
        let app = app.clone();
        let backend = backend.clone();
        Box::new(move |viewport: Viewport, cg_ref: CGContextRef| {
            // Drain any events queued from non-responder sources
            // (currently: native menu activations from
            // `Backend::install_menu_bar`). Each fires through
            // `AppLogic::handle` exactly like a mouse/keyboard event.
            // Done before painting so state mutations land in this
            // frame.
            let pending: Vec<UiEvent> = backend.borrow_mut().poll_events();
            for ev in pending {
                let _ = app.borrow_mut().handle(ev, &mut *backend.borrow_mut());
            }

            let mut backend_mut = backend.borrow_mut();
            let app_ref = app.borrow();
            render_frame(&mut backend_mut, &*app_ref, viewport, cg_ref);
        })
    };
    // Caret-blink state is shared between the backend (read each
    // frame in paint_aux), the blink timer (toggled every ~530 ms),
    // and the key handler (which pauses the blink during typing).
    let caret_visible = backend.borrow().caret_visible_handle();
    let caret_pause = backend.borrow().caret_blink_pause_handle();

    let handle: HandleFn = {
        let app = app.clone();
        let backend = backend.clone();
        let caret_visible = caret_visible.clone();
        let caret_pause = caret_pause.clone();
        Rc::new(move |ev: UiEvent| -> Reaction {
            let mut backend_mut = backend.borrow_mut();
            let mut app_mut = app.borrow_mut();
            match dispatch_event(
                ev,
                &mut backend_mut,
                &mut *app_mut,
                &caret_visible,
                &caret_pause,
            ) {
                EventOutcome::Continue => Reaction::Continue,
                EventOutcome::Redraw => Reaction::Redraw,
                // Pass the deadline through unchanged —
                // `dispatch`/`apply_reaction` routes it to
                // `ReactionSink::request_frame_in` exactly like any other
                // `Reaction::RedrawAfter`.
                EventOutcome::RedrawAfter(d) => Reaction::RedrawAfter(d),
                EventOutcome::Exit => Reaction::Exit,
            }
        })
    };

    // ── App tick hook ─────────────────────────────────────────────
    // Bridges `QuadraView::dispatch_tick` to `AppLogic::tick` — see
    // `TickFn`'s doc for what this closure does.
    let tick: TickFn = {
        let app = app.clone();
        let backend = backend.clone();
        Box::new(move || -> Reaction {
            let mut backend_mut = backend.borrow_mut();
            let mut app_mut = app.borrow_mut();
            app_mut.tick(&mut *backend_mut)
        })
    };

    // ── AppKit bootstrap ─────────────────────────────────────────
    let ns_app = NSApplication::sharedApplication(mtm);
    let _ = ns_app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    // `config.app_icon` (from `ShellConfig::with_app_icon`) becomes
    // the Dock/app-switcher icon — right after `setActivationPolicy`,
    // before the window opens, so the icon is already in place by the
    // time the app first appears in the Dock rather than flashing the
    // generic executable icon first. This is what makes a bare,
    // unbundled binary (no `Info.plist`/`.icns` bundle resource) show its
    // own icon at all. A decode failure (missing file, corrupt bytes,
    // unrecognised format) silently leaves the generic icon in place —
    // same collapse-to-nothing posture `TrayService::set_icon`'s
    // `decode_ns_image` documents — rather than failing startup over a
    // cosmetic detail.
    if let Some(source) = &config.app_icon {
        if let Some(image) = decode_ns_image(source) {
            // SAFETY: `image` is a live, owned `NSImage` for the
            // duration of this call; `ns_app` is the live, main-thread
            // `NSApplication` singleton obtained just above.
            unsafe { ns_app.setApplicationIconImage(Some(&image)) };
        }
    }

    // The app delegate needs its own handle to `handle` (Cmd-Q /
    // menu Quit routes through `applicationShouldTerminate:`, not
    // through any window's `windowShouldClose:`) — `handle` is `Rc`-
    // shared for exactly this, see `HandleFn`'s doc. Cloned here, before
    // `QuadraView::new` below moves the original into the view's ivars.
    //
    // The `ExitGate` is the one-shot double-dispatch guard (see its
    // doc for the three routes into `applicationShouldTerminate:` and
    // which two it suppresses). Shared the same way as `handle`: cloned
    // into the delegate here, moved into the view below.
    let exit_gate = Rc::new(ExitGate::default());
    let delegate = QuadraAppDelegate::new(mtm, handle.clone(), exit_gate.clone());
    let delegate_proto = ProtocolObject::from_ref(&*delegate);
    ns_app.setDelegate(Some(delegate_proto));

    let content_rect = NSRect::new(NSPoint::new(120.0, 120.0), NSSize::new(800.0, 600.0));
    let style = window_style_mask(&config);
    let window: Retained<NSWindow> = unsafe {
        msg_send![
            mtm.alloc::<NSWindow>(),
            initWithContentRect: content_rect,
            styleMask: style,
            backing: NSBackingStoreType::Buffered,
            defer: false,
        ]
    };
    window.setTitle(&NSString::from_str(&window_title(&config)));
    // Client-side titlebar — keep the native traffic lights (they
    // come along with `NSWindowStyleMask::Titled`, which
    // `window_style_mask` still sets) but hide the native title text and
    // stop the titlebar drawing an opaque background, so an app-painted
    // band in `AppShellLayout::title_bar_bounds` shows through instead of
    // being double-drawn under it. Must run after `FullSizeContentView`
    // is already in `style` above — `setTitlebarAppearsTransparent` only
    // has an effect when that mask bit is set (see its doc).
    if config.client_side_titlebar {
        window.setTitlebarAppearsTransparent(true);
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
    }

    // Stash the window handle so `Backend::begin_window_drag` /
    // `Backend::toggle_window_maximize` / `Backend::set_cursor` have
    // something to drive. Harmless for apps that never call them
    // (default no-op on every other backend, and macOS apps that don't
    // opt into a CSD titlebar).
    backend.borrow_mut().set_window(window.clone());

    let view = QuadraView::new(mtm, backend.clone(), paint, handle, tick, exit_gate);
    window.setContentView(Some(&view));
    window.setAcceptsMouseMovedEvents(true);
    window.makeFirstResponder(Some(view.as_super()));
    // The view doubles as its own window delegate so
    // `windowShouldClose:` (the red traffic-light button, Cmd-W, the
    // native "Close" menu item) can veto the close via `UiEvent::WindowClose`
    // — see `QuadraView`'s `NSWindowDelegate` impl above.
    window.setDelegate(Some(ProtocolObject::from_ref(&*view)));
    window.makeKeyAndOrderFront(None);

    // OS file drop: opt the view into `NSDraggingDestination`
    // for file-URL drags. Without this call `draggingEntered:`/
    // `performDragOperation:` are simply never invoked — AppKit only
    // considers a view a drop target for the pasteboard types it
    // registered.
    // SAFETY: `NSPasteboardTypeFileURL` is a valid extern global AppKit
    // initialises before this runs.
    let file_url_type = unsafe { NSPasteboardTypeFileURL };
    let dragged_types: Retained<NSArray<NSString>> = NSArray::from_slice(&[file_url_type]);
    view.registerForDraggedTypes(&dragged_types);

    // Install the wake target `MacBackend::waker()` invokes
    // (via `dispatch2::DispatchQueue::main().exec_async`) when a
    // background thread delivers a `UiEvent::User` — schedule the same
    // repaint request any other event's `Reaction::Redraw` would, so
    // `poll_events` (drained from inside the `paint` closure above) gets
    // a chance to observe the queued payload promptly instead of waiting
    // for unrelated user input or a timer to trigger the next `drawRect:`.
    let view_for_wake = view.clone();
    backend
        .borrow()
        .set_wake_callback(Rc::new(move || view_for_wake.request_redraw()), mtm);

    // Install the tick target `MacBackend::request_frame_in`'s
    // scheduled timer invokes once its delay elapses — mirrors the wake
    // wiring immediately above, but reaches `dispatch_tick` (fires
    // `AppLogic::tick`) instead of a bare repaint request.
    let view_for_tick = view.clone();
    backend
        .borrow()
        .set_tick_callback(Rc::new(move || view_for_tick.dispatch_tick()), mtm);

    // WindowResized wiring: opt the view into frame-change
    // notifications and observe them on itself via `viewFrameDidChange:`.
    // SAFETY: `view` is a valid, retained `QuadraView` (an `NSObject`
    // subclass) for the lifetime of the app; the observer registration
    // and the object being observed share that lifetime, and
    // `NSNotificationCenter` doesn't retain the observer, matching the
    // `target`-registration pattern `menu_bar_install` uses for menu
    // actions. The cast to `&AnyObject` is a no-op upcast through the
    // Obj-C class chain (`QuadraView` → `NSView` → `NSResponder` →
    // `NSObject`).
    view.setPostsFrameChangedNotifications(true);
    let view_obj: &AnyObject = unsafe { &*(&*view as *const QuadraView as *const AnyObject) };
    unsafe {
        NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
            view_obj,
            sel!(viewFrameDidChange:),
            Some(NSViewFrameDidChangeNotification),
            Some(view_obj),
        );
    }

    // Coarse idle-poll fallback — see `idlePollTick:`'s doc
    // on `QuadraView` and `crate::runtime::IDLE_POLL_CEILING`'s doc for
    // the full rationale. Mirrors `gtk::run::run_with`'s
    // `glib::timeout_add_local(IDLE_POLL_CEILING, ...)` install site:
    // a repeating timer targeting the view itself, kept alive for the
    // app's lifetime as a local binding (same pattern as
    // `_blink_timer`/`_blink_target` below) rather than stored on
    // `QuadraViewIvars` — nothing needs to reach it again after this
    // point, unlike `resize_timer`, which a live drag replaces.
    let _idle_poll_timer: Retained<NSTimer> = unsafe {
        NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
            crate::runtime::IDLE_POLL_CEILING.as_secs_f64(),
            view_obj,
            sel!(idlePollTick:),
            None,
            true,
        )
    };

    // HiDPI runtime change: unlike the frame-change
    // notification above (which the *view* posts about itself), it's the
    // *window* that posts `NSWindowDidChangeBackingPropertiesNotification`
    // — the drag-to-a-different-DPI-monitor / live scaling-setting-change
    // case `DpiChanged` exists for. `windowDidChangeBackingProperties:`
    // is still handled on `view_obj` (the only object here with a
    // `dispatch` method); this just tells `NSNotificationCenter` which
    // object's notifications to route there.
    let window_obj: &AnyObject = unsafe { &*(&*window as *const NSWindow as *const AnyObject) };
    unsafe {
        NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
            view_obj,
            sel!(windowDidChangeBackingProperties:),
            Some(NSWindowDidChangeBackingPropertiesNotification),
            Some(window_obj),
        );
    }

    // Drive the InlineInput caret blink. The pair is held as
    // locals for the lifetime of the app — when this function
    // eventually returns, the NSTimer's `Retained` drops, the timer
    // is released by the run loop, and the target finalises.
    let (_blink_target, _blink_timer) =
        super::caret_blink::install_blink_timer(mtm, caret_visible, caret_pause);

    // `activate()` is the non-deprecated replacement for
    // `activateIgnoringOtherApps:`, but it's a macOS 14+ selector — on
    // older systems sending it aborts with `doesNotRecognizeSelector:`.
    // Activation on 14+ is cooperative (a request, not a guarantee);
    // `activateIgnoringOtherApps:` is the unconditionally-available
    // fallback for everything older.
    if ns_app.respondsToSelector(sel!(activate)) {
        ns_app.activate();
    } else {
        #[allow(deprecated)]
        ns_app.activateIgnoringOtherApps(true);
    }

    MacBootstrap {
        ns_app,
        _window: window,
        _view: view,
        _delegate: delegate,
        _idle_poll_timer,
        _blink_target,
        _blink_timer,
    }
}

// Suppress unused warning for the c_void import — kept available for
// future opaque-pointer dancing in this file as it grows.
#[allow(dead_code)]
fn _unused_imports(_p: *mut c_void) {}

pub use crate::runner::StepOutcome;

/// Non-blocking / bounded-wait entry points for a host that owns its own
/// event loop — the macOS counterpart of [`crate::tui::run::TuiRunner`] and
/// [`crate::gtk::run::GtkRunner`].
///
/// [`run`]/[`run_with`] each own the whole process: `NSApplication::run()`
/// blocks the calling thread inside AppKit's run loop until the app quits.
/// [`MacRunner::new`]/[`MacRunner::new_with`] call the exact same
/// [`bootstrap`] function `run_with` does — the window, view, delegate, and
/// every timer/observer are wired identically either way. What differs is
/// only how the run loop gets serviced afterwards:
///
/// - [`MacRunner::step`] — one non-blocking pass: pulls at most one pending
///   event via `nextEventMatchingMask_untilDate_inMode_dequeue` with an
///   expiration of `NSDate::distantPast()` (AppKit's idiom for "don't wait —
///   return immediately if nothing's queued"), dispatches it if present via
///   `sendEvent`, then calls `updateWindows` so any `setNeedsDisplay` queued
///   by that dispatch (or by a prior `Backend::waker`/tick callback) actually
///   paints before this call returns.
/// - [`MacRunner::pump`] — the same single pass, but with the expiration
///   date bounded to `timeout` from now (`NSDate::dateWithTimeIntervalSinceNow`)
///   instead of `distantPast()`, so the call blocks until either a real event
///   arrives or `timeout` elapses, whichever is first.
///
/// Both return a [`StepOutcome`]: `Continue` while this runner's own window
/// is still open, `Exited` once *it* closes — checked via
/// `_boot._window.isVisible()`, scoped to the single window [`bootstrap`]
/// created rather than `NSApplication::windows()` (the whole process's
/// window list), mirroring `GtkRunner::outcome`'s per-`Application` scoping.
/// A host embedding quadraui alongside its own AppKit windows therefore sees
/// `Exited` exactly when quadraui's window goes away, independent of what
/// else the host has open.
///
/// A background thread's [`crate::backend::Backend::waker`] call reaches a
/// `MacRunner`-driven loop with no extra wiring: `MacBackend::waker()` posts
/// through `dispatch2::DispatchQueue::main().exec_async`, which — like any
/// other GCD main-queue block — becomes a source AppKit's run loop wakes for,
/// so a blocking `pump(timeout)` call returns as soon as the waker's posted
/// block runs, not only once `timeout` elapses.
///
/// **Caveat for `StepOutcome::Exited`:** `Reaction::Exit` and the
/// last-window-close path both route to `[NSApp terminate:]` (see this
/// module's top-level doc), and `terminate:` ends the process rather than
/// returning control to Rust. In practice this means a host driving a
/// `MacRunner` loop never actually observes `StepOutcome::Exited` from a
/// quadraui-initiated quit (Cmd-Q, the native Quit menu item, or
/// `Reaction::Exit`) — the process exits first. `Exited` is reachable today
/// only via the window's own close button/`Cmd-W`/`windowShouldClose:`
/// returning true without going through `terminate:`. A host that needs a
/// reliable non-terminating shutdown signal should treat this as a known
/// gap rather than relying on `pump`/`step` to report it.
pub struct MacRunner {
    _boot: MacBootstrap,
}

impl MacRunner {
    /// Build a runner with the default [`RunConfig`]. Builds the window,
    /// view, and delegate, and calls `app.setup()` — exactly what [`run`]
    /// does before handing control to `NSApplication::run()`.
    ///
    /// **Must be called from the main thread** — enforced by the
    /// [`MainThreadMarker::new`] check at entry, matching [`run_with`].
    pub fn new<A: AppLogic + 'static>(app: A) -> Self {
        Self::new_with(app, RunConfig::default())
    }

    /// Like [`Self::new`], but with an explicit [`RunConfig`].
    pub fn new_with<A: AppLogic + 'static>(app: A, config: RunConfig) -> Self {
        let mtm = MainThreadMarker::new()
            .expect("quadraui::macos::run::MacRunner must be built on the main thread");
        let boot = bootstrap(app, config, mtm);
        // `NSApp.isRunning`/`[NSApp run]`'s finishLaunching dance is skipped
        // entirely by the step()/pump() path below — `bootstrap` never calls
        // `ns_app.run()`, so nothing else sends `finishLaunching` either.
        // Without it `NSApp.isRunning` stays `NO` and
        // `NSApplicationWillFinishLaunching`/`DidFinishLaunching` never post;
        // the usual symptom is a dead app menu, since
        // `Backend::install_menu_bar`'s `setMainMenu` call expects a fully
        // launched application. Send it explicitly here so a step()/pump()-driven
        // app reaches the same AppKit launch state `ns_app.run()` would have
        // produced for free.
        boot.ns_app.finishLaunching();
        Self { _boot: boot }
    }

    /// Whether the app's own window is still open — see the type-level
    /// doc's third paragraph for why this checks the single window
    /// [`bootstrap`] created, not the process-wide window list.
    fn window_open(&self) -> bool {
        self._boot._window.isVisible()
    }

    fn outcome(&self) -> StepOutcome {
        if self.window_open() {
            StepOutcome::Continue
        } else {
            StepOutcome::Exited
        }
    }

    /// Pulls at most one event with the given expiration date, dispatches it
    /// if present, then flushes a queued redraw of this runner's own window
    /// only — the shared body of [`Self::step`] (`NSDate::distantPast()`)
    /// and [`Self::pump`] (a bounded future date).
    ///
    /// `updateWindows` alone only refreshes window-level chrome (titles,
    /// menu validation); a `setNeedsDisplay` queued by this pass's dispatch
    /// (or by a prior [`MacBackend::waker`]/tick callback reaching the main
    /// queue while nothing is pumping events) otherwise waits for AppKit's
    /// own idle-time display pass — which a `step()`/`pump()` caller driving
    /// the loop one pass at a time never reaches on its own. `displayIfNeeded`
    /// forces that pass to happen synchronously, inside this call, so the
    /// caller observes the resulting frame before `step`/`pump` returns.
    /// Scoped to `_boot._window` rather than `NSApplication::windows()` so a
    /// host embedding its own AppKit windows alongside this one never has
    /// them repainted on quadraui's behalf.
    fn run_one(&mut self, expiration: &NSDate) -> StepOutcome {
        if !self.window_open() {
            return StepOutcome::Exited;
        }

        // SAFETY: `expiration` is a live `NSDate`; `NSDefaultRunLoopMode` is
        // an extern global AppKit initialises before this ever runs — the
        // same global `run_with`'s own bootstrap (and every other AppKit
        // frame-tick path in this module) relies on being already set up by
        // the time any app code runs.
        let event = unsafe {
            self._boot
                .ns_app
                .nextEventMatchingMask_untilDate_inMode_dequeue(
                    NSEventMask::Any,
                    Some(expiration),
                    NSDefaultRunLoopMode,
                    true,
                )
        };
        if let Some(event) = event {
            self._boot.ns_app.sendEvent(&event);
        }
        self._boot.ns_app.updateWindows();
        self._boot._window.displayIfNeeded();

        self.outcome()
    }

    /// One non-blocking pass — returns immediately whether or not an event
    /// was waiting.
    pub fn step(&mut self) -> StepOutcome {
        let expiration = NSDate::distantPast();
        self.run_one(&expiration)
    }

    /// One pass, blocking for up to `timeout` if nothing is ready yet.
    pub fn pump(&mut self, timeout: Duration) -> StepOutcome {
        let expiration = NSDate::dateWithTimeIntervalSinceNow(timeout.as_secs_f64());
        self.run_one(&expiration)
    }
}

/// Coverage for [`MacRunner::step`]/[`MacRunner::pump`] against a real,
/// live `NSApplication` — the one place in this module that actually
/// drives AppKit's run loop from a test rather than only exercising the
/// plain-Rust routing around it (see `window_close_tests`'s doc for why
/// most coverage here avoids a live app). Each test is gated behind
/// `MainThreadMarker::new()`, the same skip-rather-than-panic guard
/// `macos::menu_bar_install`'s and `macos::backend`'s live-AppKit tests use
/// (there are no other `MainThreadMarker`-gated tests in this file).
///
/// That gate is a *skip*, not a pass, under the standard test harness:
/// `MainThreadMarker::new()` checks the OS-level main thread, and
/// `cargo test`'s default harness always runs a test body on a thread it
/// spawns — never the process's actual main thread — regardless of
/// `--test-threads`. So under any plain `cargo test --features macos`
/// invocation, including a serial one, all three tests below return
/// early and report as passed without executing a single assertion.
/// There is currently no `cargo test` invocation that runs them for
/// real; doing so needs a custom `harness = false` binary whose `main`
/// calls these functions directly from the process's own main thread,
/// which this module does not provide. Treat a green `cargo test` run
/// here as "did not regress the skip path", not as evidence the
/// step/pump behaviour itself still works — that needs an operator
/// manually invoking them from a main-thread harness.
#[cfg(test)]
mod step_pump_tests {
    use super::*;
    use std::sync::Arc;

    /// Shared instrumentation an [`AppLogic`] fixture records into —
    /// cloned into the fixture before it's moved into [`MacRunner::new_with`]
    /// so the test keeps its own handle after the runner owns the app.
    #[derive(Default, Clone)]
    struct Recorder {
        render_calls: Rc<Cell<usize>>,
        tick_calls: Rc<Cell<usize>>,
        events: Rc<RefCell<Vec<UiEvent>>>,
        waker: Rc<RefCell<Option<Arc<dyn Fn(crate::UserPayload) + Send + Sync>>>>,
    }

    /// An [`AppLogic`] that does nothing but feed a [`Recorder`] — no
    /// test below needs any particular `Reaction` other than `Continue`,
    /// since driving a real `Reaction::Exit` through `MacRunner` would
    /// reach `[NSApp terminate:]` and end the test process (see
    /// `window_close_tests::app_driven_exit_does_not_resurface_as_a_synthetic_window_close`'s
    /// doc for the same constraint).
    struct StepTestApp {
        recorder: Recorder,
        /// When set, `setup` asks the backend to schedule exactly one
        /// wake at this delay — the vehicle for the `RedrawAfter` test.
        redraw_after: Option<Duration>,
    }

    impl AppLogic for StepTestApp {
        type AreaId = ();

        fn setup(&mut self, backend: &mut dyn Backend) {
            *self.recorder.waker.borrow_mut() = Some(backend.waker());
            if let Some(delay) = self.redraw_after {
                backend.request_frame_in(delay);
            }
        }

        fn render(&self, _backend: &mut dyn Backend, _area: ()) {
            self.recorder
                .render_calls
                .set(self.recorder.render_calls.get() + 1);
        }

        fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            self.recorder.events.borrow_mut().push(event);
            Reaction::Continue
        }

        fn tick(&mut self, _backend: &mut dyn Backend) -> Reaction {
            self.recorder
                .tick_calls
                .set(self.recorder.tick_calls.get() + 1);
            Reaction::Continue
        }
    }

    /// [`MacRunner::step`] must both keep the window alive
    /// (`StepOutcome::Continue`) and force at least one real paint —
    /// a freshly ordered-front window starts out needing display, and
    /// `run_one`'s `displayIfNeeded` sweep is what turns that into an
    /// observed `app.render` call before `step` returns.
    #[test]
    fn step_observes_a_rendered_frame() {
        let Some(_mtm) = MainThreadMarker::new() else {
            return;
        };

        let recorder = Recorder::default();
        let app = StepTestApp {
            recorder: recorder.clone(),
            redraw_after: None,
        };
        let mut runner = MacRunner::new(app);

        let outcome = runner.step();

        assert_eq!(outcome, StepOutcome::Continue);
        assert!(
            recorder.render_calls.get() >= 1,
            "step() must force a synchronous repaint of the freshly opened window"
        );
    }

    /// A background thread's [`Backend::waker`] call must reach
    /// `app.handle` as a `UiEvent::User` through a single bounded
    /// `pump()` — the "without busy-polling" half of the acceptance
    /// criteria: one blocking call, not a `step()` retry loop, is enough
    /// because GCD's main-queue dispatch is itself a run-loop source
    /// `nextEventMatchingMask_untilDate_inMode_dequeue` wakes for.
    #[test]
    fn waker_delivers_a_user_event_without_busy_polling() {
        let Some(_mtm) = MainThreadMarker::new() else {
            return;
        };

        let recorder = Recorder::default();
        let app = StepTestApp {
            recorder: recorder.clone(),
            redraw_after: None,
        };
        let mut runner = MacRunner::new(app);

        let waker = recorder
            .waker
            .borrow_mut()
            .take()
            .expect("setup() must capture a waker before MacRunner::new returns");
        let handle = std::thread::spawn(move || {
            waker(crate::UserPayload::new(7_i32));
        });
        handle.join().expect("background thread must not panic");

        let outcome = runner.pump(Duration::from_secs(2));

        assert_eq!(outcome, StepOutcome::Continue);
        let events = recorder.events.borrow();
        assert_eq!(
            events.len(),
            1,
            "expected exactly the one waker-delivered event"
        );
        match &events[0] {
            UiEvent::User(payload) => {
                assert_eq!(payload.downcast_ref::<i32>(), Some(&7));
            }
            other => panic!("expected UiEvent::User, got {other:?}"),
        }
    }

    /// `Reaction::RedrawAfter`'s underlying mechanism —
    /// `Backend::request_frame_in` — schedules exactly one wake at the
    /// deadline, not a repeating timer: once it fires, `tick` runs once
    /// and never again on its own. Both `pump` windows here stay well
    /// under `crate::runtime::IDLE_POLL_CEILING` (250ms) so the
    /// always-on idle-poll fallback — a separate, already-covered
    /// mechanism — can't fire in between and confound the count.
    #[test]
    fn redraw_after_schedules_exactly_one_wake_at_the_deadline() {
        let Some(_mtm) = MainThreadMarker::new() else {
            return;
        };

        let recorder = Recorder::default();
        let app = StepTestApp {
            recorder: recorder.clone(),
            redraw_after: Some(Duration::from_millis(30)),
        };
        let mut runner = MacRunner::new(app);

        // First pump: long enough for the 30ms deadline to fire.
        runner.pump(Duration::from_millis(80));
        assert_eq!(
            recorder.tick_calls.get(),
            1,
            "the scheduled wake must fire exactly once by the deadline"
        );

        // Second pump, same short bound: the one-shot wake must not
        // repeat on its own.
        runner.pump(Duration::from_millis(80));
        assert_eq!(
            recorder.tick_calls.get(),
            1,
            "a RedrawAfter wake must not re-fire without a fresh request_frame_in call"
        );
    }
}

/// Coverage for [`RunConfig`] / [`window_title`] / [`window_style_mask`]
/// — display-free, since all three are plain Rust (or, for `window_style_mask`,
/// plain-Rust-comparable `NSWindowStyleMask` bit patterns) with no
/// live-AppKit dependency. `run_with`'s actual wiring (title → `NSWindow::setTitle`,
/// and the `setTitlebarAppearsTransparent`/`setTitleVisibility` pair) can't be
/// exercised without a live main-thread `NSApplication`; that's covered by the
/// operator-run smoke tier instead, same posture as `gtk::run::run_config_tests` /
/// `win::run::tests`.
#[cfg(test)]
mod run_config_tests {
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
    fn new_defaults_client_side_titlebar_to_false() {
        // Every pre-existing `RunConfig::new(title)` call site
        // (including `shell_runner::run_with_shell` before it started
        // threading `ShellConfig::client_side_titlebar` through) must keep
        // getting today's style mask, not silently opt into the new one.
        assert!(!RunConfig::new("kubeui").client_side_titlebar);
    }

    #[test]
    fn with_client_side_titlebar_sets_the_flag() {
        let config = RunConfig::new("kubeui").with_client_side_titlebar(true);
        assert!(config.client_side_titlebar);
    }

    /// A fresh `RunConfig` sets no app icon — every existing
    /// consumer keeps the pre-#1142 behaviour (no
    /// `setApplicationIconImage` call at all).
    #[test]
    fn new_and_default_set_no_app_icon() {
        assert_eq!(RunConfig::new("kubeui").app_icon, None);
        assert_eq!(RunConfig::default().app_icon, None);
    }

    /// `with_app_icon` stores the source verbatim for
    /// `run_with`'s AppKit-bootstrap call site to decode via
    /// `macos::tray::decode_ns_image`.
    #[test]
    fn with_app_icon_sets_the_source() {
        let config = RunConfig::new("kubeui").with_app_icon(ImageSource::Bytes(vec![0xde, 0xad]));
        assert_eq!(config.app_icon, Some(ImageSource::Bytes(vec![0xde, 0xad])));
    }

    #[test]
    fn default_matches_runs_previously_hardcoded_title() {
        // `run(app)` used to always show a `"quadraui (macos)"` window
        // title; `RunConfig::default()` must reproduce that exact value
        // so `run(app)` staying `run_with(app, RunConfig::default())`
        // doesn't change existing behaviour.
        assert_eq!(RunConfig::default().title, "quadraui (macos)");
    }

    /// Before quadraui#933, `run_with`'s window-creation call site
    /// handed AppKit the literal `"quadraui (macos)"` no matter what the
    /// caller configured. This pins `window_title` — the pure function
    /// that call site now delegates to — to `config.title`, so a future
    /// edit that goes back to hardcoding the string at that call site
    /// (bypassing this function, or reintroducing a literal inside it)
    /// fails here instead of only being discoverable by running a real app.
    #[test]
    fn window_title_comes_from_config_not_a_literal() {
        let config = RunConfig::new("vimcode");
        assert_eq!(window_title(&config), "vimcode");
        assert_ne!(
            window_title(&config),
            "quadraui (macos)",
            "a custom RunConfig title must not fall back to the generic default"
        );
    }

    /// With `client_side_titlebar` unset, the style mask
    /// [`window_style_mask`] returns must stay byte-identical to what
    /// `run_with`'s call site hardcoded before — exactly
    /// `Titled | Closable | Resizable | Miniaturizable`, no
    /// `FullSizeContentView` bit.
    #[test]
    fn style_mask_unset_matches_pre_947_hardcoded_mask() {
        let config = RunConfig::new("kubeui");
        let expected = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Resizable
            | NSWindowStyleMask::Miniaturizable;
        assert_eq!(window_style_mask(&config), expected);
        assert!(!window_style_mask(&config).contains(NSWindowStyleMask::FullSizeContentView));
    }

    /// `client_side_titlebar` ORs in `FullSizeContentView` on top of
    /// the base mask — it must not drop `Titled`/`Closable`/
    /// `Resizable`/`Miniaturizable`, since dropping `Titled` in particular
    /// would take the native traffic lights away entirely.
    #[test]
    fn style_mask_set_adds_full_size_content_view_without_losing_the_rest() {
        let config = RunConfig::new("kubeui").with_client_side_titlebar(true);
        let mask = window_style_mask(&config);
        assert!(mask.contains(NSWindowStyleMask::FullSizeContentView));
        assert!(mask.contains(NSWindowStyleMask::Titled));
        assert!(mask.contains(NSWindowStyleMask::Closable));
        assert!(mask.contains(NSWindowStyleMask::Resizable));
        assert!(mask.contains(NSWindowStyleMask::Miniaturizable));
    }
}

/// Headless coverage for [`fill_frame_background`] — the
/// whole-frame backdrop `QuadraView::draw_rect` paints before delegating
/// to the app's own render closure. Uses
/// [`super::headless::BitmapSurface`] rather than a live `NSView`, so it
/// exercises the exact FFI call `drawRect:` makes with no window/display
/// dependency (see this file's module doc + `headless.rs`'s doc for why
/// that pairing works).
#[cfg(test)]
mod background_fill_tests {
    use super::*;
    use crate::macos::headless::BitmapSurface;
    use crate::types::Color;

    /// A light theme's background must show through verbatim — this is
    /// the exact regression #1179 reports: pre-fix, this pixel would be
    /// `#1F1F24` (31, 31, 36) regardless of the theme, because the fill
    /// colour was a hard-coded literal instead of the theme background.
    #[test]
    fn fill_frame_background_paints_the_given_background_not_a_fixed_dark_literal() {
        const W: u32 = 120;
        const H: u32 = 80;
        // vscode-light-like background — nothing close to #1F1F24.
        let light_bg = Color::rgb(236, 236, 236);

        let surface = BitmapSurface::new(W, H);
        let rect = CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(W as f64, H as f64));
        // SAFETY: `surface.context_ptr()` is a valid bitmap context for
        // the surface's lifetime, borrowed only for this call.
        unsafe {
            fill_frame_background(surface.context_ptr(), rect, light_bg);
        }

        // Sample points across the window: traffic-light inset (top-left),
        // breadcrumb/status-bar gap, and scrollbar gutter — verifying every
        // unpainted pixel uses the correct background color.
        for (x, y, label) in [
            (10, 10, "traffic-light inset"),
            (5, H - 3, "breadcrumb/status-bar gap"),
            (W - 5, H / 2, "scrollbar gutter"),
        ] {
            let (r, g, b, _a) = surface.pixel(x, y);
            assert_eq!(
                (r, g, b),
                (light_bg.r, light_bg.g, light_bg.b),
                "{label} pixel ({x}, {y}) must show the theme background {light_bg:?}, \
                 not a hard-coded dark literal: got ({r}, {g}, {b})"
            );
        }
    }
}

/// macOS event-dispatch logic: the [`window_should_close_for_reaction`]
/// and [`terminate_reply_for_reaction`] functions decide whether to permit
/// window close or app termination based on the reaction; [`ExitGate`]
/// ensures a double-dispatch guard prevents redundant termination when both
/// a traffic-light close and `Reaction::Exit` arrive. These are pulled out
/// as pure functions (same pattern as [`window_title`]/[`window_style_mask`])
/// so the decision logic has unit coverage without needing a live
/// `NSWindow`/`NSView` pair or real `NSApplication`. `dispatch_event`
/// delivers `UiEvent::WindowClose` the same way [`super::testing::MacDriver::dispatch`]
/// does, proving the event reaches `app.handle` unrewritten before veto logic
/// runs. [`ExitGate::terminate_reply`] takes the dispatch as a closure so
/// tests can assert the app is not called again rather than only that its
/// answer was ignored.
#[cfg(test)]
mod window_close_tests {
    use super::*;

    #[test]
    fn only_exit_permits_the_window_to_close() {
        assert!(window_should_close_for_reaction(Reaction::Exit));
        assert!(!window_should_close_for_reaction(Reaction::Continue));
        assert!(!window_should_close_for_reaction(Reaction::Redraw));
        assert!(!window_should_close_for_reaction(Reaction::RedrawAfter(
            std::time::Duration::from_millis(50)
        )));
    }

    #[test]
    fn only_exit_answers_terminate_now() {
        assert_eq!(
            terminate_reply_for_reaction(Reaction::Exit),
            NSApplicationTerminateReply::TerminateNow
        );
        assert_eq!(
            terminate_reply_for_reaction(Reaction::Continue),
            NSApplicationTerminateReply::TerminateCancel
        );
        assert_eq!(
            terminate_reply_for_reaction(Reaction::Redraw),
            NSApplicationTerminateReply::TerminateCancel
        );
        assert_eq!(
            terminate_reply_for_reaction(Reaction::RedrawAfter(std::time::Duration::from_millis(
                50
            ))),
            NSApplicationTerminateReply::TerminateCancel
        );
    }

    struct RecordingApp {
        events: Vec<UiEvent>,
        reaction: Reaction,
    }

    impl RecordingApp {
        /// `reaction` is what every `handle` call answers with — the
        /// tests below only care about the one `WindowClose` dispatch
        /// each drives, so a single fixed reaction (rather than a
        /// per-event script) keeps each test's setup to one line.
        fn new(reaction: Reaction) -> Self {
            Self {
                events: Vec::new(),
                reaction,
            }
        }
    }

    impl AppLogic for RecordingApp {
        type AreaId = ();

        fn render(&self, _backend: &mut dyn Backend, _area: ()) {}

        fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            self.events.push(event);
            self.reaction
        }
    }

    fn dispatch(ev: UiEvent, backend: &mut MacBackend, app: &mut RecordingApp) -> EventOutcome {
        let caret_visible = backend.caret_visible_handle();
        let caret_pause = backend.caret_blink_pause_handle();
        dispatch_event(ev, backend, app, &caret_visible, &caret_pause)
    }

    /// `WindowClose` doesn't match any of `dispatch_event`'s special-cased
    /// arms (`MouseDown`/`MouseMoved`/`MouseUp`), so it must fall all the
    /// way through to the shared `runtime::preprocess_event` → `app.handle`
    /// unrewritten — not silently dropped, and not folded into some other
    /// event the way a double-click gets folded.
    #[test]
    fn window_close_reaches_the_app_unrewritten() {
        let mut backend = MacBackend::new();
        let mut app = RecordingApp::new(Reaction::Continue);
        let _ = dispatch(UiEvent::WindowClose, &mut backend, &mut app);
        assert_eq!(
            app.events,
            vec![UiEvent::WindowClose],
            "WindowClose must reach app.handle unchanged, matching gtk::run::dispatch_event/ \
             win::run::dispatch_event"
        );
    }

    /// The default reaction an app with no `WindowClose` opinion returns
    /// (`Reaction::Continue`, from its catch-all match arm) must produce
    /// `EventOutcome::Continue` — the implicit veto every other backend
    /// applies too (see `window_should_close_for_reaction`'s doc).
    #[test]
    fn unhandled_window_close_does_not_exit() {
        let mut backend = MacBackend::new();
        let mut app = RecordingApp::new(Reaction::Continue);
        let outcome = dispatch(UiEvent::WindowClose, &mut backend, &mut app);
        assert_eq!(outcome, EventOutcome::Continue);
    }

    /// An app that explicitly returns `Reaction::Exit` from its
    /// `WindowClose` handler — the only way `window_should_close_for_reaction`/
    /// `terminate_reply_for_reaction` let the window/process actually
    /// close — must see that reaction survive the round trip through
    /// `dispatch_event` as `EventOutcome::Exit`.
    #[test]
    fn exit_reaction_survives_the_round_trip() {
        let mut backend = MacBackend::new();
        let mut app = RecordingApp::new(Reaction::Exit);
        let outcome = dispatch(UiEvent::WindowClose, &mut backend, &mut app);
        assert_eq!(outcome, EventOutcome::Exit);
    }

    /// Counts how many times `ExitGate::terminate_reply`'s dispatch
    /// closure actually ran, standing in for `app.handle(WindowClose)`.
    /// A count of 0 is the assertion that matters for the guarded routes:
    /// the app is never *asked* a second time, not merely that its answer
    /// was thrown away.
    struct DispatchCounter {
        calls: Cell<usize>,
        reaction: Reaction,
    }

    impl DispatchCounter {
        fn new(reaction: Reaction) -> Self {
            Self {
                calls: Cell::new(0),
                reaction,
            }
        }

        fn dispatch(&self) -> Reaction {
            self.calls.set(self.calls.get() + 1);
            self.reaction
        }
    }

    /// Route 2 of [`ExitGate`]'s table: `windowShouldClose:`
    /// (`evaluate_window_should_close`) resolved the close to
    /// `Reaction::Exit`, AppKit's last-window-closed follow-up then
    /// reaches `applicationShouldTerminate:`. The app must not be asked
    /// again, and the process must still be allowed to exit.
    #[test]
    fn resolved_window_close_tells_terminate_to_skip_a_second_dispatch() {
        let gate = ExitGate::default();
        assert!(
            !gate.exit_already_granted(),
            "a fresh app lifetime must not start with the gate already marked"
        );

        // Mirrors `evaluate_window_should_close`'s own body: only a
        // reaction that permits the close (`Reaction::Exit`) marks it.
        if window_should_close_for_reaction(Reaction::Exit) {
            gate.note_exit_granted();
        }

        let counter = DispatchCounter::new(Reaction::Continue);
        assert_eq!(
            gate.terminate_reply(|| counter.dispatch()),
            NSApplicationTerminateReply::TerminateNow,
            "the close was already granted — the follow-up must not veto it"
        );
        assert_eq!(
            counter.calls.get(),
            0,
            "applicationShouldTerminate: must not re-dispatch WindowClose once \
             windowShouldClose: already resolved this close to Exit"
        );
    }

    /// Route 3 of [`ExitGate`]'s table, and the review-blocking one: an
    /// app-driven `Reaction::Exit` from an *ordinary* event (the
    /// `Key::Char('q') => Reaction::Exit` idiom in nearly every
    /// `examples/common/*.rs`, which has no `WindowClose` arm at all)
    /// funnels through `runtime::apply_outcome` → `request_exit` →
    /// `[NSApp terminate:]`, and `terminate:` always asks
    /// `applicationShouldTerminate:` first.
    ///
    /// Unguarded, that hook would dispatch a synthetic
    /// `UiEvent::WindowClose` the user never triggered, the example's
    /// catch-all would answer `Reaction::Continue`, and
    /// `terminate_reply_for_reaction` would turn that into
    /// `TerminateCancel` — the app silently failing to quit on `q`.
    ///
    /// `ExitTestSink` mirrors `QuadraView`'s real `ReactionSink::request_exit`
    /// with its one untestable line removed: the `[NSApp terminate:]` call
    /// needs a live `NSApplication` (see this module's doc). The
    /// `apply_outcome` leg in front of it is the real shared routing, so
    /// this covers `Reaction::Exit` → `request_exit` → gate → reply.
    #[test]
    fn app_driven_exit_does_not_resurface_as_a_synthetic_window_close() {
        struct ExitTestSink {
            gate: Rc<ExitGate>,
        }

        impl ReactionSink for ExitTestSink {
            fn request_redraw(&self) {}
            fn request_exit(&self) {
                // The line under test, verbatim from `QuadraView`.
                self.gate.note_exit_granted();
            }
            fn request_frame_in(&self, _delay: std::time::Duration) {}
        }

        let gate = Rc::new(ExitGate::default());
        let sink = ExitTestSink { gate: gate.clone() };

        // `q` pressed: the app's handler returned `Reaction::Exit`.
        runtime::apply_outcome(Reaction::Exit, &sink);

        // AppKit now calls `applicationShouldTerminate:` on the way out.
        // The example has no `WindowClose` arm, so a re-dispatch would
        // answer `Continue` → `TerminateCancel` and cancel the quit.
        let counter = DispatchCounter::new(Reaction::Continue);
        assert_eq!(
            gate.terminate_reply(|| counter.dispatch()),
            NSApplicationTerminateReply::TerminateNow,
            "an app that already returned Reaction::Exit must actually quit, not \
             be vetoed by a synthetic WindowClose its catch-all answers Continue to"
        );
        assert_eq!(
            counter.calls.get(),
            0,
            "no synthetic WindowClose may be dispatched for an event the user never \
             triggered — an app that *does* handle WindowClose would otherwise run \
             its unsaved-changes guard a second, spurious time"
        );
    }

    /// The inverse of the first route: a veto (anything but
    /// `Reaction::Exit`) must never mark the gate, because the window
    /// stays open — there is no last-window termination follow-up to
    /// guard against, and a later, independent Cmd-Q must still get its
    /// own fresh `UiEvent::WindowClose` dispatch.
    #[test]
    fn vetoed_window_close_leaves_terminate_dispatching_fresh() {
        let gate = ExitGate::default();
        for reaction in [
            Reaction::Continue,
            Reaction::Redraw,
            Reaction::RedrawAfter(std::time::Duration::from_millis(50)),
        ] {
            if window_should_close_for_reaction(reaction) {
                gate.note_exit_granted();
            }
        }
        assert!(
            !gate.exit_already_granted(),
            "a vetoed windowShouldClose: must not suppress a later, \
             independent applicationShouldTerminate: dispatch"
        );

        let counter = DispatchCounter::new(Reaction::Continue);
        assert_eq!(
            gate.terminate_reply(|| counter.dispatch()),
            NSApplicationTerminateReply::TerminateCancel
        );
        assert_eq!(counter.calls.get(), 1, "the app must be asked afresh");
    }

    /// Route 1 — a standalone Cmd-Q / menu-Quit request is preceded by
    /// neither `windowShouldClose:` nor `request_exit` (see
    /// `should_terminate`'s doc), so the gate is unmarked and the app is
    /// asked for real. `Reaction::Exit` from that fresh dispatch lets the
    /// process go.
    #[test]
    fn cmd_q_with_no_prior_grant_dispatches_fresh() {
        let gate = ExitGate::default();
        let counter = DispatchCounter::new(Reaction::Exit);
        assert_eq!(
            gate.terminate_reply(|| counter.dispatch()),
            NSApplicationTerminateReply::TerminateNow
        );
        assert_eq!(
            counter.calls.get(),
            1,
            "Cmd-Q must reach the app's WindowClose handler — that's the hole #951 exists to close"
        );
    }

    /// A fresh Cmd-Q whose handler vetoes (no explicit `Exit`) must keep
    /// the process alive *and* must not leave the gate marked, so a
    /// second Cmd-Q is still asked properly rather than silently granted.
    #[test]
    fn a_vetoed_cmd_q_does_not_grant_a_later_one() {
        let gate = ExitGate::default();
        let first = DispatchCounter::new(Reaction::Continue);
        assert_eq!(
            gate.terminate_reply(|| first.dispatch()),
            NSApplicationTerminateReply::TerminateCancel
        );

        let second = DispatchCounter::new(Reaction::Continue);
        assert_eq!(
            gate.terminate_reply(|| second.dispatch()),
            NSApplicationTerminateReply::TerminateCancel
        );
        assert_eq!(
            second.calls.get(),
            1,
            "each fresh Cmd-Q gets its own dispatch — a veto never marks the gate"
        );
    }
}

/// Coverage for `dispatch_event`'s `MouseDown`/`MouseMoved`/`MouseUp`
/// text-selection routing plus its Ctrl-C/Ctrl-A/`TextSelectionChanged`
/// pre-processing. None of this needs a live `CGContext` or running
/// `NSApplication` — the state machine and `dispatch_event` are plain Rust
/// logic once a `MacBackend` exists. `MacBackend::new()`'s default metrics
/// (16.0 line height, 8.0 char width) match other backends exactly, so the
/// same pixel math applies unchanged.
#[cfg(test)]
mod text_selection_dispatch_tests {
    use super::*;
    use crate::dispatch::TextRegion;
    use crate::event::Point;
    use crate::types::WidgetId;
    use crate::{ButtonMask, Key, Modifiers, MouseButton, UiEvent};

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

    fn region(id: &str, x: f32, y: f32, w: f32, h: f32, lines: &[&str]) -> TextRegion {
        TextRegion {
            id: WidgetId::new(id),
            bounds: crate::event::Rect::new(x, y, w, h),
            lines: lines.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn ctrl_char(c: char) -> UiEvent {
        UiEvent::KeyPressed {
            key: Key::Char(c),
            modifiers: Modifiers {
                ctrl: true,
                shift: false,
                alt: false,
                cmd: false,
            },
            repeat: false,
        }
    }

    /// Drive `dispatch_event` with a fresh caret-blink handle pair each
    /// call — the tests here don't care about caret-blink state, only
    /// about the text-selection pre-processing, so a scratch pair per
    /// call keeps each test's call sites terse (mirrors `MacDriver::dispatch`,
    /// which does the same thing against the live backend's own handles).
    fn dispatch(ev: UiEvent, backend: &mut MacBackend, app: &mut RecordingApp) -> EventOutcome {
        let caret_visible = backend.caret_visible_handle();
        let caret_pause = backend.caret_blink_pause_handle();
        dispatch_event(ev, backend, app, &caret_visible, &caret_pause)
    }

    /// End-to-end round trip: click-drag across a registered `TextRegion`,
    /// then Ctrl-C — the sequence for text selection and copy.
    #[test]
    fn drag_select_then_ctrl_c_copies_the_selection() {
        let mut backend = MacBackend::new();
        let mut app = RecordingApp::default();
        backend.register_text_region(region("body", 0.0, 0.0, 88.0, 16.0, &["hello world"]));

        // Press inside the region (row 0, col 0).
        let outcome = dispatch(
            UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(0.0, 4.0),
                modifiers: Modifiers::default(),
            },
            &mut backend,
            &mut app,
        );
        assert!(matches!(
            outcome,
            EventOutcome::Continue | EventOutcome::Redraw
        ));

        // Drag to the far right of the region (row 0, col 11 — "hello world" in full).
        let outcome = dispatch(
            UiEvent::MouseMoved {
                position: Point::new(88.0, 4.0),
                buttons: ButtonMask {
                    left: true,
                    ..ButtonMask::default()
                },
            },
            &mut backend,
            &mut app,
        );
        assert!(
            matches!(outcome, EventOutcome::Redraw),
            "a TextSelectionChanged event must force a redraw"
        );
        assert!(
            backend.active_text_selection().is_some(),
            "dragging across a registered TextRegion must produce an active selection"
        );

        // Release — ends the drag, does not clear the selection.
        let _ = dispatch(
            UiEvent::MouseUp {
                widget: None,
                button: MouseButton::Left,
                position: Point::new(88.0, 4.0),
            },
            &mut backend,
            &mut app,
        );
        assert!(
            backend.active_text_selection().is_some(),
            "MouseUp must not clear the finalised selection"
        );

        // Ctrl-C copies it, clears it, and delivers TextCopied.
        let outcome = dispatch(ctrl_char('c'), &mut backend, &mut app);
        assert!(matches!(
            outcome,
            EventOutcome::Redraw | EventOutcome::Continue
        ));
        assert!(
            backend.active_text_selection().is_none(),
            "Ctrl-C must clear the selection after copying it"
        );
        assert!(
            app.events
                .iter()
                .any(|e| matches!(e, UiEvent::TextCopied(text) if text == "hello world")),
            "Ctrl-C over a full-region selection must deliver TextCopied(\"hello world\"); \
             got {:?}",
            app.events
        );
    }

    #[test]
    fn ctrl_c_without_a_selection_falls_through_to_the_app() {
        let mut backend = MacBackend::new();
        let mut app = RecordingApp::default();
        let ev = ctrl_char('c');
        let _ = dispatch(ev.clone(), &mut backend, &mut app);
        assert_eq!(
            app.events,
            vec![ev],
            "Ctrl-C with no active selection must fall through to app.handle unchanged, \
             matching gtk::run::dispatch_event/win::run::dispatch_event"
        );
    }

    #[test]
    fn ctrl_a_selects_the_sole_registered_region() {
        let mut backend = MacBackend::new();
        let mut app = RecordingApp::default();
        backend.register_text_region(region("body", 0.0, 0.0, 40.0, 16.0, &["hi"]));
        let outcome = dispatch(ctrl_char('a'), &mut backend, &mut app);
        assert!(matches!(outcome, EventOutcome::Redraw));
        assert!(
            backend.active_text_selection().is_some(),
            "Ctrl-A must select the sole registered TextRegion"
        );
        assert!(
            app.events.is_empty(),
            "a recognised Ctrl-A must not also forward the raw KeyPressed to the app"
        );
    }

    #[test]
    fn ctrl_a_with_no_regions_falls_through_to_the_app() {
        let mut backend = MacBackend::new();
        let mut app = RecordingApp::default();
        let ev = ctrl_char('a');
        let _ = dispatch(ev.clone(), &mut backend, &mut app);
        assert_eq!(
            app.events,
            vec![ev],
            "Ctrl-A with no registered TextRegion must fall through to app.handle unchanged"
        );
    }

    #[test]
    fn mouse_down_clears_the_displayed_selection() {
        let mut backend = MacBackend::new();
        let mut app = RecordingApp::default();
        backend.set_active_text_selection(
            WidgetId::new("r"),
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
        );
        assert!(backend.active_text_selection().is_some());
        let ev = UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(0.0, 0.0),
            modifiers: Modifiers::default(),
        };
        let _ = dispatch(ev, &mut backend, &mut app);
        assert!(
            backend.active_text_selection().is_none(),
            "a MouseDown reaching dispatch_event must clear the displayed selection"
        );
    }

    #[test]
    fn text_selection_changed_updates_active_selection_and_forces_redraw() {
        let mut backend = MacBackend::new();
        let mut app = RecordingApp::default();
        backend.register_text_region(region("body", 0.0, 0.0, 40.0, 16.0, &["hi there"]));
        let ev = UiEvent::TextSelectionChanged {
            region: WidgetId::new("body"),
            anchor: Point::new(0.0, 0.0),
            focus: Point::new(40.0, 0.0),
        };
        let outcome = dispatch(ev, &mut backend, &mut app);
        assert!(matches!(outcome, EventOutcome::Redraw));
        assert!(backend.active_text_selection().is_some());
    }

    /// Before this, `macos::run::dispatch_event` had no
    /// middle-click step at all — only GTK read the PRIMARY selection.
    /// `MacClipboard` never overrides
    /// [`crate::backend::Clipboard::read_primary_selection`] (macOS has
    /// no PRIMARY-selection concept), so the shared `preprocess_event`
    /// step this backend now runs via `dispatch_event`'s `MouseDown`
    /// routing is always the "nothing to paste" branch — the click must
    /// still reach the app as an ordinary `MouseDown`, matching
    /// `gtk::run`/`win::run`'s identical fallthrough.
    #[test]
    fn middle_click_falls_through_to_the_app_no_primary_selection_on_macos() {
        let mut backend = MacBackend::new();
        let mut app = RecordingApp::default();
        let ev = UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Middle,
            position: Point::new(5.0, 5.0),
            modifiers: Modifiers::default(),
        };
        let _ = dispatch(ev, &mut backend, &mut app);
        assert!(
            app.events.iter().any(|e| matches!(
                e,
                UiEvent::MouseDown {
                    button: MouseButton::Middle,
                    ..
                }
            )),
            "middle-click must fall through to app.handle as an ordinary MouseDown when \
             there is no PRIMARY selection to paste, got {:?}",
            app.events
        );
    }
}
