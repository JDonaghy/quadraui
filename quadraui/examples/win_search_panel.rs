//! Win-GUI port of `tui_search_panel.rs` / `gtk_search_panel.rs` /
//! `macos_search_panel.rs`. Same `SearchPanelApp` `AppLogic` impl in
//! `examples/common/search_panel.rs`; only the runner call differs.
//! MSV + TreeView composition for file-search results.
//!
//! Type in the search input, click results to "jump", click file
//! headers to collapse/expand. Esc blurs input, Ctrl-q quits.
//!
//! ```sh
//! cargo run --example win_search_panel --features win
//! ```
//! (Windows only.)

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::win::run(common::SearchPanelApp::new())
}
