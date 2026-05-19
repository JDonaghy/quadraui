//! TUI runner for the full-chrome AppShell demo (#217 Stage 2).
#[path = "common/mod.rs"]
mod common;

fn main() {
    let app = common::full_chrome_demo::FullChromeDemo::new();
    let config = common::full_chrome_demo::FullChromeDemo::config();
    quadraui::tui::shell_runner::run_with_shell(app, config);
}
