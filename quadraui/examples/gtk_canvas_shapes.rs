//! `Canvas` v2 op `AppLogic` + `quadraui::gtk::run` example.
//!
//! Same chart-style demo as `tui_canvas_shapes`, painted via Cairo
//! through the same `Backend::draw_canvas` call — no per-backend app
//! code at all.
//!
//! ```sh
//! cargo run --example gtk_canvas_shapes --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::CanvasShapesApp::new())
}
