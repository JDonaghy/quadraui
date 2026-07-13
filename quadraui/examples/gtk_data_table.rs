//! GTK DataTable smoke test — Kubernetes pod list.
//!
//! ```sh
//! cargo run --example gtk_data_table --features gtk
//! ```
//!
//! j/k or ↑/↓ to navigate, s to cycle sort column, d to flip direction,
//! f to toggle the pinned footer/summary row (#432), click header to
//! sort, click row to select, q to quit.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::DataTableApp::new())
}
