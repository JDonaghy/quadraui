//! `ChatController` demo on the GTK runner.
//!
//! Demonstrates a self-contained chat overlay backed by `ChatController`:
//! scrollable transcript, alternating role colouring, multi-line input with
//! history navigation, spinner while "thinking", and a status strip. The TUI
//! twin (`tui_chat.rs`) uses the identical `ChatDemo` — only the runner
//! differs.
//!
//! ```sh
//! cargo run --example gtk_chat --features gtk
//! ```
//!
//! Controls:
//! - Type any text and press `Enter` for newlines.
//! - `Ctrl+Enter` — submit the message.
//! - `↑` / `↓` — history navigation (when cursor is on the first/last line).
//! - `PageUp` / `PageDown` — scroll the transcript.
//! - `Esc` — clear the input, or quit when the input is already empty.
//! - `q` / `Ctrl+C` — quit immediately.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::ChatDemo::new())
}
