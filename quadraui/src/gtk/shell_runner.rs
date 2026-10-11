//! GTK shell runner: `run_with_shell()` wraps a [`ShellApp`] in an
//! AppShell and drives it through the standard GTK event loop.
//!
//! The shared [`crate::shell_adapter::ShellAdapter`] owns all runtime logic
//! (bottom-panel init, panel routing, resize recalc, click dispatch), and
//! [`crate::shell_adapter::build_shell_adapter`] owns assembling one from a
//! [`ShellConfig`]. This module only wires the adapter into the GTK event loop.

use super::run::RunConfig;
use crate::shell::{ShellApp, ShellConfig};
use crate::shell_adapter::build_shell_adapter;

/// Run a [`ShellApp`] with AppShell chrome on the GTK backend.
///
/// The GTK window's identity (Wayland `xdg_toplevel.set_app_id`; X11 `WM_CLASS`)
/// comes from `RunConfig::app_id`, which the window manager uses to match the
/// live window to an installed `.desktop` file. `config.app_id` / `config.icon_name`
/// (set via [`ShellConfig::with_app_id`] / [`ShellConfig::with_icon_name`]) reach
/// the runner via [`RunConfig`]; `config.title` similarly reaches the GTK window.
pub fn run_with_shell<A: ShellApp + 'static>(app: A, config: ShellConfig) {
    let mut run_config = RunConfig::new(config.app_id.clone(), config.title.clone());
    if let Some(icon_name) = config.icon_name.clone() {
        run_config = run_config.with_icon_name(icon_name);
    }
    let adapter = build_shell_adapter(app, config);
    super::run::run_with(adapter, run_config);
}
