//! `MessageDialogController` demo on the TUI runner.
//!
//! Demonstrates the compose controller **directly** — no
//! `PlatformServices` involved — the same pattern `tui_folder_picker.rs`
//! uses for `FolderPickerController`: keyboard button focus,
//! Enter/Escape/click resolution, all rendered through the existing
//! `Dialog` primitive. Contrast with `tui_message_dialog.rs`
//! (`examples/common/message_dialog_demo.rs`), which exercises
//! `PlatformServices::show_message_dialog` — the nested-loop-driven
//! entry point this controller now powers on TUI (issue #965) — rather
//! than the controller directly. The GTK twin
//! (`gtk_message_dialog_app.rs`) uses the identical `MessageDialogApp`
//! — only the runner differs.
//!
//! ```sh
//! cargo run --example tui_message_dialog_app --features tui
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

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::MessageDialogApp::new())
}
