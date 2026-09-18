//! `CommandLine` selection-highlight demo, GTK backend (issue #1001).
//!
//! Same shape as `examples/tui_command_line_selection.rs` but rendered in
//! a GTK window. The `CommandLineSelectionDemo` state and `AppLogic` impl
//! live in `examples/common/command_line_selection_demo.rs` — the only
//! difference between this file and the TUI twin is the runner call.
//!
//! Run with:
//!
//! ```sh
//! cargo run --example gtk_command_line_selection --features gtk
//! ```
//!
//! Press `s` to toggle the selection highlight on `:select-me`; `q` or
//! Esc to quit.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::CommandLineSelectionDemo::new())
}
