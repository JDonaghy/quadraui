//! `ActivityStyleDemo` `AppLogic` + `quadraui::tui::run` example.
//!
//! VS-Code-style activity-bar row fill (quadraui#658):
//! `Backend::draw_activity_bar_with_style` paints the active item's row in
//! `ActivityBarStyle::active_bg` instead of the left-edge accent line
//! `tui_activity_nav` demonstrates — `active_accent` stays `None`, so
//! there is zero accent-line rendering. Click an icon (or press
//! `1`/`2`/`3`) to activate it, `q` quits.
//!
//! ```sh
//! cargo run --example tui_activity_style --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::ActivityStyleDemo::new())
}
