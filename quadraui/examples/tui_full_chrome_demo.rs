//! TUI runner for the full-chrome AppShell demo (#217 Stage 2).
//!
//! No window to drag/maximize here — clicking/double-clicking the title
//! bar exercises the same code path as GTK but is a documented no-op
//! (#400).
#[path = "common/mod.rs"]
mod common;

fn main() {
    let app = common::full_chrome_demo::FullChromeDemo::new();
    let config = common::full_chrome_demo::FullChromeDemo::config();
    quadraui::tui::shell_runner::run_with_shell(app, config);
}
