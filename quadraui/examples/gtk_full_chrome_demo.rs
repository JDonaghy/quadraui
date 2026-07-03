//! GTK runner for the full-chrome AppShell demo (#217 Stage 2).
//!
//! Drag the empty part of the title bar to move the window; double-click
//! it to toggle maximize/restore (#400).
#[path = "common/mod.rs"]
mod common;

fn main() {
    let app = common::full_chrome_demo::FullChromeDemo::new();
    let config = common::full_chrome_demo::FullChromeDemo::config();
    quadraui::gtk::shell_runner::run_with_shell(app, config);
}
