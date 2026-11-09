//! `SidebarPanel` + `Toolbar` `AppLogic` + `quadraui::gtk::run` example.
//!
//! Reproduces the `Panel` title-bar shape (title, close/maximize
//! actions, content area) with `SidebarPanel` + `Toolbar`.
//! Press `c` to toggle collapsed, `q` to quit.
//!
//! ```sh
//! cargo run --example gtk_panel --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::PanelApp::new())
}
