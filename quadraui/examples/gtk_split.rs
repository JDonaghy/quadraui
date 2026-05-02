//! Split `AppLogic` + `quadraui::gtk::run` example.
//!
//! Draggable split with two labelled panes. Press `v` to toggle
//! direction, `r` to reset, `q` to quit.
//!
//! ```sh
//! cargo run --example gtk_split --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::SplitApp::new())
}
