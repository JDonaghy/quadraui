//! `cargo run --example macos_demo --features macos`
//!
//! macOS port of `tui_demo.rs` / `gtk_demo.rs`. Paints both a `TabBar`
//! (top) and a `StatusBar` (bottom) and exercises tab navigation +
//! status-segment focus cycling. The whole `AppLogic` impl lives in
//! `examples/common/mod.rs::AppState` so the TUI and GTK twins render
//! byte-identical app code — the only difference between them is the
//! runner call.
//!
//! Controls:
//! - `←` / `→`           switch active tab
//! - `n`                 open a new tab
//! - `x`                 close the active tab
//! - `Tab` / `Shift-Tab` focus next / previous status segment
//! - `Return`            activate the focused status segment
//! - `q` / `Esc`         quit

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::macos::run(common::AppState::new())
}
