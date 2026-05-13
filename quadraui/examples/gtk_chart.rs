//! Chart `AppLogic` + `quadraui::gtk::run` example.
//!
//! Sparkline, line, and bar chart demo.
//! Press 1/2/3 to switch chart kind, space to add data, q to quit.
//!
//! ```sh
//! cargo run --example gtk_chart --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::ChartApp::new())
}
