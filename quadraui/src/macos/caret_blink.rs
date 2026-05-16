//! Caret-blink timer for `InlineInput` on macOS.
//!
//! AppKit text fields show a blinking insertion-point at the active
//! caret. The quadraui macOS rasteriser paints a static caret bar in
//! [`super::multi_section_view::paint_aux`] for `SectionAux::Input` /
//! `SectionAux::Search`; this module drives the on/off cycle via an
//! `NSTimer` ticking at the AppKit default period (~530 ms).
//!
//! Mechanism:
//! - [`MacBackend`][super::backend::MacBackend] owns the shared blink
//!   state — `Rc<Cell<bool>>` for "should the caret paint?" plus
//!   `Rc<Cell<Instant>>` for the typing-pause deadline.
//! - On startup, [`super::run::run`] installs a [`QuadraBlinkTarget`]
//!   whose ivars hold clones of those two cells, schedules an
//!   `NSTimer` against the target's `tick:` selector, and stores both
//!   handles so they outlive the run loop.
//! - The selector toggles `caret_visible` (unless `paused_until` is in
//!   the future) and triggers `setNeedsDisplay` on the key window's
//!   content view. The paint closure reads
//!   `MacBackend::caret_visible()` and threads it into `paint_aux`.
//!
//! Typing pauses the blink: the key handler in `run` resets
//! `paused_until = now + 500 ms` and forces `caret_visible = true`,
//! matching AppKit's "caret stays solid while typing" convention.
//!
//! Headless tests bypass this module entirely — they call
//! [`super::backend::MacBackend::set_caret_visible`] directly to pin
//! the phase for reproducible paint snapshots.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use objc2::declare_class;
use objc2::msg_send_id;
use objc2::mutability;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::sel;
use objc2::{ClassType, DeclaredClass};
use objc2_app_kit::NSApplication;
use objc2_foundation::{MainThreadMarker, NSTimer};

/// AppKit's default insertion-point blink period (`On` + `Off`
/// halves combined, ~530 ms). Honouring `NSTextInsertionPointBlinkPeriodOn`
/// / `Off` user defaults is a follow-up.
const BLINK_PERIOD_SECS: f64 = 0.530;

/// Per-instance state for [`QuadraBlinkTarget`].
pub(crate) struct QuadraBlinkTargetIvars {
    caret_visible: Rc<Cell<bool>>,
    paused_until: Rc<Cell<Instant>>,
}

declare_class!(
    /// Obj-C target for the caret-blink `NSTimer`. Held alive by
    /// [`super::run::run`] for the duration of the application.
    pub(crate) struct QuadraBlinkTarget;

    // SAFETY:
    // - Created and used exclusively on the main thread via
    //   `MainThreadMarker`. `NSTimer` callbacks fire on the run-loop
    //   that scheduled them — for `mainRunLoop`, the main thread.
    // - No Drop impl; ivars hold owned `Rc` smart pointers that drop
    //   cleanly when the Obj-C runtime finalises the instance.
    unsafe impl ClassType for QuadraBlinkTarget {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "QuadraBlinkTarget";
    }

    impl DeclaredClass for QuadraBlinkTarget {
        type Ivars = QuadraBlinkTargetIvars;
    }

    unsafe impl NSObjectProtocol for QuadraBlinkTarget {}

    unsafe impl QuadraBlinkTarget {
        /// Selector fired by the blink `NSTimer` once per
        /// [`BLINK_PERIOD_SECS`]. Toggles the shared `caret_visible`
        /// cell (skipping the toggle if we're still inside a
        /// typing-pause window) and requests a redraw of the key
        /// window's content view so the next frame paints with the
        /// new phase.
        #[method(tick:)]
        fn tick(&self, _timer: &NSTimer) {
            let now = Instant::now();
            if now < self.ivars().paused_until.get() {
                // Caret stays solid while typing.
                return;
            }
            let next = !self.ivars().caret_visible.get();
            self.ivars().caret_visible.set(next);
            let mtm = MainThreadMarker::from(self);
            let app = NSApplication::sharedApplication(mtm);
            if let Some(window) = app.keyWindow() {
                if let Some(view) = window.contentView() {
                    unsafe { view.setNeedsDisplay(true) };
                }
            }
        }
    }
);

impl QuadraBlinkTarget {
    fn new(
        mtm: MainThreadMarker,
        caret_visible: Rc<Cell<bool>>,
        paused_until: Rc<Cell<Instant>>,
    ) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(QuadraBlinkTargetIvars {
            caret_visible,
            paused_until,
        });
        unsafe { msg_send_id![super(this), init] }
    }
}

/// Schedule the caret-blink `NSTimer` against a fresh
/// [`QuadraBlinkTarget`]. Returns both handles — callers (currently
/// [`super::run::run`]) hold them so the timer keeps firing for the
/// lifetime of the app. Dropping the `NSTimer` invalidates it.
pub(crate) fn install_blink_timer(
    mtm: MainThreadMarker,
    caret_visible: Rc<Cell<bool>>,
    paused_until: Rc<Cell<Instant>>,
) -> (Retained<QuadraBlinkTarget>, Retained<NSTimer>) {
    let target = QuadraBlinkTarget::new(mtm, caret_visible, paused_until);
    let timer: Retained<NSTimer> = unsafe {
        NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
            BLINK_PERIOD_SECS,
            &target,
            sel!(tick:),
            None,
            true,
        )
    };
    (target, timer)
}
