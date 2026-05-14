//! `cargo run --example tui_form_scroll --features tui`
//!
//! Form with FormController scroll support on the TUI runner. The full
//! `AppLogic` impl lives in `examples/common/form_scroll.rs` —
//! identical app code drives this example AND its GTK twin
//! (`gtk_form_scroll.rs`).
//!
//! Controls:
//! - mouse click on toggle  → flip value
//! - scroll wheel           → scroll form
//! - scrollbar drag         → scroll form
//! - `q` / `Esc`            → quit

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::FormScrollApp::new())
}
