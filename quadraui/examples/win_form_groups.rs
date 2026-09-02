//! `cargo run --example win_form_groups --features win` (Windows only)
//!
//! Win-GUI port of `tui_form_groups.rs` / `gtk_form_groups.rs` /
//! `macos_form_groups.rs`. Same `FormGroupsApp` `AppLogic` impl in
//! `examples/common/form_groups.rs`; only the runner call differs.
//! Demonstrates a `Form` with mixed field kinds (Label, Toggle,
//! TextInput, Button) and Tab focus cycling.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::win::run(common::FormGroupsApp::new())
}
