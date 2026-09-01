//! Image + MenuBar leading-icon-slot `AppLogic` + `quadraui::gtk::run`
//! example.
//!
//! Paints the real `examples/assets/quadra_logo.png` asset via
//! `gdk_pixbuf` at the left of the menu bar — click a menu item, q to
//! quit.
//!
//! ```sh
//! cargo run --example gtk_image --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::ImageApp::new())
}
