//! `Canvas` primitive `AppLogic` + `quadraui::tui::run` example (issue
//! #1102).
//!
//! A custom gauge — a shape the 40 other shipped primitives don't cover —
//! painted entirely through `DrawOp`s. On TUI the track/fill/tick marks
//! degrade to the sub-cell braille grid `Chart`'s line charts already
//! use; the percentage label still snaps to a whole cell. See
//! `quadraui::Canvas`'s module doc for the full degrade table.
//!
//! ```sh
//! cargo run --example tui_canvas --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::CanvasApp::new())
}
