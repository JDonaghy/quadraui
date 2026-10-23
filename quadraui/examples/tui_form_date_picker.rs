//! `cargo run --example tui_form_date_picker --features tui`
//!
//! `FieldKind::DatePicker` embedded in a `Form`, routed through
//! `FormController`, on the TUI runner. The full `AppLogic` impl lives
//! in `examples/common/form_date_picker.rs` — identical app code drives
//! this example AND its GTK twin (`gtk_form_date_picker.rs`).
//!
//! Controls:
//! - arrows / PageUp / PageDown → move the focused day
//! - `Enter`                    → confirm the focused day
//! - `q` / `Esc`                → quit

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::FormDatePickerApp::new())
}
