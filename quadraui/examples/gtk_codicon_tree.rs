//! `CodiconTreeDemo` `AppLogic` + `quadraui::gtk::run` example.
//!
//! A small file-explorer-shaped `TreeView` whose rows carry
//! `Icon::codicon` icons (`"folder"`, `"file-code"`, …), rendered from
//! the bundled codicon font with no backend-specific wiring. `q` quits.
//!
//! ```sh
//! cargo run --example gtk_codicon_tree --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::CodiconTreeDemo::new())
}
