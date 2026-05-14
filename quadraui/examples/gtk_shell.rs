//! AppShell `AppLogic` + `quadraui::gtk::run` example.
//!
//! Activity bar + sidebar panel container with toggle/switch/resize.
//! Click icons, drag divider, press `q` to quit.
//!
//! ```sh
//! cargo run --example gtk_shell --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::ShellApp::new())
}
