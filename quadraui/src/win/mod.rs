//! Win-GUI backend: Direct2D + DirectWrite rasterisers.
//!
//! The Win32 window + Direct2D render-target bootstrap
//! (`run.rs`'s message loop, `backend.rs`'s `begin_frame`/`end_frame`)
//! uses real WinAPI calls gated on `cfg(target_os = "windows")` so `cargo
//! check --features win` still type-checks `WinBackend`'s trait
//! completeness on Linux (see `backend.rs`'s module docs and `Cargo.toml`'s
//! `win` feature comment). Chrome strips (menu, status and
//! activity bars) paint through their widget contracts; content-area
//! rasterisers include `list`/`form`/`data_table`/`editor`/`chart`;
//! overlay/popup rasterisers include `tooltip`/`dialog`/
//! `palette`/`completions`/`rich_text_popup`; container/
//! indicator rasterisers include `split`/`toast`/`progress`/
//! `spinner`; text-heavy rasterisers include `text_display`/
//! `message_list` — every other `draw_*`/`*_layout` rasteriser is still a
//! `todo!()` stub; implement each one against Direct2D / DirectWrite and
//! the compiler will tell you when you're done.
//!
//! See `quadraui/docs/NATIVE_GUI_LESSONS.md` for pitfalls discovered
//! during Win-GUI work. See the GTK backend (`quadraui/src/gtk/`)
//! as the reference implementation for a pixel-based backend.
//!
//! Most of the free functions re-exported at the bottom of this module are
//! painter internals, not a documented API: **unstable, not covered by
//! semver** — they can move, split, or disappear between any two 0.1.x
//! releases with no version bump. The few re-exports with doc comments on
//! this page (`WinBackend`, `run`, …) are the stable, consumer-depended-on
//! subset; everything else is `#[doc(hidden)]`.

pub mod backend;
/// Win32 message → `quadraui::UiEvent` translation (mouse, keyboard,
/// focus). Pure free functions, host-independent and unit-tested off
/// Windows — mirrors `crate::gtk::events` / `crate::macos::events`. See
/// its module docs. `WM_SIZE`/`WM_DPICHANGED`/`WM_CLOSE` translation
/// is handled in `msg` + `run` — see this module's docs.
pub mod events;
/// Win32 message-payload decoding (`WPARAM`/`LPARAM` word unpacking, DPI
/// ratio). Crate-private and host-independent — it is the one part of this
/// backend that is pure arithmetic, so it is the one part that can be
/// unit-tested off Windows. See its module docs.
pub(crate) mod msg;
// `gtk`/`tui`/`macos` keep this submodule crate-private (`mod run;`) and
// surface only the flat re-exports below; `win` is the one backend where
// it is `pub mod` instead. No caller in `~/src/vimcode/src`,
// `~/src/coord-tui/src`, or any in-tree consumer reaches it module-qualified
// (`crate::win::run::…`) — every real caller goes through the flat
// `crate::win::{run, run_with, RunConfig, StepOutcome, WinRunner}` paths
// below instead — so `#[doc(hidden)]` brings this module's own tracked
// surface to parity with the other three backends.
#[doc(hidden)]
pub mod run;
pub mod services;
/// `run_with_shell()`: composes a [`crate::ShellApp`] into a `ShellView`
/// and drives it through the Win32 message loop — the Windows
/// analogue of `gtk::shell_runner` / `tui::shell_runner` /
/// `macos::shell_runner`. `pub` (not target-gated) for the same
/// "compiles everywhere, only *works* on Windows" reason as `run`/`mod
/// run` above — see this module's own doc.
pub mod shell_runner;
/// Shared `PaintSurface` adapter over a bare `&ID2D1RenderTarget`.
/// Windows-only — same gating as the per-primitive
/// rasteriser modules above that consume it.
#[cfg(target_os = "windows")]
mod surface;
/// Headless Direct2D test surface: an offscreen `ID2D1DCRenderTarget`
/// backed by an in-memory DIB section, so `#[cfg(test)]` blocks can paint
/// a primitive and read pixels back with no `HWND`, display, or GPU.
/// `pub` (like [`crate::tui::testing`] / [`crate::gtk::testing`]) rather
/// than `pub(crate)` — see its module docs for why.
///
/// `feature = "win"` alone — **not** `target_os = "windows"`
/// — mirrors `backend`/`run`/`shell_runner` above: every real Direct2D/GDI
/// call inside is individually `cfg(target_os = "windows")`-gated with a
/// non-functional fallback everywhere else, so `WinDriver`/
/// `driver_with_shell`/`ConformanceDriver for WinDriver` type-check on an
/// ordinary Linux host — see this module's own doc for why that matters
/// to downstream adoption of `quadraui::testing::ConformanceDriver`.
pub mod testing;
/// DirectWrite text infrastructure — factory + text-format creation,
/// font-metrics measurement, and Direct2D text painting. Windows-only
/// in full: nothing in here has a meaningful non-Windows fallback (unlike
/// `msg`, this isn't pure arithmetic), so the module itself only exists on
/// `target_os = "windows"` rather than being internally `cfg`-gated
/// line-by-line like `backend.rs`.
///
/// `pub` rather than `pub(crate)`: the chrome rasterisers re-exported
/// below take a `&DWrite` measurer in
/// their signatures, exactly as [`crate::macos`]'s take a `&CTFont` from its
/// own `pub mod text`. A `pub` function whose parameter type is `pub(crate)`
/// is a `private_interfaces` warning — and this repo's CI runs with
/// `RUSTFLAGS: "-D warnings"`, so it is a *build failure* on the
/// windows-latest leg (nothing in this module compiles on Linux, so the
/// ubuntu `cargo check --features win` leg cannot see it). Keep this module
/// and `DWrite`'s public surface in step with the rasterisers that take it.
#[cfg(target_os = "windows")]
pub mod text;
/// Tray/status-bar icon control (`Shell_NotifyIconW`) for `Backend::tray`.
/// `pub(crate)` (not target-gated) for the same "compiles
/// everywhere via internal `cfg(target_os = "windows")`, only *works* on
/// Windows" reason as `backend`/`services`/`run` above — see this
/// module's own doc and `tray`'s module doc.
pub(crate) mod tray;

// The flat re-exports below split into two groups, verified by grepping
// `~/src/vimcode/src`, `~/src/coord-tui/src`, and every in-tree consumer
// (`quadraui/examples`, `quadraui/benches`, `quadraui/tests`, `kubeui*`) for
// `quadraui::win::<name>` / `crate::win::<name>`: the handful a real caller
// reaches by this exact flat path stay documented; everything else is a
// painter internal duplicating what `Backend::draw_*` already exposes, with
// zero reachers, so it is `#[doc(hidden)]` — unstable, not covered by
// semver, free to move or disappear without a version bump (see
// `Cargo.toml`'s `[package.metadata.cargo-semver-checks.lints]`).
pub use backend::WinBackend;
pub use run::{run, run_with, RunConfig, StepOutcome};
// `WinRunner` needs a real `HWND`/`WindowState<A>` (built inside
// `run::win32`), unlike `RunConfig`/`run`/`run_with`/`StepOutcome` above,
// which all compile on every host — so, mirroring every other
// Windows-only rasteriser re-export in this file, this one is gated too.
#[cfg(target_os = "windows")]
pub use run::WinRunner;
#[doc(hidden)]
pub use services::WinPlatformServices;

/// Serialises every `#[test]` fn in this module tree that drives the
/// **one real systemwide Windows clipboard**.
///
/// `cargo test` runs `#[test]` fns on a pool of spawned threads, so any
/// two tests that write to the OS clipboard race each other — and the
/// loser reads back whatever the winner just wrote. On Windows the
/// clipboard is a single global owned by whichever thread most recently
/// called `OpenClipboard`/`SetClipboardData`, so there is no per-test
/// isolation to fall back on.
///
/// Three tests in [`run`] touch it, all only on `target_os = "windows"`
/// (off Windows `WinClipboard` is an unconditional no-op stub, so they
/// can't race):
///
/// - `paste_dispatch_tests::ctrl_v_delivers_real_clipboard_text_as_clipboard_paste`
///   seeds a distinctive payload, reads it back to confirm the host's
///   clipboard is usable at all, then asserts Ctrl-V delivers *that*
///   payload — a three-step sequence that is only sound if nothing else
///   writes in between.
/// - `text_selection_dispatch_tests::drag_select_then_ctrl_c_copies_the_selection`
///   presses Ctrl-C over a selection, which copies `"hello world"` to
///   that same real clipboard.
/// - `paste_dispatch_tests::ctrl_v_is_never_forwarded_to_the_app_as_a_raw_keypress`
///   reads it (its assertion tolerates any contents, but it holds the
///   clipboard open while doing so, which is enough to make a concurrent
///   `OpenClipboard` fail).
///
/// This enforces the invariant that any sequence of clipboard
/// operations must complete atomically without interleaving from other
/// tests.
///
/// **Anything that reads or writes the real clipboard takes this lock
/// first**, for its whole read/write sequence, and holds it until the
/// assertions are done. Poisoning is recovered from rather than
/// propagated — one already-failing clipboard test should report its own
/// failure, not convert every other clipboard test into a second red
/// line.
#[cfg(test)]
static REAL_CLIPBOARD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Acquire [`REAL_CLIPBOARD_LOCK`] — see that static's doc for the
/// invariant this enforces. Bind the guard to a named local (`let
/// _clipboard = …`), never to `_`, or it is dropped immediately and the
/// lock buys nothing.
#[cfg(test)]
pub(crate) fn lock_real_clipboard() -> std::sync::MutexGuard<'static, ()> {
    REAL_CLIPBOARD_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
