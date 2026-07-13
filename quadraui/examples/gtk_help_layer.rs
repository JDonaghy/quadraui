//! GTK runner for the context-sensitive help layer demo (#431).
//!
//! Same `HelpLayerDemo` `ShellApp` as `tui_help_layer` — proves the help
//! registry + cheatsheet overlay + palette integration render on GTK with
//! no per-backend code (composed entirely from `Panel`, `TextDisplay`, and
//! `Palette`, all of which already have GTK rasterisers).
//!
//! ```sh
//! cargo run --example gtk_help_layer --features gtk
//! ```
#[path = "common/mod.rs"]
mod common;

fn main() {
    let app = common::help_layer_demo::HelpLayerDemo::new();
    let config = common::help_layer_demo::HelpLayerDemo::config();
    quadraui::gtk::shell_runner::run_with_shell(app, config);
}
