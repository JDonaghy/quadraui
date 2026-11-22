//! TUI runner for `FrameDemo` — ScreenLayout rendering + rect-based
//! click dispatch proof.
#[path = "common/mod.rs"]
mod common;

fn main() -> Result<(), quadraui::RunError> {
    quadraui::tui::run(common::FrameDemo::new())
}
