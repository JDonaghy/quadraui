//! `cargo run --example tui_form_groups --features tui`
//!
//! Form with ToggleGroup + ButtonRow on the TUI runner. The full
//! `AppLogic` impl lives in `examples/common/form_groups.rs` —
//! identical app code drives this example AND its GTK twin
//! (`gtk_form_groups.rs`).
//!
//! Controls:
//! - mouse click on toggle  → flip value
//! - mouse click on button  → log to status bar
//! - `Tab`                  → cycle focused field
//! - `q` / `Esc`            → quit

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::FormGroupsApp::new())
}
