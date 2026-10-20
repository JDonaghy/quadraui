//! `Canvas` v2 op `AppLogic` + `quadraui::tui::run` example.
//!
//! A chart-style demo exercising the `DrawOp` variants `canvas_app`'s
//! older gauge demo doesn't: `Polygon`, `Ellipse`, `Arc`, `Bezier`,
//! `DashedLine`, `StrokeRect` and `StyledTextRun`. Every op here
//! degrades to the sub-cell braille grid on TUI — see
//! `quadraui::DrawOp`'s module doc for the full degrade table.
//!
//! ```sh
//! cargo run --example tui_canvas_shapes --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::CanvasShapesApp::new())
}
