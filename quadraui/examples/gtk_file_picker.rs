//! `FilePickerController` demo on the GTK runner.
//!
//! Demonstrates cross-backend file open/save browsing via the compose
//! controller. The TUI twin (`tui_file_picker.rs`) uses the identical
//! `FilePickerApp` — only the runner differs.
//!
//! ```sh
//! cargo run --example gtk_file_picker --features gtk
//! ```
//!
//! Controls (while picker is open):
//! - Type to fuzzy-filter entries (Save mode: also the destination name).
//! - `↑` / `↓` to move selection.
//! - `Enter` on `..` or a directory → navigate.
//! - `Enter` on a file (Open) / with a typed name (Save) → confirm.
//! - `Backspace` → delete last query character.
//! - `Esc` → dismiss picker.
//!
//! Controls (picker dismissed):
//! - `o` → reopen in Open mode.
//! - `s` → reopen in Save mode.
//! - `q` / `Esc` → quit.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::FilePickerApp::new())
}
