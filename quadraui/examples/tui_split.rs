//! Split `AppLogic` + `quadraui::tui::run` example.
//!
//! Draggable split with two labelled panes. Press `v` to toggle
//! direction, `r` to reset, `q` to quit.
//!
//! ```sh
//! cargo run --example tui_split --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::SplitApp::new())
}
