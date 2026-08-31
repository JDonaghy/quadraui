//! Minimap `AppLogic` + `quadraui::tui::run` example.
//!
//! Code-overview minimap demo, painted via TUI braille.
//! Up/Down to scroll, click the minimap to seek, q to quit.
//!
//! ```sh
//! cargo run --example tui_minimap --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::MinimapApp::new())
}
