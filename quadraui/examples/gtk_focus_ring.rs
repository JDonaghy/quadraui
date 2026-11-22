//! GTK runner for `FocusDemo` — runner-owned `FocusTree` + focus ring.
//!
//! Same `AppLogic` as `tui_focus_ring`, proving the Tab traversal and the
//! ring paint are backend-generic.
#[path = "common/mod.rs"]
mod common;

fn main() -> Result<(), quadraui::RunError> {
    quadraui::gtk::run(common::FocusDemo::new())
}
