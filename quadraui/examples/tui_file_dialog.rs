//! FileDialogDemo `AppLogic` + `quadraui::tui::run` example.
//!
//! `PlatformServices::show_file_open_dialog` / `show_file_save_dialog`
//! are documented to always return `None` on TUI (apps should provide an
//! in-TUI picker instead) — this demo exercises that contract. See
//! `examples/common/file_dialog_demo.rs` and the paired `gtk_file_dialog`
//! example (#427) for the GTK counterpart that actually shows a native
//! dialog.
//!
//! - `o` open-file dialog (always reports "unsupported")
//! - `s` save-as dialog (always reports "unsupported")
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
