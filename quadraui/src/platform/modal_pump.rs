// ─────────────────────────────────────────────────────────────────────
// ModalPumpDepth / ModalPumpGuard
// ─────────────────────────────────────────────────────────────────────

/// Shared, cloneable re-entrancy depth counter for a nested native
/// modal-loop pump (GTK's `MainContext::iteration(true)` wait for an
/// async `gtk4::FileDialog`; AppKit's `runModal` / any call that
/// internally pumps the run loop, e.g. `performWindowDragWithEvent:`;
/// Win32's `IFileOpenDialog::Show`). `> 0` while such a pump is in
/// flight (possibly several deep, if one nested loop somehow triggers
/// another) — see [`ModalPumpGuard`].
///
/// Extracted from `gtk::services::GtkPlatformServices`'s `pumping` field.
/// The hazard it guards against: a nested pump lets *any* other
/// pending event-loop source run too — including the runner's own idle
/// timer and every input event controller, all of which may also
/// mutably borrow the shared `Rc<RefCell<Backend>>` that the code
/// driving the pump *already* holds borrowed. Left unguarded, that
/// second borrow panics with "already borrowed", and because it
/// typically happens inside a non-unwindable native callback frame, the
/// panic aborts the whole process instead of propagating. Every runner
/// closure that might re-enter should clone this handle and check
/// [`Self::is_pumping`] before touching the backend.
///
/// `#[cfg(any(feature = "gtk", all(feature = "win", any(target_os =
/// "windows", test))))]`, not `any(gtk, macos)`: GTK's async
/// `FileDialog` pump and `win::run`'s `wndproc` → `dispatch`/`WM_PAINT`
/// borrows are the adopters today. Unlike `win/backend.rs`'s WinAPI-calling
/// methods, `guarded_call` is pure `RefCell`/`Rc<Cell<>>` logic with no
/// toolkit dependency of its own, so it (and its unit test reproducing
/// the double-borrow hazard) compile and run under plain `--features win`
/// on *any* host — matching this module's own "no toolkit feature needed"
/// design — but only in a `cfg(test)` build: `win::run::guarded_call` has
/// no *production* caller unless `win32::wndproc` (its Windows-only
/// adopter) also compiles, so the `test` alternative exists purely to
/// keep a genuine consumer around for a plain, non-test
/// `cargo check --features win` on a non-Windows host — see
/// `win::run::guarded_call`'s own gate for the matching half of this.
/// `macos::backend`'s `performWindowDragWithEvent:` is listed above as a
/// *candidate* nested pump, but nothing in `macos/` guards on this
/// counter yet — and per the module doc, a `pub(crate)` item with no
/// caller under some compiled feature set is a hard `-D warnings` build
/// failure, not a warning. A macOS adopter widens this gate in the same
/// commit that adds the call.
#[cfg(any(
    feature = "gtk",
    all(feature = "win", any(target_os = "windows", test))
))]
#[derive(Debug, Default, Clone)]
pub(crate) struct ModalPumpDepth(std::rc::Rc<std::cell::Cell<u32>>);

#[cfg(any(
    feature = "gtk",
    all(feature = "win", any(target_os = "windows", test))
))]
impl ModalPumpDepth {
    /// A fresh counter at depth 0 (no pump in flight).
    pub(crate) fn new() -> Self {
        Self(std::rc::Rc::new(std::cell::Cell::new(0)))
    }

    /// Current nesting depth.
    pub(crate) fn get(&self) -> u32 {
        self.0.get()
    }

    /// `true` while at least one nested pump is in flight. Runner
    /// closures that would otherwise call `backend.borrow_mut()` check
    /// this first and no-op instead.
    pub(crate) fn is_pumping(&self) -> bool {
        self.get() > 0
    }
}

/// RAII guard that increments a [`ModalPumpDepth`] for its lifetime and
/// decrements it on drop (including on an early return or panic-unwind
/// out of the pump), so nested pumps stay guarded until the *outermost*
/// one finishes. Construct one right before starting a nested native
/// modal loop; hold it for the loop's duration.
///
/// See [`ModalPumpDepth`]'s gate note for this type's identical gate.
#[cfg(any(
    feature = "gtk",
    all(feature = "win", any(target_os = "windows", test))
))]
pub(crate) struct ModalPumpGuard<'a> {
    depth: &'a ModalPumpDepth,
}

#[cfg(any(
    feature = "gtk",
    all(feature = "win", any(target_os = "windows", test))
))]
impl<'a> ModalPumpGuard<'a> {
    pub(crate) fn new(depth: &'a ModalPumpDepth) -> Self {
        depth.0.set(depth.0.get() + 1);
        Self { depth }
    }
}

#[cfg(any(
    feature = "gtk",
    all(feature = "win", any(target_os = "windows", test))
))]
impl Drop for ModalPumpGuard<'_> {
    fn drop(&mut self) {
        self.depth.0.set(self.depth.0.get() - 1);
    }
}
#[cfg(all(test, any(feature = "gtk", feature = "win")))]
mod modal_pump_tests {
    use super::*;

    #[test]
    fn new_starts_at_zero() {
        let depth = ModalPumpDepth::new();
        assert_eq!(depth.get(), 0);
        assert!(!depth.is_pumping());
    }

    #[test]
    fn default_starts_at_zero() {
        let depth = ModalPumpDepth::default();
        assert_eq!(depth.get(), 0);
    }

    #[test]
    fn guard_increments_while_held_and_decrements_on_drop() {
        let depth = ModalPumpDepth::new();
        {
            let _guard = ModalPumpGuard::new(&depth);
            assert_eq!(depth.get(), 1);
            assert!(depth.is_pumping());
        }
        assert_eq!(depth.get(), 0);
        assert!(!depth.is_pumping());
    }

    #[test]
    fn nested_guards_stay_pumping_until_the_outermost_drops() {
        let depth = ModalPumpDepth::new();
        let outer = ModalPumpGuard::new(&depth);
        assert_eq!(depth.get(), 1);
        {
            let _inner = ModalPumpGuard::new(&depth);
            assert_eq!(depth.get(), 2);
        }
        assert_eq!(depth.get(), 1, "inner drop must not clear the outer guard");
        assert!(depth.is_pumping());
        drop(outer);
        assert_eq!(depth.get(), 0);
    }

    #[test]
    fn cloned_handle_observes_the_same_counter() {
        let depth = ModalPumpDepth::new();
        let handle = depth.clone();
        let _guard = ModalPumpGuard::new(&depth);
        assert_eq!(
            handle.get(),
            1,
            "clone must share the underlying Rc<Cell<>>"
        );
    }
}
