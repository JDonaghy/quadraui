//! TUI runner for `FrameDemo` — ScreenLayout + FrameHitMap proof.
#[path = "common/mod.rs"]
mod common;

fn main() -> Result<(), quadraui::RunError> {
    quadraui::tui::run(common::FrameDemo::new())
}
