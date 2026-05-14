//! macOS runner — opens an AppKit window, installs a custom `NSView`
//! subclass that forwards each `drawRect:` to a Core Graphics fill of
//! the bounds, and pumps the AppKit run loop.
//!
//! Smoke-only for issue #32: there is no `AppLogic` integration, no
//! event translation, no Core Text. Subsequent tickets layer those on:
//!
//! - #33 — `NSEvent → UiEvent` translation (overrides on `QuadraView`).
//! - #34 — Core Text font metrics + `draw_text`.
//! - #35 — `MacBackend` struct, `Backend` trait impl, and the final
//!   `pub fn run<A: AppLogic + 'static>(app: A) -> ExitCode` shape that
//!   threads `app.render(backend, AreaId::default())` through this
//!   draw callback.
//!
//! The retina backing factor is read from the window each frame and
//! packed into [`crate::Viewport::scale`] — the only piece of state
//! #32 surfaces to its (currently empty) frame body.

use std::cell::Cell;

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
    NSGraphicsContext, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};

use crate::event::Viewport;

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

/// Per-view state tracked across `drawRect:` calls. `last_viewport`
/// captures the most recent viewport so future tickets (#35) can
/// inspect it without re-querying AppKit, and so tests can assert
/// scale plumbing without spinning up a run loop.
pub(crate) struct QuadraViewIvars {
    last_viewport: Cell<Viewport>,
}

declare_class!(
    /// Quadraui's custom `NSView`. Overrides `drawRect:` to obtain a
    /// `CGContextRef` from the current [`NSGraphicsContext`] and fill
    /// the view bounds with a flat background colour.
    pub(crate) struct QuadraView;

    // SAFETY:
    // - NSView is documented to be subclassable for custom drawing.
    // - MainThreadOnly: AppKit views must be created + used on the
    //   main thread; `QuadraView::new` enforces this via the
    //   `MainThreadMarker` argument.
    // - `QuadraView` doesn't implement Drop — ivars are POD-ish
    //   (`Cell<Viewport>` is `Copy` inside).
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
            // The typed wrappers in objc2-app-kit don't expose
            // `CGContext` (it returns a CoreFoundation pointer that
            // sits outside the objc2 type system). We drop to
            // `msg_send!` with a custom opaque return type whose
            // encoding (`^{CGContext=}`) matches the ObjC method
            // signature — `*mut c_void` (`^v`) panics under objc2's
            // debug-mode encoding check.
            let cg_opaque: *mut OpaqueCGContext = unsafe { msg_send![&*gctx, CGContext] };
            if cg_opaque.is_null() {
                return;
            }
            let cg_ref: CGContextRef = cg_opaque.cast();

            // `NSRect` (objc2-foundation) and `CGRect` (core-graphics)
            // are layout-compatible but distinct Rust types — convert
            // by re-constructing.
            let rect = CGRect::new(
                &CGPoint::new(bounds.origin.x, bounds.origin.y),
                &CGSize::new(bounds.size.width, bounds.size.height),
            );

            // Flat dark grey — picked to make it obvious the view is
            // painting (vs. the AppKit default white that would show
            // through if `drawRect:` no-op'd). The theme integration
            // proper lands in #35 alongside `MacBackend`.
            //
            // We call CoreGraphics directly here rather than wrap the
            // borrowed pointer — `core_graphics::context`'s safe
            // wrappers want ownership semantics that don't fit a
            // pointer AppKit reclaims when `drawRect:` returns.
            //
            // SAFETY: `cg_ref` is a non-null `CGContextRef` owned by
            // AppKit for the duration of this call; CoreGraphics is
            // linked transitively via the `core-graphics` crate.
            unsafe {
                CGContextSetRGBFillColor(cg_ref, 0.12, 0.12, 0.14, 1.0);
                CGContextFillRect(cg_ref, rect);
            }
        }

        /// Use top-left origin to match the rest of the library
        /// (TUI + GTK both lay out from the top-left).
        #[method(isFlipped)]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl QuadraView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(QuadraViewIvars {
            last_viewport: Cell::new(Viewport::default()),
        });
        unsafe { msg_send_id![super(this), init] }
    }
}

declare_class!(
    /// Minimal `NSApplicationDelegate` so the smoke harness terminates
    /// when the last window closes (red traffic-light button quits the
    /// process). Without a delegate, `NSApplication::run` keeps
    /// pumping the loop forever — leaving no exit short of `kill`,
    /// since #32 doesn't yet wire `[NSApp terminate:]` to a menu or
    /// keystroke. #35 replaces this with a delegate that bridges
    /// AppKit notifications into [`crate::Reaction::Exit`].
    pub(crate) struct QuadraAppDelegate;

    // SAFETY:
    // - NSObject has no subclassing requirements.
    // - MainThreadOnly: application delegates must live on the main thread.
    // - No Drop impl.
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
        // No state, but `set_ivars(())` is still required to advance
        // the allocation to `PartialInit` before the `super init` call.
        let this = this.set_ivars(());
        unsafe { msg_send_id![super(this), init] }
    }
}

/// Open an AppKit window, install a `QuadraView`, and run the
/// `NSApplication` event loop until the user closes the window.
///
/// Returns [`std::process::ExitCode::SUCCESS`] when the loop exits
/// cleanly. **Must be called from the main thread.**
///
/// This is the smoke-test entry point for issue #32. Once #35 lands,
/// this signature will change to `run<A: AppLogic + 'static>(app: A)`
/// and `drawRect:` will dispatch to `app.render(...)` via
/// `MacBackend::enter_frame_scope`. Today it paints a flat colour and
/// proves AppKit / Core Graphics / Retina plumbing all line up.
pub fn run() -> std::process::ExitCode {
    let mtm =
        MainThreadMarker::new().expect("quadraui::macos::run must be called from the main thread");

    let app = NSApplication::sharedApplication(mtm);
    let _ = app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    // Install the delegate before run() so the first window-close
    // triggers terminate. The delegate is retained by NSApp.
    let delegate = QuadraAppDelegate::new(mtm);
    let delegate_proto = ProtocolObject::from_ref(&*delegate);
    app.setDelegate(Some(delegate_proto));

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
    window.setTitle(&NSString::from_str("quadraui (macos smoke)"));

    let view = QuadraView::new(mtm);
    window.setContentView(Some(&view));
    window.makeKeyAndOrderFront(None);

    // Bring the app to the foreground when launched from a terminal,
    // otherwise the window opens behind the calling Terminal.
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);

    // SAFETY: blocks on AppKit run loop; returns when the last
    // window closes or `[NSApp terminate:]` is invoked.
    unsafe { app.run() };

    std::process::ExitCode::SUCCESS
}
