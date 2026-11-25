//! macOS shell runner: `run_with_shell()` wraps a [`ShellApp`] in an AppShell and
//! drives it through the AppKit event loop, forwarding its `Result<(), RunError>`.
//!
//! The shared [`crate::shell_adapter::ShellAdapter`] owns all runtime logic
//! (bottom-panel init, panel routing, resize recalc, click dispatch), and
//! [`crate::shell_adapter::build_shell_adapter`] owns assembling one from a
//! [`ShellConfig`]. This module only wires the adapter into the macOS event
//! loop, mirroring `tui::shell_runner` / `gtk::shell_runner`.
//!
//! `MacBackend` already paints everything `AppShell` renders (its
//! activity bar and status bars through the widget contract,
//! `draw_menu_bar`, …), so no new rasteriser
//! work is needed here — this is pure composition, same as the TUI/GTK
//! runners.
//!
//! `config.title` reaches the runner via [`super::run::RunConfig`], mirroring
//! `gtk::shell_runner::run_with_shell` / `win::shell_runner::run_with_shell`.
//!
//! [`ShellConfig::client_side_titlebar`] reaches the runner the same
//! way — copied onto [`RunConfig`] before this module's `config` is moved
//! into [`build_shell_adapter`], since `RunConfig` (not `ShellConfig`) is
//! what [`super::run::run_with`]'s window-creation call site actually
//! consults.

use super::run::RunConfig;
use crate::shell::{ShellApp, ShellConfig};
use crate::{shell_adapter::build_shell_adapter, RunError};

/// Run a [`ShellApp`] with AppShell chrome on the macOS backend.
///
/// `config.app_icon` (set via [`ShellConfig::with_app_icon`])
/// reaches [`RunConfig::app_icon`] the same way `config.title` /
/// `config.client_side_titlebar` do — cloned out before `config` moves
/// into [`build_shell_adapter`], since `RunConfig` (not `ShellConfig`)
/// is what [`super::run::run_with`]'s AppKit-bootstrap call site
/// actually consults.
///
/// **Must be called from the main thread** — enforced transitively by
/// [`super::run::run_with`].
pub fn run_with_shell<A: ShellApp + 'static>(app: A, config: ShellConfig) -> Result<(), RunError> {
    let mut run_config =
        RunConfig::new(config.title.clone()).with_client_side_titlebar(config.client_side_titlebar);
    if let Some(app_icon) = config.app_icon.clone() {
        run_config = run_config.with_app_icon(app_icon);
    }
    let adapter = build_shell_adapter(app, config);
    super::run::run_with(adapter, run_config)
}

/// Build a [`ShellApp`] as a [`crate::macos::run::MacRunner`] instead of
/// entering [`run_with_shell`]'s blocking `NSApplication::run()` loop —
/// for a host that wants the AppShell chrome but also needs
/// [`crate::macos::run::MacRunner::attach_control`] (see
/// [`crate::control`]'s module doc), which `run_with_shell` (which owns
/// the process for its whole lifetime) can't give. Mirrors
/// [`crate::tui::shell_runner::runner_with_shell`]/
/// [`crate::gtk::shell_runner::runner_with_shell`]'s identical shape for
/// the TUI/GTK backends.
///
/// **Must be called from the main thread** — enforced transitively by
/// [`super::run::MacRunner::new_with`].
#[cfg(feature = "control")]
#[cfg_attr(docsrs, doc(cfg(feature = "control")))]
pub fn runner_with_shell<A: ShellApp + 'static>(
    app: A,
    config: ShellConfig,
) -> super::run::MacRunner {
    let mut run_config =
        RunConfig::new(config.title.clone()).with_client_side_titlebar(config.client_side_titlebar);
    if let Some(app_icon) = config.app_icon.clone() {
        run_config = run_config.with_app_icon(app_icon);
    }
    let adapter = build_shell_adapter(app, config);
    super::run::MacRunner::new_with(adapter, run_config)
}
