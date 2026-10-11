//! GTK runner for the full-chrome AppShell demo.
//!
//! Drag the empty part of the title bar to move the window; double-click
//! it to toggle maximize/restore. Drag any outer window edge/corner
//! (other than the top, which the title bar owns) to resize — hover
//! shows the OS resize cursor.
#[path = "common/mod.rs"]
mod common;

fn main() {
    let app = common::full_chrome_demo::FullChromeDemo::new();
    let config = common::full_chrome_demo::FullChromeDemo::config();
    quadraui::gtk::shell_runner::run_with_shell(app, config);
}
