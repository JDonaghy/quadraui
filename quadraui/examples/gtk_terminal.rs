//! Live terminal emulator — GTK example for the `terminal_engine` primitive.
//!
//! Paired with `tui_terminal.rs` — uses the same [`TerminalApp`] `AppLogic`.
//! See `examples/common/terminal_app.rs` for the backend-agnostic logic.
//!
//! ```sh
//! cargo run --example gtk_terminal --features gtk,terminal
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::TerminalApp::new())
}
