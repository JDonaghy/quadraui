//! `cargo run --example gtk_form_visibility --features gtk`
//!
//! Declarative `visible_when` / `enabled_when` field conditions
//! (`quadraui::visibility`) on the GTK runner. The full `AppLogic` impl
//! lives in `examples/common/form_visibility.rs` — identical app code
//! drives this example AND its TUI twin (`tui_form_visibility.rs`).
//!
//! Controls:
//! - mouse click on a toggle → flip value; visibility/enablement
//!   re-evaluates against the new value
//! - mouse click on Submit  → honored only while "I agree…" is on
//! - `Tab` / `Shift+Tab`    → move focus between visible fields
//! - `q` / `Esc`            → quit

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::FormVisibilityApp::new())
}
