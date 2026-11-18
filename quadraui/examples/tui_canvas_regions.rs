//! `Canvas::regions`/`Canvas::focusable` `AppLogic` + `quadraui::tui::run`
//! example.
//!
//! Two named, independently clickable/focusable [`quadraui::CanvasRegion`]s
//! on a single [`quadraui::Canvas`] — click-per-region, a hover cursor, and
//! Tab-to-focus plus a keystroke reaching the focused canvas. See
//! `examples/common/canvas_regions_demo.rs`'s module doc for the full
//! story.
//!
//! TUI-only: `Canvas` hit-testing of named regions is a backend-neutral
//! seam (`CanvasLayout::region_hit_test`, `primitives/canvas.rs`), but this
//! example itself targets the TUI backend only (no `gtk_canvas_regions`
//! pairing in this issue).
//!
//! ```sh
//! cargo run --example tui_canvas_regions --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::CanvasRegionsDemo::new())
}
