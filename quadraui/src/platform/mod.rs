//! Shared, backend-neutral desktop-interaction plumbing.
//!
//! Everything in this module is genuinely windowing-generic: it has no
//! dependency on `gtk4`, `objc2`, `windows`, or any other toolkit crate,
//! and none of the *code* here needs a toolkit feature to compile — the
//! whole point of extracting it is that a brand-new backend gets it for
//! free. Each piece is implemented once here rather than per-backend; a
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
//!   generalised to also cover macOS's Cmd-V instead of an inline
//!   `match` guard in `macos::run::dispatch_event`, so it stays a named,
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
//!
//! ## Layout
//!
//! One submodule per OS-service concern, grouping definitions with their
//! own tests: [`window_drag`] (CSD drag-to-move/resize arming),
//! [`modal_pump`] (nested native modal-loop re-entrancy guard),
//! [`trash`] (move/restore via the `trash` crate), [`open_with`] (the
//! cross-platform "open with the default handler" opener, plus the
//! shared UTF-16 wide-string framing Win32 callers need),
//! [`open_url`] (the scheme/extension allowlist every opener validates
//! a URL against first), [`reveal`] (reveal-in-file-manager and the
//! `file://` URI builder it and [`crate::drag`] share), [`notifications`]
//! (AppleScript `display notification` escaping/building and the
//! notification-icon materializer), [`clipboard`] (`arboard::Error`
//! mapping), [`smoke`] (headless smoke-mode config/predicates),
//! [`paste`] (the clipboard-paste keypress predicate), [`pointer_shape`]
//! (the `ResizeEdge`/`PointerShape` enum-walk scaffold), and
//! [`panic_report`] (caught-panic reporting dedup for the two backends
//! that hand Rust callbacks to a non-unwinding C ABI).
//!
//! [`crate::desktop`] re-exports every `pub(crate)` item here at its
//! historical `crate::desktop::X` path, so every existing call site
//! across `gtk`/`macos`/`win`/`tui` keeps compiling unchanged — this
//! split is a pure move, not a rename of the public (crate-internal)
//! surface.

pub(crate) mod clipboard;
pub(crate) mod modal_pump;
pub(crate) mod notifications;
pub(crate) mod open_url;
pub(crate) mod open_with;
pub(crate) mod panic_report;
pub(crate) mod paste;
pub(crate) mod pointer_shape;
pub(crate) mod reveal;
pub(crate) mod smoke;
pub(crate) mod trash;
pub(crate) mod window_drag;

// Each submodule gates its own items on the feature(s)/`target_os` of the
// backend(s) that call them (see each submodule's own doc) — a feature
// combination with none of those active (e.g. a bare `--features tui`
// build, where `clipboard`/`modal_pump`/`panic_report`/`pointer_shape`/
// `smoke`/`window_drag` have no live item at all) makes the matching glob
// below import nothing, which is by design, not a leftover.
#[allow(unused_imports)]
pub(crate) use clipboard::*;
#[allow(unused_imports)]
pub(crate) use modal_pump::*;
#[allow(unused_imports)]
pub(crate) use notifications::*;
#[allow(unused_imports)]
pub(crate) use open_url::*;
#[allow(unused_imports)]
pub(crate) use open_with::*;
#[allow(unused_imports)]
pub(crate) use panic_report::*;
#[allow(unused_imports)]
pub(crate) use paste::*;
#[allow(unused_imports)]
pub(crate) use pointer_shape::*;
#[allow(unused_imports)]
pub(crate) use reveal::*;
#[allow(unused_imports)]
pub(crate) use smoke::*;
#[allow(unused_imports)]
pub(crate) use trash::*;
#[allow(unused_imports)]
pub(crate) use window_drag::*;
