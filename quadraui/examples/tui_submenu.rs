//! Cascading submenus in the TUI backend — example driver.
//!
//! Demonstrates pull-right submenus in both a menu-bar dropdown
//! (View → Export → PNG / SVG, with a third nested level on PNG) and
//! an in-window right-click context menu (Refactor → Rename / Extract).
//!
//! Run with:
//!
//! ```sh
//! cargo run --example tui_submenu --features tui
//! ```
//!
//! Controls:
//!   Right-click  — open the in-window context menu
//!   Alt+F/V      — open the menu-bar File / View dropdown
//!   ↑/↓          — navigate within the open level
//!   →/Enter      — open a submenu or activate a leaf
//!   ←/Esc        — close deepest submenu (Esc at root closes all)
//!   q / Esc      — quit (when no menu is open)

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::SubmenuApp::new())
}
