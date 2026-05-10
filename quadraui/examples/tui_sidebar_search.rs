//! SidebarSystem search panel — Form (ToggleGroup) + Tree (Header rows).
//!
//! Click individual toggle items, click file headers to collapse,
//! click match rows to select. Status bar shows received events.
//!
//! ```sh
//! cargo run --example tui_sidebar_search --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::SidebarSearchApp::new())
}
