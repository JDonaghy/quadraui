//! TUI runner for `FrameDemo` — ScreenLayout + FrameHitMap proof.
#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::FrameDemo::new())
}
