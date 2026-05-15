//! `cargo run --example macos_app --features macos`
//!
//! macOS port of `tui_app.rs` / `gtk_app.rs`. Same `MiniApp`
//! `AppLogic` impl in `examples/common/mod.rs`; only the runner call
//! differs. Opens a native AppKit window with a single bottom-anchored
//! `StatusBar`.
//!
//! Press any key to bump the counter; `q` or Esc to quit.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::macos::run(common::MiniApp::new())
}
