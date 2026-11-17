//! `Toolbar` overflow `AppLogic` + `quadraui::tui::run` example.
//!
//! Exercises `ToolbarLayoutOptions::with_overflow` on a bar deliberately
//! narrower than its five buttons' combined width — press `m` to cycle
//! Clip -> Wrap -> Menu and watch the overflow buttons clip, flow onto
//! extra rows, or collapse behind a `[ » ]` affordance.
//!
//! - m                           cycle Clip -> Wrap -> Menu
//! - Click a button              report it in the status line
//! - Click `[ » ]` (Menu mode)   list the dropped items in the status line
//! - q / Esc                     quit
//!
//! ```sh
//! cargo run --example tui_toolbar_overflow --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::ToolbarOverflowDemo::new())
}
