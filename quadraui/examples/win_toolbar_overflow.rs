//! Win-GUI leg of the toolbar overflow `AppLogic` + `quadraui::win::run`
//! example. Same logic as `tui_toolbar_overflow`/`gtk_toolbar_overflow`,
//! paired by shape — see `examples/common/toolbar_overflow_demo.rs`.
//!
//! Press `m` to cycle Clip -> Wrap -> Menu: `Wrap` flows the overflowing
//! buttons onto additional rows instead of clipping, `Menu` collapses them
//! behind a trailing `[ » ]` affordance — click it to list what was
//! dropped in the status line.
//!
//! ```sh
//! cargo run --example win_toolbar_overflow --features win
//! ```
//! (Windows only — Direct2D/DirectWrite requires a real Windows host.)

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::win::run(common::ToolbarOverflowDemo::new())
}
