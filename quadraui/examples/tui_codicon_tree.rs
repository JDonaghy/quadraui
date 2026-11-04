//! `CodiconTreeDemo` `AppLogic` + `quadraui::tui::run` example.
//!
//! A small file-explorer-shaped `TreeView` whose rows carry
//! `Icon::codicon` icons (`"folder"`, `"file-code"`, …). `n` toggles
//! nerd fonts to flip between the bundled codicon glyph and each icon's
//! plain-text fallback; `q` quits.
//!
//! ```sh
//! cargo run --example tui_codicon_tree --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::CodiconTreeDemo::new())
}
