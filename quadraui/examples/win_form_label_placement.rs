//! `cargo run --example win_form_label_placement --features win` (Windows only)
//!
//! Win-GUI port of `tui_form_label_placement.rs` /
//! `macos_form_label_placement.rs` / `gtk_form_label_placement.rs`. Same
//! `FormLabelPlacementApp` `AppLogic` impl in
//! `examples/common/form_label_placement.rs`; only the runner call differs.
//! Demonstrates per-field `label_position` / `label_align` / `label_width`
//! and a custom `tab_index` order.
//!
//! Controls:
//! - `Tab` / `Shift+Tab`    → move focus in `tab_index` order
//! - typing (focused field) → edit the focused text field
//! - `Enter` (focused field) → commit the focused text field
//! - `q` / `Esc`            → quit

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::win::run(common::FormLabelPlacementApp::new())
}
