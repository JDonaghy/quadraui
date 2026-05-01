//! Minimal `AppLogic` + `quadraui::gtk::run` example.
//!
//! Same shape as `examples/tui_app.rs` but rendered in a GTK window
//! with a single full-window `DrawingArea`. The `MiniApp` state and
//! `AppLogic` impl live in `examples/common/mod.rs` — the only
//! difference between this file and `tui_app.rs` is the runner call
//! (`quadraui::gtk::run` vs `quadraui::tui::run`).
//!
//! Run with:
//!
//! ```sh
//! cargo run --example gtk_app --features gtk
//! ```
//!
//! Press any key to bump the counter; `q` or Esc to quit.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::MiniApp::new())
}
