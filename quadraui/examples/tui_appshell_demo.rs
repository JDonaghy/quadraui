//! TUI runner for ShellView demo — proves run_with_shell() pattern.
#[path = "common/mod.rs"]
mod common;

fn main() -> Result<(), quadraui::RunError> {
    let app = common::appshell_demo::AppShellDemo::new();
    let config = common::appshell_demo::AppShellDemo::config();
    quadraui::tui::shell_runner::run_with_shell(app, config)
}
