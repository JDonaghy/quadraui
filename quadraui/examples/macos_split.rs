//! `cargo run --example macos_split --features macos`
//!
//! macOS port of `tui_split.rs` / `gtk_split.rs`. Same `SplitApp`
//! `AppLogic` impl in `examples/common/split_app.rs`; only the runner
//! call differs. Demonstrates a draggable `Split` with two labelled
//! panes — toggle horizontal/vertical, reset ratio.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::macos::run(common::SplitApp::new())
}
