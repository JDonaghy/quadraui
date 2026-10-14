//! Button + RadioGroup + Switch `AppLogic` + `quadraui::tui::run` example.
//!
//! Tab to move focus, Space/Enter to activate, Up/Down to move the radio
//! group cursor, `q` to quit.
//!
//! ```sh
//! cargo run --example tui_controls --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::ControlsApp::new())
}
