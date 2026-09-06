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
// objc2 0.6 (#796) removed `declare_class!` in favour of `define_class!`
// (superclass/mutability move from a `ClassType` impl block onto struct
// attributes), renamed `DeclaredClass` to `DefinedClass`, and deprecated
// `msg_send_id!` in favour of `msg_send!` (which now performs the same
// `Retained` conversion itself — see that macro's own doc).
use objc2::encode::{Encoding, RefEncode};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, sel, ClassType, DefinedClass, MainThreadOnly, Message};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSBackingStoreType,
    NSEvent, NSGraphicsContext, NSView, NSViewFrameDidChangeNotification, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSNotificationCenter, NSObject, NSObjectProtocol, NSPoint,
    NSRect, NSSize, NSString, NSTimer,
};

use super::backend::MacBackend;
use super::events::{ns_key_to_uievent, ns_mouse_down, ns_mouse_moved, ns_mouse_up, ns_scroll};
use super::text::make_font;
use crate::backend::Backend;
use crate::dispatch::DragTarget;
use crate::event::Viewport;
use crate::runner::{AppLogic, Reaction};
use crate::runtime::{self, ReactionSink, ResizeDebouncer, RESIZE_SETTLE};
// Re-exported (not just imported) so `macos::testing` — and any other
// in-crate caller that historically reached `EventOutcome` through this
// module — keeps working unchanged after the type moved to
// `crate::runtime` (quadraui#496).
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

/// Type-erased closures the view invokes from its responder + draw
/// callbacks. Built once per [`run`] call from the concrete `A:
/// AppLogic`; from `define_class!`'s perspective they're just two
/// `Box<dyn Fn>` smart pointers.
type PaintFn = Box<dyn Fn(Viewport, CGContextRef) + 'static>;
type HandleFn = Box<dyn Fn(UiEvent) -> Reaction + 'static>;

// `EventOutcome` — what the caller should do after [`dispatch_event`]
// handles one event — is defined once in `crate::runtime` and shared by
// every backend runner (quadraui#496); imported at the top of this file
// (re-exported so `macos::testing` keeps reaching it through this path).

/// Dispatch one already-translated [`UiEvent`] through the app, applying
/// the shared runner pre-processing pipeline first — same funnel both the
/// live `QuadraView` responder methods (via [`run`]'s `handle` closure)
/// and [`super::testing::MacDriver`] (quadraui#493) route through, so a
/// test exercises the exact pre-processing a real keypress/click gets.
///
/// Pre-processing handled here, in order:
/// - Caret-blink bump: any `KeyPressed`/`CharTyped` makes the caret solid
///   for ~500ms (`caret_visible`/`caret_pause`), matching AppKit's
///   text-field typing convention. macOS-only — no other backend paints
///   a blinking caret through this mechanism.
/// - Modal click dispatch (#493): `MouseDown` routes through
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
/// - `MouseMoved` (#803): routes through
///   [`crate::dispatch::dispatch_mouse_drag`] so an in-progress
///   `TextSelection`/scrollbar/split-divider drag (armed by the
///   `MouseDown` branch above) emits its synthetic event alongside the
///   plain `MouseMoved`, mirroring `gtk::run`'s motion controller /
///   `win::run::route_mouse_move`.
/// - `MouseUp` (#803): routes through [`crate::dispatch::dispatch_mouse_up`]
///   so an in-progress drag ends cleanly.
///
/// Everything else — double-click folding, ActivityBar keyboard-focus
/// redirect, global accelerator rewrite, Ctrl-C copy, Cmd-V/Cmd-Shift-V
/// paste, middle-click PRIMARY-selection paste (a no-op on macOS — see
/// [`crate::backend::Clipboard::read_primary_selection`]'s default),
/// Ctrl-A select-all, selection-display clearing, `TextSelectionChanged`
/// — lives in [`crate::runtime::preprocess_event`] (quadraui#813), shared
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

    // Modal click dispatch (#493): every mouse-down consults the
    // `ModalStack` via the shared dispatch layer before the app sees it,
    // so a click inside an open dialog stays inside the dialog and a
    // click outside dismisses it — instead of falling straight through
    // to whatever widget is underneath. `dispatch_click` may return 0..n
    // events (e.g. dismiss emits `MouseDown` + `Palette(Closed)`); each
    // goes through the shared `preprocess_event` (which folds double-
    // clicks, clears the selection display, etc. — see this function's
    // doc), and the outcomes are folded the way `gtk::run`'s click loop
    // folds them: `Exit` wins immediately, else `Redraw` wins over
    // `Continue`.
    if let UiEvent::MouseDown {
        button,
        position,
        modifiers,
        ..
    } = &event
    {
        let (button, position, modifiers) = (*button, *position, *modifiers);
        let dispatched = {
            let stack_rc = backend.modal_stack_handle();
            let drag_rc = backend.drag_state_handle();
            let stack = stack_rc.borrow();
            let mut drag = drag_rc.borrow_mut();
            let evs = crate::dispatch::dispatch_click(
                &stack,
                &[], // scroll surfaces not tracked by MacBackend yet
                backend.text_regions(),
                &mut drag,
                position,
                button,
                modifiers,
            );
            // #803: track which region was clicked so Ctrl-A can target
            // it even before the first drag-move fires a
            // `TextSelectionChanged` event — mirrors gtk::run/win::run.
            if let Some(DragTarget::TextSelection { region, .. }) = drag.target() {
                backend.track_focused_text_region(region.clone());
            }
            evs
        };
        let mut outcome = EventOutcome::Continue;
        for ev in dispatched {
            match runtime::preprocess_event(ev, backend, app) {
                EventOutcome::Exit => return EventOutcome::Exit,
                EventOutcome::Redraw => outcome = EventOutcome::Redraw,
                EventOutcome::Continue => {}
            }
        }
        return outcome;
    }

    // #803: route a `MouseMoved` through `dispatch_mouse_drag` so an
    // in-progress `TextSelection`/scrollbar/split-divider drag (armed by
    // the `MouseDown` branch above) emits its synthetic event
    // (`TextSelectionChanged`/`ScrollOffsetChanged`/`SplitDividerDragged`)
    // alongside the plain `MouseMoved` — mirrors gtk::run's motion
    // controller / win::run::route_mouse_move. The plain `MouseMoved`
    // goes straight to `app.handle` (nothing in the shared pipeline
    // matches it); the extra event, if any, recurses into `dispatch_event`
    // so its own pre-processing (`TextSelectionChanged`, via
    // `preprocess_event`) applies uniformly — safe because
    // `dispatch_mouse_drag` never emits a second `MouseMoved`, so this
    // can't loop.
    if let UiEvent::MouseMoved { position, buttons } = &event {
        let (position, buttons) = (*position, *buttons);
        let events = {
            let drag_rc = backend.drag_state_handle();
            let drag = drag_rc.borrow();
            crate::dispatch::dispatch_mouse_drag(&drag, position, buttons)
        };
        let mut outcome = EventOutcome::Continue;
        for ev in events {
            let step = if matches!(ev, UiEvent::MouseMoved { .. }) {
                app.handle(ev, backend).into()
            } else {
                dispatch_event(ev, backend, app, caret_visible, caret_pause)
            };
            match step {
                EventOutcome::Exit => return EventOutcome::Exit,
                EventOutcome::Redraw => outcome = EventOutcome::Redraw,
                EventOutcome::Continue => {}
            }
        }
        return outcome;
    }

    // #803: route a `MouseUp` through `dispatch_mouse_up` so an
    // in-progress drag ends cleanly — mirrors gtk::run's
    // `connect_released` / win::run::route_mouse_up. `dispatch_mouse_up`
    // always returns exactly one `MouseUp` (see its doc), rewritten with
    // `widget` when the release lands inside an open modal.
    if let UiEvent::MouseUp {
        position, button, ..
    } = &event
    {
        let (position, button) = (*position, *button);
        let ev = {
            let stack_rc = backend.modal_stack_handle();
            let drag_rc = backend.drag_state_handle();
            let stack = stack_rc.borrow();
            let mut drag = drag_rc.borrow_mut();
            crate::dispatch::dispatch_mouse_up(&stack, &mut drag, position, button)
                .into_iter()
                .next()
                .expect("dispatch_mouse_up always returns exactly one MouseUp")
        };
        return app.handle(ev, backend).into();
    }

    // Everything else — double-click folding, ActivityBar redirect,
    // accelerators, Ctrl-C/Cmd-V/middle-click/Ctrl-A, selection-display
    // clearing, TextSelectionChanged — is the shared pipeline. See this
    // function's doc.
    runtime::preprocess_event(event, backend, app)
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
        // After app.render: overlay the text-selection highlight on top of
        // the rendered content (#803) — mirrors gtk::run::render_frame's
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
    /// can reach it directly. Used by `mouseDown:` (#498) to stash the
    /// raw press `NSEvent` for `Backend::begin_window_drag` before the
    /// press is translated to a portable `UiEvent`.
    backend: Rc<RefCell<MacBackend>>,
    paint: PaintFn,
    handle: HandleFn,
    /// Resize-settle debounce state (quadraui#780) — see
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
}

define_class!(
    /// Quadraui's custom `NSView`. `drawRect:` resolves viewport +
    /// CG context, paints a debug background (theme-defaulted grey
    /// + a #34 smoke label), then delegates to the stored `paint`
    /// closure so the active [`AppLogic`] can render on top.
    /// Responder methods translate `NSEvent` → [`UiEvent`] and route
    /// through the stored `handle` closure for `AppLogic::handle`.
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

            // `currentContext` is a safe method as of objc2-app-kit 0.3
            // (#796 bump). `drawRect:` is always invoked inside a valid
            // graphics scope, so it always returns `Some` here.
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
            // #498: stash the raw press before it's translated to a
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

        // ── Window resize (#486) ─────────────────────────────────

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
    }
);

impl QuadraView {
    fn new(
        mtm: MainThreadMarker,
        backend: Rc<RefCell<MacBackend>>,
        paint: PaintFn,
        handle: HandleFn,
    ) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(QuadraViewIvars {
            last_viewport: Cell::new(Viewport::default()),
            backend,
            paint,
            handle,
            resize_debouncer: RefCell::new(ResizeDebouncer::new()),
            resize_timer: RefCell::new(None),
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
        self.setNeedsDisplay(true)
    }

    fn request_exit(&self) {
        let mtm = MainThreadMarker::from(self);
        let app = NSApplication::sharedApplication(mtm);
        // SAFETY: `terminate:` on NSApp on the main thread is the
        // documented exit path.
        app.terminate(None);
    }
}

define_class!(
    /// Minimal `NSApplicationDelegate` — terminate the process when
    /// the last window closes (red traffic-light → exit). #36 may
    /// extend this with notification + URL-scheme handling.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "QuadraUiAppDelegate"]
    pub(crate) struct QuadraAppDelegate;

    unsafe impl NSObjectProtocol for QuadraAppDelegate {}

    unsafe impl NSApplicationDelegate for QuadraAppDelegate {
        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn should_terminate_after_last_window(&self, _sender: &NSApplication) -> bool {
            true
        }
    }
);

impl QuadraAppDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(());
        unsafe { msg_send![super(this), init] }
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
        Box::new(move |ev: UiEvent| -> Reaction {
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
        msg_send![
            mtm.alloc::<NSWindow>(),
            initWithContentRect: content_rect,
            styleMask: style,
            backing: NSBackingStoreType::Buffered,
            defer: false,
        ]
    };
    window.setTitle(&NSString::from_str("quadraui (macos)"));

    // #498: stash the window handle so `Backend::begin_window_drag` /
    // `Backend::toggle_window_maximize` / `Backend::set_cursor` have
    // something to drive. Harmless for apps that never call them
    // (default no-op on every other backend, and macOS apps that don't
    // opt into a CSD titlebar).
    backend.borrow_mut().set_window(window.clone());

    let view = QuadraView::new(mtm, backend.clone(), paint, handle);
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
    ns_app.run();

    std::process::ExitCode::SUCCESS
}

// Suppress unused warning for the c_void import — kept available for
// future opaque-pointer dancing in this file as it grows.
#[allow(dead_code)]
fn _unused_imports(_p: *mut c_void) {}

/// Coverage for #803: `dispatch_event`'s `MouseDown`/`MouseMoved`/`MouseUp`
/// text-selection routing plus its Ctrl-C/Ctrl-A/`TextSelectionChanged`
/// pre-processing. None of this needs a live `CGContext` or a running
/// `NSApplication` — the state machine and `dispatch_event` are plain Rust
/// logic once a `MacBackend` exists — mirroring `win::run`'s identical
/// `text_selection_dispatch_tests` module (#741) almost line for line;
/// `MacBackend::new()`'s default metrics (16.0 line height, 8.0 char
/// width) match `WinBackend::new()`'s exactly, so the same pixel math
/// applies unchanged.
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
    /// then Ctrl-C — the exact sequence `panel.drag_select_copy` (the
    /// Tier-1 scenario #803 unblocks on macOS) drives.
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

    /// quadraui#813: before this, `macos::run::dispatch_event` had no
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
