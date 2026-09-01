//! GTK shell runner: `run_with_shell()` wraps a [`ShellApp`] in an
//! AppShell and drives it through the standard GTK event loop.
//!
//! The shared [`crate::shell_adapter::ShellAdapter`] owns all runtime logic
//! (bottom-panel init, panel routing, resize recalc, click dispatch), and
//! [`crate::shell_adapter::build_shell_adapter`] owns assembling one from a
//! [`ShellConfig`] — shared across every backend since #497 (previously this
//! module re-inlined the same ~45 lines `tui::shell_runner` did). This
//! module only wires the adapter into the GTK event loop.

use super::run::RunConfig;
use crate::shell::{ShellApp, ShellConfig};
use crate::shell_adapter::build_shell_adapter;

/// Run a [`ShellApp`] with AppShell chrome on the GTK backend.
///
/// Before quadraui#656, this always ran through [`super::run::run`], which
/// hardcodes the generic app id (`"org.quadraui.app"`), the generic window
/// title (`"quadraui app"` — [`ShellConfig::title`] was captured but never
/// actually reached the GTK window), and no icon. `RunConfig::app_id`
/// feeds `gtk4::Application::builder().application_id(..)`, which both
/// live GDK backends derive the toplevel's identity from (Wayland
/// `xdg_toplevel.set_app_id`; X11 `WM_CLASS`) — the key the window manager
/// uses to match a live window to an installed `.desktop` file. With the
/// generic id, every downstream app's taskbar/dock/Alt-Tab entry showed a
/// generic icon regardless of how correctly it installed its own icon
/// theme entries. `config.app_id` / `config.icon_name` (set via
/// [`ShellConfig::with_app_id`] / [`ShellConfig::with_icon_name`]) now
/// reach the runner via [`RunConfig`], fixing both — and `config.title`
/// reaching the window is a side effect of the same plumbing.
pub fn run_with_shell<A: ShellApp + 'static>(app: A, config: ShellConfig) {
    let mut run_config = RunConfig::new(config.app_id.clone(), config.title.clone());
    if let Some(icon_name) = config.icon_name.clone() {
        run_config = run_config.with_icon_name(icon_name);
    }
    let adapter = build_shell_adapter(app, config);
    super::run::run_with(adapter, run_config);
}
