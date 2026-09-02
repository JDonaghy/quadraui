//! Win-GUI port of `tui_menu_bar.rs` / `gtk_menu_bar.rs` /
//! `macos_menu_bar.rs`. Same `MenuBarApp` `AppLogic` impl in
//! `examples/common/menu_bar_app.rs`; only the runner call differs.
//! Paints an in-window `MenuBar` at the top with a `StatusBar` at the
//! bottom.
//!
//! Note: this example uses the painted in-window `MenuBar` primitive
//! (consistent across all backends), not a native `HMENU`.
//!
//! Click a menu item to activate. `q` or Esc to quit.
//!
//! ```sh
//! cargo run --example win_menu_bar --features win
//! ```
//! (Windows only.)

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::win::run(common::MenuBarApp::new())
}
