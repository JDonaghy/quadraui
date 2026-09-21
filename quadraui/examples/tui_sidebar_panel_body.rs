//! `SidebarPanelBody` (#1041) `AppLogic` + `quadraui::tui::run` example.
//! See `examples/common/sidebar_panel_body_demo.rs` for the consumer
//! pattern.
//!
//! - `↑` / `↓`     scroll the list
//! - `c`           cycle chrome: none → header → header+search → none
//! - `/`           toggle search-input focus (header+search mode only)
//! - typed chars / `Backspace` — edit the search query while focused
//! - `q` / `Esc`   quit
//!
//! ```sh
//! cargo run --example tui_sidebar_panel_body --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::SidebarPanelBodyDemo::new())
}
