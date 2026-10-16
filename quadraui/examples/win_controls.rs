//! `cargo run --example win_controls --features win` (Windows only)
//!
//! Win-GUI port of `tui_controls.rs` / `gtk_controls.rs`. Same
//! `ControlsApp` `AppLogic` impl in `examples/common/controls.rs`;
//! only the runner call differs — real `draw_button`/`draw_radio_group`/
//! `draw_switch` Direct2D/DirectWrite chrome instead of the TUI
//! fallback painter.
//!
//! Tab to move focus, Space/Enter to activate, Up/Down to move the radio
//! group cursor, `q` to quit.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::win::run(common::ControlsApp::new())
}
