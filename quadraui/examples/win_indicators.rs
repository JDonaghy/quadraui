//! `cargo run --example win_indicators --features win` (Windows only)
//!
//! Win-GUI port of `tui_indicators.rs` / `gtk_indicators.rs` /
//! `macos_indicators.rs`. Same `IndicatorsApp` `AppLogic` impl in
//! `examples/common/indicators_app.rs`; only the runner call differs.
//! Demonstrates `ProgressBar` + `Spinner` — determinate /
//! indeterminate, cancel.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::win::run(common::IndicatorsApp::new())
}
