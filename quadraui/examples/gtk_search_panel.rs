//! Search panel spike: MSV + TreeView composition for file-search results.
//!
//! Type in the search input, click results to "jump", click file
//! headers to collapse/expand. Esc blurs input, Ctrl-q quits.
//!
//! ```sh
//! cargo run --example gtk_search_panel --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::SearchPanelApp::new())
}
