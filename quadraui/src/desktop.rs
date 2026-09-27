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
//!   `docs/decisions/DECISIONS.md` D-011 for the shift-tolerance contract this
//!   settles once for every adopter instead of each backend picking its
//!   own, and D-011 §4 specifically for why the predicate takes a
//!   [`PasteModifier`] parameter naming *which* platform modifier is
//!   native to the calling backend, rather than accepting either
//!   `ctrl` or `cmd` interchangeably. Adopted by `win::run::dispatch_event`
//!   (#728) to give Win-GUI its first `UiEvent::ClipboardPaste` at all.
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
// move_to_trash (issue #956)
// ─────────────────────────────────────────────────────────────────────

/// Move `path` to the platform trash/recycle bin via the cross-platform
/// `trash` crate (issue #956) — the one implementation shared by every
/// `PlatformServices::move_to_trash` override
/// (`tui`/`gtk`/`macos`/`win::services`), TUI included. Unlike this
/// module's other genuinely backend-neutral helpers, this one *does*
/// wrap a native call — but the same one on every platform: `trash`
/// itself dispatches internally to `NSFileManager
/// -trashItemAtURL:resultingItemURL:error:` on macOS, the
/// freedesktop.org trash spec on Linux, and
/// `SHFileOperationW(FOF_ALLOWUNDO)` on Windows, none of which need a
/// live desktop/window-server session — only a filesystem — so there is
/// no backend-specific variation left to hand-write. See
/// [`crate::backend::PlatformServices::move_to_trash`]'s doc for why
/// that means TUI gets this exact function too, instead of the
/// `Err(BackendError::Unsupported)` degrade its other desktop-shell
/// methods (`reveal_in_file_manager`, native dialogs, …) fall back to.
///
/// `#[cfg]`-gated on every adopting feature (mirroring this module's own
/// "why every item is gated" note) — `win`'s production call site is
/// itself nested inside `win::services`'s own `#[cfg(target_os =
/// "windows")]` arm (see `WinPlatformServices::move_to_trash`), so the
/// `win` alternative here needs the identical `any(target_os =
/// "windows", test)` qualifier [`ModalPumpDepth`]'s gate note explains:
/// without it, `cargo check --features win` on a non-Windows host would
/// compile this function with no caller at all and trip `-D warnings`'
/// dead-code lint. The `test` half of that `any(..)` is what keeps this a
/// real, unit-testable consumer under `cargo test --features win` on any
/// host, matching that same precedent.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    all(feature = "macos", target_os = "macos"),
    all(feature = "win", any(target_os = "windows", test))
))]
pub(crate) fn move_to_trash(path: &std::path::Path) -> crate::backend::ServiceResult<()> {
    trash::delete(path).map_err(|e| crate::backend::BackendError::PlatformFailure {
        context: format!("trash::delete: {e}"),
    })
}

#[cfg(all(
    test,
    any(
        feature = "tui",
        feature = "gtk",
        all(feature = "macos", target_os = "macos"),
        feature = "win"
    )
))]
mod move_to_trash_tests {
    use super::*;

    /// Real round trip against this host's actual trash/recycle bin: a
    /// freshly-written temp file must be gone from its original path
    /// (moved, not copied) once [`move_to_trash`] reports success. Skips
    /// gracefully (rather than failing) when the underlying OS call
    /// itself fails — same posture as this crate's OS-clipboard tests on
    /// a headless display: on macOS, `trash`'s implementation goes
    /// through a Finder AppleEvent, which needs a live desktop session
    /// (and, on a *first* run, an Automation permission grant) that a
    /// sandboxed/headless dev box or CI container genuinely may not have,
    /// even though the *filesystem* `move_to_trash` itself only ever
    /// touches is always present.
    #[allow(clippy::print_stderr)]
    #[test]
    fn move_to_trash_removes_the_file_from_its_original_path() {
        let path = std::env::temp_dir().join(format!(
            "quadraui-956-move-to-trash-test-{}.txt",
            std::process::id()
        ));
        std::fs::write(&path, b"quadraui#956").expect("write temp file");
        assert!(path.exists(), "temp file must exist before trashing it");

        if let Err(e) = move_to_trash(&path) {
            eprintln!(
                "skipping: move_to_trash failed in this environment ({e:?}) — \
                 likely no live desktop/trash session"
            );
            let _ = std::fs::remove_file(&path);
            return;
        }

        assert!(
            !path.exists(),
            "the original path must no longer exist once move_to_trash succeeds"
        );
    }

    /// A path that never existed is a real, reportable failure — not
    /// silently `Ok(())` — matching every other `PlatformServices` method
    /// in this crate that surfaces the native call's actual outcome
    /// rather than papering over it.
    #[test]
    fn move_to_trash_on_a_nonexistent_path_reports_a_failure() {
        let path = std::env::temp_dir().join(format!(
            "quadraui-956-does-not-exist-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        assert!(!path.exists());
        assert!(move_to_trash(&path).is_err());
    }
}

// ─────────────────────────────────────────────────────────────────────
// wide_nul_terminated (issue #1087)
// ─────────────────────────────────────────────────────────────────────

/// UTF-16, NUL-terminated — the framing every wide (`W`-suffixed) Win32
/// API expects (`ShellExecuteW`'s `operation`/`file`, `CF_UNICODETEXT`'s
/// clipboard payload, `NOTIFYICONDATAW`'s fixed-size fields, a context
/// menu item's label). The single implementation shared by
/// `win::services`, `win::tray`, `win::backend`, and this module's own
/// [`windows_shell_execute_open`] (issue #1087) — before this, each of
/// the first three modules carried its own private copy of this exact
/// two-line conversion (`win::services::wide_nul_terminated`,
/// `win::tray::win_wide_nul_terminated`,
/// `win::backend::win_wide_nul_terminated`), and [`windows_shell_execute_open`]
/// needed a fourth.
///
/// `#[cfg_attr(not(target_os = "windows"), allow(dead_code))]`: every
/// real caller only exists under `target_os = "windows"` (the WinAPI
/// call sites this feeds, and this module's own Windows-only opener), so
/// on any other host this is unreachable dead code the moment it
/// compiles at all under `--features win` alone — the same posture
/// `win::services`'s pre-#1087 copy of this function already carried.
#[cfg(any(feature = "tui", feature = "win"))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn wide_nul_terminated(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(all(test, any(feature = "tui", feature = "win")))]
mod wide_nul_terminated_tests {
    use super::*;

    #[test]
    fn encodes_utf16_and_appends_a_nul() {
        assert_eq!(
            wide_nul_terminated("ab"),
            vec!['a' as u16, 'b' as u16, 0u16]
        );
        assert_eq!(wide_nul_terminated(""), vec![0u16]);
    }
}

// ─────────────────────────────────────────────────────────────────────
// open_with_default (issue #1087)
// ─────────────────────────────────────────────────────────────────────

/// Open `target` (a URL string or a filesystem path, carried as `&OsStr`
/// so a non-UTF-8 path need not be lossily converted before the call) —
/// the single opener implementation shared by `tui::services`
/// (`TuiPlatformServices::open_path` directly, and
/// `TuiPlatformServices::open_url_result`'s opener step via
/// [`try_open_with_default`] — that one can't call this function itself
/// since it needs the raw bool to chain its own OSC 8 fallback),
/// `macos::services` (`open_url`/`open_path`/`open_url_result`), and
/// `win::services` (`open_url`/`open_path`/`open_url_result`).
///
/// Before this issue, each backend wrote its own copy: `tui` via
/// `build_url_opener_command`/`try_platform_opener`/a hand-written
/// `ShellExecuteW` FFI declaration, `macos` via an inline
/// `Command::new("open").arg(url).spawn()` whose result was discarded,
/// `win` via a `windows`-crate `ShellExecuteW` call whose result
/// `open_url` also discarded — the "capability lie" this issue's own
/// title names: `open_url_result`'s default (`Backend::open_url_result`)
/// calls `open_url` and always answers `Ok(())`, so a GUI backend could
/// never actually report "the browser didn't open" even though the
/// underlying spawn/`ShellExecuteW` call already knew.
///
/// Unix: spawns `open` (macOS) / `xdg-open` (other Unix), detached
/// (stdout/stderr nulled so a slow or chatty opener can't wedge the
/// caller's own streams — matters most for `tui`, which shares stdout
/// with the terminal UI) and never through a shell — `execve`, so
/// `target` is never re-parsed for shell metacharacters. Windows:
/// `ShellExecuteW(NULL, "open", target, NULL, NULL, SW_SHOWNORMAL)`
/// through a **minimal, hand-written FFI declaration**, not the
/// `windows` crate — `windows` is `optional`, pulled in only by the
/// `win` feature's `dep:windows` (see `Cargo.toml`), so it is not a
/// dependency of a bare `--features tui` build, and this must compile
/// there too. Never goes through `cmd.exe`: the original vimcode-derived
/// `cmd /c start "" <url>` approach this crate briefly carried (issue
/// #969 review) is a CWE-78 command-injection hole, since `cmd.exe`
/// re-parses its own command-line text for `&`/`|`/`^`/`%` regardless of
/// how the argument was quoted for `CreateProcess`.
#[cfg(any(
    feature = "tui",
    all(feature = "macos", target_os = "macos"),
    all(feature = "win", target_os = "windows")
))]
pub(crate) fn open_with_default(target: &std::ffi::OsStr) -> crate::backend::ServiceResult<()> {
    if try_open_with_default(target) {
        Ok(())
    } else {
        Err(crate::backend::BackendError::PlatformFailure {
            context: format!("open_with_default: no opener launched for {target:?}"),
        })
    }
}

/// The "did the platform opener genuinely launch" step [`open_with_default`]
/// wraps into a [`crate::backend::ServiceResult`], exposed separately
/// (`pub(crate)`, not folded into `open_with_default` itself) because
/// `tui::services`'s `open_url_via` needs the raw bool to chain its own
/// OSC 8 hyperlink fallback when this returns `false` —
/// `open_with_default`'s `Err` shape has nowhere to carry that extra
/// step.
#[cfg(any(
    feature = "tui",
    all(feature = "macos", target_os = "macos"),
    all(feature = "win", target_os = "windows")
))]
pub(crate) fn try_open_with_default(target: &std::ffi::OsStr) -> bool {
    #[cfg(windows)]
    {
        windows_shell_execute_open(target)
    }
    #[cfg(not(windows))]
    {
        unix_open_with_default_command(target).spawn().is_ok()
    }
}

/// macOS opener command, not yet spawned — factored out so a test can
/// assert on the program name and arguments a real call would spawn
/// without actually launching anything. See the Linux/BSD overload of
/// this same function (below) for the shared shape and doc; lifted from
/// `tui::services`'s pre-#1087 `build_url_opener_command`.
#[cfg(all(target_os = "macos", any(feature = "tui", feature = "macos")))]
pub(crate) fn unix_open_with_default_command(target: &std::ffi::OsStr) -> std::process::Command {
    let mut cmd = std::process::Command::new("open");
    cmd.arg(target)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd
}

/// Linux/BSD opener command. See the macOS overload of this same
/// function (above) for the shared doc.
#[cfg(all(unix, not(target_os = "macos"), feature = "tui"))]
pub(crate) fn unix_open_with_default_command(target: &std::ffi::OsStr) -> std::process::Command {
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(target)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd
}

/// Windows opener: `ShellExecuteW`, called through a minimal hand-written
/// FFI declaration rather than the `windows` crate — see
/// [`open_with_default`]'s doc for why. Returns whether `ShellExecuteW`
/// reports success, per [`shell_execute_reports_success`]'s `> 32` rule.
#[cfg(all(windows, any(feature = "tui", feature = "win")))]
fn windows_shell_execute_open(target: &std::ffi::OsStr) -> bool {
    shell_execute_reports_success(windows_shell_execute_open_code(target))
}

/// `ShellExecuteW`'s documented success test, split out from the FFI call
/// itself ([`windows_shell_execute_open_code`]) so it is unit-testable on
/// *every* host — the FFI call is not.
///
/// Per `ShellExecuteW`'s own docs the return value is "greater than 32 if
/// successful, or an error value that is less than or equal to 32
/// otherwise" (e.g. `SE_ERR_FNF` = 2, `SE_ERR_NOASSOC` = 31). `32` itself
/// is the boundary and is a *failure* (`SE_ERR_DDEFAIL`), which is the
/// off-by-one this predicate exists to pin down.
///
/// Deliberately **not** `cfg(windows)`-gated: it is pure arithmetic with
/// no WinAPI in it, exactly like `win::msg`'s `WM_SIZE` unpacking, so
/// `ci.yml`'s ubuntu-leg `cargo test -p quadraui --features win` executes
/// its tests too. Only [`windows_shell_execute_open_code`] — the part
/// that genuinely calls into `shell32` — stays Windows-only.
///
/// `#[cfg_attr(not(target_os = "windows"), allow(dead_code))]` for the
/// same reason [`wide_nul_terminated`] carries one: its only non-test
/// caller is the Windows-only [`windows_shell_execute_open`].
#[cfg(any(feature = "tui", feature = "win"))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn shell_execute_reports_success(code: isize) -> bool {
    code > 32
}

#[cfg(all(test, any(feature = "tui", feature = "win")))]
mod shell_execute_reports_success_tests {
    use super::*;

    /// [`shell_execute_reports_success`] is `ShellExecuteW`'s documented
    /// "greater than 32" rule, and the boundary is the whole reason it's a
    /// named function rather than an inline comparison: `32` itself
    /// (`SE_ERR_DDEFAIL`) is a *failure*, so a `>=` here would silently
    /// turn one real failure code into a reported success. Pure
    /// arithmetic, no WinAPI — so this runs on every host, including
    /// `ci.yml`'s ubuntu-leg `cargo test -p quadraui --features win`,
    /// where nothing else in the Windows opener compiles at all.
    #[test]
    fn success_threshold_is_strictly_above_32() {
        const SE_ERR_FNF: isize = 2;
        const SE_ERR_NOASSOC: isize = 31;
        const SE_ERR_DDEFAIL: isize = 32;

        assert!(!shell_execute_reports_success(0));
        assert!(!shell_execute_reports_success(SE_ERR_FNF));
        assert!(!shell_execute_reports_success(SE_ERR_NOASSOC));
        assert!(!shell_execute_reports_success(SE_ERR_DDEFAIL));
        assert!(shell_execute_reports_success(33));
        assert!(shell_execute_reports_success(42));
    }
}

/// The raw `ShellExecuteW(NULL, "open", target, NULL, NULL,
/// SW_SHOWNORMAL)` return value. Split from
/// [`windows_shell_execute_open`] so a test can assert on the actual
/// documented `SE_ERR_*` code a given target produces on a real host;
/// callers outside this module want the `bool`.
#[cfg(all(windows, any(feature = "tui", feature = "win")))]
fn windows_shell_execute_open_code(target: &std::ffi::OsStr) -> isize {
    // SAFETY: `ShellExecuteW` is a well-known, stable Win32 API. Both
    // wide-string buffers passed below are nul-terminated and kept alive
    // (as local `Vec<u16>`s) for the duration of the call; the remaining
    // arguments are the documented "no window handle / no extra
    // parameters / no explicit working directory" null pointers.
    //
    // `#[link(name = "shell32")]`: `ShellExecuteW` lives in
    // `shell32.dll`/`shell32.lib` — unlike the `kernel32`/`user32`
    // imports the MSVC CRT startup pulls in implicitly, this one needs an
    // explicit link directive since nothing else in a bare `tui`-feature
    // build references `shell32` at all.
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteW(
            hwnd: *mut core::ffi::c_void,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show_cmd: i32,
        ) -> isize;
    }
    const SW_SHOWNORMAL: i32 = 1;

    let operation = wide_nul_terminated("open");
    let file = wide_nul_terminated(&target.to_string_lossy());
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    }
}

/// Process-wide `$PATH` is shared, mutable global state — every test
/// below (and `macos::services`'s `open_url_result` failure test, issue
/// #1087) that temporarily redirects it to prove a real "opener missing"
/// failure must serialize against every *other* such test in the same
/// process, or a concurrently-running test can observe (or clobber) the
/// override. One crate-wide lock rather than a per-module copy so
/// `cargo test --features tui` (this module's own tests) and
/// `cargo test --features macos` (which never run in the same process,
/// per `CLAUDE.md`'s per-backend CI legs, but might in a future combined
/// local run) can't race each other either.
///
/// **`unix`-gated, deliberately** — every test that takes this lock is a
/// `$PATH`-override test, and the `$PATH`-stub technique only applies to
/// [`unix_open_with_default_command`]'s arm: Windows' opener
/// ([`windows_shell_execute_open`]) is a direct `ShellExecuteW` FFI call
/// with no `$PATH`-resolved binary to intercept, so no Windows test has
/// any reason to hold this. Without the `unix` here, a
/// `cargo test --features tui` on a Windows host compiles a lock with
/// zero call sites — `dead_code`, which `ci.yml`'s workflow-wide
/// `RUSTFLAGS: -D warnings` promotes to a hard error on the
/// `windows-latest` leg of the `tui` job alone, while every Linux and
/// macOS build of the identical source stays green (the same
/// cfg-asymmetry trap `emit_osc8_hyperlink_to_tty`'s `unused_mut`
/// already cost this crate once — see `tui::services`'s
/// `emit_osc8_hyperlink_to_tty_reports_false_without_a_dev_tty`).
#[cfg(all(
    test,
    unix,
    any(feature = "tui", all(feature = "macos", target_os = "macos"))
))]
pub(crate) static PATH_OVERRIDE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(all(
    test,
    any(
        feature = "tui",
        all(feature = "macos", target_os = "macos"),
        all(feature = "win", target_os = "windows")
    )
))]
mod open_with_default_tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn unix_open_with_default_command_uses_macos_open() {
        let cmd = unix_open_with_default_command(std::ffi::OsStr::new("https://example.com/1087"));
        assert_eq!(cmd.get_program(), "open");
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            ["https://example.com/1087"]
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn unix_open_with_default_command_uses_xdg_open() {
        let cmd = unix_open_with_default_command(std::ffi::OsStr::new("https://example.com/1087"));
        assert_eq!(cmd.get_program(), "xdg-open");
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            ["https://example.com/1087"]
        );
    }

    /// A target that can never resolve, chosen so the real
    /// `ShellExecuteW` call fails *without* handing anything to the shell:
    /// an absolute path under a directory that does not exist, with an
    /// extension no handler is registered for. Verified on a real Windows
    /// host to return `SE_ERR_FNF` (2) immediately, with no UI.
    ///
    /// Deliberately **not** an unregistered URI scheme. That was this
    /// module's first attempt (`quadraui-1087-nonexistent-scheme://x`) and
    /// it is simply wrong on modern Windows: `ShellExecuteW` hands an
    /// unknown scheme off to the shell's "how do you want to open this?"
    /// / Store-lookup flow and returns **42 — success** — before any of
    /// that resolves, so the call reports success for a launch that never
    /// happens. It red-lined `ci.yml`'s "Test (win feature, real Windows)"
    /// step, which is the only leg that executes these `cfg(windows)`
    /// arms at all. Don't reintroduce it.
    #[cfg(windows)]
    fn unopenable_windows_target() -> std::ffi::OsString {
        std::ffi::OsString::from(format!(
            "C:\\quadraui-1087-no-such-directory-{}\\x.quadraui1087nohandler",
            std::process::id()
        ))
    }

    /// The real FFI call's real verdict on a target that cannot open:
    /// `SE_ERR_FNF`, which [`shell_execute_reports_success`] then reports
    /// as `false`. This is the Windows half of #1087's "stop lying about
    /// the launch" bar — the `$PATH`-stub tests below cover the Unix arm,
    /// which has a spawned binary to intercept and Windows does not.
    #[cfg(windows)]
    #[test]
    fn windows_shell_execute_open_reports_a_real_failure_code() {
        const SE_ERR_FNF: isize = 2;
        let target = unopenable_windows_target();
        assert_eq!(windows_shell_execute_open_code(&target), SE_ERR_FNF);
        assert!(!windows_shell_execute_open(&target));
    }

    /// `open_with_default`'s `ServiceResult` wrapping on Windows — the
    /// same bar `open_with_default_reports_err_when_the_opener_is_missing`
    /// sets on Unix, reached through a real `ShellExecuteW` call instead
    /// of a `$PATH` stub.
    #[cfg(windows)]
    #[test]
    fn open_with_default_reports_err_when_windows_cannot_open_the_target() {
        let target = unopenable_windows_target();
        assert_eq!(
            open_with_default(&target),
            Err(crate::backend::BackendError::PlatformFailure {
                context: format!("open_with_default: no opener launched for {target:?}"),
            })
        );
    }

    /// [`windows_shell_execute_open`] never builds a `std::process::Command`
    /// (that's the whole point — no `cmd.exe`/shell in the loop at all),
    /// so there is no command to assert on beyond invoking `ShellExecuteW`
    /// for real. What this pins down: the exact metacharacters (`&`, `|`,
    /// `^`, `%`) that made the original `cmd /c start` approach
    /// exploitable (issue #969 review) are carried through inertly — a
    /// bare FFI call has no command-line text for those bytes to land in,
    /// so they stay part of the *filename* and the call fails with
    /// `SE_ERR_FNF` rather than executing `calc.exe`.
    ///
    /// The target is an unopenable path (see
    /// [`unopenable_windows_target`]) rather than the `https://` URL this
    /// test first used: a real URL launches the host's actual browser on
    /// every CI run and on every developer's desktop, which is both a
    /// side effect a unit test has no business having and a way to wedge
    /// the run behind a shell dialog.
    #[cfg(windows)]
    #[test]
    fn windows_shell_execute_open_treats_shell_metacharacters_as_inert_text() {
        const SE_ERR_FNF: isize = 2;
        let target = std::ffi::OsString::from(format!(
            "C:\\quadraui-1087-no-such-directory-{}\\q=foo&run=bar|calc.exe^%1.quadraui1087nohandler",
            std::process::id()
        ));
        assert_eq!(windows_shell_execute_open_code(&target), SE_ERR_FNF);
    }

    /// #1087 acceptance bar: `try_open_with_default`/`open_with_default`
    /// through a real delegation — the same `$PATH`-override stub
    /// technique issue #969's own `tui`-only test used, generalised here
    /// since the opener itself is now shared. Safe to do without
    /// launching a real browser only because `$PATH` is temporarily
    /// redirected (guarded by [`PATH_OVERRIDE_TEST_LOCK`], and always
    /// restored via a drop guard even on panic) to a directory containing
    /// a stub executable under the exact name
    /// [`unix_open_with_default_command`] looks up on this platform
    /// (`open` on macOS, `xdg-open` elsewhere on Unix) that exits `0`
    /// immediately.
    ///
    /// **Unix only** — Windows' opener ([`windows_shell_execute_open`])
    /// is a direct `ShellExecuteW` FFI call with no `$PATH`-resolved
    /// binary to intercept this way; see
    /// `windows_shell_execute_open_does_not_panic_on_shell_metacharacters`
    /// above for that platform's own coverage of the real function, and
    /// `win::services::open_url_result_reports_failure_for_an_unregistered_scheme`
    /// for its `ServiceResult`-level failure coverage.
    #[cfg(unix)]
    #[test]
    fn try_open_with_default_returns_true_through_a_real_delegation() {
        let _guard = PATH_OVERRIDE_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());

        #[cfg(target_os = "macos")]
        const OPENER_NAME: &str = "open";
        #[cfg(all(unix, not(target_os = "macos")))]
        const OPENER_NAME: &str = "xdg-open";

        let tmp = tempfile::tempdir().expect("tempdir");
        let stub_path = tmp.path().join(OPENER_NAME);
        std::fs::write(&stub_path, b"#!/bin/sh\nexit 0\n").expect("write stub opener");
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&stub_path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod +x stub opener");
        }
        let _restore = RestorePath::capture();
        std::env::set_var("PATH", tmp.path());

        assert!(try_open_with_default(std::ffi::OsStr::new(
            "https://example.com/1087"
        )));
    }

    /// The other half of #1087's acceptance bar: with `$PATH` redirected
    /// to a directory containing *no* opener binary at all, the real
    /// spawn genuinely fails (`ENOENT`) and `try_open_with_default`
    /// honestly reports `false`. RED before this issue (`open_url_result`'s
    /// default just called the infallible `open_url` and always answered
    /// `Ok(())`). See `open_with_default_reports_err_when_the_opener_is_missing`
    /// below for the `ServiceResult`-level half of this — `open_with_default`
    /// itself is also compiled for a bare `--features tui` build (it backs
    /// `TuiPlatformServices::open_path`), even though `open_url_result`
    /// still calls `try_open_with_default` directly there so it can chain
    /// its own OSC 8 fallback.
    #[cfg(unix)]
    #[test]
    fn try_open_with_default_returns_false_when_the_opener_is_missing() {
        let _guard = PATH_OVERRIDE_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _restore = RestorePath::capture();
        let empty_dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var("PATH", empty_dir.path());

        assert!(!try_open_with_default(std::ffi::OsStr::new(
            "https://example.com/1087"
        )));
    }

    /// `open_with_default`'s own `ServiceResult` wrapping, via the `$PATH`
    /// stub technique — Unix only: this technique needs a `$PATH`-resolved
    /// binary to intercept, which only applies to `open_with_default`'s
    /// Unix arm. Runs under either `macos` or bare `tui` (the latter is
    /// `TuiPlatformServices::open_path`'s own coverage — issue #1087
    /// review, "TUI `open_path` left as a fourth un-consolidated copy").
    /// `win::services`'s own `ServiceResult`-level coverage
    /// (`open_url_result_reports_failure_for_an_unregistered_scheme`) uses
    /// a real `ShellExecuteW` call instead, since Windows' opener has no
    /// `$PATH`-resolved binary to intercept this way at all.
    #[cfg(any(
        all(feature = "macos", target_os = "macos"),
        all(feature = "tui", unix)
    ))]
    #[test]
    fn open_with_default_reports_err_when_the_opener_is_missing() {
        let _guard = PATH_OVERRIDE_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _restore = RestorePath::capture();
        let empty_dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var("PATH", empty_dir.path());

        assert_eq!(
            open_with_default(std::ffi::OsStr::new("https://example.com/1087")),
            Err(crate::backend::BackendError::PlatformFailure {
                context: "open_with_default: no opener launched for \"https://example.com/1087\""
                    .to_string(),
            })
        );
    }

    /// RAII guard restoring the process-wide `$PATH` this test module
    /// temporarily overrides, even on an early return or panic-unwind —
    /// same "always restore, drop-guard" posture
    /// [`crate::desktop::ModalPumpGuard`] and this crate's other
    /// global-state test helpers use.
    #[cfg(unix)]
    struct RestorePath(Option<std::ffi::OsString>);

    #[cfg(unix)]
    impl RestorePath {
        fn capture() -> Self {
            Self(std::env::var_os("PATH"))
        }
    }

    #[cfg(unix)]
    impl Drop for RestorePath {
        fn drop(&mut self) {
            match self.0.take() {
                Some(path) => std::env::set_var("PATH", path),
                None => std::env::remove_var("PATH"),
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// map_arboard_error (issue #1087)
// ─────────────────────────────────────────────────────────────────────

/// Map an `arboard::Error` from a named native call into a
/// [`crate::backend::BackendError::PlatformFailure`] (issue #954,
/// consolidated to one implementation by #1087 — `gtk::services` and
/// `macos::services` each carried an identical copy). `arboard::Error`
/// has no "unsupported" variant of its own — every arm this maps
/// (including `ContentNotAvailable`, e.g. "clipboard has no image right
/// now") is a real outcome of a call the calling backend *does*
/// implement, so `PlatformFailure` — not
/// [`crate::backend::BackendError::Unsupported`] — is the honest
/// mapping; see `BackendError::Unsupported`'s own doc for why that
/// variant is reserved for "this backend has no implementation" instead.
#[cfg(any(feature = "gtk", all(feature = "macos", target_os = "macos")))]
pub(crate) fn map_arboard_error(call: &str, err: arboard::Error) -> crate::backend::BackendError {
    crate::backend::BackendError::PlatformFailure {
        context: format!("{call}: {err}"),
    }
}

#[cfg(all(
    test,
    any(feature = "gtk", all(feature = "macos", target_os = "macos"))
))]
mod map_arboard_error_tests {
    use super::*;

    #[test]
    fn wraps_the_call_name_and_error_into_a_platform_failure() {
        let err = map_arboard_error("arboard::get_image", arboard::Error::ContentNotAvailable);
        match err {
            crate::backend::BackendError::PlatformFailure { context } => {
                assert!(context.contains("arboard::get_image"));
                assert!(context.contains(&arboard::Error::ContentNotAvailable.to_string()));
            }
            other => panic!("expected PlatformFailure, got {other:?}"),
        }
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

/// Which single platform modifier a backend's clipboard-paste chord
/// treats as native — Ctrl on Linux/GTK and Windows, Cmd (⌘) on macOS.
/// [`crate::Modifiers::cmd`] is the crate-wide abstraction for "this
/// platform's OS-level modifier key" (`win::events::win_modifiers` maps
/// the Windows key to `cmd` the same way `macos::events` maps ⌘; GTK's
/// `gdk_modifiers_to_quadraui` maps `SUPER_MASK`/`META_MASK` to `cmd` the
/// same way) — but that shared abstraction means `modifiers.cmd` can be
/// held on *any* backend (Super on Linux, the Windows key on Win32), not
/// only on macOS. [`is_paste_keypress`] takes this enum so each backend
/// tells the predicate which one of `ctrl`/`cmd` is *its* paste modifier,
/// instead of the predicate accepting either interchangeably — see
/// D-011 §4 (`docs/decisions/DECISIONS.md`) for the regression this fixes (plain
/// Super+V used to trigger paste on GTK, and plain Ctrl+V used to
/// trigger paste on macOS, neither of which is that platform's actual
/// convention).
///
/// `#[allow(dead_code)]`: each variant is constructed in production code
/// by exactly one backend (`Ctrl` by TUI/GTK/Windows via
/// [`crate::runtime::preprocess_event`], `Cmd` by macOS via the same
/// function's `PreprocessBackend::paste_modifier` override), and this
/// module compiles under `any(tui, gtk, macos, win)` — the same
/// single-backend feature-gate shape `win::run::dispatch_event`'s own
/// `#[allow(dead_code)]` documents. Building with only one of those
/// features active (e.g. the `ubuntu-latest` `--features win` leg, which
/// has no `tui`/`gtk`/`macos`) so only one variant is ever constructed
/// outside `#[cfg(test)]` would otherwise trip `-D warnings`' dead-code
/// lint on the other variant — despite both being genuinely constructed
/// on the CI legs where their respective backend feature is active, and
/// both being exercised unconditionally by `is_paste_keypress_tests`
/// below, which is what actually proves out D-011 §4's contract for
/// every backend on any host.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    all(feature = "macos", target_os = "macos"),
    feature = "win"
))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum PasteModifier {
    /// Ctrl-V / Ctrl-Shift-V — TUI, GTK (Linux), and Win32.
    Ctrl,
    /// Cmd-V / Cmd-Shift-V (⌘V / ⌘⇧V) — macOS.
    Cmd,
}

/// Is `key`+`modifiers` this platform's clipboard-paste shortcut?
///
/// One predicate for every desktop backend (#728); `native` names which
/// platform modifier the *calling* backend treats as "paste" — GTK and
/// `win` both pass [`PasteModifier::Ctrl`], macOS passes
/// [`PasteModifier::Cmd`]. Only that modifier is accepted: the *other*
/// one must be explicitly absent, not merely ignored, so e.g. plain
/// Super+V (GTK, where `modifiers.cmd` is Super) and plain Ctrl+V
/// (macOS) are both rejected exactly as they were before this predicate
/// was lifted out of `gtk::run`/inlined in `macos::run` — see D-011 §4
/// (`docs/decisions/DECISIONS.md`) for why "either modifier alone" was wrong.
/// `alt` is never a valid paste chord on any backend.
///
/// `shift` is deliberately **not** checked either way — Ctrl-Shift-V and
/// (by the same reasoning, extended here) Cmd-Shift-V both trigger paste
/// too. This was already GTK's behavior (quadraui#415: some terminal
/// emulators reserve plain Ctrl-V for a literal control byte and use
/// Ctrl-Shift-V as the paste shortcut instead), and D-011
/// (`docs/decisions/DECISIONS.md`) extends the same shift-tolerant contract to
/// every adopter — including macOS, which used to require `shift: false`
/// in its inline Cmd-V match guard before this predicate replaced it.
/// See D-011 for why this settles the contract once instead of letting
/// each new adopter (this issue's `win` included) silently pick its own.
///
/// Case-insensitive on the letter (`'v'` or `'V'`) on every backend —
/// this was already true everywhere a paste chord existed, so unifying
/// the predicate doesn't change it.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    all(feature = "macos", target_os = "macos"),
    feature = "win"
))]
pub(crate) fn is_paste_keypress(
    key: &crate::Key,
    modifiers: &crate::Modifiers,
    native: PasteModifier,
) -> bool {
    let (native_held, other_held) = match native {
        PasteModifier::Ctrl => (modifiers.ctrl, modifiers.cmd),
        PasteModifier::Cmd => (modifiers.cmd, modifiers.ctrl),
    };
    matches!(key, crate::Key::Char('v') | crate::Key::Char('V'))
        && native_held
        && !other_held
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

// ─────────────────────────────────────────────────────────────────────
// Caught-panic reporting dedup (#922)
// ─────────────────────────────────────────────────────────────────────
//
// `macos::run`'s `drawRect:`/responder-method bodies and `win::run`'s
// `wndproc` both hand a Rust callback directly to a C ABI that cannot
// unwind (objc2's `define_class!`-generated dispatch trampoline, Win32's
// `WNDPROC` contract) — an uncaught panic there escalates straight to
// `abort()` instead of staying a recoverable panic (vimcode#896: a
// reachable `todo!()` in a rasteriser took down the whole host). Both
// bracket every such body — or, for `win::run`, the shared
// `dispatch_event`/`render_frame` helpers every message ultimately
// funnels through — in `std::panic::catch_unwind(AssertUnwindSafe(..))`
// and report a caught panic through [`report_caught_panic_once`] rather
// than calling `crate::diagnostics::emit` directly, so a rasteriser gap
// re-entered on every redraw is reported once per process, not once per
// frame.
#[cfg(any(feature = "win", all(feature = "macos", target_os = "macos")))]
pub(crate) fn report_caught_panic_once(site: &str, payload: &(dyn std::any::Any + Send)) {
    static SEEN: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    let mut seen = SEEN
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Dedup key is `site` alone, not `site` + message: the goal is "don't
    // spam the same reachable panic on every repaint," not "distinguish
    // every possible payload string" — a `todo!()` at a fixed call site
    // always carries the same message anyway.
    if !seen.insert(site.to_string()) {
        return;
    }
    drop(seen);

    let message = panic_payload_message(payload);
    crate::diagnostics::emit(format!(
        "quadraui: panic caught at {site} — frame/event skipped, process kept alive ({message})"
    ));
}

/// Best-effort text for a `catch_unwind` payload. `Any` gives us nothing
/// beyond a downcast; the two shapes `panic!`/`todo!`/`unwrap()` actually
/// produce are a `&'static str` literal or a `String` from a `format!`ed
/// message, so those are the only two cases worth naming.
#[cfg(any(feature = "win", all(feature = "macos", target_os = "macos")))]
fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

#[cfg(all(
    test,
    any(feature = "win", all(feature = "macos", target_os = "macos"))
))]
mod panic_report_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Distinct `site` per test — the dedup registry is a process-global
    /// static shared by every test in this binary, so a colliding label
    /// between tests (or a re-run of the same test) would make the
    /// second call a silent no-op and the assertion below flaky.
    fn unique_site(tag: &str) -> String {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        format!("desktop::tests::{tag}#{n}")
    }

    /// Installs a recording sink and returns it. Callers must hold
    /// [`crate::diagnostics::test_guard`] for the duration — the sink
    /// slot is a process-global shared with `diagnostics.rs`'s own tests.
    fn install_recording_sink() -> Arc<Mutex<Vec<String>>> {
        let received: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let received_in_sink = Arc::clone(&received);
        crate::diagnostics::set_sink(move |msg: &str| {
            received_in_sink.lock().unwrap().push(msg.to_string());
        });
        received
    }

    #[test]
    fn first_report_at_a_site_reaches_the_sink() {
        let _g = crate::diagnostics::test_guard();
        let site = unique_site("first_report");
        let received = install_recording_sink();
        let payload: Box<dyn std::any::Any + Send> = Box::new("boom");
        report_caught_panic_once(&site, payload.as_ref());
        crate::diagnostics::clear_sink();
        assert_eq!(received.lock().unwrap().len(), 1);
        assert!(received.lock().unwrap()[0].contains(&site));
        assert!(received.lock().unwrap()[0].contains("boom"));
    }

    #[test]
    fn repeated_reports_at_the_same_site_are_reported_once() {
        let _g = crate::diagnostics::test_guard();
        let site = unique_site("repeated_reports");
        let received = install_recording_sink();
        for _ in 0..5 {
            let payload: Box<dyn std::any::Any + Send> = Box::new("same panic every frame");
            report_caught_panic_once(&site, payload.as_ref());
        }
        crate::diagnostics::clear_sink();
        assert_eq!(
            received.lock().unwrap().len(),
            1,
            "five panics at the same site must produce exactly one report, \
             not one per frame"
        );
    }

    #[test]
    fn distinct_sites_each_report_independently() {
        let _g = crate::diagnostics::test_guard();
        let site_a = unique_site("distinct_a");
        let site_b = unique_site("distinct_b");
        let received = install_recording_sink();
        let payload_a: Box<dyn std::any::Any + Send> = Box::new("a");
        let payload_b: Box<dyn std::any::Any + Send> = Box::new("b");
        report_caught_panic_once(&site_a, payload_a.as_ref());
        report_caught_panic_once(&site_b, payload_b.as_ref());
        crate::diagnostics::clear_sink();
        assert_eq!(received.lock().unwrap().len(), 2);
    }

    #[test]
    fn string_payload_message_is_included() {
        let _g = crate::diagnostics::test_guard();
        let site = unique_site("string_payload");
        let received = install_recording_sink();
        let payload: Box<dyn std::any::Any + Send> = Box::new(format!("formatted {}", 42));
        report_caught_panic_once(&site, payload.as_ref());
        crate::diagnostics::clear_sink();
        assert!(received.lock().unwrap()[0].contains("formatted 42"));
    }

    #[test]
    fn non_string_payload_does_not_panic_the_reporter() {
        let _g = crate::diagnostics::test_guard();
        let site = unique_site("non_string_payload");
        let received = install_recording_sink();
        let payload: Box<dyn std::any::Any + Send> = Box::new(42i32);
        report_caught_panic_once(&site, payload.as_ref());
        crate::diagnostics::clear_sink();
        assert_eq!(received.lock().unwrap().len(), 1);
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
        feature = "tui",
        feature = "gtk",
        all(feature = "macos", target_os = "macos"),
        feature = "win"
    )
))]
mod is_paste_keypress_tests {
    //! Coverage for [`is_paste_keypress`] (#728) — the single predicate
    //! `tui::run`, `gtk::run`, `macos::run`, and `win::run`'s
    //! `dispatch_event`s all now call (via
    //! `crate::runtime::preprocess_event`), each backend supplying its
    //! own [`PasteModifier`]. Pure/display-
    //! free: no live backend needed to exercise every branch of the
    //! contract D-011 (`docs/decisions/DECISIONS.md`) records.
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
    fn plain_ctrl_v_is_a_paste_keypress_for_ctrl_native_backends() {
        // GTK (Linux) and Win32.
        assert!(is_paste_keypress(
            &Key::Char('v'),
            &mods(true, false, false, false),
            PasteModifier::Ctrl
        ));
        assert!(is_paste_keypress(
            &Key::Char('V'),
            &mods(true, false, false, false),
            PasteModifier::Ctrl
        ));
    }

    #[test]
    fn plain_cmd_v_is_a_paste_keypress_for_cmd_native_backends() {
        // macOS — `cmd` is the crate-wide abstraction for the platform
        // OS-modifier key (⌘ on macOS).
        assert!(is_paste_keypress(
            &Key::Char('v'),
            &mods(false, false, false, true),
            PasteModifier::Cmd
        ));
    }

    #[test]
    fn plain_cmd_v_is_not_a_paste_keypress_for_ctrl_native_backends() {
        // D-011 §4 regression check: on GTK, `modifiers.cmd` is Super
        // (SUPER_MASK/META_MASK — see `gtk::events`), and on `win` it's
        // the Windows key. Plain Super+V / Win+V must NOT trigger paste
        // just because *some* backend's native modifier is held — GTK's
        // pre-lift `is_paste_keypress` never accepted it, and adopting
        // the shared predicate must not silently start accepting it.
        assert!(!is_paste_keypress(
            &Key::Char('v'),
            &mods(false, false, false, true),
            PasteModifier::Ctrl
        ));
    }

    #[test]
    fn plain_ctrl_v_is_not_a_paste_keypress_for_cmd_native_backends() {
        // D-011 §4 regression check: macOS's pre-lift inline Cmd-V guard
        // required `ctrl: false` exactly, so plain Ctrl-V never triggered
        // paste on macOS. The shared predicate must preserve that.
        assert!(!is_paste_keypress(
            &Key::Char('v'),
            &mods(true, false, false, false),
            PasteModifier::Cmd
        ));
    }

    #[test]
    fn ctrl_shift_v_is_a_paste_keypress_for_ctrl_native_backends() {
        // quadraui#415: some terminal emulators reserve plain Ctrl-V for
        // a control byte and use Ctrl-Shift-V for paste instead.
        assert!(is_paste_keypress(
            &Key::Char('v'),
            &mods(true, true, false, false),
            PasteModifier::Ctrl
        ));
    }

    #[test]
    fn cmd_shift_v_is_a_paste_keypress_for_cmd_native_backends() {
        // D-011: the same shift-tolerant contract extends to macOS's
        // Cmd-V, which used to require `shift: false` before this
        // predicate replaced `macos::run`'s inline match guard.
        assert!(is_paste_keypress(
            &Key::Char('v'),
            &mods(false, true, false, true),
            PasteModifier::Cmd
        ));
    }

    #[test]
    fn ctrl_alt_v_is_not_a_paste_keypress() {
        assert!(!is_paste_keypress(
            &Key::Char('v'),
            &mods(true, false, true, false),
            PasteModifier::Ctrl
        ));
    }

    #[test]
    fn cmd_alt_v_is_not_a_paste_keypress() {
        assert!(!is_paste_keypress(
            &Key::Char('v'),
            &mods(false, false, true, true),
            PasteModifier::Cmd
        ));
    }

    #[test]
    fn both_ctrl_and_cmd_is_not_a_paste_keypress() {
        // The other platform modifier must be explicitly absent, not
        // merely not-required — holding both is rejected regardless of
        // which one is native to the backend.
        assert!(!is_paste_keypress(
            &Key::Char('v'),
            &mods(true, false, false, true),
            PasteModifier::Ctrl
        ));
        assert!(!is_paste_keypress(
            &Key::Char('v'),
            &mods(true, false, false, true),
            PasteModifier::Cmd
        ));
    }

    #[test]
    fn shift_v_alone_is_not_a_paste_keypress() {
        assert!(!is_paste_keypress(
            &Key::Char('v'),
            &mods(false, true, false, false),
            PasteModifier::Ctrl
        ));
    }

    #[test]
    fn plain_v_is_not_a_paste_keypress() {
        assert!(!is_paste_keypress(
            &Key::Char('v'),
            &Modifiers::default(),
            PasteModifier::Ctrl
        ));
    }

    #[test]
    fn ctrl_c_is_not_a_paste_keypress() {
        assert!(!is_paste_keypress(
            &Key::Char('c'),
            &mods(true, false, false, false),
            PasteModifier::Ctrl
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
