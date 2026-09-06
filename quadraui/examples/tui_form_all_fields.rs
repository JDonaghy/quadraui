//! `cargo run --example tui_form_all_fields --features tui`
//!
//! Static [`quadraui::Form`] exercising all 14 `FieldKind` variants on
//! the TUI runner (quadraui#808). The full `AppLogic` impl lives in
//! `examples/common/form_all_fields.rs` — identical app code drives
//! this example AND its GTK twin (`gtk_form_all_fields.rs`).
//!
//! Controls:
//! - `q` / `Esc` → quit

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::FormAllFieldsApp::new())
}
