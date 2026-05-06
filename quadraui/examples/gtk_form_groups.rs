//! `cargo run --example gtk_form_groups --features gtk`
//!
//! Form with ToggleGroup + ButtonRow on the GTK runner. The full
//! `AppLogic` impl lives in `examples/common/form_groups.rs` —
//! identical app code drives this example AND its TUI twin
//! (`tui_form_groups.rs`).
//!
//! Controls:
//! - mouse click on toggle  → flip value
//! - mouse click on button  → log to status bar
//! - `Tab` / `Shift+Tab`    → cycle focused field (via FocusRing)
//! - `q` / `Esc`            → quit

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::FormGroupsApp::new())
}
