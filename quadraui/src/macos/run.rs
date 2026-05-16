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
use objc2::runtime::ProtocolObject;
use objc2::ClassType;
use objc2::DeclaredClass;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSBackingStoreType,
    NSEvent, NSGraphicsContext, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};

use super::backend::MacBackend;
use super::events::{ns_key_to_uievent, ns_mouse_down, ns_mouse_moved, ns_mouse_up, ns_scroll};
use super::text::make_font;
use crate::backend::Backend;
use crate::event::Viewport;
use crate::runner::{AppLogic, Reaction};
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

/// Type-erased closures the view invokes from its responder + draw
/// callbacks. Built once per [`run`] call from the concrete `A:
/// AppLogic`; from `declare_class!`'s perspective they're just two
/// `Box<dyn Fn>` smart pointers.
type PaintFn = Box<dyn Fn(Viewport, CGContextRef) + 'static>;
type HandleFn = Box<dyn Fn(UiEvent) -> Reaction + 'static>;

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

    fn apply_reaction(&self, reaction: Reaction) {
        match reaction {
            Reaction::Continue => {}
            Reaction::Redraw => unsafe { self.setNeedsDisplay(true) },
            Reaction::Exit => {
                let mtm = MainThreadMarker::from(self);
                let app = NSApplication::sharedApplication(mtm);
                // SAFETY: `terminate:` on NSApp on the main thread is
                // the documented exit path.
                unsafe { app.terminate(None) };
            }
        }
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
            backend_mut.begin_frame(viewport);
            backend_mut.enter_frame_scope(cg_ref, |b| {
                let app_ref = app.borrow();
                app_ref.render(b, <A as AppLogic>::AreaId::default());
            });
            backend_mut.end_frame();
        })
    };
    let handle: HandleFn = {
        let app = app.clone();
        let backend = backend.clone();
        Box::new(move |ev: UiEvent| -> Reaction {
            let mut backend_mut = backend.borrow_mut();
            let mut app_mut = app.borrow_mut();
            app_mut.handle(ev, &mut *backend_mut)
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
