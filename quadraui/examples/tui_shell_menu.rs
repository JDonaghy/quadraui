//! TUI runner for the shell + MenuSystem regression demo (#411).
//!
//! Open the "File" menu and click an item — including the ones drawn
//! over the activity bar strip on the left. Before the #411 fix, those
//! clicks were silently swallowed by `AppShell`'s chrome hit-testing and
//! never reached the app.
#[path = "common/mod.rs"]
mod common;

fn main() {
    let app = common::ShellMenuDemo::new();
    let config = common::shell_menu_demo::ShellMenuDemo::config();
    quadraui::tui::shell_runner::run_with_shell(app, config);
}
