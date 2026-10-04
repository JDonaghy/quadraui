//! `Canvas` primitive `AppLogic` + `quadraui::gtk::run` example (issue
//! #1102).
//!
//! Same gauge as `tui_canvas`, painted via Cairo through the same
//! `Backend::draw_canvas` call — no per-backend app code at all.
//!
//! ```sh
//! cargo run --example gtk_canvas --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::CanvasApp::new())
}
