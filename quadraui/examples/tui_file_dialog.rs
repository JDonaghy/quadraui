//! FileDialogDemo `AppLogic` + `quadraui::tui::run` example.
//!
//! `PlatformServices::show_file_open_dialog` / `show_file_save_dialog` /
//! `show_folder_open_dialog` each drive a real in-canvas
//! `compose::FilePickerController` / `compose::FolderPickerController` on
//! TUI and resolve to a real chosen path, not `None` — this demo exercises
//! all three. See `examples/common/file_dialog_demo.rs` and the paired
//! `gtk_file_dialog` example for the GTK counterpart, which shows a native
//! dialog for all three.
//!
//! - `o` open-file dialog (type to filter, Enter confirms a real path)
//! - `s` save-as dialog (Enter confirms the seeded/typed name)
//! - `f` open-folder dialog (type to filter, Enter confirms a real path)
//! - `q` / `Esc` quits
//!
//! ```sh
//! cargo run --example tui_file_dialog --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::FileDialogDemo::new())
}
