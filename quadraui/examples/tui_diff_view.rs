//! DiffView `AppLogic` + `quadraui::tui::run` example.
//!
//! Demonstrates compute_hunks + DiffView + lock-step scrolling in the TUI
//! backend. Uses two small Rust functions as the diff content.
//!
//! - j / ↓    scroll down
//! - k / ↑    scroll up
//! - Page Down / Page Up  page scroll
//! - m        toggle SideBySide ↔ Unified
//! - q / Esc  quit
//!
//! ```sh
//! cargo run --example tui_diff_view --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::DiffViewApp::new())
}
