//! `cargo run --example tui_form_dropdown --features tui`
//!
//! `FieldKind::Dropdown` embedded in a `Form`, routed through
//! `FormController`, on the TUI runner. The full `AppLogic` impl lives
//! in `examples/common/form_dropdown.rs` — TUI-only for now, no GTK
//! twin yet.
//!
//! Press `Up`/`Down` to move the selection and watch the popup list
//! paint below the field; `q`/`Esc` quits.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::FormDropdownApp::new())
}
