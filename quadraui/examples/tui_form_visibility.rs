//! `cargo run --example tui_form_visibility --features tui`
//!
//! Declarative `visible_when` / `enabled_when` field conditions
//! (`quadraui::visibility`) on the TUI runner. The full `AppLogic` impl
//! lives in `examples/common/form_visibility.rs` — identical app code
//! drives this example AND its GTK twin (`gtk_form_visibility.rs`).
//!
//! Controls:
//! - mouse click on a toggle → flip value; visibility/enablement
//!   re-evaluates against the new value
//! - mouse click on Submit  → honored only while "I agree…" is on
//! - `Tab` / `Shift+Tab`    → move focus between visible fields
//! - `q` / `Esc`            → quit

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::FormVisibilityApp::new())
}
