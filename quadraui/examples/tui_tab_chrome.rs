//! `TabChromeDemo` `AppLogic` + `quadraui::tui::run` example.
//!
//! Active-tab bracket framing (quadraui#631): `Backend::draw_tab_bar_with_chrome`
//! encloses the active tab's label *and* close glyph in `[` / `]`, and
//! clicks resolve through `Backend::tab_bar_layout_with_chrome`'s
//! `close_bounds` — never a hand-rolled scan for `×` in the label string.
//! Click the `×` to close the active tab, click the other tab (or press
//! `tab`) to activate it, `q` quits.
//!
//! ```sh
//! cargo run --example tui_tab_chrome --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::TabChromeDemo::new())
}
