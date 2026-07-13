//! TUI runner for the context-sensitive help layer demo (#431).
//!
//! `?` opens a cheatsheet for the active panel, `p` opens a command
//! palette populated from the same registered actions.
//!
//! ```sh
//! cargo run --example tui_help_layer --features tui
//! ```
#[path = "common/mod.rs"]
mod common;

fn main() {
    let app = common::help_layer_demo::HelpLayerDemo::new();
    let config = common::help_layer_demo::HelpLayerDemo::config();
    quadraui::tui::shell_runner::run_with_shell(app, config);
}
