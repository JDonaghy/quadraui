//! `cargo run --example win_app --features win` (Windows only)
//!
//! Win-GUI port of `tui_app.rs` / `gtk_app.rs` / `macos_app.rs`. Same
//! `MiniApp` `AppLogic` impl in `examples/common/mod.rs`; only the
//! runner call differs. Opens a native window with a single
//! bottom-anchored `StatusBar`.
//!
//! Press any key to bump the counter; `q` or Esc to quit.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::win::run(common::MiniApp::new())
}
