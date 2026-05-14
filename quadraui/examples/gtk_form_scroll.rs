//! `cargo run --example gtk_form_scroll --features gtk`
//!
//! Form with FormController scroll support on the GTK runner. The full
//! `AppLogic` impl lives in `examples/common/form_scroll.rs` —
//! identical app code drives this example AND its TUI twin
//! (`tui_form_scroll.rs`).
//!
//! Controls:
//! - mouse click on toggle  → flip value
//! - scroll wheel           → scroll form
//! - scrollbar drag         → scroll form
//! - `q` / `Esc`            → quit

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::FormScrollApp::new())
}
