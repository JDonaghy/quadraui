//! Live terminal emulator — TUI example for the `terminal_engine` primitive.
//!
//! Spawns an interactive shell (reads `$SHELL`, falls back to `/bin/bash`)
//! and renders its output through [`quadraui::terminal_engine::TerminalSession`]
//! into the standard [`quadraui::Terminal`] cell-grid primitive, painted by
//! the existing [`quadraui::tui::draw_terminal`] rasteriser.
//!
//! This example is the acceptance smoke-test for issue #279.
//!
//! # Controls
//!
//! | Key             | Action                                 |
//! |-----------------|----------------------------------------|
//! | Ctrl+Q          | Quit                                   |
//! | Any printable   | Forward to shell                       |
//! | Arrow keys      | Forward (or navigate history with Scroll) |
//! | Scroll wheel    | Scroll into history / back to live     |
//! | Ctrl+C / Ctrl+D | Forward terminal-control bytes         |
//! | Bracketed paste | Forward verbatim                       |
//!
//! ```sh
//! cargo run --example tui_terminal --features tui,terminal
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::TerminalApp::new())
}
