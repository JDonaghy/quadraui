// ─────────────────────────────────────────────────────────────────────
// Re-entrancy-safe access to a runner's shared backend + app cells
// ─────────────────────────────────────────────────────────────────────
//
// A runner keeps its backend and app in `Rc<RefCell<_>>`s that every
// native callback (paint, input, tick) borrows. A modal service called
// from inside one of those callbacks (an `NSAlert`, an open/save panel, a
// popup menu) runs a nested event loop that fires the same callbacks
// again while the outer borrow is still live, and a plain `borrow_mut`
// there panics inside a non-unwinding C ABI frame, aborting the process.
// These helpers make every such callback skip under contention instead,
// and [`DeferredPaint`] remembers a skipped frame so it is repainted once
// the outer borrow ends.
//
// Gated on the adopting backend (`macos::run`) plus `test`, so the
// helpers and their tests also build and run on non-macOS hosts.

#[cfg(any(test, all(feature = "macos", target_os = "macos")))]
use std::cell::{Cell, RefCell};

/// Runs `f` with `backend` and `app` both mutably borrowed, or returns
/// `None` without calling it when either is already borrowed further up
/// the stack.
#[cfg(any(test, all(feature = "macos", target_os = "macos")))]
pub(crate) fn try_with_both<B, A: ?Sized, R>(
    backend: &RefCell<B>,
    app: &RefCell<A>,
    f: impl FnOnce(&mut B, &mut A) -> R,
) -> Option<R> {
    let (Ok(mut b), Ok(mut a)) = (backend.try_borrow_mut(), app.try_borrow_mut()) else {
        return None;
    };
    Some(f(&mut b, &mut a))
}

/// [`try_with_both`] for a paint pass, which only reads `app` — so it
/// still runs while an outer caller holds `app` shared.
#[cfg(any(test, all(feature = "macos", target_os = "macos")))]
pub(crate) fn try_with_both_ref<B, A: ?Sized, R>(
    backend: &RefCell<B>,
    app: &RefCell<A>,
    f: impl FnOnce(&mut B, &A) -> R,
) -> Option<R> {
    let (Ok(mut b), Ok(a)) = (backend.try_borrow_mut(), app.try_borrow()) else {
        return None;
    };
    Some(f(&mut b, &a))
}

/// A frame owed because a paint (or a frame request) was skipped under
/// re-entrant borrow contention.
#[cfg(any(test, all(feature = "macos", target_os = "macos")))]
#[derive(Debug, Default)]
pub(crate) struct DeferredPaint(Cell<bool>);

#[cfg(any(test, all(feature = "macos", target_os = "macos")))]
impl DeferredPaint {
    /// Marks a frame as owed.
    pub(crate) fn defer(&self) {
        self.0.set(true);
    }

    /// Records one paint attempt: a skipped paint leaves a frame owed, a
    /// completed one settles it.
    pub(crate) fn record(&self, painted: bool) {
        self.0.set(!painted);
    }

    /// Hands out an owed frame exactly once, and only once `backend` is
    /// free — so a nested loop doesn't spin request-repaint → skipped
    /// paint while the outer borrow is still live.
    pub(crate) fn take_if_free<B>(&self, backend: &RefCell<B>) -> bool {
        self.0.get() && backend.try_borrow_mut().is_ok() && self.0.replace(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_with_both_runs_only_when_both_cells_are_free() {
        let backend = RefCell::new(1);
        let app = RefCell::new(2);
        assert_eq!(try_with_both(&backend, &app, |b, a| *b + *a), Some(3));
        {
            let _outer = backend.borrow_mut();
            assert_eq!(try_with_both(&backend, &app, |_, _| ()), None);
        }
        let _outer = app.borrow();
        assert_eq!(try_with_both(&backend, &app, |_, _| ()), None);
    }

    #[test]
    fn try_with_both_ref_tolerates_a_shared_outer_app_borrow() {
        let backend = RefCell::new(1);
        let app = RefCell::new(2);
        {
            let _outer = app.borrow();
            assert_eq!(try_with_both_ref(&backend, &app, |b, a| *b + *a), Some(3));
        }
        {
            let _outer = app.borrow_mut();
            assert_eq!(try_with_both_ref(&backend, &app, |_, _| ()), None);
        }
        let _outer = backend.borrow();
        assert_eq!(try_with_both_ref(&backend, &app, |_, _| ()), None);
    }

    /// A skipped frame stays pending while the outer borrow is live and
    /// is handed out exactly once after it ends.
    #[test]
    fn deferred_paint_is_released_once_the_outer_borrow_ends() {
        let backend = RefCell::new(());
        let deferred = DeferredPaint::default();
        deferred.record(false);
        {
            let _outer = backend.borrow_mut();
            assert!(!deferred.take_if_free(&backend));
        }
        assert!(deferred.take_if_free(&backend));
        assert!(!deferred.take_if_free(&backend));

        deferred.defer();
        deferred.record(true);
        assert!(
            !deferred.take_if_free(&backend),
            "a completed paint settles it"
        );
    }
}
