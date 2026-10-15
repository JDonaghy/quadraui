//! Button + RadioGroup + Switch `AppLogic` + `quadraui::gtk::run` example.
//!
//! Tab to move focus, Space/Enter to activate, Up/Down to move the radio
//! group cursor, `q` to quit. Pairs [`tui_controls`] — same `ControlsApp`,
//! real `draw_button`/`draw_radio_group`/`draw_switch` GTK chrome instead
//! of the TUI fallback painter.
//!
//! ```sh
//! cargo run --example gtk_controls --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::ControlsApp::new())
}
