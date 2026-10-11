//! TUI runner for the shell + MenuSystem regression demo.
//!
//! Open the "File" menu and click an item — including the ones drawn
//! over the activity bar strip on the left. Menu clicks are properly
//! routed through `AppShell`'s chrome to the app.
#[path = "common/mod.rs"]
mod common;

fn main() {
    let app = common::ShellMenuDemo::new();
    let config = common::shell_menu_demo::ShellMenuDemo::config();
    quadraui::tui::shell_runner::run_with_shell(app, config);
}
