//! Public macOS (AppKit + Core Graphics + Core Text) rasterisers for
//! `quadraui` primitives.
//!
//! Enabled via the `macos` Cargo feature on a macOS host. Apps depend
//! on `quadraui` with `features = ["macos"]` and call into this module
//! to open an AppKit window and paint primitives onto a `CGContextRef`
//! that the runner sets up inside the view's `drawRect:` override.
//!
//! Mirrors the layout of [`crate::gtk`] and [`crate::tui`]: a `run`
//! entry point owns window + run-loop bootstrap, and per-primitive
//! rasterisers live as sibling modules.
//!
//! The per-primitive rasteriser submodules, and most of the free functions
//! re-exported at the bottom of this module, are painter internals, not a
//! documented API: **unstable, not covered by semver** — they can move,
//! split, or disappear between any two 0.1.x releases with no version
//! bump. `backend`/`events`/`services`/`shell_runner`/`testing`/`text` and
//! the handful of re-exports with doc comments on this page (`MacBackend`,
//! `run`, …) are the stable, consumer-depended-on subset; everything else
//! is `#[doc(hidden)]`.

// Per-primitive rasteriser modules below stay `pub mod` (not `pub(crate)`)
// so every existing `crate::macos::<module>::<item>` path keeps compiling —
// narrowing to `pub(crate)` is a breaking visibility change this crate's
// pre-1.0 semver convention (and `cargo-semver-checks`) treats like any
// other removal. Each one is `#[doc(hidden)]` instead: unstable, not
// covered by semver, and excluded from rustdoc's module index — a painter
// internal splitting (or its functions moving between sibling modules) is
// free to happen without a version bump. The handful of items a consumer
// actually needs stay documented at their flat `crate::macos::<item>`
// re-export path in the `pub use` block below, which is unaffected by the
// `#[doc(hidden)]` on its source module. `Cargo.toml`'s
// `[package.metadata.cargo-semver-checks.lints]` downgrades the
// `*_now_doc_hidden` lint family to `allow` so marking one of these hidden
// for the first time doesn't itself register as a breaking change.
//
// `backend`/`events`/`services`/`shell_runner`/`testing`/`text` are the
// exception: those six are the real, module-qualified stable surface
// (`crate::macos::testing::MacDriver`, `crate::macos::shell_runner::
// run_with_shell`, `crate::macos::text::system_monospace_font`, …) that
// `vimcode` and `coord-tui` already depend on by module path, not just
// through a flat re-export — mirrors the set `crate::gtk`/`crate::tui`/
// `crate::win` already keep `pub` (undecorated) for the same reason.
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

// The flat re-exports below split into two groups, verified by grepping
// `~/src/vimcode/src`, `~/src/coord-tui/src`, and every in-tree consumer
// (`quadraui/examples`, `quadraui/benches`, `quadraui/tests`, `kubeui*`) for
// `quadraui::macos::<name>` / `crate::macos::<name>`: the handful a real
// caller reaches by this exact flat path stay documented; everything else
// is a painter internal duplicating what `Backend::draw_*` already
// exposes, with zero reachers, so it is `#[doc(hidden)]` — unstable, not
// covered by semver, free to move or disappear without a version bump
// (see `Cargo.toml`'s `[package.metadata.cargo-semver-checks.lints]`).
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
