//! GTK runner for `HitMapRecoverDemo` — `ScreenLayout::hit_map()` proof.
#[path = "common/mod.rs"]
mod common;

fn main() {
    quadraui::gtk::run(common::HitMapRecoverDemo::new());
}
