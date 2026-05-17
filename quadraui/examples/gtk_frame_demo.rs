//! GTK runner for `FrameDemo` — ScreenLayout + FrameHitMap proof.
#[path = "common/mod.rs"]
mod common;

fn main() {
    quadraui::gtk::run(common::FrameDemo::new());
}
