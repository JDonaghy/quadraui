//! `cargo run --example macos_chart --features macos`
//!
//! macOS port of `tui_chart.rs` / `gtk_chart.rs`. Same `ChartApp`
//! `AppLogic` impl in `examples/common/chart_app.rs`; only the runner
//! call differs. Demonstrates sparkline / line / bar chart variants
//! with hover tracking and a vertical crosshair.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::macos::run(common::ChartApp::new())
}
