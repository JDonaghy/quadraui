//! DiffView `AppLogic` + `quadraui::gtk::run` example.
//!
//! Demonstrates compute_hunks + DiffView + lock-step scrolling in the GTK
//! backend. Uses two small Rust functions as the diff content.
//!
//! - j / ↓    scroll down
//! - k / ↑    scroll up
//! - Page Down / Page Up  page scroll
//! - m        toggle SideBySide ↔ Unified
//! - q / Esc  quit
//!
//! ```sh
//! cargo run --example gtk_diff_view --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::DiffViewApp::new())
}
