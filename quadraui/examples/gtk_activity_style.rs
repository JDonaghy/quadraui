//! `ActivityStyleDemo` `AppLogic` + `quadraui::gtk::run` example.
//!
//! GTK twin of `tui_activity_style` — the same `AppLogic`, so the
//! VS-Code-style row fill (quadraui#658) is exercised through the
//! Pango/Cairo rasteriser instead of the cell grid. Click an icon (or
//! press `1`/`2`/`3`) to activate it, `q` quits.
//!
//! ```sh
//! cargo run --example gtk_activity_style --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::ActivityStyleDemo::new())
}
