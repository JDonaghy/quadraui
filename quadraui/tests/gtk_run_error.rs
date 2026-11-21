//! Proof that the GTK runner's public entry points report failure as a
//! [`quadraui::RunError`] a host's `fn main` can `?`-propagate.
//!
//! This file is a separate compilation unit, so the signatures coerced
//! below are the real public API, not an in-crate convenience.
//!
//! All three entry points ([`quadraui::gtk::run`],
//! [`quadraui::gtk::run_with`], [`quadraui::gtk::shell_runner::run_with_shell`])
//! own the process for their whole lifetime — they build a `GtkApplication`,
//! need a live display, and only return once the last window closes — so a
//! test harness cannot call one and come back. What is checkable on any host
//! is the contract itself: each entry point returns `Result<(), RunError>`,
//! that type drops straight into a `-> Result<(), RunError>` host `main` via
//! `?`, and a non-success GLib exit code arrives as [`RunError::Code`] with
//! the code intact instead of being flattened to a bare failure. The live
//! display path stays covered by the headless `QUADRAUI_GTK_SMOKE_MS` mode
//! (`scripts/gtk_smoke.sh`).
#![cfg(feature = "gtk")]

use quadraui::gtk::RunConfig;
use quadraui::prelude::*;
use quadraui::{AppShellLayout, RunError};

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
/// [`quadraui::gtk::shell_runner::run_with_shell`]'s signature.
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

/// Stand-in for a runner whose GLib main loop exited non-zero (or whose
/// headless smoke self-check failed), so the propagation through
/// [`host_main`] can be exercised without a display.
fn glib_loop_exited_nonzero(_app: NoopApp) -> Result<(), RunError> {
    Err(RunError::Code(3))
}

/// Stand-in for a runner the user closed the window out of cleanly.
fn exited_gracefully(_app: NoopApp) -> Result<(), RunError> {
    Ok(())
}

/// Every entry point hands back `Result<(), RunError>`. Each coercion
/// below fails to compile if the return type drifts — including back to
/// `std::process::ExitCode`, or to `()` for `run_with_shell`, which would
/// make a GLib-level failure invisible to its caller.
#[test]
fn every_gtk_entry_point_returns_a_run_error_result() {
    // Nothing is invoked — each of these would create a `GtkApplication`
    // and block until its last window closed. The coercion is the
    // assertion: it only type-checks while the entry point's return type
    // is exactly `Result<(), RunError>`. `gtk::run`'s own coercion is
    // additionally the proof that `host_main` below accepts it unchanged.
    let _run: fn(NoopApp) -> Result<(), RunError> = quadraui::gtk::run;
    let _run_with: fn(NoopApp, RunConfig) -> Result<(), RunError> = quadraui::gtk::run_with;
    let _run_with_shell: fn(NoopShell, ShellConfig) -> Result<(), RunError> =
        quadraui::gtk::shell_runner::run_with_shell;

    assert!(
        host_main(exited_gracefully).is_ok(),
        "a closed window must reach the host's `fn main` as Ok(()), not an error"
    );
}

/// A non-success GLib exit code propagated out of the runner through `?`
/// keeps the code itself, so a host (or `xvfb-run`) still sees the status
/// the main loop reported rather than a generic failure.
#[test]
fn a_nonzero_glib_exit_propagates_through_question_mark_with_its_code() {
    let err = host_main(glib_loop_exited_nonzero)
        .expect_err("the stand-in runner returns Err, so host_main must propagate it");

    assert!(
        matches!(err, RunError::Code(3)),
        "a non-success GLib exit code must arrive as RunError::Code with the code intact, \
         got {err:?}"
    );
    assert!(
        format!("{err}").contains('3'),
        "the exit code must survive into the Display output, got {err}"
    );
}
