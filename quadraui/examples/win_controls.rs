//! `cargo run --example win_controls --features win` (Windows only)
//!
//! Win-GUI port of `tui_controls.rs` / `gtk_controls.rs`. Same
//! `ControlsApp` `AppLogic` impl in `examples/common/controls.rs`;
//! only the runner call differs — `Button`/`RadioGroup`/`Switch`'s own
//! shared pixel painter (`native_surface_paint::paint`) through the
//! widget contract, Direct2D/DirectWrite chrome instead of the TUI
//! fallback painter.
//!
//! Tab to move focus, Space/Enter to activate, Up/Down to move the radio
//! group cursor, `q` to quit.

#[path = "common/mod.rs"]
mod common;

fn main() -> Result<(), quadraui::RunError> {
    quadraui::win::run(common::ControlsApp::new())
}
