//! Win-GUI port of `tui_sidebar_search.rs` / `gtk_sidebar_search.rs` /
//! `macos_sidebar_search.rs`. Same `SidebarSearchApp` `AppLogic` impl
//! in `examples/common/sidebar_search.rs`; only the runner call
//! differs. SidebarSystem search panel — Form (ToggleGroup) + Tree
//! (Header rows).
//!
//! Click individual toggle items, click file headers to collapse,
//! click match rows to select. Status bar shows received events.
//!
//! ```sh
//! cargo run --example win_sidebar_search --features win
//! ```
//! (Windows only.)

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::win::run(common::SidebarSearchApp::new())
}
