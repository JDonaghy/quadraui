//! Minimap `AppLogic` + `quadraui::gtk::run` example.
//!
//! Code-overview minimap demo, painted through `GtkBackend::draw_minimap`
//! — real character shapes from a cached glyph atlas (issue #1035), not
//! per-column colour blocks (`crate::gtk::minimap::draw_minimap`, the
//! uncached free function, still paints the older #667 colour-block look,
//! but the app path here doesn't call it). Up/Down to scroll, click the
//! minimap to seek, q to quit — the buffer is taller than the strip, so
//! scrolling slides the visible window across the map.
//!
//! ```sh
//! cargo run --example gtk_minimap --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::MinimapApp::new())
}
