//! GTK runner for `FocusDemo` — runner-owned `FocusManager` + focus ring (#830).
//!
//! Same `AppLogic` as `tui_focus_ring`, proving the Tab traversal and the
//! ring paint are backend-generic.
#[path = "common/mod.rs"]
mod common;

fn main() {
    quadraui::gtk::run(common::FocusDemo::new());
}
