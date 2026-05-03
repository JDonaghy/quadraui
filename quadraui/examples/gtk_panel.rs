//! Panel `AppLogic` + `quadraui::gtk::run` example.
//!
//! Panel with title bar, close/maximize actions, content area.
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
