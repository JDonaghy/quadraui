//! Panel `AppLogic` + `quadraui::tui::run` example.
//!
//! Panel with title bar, close/maximize actions, content area.
//! Press `c` to toggle collapsed, `q` to quit.
//!
//! ```sh
//! cargo run --example tui_panel --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::PanelApp::new())
}
