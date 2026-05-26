//! `FolderPickerController` demo on the TUI runner.
//!
//! Demonstrates cross-backend directory browsing via the compose
//! controller: filesystem walking, fuzzy filtering, keyboard navigation,
//! and scroll management — all rendered through the existing `Palette`
//! primitive. The GTK twin (`gtk_folder_picker.rs`) uses the identical
//! `FolderPickerApp` — only the runner differs.
//!
//! ```sh
//! cargo run --example tui_folder_picker --features tui
//! ```
//!
//! Controls (while picker is open):
//! - Type to fuzzy-filter entries.
//! - `↑` / `k` and `↓` / `j` to move selection.
//! - `-` or `Enter` on `..` → navigate up.
//! - `Enter` on any other entry → confirm path.
//! - `Backspace` → delete last query character.
//! - `Esc` → dismiss picker.
//!
//! Controls (picker dismissed):
//! - `o` → reopen picker.
//! - `q` / `Esc` → quit.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::FolderPickerApp::new())
}
