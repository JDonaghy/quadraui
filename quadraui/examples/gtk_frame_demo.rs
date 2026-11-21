//! GTK runner for `FrameDemo` — ScreenLayout + FrameHitMap proof.
#[path = "common/mod.rs"]
mod common;

fn main() -> Result<(), quadraui::RunError> {
    quadraui::gtk::run(common::FrameDemo::new())
}
