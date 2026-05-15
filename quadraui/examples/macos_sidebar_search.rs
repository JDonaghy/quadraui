//! macOS port of `tui_sidebar_search.rs` / `gtk_sidebar_search.rs`.
//! Same `SidebarSearchApp` `AppLogic` impl in
//! `examples/common/sidebar_search.rs`; only the runner call differs.
//! SidebarSystem search panel — Form (ToggleGroup) + Tree (Header rows).
//!
//! Click individual toggle items, click file headers to collapse,
//! click match rows to select. Status bar shows received events.
//!
//! ```sh
//! cargo run --example macos_sidebar_search --features macos
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::macos::run(common::SidebarSearchApp::new())
}
