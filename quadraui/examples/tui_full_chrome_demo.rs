//! TUI runner for the full-chrome AppShell demo.
//!
//! Clicking or double-clicking the title bar exercises the same code path
//! as GTK but is a documented no-op. Edge-resize clicking runs the same call
//! path but reports back "no window".
#[path = "common/mod.rs"]
mod common;

fn main() {
    let app = common::full_chrome_demo::FullChromeDemo::new();
    let config = common::full_chrome_demo::FullChromeDemo::config();
    quadraui::tui::shell_runner::run_with_shell(app, config);
}
