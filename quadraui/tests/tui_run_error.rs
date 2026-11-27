//! Proof that the TUI runner's public entry points report failure as a
//! [`quadraui::RunError`] a host's `fn main` can `?`-propagate.
//!
//! This file is a separate compilation unit, so the signatures coerced
//! below are the real public API, not an in-crate convenience.
//!
//! All three entry points ([`quadraui::tui::run`],
//! [`quadraui::tui::run_with`], [`quadraui::tui::shell_runner::run_with_shell`])
//! own the process' terminal for their whole lifetime — raw mode plus the
//! alternate screen, blocking on real key events — so a test harness
//! cannot call one and come back. What is checkable without a TTY is the
//! contract itself: each entry point returns `Result<(), RunError>`, that
//! type drops straight into a `-> Result<(), RunError>` host `main` via
//! `?`, and a failure's cause survives the propagation instead of being
//! discarded. The live raw-mode path stays covered by
//! `tests/tui_pty_smoke.rs`.
#![cfg(feature = "tui")]

use quadraui::prelude::*;
use quadraui::tui::RunConfig;
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
/// [`quadraui::tui::shell_runner::run_with_shell`]'s signature.
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

/// Stand-in for a runner whose terminal setup failed, so the propagation
/// through [`host_main`] can be exercised without owning a TTY.
fn terminal_setup_failed(_app: NoopApp) -> Result<(), RunError> {
    Err(std::io::Error::other("enable_raw_mode failed").into())
}

/// Stand-in for a runner the app quit cleanly out of.
fn exited_gracefully(_app: NoopApp) -> Result<(), RunError> {
    Ok(())
}

/// Every entry point hands back `Result<(), RunError>`. Each coercion
/// below fails to compile if the return type drifts — including back to
/// `io::Result<()>`, or to `()` for `run_with_shell`, which would make a
/// terminal-setup failure invisible to its caller.
#[test]
fn every_tui_entry_point_returns_a_run_error_result() {
    // Nothing is invoked — each of these would seize the terminal and
    // block on real key events. The coercion is the assertion: it only
    // type-checks while the entry point's return type is exactly
    // `Result<(), RunError>`. `tui::run`'s own coercion is additionally
    // the proof that `host_main` below accepts it unchanged.
    let _run: fn(NoopApp) -> Result<(), RunError> = quadraui::tui::run;
    let _run_with: fn(NoopApp, RunConfig) -> Result<(), RunError> = quadraui::tui::run_with;
    let _run_with_shell: fn(NoopShell, ShellConfig) -> Result<(), RunError> =
        quadraui::tui::shell_runner::run_with_shell;

    assert!(
        host_main(exited_gracefully).is_ok(),
        "a graceful exit must reach the host's `fn main` as Ok(()), not an error"
    );
}

/// A failure propagated out of the runner through `?` keeps its cause, so
/// a host can log the real reason instead of a bare exit code.
#[test]
fn a_runner_failure_propagates_through_question_mark_with_its_cause() {
    let err = host_main(terminal_setup_failed)
        .expect_err("the stand-in runner returns Err, so host_main must propagate it");

    assert!(
        matches!(err, RunError::Io(_)),
        "an io failure from terminal setup must arrive as RunError::Io, got {err:?}"
    );
    assert!(
        std::error::Error::source(&err).is_some(),
        "RunError must expose the underlying io error as its source so a host can log the cause"
    );
    assert!(
        format!("{err}").contains("enable_raw_mode failed"),
        "the cause's message must survive into the Display output, got {err}"
    );
}
