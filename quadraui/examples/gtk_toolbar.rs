//! Toolbar `AppLogic` + `quadraui::gtk::run` example. Same logic as
//! `tui_toolbar`, paired by shape — see `examples/common/toolbar_app.rs`.
//!
//! ```sh
//! cargo run --example gtk_toolbar --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::ToolbarApp::new())
}
