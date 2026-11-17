//! Toolbar overflow `AppLogic` + `quadraui::gtk::run` example. Same logic
//! as `tui_toolbar_overflow`, paired by shape — see
//! `examples/common/toolbar_overflow_demo.rs`.
//!
//! Press `m` to cycle Clip -> Wrap -> Menu: `Wrap` flows the overflowing
//! buttons onto additional rows instead of clipping, `Menu` collapses them
//! behind a trailing `[ » ]` affordance — click it to list what was
//! dropped in the status line.
//!
//! ```sh
//! cargo run --example gtk_toolbar_overflow --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::ToolbarOverflowDemo::new())
}
