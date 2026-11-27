//! Proof that the macOS runner's public entry points report failure as a
//! [`quadraui::RunError`] a host's `fn main` can `?`-propagate.
//!
//! This file is a separate compilation unit, so the signatures coerced
//! below are the real public API, not an in-crate convenience.
//!
//! All three entry points ([`quadraui::macos::run`],
//! [`quadraui::macos::run_with`],
//! [`quadraui::macos::shell_runner::run_with_shell`]) own the process for
//! their whole lifetime — they build an `NSApplication` + `NSWindow` and
//! block on `NSApplication::run()` until the last window closes — so a
//! test harness cannot call one and come back. What is checkable on any
//! host is the contract itself: each entry point returns `Result<(),
//! RunError>`, and that type drops straight into a `-> Result<(),
//! RunError>` host `main` via `?`. Unlike the TUI/GTK siblings, this
//! backend has no headless self-check mode and
//! `NSApplication::run()` itself reports no exit status, so `run_with`
//! always resolves `Ok(())` today — see that function's doc. The live
//! AppKit path stays covered by `macos.yml`'s `macos-latest` runner.
#![cfg(all(feature = "macos", target_os = "macos"))]

use quadraui::macos::RunConfig;
use quadraui::prelude::*;
use quadraui::{RunError, ShellLayout};

/// Smallest possible [`AppLogic`]: paints nothing and quits on the first
/// event. Never actually driven here — it exists to give the entry-point
/// signatures a concrete type argument.
struct NoopApp;

impl AppLogic for NoopApp {
    type AreaId = ();

    fn render(&self, _backend: &mut dyn Backend, _area: ()) {}

    fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Exit
    }
}

/// [`NoopApp`]'s equivalent on the [`ShellApp`] path, for
/// [`quadraui::macos::shell_runner::run_with_shell`]'s signature.
struct NoopShell;

impl ShellApp for NoopShell {
    fn render_content(&self, _backend: &mut dyn Backend, _layout: &ShellLayout) {}

    fn handle(
        &mut self,
        _event: UiEvent,
        _backend: &mut dyn Backend,
        _ctx: &ShellContext,
    ) -> Reaction {
        Reaction::Exit
    }
}

/// The shape of a consumer's entry point: hand it any of the runner's
/// entry points and it propagates the outcome with `?`, exactly as a
/// `fn main() -> Result<(), quadraui::RunError>` does.
fn host_main(run: fn(NoopApp) -> Result<(), RunError>) -> Result<(), RunError> {
    run(NoopApp)?;
    Ok(())
}

/// Stand-in for a runner the user closed the window out of cleanly.
fn exited_gracefully(_app: NoopApp) -> Result<(), RunError> {
    Ok(())
}

/// Every entry point hands back `Result<(), RunError>`. Each coercion
/// below fails to compile if the return type drifts — including back to
/// `std::process::ExitCode`, or to `()` for `run_with_shell`, which would
/// make a bootstrap failure invisible to its caller.
#[test]
fn every_macos_entry_point_returns_a_run_error_result() {
    // Nothing is invoked — each of these would need the main thread and
    // would create an `NSApplication` + `NSWindow`, blocking until its
    // last window closed. The coercion is the assertion: it only
    // type-checks while the entry point's return type is exactly
    // `Result<(), RunError>`. `macos::run`'s own coercion is additionally
    // the proof that `host_main` below accepts it unchanged.
    let _run: fn(NoopApp) -> Result<(), RunError> = quadraui::macos::run;
    let _run_with: fn(NoopApp, RunConfig) -> Result<(), RunError> = quadraui::macos::run_with;
    let _run_with_shell: fn(NoopShell, ShellConfig) -> Result<(), RunError> =
        quadraui::macos::shell_runner::run_with_shell;

    assert!(
        host_main(exited_gracefully).is_ok(),
        "a closed window must reach the host's `fn main` as Ok(()), not an error"
    );
}
