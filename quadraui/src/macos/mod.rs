//! macOS backend: AppKit windows, Core Graphics painting and Core Text.
//!
//! Enabled via the `macos` Cargo feature on a macOS host. A `run` entry
//! point owns window and run-loop bootstrap; every widget paints through
//! its widget contract (`Backend::draw_dyn`) on `MacBackend`'s
//! `PaintSurface` over the `CGContextRef` the runner sets up inside the
//! view's `drawRect:` override. This module carries only the platform
//! plumbing (window, events, text, services, tray, the test driver).
//!
//! `backend`/`events`/`services`/`shell_runner`/`text` and the re-exports
//! with doc comments on this page (`MacBackend`, `run`, …) are the stable,
//! consumer-depended-on subset — `vimcode` and `coord-tui` reach the first
//! five by module path (`crate::macos::shell_runner::run_with_shell`,
//! `crate::macos::text::system_monospace_font`, …). Everything else is
//! `#[doc(hidden)]`: **unstable, not covered by semver**.
pub mod backend;
pub(crate) mod caret_blink;
// Shared CoreGraphics FFI bindings + fill/stroke/rect helpers.
mod cg;
pub mod events;
#[doc(hidden)]
pub mod headless;
pub(crate) mod menu_bar_install;
mod run;
pub mod services;
pub mod shell_runner;
// Shared `PaintSurface` adapter over a bare `CGContextRef`.
mod surface;
#[doc(hidden)]
pub mod testing;
pub mod text;
// Text-selection highlight painting — the CoreGraphics twin of
// `gtk::backend`'s Cairo `fill()` / `win::backend`'s Direct2D
// `FillRectangle` call. `pub(crate)`, not `pub`: only `MacBackend::
// apply_selection_highlight` calls into it, mirroring `menu_bar_install`'s
// visibility above.
pub(crate) mod text_selection;
// Tray / status-bar icon control — `NSStatusBar`/`NSStatusItem`
// glue for `Backend::tray`. `pub(crate)`, not `pub`: only
// `impl TrayService for MacBackend` (`backend.rs`) calls into it,
// mirroring `menu_bar_install`'s visibility above.
pub(crate) mod tray;

// The flat re-exports below a real caller reaches by this exact path stay
// documented; the rest are `#[doc(hidden)]` — unstable, not covered by
// semver (see `Cargo.toml`'s `[package.metadata.cargo-semver-checks.lints]`).
pub use backend::MacBackend;
#[doc(hidden)]
pub use run::run_with;
pub use run::{run, MacRunner, RunConfig, StepOutcome};
#[doc(hidden)]
pub use services::MacPlatformServices;

/// Serialises every `#[test]` fn in this module tree that drives the
/// **one real systemwide `NSPasteboard`**.
///
/// `cargo test` runs `#[test]` fns on a pool of spawned threads, so any
/// two tests that write to the OS clipboard race each other.
///
/// The invariant: **anything that reads or writes the real pasteboard
/// takes this lock first**, for its whole read/write sequence, and holds
/// it until the assertions are done. Poisoning is recovered from rather
/// than propagated — one already-failing clipboard test should report its
/// own failure, not convert every other clipboard test into a second red
/// line.
#[cfg(test)]
static REAL_PASTEBOARD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Acquire [`REAL_PASTEBOARD_LOCK`] — see that static's doc for the
/// invariant this enforces. Bind the guard to a named local (`let
/// _pasteboard = …`), never to `_`, or it is dropped immediately and
/// the lock buys nothing.
#[cfg(test)]
pub(crate) fn lock_real_pasteboard() -> std::sync::MutexGuard<'static, ()> {
    REAL_PASTEBOARD_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
