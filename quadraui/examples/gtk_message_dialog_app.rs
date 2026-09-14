//! `MessageDialogController` demo on the GTK runner.
//!
//! Demonstrates the compose controller **directly** — no
//! `PlatformServices` involved. Contrast with `gtk_message_dialog.rs`
//! (`examples/common/message_dialog_demo.rs`), which opens GTK's native
//! `gtk4::AlertDialog` via `PlatformServices::show_message_dialog`. The
//! TUI twin (`tui_message_dialog_app.rs`) uses the identical
//! `MessageDialogApp` — only the runner differs.
//!
//! ```sh
//! cargo run --example gtk_message_dialog_app --features gtk
//! ```
//!
//! Controls (while dialog is open):
//! - `←` / `→` / `Tab` / `Shift+Tab` — move keyboard focus between buttons.
//! - `Enter` — activate the focused button.
//! - `Esc` — resolves to the cancel button.
//! - Click a button — resolves it directly.
//!
//! Controls (after resolve):
//! - `o` → reopen the dialog.
//! - `q` / `Esc` → quit.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::MessageDialogApp::new())
}
