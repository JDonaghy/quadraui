//! FileDialogDemo `AppLogic` + `quadraui::tui::run` example.
//!
//! Since quadraui#965, `PlatformServices::show_file_open_dialog` /
//! `show_file_save_dialog` drive a real in-canvas
//! `compose::FilePickerController` on TUI (a real chosen path, not
//! `None`) — this demo exercises that. `show_folder_open_dialog` is the
//! one #965 deliberately left alone; it's still documented to always
//! return `None` on TUI (apps should provide an in-canvas picker
//! instead). See `examples/common/file_dialog_demo.rs` and the paired
//! `gtk_file_dialog` example (#427, #935) for the GTK counterpart, which
//! shows a native dialog for all three.
//!
//! - `o` open-file dialog (type to filter, Enter confirms a real path)
//! - `s` save-as dialog (Enter confirms the seeded/typed name)
//! - `f` open-folder dialog (always reports "unsupported", quadraui#935)
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
