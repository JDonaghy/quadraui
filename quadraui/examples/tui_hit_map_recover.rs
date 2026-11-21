//! TUI runner for `HitMapRecoverDemo` — `ScreenLayout::hit_map()` proof.
#[path = "common/mod.rs"]
mod common;

fn main() -> Result<(), quadraui::RunError> {
    quadraui::tui::run(common::HitMapRecoverDemo::new())
}
