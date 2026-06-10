//! `TabGroupController` demo (TUI backend).
//!
//! Two split panes, each with its own tab bar. Demonstrates tab
//! switching, closing, new-tab, pane focus, divider dragging, and the
//! full cross-pane tab drag-and-drop suite (reorder within a strip,
//! merge into another pane, split off a new adjacent pane).
//!
//! ```sh
//! cargo run --example tui_tabgroup --features tui
//! ```
//!
//! Controls:
//! - click tab label              — activate that tab
//! - click `×`                   — close the tab (last tab collapses the pane)
//! - click `+`                   — add an untitled tab
//! - click content area          — focus that pane
//! - drag divider                — resize panes
//! - drag tab onto another pane  — merge tab into that pane
//! - drag tab to a pane's edge   — split off a new adjacent pane
//! - drag tab within its strip   — reorder tabs in the same pane
//! - Tab / Shift+Tab             — cycle focus
//! - q / Esc                     — quit

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::TabGroupDemo::new())
}
