//! `cargo run --example gtk_form_control_width --features gtk`
//!
//! Static [`quadraui::Form`] exercising all three [`quadraui::ControlWidth`]
//! modes on the GTK runner. The full `AppLogic` impl lives in
//! `examples/common/form_control_width.rs` — identical app code drives
//! this example AND its TUI twin (`tui_form_control_width.rs`).
//!
//! Controls:
//! - `q` / `Esc` → quit

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::FormControlWidthApp::new())
}
