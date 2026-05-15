//! macOS port of `tui_search_panel.rs` / `gtk_search_panel.rs`. Same
//! `SearchPanelApp` `AppLogic` impl in `examples/common/search_panel.rs`;
//! only the runner call differs. MSV + TreeView composition for
//! file-search results.
//!
//! Type in the search input, click results to "jump", click file
//! headers to collapse/expand. Esc blurs input, Ctrl-q quits.
//!
//! ```sh
//! cargo run --example macos_search_panel --features macos
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::macos::run(common::SearchPanelApp::new())
}
