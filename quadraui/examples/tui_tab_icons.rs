//! `TabIconsDemo` `AppLogic` + `quadraui::tui::run` example.
//!
//! A `TabBar` whose tabs carry per-tab [`quadraui::TabIcon`] glyphs,
//! supplied as a sidecar list on the bar's own `TabBar::icons` field.
//! `i` toggles the sidecar on/off — the labels shift by exactly the icon
//! reservation and nothing else moves.
//! `tab` cycles the active tab, `q` quits.
//!
//! ```sh
//! cargo run --example tui_tab_icons --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> Result<(), quadraui::RunError> {
    quadraui::tui::run(common::TabIconsDemo::new())
}
