//! `cargo run --example win_multi_tree --features win` (Windows only)
//!
//! Win-GUI port of `msv_multi_tree.rs` / `gtk_multi_tree.rs` /
//! `macos_multi_tree.rs`. Same `DebugSidebar` `AppLogic` impl in
//! `examples/common/multi_tree.rs`; only the runner call differs.
//! Demonstrates the debug-sidebar consumer pattern: a
//! `MultiSectionView` with four `TreeView` sections (the
//! `SidebarSystem` compose helper drives focus, scrolling, and
//! scrollbar drag).
//!
//! Controls:
//! - mouse click on header / body / scrollbar
//! - `Tab` / `Shift+Tab` cycle active section
//! - `↑` / `↓`            scroll active section
//! - `Enter`              select first row of active section
//! - `q` / `Esc`          quit

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::win::run(common::DebugSidebar::new())
}
