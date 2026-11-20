//! Win-GUI shell runner: `run_with_shell()` wraps a [`ShellApp`] in an
//! AppShell and drives it through the standard Win32 message loop.
//!
//! The shared [`crate::shell_adapter::ShellAdapter`] owns all runtime logic
//! (bottom-panel init, panel routing, resize recalc, click dispatch), and
//! [`crate::shell_adapter::build_shell_adapter`] owns assembling one from a
//! [`ShellConfig`]. This module wires the adapter into the Win32 message
//! loop, mirroring `tui::shell_runner` / `gtk::shell_runner` /
//! `macos::shell_runner`.
//!
//! `config.title` reaches the real Win32 window via [`RunConfig`], the
//! same way `gtk::shell_runner::run_with_shell` threads `config.title`
//! through `gtk::run::RunConfig` — see that module's doc for why a bare
//! `win::run` isn't enough for a shell consumer (the window title is
//! otherwise hardcoded to `"quadraui"`).
//!
//! `WinBackend` carries many `todo!()` rasteriser stubs (see
//! `win::backend`'s module docs) — this module composes purely over the
//! `Backend` trait, so it compiles and runs against those stubs; each stub
//! retired afterwards lights up more of the shell with no further change
//! needed here.

use super::run::RunConfig;
use crate::shell::{ShellApp, ShellConfig};
use crate::shell_adapter::build_shell_adapter;

/// Run a [`ShellApp`] with AppShell chrome on the Win-GUI backend.
///
/// `config.app_icon` (set via [`ShellConfig::with_app_icon`])
/// reaches [`RunConfig::app_icon`] the same way `config.title` does —
/// cloned out before `config` moves into [`build_shell_adapter`], since
/// `RunConfig` (not `ShellConfig`) is what [`super::run::run_with`]'s
/// window-creation call site actually consults.
pub fn run_with_shell<A: ShellApp + 'static>(
    app: A,
    config: ShellConfig,
) -> std::process::ExitCode {
    let mut run_config = RunConfig::new(config.title.clone());
    if let Some(app_icon) = config.app_icon.clone() {
        run_config = run_config.with_app_icon(app_icon);
    }
    let adapter = build_shell_adapter(app, config);
    super::run::run_with(adapter, run_config)
}

/// Build a [`ShellApp`] as a [`crate::win::run::WinRunner`] instead of
/// entering [`run_with_shell`]'s blocking Win32 message loop — for a host
/// that wants the AppShell chrome but also needs
/// [`crate::win::run::WinRunner::attach_control`] (see [`crate::control`]'s
/// module doc), which `run_with_shell` (which owns the process for its
/// whole lifetime) can't give. Mirrors
/// [`crate::tui::shell_runner::runner_with_shell`]/
/// [`crate::gtk::shell_runner::runner_with_shell`]/
/// [`crate::macos::shell_runner::runner_with_shell`]'s identical shape
/// for the TUI/GTK/macOS backends.
///
/// `target_os = "windows"`-gated (unlike `run_with_shell` above, which
/// has a no-op fallback on every other host): [`super::run::WinRunner`]
/// itself only exists under `target_os = "windows"` — see that type's
/// doc — so there is no non-Windows stand-in to return here.
///
/// # Safety
///
/// See [`super::run::WinRunner::new_with`]'s `# Safety` doc — must be
/// called on, and the returned `WinRunner` must be stepped from, a
/// single thread.
#[cfg(all(feature = "control", target_os = "windows"))]
#[cfg_attr(docsrs, doc(cfg(all(feature = "control", target_os = "windows"))))]
pub unsafe fn runner_with_shell<A: ShellApp + 'static>(
    app: A,
    config: ShellConfig,
) -> windows::core::Result<super::run::WinRunner<impl crate::AppLogic>> {
    let mut run_config = RunConfig::new(config.title.clone());
    if let Some(app_icon) = config.app_icon.clone() {
        run_config = run_config.with_app_icon(app_icon);
    }
    let adapter = build_shell_adapter(app, config);
    // SAFETY: forwards this function's own `# Safety` contract
    // (documented above) unchanged to `WinRunner::new_with`.
    unsafe { super::run::WinRunner::new_with(adapter, &run_config) }
}
