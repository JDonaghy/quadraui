//! Minimap `AppLogic` + `quadraui::gtk::run` example.
//!
//! Code-overview minimap demo, painted via GTK font scaling.
//! Up/Down to scroll, click the minimap to seek, q to quit.
//!
//! ```sh
//! cargo run --example gtk_minimap --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::MinimapApp::new())
}
