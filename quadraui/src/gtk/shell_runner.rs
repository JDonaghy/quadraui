//! GTK shell runner: `run_with_shell()` wraps a [`ShellApp`] in an
//! AppShell and drives it through the standard GTK event loop.
//!
//! The shared [`crate::shell_adapter::ShellAdapter`] owns all runtime logic
//! (bottom-panel init, panel routing, resize recalc, click dispatch), and
//! [`crate::shell_adapter::build_shell_adapter`] owns assembling one from a
//! [`ShellConfig`] — shared across every backend since #497 (previously this
//! module re-inlined the same ~45 lines `tui::shell_runner` did). This
//! module only wires the adapter into the GTK event loop.

use crate::shell::{ShellApp, ShellConfig};
use crate::shell_adapter::build_shell_adapter;

/// Run a [`ShellApp`] with AppShell chrome on the GTK backend.
pub fn run_with_shell<A: ShellApp + 'static>(app: A, config: ShellConfig) {
    let adapter = build_shell_adapter(app, config);
    super::run::run(adapter);
}
