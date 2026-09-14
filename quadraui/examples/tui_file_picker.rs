//! `FilePickerController` demo on the TUI runner.
//!
//! Demonstrates cross-backend file open/save browsing via the compose
//! controller — same fuzzy-filter + keyboard navigation shape as
//! `tui_folder_picker.rs`'s `FolderPickerController`, but for files, with
//! a Save mode where the typed query doubles as the destination
//! filename. The GTK twin (`gtk_file_picker.rs`) uses the identical
//! `FilePickerApp` — only the runner differs.
//!
//! ```sh
//! cargo run --example tui_file_picker --features tui
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

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::FilePickerApp::new())
}
