//! `SidebarPanel` + `Toolbar` `AppLogic` + `quadraui::tui::run` example.
//!
//! Reproduces the `Panel` title-bar shape (title, close/maximize
//! actions, content area) with `SidebarPanel` + `Toolbar`.
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
