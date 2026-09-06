//! `cargo run --example gtk_form_all_fields --features gtk`
//!
//! Static [`quadraui::Form`] exercising all 14 `FieldKind` variants on
//! the GTK runner (quadraui#808). The full `AppLogic` impl lives in
//! `examples/common/form_all_fields.rs` — identical app code drives
//! this example AND its TUI twin (`tui_form_all_fields.rs`).
//!
//! Controls:
//! - `q` / `Esc` → quit

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::FormAllFieldsApp::new())
}
