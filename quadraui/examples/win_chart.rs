//! `cargo run --example win_chart --features win` (Windows only)
//!
//! Win-GUI port of `tui_chart.rs` / `gtk_chart.rs` / `macos_chart.rs`.
//! Same `ChartApp` `AppLogic` impl in `examples/common/chart_app.rs`;
//! only the runner call differs. Demonstrates sparkline / line / bar
//! chart variants with hover tracking and a vertical crosshair.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::win::run(common::ChartApp::new())
}
