//! Win-GUI shell runner: `run_with_shell()` wraps a [`ShellApp`] in an
//! AppShell and drives it through the standard Win32 message loop.
//!
//! The shared [`crate::shell_adapter::ShellAdapter`] owns all runtime logic
//! (bottom-panel init, panel routing, resize recalc, click dispatch), and
//! [`crate::shell_adapter::build_shell_adapter`] owns assembling one from a
//! [`ShellConfig`] — shared across every backend since #497. This module
//! only wires the adapter into the Win32 message loop, mirroring
//! `tui::shell_runner` / `gtk::shell_runner` / `macos::shell_runner` — the
//! Windows analogue of #465 (quadraui#707).
//!
//! `config.title` reaches the real Win32 window via [`RunConfig`], the
//! same way `gtk::shell_runner::run_with_shell` threads `config.title`
//! through `gtk::run::RunConfig` — see that module's doc for why a bare
//! `win::run` isn't enough for a shell consumer (the window title is
//! otherwise hardcoded to `"quadraui"`).
//!
//! `WinBackend` still carries dozens of `todo!()` rasteriser stubs (see
//! `win::backend`'s module docs) — this module composes purely over the
//! `Backend` trait, so it compiles and runs against those stubs today;
//! each stub retired afterwards lights up more of the shell with no
//! further change needed here.

use super::run::RunConfig;
use crate::shell::{ShellApp, ShellConfig};
use crate::shell_adapter::build_shell_adapter;

/// Run a [`ShellApp`] with AppShell chrome on the Win-GUI backend.
pub fn run_with_shell<A: ShellApp + 'static>(
    app: A,
    config: ShellConfig,
) -> std::process::ExitCode {
    let run_config = RunConfig::new(config.title.clone());
    let adapter = build_shell_adapter(app, config);
    super::run::run_with(adapter, run_config)
}
