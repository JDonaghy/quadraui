//! Shared, backend-neutral desktop-interaction plumbing (#498).
//!
//! Everything in this module is genuinely windowing-generic: it has no
//! dependency on `gtk4`, `objc2`, `windows`, or any other toolkit crate,
//! and none of the *code* here needs a toolkit feature to compile — the
//! whole point of extracting it (see `BACKEND.md` §10) is that a
//! brand-new backend gets it for free. Each piece used to be implemented
//! once inside `gtk/` only; a macOS or Windows backend would otherwise
//! have had to reinvent it from scratch:
//!
//! - [`WindowDragArm`] — the arm/threshold/commit state machine behind
//!   a CSD titlebar drag-to-move (or edge-resize) gesture. Extracted
//!   from `gtk/backend.rs`'s `armed_window_drag` field +
//!   `gtk/run.rs`'s motion-controller threshold check (#400). **Not**
//!   adopted by `win::run`/`win::backend` (#702 audit): `win::run`
//!   creates its window with `WS_OVERLAPPEDWINDOW` — the standard native
//!   title bar and non-client resize border, not a client-side-decorated
//!   one — so title-bar drag-to-move and edge-resize are already handled
//!   entirely inside Windows' own non-client (`WM_NC*`) message
//!   handling; there is no quadraui-drawn titlebar/border for an app to
//!   press-and-drag on, so there is nothing for this arm/threshold/commit
//!   sequencing to gate. Revisit only if a future issue gives `win::run`
//!   a custom (CSD-style) frame — don't invent one just to have a use
//!   for this type.
//! - [`ModalPumpDepth`] / [`ModalPumpGuard`] — the re-entrancy guard
//!   around a nested native modal loop (a GTK async-dialog
//!   `MainContext::iteration` pump, an AppKit `runModal` /
//!   `performWindowDragWithEvent:`, a Win32 `IFileOpenDialog::Show`).
//!   Extracted from `gtk/services.rs`'s `pumping` field + `PumpGuard`
//!   (#427). Adopted by `win::run`'s `wndproc` → `app.handle`/
//!   `app.render` dispatch (#702) via `win::run`'s own
//!   `guarded_call` — the actual `RefCell` double-borrow guard lives
//!   there since it has no toolkit dependency of its own either; see
//!   that function's docs.
//! - [`SmokeConfig`] + [`smoke_size_ok`] / [`smoke_clipboard_round_trip_ok`]
//!   — the headless smoke-mode predicates behind `QUADRAUI_*_SMOKE_MS` /
//!   `_SMOKE_PASTE`. Extracted from `gtk/run.rs` (#450, GD-5), renamed
//!   backend-neutral (no more `Gtk` in the name) and parameterised over
//!   the env-var names and size floor so a future backend can reuse the
//!   mechanism under its own env vars and default window size. Adopted by
//!   `win::run`'s `QUADRAUI_WIN_SMOKE_MS`/`_SMOKE_PASTE` one-shot
//!   `WM_TIMER` check (#702) alongside GTK's `QUADRAUI_GTK_SMOKE_MS`/
//!   `_SMOKE_PASTE`.
//! - [`ALL_RESIZE_EDGES`] / [`all_pointer_shapes`] — an enum-walk
//!   scaffold for a [`PointerShape`] → native-cursor lookup table.
//!   Every backend still owns its own table (cursor *names*/objects are
//!   inherently native), but this gives it a canonical, exhaustively-
//!   maintained list of inputs to build and test that table against
//!   instead of hand-duplicating the variant list per backend. Adopted by
//!   `win::backend`'s `PointerShape` → `IDC_*` cursor-resource mapping
//!   (#702), driving `SetCursor`/`WM_SETCURSOR`.
//! - [`is_paste_keypress`] — the clipboard-paste keypress predicate
//!   (#728). Extracted from `gtk::run`'s private `is_paste_keypress`
//!   (quadraui#415) and generalised to also cover macOS's Cmd-V, which
//!   used to be an inline `match` guard in `macos::run::dispatch_event`
//!   instead of a named, independently-testable predicate. See
//!   `docs/DECISIONS.md` D-011 for the shift-tolerance contract this
//!   settles once for every adopter instead of each backend picking its
//!   own. Adopted by `win::run::dispatch_event` (#728) to give Win-GUI
//!   its first `UiEvent::ClipboardPaste` at all.
//!
//! ## What stays backend-specific
//!
//! The actual native calls — `gdk4::Toplevel::begin_move`,
//! `NSWindow::performWindowDragWithEvent`, `DwmExtendFrameIntoClientArea`
//! — are deliberately **not** here. [`WindowDragArm::commit_if_past_threshold`]
//! hands back the caller's own press payload `P` once the gesture should
//! commit; what the caller does with it (the native "begin move" call)
//! stays in `gtk/backend.rs` / `macos/backend.rs` / a future
//! `win/backend.rs`. Same story for [`ModalPumpGuard`] (wraps *a*
//! nested-loop call, doesn't make one) and the pointer-shape scaffold
//! (lists the variants, doesn't map them to a native cursor).
//!
//! ## Why every item below is `#[cfg]`-gated despite having no toolkit
//! dependency
//!
//! Mirrors `crate::runtime`'s precedent exactly (see its module doc):
//! the workspace-wide `RUSTFLAGS: "-D warnings"` (`ci.yml`) turns an
//! unused-`pub(crate)`-item warning into a hard build failure the
//! moment a feature combination compiles this module without any
//! backend around to call into it (e.g. `--features tui`, which has no
//! window chrome at all). Each item is gated on the feature(s) of the
//! backend(s) that actually call it today, not left permanently
//! `#[allow(dead_code)]] — adding a new adopter (e.g. a `win` backend
//! wiring up `WindowDragArm`) is a one-line `cfg` addition, not a
//! license to leave the guard off.

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
/// #400 rationale this generalises).
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
/// Extracted from `gtk::services::GtkPlatformServices`'s `pumping` field
/// (#427). The hazard it guards against: a nested pump lets *any* other
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
/// borrows (#702, via `win::run::guarded_call`) are the adopters today.
/// Unlike `win/backend.rs`'s WinAPI-calling methods, `guarded_call` is
/// pure `RefCell`/`Rc<Cell<>>` logic with no toolkit dependency of its
/// own, so it (and its unit test reproducing the double-borrow hazard)
/// compile and run under plain `--features win` on *any* host —
/// matching this module's own "no toolkit feature needed" design — but
/// only in a `cfg(test)` build: `win::run::guarded_call` has no
/// *production* caller unless `win32::wndproc` (its Windows-only
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

// ─────────────────────────────────────────────────────────────────────
// Headless smoke-mode config + predicates
// ─────────────────────────────────────────────────────────────────────

/// Headless smoke-mode config (originally quadraui#450, GD-5, GTK-only;
/// generalised here for #498). `None` unless the `ms`-suffixed env var
/// [`Self::from_env`] is asked to read is set — see
/// `gtk::run`'s module doc's "Headless smoke mode" section for the full
/// motivating rationale (quadraui#437: a live-window class of bug a
/// display-free driver test structurally can't catch).
///
/// `#[cfg(any(feature = "gtk", all(feature = "win", any(target_os =
/// "windows", test))))]` — same reasoning as [`ModalPumpDepth`]'s gate
/// note: `win::run`'s real smoke lane (#702) only exists once
/// `target_os = "windows"` also compiles `mod win32`, but the `test`
/// alternative keeps this type (and its pure predicates below) a real,
/// unit-testable consumer under a plain `cargo test --features win` on
/// any host — see `smoke_config_tests` below.
#[cfg(any(
    feature = "gtk",
    all(feature = "win", any(target_os = "windows", test))
))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SmokeConfig {
    /// Delay after the window is presented before the one-shot check
    /// fires and the window is closed.
    pub(crate) after_ms: u64,
    /// Optional text to round-trip through the real OS clipboard and
    /// replay as a synthetic paste.
    pub(crate) paste_text: Option<String>,
}

#[cfg(any(
    feature = "gtk",
    all(feature = "win", any(target_os = "windows", test))
))]
impl SmokeConfig {
    /// Reads the smoke-mode env vars once. `ms_var` is the env var that
    /// enables smoke mode (e.g. `QUADRAUI_GTK_SMOKE_MS`) — returns
    /// `None` (the default — zero behavioral change) unless it's set and
    /// parses as a `u64`. `paste_var` (e.g. `QUADRAUI_GTK_SMOKE_PASTE`)
    /// is optional; if set, its value round-trips through the paste
    /// path. Parameterised by name (rather than hardcoding the `GTK`
    /// infix) so a future backend can reuse this under its own env-var
    /// names without colliding with GTK's.
    pub(crate) fn from_env(ms_var: &str, paste_var: &str) -> Option<Self> {
        let after_ms = std::env::var(ms_var).ok()?.parse().ok()?;
        let paste_text = std::env::var(paste_var).ok();
        Some(Self {
            after_ms,
            paste_text,
        })
    }
}

/// Is `width`x`height` a plausible, non-broken window/surface
/// allocation? The direct regression check for the quadraui#437
/// tiny/wrapped-window bug class. Pure and display-free. Each backend
/// supplies its own floor (`min_width`/`min_height`) — its own default
/// window size and how far below it is still "plausible" — this
/// function only owns the comparison.
#[cfg(any(
    feature = "gtk",
    all(feature = "win", any(target_os = "windows", test))
))]
pub(crate) fn smoke_size_ok(width: i32, height: i32, min_width: i32, min_height: i32) -> bool {
    width >= min_width && height >= min_height
}

/// Did the OS clipboard round-trip `written` back byte-for-byte? Pure
/// comparison, factored out so the pass/fail rule is unit-testable
/// without a real clipboard.
#[cfg(any(
    feature = "gtk",
    all(feature = "win", any(target_os = "windows", test))
))]
pub(crate) fn smoke_clipboard_round_trip_ok(written: &str, read_back: Option<&str>) -> bool {
    read_back == Some(written)
}

// ─────────────────────────────────────────────────────────────────────
// Paste-keypress predicate
// ─────────────────────────────────────────────────────────────────────

/// Is `key`+`modifiers` this platform's clipboard-paste shortcut?
///
/// One predicate for every desktop backend (#728), where each backend's
/// paste shortcut differs only in *which* platform modifier means
/// "paste" — Ctrl-V on Linux/GTK and Windows, Cmd-V on macOS — and
/// [`crate::Modifiers::cmd`] is already the crate-wide abstraction for
/// "this platform's OS-level modifier key" (`win::events::win_modifiers`
/// maps the Windows key to `cmd` the same way `macos::events` maps ⌘;
/// see those modules' docs). Exactly one of `ctrl`/`cmd` must be held —
/// `^` (`bool` XOR) rejects both plain `v` (neither held) and the
/// vanishingly-unlikely chord of both platform modifiers at once — and
/// `alt` is never a valid paste chord on any backend.
///
/// `shift` is deliberately **not** checked either way — Ctrl-Shift-V and
/// (by the same reasoning, extended here) Cmd-Shift-V both trigger paste
/// too. This was already GTK's behavior (quadraui#415: some terminal
/// emulators reserve plain Ctrl-V for a literal control byte and use
/// Ctrl-Shift-V as the paste shortcut instead), and D-011
/// (`docs/DECISIONS.md`) extends the same shift-tolerant contract to
/// every adopter — including macOS, which used to require `shift: false`
/// in its inline Cmd-V match guard before this predicate replaced it.
/// See D-011 for why this settles the contract once instead of letting
/// each new adopter (this issue's `win` included) silently pick its own.
///
/// Case-insensitive on the letter (`'v'` or `'V'`) on every backend —
/// this was already true everywhere a paste chord existed, so unifying
/// the predicate doesn't change it.
#[cfg(any(
    feature = "gtk",
    all(feature = "macos", target_os = "macos"),
    feature = "win"
))]
pub(crate) fn is_paste_keypress(key: &crate::Key, modifiers: &crate::Modifiers) -> bool {
    matches!(key, crate::Key::Char('v') | crate::Key::Char('V'))
        && (modifiers.ctrl ^ modifiers.cmd)
        && !modifiers.alt
}

// ─────────────────────────────────────────────────────────────────────
// PointerShape enum-walk scaffold
// ─────────────────────────────────────────────────────────────────────

/// Every [`crate::backend::ResizeEdge`] variant, in a stable order.
/// Backends building a `ResizeEdge` → native-surface-edge or →
/// cursor-name table (and the tests that exercise it exhaustively)
/// iterate this instead of hand-listing all 8 arms — see e.g.
/// `gtk::backend::resize_edge_to_surface_edge_maps_every_variant`.
/// `#[cfg(test)]`: every consumer today is a mapping-table *test* (a
/// production `set_cursor`/`begin_window_resize` match handles one
/// `ResizeEdge` at a time and never needs the full list) — see the
/// module-doc note on why every item here is gated.
#[cfg(all(
    test,
    any(
        feature = "gtk",
        all(feature = "macos", target_os = "macos"),
        feature = "win"
    )
))]
pub(crate) const ALL_RESIZE_EDGES: [crate::backend::ResizeEdge; 8] = [
    crate::backend::ResizeEdge::North,
    crate::backend::ResizeEdge::South,
    crate::backend::ResizeEdge::East,
    crate::backend::ResizeEdge::West,
    crate::backend::ResizeEdge::NorthEast,
    crate::backend::ResizeEdge::NorthWest,
    crate::backend::ResizeEdge::SouthEast,
    crate::backend::ResizeEdge::SouthWest,
];

/// Every [`crate::backend::PointerShape`] variant, in a stable order —
/// [`crate::backend::PointerShape::Default`] plus one
/// [`crate::backend::PointerShape::Resize`] per [`ALL_RESIZE_EDGES`]
/// edge (9 total). Each backend still owns its own `PointerShape` →
/// native cursor table (cursor names/objects are inherently native —
/// GTK's are CSS cursor-name strings, AppKit's are `NSCursor` objects,
/// and AppKit in particular has no public diagonal-resize cursor, so
/// backends may legitimately map more than one `PointerShape` to the
/// same native cursor) — this is the enum-walk *scaffold* that was
/// missing: a canonical list backends' own mapping tests iterate
/// against instead of hand-duplicating the variant list per backend.
#[cfg(all(
    test,
    any(
        feature = "gtk",
        all(feature = "macos", target_os = "macos"),
        feature = "win"
    )
))]
pub(crate) fn all_pointer_shapes() -> [crate::backend::PointerShape; 9] {
    use crate::backend::PointerShape;
    [
        PointerShape::Default,
        PointerShape::Resize(ALL_RESIZE_EDGES[0]),
        PointerShape::Resize(ALL_RESIZE_EDGES[1]),
        PointerShape::Resize(ALL_RESIZE_EDGES[2]),
        PointerShape::Resize(ALL_RESIZE_EDGES[3]),
        PointerShape::Resize(ALL_RESIZE_EDGES[4]),
        PointerShape::Resize(ALL_RESIZE_EDGES[5]),
        PointerShape::Resize(ALL_RESIZE_EDGES[6]),
        PointerShape::Resize(ALL_RESIZE_EDGES[7]),
    ]
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

#[cfg(all(test, any(feature = "gtk", feature = "win")))]
mod smoke_config_tests {
    use super::*;
    use std::env;
    use std::sync::Mutex;

    // `env::set_var`/`remove_var` are process-global; serialise this
    // module's tests against each other so they don't race (matches the
    // pattern `gtk::run`'s own smoke tests didn't need, since those only
    // tested the pure predicates — `from_env` itself is new coverage
    // here). Shared by both the GTK and `win` (#702) adopters — the env
    // var names below are test-local sentinels, not either backend's real
    // `QUADRAUI_*_SMOKE_MS` names, so there's no cross-backend collision
    // risk running both features' tests in the same process.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn from_env_none_when_ms_var_unset() {
        let _lock = ENV_LOCK.lock().unwrap();
        let ms_var = "QUADRAUI_DESKTOP_TEST_SMOKE_MS_UNSET";
        let paste_var = "QUADRAUI_DESKTOP_TEST_SMOKE_PASTE_UNSET";
        env::remove_var(ms_var);
        env::remove_var(paste_var);
        assert_eq!(SmokeConfig::from_env(ms_var, paste_var), None);
    }

    #[test]
    fn from_env_none_when_ms_var_unparseable() {
        let _lock = ENV_LOCK.lock().unwrap();
        let ms_var = "QUADRAUI_DESKTOP_TEST_SMOKE_MS_BAD";
        env::set_var(ms_var, "not-a-number");
        assert_eq!(
            SmokeConfig::from_env(ms_var, "QUADRAUI_DESKTOP_TEST_UNSET"),
            None
        );
        env::remove_var(ms_var);
    }

    #[test]
    fn from_env_reads_ms_and_optional_paste() {
        let _lock = ENV_LOCK.lock().unwrap();
        let ms_var = "QUADRAUI_DESKTOP_TEST_SMOKE_MS_OK";
        let paste_var = "QUADRAUI_DESKTOP_TEST_SMOKE_PASTE_OK";
        env::set_var(ms_var, "250");
        env::set_var(paste_var, "hello");
        assert_eq!(
            SmokeConfig::from_env(ms_var, paste_var),
            Some(SmokeConfig {
                after_ms: 250,
                paste_text: Some("hello".to_string()),
            })
        );
        env::remove_var(ms_var);
        env::remove_var(paste_var);
    }

    #[test]
    fn from_env_paste_none_when_paste_var_unset() {
        let _lock = ENV_LOCK.lock().unwrap();
        let ms_var = "QUADRAUI_DESKTOP_TEST_SMOKE_MS_NOPASTE";
        let paste_var = "QUADRAUI_DESKTOP_TEST_SMOKE_PASTE_NOPASTE_UNSET";
        env::remove_var(paste_var);
        env::set_var(ms_var, "10");
        assert_eq!(
            SmokeConfig::from_env(ms_var, paste_var),
            Some(SmokeConfig {
                after_ms: 10,
                paste_text: None,
            })
        );
        env::remove_var(ms_var);
    }

    #[test]
    fn smoke_size_ok_accepts_at_or_above_the_floor() {
        assert!(smoke_size_ok(200, 150, 200, 150));
        assert!(smoke_size_ok(800, 600, 200, 150));
    }

    #[test]
    fn smoke_size_ok_rejects_below_the_floor() {
        assert!(!smoke_size_ok(199, 150, 200, 150));
        assert!(!smoke_size_ok(200, 149, 200, 150));
        // quadraui#437: content wrapped into an ~8px-wide column.
        assert!(!smoke_size_ok(8, 600, 200, 150));
    }

    #[test]
    fn clipboard_round_trip_ok_when_read_back_matches() {
        assert!(smoke_clipboard_round_trip_ok(
            "quadraui smoke",
            Some("quadraui smoke")
        ));
    }

    #[test]
    fn clipboard_round_trip_rejects_a_missing_or_mismatched_read() {
        assert!(!smoke_clipboard_round_trip_ok("quadraui smoke", None));
        assert!(!smoke_clipboard_round_trip_ok(
            "quadraui smoke",
            Some("something else")
        ));
    }
}

#[cfg(all(
    test,
    any(
        feature = "gtk",
        all(feature = "macos", target_os = "macos"),
        feature = "win"
    )
))]
mod is_paste_keypress_tests {
    //! Coverage for [`is_paste_keypress`] (#728) — the single predicate
    //! `gtk::run`, `macos::run`, and `win::run`'s `dispatch_event`s all
    //! now call. Pure/display-free: no live backend needed to exercise
    //! every branch of the contract D-011 (`docs/DECISIONS.md`) records.
    use super::*;
    use crate::{Key, Modifiers};

    fn mods(ctrl: bool, shift: bool, alt: bool, cmd: bool) -> Modifiers {
        Modifiers {
            ctrl,
            shift,
            alt,
            cmd,
        }
    }

    #[test]
    fn plain_ctrl_v_is_a_paste_keypress() {
        assert!(is_paste_keypress(
            &Key::Char('v'),
            &mods(true, false, false, false)
        ));
        assert!(is_paste_keypress(
            &Key::Char('V'),
            &mods(true, false, false, false)
        ));
    }

    #[test]
    fn plain_cmd_v_is_a_paste_keypress() {
        // macOS convention — `cmd` is the crate-wide abstraction for the
        // platform OS-modifier key (⌘ on macOS, mapped from the Windows
        // key by `win::events::win_modifiers`).
        assert!(is_paste_keypress(
            &Key::Char('v'),
            &mods(false, false, false, true)
        ));
    }

    #[test]
    fn ctrl_shift_v_is_a_paste_keypress() {
        // quadraui#415: some terminal emulators reserve plain Ctrl-V for
        // a control byte and use Ctrl-Shift-V for paste instead.
        assert!(is_paste_keypress(
            &Key::Char('v'),
            &mods(true, true, false, false)
        ));
    }

    #[test]
    fn cmd_shift_v_is_a_paste_keypress() {
        // D-011: the same shift-tolerant contract extends to macOS's
        // Cmd-V, which used to require `shift: false` before this
        // predicate replaced `macos::run`'s inline match guard.
        assert!(is_paste_keypress(
            &Key::Char('v'),
            &mods(false, true, false, true)
        ));
    }

    #[test]
    fn ctrl_alt_v_is_not_a_paste_keypress() {
        assert!(!is_paste_keypress(
            &Key::Char('v'),
            &mods(true, false, true, false)
        ));
    }

    #[test]
    fn cmd_alt_v_is_not_a_paste_keypress() {
        assert!(!is_paste_keypress(
            &Key::Char('v'),
            &mods(false, false, true, true)
        ));
    }

    #[test]
    fn both_ctrl_and_cmd_is_not_a_paste_keypress() {
        // XOR rejects the chord holding both platform modifiers at once.
        assert!(!is_paste_keypress(
            &Key::Char('v'),
            &mods(true, false, false, true)
        ));
    }

    #[test]
    fn shift_v_alone_is_not_a_paste_keypress() {
        assert!(!is_paste_keypress(
            &Key::Char('v'),
            &mods(false, true, false, false)
        ));
    }

    #[test]
    fn plain_v_is_not_a_paste_keypress() {
        assert!(!is_paste_keypress(&Key::Char('v'), &Modifiers::default()));
    }

    #[test]
    fn ctrl_c_is_not_a_paste_keypress() {
        assert!(!is_paste_keypress(
            &Key::Char('c'),
            &mods(true, false, false, false)
        ));
    }
}

#[cfg(all(
    test,
    any(
        feature = "gtk",
        all(feature = "macos", target_os = "macos"),
        feature = "win"
    )
))]
mod pointer_shape_scaffold_tests {
    use super::*;
    use crate::backend::{PointerShape, ResizeEdge};

    #[test]
    fn all_resize_edges_has_every_variant_exactly_once() {
        let mut seen: Vec<ResizeEdge> = ALL_RESIZE_EDGES.to_vec();
        seen.sort_by_key(|e| format!("{e:?}"));
        seen.dedup();
        assert_eq!(
            seen.len(),
            ALL_RESIZE_EDGES.len(),
            "ALL_RESIZE_EDGES must list every ResizeEdge variant exactly once"
        );
    }

    #[test]
    fn all_pointer_shapes_is_default_plus_one_resize_per_edge() {
        let shapes = all_pointer_shapes();
        assert_eq!(shapes.len(), ALL_RESIZE_EDGES.len() + 1);
        assert_eq!(shapes[0], PointerShape::Default);
        for (shape, edge) in shapes[1..].iter().zip(ALL_RESIZE_EDGES.iter()) {
            assert_eq!(*shape, PointerShape::Resize(*edge));
        }
    }
}
