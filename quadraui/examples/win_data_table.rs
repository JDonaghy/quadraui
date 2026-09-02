//! `cargo run --example win_data_table --features win` (Windows only)
//!
//! Win-GUI port of `tui_data_table.rs` / `gtk_data_table.rs` /
//! `macos_data_table.rs`. Same `DataTableApp` `AppLogic` impl in
//! `examples/common/data_table_app.rs`; only the runner call differs.
//! Demonstrates a sortable Kubernetes pod list with column headers,
//! hover row tint, and row selection.
//!
//! j/k or ↑/↓ to navigate, s to cycle sort column, d to flip direction,
//! f to toggle the pinned footer/summary row, click header to sort,
//! click row to select, q to quit.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::win::run(common::DataTableApp::new())
}
