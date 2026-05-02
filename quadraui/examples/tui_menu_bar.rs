//! MenuBar `AppLogic` + `quadraui::tui::run` example.
//!
//! Same shape as `examples/tui_app.rs` but with a menu bar at the top.
//! See `gtk_menu_bar.rs` for the GTK twin — same `MenuBarApp`,
//! different runner.
//!
//! Run with:
//!
//! ```sh
//! cargo run --example tui_menu_bar --features tui
//! ```
//!
//! Click a menu item or press Alt+F/E/V to activate. `q` or Esc to quit.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::MenuBarApp::new())
}
