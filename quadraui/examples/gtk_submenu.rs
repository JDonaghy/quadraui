//! Cascading submenus in the GTK backend — example driver.
//!
//! GTK twin of `tui_submenu.rs` (#370). Same `SubmenuApp` — pull-right
//! submenus in both a menu-bar dropdown (View → Export → PNG → {Lossless,
//! Compressed} / SVG, ≥2 levels) and an in-window right-click context menu
//! (Refactor → Rename / Extract) — driven entirely through the
//! backend-agnostic `Backend` trait, so no GTK-specific app code is needed
//! (#371: bringing the GTK rasteriser to submenu-painting parity with TUI
//! is what makes this example behave the same as its TUI twin).
//!
//! Run with:
//!
//! ```sh
//! cargo run --example gtk_submenu --features gtk
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

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::SubmenuApp::new())
}
