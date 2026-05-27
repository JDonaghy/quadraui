//! SidebarPanel `AppLogic` + `quadraui::gtk::run` example. Same logic
//! as `tui_sidebar_panel`, paired by shape — see
//! `examples/common/sidebar_panel_app.rs`.
//!
//! ```sh
//! cargo run --example gtk_sidebar_panel --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::SidebarPanelApp::new())
}
