//! macOS port of `tui_menu_bar.rs` / `gtk_menu_bar.rs`. Same
//! `MenuBarApp` `AppLogic` impl in `examples/common/menu_bar_app.rs`;
//! only the runner call differs. Paints an in-window `MenuBar` at the
//! top with a `StatusBar` at the bottom.
//!
//! Note: this example uses the painted in-window `MenuBar` primitive
//! (consistent across all backends). Future work in #184 will add a
//! native `NSMenu` install path for apps that want the system menu bar
//! at the top of the screen.
//!
//! Click a menu item to activate. `q` or Esc to quit.
//!
//! ```sh
//! cargo run --example macos_menu_bar --features macos
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::macos::run(common::MenuBarApp::new())
}
