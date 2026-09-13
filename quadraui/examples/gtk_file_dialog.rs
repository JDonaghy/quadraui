//! FileDialogDemo `AppLogic` + `quadraui::gtk::run` example.
//!
//! Manual smoke test for the GTK `PlatformServices::show_file_open_dialog`
//! / `show_file_save_dialog` (#427) / `show_folder_open_dialog` (#935)
//! implementations: opens a real native `gtk4::FileDialog`, parented to
//! this window, pumped through a nested-mainloop adapter so the trait's
//! synchronous signature still holds. See
//! `examples/common/file_dialog_demo.rs` for details.
//!
//! - `o` open-file dialog (filtered to `*.rs`)
//! - `s` save-as dialog (initial name `untitled.txt`)
//! - `f` open-folder dialog (quadraui#935)
//! - `q` / `Esc` quits
//!
//! ```sh
//! cargo run --example gtk_file_dialog --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::FileDialogDemo::new())
}
