//! `TabIconsDemo` `AppLogic` + `quadraui::gtk::run` example.
//!
//! GTK twin of `tui_tab_icons` — the same `AppLogic`, so the icon
//! sidecar (quadraui#620) is exercised through the Pango/Cairo
//! rasteriser instead of the cell grid. `i` toggles the sidecar,
//! `tab` cycles the active tab, `q` quits.
//!
//! ```sh
//! cargo run --example gtk_tab_icons --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::TabIconsDemo::new())
}
