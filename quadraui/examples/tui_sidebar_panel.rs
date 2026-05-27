//! SidebarPanel `AppLogic` + `quadraui::tui::run` example.
//!
//! Smoke-tests every #259 gap end-to-end: 2-row toolbar (Gap 1),
//! icon-only / wide-glyph buttons (Gap 4), `ToolbarHoverTracker`
//! managing hover state (Gap 3), and the `SidebarPanel` primitive
//! itself owning the toolbar-header + content-rect coordination
//! (Gap 2).
//!
//! - Click `+`              add a task
//! - Click `↻`              clear last action message
//! - Click `Filter` / `f`   toggle the filter
//! - Click `Clear` / `c`    empty the task list
//! - Click a task row       select it
//! - q / Esc                quit
//!
//! ```sh
//! cargo run --example tui_sidebar_panel --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::SidebarPanelApp::new())
}
