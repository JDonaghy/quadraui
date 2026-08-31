//! macOS runner for AppShell demo — proves `run_with_shell()` pattern
//! (#465). Same `AppShellDemo` `ShellApp` impl `tui_appshell_demo.rs` /
//! `gtk_appshell_demo.rs` already share; only the runner call differs.
#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    let app = common::appshell_demo::AppShellDemo::new();
    let config = common::appshell_demo::AppShellDemo::config();
    quadraui::macos::shell_runner::run_with_shell(app, config)
}
