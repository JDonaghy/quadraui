//! TUI runner for `HitMapRecoverDemo` — `ScreenLayout::hit_map()` proof (#425).
#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::HitMapRecoverDemo::new())
}
