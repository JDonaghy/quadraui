//! `cargo run --example macos_form_groups --features macos`
//!
//! macOS port of `tui_form_groups.rs` / `gtk_form_groups.rs`. Same
//! `FormGroupsApp` `AppLogic` impl in `examples/common/form_groups.rs`;
//! only the runner call differs. Demonstrates a `Form` with mixed
//! field kinds (Label, Toggle, TextInput, Button) and Tab focus
//! cycling.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::macos::run(common::FormGroupsApp::new())
}
