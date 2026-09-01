//! Image + MenuBar leading-icon-slot `AppLogic` + `quadraui::tui::run`
//! example.
//!
//! TUI can't rasterise the logo PNG, so it paints the `Image`
//! descriptor's fallback text instead (`[Q]`) — click a menu item, q to
//! quit.
//!
//! ```sh
//! cargo run --example tui_image --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::ImageApp::new())
}
