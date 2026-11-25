//! SidebarPanel `AppLogic` + `quadraui::tui::run` example.
//!
//! Smoke-tests a 2-row toolbar header, icon-only / wide-glyph buttons,
//! header clicks routed through the panel's widget contract, and the
//! `SidebarPanel` primitive owning the toolbar-header + content-rect
//! coordination.
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

fn main() -> Result<(), quadraui::RunError> {
    quadraui::tui::run(common::SidebarPanelApp::new())
}
