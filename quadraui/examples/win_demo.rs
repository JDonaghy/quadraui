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
//! Supersedes the #19 window-bootstrap-only version of this example now
//! that #25–#30 have landed the chrome-strip rasterisers this app
//! actually paints with (`TabBar`, `StatusBar`). `quadraui::win::run`
//! stays available on every host (`src/win/mod.rs` keeps `mod win`
//! un-target-gated, unlike `macos` — see that module's doc), so this
//! compiles on Linux too under plain `cargo check --features win`; it
//! only opens a real window and *runs* on Windows.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::win::run(common::AppState::new())
}
