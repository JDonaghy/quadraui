//! Shared, backend-neutral desktop-interaction plumbing.
//!
//! Everything in this module is genuinely windowing-generic: it has no
//! dependency on `gtk4`, `objc2`, `windows`, or any other toolkit crate,
//! and none of the *code* here needs a toolkit feature to compile — the
//! whole point of extracting it is that a brand-new backend gets it for
//! free. Each piece used to be implemented once inside `gtk/` only; a
//! macOS or Windows backend would otherwise have had to reinvent it from
//! scratch:
//!
//! - [`WindowDragArm`] — the arm/threshold/commit state machine behind
//!   a CSD titlebar drag-to-move (or edge-resize) gesture. Extracted
//!   from `gtk/backend.rs`'s `armed_window_drag` field +
//!   `gtk/run.rs`'s motion-controller threshold check. **Not**
//!   adopted by `win::run`/`win::backend`: `win::run` creates its
//!   window with `WS_OVERLAPPEDWINDOW` — the standard native title bar
//!   and non-client resize border, not a client-side-decorated one —
//!   so title-bar drag-to-move and edge-resize are already handled
//!   entirely inside Windows' own non-client (`WM_NC*`) message
//!   handling; there is no quadraui-drawn titlebar/border for an app to
//!   press-and-drag on, so there is nothing for this arm/threshold/commit
//!   sequencing to gate.
//! - [`ModalPumpDepth`] / [`ModalPumpGuard`] — the re-entrancy guard
//!   around a nested native modal loop (a GTK async-dialog
//!   `MainContext::iteration` pump, an AppKit `runModal` /
//!   `performWindowDragWithEvent:`, a Win32 `IFileOpenDialog::Show`).
//!   Extracted from `gtk/services.rs`'s `pumping` field + `PumpGuard`.
//!   Adopted by `win::run`'s `wndproc` → `app.handle`/`app.render`
//!   dispatch via `win::run`'s own `guarded_call` — the actual
//!   `RefCell` double-borrow guard lives there since it has no toolkit
//!   dependency of its own either; see that function's docs.
//! - [`SmokeConfig`] + [`smoke_size_ok`] / [`smoke_clipboard_round_trip_ok`]
//!   — the headless smoke-mode predicates behind `QUADRAUI_*_SMOKE_MS` /
//!   `_SMOKE_PASTE`. Extracted from `gtk/run.rs`, renamed backend-neutral
//!   (no more `Gtk` in the name) and parameterised over the env-var
//!   names and size floor so a future backend can reuse the mechanism
//!   under its own env vars and default window size. Adopted by
//!   `win::run`'s `QUADRAUI_WIN_SMOKE_MS`/`_SMOKE_PASTE` one-shot
//!   `WM_TIMER` check alongside GTK's `QUADRAUI_GTK_SMOKE_MS`/
//!   `_SMOKE_PASTE`.
//! - [`ALL_RESIZE_EDGES`] / [`all_pointer_shapes`] — an enum-walk
//!   scaffold for a [`PointerShape`] → native-cursor lookup table.
//!   Every backend still owns its own table (cursor *names*/objects are
//!   inherently native), but this gives it a canonical, exhaustively-
//!   maintained list of inputs to build and test that table against
//!   instead of hand-duplicating the variant list per backend. Adopted by
//!   `win::backend`'s `PointerShape` → `IDC_*` cursor-resource mapping,
//!   driving `SetCursor`/`WM_SETCURSOR`.
//! - [`is_paste_keypress`] — the clipboard-paste keypress predicate.
//!   Extracted from `gtk::run`'s private `is_paste_keypress` and
//!   generalised to also cover macOS's Cmd-V, which used to be an inline
//!   `match` guard in `macos::run::dispatch_event` instead of a named,
//!   independently-testable predicate. See `docs/decisions/DECISIONS.md`
//!   D-011 for the shift-tolerance contract this settles once for every
//!   adopter instead of each backend picking its own, and D-011 §4
//!   specifically for why the predicate takes a [`PasteModifier`]
//!   parameter naming *which* platform modifier is native to the calling
//!   backend, rather than accepting either `ctrl` or `cmd`
//!   interchangeably. Adopted by `win::run::dispatch_event` to give
//!   Win-GUI its first `UiEvent::ClipboardPaste` at all.
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
//! the workspace-wide `RUSTFLAGS: "-D warnings"` turns an
//! unused-`pub(crate)`-item warning into a hard build failure the moment
//! a feature combination compiles this module without any backend around
//! to call into it (e.g. `--features tui`, which has no window chrome
//! at all). Each item is gated on the feature(s) of the backend(s) that
//! actually call it today, not left permanently `#[allow(dead_code)]` —
//! adding a new adopter (e.g. a `win` backend wiring up `WindowDragArm`)
//! is a one-line `cfg` addition, not a license to leave the guard off.

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

// ─────────────────────────────────────────────────────────────────────
// move_to_trash
// ─────────────────────────────────────────────────────────────────────

/// Move `path` to the platform trash/recycle bin via the cross-platform
/// `trash` crate — the one implementation shared by every
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
/// `Err(BackendError::Unsupported)` degrade TUI's remaining
/// desktop-shell-flavoured methods (native dialogs, notably) still fall
/// back to — [`reveal_in_file_manager`] below is the other
/// one that has since earned a real, best-effort implementation instead
/// of that placeholder.
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
// wide_nul_terminated
// ─────────────────────────────────────────────────────────────────────

/// UTF-16, NUL-terminated — the framing every wide (`W`-suffixed) Win32
/// API expects (`ShellExecuteW`'s `operation`/`file`, `CF_UNICODETEXT`'s
/// clipboard payload, `NOTIFYICONDATAW`'s fixed-size fields, a context
/// menu item's label). The single implementation shared by
/// `win::services`, `win::tray`, `win::backend`, and this module's own
/// [`windows_shell_execute_open`] — before this, each of the first three
/// modules carried its own private copy of this exact two-line
/// conversion, and [`windows_shell_execute_open`] needed a fourth.
///
/// `#[cfg_attr(not(target_os = "windows"), allow(dead_code))]`: every
/// real caller only exists under `target_os = "windows"` (the WinAPI
/// call sites this feeds, and this module's own Windows-only opener), so
/// on any other host this is unreachable dead code the moment it
/// compiles at all under `--features win` alone.
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
// open_with_default
// ─────────────────────────────────────────────────────────────────────

/// Open `target` (a URL string or a filesystem path, carried as `&OsStr`
/// so a non-UTF-8 path need not be lossily converted before the call) —
/// the single opener implementation shared by `tui::services`, `macos::services`,
/// and `win::services`.
///
/// Unix: spawns `open` (macOS) / `xdg-open` (other Unix), detached
/// (stdout/stderr nulled so a slow or chatty opener can't wedge the
/// caller's own streams — matters most for `tui`, which shares stdout
/// with the terminal UI) and never through a shell — `execve`, so
/// `target` is never re-parsed for shell metacharacters. Windows:
/// `ShellExecuteW(NULL, "open", target, NULL, NULL, SW_SHOWNORMAL)`
/// through a **minimal, hand-written FFI declaration**, not the
/// `windows` crate — `windows` is `optional`, pulled in only by the
/// `win` feature's `dep:windows`, so it is not a dependency of a bare
/// `--features tui` build, and this must compile there too. Never goes
/// through `cmd.exe`: the `cmd /c start` approach is a CWE-78
/// command-injection hole, since `cmd.exe` re-parses its own command-line
/// text for `&`/`|`/`^`/`%` regardless of how the argument was quoted for
/// `CreateProcess`.
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
/// this same function (below) for the shared shape and doc.
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
/// below (and `macos::services`'s `open_url_result` failure test) that
/// temporarily redirects it to prove a real "opener missing" failure must
/// serialize against every *other* such test in the same process, or a
/// concurrently-running test can observe (or clobber) the override. One
/// crate-wide lock rather than a per-module copy so tests under different
/// feature combinations can't race each other.
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
/// macOS build of the identical source stays green.
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
    /// as `false`. This is the Windows half of the "stop lying about
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
    /// exploitable are carried through inertly — a
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

    /// Acceptance bar: `try_open_with_default`/`open_with_default`
    /// through a real delegation — the same `$PATH`-override stub
    /// technique generalised from a previous test. Safe to do without
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

    /// The other half of the acceptance bar: with `$PATH` redirected
    /// to a directory containing *no* opener binary at all, the real
    /// spawn genuinely fails (`ENOENT`) and `try_open_with_default`
    /// honestly reports `false`. See `open_with_default_reports_err_when_the_opener_is_missing`
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
    /// Unix arm. Runs under either `macos` or bare `tui`. `win::services`'s
    /// own `ServiceResult`-level coverage uses a real `ShellExecuteW` call
    /// instead, since Windows' opener has no `$PATH`-resolved binary to
    /// intercept this way at all.
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
// reveal_in_file_manager
// ─────────────────────────────────────────────────────────────────────

/// The TUI degrade for `PlatformServices::reveal_in_file_manager`. A
/// terminal session on a desktop can reveal a file in its file manager
/// exactly as readily as it can launch a browser, so `Unsupported` was a
/// placeholder, not a structural fact.
///
/// Tries, per platform:
/// - **Windows**: [`windows_explorer_select`] —
///   `ShellExecuteW(NULL, "open", "explorer.exe", "/select,\"<path>\"",
///   NULL, SW_SHOWNORMAL)`, the same minimal hand-written FFI declaration
///   [`windows_shell_execute_open_code`] uses, pointed at `explorer.exe`
///   with an explicit `/select,` switch instead of handing `path`
///   straight to the registered "open" handler.
/// - **macOS**: [`macos_reveal_in_file_manager_command`] — `open -R
///   <path>`, the CLI front end to the same
///   `NSWorkspace::activateFileViewerSelectingURLs` call
///   `macos::services::MacPlatformServices::reveal_in_file_manager` makes
///   directly; TUI has no AppKit to link against, so this reaches for the
///   command-line door into the identical native call instead.
/// - **Linux/BSD**: [`linux_show_items_via_dbus`] —
///   `org.freedesktop.FileManager1.ShowItems` over the session D-Bus, the
///   same standard method
///   `gtk::services::GtkPlatformServices::reveal_in_file_manager` drives
///   through a linked `gio::DBusConnection::call_sync`; TUI has no `gio`
///   (a `gtk`-feature-only dependency), so this drives the identical
///   well-known method through the `dbus-send` CLI tool instead — the
///   same "reach for the CLI door onto a native facility instead of
///   linking a client library" trade [`write_clipboard_via_native_tool`]
///   already makes for the system clipboard. Falls back to
///   [`try_open_with_default`] on `path`'s *parent directory* when that
///   call fails (no `FileManager1` listener — a headless container, a
///   minimal window manager with no file manager running): not a perfect
///   "reveal, selected" experience, but a real file-manager window opened
///   at the right place, on a desktop session that plainly has one,
///   instead of a blanket `Unsupported`.
///
/// Best-effort throughout, matching every other `ServiceResult`-returning
/// TUI degrade in this crate: a successful spawn/D-Bus call reports
/// `Ok(())` even though the launched file manager may itself fail to open
/// a window with no way for this call to observe it.
#[cfg(feature = "tui")]
pub(crate) fn reveal_in_file_manager(path: &std::path::Path) -> crate::backend::ServiceResult<()> {
    if try_reveal_in_file_manager(path) {
        Ok(())
    } else {
        Err(crate::backend::BackendError::Unsupported)
    }
}

/// The raw "did anything launch" half of [`reveal_in_file_manager`],
/// split out (mirroring [`try_open_with_default`]'s own split from
/// [`open_with_default`]) purely so tests can exercise the per-platform
/// dispatch without wrapping every assertion in a `ServiceResult`.
#[cfg(feature = "tui")]
fn try_reveal_in_file_manager(path: &std::path::Path) -> bool {
    #[cfg(windows)]
    {
        windows_explorer_select(path)
    }
    #[cfg(target_os = "macos")]
    {
        macos_reveal_in_file_manager_command(path).spawn().is_ok()
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if linux_show_items_via_dbus(path) {
            return true;
        }
        match path.parent() {
            Some(parent) => try_open_with_default(parent.as_os_str()),
            None => false,
        }
    }
}

/// macOS opener command, not yet spawned — `open -R <path>` ("Reveal in
/// Finder" from the command line). Factored out so a test can assert on
/// the program name and arguments a real call would spawn without
/// actually launching anything, the same shape
/// [`unix_open_with_default_command`] uses.
#[cfg(all(target_os = "macos", feature = "tui"))]
fn macos_reveal_in_file_manager_command(path: &std::path::Path) -> std::process::Command {
    let mut cmd = std::process::Command::new("open");
    cmd.arg("-R")
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd
}

/// Windows opener: `ShellExecuteW`, pointed at `explorer.exe` with an
/// explicit `/select,"<path>"` parameter — see [`reveal_in_file_manager`]'s
/// doc for why. Returns whether `ShellExecuteW` reports success, per
/// [`shell_execute_reports_success`]'s `> 32` rule, the same predicate
/// [`windows_shell_execute_open`] already uses.
#[cfg(all(windows, feature = "tui"))]
fn windows_explorer_select(path: &std::path::Path) -> bool {
    shell_execute_reports_success(windows_explorer_select_code(path))
}

/// The raw `ShellExecuteW(NULL, "open", "explorer.exe",
/// "/select,\"<path>\"", NULL, SW_SHOWNORMAL)` return value. Split from
/// [`windows_explorer_select`] so a test can assert on the actual
/// documented `SE_ERR_*` code a given path produces on a real host —
/// same split [`windows_shell_execute_open_code`] has from
/// [`windows_shell_execute_open`]. `path.display()` is safe to embed
/// unquoted-of-inner-quotes here: Windows' filesystem itself forbids `"`
/// in a path, so there is no in-band way for `path` to break out of the
/// `/select,"..."` quoting this builds. That covers embedded quotes, but
/// not the classic MSVCRT argv-splitting edge case where a path's literal
/// string ends in a backslash immediately before the closing quote (an
/// odd run of trailing backslashes there escapes the quote instead of
/// closing the string) — not exploitable for injection (nothing follows
/// for an escaped quote to expose) and not reachable through
/// `path.display()` for an ordinary file/directory path (which doesn't
/// carry a trailing separator), so [`windows_explorer_select_parameters`]
/// strips one defensively rather than leaving it as a latent trap.
#[cfg(all(windows, feature = "tui"))]
fn windows_explorer_select_code(path: &std::path::Path) -> isize {
    // SAFETY: see `windows_shell_execute_open_code`'s identical SAFETY
    // note — both wide-string buffers below are nul-terminated and kept
    // alive for the duration of the call.
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
    let file = wide_nul_terminated("explorer.exe");
    let parameters = wide_nul_terminated(&windows_explorer_select_parameters(path));
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            parameters.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    }
}

/// Build the `/select,"<path>"` parameter string
/// [`windows_explorer_select_code`] hands `explorer.exe`, split out (pure
/// string formatting, no WinAPI) so it is unit-testable on every host —
/// the FFI call itself is not. Mirrors
/// [`shell_execute_reports_success`]'s "no WinAPI, so it runs everywhere"
/// posture: unconditionally compiled under `feature = "tui"` rather than
/// `cfg(windows)`-gated, so `ci.yml`'s ubuntu-leg `cargo test --features
/// tui` exercises it too.
///
/// `#[cfg_attr(not(windows), allow(dead_code))]` for the same reason
/// [`wide_nul_terminated`] carries one: its only non-test caller
/// ([`windows_explorer_select_code`]) is Windows-only.
#[cfg(feature = "tui")]
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_explorer_select_parameters(path: &std::path::Path) -> String {
    // Strip a trailing separator before quoting: an odd run of
    // backslashes immediately before the closing `"` escapes that quote
    // rather than closing the string under MSVCRT-style argv splitting
    // (see this function's doc comment above its caller,
    // `windows_explorer_select_code`, for the full reasoning). Ordinary
    // file/directory paths never carry one, so this is a defensive
    // no-op in practice, not a behavior change for real callers.
    let mut rendered = path.display().to_string();
    while rendered.len() > 1 && (rendered.ends_with('\\') || rendered.ends_with('/')) {
        rendered.pop();
    }
    format!("/select,\"{rendered}\"")
}

/// Build the `dbus-send` invocation for
/// `org.freedesktop.FileManager1.ShowItems` — not yet
/// spawned, so a test can assert on the constructed command without a
/// live D-Bus session. Drives the same standard method
/// `gtk::services::GtkPlatformServices::reveal_in_file_manager` reaches
/// through a linked `gio::DBusConnection::call_sync`, at the CLI-tool
/// level instead — see [`reveal_in_file_manager`]'s doc for why TUI can't
/// link `gio` directly.
///
/// `None` when `path` has no well-defined `file://` URI — see
/// [`path_to_file_uri`] — because `ShowItems` takes URIs and there is no
/// honest invocation to build without one.
#[cfg(all(unix, not(target_os = "macos"), feature = "tui"))]
fn linux_show_items_via_dbus_command(path: &std::path::Path) -> Option<std::process::Command> {
    // `PlatformServices::reveal_in_file_manager` is public and forwards
    // the consumer's `&Path` verbatim, so a relative path reaches here.
    // Bail out rather than substituting an empty URI: `array:string:`
    // with an empty value is a one-element array holding the empty
    // string, which a session that *does* have a `FileManager1` listener
    // answers with a success exit — a false `Ok(())` for a reveal that
    // never happened. Reporting `None` lets the caller fall through to
    // the `xdg-open`-the-parent leg instead.
    let uri = path_to_file_uri(path)?;
    let mut cmd = std::process::Command::new("dbus-send");
    cmd.args([
        "--session",
        "--dest=org.freedesktop.FileManager1",
        "--type=method_call",
        "/org/freedesktop/FileManager1",
        "org.freedesktop.FileManager1.ShowItems",
    ])
    .arg(format!("array:string:{uri}"))
    .arg("string:")
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null());
    Some(cmd)
}

/// Run [`linux_show_items_via_dbus_command`], reporting whether
/// `dbus-send` itself exited successfully — `dbus-send` reports a
/// non-zero exit for a D-Bus-level failure (no session bus, no
/// `FileManager1` listener) exactly the way a failed native
/// `gio::DBusConnection::call_sync` would, so an exit-status check is
/// enough; there is no reply payload this call needs to read back.
///
/// `false` when there is no command to run at all (an un-URI-able path),
/// which reads the same way to the caller as a failed D-Bus call: this
/// leg didn't reveal anything, try the next one.
#[cfg(all(unix, not(target_os = "macos"), feature = "tui"))]
fn linux_show_items_via_dbus(path: &std::path::Path) -> bool {
    linux_show_items_via_dbus_command(path)
        .and_then(|mut cmd| cmd.status().ok())
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Percent-encode `path` into a `file://` URI (RFC 8089), honoring the
/// host's native path flavor rather than assuming POSIX:
///
/// - **Unix** (including macOS): every byte outside RFC 3986's
///   unreserved set (`ALPHA` / `DIGIT` / `-._~`) plus the path separator
///   `/` is escaped as `%XX`. Operates on the path's raw `OsStr` bytes,
///   not a lossy `&str` conversion, so it is lossless for a path with no
///   valid UTF-8 representation. In particular escapes `,` (which would
///   otherwise be misread as a `dbus-send` `array:string:` element
///   separator — [`linux_show_items_via_dbus_command`]'s use) and
///   whitespace, both of which a real filesystem path can easily carry.
/// - **Windows**: routed through [`file_uri_from_windows_path_str`] for
///   drive-letter (`C:\a\b.txt` -> `file:///C:/a/b.txt`) and UNC
///   (`\\server\share\f` -> `file://server/share/f`) handling — a raw
///   `file://{backslash-path}` concatenation is not a parseable URI.
///
/// Returns `None` for a path with no well-defined `file://` form: a
/// relative path (no leading `/` on Unix, no drive letter or UNC prefix
/// on Windows) would otherwise render with its first path segment read
/// back as the URI *authority* instead of as part of the path. A host
/// that is neither Unix nor Windows has no native path flavor this crate
/// knows how to render and gets `None` for everything.
///
/// [`gtk::services::GtkPlatformServices::reveal_in_file_manager`] gets
/// the Unix encoding for free from `gio::File::for_path(path).uri()`;
/// TUI has no `gio` to call, so this is the hand-rolled equivalent, also
/// reused unconditionally by [`crate::drag::DragOutPayload::to_uri_list`]
/// — which is why this function itself carries no backend/feature `cfg`.
pub(crate) fn path_to_file_uri(path: &std::path::Path) -> Option<String> {
    #[cfg(windows)]
    {
        file_uri_from_windows_path_str(&path.to_string_lossy())
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;

        let bytes = path.as_os_str().as_bytes();
        if !bytes.starts_with(b"/") {
            return None;
        }
        let mut out = String::from("file://");
        for byte in bytes {
            percent_encode_byte(&mut out, *byte, true);
        }
        Some(out)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = path;
        None
    }
}

/// Percent-encode one byte into `out` per RFC 3986's unreserved set
/// (`ALPHA` / `DIGIT` / `-._~`); `keep_slash` additionally passes `/`
/// through literally, for a caller encoding a whole `/`-separated path
/// body in one pass rather than per-segment.
fn percent_encode_byte(out: &mut String, b: u8, keep_slash: bool) {
    let is_unreserved = b.is_ascii_alphanumeric()
        || matches!(b, b'-' | b'.' | b'_' | b'~')
        || (keep_slash && b == b'/');
    if is_unreserved {
        out.push(b as char);
    } else {
        out.push_str(&format!("%{b:02X}"));
    }
}

/// Percent-encode a single Windows path segment (no `/` passthrough —
/// the caller has already split on separators).
#[cfg_attr(not(windows), allow(dead_code))]
fn encode_windows_segment(out: &mut String, segment: &str) {
    for b in segment.bytes() {
        percent_encode_byte(out, b, false);
    }
}

/// Pure string-level translation of a Windows-flavored path string into
/// its `file://` form — factored out of [`path_to_file_uri`] (rather
/// than operating on a host `&Path`) so the drive-letter and UNC shapes
/// can be pinned by a test on any host, not only a real Windows one:
/// `Path`'s own component parsing is host-dependent (a Linux host treats
/// a backslash as an ordinary filename byte, not a separator), so
/// driving this logic through `Path` there would silently test nothing.
///
/// - A UNC prefix (`\\server\share\path`, or `//server/share/path`)
///   becomes `file://server/share/path` — the server name is the URI
///   authority.
/// - A drive letter (`C:\a\b.txt`, or `C:/a/b.txt`) becomes
///   `file:///C:/a/b.txt` — empty authority, drive letter as the first
///   path segment, and a bare drive root (`C:\`) keeps its trailing
///   slash (`file:///C:/`). The colon is kept literal (RFC 8089
///   Appendix A convention) rather than percent-encoded.
/// - A verbatim prefix (`\\?\C:\a`, `\\?\UNC\server\share\f`) is
///   stripped first and the remainder classified as the drive-letter or
///   UNC form it names. This is the shape `std::fs::canonicalize`
///   returns on Windows, and therefore the obvious way a caller
///   satisfies this function's absolute-path requirement.
/// - Anything else (no drive letter, no UNC prefix — a relative path)
///   returns `None`; see [`path_to_file_uri`]'s doc for why.
#[cfg_attr(not(windows), allow(dead_code))]
fn file_uri_from_windows_path_str(raw: &str) -> Option<String> {
    // `\\?\UNC\` names a UNC path whose body no longer carries the
    // leading `\\`, so remember that the UNC branch still applies;
    // without this pre-pass both verbatim forms take the UNC branch
    // below with `?` read back as the server name.
    let (raw, verbatim_unc) = if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        (rest, true)
    } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
        (rest, false)
    } else {
        (raw, false)
    };

    let unc = if verbatim_unc {
        Some(raw)
    } else {
        raw.strip_prefix(r"\\").or_else(|| raw.strip_prefix("//"))
    };
    if let Some(unc) = unc {
        let mut parts = unc.splitn(2, ['\\', '/']);
        let server = parts.next().filter(|s| !s.is_empty())?;
        let share_and_path = parts.next().unwrap_or("");

        let mut out = String::from("file://");
        encode_windows_segment(&mut out, server);
        for segment in share_and_path.split(['\\', '/']).filter(|s| !s.is_empty()) {
            out.push('/');
            encode_windows_segment(&mut out, segment);
        }
        return Some(out);
    }

    let mut chars = raw.chars();
    let drive = chars.next().filter(char::is_ascii_alphabetic)?;
    if chars.next() != Some(':') {
        return None;
    }

    let mut out = format!("file:///{drive}:/");
    for (i, segment) in raw[2..]
        .split(['\\', '/'])
        .filter(|s| !s.is_empty())
        .enumerate()
    {
        if i > 0 {
            out.push('/');
        }
        encode_windows_segment(&mut out, segment);
    }
    Some(out)
}

/// Covers [`path_to_file_uri`] and its Windows string-level half. Not
/// gated on `feature = "tui"` — unlike
/// [`reveal_in_file_manager_tests`], whose subjects are TUI-only — so
/// these also run under the real-Windows command
/// (`tools/win-test.sh`, `--features win`), the host they describe.
#[cfg(test)]
mod file_uri_tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn path_to_file_uri_percent_encodes_spaces_and_commas() {
        assert_eq!(
            path_to_file_uri(std::path::Path::new("/tmp/a,b c/d.txt")),
            Some("file:///tmp/a%2Cb%20c/d.txt".to_string())
        );
    }

    #[cfg(unix)]
    #[test]
    fn path_to_file_uri_is_none_for_a_relative_path() {
        // A relative path has no well-defined `file://` form: rendering
        // it anyway would read its first segment back as the URI
        // authority instead of as part of the path.
        assert_eq!(path_to_file_uri(std::path::Path::new("a.txt")), None);
    }

    #[cfg(windows)]
    #[test]
    fn path_to_file_uri_renders_a_windows_drive_letter_path() {
        assert_eq!(
            path_to_file_uri(std::path::Path::new(r"C:\dir\a b.txt")),
            Some("file:///C:/dir/a%20b.txt".to_string())
        );
        assert_eq!(path_to_file_uri(std::path::Path::new(r"dir\a.txt")), None);
    }

    /// Pure string-level logic, no WinAPI — runs on every host, same
    /// posture as
    /// [`reveal_in_file_manager_tests::windows_explorer_select_parameters_quotes_the_path`].
    #[test]
    fn file_uri_from_windows_path_str_handles_a_drive_letter() {
        assert_eq!(
            file_uri_from_windows_path_str(r"C:\dir\a b.txt"),
            Some("file:///C:/dir/a%20b.txt".to_string())
        );
    }

    #[test]
    fn file_uri_from_windows_path_str_keeps_a_bare_drive_root_slash() {
        assert_eq!(
            file_uri_from_windows_path_str(r"C:\"),
            Some("file:///C:/".to_string())
        );
        assert_eq!(
            file_uri_from_windows_path_str("C:"),
            Some("file:///C:/".to_string())
        );
    }

    #[test]
    fn file_uri_from_windows_path_str_handles_a_unc_share() {
        assert_eq!(
            file_uri_from_windows_path_str(r"\\server\share\a b.txt"),
            Some("file://server/share/a%20b.txt".to_string())
        );
    }

    #[test]
    fn file_uri_from_windows_path_str_strips_a_verbatim_prefix() {
        // `std::fs::canonicalize` hands back these forms on Windows.
        assert_eq!(
            file_uri_from_windows_path_str(r"\\?\C:\dir\a b.txt"),
            Some("file:///C:/dir/a%20b.txt".to_string())
        );
        assert_eq!(
            file_uri_from_windows_path_str(r"\\?\UNC\server\share\f.txt"),
            Some("file://server/share/f.txt".to_string())
        );
    }

    #[test]
    fn file_uri_from_windows_path_str_is_none_for_a_relative_path() {
        assert_eq!(file_uri_from_windows_path_str(r"dir\a.txt"), None);
    }
}

#[cfg(all(test, feature = "tui"))]
mod reveal_in_file_manager_tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_reveal_in_file_manager_command_uses_open_dash_r() {
        let cmd = macos_reveal_in_file_manager_command(std::path::Path::new("/tmp/quadraui1092"));
        assert_eq!(cmd.get_program(), "open");
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            ["-R", "/tmp/quadraui1092"]
        );
    }

    /// Pure string formatting, no WinAPI — runs on every host, including
    /// `ci.yml`'s ubuntu-leg `cargo test --features tui`, same posture
    /// [`shell_execute_reports_success_tests`]'s own doc explains.
    /// `Path::new(r"C:\a b\c.txt")` renders identically via `.display()`
    /// on Unix (one opaque path component, backslashes preserved as
    /// literal bytes) and on Windows (a real backslash-separated path),
    /// so the expected output is the same on either host.
    #[test]
    fn windows_explorer_select_parameters_quotes_the_path() {
        assert_eq!(
            windows_explorer_select_parameters(std::path::Path::new(r"C:\a b\c.txt")),
            r#"/select,"C:\a b\c.txt""#
        );
    }

    #[test]
    fn windows_explorer_select_parameters_strips_a_trailing_separator() {
        // A path ending in a backslash right before the closing quote
        // would otherwise escape that quote under MSVCRT-style argv
        // splitting instead of closing the string.
        assert_eq!(
            windows_explorer_select_parameters(std::path::Path::new(r"C:\dir\")),
            r#"/select,"C:\dir""#
        );
        assert_eq!(
            windows_explorer_select_parameters(std::path::Path::new(r"C:\a b\c.txt")),
            r#"/select,"C:\a b\c.txt""#
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn linux_show_items_via_dbus_command_targets_file_manager1() {
        let cmd = linux_show_items_via_dbus_command(std::path::Path::new("/tmp/quadraui 1092"))
            .expect("an absolute path has a file:// URI to show");
        assert_eq!(cmd.get_program(), "dbus-send");
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            [
                "--session",
                "--dest=org.freedesktop.FileManager1",
                "--type=method_call",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
                "array:string:file:///tmp/quadraui%201092",
                "string:",
            ]
        );
    }

    /// A relative path has no `file://` URI, so there is no `ShowItems`
    /// invocation to build; the caller reads that as "this leg found
    /// nothing" and falls through to the `xdg-open`-the-parent leg
    /// rather than sending a one-element array holding the empty string.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn linux_show_items_via_dbus_command_is_none_for_a_relative_path() {
        assert!(linux_show_items_via_dbus_command(std::path::Path::new("a.txt")).is_none());
    }

    /// Acceptance bar: when neither the D-Bus leg nor the
    /// `xdg-open`-the-parent fallback can reach anything (`$PATH`
    /// redirected to an empty directory), `reveal_in_file_manager` honestly
    /// reports `Unsupported` rather than a false `Ok(())`. Doesn't exercise
    /// the D-Bus leg for real (that needs a live session bus this test
    /// environment may not have) — only pins the "both legs failed" outcome
    /// via the `$PATH` stub, which is the fallback this crate can actually
    /// control in a test.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn reveal_in_file_manager_reports_unsupported_when_every_leg_fails() {
        let _guard = PATH_OVERRIDE_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        struct RestorePath(Option<std::ffi::OsString>);
        impl Drop for RestorePath {
            fn drop(&mut self) {
                match self.0.take() {
                    Some(path) => std::env::set_var("PATH", path),
                    None => std::env::remove_var("PATH"),
                }
            }
        }
        let _restore = RestorePath(std::env::var_os("PATH"));
        let empty_dir = tempfile::tempdir().expect("tempdir");
        std::env::set_var("PATH", empty_dir.path());

        let target = std::env::temp_dir().join("quadraui-1092-no-such-file.txt");
        // `dbus-send` itself is very unlikely to be on an empty `$PATH`
        // (it's the binary this leg spawns), so this exercises the "not
        // even `dbus-send` present" corner of the D-Bus leg alongside the
        // `xdg-open`-the-parent fallback's own missing-opener leg.
        assert_eq!(
            reveal_in_file_manager(&target),
            Err(crate::backend::BackendError::Unsupported)
        );
    }
}

/// Escape `"` and `\` so a string can be embedded inside an AppleScript
/// double-quoted string literal. Shared by [`display_notification_script`]
/// below — `macos::services::MacPlatformServices::send_notification`'s
/// own private helper until issue #1092 gave `tui::services` a second
/// `osascript` call site (the TUI degrade's macOS notification fallback
/// leg) that needed byte-for-byte the same escaping.
#[cfg(all(target_os = "macos", any(feature = "macos", feature = "tui")))]
pub(crate) fn applescript_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Build the `osascript -e` argument an AppleScript `display
/// notification` call spawns. `display notification` needs an explicit
/// `sound name` clause to make *any* sound — omitting it entirely is
/// silent, not the system default. So `silent: false` (the default — a
/// caller that never calls `Notification::with_silent` gets a
/// notification that actually makes a sound, matching ordinary
/// desktop-notification expectations) adds `sound name ""`, which plays
/// the system default notification sound.
#[cfg(all(target_os = "macos", any(feature = "macos", feature = "tui")))]
pub(crate) fn display_notification_script(title: &str, body: &str, silent: bool) -> String {
    let mut script = format!(
        "display notification \"{body}\" with title \"{title}\"",
        body = applescript_escape(body),
        title = applescript_escape(title),
    );
    if !silent {
        script.push_str(" sound name \"\"");
    }
    script
}

/// Sniff the file extension that matches an encoded image's magic
/// bytes, for naming a temp copy a native API will re-decode from disk.
///
/// `UNNotificationAttachment` (macOS) picks its decoder from the
/// attached file's *extension*, not from its content, so an
/// [`crate::ImageSource::Bytes`] icon needs a plausible extension before it
/// will render. `None` means "not a format recognised here" — callers
/// skip the icon rather than guess an extension that would make the
/// platform decoder fail on a correctly-encoded image.
#[cfg(any(
    all(target_os = "macos", any(feature = "macos", feature = "tui")),
    test
))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn encoded_image_extension(bytes: &[u8]) -> Option<&'static str> {
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if bytes.starts_with(PNG) {
        return Some("png");
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return Some("jpg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("gif");
    }
    if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        return Some("tiff");
    }
    if bytes.starts_with(b"BM") {
        return Some("bmp");
    }
    if bytes.starts_with(b"RIFF") && bytes.len() >= 12 && &bytes[8..12] == b"WEBP" {
        return Some("webp");
    }
    // SVG has no magic number; the two openings a serialiser emits are
    // an XML prolog and a bare root element, either possibly preceded
    // by whitespace or a UTF-8 BOM.
    let text = bytes
        .strip_prefix(b"\xef\xbb\xbf".as_slice())
        .unwrap_or(bytes);
    let text = std::str::from_utf8(text.get(..text.len().min(512))?)
        .ok()?
        .trim_start();
    if text.starts_with("<?xml") || text.starts_with("<svg") {
        return Some("svg");
    }
    None
}

/// Copy a notification icon into `dir` as `<stem>.<ext>` and return the
/// path written, so a native notification API that can only attach a
/// *file* has one to point at.
///
/// Always copies, never hands over the caller's own file: macOS's
/// `UNNotificationAttachment` **moves** the attached file into the
/// system attachment data store, so attaching a consumer-owned path
/// directly would delete that file out from under them.
///
/// The extension comes from the source path when it has one (a
/// consumer naming `logo.png` knows the format better than any sniffer)
/// and otherwise from [`encoded_image_extension`]; an unrecognised
/// encoding is an [`std::io::ErrorKind::InvalidData`] error so the
/// caller can skip the icon instead of writing a file the platform
/// cannot decode. `dir` is created if missing.
#[cfg(any(
    all(target_os = "macos", any(feature = "macos", feature = "tui")),
    test
))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn materialize_notification_icon(
    source: &crate::primitives::image::ImageSource,
    dir: &std::path::Path,
    stem: &str,
) -> std::io::Result<std::path::PathBuf> {
    use crate::primitives::image::ImageSource;

    let (bytes, source_extension) = match source {
        ImageSource::Bytes(bytes) => (bytes.clone(), None),
        ImageSource::Path(path) => {
            let extension = path
                .extension()
                .and_then(|e| e.to_str())
                .filter(|e| !e.is_empty())
                .map(str::to_ascii_lowercase);
            (std::fs::read(path)?, extension)
        }
    };
    let extension = match source_extension {
        Some(extension) => extension,
        None => encoded_image_extension(&bytes)
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "notification icon is not a recognised image encoding",
                )
            })?
            .to_string(),
    };
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{stem}.{extension}"));
    std::fs::write(&path, &bytes)?;
    Ok(path)
}

#[cfg(test)]
mod notification_icon_tests {
    use super::{encoded_image_extension, materialize_notification_icon};
    use crate::primitives::image::ImageSource;

    const PNG_HEADER: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";

    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("quadraui-icon-tests-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn encoded_image_extension_recognises_common_encodings() {
        assert_eq!(encoded_image_extension(PNG_HEADER), Some("png"));
        assert_eq!(
            encoded_image_extension(b"\xff\xd8\xff\xe0JFIF"),
            Some("jpg")
        );
        assert_eq!(encoded_image_extension(b"GIF89a....."), Some("gif"));
        assert_eq!(encoded_image_extension(b"II*\0\x08\0\0\0"), Some("tiff"));
        assert_eq!(encoded_image_extension(b"MM\0*\0\0\0\x08"), Some("tiff"));
        assert_eq!(encoded_image_extension(b"BM\x8a\x02\0\0"), Some("bmp"));
        assert_eq!(
            encoded_image_extension(b"RIFF\x1a\0\0\0WEBPVP8 "),
            Some("webp")
        );
    }

    #[test]
    fn encoded_image_extension_recognises_svg_with_prolog_bom_or_bare_root() {
        assert_eq!(
            encoded_image_extension(b"<?xml version=\"1.0\"?><svg/>"),
            Some("svg")
        );
        assert_eq!(
            encoded_image_extension(b"  \n<svg width=\"8\"/>"),
            Some("svg")
        );
        assert_eq!(encoded_image_extension(b"\xef\xbb\xbf<svg/>"), Some("svg"));
    }

    #[test]
    fn encoded_image_extension_rejects_unrecognised_bytes() {
        assert_eq!(encoded_image_extension(b""), None);
        assert_eq!(encoded_image_extension(b"not an image at all"), None);
        // A truncated PNG signature is not a PNG.
        assert_eq!(encoded_image_extension(b"\x89PNG"), None);
    }

    #[test]
    fn materialize_bytes_icon_sniffs_extension_and_creates_the_directory() {
        let dir = scratch_dir("bytes");
        let written =
            materialize_notification_icon(&ImageSource::Bytes(PNG_HEADER.to_vec()), &dir, "icon-7")
                .expect("png bytes materialize");

        assert_eq!(written, dir.join("icon-7.png"));
        assert_eq!(std::fs::read(&written).unwrap(), PNG_HEADER);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn materialize_path_icon_copies_and_keeps_the_source_extension() {
        let dir = scratch_dir("path");
        std::fs::create_dir_all(&dir).unwrap();
        let original = dir.join("logo.PNG");
        std::fs::write(&original, PNG_HEADER).unwrap();

        let written =
            materialize_notification_icon(&ImageSource::Path(original.clone()), &dir, "icon-1")
                .expect("path icon materializes");

        assert_eq!(written, dir.join("icon-1.png"));
        // The consumer's own file is copied, never moved away.
        assert!(original.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn materialize_path_icon_without_extension_falls_back_to_sniffing() {
        let dir = scratch_dir("extensionless");
        std::fs::create_dir_all(&dir).unwrap();
        let original = dir.join("logo");
        std::fs::write(&original, PNG_HEADER).unwrap();

        let written = materialize_notification_icon(&ImageSource::Path(original), &dir, "icon-2")
            .expect("extensionless icon materializes");

        assert_eq!(written, dir.join("icon-2.png"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn materialize_unrecognised_bytes_reports_invalid_data_and_writes_nothing() {
        let dir = scratch_dir("unrecognised");
        let err = materialize_notification_icon(
            &ImageSource::Bytes(b"definitely not an image".to_vec()),
            &dir,
            "icon-3",
        )
        .expect_err("unrecognised encoding is rejected");

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(!dir.exists());
    }

    #[test]
    fn materialize_missing_path_reports_not_found() {
        let dir = scratch_dir("missing");
        let err =
            materialize_notification_icon(&ImageSource::Path(dir.join("nope.png")), &dir, "icon-4")
                .expect_err("a missing icon file is an error");

        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }
}

/// Build the `osascript -e <script>` command
/// `tui::services::TuiPlatformServices::send_notification`'s macOS
/// fallback leg spawns — not yet spawned, so a test can
/// assert on the constructed argv (in particular, that `title`/`body`
/// reach `osascript` via [`display_notification_script`] unmodified)
/// without a live display session. Split out for the same reason
/// [`macos_reveal_in_file_manager_command`] and
/// [`linux_show_items_via_dbus_command`] are: every other command this
/// issue builds is unit-testable this way, and the notification fallback
/// legs shouldn't be the one exception.
#[cfg(all(target_os = "macos", feature = "tui"))]
pub(crate) fn tui_macos_notify_command(
    title: &str,
    body: &str,
    silent: bool,
) -> std::process::Command {
    let script = display_notification_script(title, body, silent);
    let mut cmd = std::process::Command::new("osascript");
    cmd.arg("-e").arg(script);
    cmd
}

/// Build the `notify-send <title> <body>` command
/// `tui::services::TuiPlatformServices::send_notification`'s Linux/BSD
/// fallback leg spawns — not yet spawned, so a test can
/// assert `title` and `body` arrive as two separate argv entries (not
/// shell-concatenated, so a body containing spaces can't be mistaken for
/// extra `notify-send` flags) without a live notification-daemon
/// session.
#[cfg(all(unix, not(target_os = "macos"), feature = "tui"))]
pub(crate) fn tui_notify_send_command(title: &str, body: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new("notify-send");
    cmd.arg(title).arg(body);
    cmd
}

/// `unix`-gated as a whole, not per-test: both command builders this
/// module covers ([`tui_macos_notify_command`],
/// [`tui_notify_send_command`]) are themselves `cfg(unix)` — Windows has
/// no native command-line notifier leg at all (OSC 9 is natively
/// supported by Windows Terminal, so `tui::services`'s escape leg covers
/// it alone; see that module's "Notifications" doc). A
/// `#[cfg(all(test, feature = "tui"))]` module with only `cfg(unix)` and
/// `cfg(target_os = "macos")` tests inside compiles to an *empty* module
/// on Windows, whose `use super::*` is then an `unused_imports` warning —
/// and `ci.yml`'s windows-latest "Test (tui feature)" step runs under
/// `RUSTFLAGS: -D warnings`, so that warning is a hard build failure
/// there (and only there: the "Build (tui feature)" step above it sets no
/// such flag, which is why this surfaced as a test-step-only red).
/// Windows-side coverage of `send_notification` lives in
/// `tui::services`'s `notification_osc_tests` instead, which is exactly
/// the whole method on that platform.
#[cfg(all(test, unix, feature = "tui"))]
mod tui_send_notification_command_tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn tui_macos_notify_command_uses_osascript_with_the_display_notification_script() {
        let cmd = tui_macos_notify_command("Build failed", "3 errors", false);
        assert_eq!(cmd.get_program(), "osascript");
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args[0], "-e");
        assert_eq!(
            args[1],
            display_notification_script("Build failed", "3 errors", false).as_str()
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn tui_notify_send_command_passes_title_and_body_as_separate_args() {
        let cmd = tui_notify_send_command("Build failed", "3 errors");
        assert_eq!(cmd.get_program(), "notify-send");
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            ["Build failed", "3 errors"]
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn tui_notify_send_command_keeps_body_out_of_argv0() {
        // A body containing something that looks like a flag must stay
        // its own argv entry, never get shell-concatenated with the
        // title into something `notify-send` could misparse.
        let cmd = tui_notify_send_command("t", "--urgency=critical");
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            ["t", "--urgency=critical"]
        );
    }
}

#[cfg(all(test, target_os = "macos", any(feature = "macos", feature = "tui")))]
mod display_notification_script_tests {
    use super::*;

    #[test]
    fn applescript_escape_handles_quotes_and_backslashes() {
        // Empty + pass-through.
        assert_eq!(applescript_escape(""), "");
        assert_eq!(applescript_escape("plain text"), "plain text");
        // Single-character escapes.
        assert_eq!(applescript_escape("a\"b"), "a\\\"b");
        assert_eq!(applescript_escape("c\\d"), "c\\\\d");
        // Combined. Backslash MUST be escaped first so the subsequent
        // quote-escape's added backslashes aren't re-escaped.
        assert_eq!(applescript_escape("e\"f\\g"), "e\\\"f\\\\g");
        // Order check: a backslash followed by a quote in input should
        // produce `\\\"` (escaped slash + escaped quote), not `\\\\\"`
        // (double-escaped slash + quote).
        assert_eq!(applescript_escape("\\\""), "\\\\\\\"");
    }

    #[test]
    fn display_notification_script_not_silent_adds_default_sound_clause() {
        let script = display_notification_script("t", "b", false);
        assert_eq!(
            script,
            "display notification \"b\" with title \"t\" sound name \"\""
        );
    }

    #[test]
    fn display_notification_script_silent_omits_sound_clause() {
        let script = display_notification_script("t", "b", true);
        assert_eq!(script, "display notification \"b\" with title \"t\"");
        assert!(!script.contains("sound"));
    }

    #[test]
    fn display_notification_script_escapes_title_and_body() {
        let script = display_notification_script("t\"itle", "b\\ody", true);
        assert_eq!(
            script,
            "display notification \"b\\\\ody\" with title \"t\\\"itle\""
        );
    }
}

// ─────────────────────────────────────────────────────────────────────
// map_arboard_error
// ─────────────────────────────────────────────────────────────────────

/// Map an `arboard::Error` from a named native call into a
/// [`crate::backend::BackendError::PlatformFailure`]. `arboard::Error`
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

/// Headless smoke-mode config. `None` unless the `ms`-suffixed env var
/// [`Self::from_env`] is asked to read is set — see `gtk::run`'s module
/// doc's "Headless smoke mode" section for the full motivating rationale
/// (a live-window class of bug a display-free driver test structurally
/// can't catch).
///
/// `#[cfg(any(feature = "gtk", all(feature = "win", any(target_os =
/// "windows", test))))]` — same reasoning as [`ModalPumpDepth`]'s gate
/// note: `win::run`'s real smoke lane only exists once
/// `target_os = "windows"` also compiles `mod win32`, but the `test`
/// alternative keeps this type (and its pure predicates below) a real,
/// unit-testable consumer under a plain `cargo test --features win` on
/// any host.
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
/// allocation? The direct regression check for a tiny/wrapped-window bug
/// class. Pure and display-free. Each backend supplies its own floor
/// (`min_width`/`min_height`) — its own default window size and how far
/// below it is still "plausible" — this function only owns the comparison.
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
/// platform's OS-level modifier key" — but that shared abstraction means
/// `modifiers.cmd` can be held on *any* backend (Super on Linux, the
/// Windows key on Win32), not only on macOS. [`is_paste_keypress`] takes
/// this enum so each backend tells the predicate which one of `ctrl`/`cmd`
/// is *its* paste modifier, instead of the predicate accepting either
/// interchangeably — see D-011 in `docs/decisions/DECISIONS.md` for the
/// regression this fixes (plain Super+V used to trigger paste on GTK, and
/// plain Ctrl+V used to trigger paste on macOS, neither of which is that
/// platform's actual convention).
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
/// One predicate for every desktop backend; `native` names which
/// platform modifier the *calling* backend treats as "paste" — GTK and
/// `win` both pass [`PasteModifier::Ctrl`], macOS passes
/// [`PasteModifier::Cmd`]. Only that modifier is accepted: the *other*
/// one must be explicitly absent, not merely ignored, so e.g. plain
/// Super+V (GTK, where `modifiers.cmd` is Super) and plain Ctrl+V
/// (macOS) are both rejected exactly as they were before this predicate
/// was lifted out of `gtk::run`/inlined in `macos::run` — see D-011 §4
/// in `docs/decisions/DECISIONS.md` for why "either modifier alone" was wrong.
/// `alt` is never a valid paste chord on any backend.
///
/// `shift` is deliberately **not** checked either way — Ctrl-Shift-V and
/// (by the same reasoning, extended here) Cmd-Shift-V both trigger paste
/// too. This extends the shift-tolerant contract to every adopter —
/// including macOS, which used to require `shift: false` in its inline
/// Cmd-V match guard before this predicate replaced it. See D-011 in
/// `docs/decisions/DECISIONS.md` for why this settles the contract once
/// instead of letting each new adopter silently pick its own.
///
/// Case-insensitive on the letter (`'v'` or `'V'`) on every backend.
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
/// [`crate::backend::PointerShape::Default`], one
/// [`crate::backend::PointerShape::Resize`] per [`ALL_RESIZE_EDGES`] edge
/// (8), and one of each remaining non-exhaustive variant (`Text`,
/// `Pointer`, `Grab`, `Grabbing`, `Crosshair`, `Wait`, `NotAllowed`,
/// `ColResize`, `RowResize`) — 18 total. Each backend still owns its own
/// `PointerShape` → native cursor table (cursor names/objects are
/// inherently native — GTK's are CSS cursor-name strings, AppKit's are
/// `NSCursor` objects, and AppKit in particular has no public diagonal-
/// resize cursor, so backends may legitimately map more than one
/// `PointerShape` to the same native cursor) — this is the enum-walk
/// *scaffold* that was missing: a canonical list backends' own mapping
/// tests iterate against instead of hand-duplicating the variant list
/// per backend.
#[cfg(all(
    test,
    any(
        feature = "gtk",
        all(feature = "macos", target_os = "macos"),
        feature = "win"
    )
))]
pub(crate) fn all_pointer_shapes() -> [crate::backend::PointerShape; 18] {
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
        PointerShape::Text,
        PointerShape::Pointer,
        PointerShape::Grab,
        PointerShape::Grabbing,
        PointerShape::Crosshair,
        PointerShape::Wait,
        PointerShape::NotAllowed,
        PointerShape::ColResize,
        PointerShape::RowResize,
    ]
}

// ─────────────────────────────────────────────────────────────────────
// Caught-panic reporting dedup
// ─────────────────────────────────────────────────────────────────────
//
// `macos::run`'s `drawRect:`/responder-method bodies and `win::run`'s
// `wndproc` both hand a Rust callback directly to a C ABI that cannot
// unwind — an uncaught panic there escalates straight to `abort()`
// instead of staying a recoverable panic. Both bracket every such body —
// or, for `win::run`, the shared `dispatch_event`/`render_frame` helpers
// every message ultimately funnels through — in
// `std::panic::catch_unwind(AssertUnwindSafe(..))` and report a caught
// panic through [`report_caught_panic_once`] rather than calling
// `crate::diagnostics::emit` directly, so a rasteriser gap re-entered on
// every redraw is reported once per process, not once per frame.
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
    // here). Shared by both the GTK and `win` adopters — the env
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
        // Content wrapped into an ~8px-wide column.
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
    //! Coverage for [`is_paste_keypress`] — the single predicate
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
        // Some terminal emulators reserve plain Ctrl-V for
        // a control byte and use Ctrl-Shift-V for paste instead.
        assert!(is_paste_keypress(
            &Key::Char('v'),
            &mods(true, true, false, false),
            PasteModifier::Ctrl
        ));
    }

    #[test]
    fn cmd_shift_v_is_a_paste_keypress_for_cmd_native_backends() {
        // D-011: shift tolerance: Cmd-Shift-V also triggers paste on macOS.
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
    fn all_pointer_shapes_is_default_plus_one_resize_per_edge_plus_rest() {
        const REST: [PointerShape; 9] = [
            PointerShape::Text,
            PointerShape::Pointer,
            PointerShape::Grab,
            PointerShape::Grabbing,
            PointerShape::Crosshair,
            PointerShape::Wait,
            PointerShape::NotAllowed,
            PointerShape::ColResize,
            PointerShape::RowResize,
        ];
        let shapes = all_pointer_shapes();
        assert_eq!(shapes.len(), ALL_RESIZE_EDGES.len() + 1 + REST.len());
        assert_eq!(shapes[0], PointerShape::Default);
        for (shape, edge) in shapes[1..9].iter().zip(ALL_RESIZE_EDGES.iter()) {
            assert_eq!(*shape, PointerShape::Resize(*edge));
        }
        assert_eq!(&shapes[9..], &REST);
    }
}
