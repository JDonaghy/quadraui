//! MessageDialogDemo `AppLogic` + `quadraui::tui::run` example.
//!
//! `PlatformServices::show_message_dialog` is documented to always
//! return `None` on TUI (the in-canvas `Dialog` primitive /
//! `Backend::draw_dialog` stays the TUI path) — this demo exercises that
//! contract. See `examples/common/message_dialog_demo.rs` and the paired
//! `gtk_message_dialog` example (#666) for the GTK counterpart that
//! actually shows a native `gtk4::AlertDialog`.
//!
//! - `m` show the "Discard unsaved changes?" message dialog (always
//!   reports "unsupported" on TUI)
//! - `q` / `Esc` quits
//!
//! ```sh
//! cargo run --example tui_message_dialog --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::MessageDialogDemo::new())
}
