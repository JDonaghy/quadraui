//! `TabChromeDemo` `AppLogic` + `quadraui::tui::run` example.
//!
//! Active-tab bracket framing: the bar's own `TabBar::chrome` field
//! encloses the active tab's label *and* close glyph in `[` / `]`, and
//! clicks resolve through the painted `TabBarLayout`'s `hit_test` — never
//! a hand-rolled scan for `×` in the label string.
//! Click the `×` to close the active tab, click the other tab (or press
//! `tab`) to activate it, `q` quits.
//!
//! ```sh
//! cargo run --example tui_tab_chrome --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> Result<(), quadraui::RunError> {
    quadraui::tui::run(common::TabChromeDemo::new())
}
