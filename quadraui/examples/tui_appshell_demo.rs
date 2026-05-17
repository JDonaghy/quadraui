//! TUI runner for AppShell demo — proves run_with_shell() pattern.
#[path = "common/mod.rs"]
mod common;

fn main() {
    let app = common::appshell_demo::AppShellDemo::new();
    let config = common::appshell_demo::AppShellDemo::config();
    quadraui::tui::shell_runner::run_with_shell(app, config);
}
