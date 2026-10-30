//! `cargo run --example tui_form_combo_box --features tui`
//!
//! `FieldKind::ComboBox` embedded in a `Form`, routed through
//! `FormController`, on the TUI runner. The full `AppLogic` impl lives
//! in `examples/common/form_combo_box.rs` — TUI-only for now, no GTK
//! twin yet.
//!
//! Controls:
//! - type any letter → filter the dropdown, opens it
//! - `Up` / `Down`    → move the highlighted option
//! - `Enter`          → commit the highlighted (or typed) value
//! - `Esc`            → cancel the edit (or quit, once closed)
//! - `q`              → quit (once the dropdown is closed)

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::FormComboBoxApp::new())
}
