//! `cargo run --example gtk_demo --features gtk`
//!
//! Slightly richer than `gtk_app.rs`: paints both a `TabBar` (top)
//! and a `StatusBar` (bottom) and exercises tab navigation +
//! status-segment focus cycling. The whole `AppLogic` impl lives
//! in `examples/common/mod.rs::AppState` so the TUI twin
//! (`tui_demo.rs`) renders byte-identical app code. The only
//! difference between the two demos is the runner call.
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
    quadraui::gtk::run(common::AppState::new())
}
