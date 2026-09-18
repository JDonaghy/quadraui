//! `CommandLine` selection-highlight demo, TUI backend (issue #1001).
//!
//! See `examples/common/command_line_selection_demo.rs` for the shared
//! `AppLogic`. See `gtk_command_line_selection.rs` for the GTK twin.
//!
//! Run with:
//!
//! ```sh
//! cargo run --example tui_command_line_selection --features tui
//! ```
//!
//! Press `s` to toggle the selection highlight on `:select-me`; `q` or
//! Esc to quit.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::CommandLineSelectionDemo::new())
}
