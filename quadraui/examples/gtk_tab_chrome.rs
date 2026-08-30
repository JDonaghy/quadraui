//! `TabChromeDemo` `AppLogic` + `quadraui::gtk::run` example.
//!
//! GTK twin of `tui_tab_chrome` — the same `AppLogic`, so the bracket
//! framing (quadraui#631) is exercised through the Pango/Cairo rasteriser
//! instead of the cell grid. Click the `×` to close the active tab, click
//! the other tab (or press `tab`) to activate it, `q` quits.
//!
//! ```sh
//! cargo run --example gtk_tab_chrome --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::TabChromeDemo::new())
}
