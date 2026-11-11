// ─────────────────────────────────────────────────────────────────────
// WindowDragArm
// ─────────────────────────────────────────────────────────────────────

/// Arm/threshold/commit state machine for a deferred native window-drag
/// (or, by the same shape, edge-resize) gesture, generic over `P` — the
/// backend-native press payload (GTK: device + button + x + y +
/// timestamp; a future AppKit/Win32 backend has its own shape). This
/// struct only owns the arm/discard/threshold/commit *sequencing*; it
/// never interprets `P` itself.
///
/// ## Why "arm now, commit later past a threshold" instead of acting on
/// the raw press immediately
///
/// Calling a native "begin move" synchronously on the very first press
/// — before it's known whether a second press is coming — starts an
/// interactive move grab that swallows the second press, so a
/// double-click on a CSD titlebar never reaches the app as
/// `UiEvent::DoubleClick`. Deferring the actual native call until the
/// pointer has moved past a threshold distance since the arming press
/// avoids this; it's exactly what native `gtk4::WindowHandle` does too,
/// deferring its own move-start to `GestureDrag`'s threshold-gated
/// `drag-begin` signal rather than the raw press (see
/// `gtk::backend::GtkBackend::armed_window_drag`'s doc for the full
/// rationale this generalises).
///
/// Not every backend needs the threshold gating (AppKit's
/// `performWindowDragWithEvent:` already disambiguates a drag from a
/// double-click internally, so `macos::backend::MacBackend` uses
/// [`Self::arm`] / [`Self::take`] without ever calling
/// [`Self::commit_if_past_threshold`]) — the type doesn't force either
/// usage.
#[cfg(any(feature = "gtk", all(feature = "macos", target_os = "macos")))]
#[derive(Debug)]
pub(crate) struct WindowDragArm<P> {
    /// `(press, origin_x, origin_y)` — `None` when nothing is armed.
    armed: Option<(P, f64, f64)>,
}

#[cfg(any(feature = "gtk", all(feature = "macos", target_os = "macos")))]
impl<P> WindowDragArm<P> {
    /// No request armed.
    pub(crate) const fn new() -> Self {
        Self { armed: None }
    }

    /// Arm a deferred request with `press` and the screen-space origin
    /// `(origin_x, origin_y)` of the press that armed it. Overwrites any
    /// previously-armed (and never committed) request.
    pub(crate) fn arm(&mut self, press: P, origin_x: f64, origin_y: f64) {
        self.armed = Some((press, origin_x, origin_y));
    }

    /// Whether a request is currently armed. Test-only today: no
    /// production caller needs it (both `try_commit_window_drag`-style
    /// callers and [`Self::commit_if_past_threshold`] itself work
    /// without ever asking "is something armed?" directly), but it's
    /// handy for asserting a backend's own no-window/no-press guard
    /// state without exposing the private `armed` field.
    #[cfg(test)]
    pub(crate) fn is_armed(&self) -> bool {
        self.armed.is_some()
    }

    /// Screen-space origin of the press that armed the current request,
    /// if any. Test-only today: production callers only ever need
    /// [`Self::commit_if_past_threshold`], which does its own origin
    /// math internally.
    #[cfg(test)]
    pub(crate) fn origin(&self) -> Option<(f64, f64)> {
        self.armed.as_ref().map(|(_, x, y)| (*x, *y))
    }

    /// Discard an armed-but-uncommitted request without starting the
    /// gesture. Call when the button goes up before the pointer ever
    /// moved past the threshold — the press was a plain click (or the
    /// first half of a double-click), and leaving the state armed would
    /// let a later, unrelated hover-motion event accidentally commit it.
    ///
    /// Gated on `gtk`-or-`test` for the same `-D warnings` reason the
    /// module doc gives: `macos::backend` uses [`Self::arm`] /
    /// [`Self::take`] only (AppKit's `performWindowDragWithEvent:` needs
    /// no threshold gating), so on a `--features macos` build with no
    /// `gtk` around this method has no production caller and an ungated
    /// `pub(crate) fn` would be a hard `dead_code` build failure. A
    /// future backend that adopts the threshold path adds its feature
    /// here.
    #[cfg(any(feature = "gtk", test))]
    pub(crate) fn discard(&mut self) {
        self.armed = None;
    }

    /// Unconditionally take the armed press, if any, discarding origin
    /// tracking. For gestures with no competing double-click ambiguity
    /// to protect against (an edge-resize; or a backend, like AppKit,
    /// whose native call already disambiguates drag-vs-click itself) —
    /// see the type's doc comment.
    pub(crate) fn take(&mut self) -> Option<P> {
        self.armed.take().map(|(p, _, _)| p)
    }

    /// If a request is armed and the live pointer position
    /// `(current_x, current_y)` has moved at least `threshold_px` from
    /// the arming origin, take and return the press so the caller can
    /// start the native gesture. Returns `None` — leaving the request
    /// armed — if nothing is armed yet, or if it's armed but still
    /// under threshold.
    ///
    /// Gated on `gtk`-or-`test` for the same reason as [`Self::discard`]
    /// — see that method's note.
    #[cfg(any(feature = "gtk", test))]
    pub(crate) fn commit_if_past_threshold(
        &mut self,
        current_x: f64,
        current_y: f64,
        threshold_px: f64,
    ) -> Option<P> {
        let (_, origin_x, origin_y) = self.armed.as_ref()?;
        let dx = current_x - origin_x;
        let dy = current_y - origin_y;
        if (dx * dx + dy * dy).sqrt() >= threshold_px {
            self.take()
        } else {
            None
        }
    }
}

#[cfg(any(feature = "gtk", all(feature = "macos", target_os = "macos")))]
impl<P> Default for WindowDragArm<P> {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(all(
    test,
    any(feature = "gtk", all(feature = "macos", target_os = "macos"))
))]
mod window_drag_arm_tests {
    use super::*;

    #[test]
    fn new_is_unarmed() {
        let arm: WindowDragArm<i32> = WindowDragArm::new();
        assert!(!arm.is_armed());
        assert!(arm.origin().is_none());
    }

    #[test]
    fn default_is_unarmed() {
        let arm: WindowDragArm<i32> = WindowDragArm::default();
        assert!(!arm.is_armed());
    }

    #[test]
    fn arm_records_press_and_origin() {
        let mut arm = WindowDragArm::new();
        arm.arm(42, 10.0, 20.0);
        assert!(arm.is_armed());
        assert_eq!(arm.origin(), Some((10.0, 20.0)));
    }

    #[test]
    fn arm_overwrites_a_previous_unconsumed_request() {
        let mut arm = WindowDragArm::new();
        arm.arm(1, 0.0, 0.0);
        arm.arm(2, 5.0, 5.0);
        assert_eq!(arm.origin(), Some((5.0, 5.0)));
        assert_eq!(arm.take(), Some(2));
    }

    #[test]
    fn discard_clears_state() {
        let mut arm = WindowDragArm::new();
        arm.arm(1, 0.0, 0.0);
        arm.discard();
        assert!(!arm.is_armed());
        assert!(arm.origin().is_none());
    }

    #[test]
    fn take_consumes_and_clears() {
        let mut arm = WindowDragArm::new();
        arm.arm("press", 0.0, 0.0);
        assert_eq!(arm.take(), Some("press"));
        assert!(!arm.is_armed());
        assert_eq!(arm.take(), None);
    }

    #[test]
    fn commit_if_past_threshold_none_when_unarmed() {
        let mut arm: WindowDragArm<i32> = WindowDragArm::new();
        assert_eq!(arm.commit_if_past_threshold(100.0, 100.0, 8.0), None);
    }

    #[test]
    fn commit_if_past_threshold_none_under_threshold_and_stays_armed() {
        let mut arm = WindowDragArm::new();
        arm.arm("press", 0.0, 0.0);
        // 3-4-5 triangle: distance 5, under an 8px threshold.
        assert_eq!(arm.commit_if_past_threshold(3.0, 4.0, 8.0), None);
        assert!(arm.is_armed(), "still under threshold: must stay armed");
    }

    #[test]
    fn commit_if_past_threshold_takes_at_exactly_the_threshold() {
        let mut arm = WindowDragArm::new();
        arm.arm("press", 0.0, 0.0);
        assert_eq!(arm.commit_if_past_threshold(8.0, 0.0, 8.0), Some("press"));
        assert!(!arm.is_armed());
    }

    #[test]
    fn commit_if_past_threshold_takes_past_threshold() {
        let mut arm = WindowDragArm::new();
        arm.arm("press", 10.0, 10.0);
        // 6-8-10 triangle: distance 10 from origin (10, 10) to (16, 18).
        assert_eq!(arm.commit_if_past_threshold(16.0, 18.0, 8.0), Some("press"));
        assert!(!arm.is_armed());
    }
}
