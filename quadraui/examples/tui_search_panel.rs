//! Search panel spike: MSV + TreeView composition for file-search results.
//!
//! Type in the search input, click results to "jump", click file
//! headers to collapse/expand. Esc blurs input, Ctrl-q quits.
//!
//! ```sh
//! cargo run --example tui_search_panel --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::SearchPanelApp::new())
}
