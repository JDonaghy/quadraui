//! Button + RadioGroup + Switch `AppLogic` + `quadraui::gtk::run` example.
//!
//! Tab to move focus, Space/Enter to activate, Up/Down to move the radio
//! group cursor, `q` to quit. Pairs [`tui_controls`] — same `ControlsApp`,
//! `Button`/`RadioGroup`/`Switch`'s own shared pixel painter
//! (`native_surface_paint::paint`) through the widget contract, GTK
//! chrome instead of the TUI fallback painter.
//!
//! ```sh
//! cargo run --example gtk_controls --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> Result<(), quadraui::RunError> {
    quadraui::gtk::run(common::ControlsApp::new())
}
