//! `cargo run --example tui_demo --features tui`
//!
//! Slightly richer than `tui_app.rs`: paints both a `TabBar` (top)
//! and a `StatusBar` (bottom) and exercises tab navigation +
//! status-segment focus cycling. The whole `AppLogic` impl lives
//! in `examples/common/mod.rs::AppState` so the GTK twin
//! (`gtk_demo.rs`) renders byte-identical app code. The only
//! difference between the two demos is the runner call.
//!
//! Controls:
//! - `←` / `→`           switch active tab
//! - `n`                 open a new tab
//! - `x`                 close the active tab
//! - `Tab` / `Shift-Tab` focus next / previous status segment
//! - `Return`            activate the focused status segment
//! - `q` / `Esc`         quit
//!
//! Resize the terminal narrow + wide while many tabs are open. Active
//! tab stays visible (`TabBar` contract); right status segments drop
//! from the front when the bar gets narrow (`StatusBar` contract).

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::AppState::new())
}
