//! MenuBar `AppLogic` + `quadraui::gtk::run` example.
//!
//! Same shape as `examples/gtk_app.rs` but with a menu bar at the top.
//! See `tui_menu_bar.rs` for the TUI twin — same `MenuBarApp`,
//! different runner.
//!
//! Run with:
//!
//! ```sh
//! cargo run --example gtk_menu_bar --features gtk
//! ```
//!
//! Click a menu item or press Alt+F/E/V to activate. `q` or Esc to quit.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::MenuBarApp::new())
}
