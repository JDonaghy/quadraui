//! Chart `AppLogic` + `quadraui::tui::run` example.
//!
//! Sparkline, line, and bar chart demo.
//! Press 1/2/3 to switch chart kind, space to add data, q to quit.
//!
//! ```sh
//! cargo run --example tui_chart --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::ChartApp::new())
}
