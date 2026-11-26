//! Minimap `AppLogic` + `quadraui::gtk::run` example.
//!
//! Code-overview minimap demo, painted through the `Minimap` widget
//! contract — real character shapes from a cached glyph atlas. Up/Down to scroll, click the
//! minimap to seek, q to quit — the buffer is taller than the strip, so
//! scrolling slides the visible window across the map.
//!
//! ```sh
//! cargo run --example gtk_minimap --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> Result<(), quadraui::RunError> {
    quadraui::gtk::run(common::MinimapApp::new())
}
