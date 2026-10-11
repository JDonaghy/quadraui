//! `cargo run --example win_demo --features win` (Windows only)
//!
//! Win-GUI port of `tui_demo.rs` / `gtk_demo.rs` / `macos_demo.rs`. Same
//! `AppState` `AppLogic` impl in `examples/common/demo.rs`; only the
//! runner call differs. Paints both a `TabBar` (top) and a `StatusBar`
//! (bottom) and exercises tab navigation + status-segment focus cycling.
//!
//! Controls:
//! - `←` / `→`           switch active tab
//! - `n`                 open a new tab
//! - `x`                 close the active tab
//! - `Tab` / `Shift-Tab` focus next / previous status segment
//! - `Return`            activate the focused status segment
//! - `q` / `Esc`         quit
//!
//! `quadraui::win::run` is available on every host and compiles on Linux
//! too under plain `cargo check --features win`, but only opens a real
//! window and runs on Windows.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::win::run(common::AppState::new())
}
