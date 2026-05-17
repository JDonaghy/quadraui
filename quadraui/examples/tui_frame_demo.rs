//! TUI runner for `FrameDemo` — ScreenLayout + FrameHitMap proof.
#[path = "common/mod.rs"]
mod common;

fn main() {
    quadraui::tui::run(common::FrameDemo::new());
}
