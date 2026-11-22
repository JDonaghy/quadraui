//! GTK runner for `FrameDemo` — ScreenLayout rendering + rect-based
//! click dispatch proof.
#[path = "common/mod.rs"]
mod common;

fn main() -> Result<(), quadraui::RunError> {
    quadraui::gtk::run(common::FrameDemo::new())
}
