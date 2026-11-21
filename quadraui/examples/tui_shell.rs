//! AppShell `AppLogic` + `quadraui::tui::run` example.
//!
//! Activity bar + sidebar panel container with toggle/switch/resize.
//! Click icons, drag divider, press `q` to quit.
//!
//! ```sh
//! cargo run --example tui_shell --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> Result<(), quadraui::RunError> {
    quadraui::tui::run(common::ShellApp::new())
}
