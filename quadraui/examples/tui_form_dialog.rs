//! `cargo run --example tui_form_dialog --features tui`
//!
//! Renders a `Dialog` hosting a multi-field `Form` (Name + Description)
//! via `DialogInput::Form`, driven through `FormDialogController`.
//!
//! Controls:
//! - `Tab` / `Shift+Tab`  move focus across the fields and buttons
//! - typing                edit the focused field
//! - `Enter`               commit text / activate the focused button
//! - `Esc`                 cancel and quit
//! - `q` (no field focused) quit

#[path = "common/form_dialog_demo.rs"]
mod form_dialog_demo;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(form_dialog_demo::FormDialogApp::new())
}
