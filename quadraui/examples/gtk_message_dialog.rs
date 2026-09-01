//! MessageDialogDemo `AppLogic` + `quadraui::gtk::run` example.
//!
//! Manual smoke test for the GTK `PlatformServices::show_message_dialog`
//! implementation (#666): opens a real native `gtk4::AlertDialog`,
//! parented to this window, pumped through the same nested-mainloop
//! adapter `gtk_file_dialog` uses. See
//! `examples/common/message_dialog_demo.rs` for details, and
//! `docs/TESTING.md`'s "What unit tests don't cover" for why this has to
//! be a manual smoke rather than an automated one (`GtkDriver` paints
//! Cairo offscreen — it never opens a native window, so it structurally
//! cannot see an `AlertDialog`).
//!
//! - `m` show the "Discard unsaved changes?" message dialog
//! - `q` / `Esc` quits
//!
//! ```sh
//! cargo run --example gtk_message_dialog --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::MessageDialogDemo::new())
}
