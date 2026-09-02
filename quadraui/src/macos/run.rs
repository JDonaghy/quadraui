//! macOS runner — boots `NSApplication`, opens an `NSWindow`, installs
//! a custom `NSView` (`QuadraView`) that bridges AppKit's
//! responder-chain + `drawRect:` model into [`crate::AppLogic`]'s
//! `setup` / `render` / `handle` shape.
//!
//! Issue #35 ties together the foundation work from #32–#34 into the
//! final `run<A: AppLogic + 'static>(app)` signature. Subsequent
//! per-primitive rasteriser tickets (#38–#43) fill in the `draw_*`
//! stubs on [`super::backend::MacBackend`]; nothing in this file
//! changes when those land.
//!
//! ## Type-erasure shape
//!
//! `QuadraView` is declared via [`objc2::declare_class!`], which
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
//! - `handle: Box<dyn Fn(UiEvent) -> Reaction + 'static>` — invoked by
//!   every responder override (mouse, scroll, key) after the
//!   [`super::events`] translator produces a `UiEvent`. Calls
//!   `app.handle(ev, &mut *backend)` and returns the reaction. The
//!   responder dispatches `Reaction` synchronously through
//!   [`QuadraView::apply_reaction`] — `Redraw` → `setNeedsDisplay`,
//!   `Exit` → `[NSApp terminate:]`.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::rc::Rc;

use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::sys::CGContextRef;
use objc2::declare_class;
use objc2::encode::{Encoding, RefEncode};
use objc2::msg_send;
use objc2::msg_send_id;
use objc2::mutability;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::sel;
use objc2::ClassType;
use objc2::DeclaredClass;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSBackingStoreType,
    NSEvent, NSGraphicsContext, NSView, NSViewFrameDidChangeNotification, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSNotificationCenter, NSObject, NSObjectProtocol, NSPoint,
    NSRect, NSSize, NSString,
};

use super::backend::MacBackend;
use super::events::{ns_key_to_uievent, ns_mouse_down, ns_mouse_moved, ns_mouse_up, ns_scroll};
use super::text::make_font;
use crate::backend::Backend;
use crate::event::Viewport;
use crate::runner::{AppLogic, Reaction};
use crate::runtime::{self, ReactionSink};
// Re-exported (not just imported) so `macos::testing` — and any other
// in-crate caller that historically reached `EventOutcome` through this
// module — keeps working unchanged after the type moved to
// `crate::runtime` (quadraui#496).
pub(crate) use crate::runtime::EventOutcome;
use crate::{ButtonMask, Key, Modifiers, UiEvent};

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

/// Type-erased closures the view invokes from its responder + draw
/// callbacks. Built once per [`run`] call from the concrete `A:
/// AppLogic`; from `declare_class!`'s perspective they're just two
/// `Box<dyn Fn>` smart pointers.
type PaintFn = Box<dyn Fn(Viewport, CGContextRef) + 'static>;
type HandleFn = Box<dyn Fn(UiEvent) -> Reaction + 'static>;

// `EventOutcome` — what the caller should do after [`dispatch_event`]
// handles one event — is defined once in `crate::runtime` and shared by
// every backend runner (quadraui#496); imported at the top of this file
// (re-exported so `macos::testing` keeps reaching it through this path).

/// Dispatch one already-translated [`UiEvent`] through the app, applying
/// the runner's built-in pre-processing first — same funnel both the live
/// `QuadraView` responder methods (via [`run`]'s `handle` closure) and
/// [`super::testing::MacDriver`] (quadraui#493) route through, so a test
/// exercises the exact pre-processing a real keypress/click gets. Mirrors
/// [`crate::gtk::run::dispatch_event`].
///
/// Pre-processing handled here, in order:
/// - Caret-blink bump: any `KeyPressed`/`CharTyped` makes the caret solid
///   for ~500ms (`caret_visible`/`caret_pause`), matching AppKit's
///   text-field typing convention.
/// - Modal click dispatch (#493): `MouseDown` routes through
///   [`crate::dispatch::dispatch_click`] so the backend's `ModalStack`
///   arbitrates first — a click inside an open modal is tagged with the
///   modal's `WidgetId`, and a click outside every modal dismisses the
///   topmost instead of falling through to the widget underneath.
///   Scroll surfaces and text regions are not tracked by `MacBackend`
///   yet (see its `capabilities` doc), hence the empty slices. Mirrors
///   `TuiBackend::apply_dispatch` and `gtk::run`'s `connect_pressed`
///   closure.
/// - Double-click folding (#486): `MouseDown` → `DoubleClick` within the
///   detector's time/position window. Runs on the *dispatched* events,
///   after modal arbitration — same relative order as
///   `TuiBackend::translate_events`.
/// - ActivityBar keyboard-focus redirect (#465): while the last painted
///   frame contained an `ActivityBar` with `is_keyboard_focused = true`,
///   every `KeyPressed` becomes
///   `UiEvent::ActivityBar(bar_id, ActivityBarEvent::KeyPressed { … })`
///   instead of reaching the app as a raw key. This must win over
///   accelerator dispatch below (same ordering `gtk::run::dispatch_event`
///   uses), otherwise a bound accelerator would steal a navigation key
///   out from under the focused bar.
/// - Global accelerator dispatch (#486): a `KeyPressed` matching a
///   registered `Global`-scope accelerator becomes `UiEvent::Accelerator`.
/// - Cmd-V paste interception (#486): AppKit has no native paste signal
///   on a bespoke `NSView`, so a plain Cmd-V reads the system clipboard
///   directly and delivers `ClipboardPaste` instead of forwarding the raw
///   key press.
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

    // Modal click dispatch (#493): every mouse-down consults the
    // `ModalStack` via the shared dispatch layer before the app sees it,
    // so a click inside an open dialog stays inside the dialog and a
    // click outside dismisses it — instead of falling straight through
    // to whatever widget is underneath. `dispatch_click` may return 0..n
    // events (e.g. dismiss emits `MouseDown` + `Palette(Closed)`); each
    // is double-click-folded, handed to the app, and the outcomes are
    // folded the way `gtk::run`'s click loop folds them: `Exit` wins
    // immediately, else `Redraw` wins over `Continue`.
    if let UiEvent::MouseDown {
        button,
        position,
        modifiers,
        ..
    } = &event
    {
        let (button, position, modifiers) = (*button, *position, *modifiers);
        let dispatched = {
            let (drag_state, modal_stack) = backend.drag_and_modal_mut();
            crate::dispatch::dispatch_click(
                modal_stack,
                &[], // scroll surfaces not tracked by MacBackend yet
                &[], // text regions not tracked by MacBackend yet
                drag_state,
                position,
                button,
                modifiers,
            )
        };
        let mut outcome = EventOutcome::Continue;
        for ev in dispatched {
            let ev = backend.fold_double_click(ev);
            match app.handle(ev, backend).into() {
                EventOutcome::Exit => return EventOutcome::Exit,
                EventOutcome::Redraw => outcome = EventOutcome::Redraw,
                EventOutcome::Continue => {}
            }
        }
        return outcome;
    }

    // `MouseDown` is the only variant `fold_double_click` acts on, and
    // every `MouseDown` returned above — so the remaining pipeline
    // (activity-bar redirect, accelerators, Cmd-V paste, plain forward)
    // skips the fold.

    // ── ActivityBar keyboard focus intercept (#465) ──────────────────
    //
    // `AppShell`/`ShellAdapter`'s built-in activity-bar keyboard cursor
    // (#409) is driven by `UiEvent::ActivityBar(id, KeyPressed { … })`,
    // which is *synthesized by the backend* — `TuiBackend::apply_dispatch`
    // and `gtk::run::dispatch_event` both do it. Without this arm the
    // macOS backend delivered the raw `KeyPressed` instead, so `j`/`k`/
    // `Enter` fell through to `ShellApp::handle` and the shell's cursor
    // never moved: every `ShellApp` on macOS (#465's `run_with_shell`)
    // silently lost keyboard navigation the other two backends have.
    //
    // Ordered before accelerator matching, matching `gtk::run`.
    if let UiEvent::KeyPressed {
        ref key, modifiers, ..
    } = event
    {
        if let Some(bar_id) = backend.focused_activity_bar_id().cloned() {
            let key_str = crate::primitives::activity_bar::key_to_activity_bar_string(key);
            let bar_ev = UiEvent::ActivityBar(
                bar_id,
                crate::ActivityBarEvent::KeyPressed {
                    key: key_str,
                    modifiers,
                },
            );
            return app.handle(bar_ev, backend).into();
        }
    }

    let event = if let UiEvent::KeyPressed { key, modifiers, .. } = &event {
        match backend.match_keypress(key, *modifiers) {
            Some(id) => UiEvent::Accelerator(id, *modifiers),
            None => event,
        }
    } else {
        event
    };

    // Cmd (not Ctrl) is the Mac paste modifier.
    if let UiEvent::KeyPressed {
        key: Key::Char('v') | Key::Char('V'),
        modifiers:
            Modifiers {
                cmd: true,
                shift: false,
                alt: false,
                ctrl: false,
            },
        ..
    } = &event
    {
        return if let Some(text) = backend.services().clipboard().read_text() {
            app.handle(UiEvent::ClipboardPaste(text), backend).into()
        } else {
            EventOutcome::Continue
        };
    }

    app.handle(event, backend).into()
}

/// Render one frame: `begin_frame` + [`MacBackend::enter_frame_scope`] +
/// `end_frame` — the exact body `run`'s `paint` closure used to run
/// inline, extracted so it never depends on a live `NSView`, only a
/// [`Viewport`] + a borrowed `CGContextRef`. Shared by the live runner and
/// [`super::testing::MacDriver`] (quadraui#493) — mirrors
/// [`crate::gtk::run::render_frame`].
pub(crate) fn render_frame<A: AppLogic>(
    backend: &mut MacBackend,
    app: &A,
    viewport: Viewport,
    ctx: CGContextRef,
) {
    backend.begin_frame(viewport);
    backend.enter_frame_scope(ctx, |b| {
        app.render(b, <A as AppLogic>::AreaId::default());
    });
    backend.end_frame();
}

/// `QuadraView`'s per-instance state. `last_viewport` is retained for
/// diagnostics + future paint↔click harness work; `paint` / `handle`
/// are the AppLogic bridge.
pub(crate) struct QuadraViewIvars {
    last_viewport: Cell<Viewport>,
    paint: PaintFn,
    handle: HandleFn,
}

declare_class!(
    /// Quadraui's custom `NSView`. `drawRect:` resolves viewport +
    /// CG context, paints a debug background (theme-defaulted grey
    /// + a #34 smoke label), then delegates to the stored `paint`
    /// closure so the active [`AppLogic`] can render on top.
    /// Responder methods translate `NSEvent` → [`UiEvent`] and route
    /// through the stored `handle` closure for `AppLogic::handle`.
    pub(crate) struct QuadraView;

    // SAFETY:
    // - NSView is documented to be subclassable for custom drawing.
    // - MainThreadOnly: AppKit views must be created + used on the
    //   main thread; `QuadraView::new` enforces this via the
    //   `MainThreadMarker` argument.
    // - `QuadraView` doesn't implement Drop — its ivars hold owned
    //   `Box<dyn Fn>` smart pointers that drop cleanly when the
    //   class instance is finalized by the Obj-C runtime.
    unsafe impl ClassType for QuadraView {
        type Super = NSView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "QuadraUiView";
    }

    impl DeclaredClass for QuadraView {
        type Ivars = QuadraViewIvars;
    }

    unsafe impl QuadraView {
        #[method(drawRect:)]
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

            // SAFETY: `drawRect:` is always invoked inside a valid
            // graphics scope, so `currentContext` returns `Some`.
            let Some(gctx) = (unsafe { NSGraphicsContext::currentContext() }) else {
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
            // a consistent backdrop until content rasterisers (#39+)
            // fill the middle. Removed once content rasterisers paint
            // the full client area.
            //
            // SAFETY: `cg_ref` is a non-null `CGContextRef` borrowed
            // for the duration of this call.
            unsafe {
                CGContextSetRGBFillColor(cg_ref, 0.12, 0.12, 0.14, 1.0);
                CGContextFillRect(cg_ref, rect);
            }

            // Now run the app's render via the stored closure.
            // `paint` is responsible for `begin_frame` / frame-scope /
            // `end_frame` orchestration so this method doesn't need
            // to know anything about the concrete `A`.
            (self.ivars().paint)(viewport, cg_ref);
        }

        /// Top-left origin to match TUI + GTK conventions.
        #[method(isFlipped)]
        fn is_flipped(&self) -> bool {
            true
        }

        /// Required so AppKit routes `keyDown:` here.
        #[method(acceptsFirstResponder)]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        // ── Mouse press / release ────────────────────────────────

        #[method(mouseDown:)]
        fn objc_mouse_down(&self, event: &NSEvent) {
            let (x, y, flags) = self.locate(event);
            let button = unsafe { event.buttonNumber() } as i64;
            self.dispatch(ns_mouse_down(button, x, y, flags));
        }

        #[method(rightMouseDown:)]
        fn objc_right_mouse_down(&self, event: &NSEvent) {
            let (x, y, flags) = self.locate(event);
            let button = unsafe { event.buttonNumber() } as i64;
            self.dispatch(ns_mouse_down(button, x, y, flags));
        }

        #[method(otherMouseDown:)]
        fn objc_other_mouse_down(&self, event: &NSEvent) {
            let (x, y, flags) = self.locate(event);
            let button = unsafe { event.buttonNumber() } as i64;
            self.dispatch(ns_mouse_down(button, x, y, flags));
        }

        #[method(mouseUp:)]
        fn objc_mouse_up(&self, event: &NSEvent) {
            let (x, y, _flags) = self.locate(event);
            let button = unsafe { event.buttonNumber() } as i64;
            self.dispatch(ns_mouse_up(button, x, y));
        }

        #[method(rightMouseUp:)]
        fn objc_right_mouse_up(&self, event: &NSEvent) {
            let (x, y, _flags) = self.locate(event);
            let button = unsafe { event.buttonNumber() } as i64;
            self.dispatch(ns_mouse_up(button, x, y));
        }

        #[method(otherMouseUp:)]
        fn objc_other_mouse_up(&self, event: &NSEvent) {
            let (x, y, _flags) = self.locate(event);
            let button = unsafe { event.buttonNumber() } as i64;
            self.dispatch(ns_mouse_up(button, x, y));
        }

        // ── Mouse move / drag ────────────────────────────────────

        #[method(mouseMoved:)]
        fn objc_mouse_moved(&self, event: &NSEvent) {
            let (x, y, _flags) = self.locate(event);
            self.dispatch(ns_mouse_moved(x, y, ButtonMask::default()));
        }

        #[method(mouseDragged:)]
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

        #[method(rightMouseDragged:)]
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

        #[method(otherMouseDragged:)]
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

        #[method(scrollWheel:)]
        fn objc_scroll_wheel(&self, event: &NSEvent) {
            let (x, y, _flags) = self.locate(event);
            // SAFETY: scrollingDeltaX/Y are safe on a scroll event.
            let dx = unsafe { event.scrollingDeltaX() };
            let dy = unsafe { event.scrollingDeltaY() };
            self.dispatch(ns_scroll(dx, dy, x, y));
        }

        #[method(keyDown:)]
        fn objc_key_down(&self, event: &NSEvent) {
            let flags = unsafe { event.modifierFlags() }.0;
            let key_code = unsafe { event.keyCode() };
            let repeat = unsafe { event.isARepeat() };
            let chars_ns = unsafe { event.characters() };
            let chars_str = chars_ns.as_ref().map(|s| s.to_string());
            if let Some(ev) = ns_key_to_uievent(chars_str.as_deref(), key_code, flags, repeat) {
                self.dispatch(ev);
            }
        }

        // ── Window resize (#486) ─────────────────────────────────

        /// Registered (in [`run`]) as the observer for
        /// `NSViewFrameDidChangeNotification`, which fires whenever this
        /// view's `frame` changes — live window resize, programmatic
        /// resize, and the split-second the window first opens. Compares
        /// against `last_viewport` (also updated here) so a notification
        /// that doesn't actually change the size — AppKit can post one
        /// on origin-only moves — doesn't spam `AppLogic::handle` with a
        /// same-size `WindowResized`.
        #[method(viewFrameDidChange:)]
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
            self.dispatch(UiEvent::WindowResized { viewport });
        }
    }
);

impl QuadraView {
    fn new(mtm: MainThreadMarker, paint: PaintFn, handle: HandleFn) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(QuadraViewIvars {
            last_viewport: Cell::new(Viewport::default()),
            paint,
            handle,
        });
        unsafe { msg_send_id![super(this), init] }
    }

    /// Convert `NSEvent.locationInWindow` into view-local coordinates
    /// and return `(x, y, modifier_flags)`. Top-left origin matches
    /// the rest of quadraui because `isFlipped` is true.
    fn locate(&self, event: &NSEvent) -> (f64, f64, usize) {
        // SAFETY: NSResponder callbacks run on the main thread inside
        // an active event scope.
        let win_pt = unsafe { event.locationInWindow() };
        let view_pt = self.convertPoint_fromView(win_pt, None);
        let flags = unsafe { event.modifierFlags() }.0;
        (view_pt.x, view_pt.y, flags)
    }

    /// Route a translated [`UiEvent`] through `AppLogic::handle` and
    /// act on the returned [`Reaction`].
    fn dispatch(&self, ev: UiEvent) {
        let reaction = (self.ivars().handle)(ev);
        self.apply_reaction(reaction);
    }

    /// Apply a [`Reaction`] — delegates to the shared
    /// [`runtime::apply_outcome`] (quadraui#496) via this view's
    /// [`ReactionSink`] impl below.
    fn apply_reaction(&self, reaction: Reaction) {
        runtime::apply_outcome(reaction, self);
    }
}

impl ReactionSink for QuadraView {
    fn request_redraw(&self) {
        // SAFETY: `setNeedsDisplay:` on the main thread is the documented
        // way to schedule a repaint.
        unsafe { self.setNeedsDisplay(true) }
    }

    fn request_exit(&self) {
        let mtm = MainThreadMarker::from(self);
        let app = NSApplication::sharedApplication(mtm);
        // SAFETY: `terminate:` on NSApp on the main thread is the
        // documented exit path.
        unsafe { app.terminate(None) };
    }
}

declare_class!(
    /// Minimal `NSApplicationDelegate` — terminate the process when
    /// the last window closes (red traffic-light → exit). #36 may
    /// extend this with notification + URL-scheme handling.
    pub(crate) struct QuadraAppDelegate;

    unsafe impl ClassType for QuadraAppDelegate {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "QuadraUiAppDelegate";
    }

    impl DeclaredClass for QuadraAppDelegate {}

    unsafe impl NSObjectProtocol for QuadraAppDelegate {}

    unsafe impl NSApplicationDelegate for QuadraAppDelegate {
        #[method(applicationShouldTerminateAfterLastWindowClosed:)]
        fn should_terminate_after_last_window(&self, _sender: &NSApplication) -> bool {
            true
        }
    }
);

impl QuadraAppDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(());
        unsafe { msg_send_id![super(this), init] }
    }
}

/// Open an AppKit window, install a [`MacBackend`], and drive `app`
/// against it. Returns when the user closes the window (red
/// traffic-light) or `app.handle` returns [`Reaction::Exit`].
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
    let mtm =
        MainThreadMarker::new().expect("quadraui::macos::run must be called from the main thread");

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
            render_frame(&mut *backend_mut, &*app_ref, viewport, cg_ref);
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
        Box::new(move |ev: UiEvent| -> Reaction {
            let mut backend_mut = backend.borrow_mut();
            let mut app_mut = app.borrow_mut();
            match dispatch_event(
                ev,
                &mut *backend_mut,
                &mut *app_mut,
                &caret_visible,
                &caret_pause,
            ) {
                EventOutcome::Continue => Reaction::Continue,
                EventOutcome::Redraw => Reaction::Redraw,
                EventOutcome::Exit => Reaction::Exit,
            }
        })
    };

    // ── AppKit bootstrap ─────────────────────────────────────────
    let ns_app = NSApplication::sharedApplication(mtm);
    let _ = ns_app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    let delegate = QuadraAppDelegate::new(mtm);
    let delegate_proto = ProtocolObject::from_ref(&*delegate);
    ns_app.setDelegate(Some(delegate_proto));

    let content_rect = NSRect::new(NSPoint::new(120.0, 120.0), NSSize::new(800.0, 600.0));
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Resizable
        | NSWindowStyleMask::Miniaturizable;
    let window: Retained<NSWindow> = unsafe {
        msg_send_id![
            mtm.alloc::<NSWindow>(),
            initWithContentRect: content_rect,
            styleMask: style,
            backing: NSBackingStoreType::NSBackingStoreBuffered,
            defer: false,
        ]
    };
    window.setTitle(&NSString::from_str("quadraui (macos)"));

    let view = QuadraView::new(mtm, paint, handle);
    window.setContentView(Some(&view));
    window.setAcceptsMouseMovedEvents(true);
    window.makeFirstResponder(Some(view.as_super()));
    window.makeKeyAndOrderFront(None);

    // WindowResized wiring (#486): opt the view into frame-change
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

    // Drive the InlineInput caret blink (#188). The pair is held as
    // locals for the lifetime of the app — when this function
    // eventually returns, the NSTimer's `Retained` drops, the timer
    // is released by the run loop, and the target finalises.
    let (_blink_target, _blink_timer) =
        super::caret_blink::install_blink_timer(mtm, caret_visible, caret_pause);

    #[allow(deprecated)]
    ns_app.activateIgnoringOtherApps(true);

    // SAFETY: blocks on AppKit run loop; returns when the last
    // window closes or `[NSApp terminate:]` is invoked.
    unsafe { ns_app.run() };

    std::process::ExitCode::SUCCESS
}

// Suppress unused warning for the c_void import — kept available for
// future opaque-pointer dancing in this file as it grows.
#[allow(dead_code)]
fn _unused_imports(_p: *mut c_void) {}
