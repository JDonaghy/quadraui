//! `cargo run --example macos_indicators --features macos`
//!
//! macOS port of `tui_indicators.rs` / `gtk_indicators.rs`. Same
//! `IndicatorsApp` `AppLogic` impl in `examples/common/indicators_app.rs`;
//! only the runner call differs. Demonstrates `ProgressBar` +
//! `Spinner` — determinate / indeterminate, cancel.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::macos::run(common::IndicatorsApp::new())
}
