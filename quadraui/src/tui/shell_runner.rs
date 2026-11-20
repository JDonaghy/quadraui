//! TUI shell runner: `run_with_shell()` wraps a [`ShellApp`] in an
//! AppShell and drives it through the standard TUI event loop.
//!
//! The shared [`crate::shell_adapter::ShellAdapter`] owns all runtime logic
//! (bottom-panel init, panel routing, resize recalc, click dispatch), and
//! [`crate::shell_adapter::build_shell_adapter`] owns assembling one from a
//! [`ShellConfig`]. This module only wires the adapter into the TUI event loop.

use crate::shell::{ShellApp, ShellConfig};
use crate::shell_adapter::build_shell_adapter;

/// Run a [`ShellApp`] with AppShell chrome on the TUI backend.
pub fn run_with_shell<A: ShellApp + 'static>(app: A, config: ShellConfig) {
    let adapter = build_shell_adapter(app, config);
    let _ = super::run::run(adapter);
}

/// Build a [`ShellApp`] as a [`crate::tui::run::TuiRunner`] instead of
/// entering [`run_with_shell`]'s blocking loop — for a host that wants
/// the AppShell chrome but also needs
/// [`crate::tui::run::TuiRunner::attach_control`] (see [`crate::control`]'s
/// module doc) or the step/pump-level control `run_with_shell` (which owns
/// the process for its whole lifetime) can't give. Mirrors
/// [`crate::tui::testing::driver_with_shell`]'s identical shape for the
/// headless test driver.
#[cfg(feature = "control")]
#[cfg_attr(docsrs, doc(cfg(feature = "control")))]
pub fn runner_with_shell<A: ShellApp + 'static>(
    app: A,
    config: ShellConfig,
) -> std::io::Result<super::run::TuiRunner<impl crate::runner::AppLogic>> {
    let adapter = build_shell_adapter(app, config);
    super::run::TuiRunner::new(adapter)
}
