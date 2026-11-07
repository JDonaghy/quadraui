//! `cargo run --example tui_form_control_width --features tui`
//!
//! Static [`quadraui::Form`] exercising all three [`quadraui::ControlWidth`]
//! modes on the TUI runner. The full `AppLogic` impl lives in
//! `examples/common/form_control_width.rs` — identical app code drives
//! this example AND its GTK twin (`gtk_form_control_width.rs`).
//!
//! Controls:
//! - `q` / `Esc` → quit

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::FormControlWidthApp::new())
}
