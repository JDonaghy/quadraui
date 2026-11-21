//! Proof that the Win-GUI runner's public entry points report failure as a
//! [`quadraui::RunError`] a host's `fn main` can `?`-propagate.
//!
//! This file is a separate compilation unit, so the signatures coerced
//! below are the real public API, not an in-crate convenience.
//!
//! All three entry points ([`quadraui::win::run`], [`quadraui::win::run_with`],
//! [`quadraui::win::shell_runner::run_with_shell`]) own the process for
//! their whole lifetime on a real Windows host — they create a Win32
//! window and block on the message loop until the last window closes — so
//! a test harness cannot call one and come back. What is checkable on any
//! host is the contract itself: each entry point returns `Result<(),
//! RunError>`, that type drops straight into a `-> Result<(), RunError>`
//! host `main` via `?`, and a failed native call (or a headless
//! `QUADRAUI_WIN_SMOKE_MS` self-check failure) arrives as a `RunError`
//! variant instead of being flattened to a bare exit code. The live
//! message-loop path stays covered by `tools/win-test.sh` on a real
//! Windows host.
#![cfg(feature = "win")]

use quadraui::prelude::*;
use quadraui::win::RunConfig;
use quadraui::{AppShellLayout, BackendError, RunError};

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
/// [`quadraui::win::shell_runner::run_with_shell`]'s signature.
struct NoopShell;

impl ShellApp for NoopShell {
    fn render_content(&self, _backend: &mut dyn Backend, _layout: &AppShellLayout) {}

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

/// Stand-in for a runner whose Win32 message loop hit a native call
/// failure (or a headless `QUADRAUI_WIN_SMOKE_MS` self-check failure), so
/// the propagation through [`host_main`] can be exercised without a
/// window.
fn run_inner_failed(_app: NoopApp) -> Result<(), RunError> {
    Err(RunError::Backend(BackendError::PlatformFailure {
        context: "win::run: CreateWindowExW".to_string(),
    }))
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
fn every_win_entry_point_returns_a_run_error_result() {
    // Nothing is invoked — each of these would create a Win32 window and
    // block until it closed. The coercion is the assertion: it only
    // type-checks while the entry point's return type is exactly
    // `Result<(), RunError>`. `win::run`'s own coercion is additionally
    // the proof that `host_main` below accepts it unchanged.
    let _run: fn(NoopApp) -> Result<(), RunError> = quadraui::win::run;
    let _run_with: fn(NoopApp, RunConfig) -> Result<(), RunError> = quadraui::win::run_with;
    let _run_with_shell: fn(NoopShell, ShellConfig) -> Result<(), RunError> =
        quadraui::win::shell_runner::run_with_shell;

    assert!(
        host_main(exited_gracefully).is_ok(),
        "a closed window must reach the host's `fn main` as Ok(()), not an error"
    );
}

/// A failed native call propagated out of the runner through `?` keeps
/// its cause, so a host still sees which Win32 call failed rather than a
/// generic failure.
#[test]
fn a_backend_failure_propagates_through_question_mark_with_its_cause() {
    let err = host_main(run_inner_failed)
        .expect_err("the stand-in runner returns Err, so host_main must propagate it");

    assert!(
        matches!(err, RunError::Backend(BackendError::PlatformFailure { .. })),
        "a failed native call must arrive as RunError::Backend(PlatformFailure), got {err:?}"
    );
    assert!(
        format!("{err}").contains("CreateWindowExW"),
        "the failing call's name must survive into the Display output, got {err}"
    );
}
