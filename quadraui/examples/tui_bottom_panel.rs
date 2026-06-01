//! TUI runner for the bottom-panel demo.
//!
//! Run with:
//!   cargo run --example tui_bottom_panel --features tui
//!
//! Demonstrates:
//! - Two tabs (TERMINAL, PROBLEMS) with per-tab BackendWidget content.
//! - Click a tab to activate it; click `×` on PROBLEMS to close it.
//! - Click the `^` button (top-right of the tab strip) to maximise/restore.
//! - Drag the top edge of the panel to resize it.
//! - Press `q` or Esc to quit.
#[path = "common/mod.rs"]
mod common;

fn main() {
    let app = common::bottom_panel_demo::BottomPanelDemo::new();
    let config = common::bottom_panel_demo::BottomPanelDemo::config();
    quadraui::tui::shell_runner::run_with_shell(app, config);
}
