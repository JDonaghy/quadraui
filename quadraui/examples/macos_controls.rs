//! `cargo run --example macos_controls --features macos`
//!
//! macOS port of `tui_controls.rs` / `gtk_controls.rs` / `win_controls.rs`.
//! Same `ControlsApp` `AppLogic` impl in `examples/common/controls.rs`;
//! only the runner call differs — real `draw_button` CoreGraphics/CoreText
//! chrome for `Button` instead of the TUI fallback painter, and
//! `RadioGroup`/`Switch`'s own shared pixel painter
//! (`native_surface_paint::paint`) through the widget contract either
//! way. Demonstrates every `ButtonVariant` (`Primary`, `Secondary`,
//! `Danger`, `Warning`, `Icon`), the leading icon toggle (`i`), the
//! Nerd Font/fallback toggle (`n`), and pointer hover/press tinting.
//!
//! Tab to move focus, Space/Enter to activate, Up/Down to move the radio
//! group cursor, `q` to quit.

#[path = "common/mod.rs"]
mod common;

fn main() -> Result<(), quadraui::RunError> {
    quadraui::macos::run(common::ControlsApp::new())
}
