//! TUI DataTable smoke test — Kubernetes pod list.
//!
//! ```sh
//! cargo run --example tui_data_table --features tui
//! ```
//!
//! j/k or ↑/↓ to navigate, s to cycle sort column, d to flip direction,
//! f to toggle the pinned footer/summary row (#432), click header to
//! sort, click row to select, q to quit.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::DataTableApp::new())
}
