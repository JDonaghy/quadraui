//! The unified return type every backend's `run`/`run_with` converges on.
//!
//! Today the four backends disagree on what their entry point returns:
//! `tui::run`/`run_with` return [`std::io::Result`]`<()>` (terminal
//! setup/teardown is the only failure mode crossterm gives a structured
//! cause for); `gtk::run`, `macos::run`, and `win::run` all return
//! [`std::process::ExitCode`] (so an app's `main` is a one-liner, but the
//! *why* behind a non-zero exit — a missing display, a failed native
//! window create, a headless smoke self-check — is lost by the time it
//! reaches the exit code, and `ExitCode` doesn't implement
//! [`std::error::Error`], so it can't be `?`-propagated through anything
//! that expects one). [`RunError`] is the seam the per-backend follow-on
//! issues converge their `run`/`run_with` signatures on, so every backend
//! ends up returning `Result<(), RunError>` instead of either shape above.
//!
//! **This module only defines the type.** No backend's `run.rs` or
//! `shell_runner.rs` is wired to return it yet — that's deliberately split
//! into one issue per backend (per `CLAUDE.md`'s "one backend per issue"
//! rule) so each conversion is reviewed and tested against that backend's
//! own host, instead of one sprawling cross-backend diff.
//!
//! # Why not keep using `ExitCode` everywhere
//!
//! `std::process::ExitCode` is a fine shape for a process that has
//! nothing more to say than a number, but a `run`/`run_with` failure
//! always *does* have more to say: which syscall failed, which native API
//! call returned an error, which self-check didn't pass. Collapsing that
//! into a bare exit code before the caller ever sees it means the only
//! place the real cause can still show up is a `stderr` print the runner
//! does on its own initiative — the caller's `main` can't log it
//! differently, can't match on it, and can't wrap it in a richer
//! application error type with `?`. `ExitCode` also isn't
//! [`std::error::Error`], so it can't flow through the now-common
//! `fn main() -> Result<(), Box<dyn std::error::Error>>` idiom at all;
//! the TUI backend's `io::Result<()>` gets that part right but only for
//! the one failure shape `io::Error` can represent.
//!
//! [`RunError`] keeps the one-liner `main` ergonomics `ExitCode` gives
//! today — it implements [`std::process::Termination`], so
//! `RunError::report` turns a value into an [`std::process::ExitCode`]
//! exactly like `ExitCode` itself would — while also implementing
//! [`std::error::Error`] + [`std::fmt::Display`], so a `Result<(),
//! RunError>` can be `?`-propagated, logged with its real cause, and
//! matched on by variant.
//!
//! ```ignore
//! // The shape every backend's `run`/`run_with` will return once its own
//! // follow-on issue lands.
//! fn main() -> Result<(), quadraui::RunError> {
//!     quadraui::tui::run(MyApp)
//! }
//!
//! // Or, where the caller wants `ExitCode` ergonomics specifically
//! // (matches today's `gtk`/`macos`/`win` call sites) rather than letting
//! // the runtime's own `Result` glue print and exit on error:
//! fn main() -> std::process::ExitCode {
//!     match quadraui::tui::run(MyApp) {
//!         Ok(()) => std::process::ExitCode::SUCCESS,
//!         Err(err) => err.report(),
//!     }
//! }
//! ```
//!
//! One caveat worth calling out for whichever follow-on issue wires a
//! backend to this type: the standard library's blanket
//! `impl<T: Termination, E: Debug> Termination for Result<T, E>` only
//! requires `E: Debug`, not `E: Termination` — so a plain `fn main() ->
//! Result<(), RunError>` dispatches through *that* impl, not
//! [`RunError`]'s own, and always maps an `Err` to
//! [`std::process::ExitCode::FAILURE`] after a `Debug`-formatted
//! `stderr` print. [`RunError`]'s own [`std::process::Termination`] impl
//! (the second `main` shape above, calling `.report()` explicitly) is
//! what preserves [`RunError::Code`]'s specific exit code; it does not
//! change what a bare `-> Result<(), RunError>` `main` does.
//!
//! Compiled unconditionally, like `crash`/`update`/`power`: the type
//! itself carries no backend-specific state, so every backend (including
//! ones compiled out of a given build) can convert its own failures into
//! it without a feature gate.

use std::fmt;

use crate::backend::BackendError;

/// A `run`/`run_with` failure, unified across every backend.
///
/// `#[non_exhaustive]`: a downstream `match` with no wildcard arm still
/// compiles after a new variant lands (e.g. a `win`-specific HRESULT
/// shape), so adding one is additive, not breaking. Variant construction
/// (`RunError::Io(err)`, `RunError::Backend(err)`, `RunError::Code(2)`)
/// and single-variant destructuring are unaffected.
#[derive(Debug)]
#[non_exhaustive]
pub enum RunError {
    /// An OS-level I/O failure during backend setup or teardown — the
    /// TUI backend's raw-mode/alternate-screen/mouse-capture negotiation
    /// today, a future `win` backend's window-class registration
    /// tomorrow. Carries the original [`std::io::Error`] so the cause
    /// (`ErrorKind`, OS error code) survives instead of being summarized
    /// away before the caller sees it.
    Io(std::io::Error),
    /// A backend-reported failure at the same seam [`BackendError`]
    /// already covers for individual [`crate::backend::Backend`]
    /// methods (a native window/dialog call that failed, a lost render
    /// surface) — surfaced here because it happened while `run`/
    /// `run_with` itself was driving the loop, not inside an
    /// app-initiated call the app could otherwise recover from.
    Backend(BackendError),
    /// A plain numeric exit status with no richer cause to attach —
    /// the shape `gtk::run`/`gtk::run_with` falls back to today: GLib's
    /// own `Application::run` exit code, or the headless
    /// `QUADRAUI_GTK_SMOKE_MS` self-check's pass/fail folded into
    /// `std::process::ExitCode::FAILURE`. Kept as an escape hatch for
    /// exactly that case rather than inventing a `SelfCheckFailed`
    /// variant for a signal that is, structurally, just an exit code.
    Code(u8),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Io(err) => write!(f, "run failed: {err}"),
            RunError::Backend(err) => write!(f, "run failed: {err:?}"),
            RunError::Code(code) => write!(f, "run exited with status {code}"),
        }
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RunError::Io(err) => Some(err),
            // `BackendError` deliberately doesn't implement
            // `std::error::Error` (see its own doc comment) — nothing to
            // hand back here.
            RunError::Backend(_) | RunError::Code(_) => None,
        }
    }
}

impl std::process::Termination for RunError {
    // Exempt from the crate-wide `print_stderr` deny (`diagnostics`'s
    // module doc lists the library-internal call sites that go through
    // `diagnostics::emit` instead): by the time this runs, `run`/
    // `run_with` has already returned to the app's own `main` and torn
    // down whatever terminal/window state it owned (the TUI backend's
    // raw mode + alternate screen included), so this is no different
    // from the std blanket `Termination` impl's own `eprintln!` for
    // `Result<T, E: Debug>` — the caller *is* the host at this point,
    // not a library writing into a host's live cell grid out from under
    // it.
    #[allow(clippy::print_stderr)]
    fn report(self) -> std::process::ExitCode {
        eprintln!("error: {self}");
        match self {
            RunError::Io(_) | RunError::Backend(_) => std::process::ExitCode::FAILURE,
            RunError::Code(code) => std::process::ExitCode::from(code),
        }
    }
}

impl From<std::io::Error> for RunError {
    fn from(err: std::io::Error) -> Self {
        RunError::Io(err)
    }
}

impl From<BackendError> for RunError {
    fn from(err: BackendError) -> Self {
        RunError::Backend(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_variant_reports_failure_and_preserves_source() {
        let io_err = std::io::Error::new(std::io::ErrorKind::Other, "raw mode negotiation failed");
        let err = RunError::from(io_err);
        assert!(std::error::Error::source(&err).is_some());
        assert_eq!(
            std::process::Termination::report(err),
            std::process::ExitCode::FAILURE
        );
    }

    #[test]
    fn backend_variant_has_no_source_but_displays_cause() {
        let err = RunError::from(BackendError::Unsupported);
        assert!(std::error::Error::source(&err).is_none());
        assert!(format!("{err}").contains("Unsupported"));
    }

    #[test]
    fn code_variant_round_trips_through_report() {
        let err = RunError::Code(3);
        assert_eq!(
            std::process::Termination::report(err),
            std::process::ExitCode::from(3)
        );
    }

    #[test]
    fn display_is_non_empty_for_every_variant() {
        assert!(!format!("{}", RunError::Io(std::io::Error::other("x"))).is_empty());
        assert!(!format!("{}", RunError::Backend(BackendError::Unsupported)).is_empty());
        assert!(!format!("{}", RunError::Code(1)).is_empty());
    }
}
