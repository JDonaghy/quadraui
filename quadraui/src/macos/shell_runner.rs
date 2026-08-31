//! macOS shell runner: `run_with_shell()` wraps a [`ShellApp`] in an
//! AppShell and drives it through the standard macOS (AppKit) event loop.
//!
//! The shared [`crate::shell_adapter::ShellAdapter`] owns all runtime logic
//! (bottom-panel init, panel routing, resize recalc, click dispatch), and
//! [`crate::shell_adapter::build_shell_adapter`] owns assembling one from a
//! [`ShellConfig`] — shared across every backend since #497. This module
//! only wires the adapter into the macOS event loop, mirroring
//! `tui::shell_runner` / `gtk::shell_runner` (#465).
//!
//! `MacBackend` already implements every `Backend` trait method `AppShell`
//! renders through (`draw_activity_bar`, `draw_sidebar_panel`,
//! `draw_tab_bar`, `draw_panel`, `draw_status_bar`, …), so no new rasteriser
//! work is needed here — this is pure composition, same as the TUI/GTK
//! runners.

use crate::shell::{ShellApp, ShellConfig};
use crate::shell_adapter::build_shell_adapter;

/// Run a [`ShellApp`] with AppShell chrome on the macOS backend.
///
/// **Must be called from the main thread** — enforced transitively by
/// [`super::run::run`].
pub fn run_with_shell<A: ShellApp + 'static>(
    app: A,
    config: ShellConfig,
) -> std::process::ExitCode {
    let adapter = build_shell_adapter(app, config);
    super::run::run(adapter)
}
