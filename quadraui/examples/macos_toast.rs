//! `cargo run --example macos_toast --features macos`
//!
//! macOS port of `tui_toast.rs` / `gtk_toast.rs`. Same `ToastApp`
//! `AppLogic` impl in `examples/common/toast_app.rs`; only the runner
//! call differs. Demonstrates a `ToastStack` with severity tints,
//! dismiss, and action buttons.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::macos::run(common::ToastApp::new())
}
