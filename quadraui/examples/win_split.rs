//! `cargo run --example win_split --features win` (Windows only)
//!
//! Win-GUI port of `tui_split.rs` / `gtk_split.rs` / `macos_split.rs`.
//! Same `SplitApp` `AppLogic` impl in `examples/common/split_app.rs`;
//! only the runner call differs. Demonstrates a draggable `Split` with
//! two labelled panes — toggle horizontal/vertical, reset ratio.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::win::run(common::SplitApp::new())
}
