//! `cargo run --example macos_panel --features macos`
//!
//! macOS port of `tui_panel.rs` / `gtk_panel.rs`. Same `PanelApp`
//! `AppLogic` impl in `examples/common/panel_app.rs`; only the runner
//! call differs. Demonstrates the `Panel` title-bar shape (title,
//! close/maximize actions, content area, collapse toggle) reproduced
//! with `SidebarPanel` + `Toolbar`.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::macos::run(common::PanelApp::new())
}
