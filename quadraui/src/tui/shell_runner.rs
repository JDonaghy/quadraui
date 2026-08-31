//! TUI shell runner: `run_with_shell()` wraps a [`ShellApp`] in an
//! AppShell and drives it through the standard TUI event loop.
//!
//! The shared [`crate::shell_adapter::ShellAdapter`] owns all runtime logic
//! (bottom-panel init, panel routing, resize recalc, click dispatch), and
//! [`crate::shell_adapter::build_shell_adapter`] owns assembling one from a
//! [`ShellConfig`] — shared across every backend since #497. This module
//! only wires the adapter into the TUI event loop.

use crate::shell::{ShellApp, ShellConfig};
use crate::shell_adapter::build_shell_adapter;

/// Run a [`ShellApp`] with AppShell chrome on the TUI backend.
pub fn run_with_shell<A: ShellApp + 'static>(app: A, config: ShellConfig) {
    let adapter = build_shell_adapter(app, config);
    let _ = super::run::run(adapter);
}
