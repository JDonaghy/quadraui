//! TUI horizontal-scroll smoke test.
//!
//! 500-char line editor — press `$` to jump to end, `0` to jump to start.
//!
//! ```sh
//! cargo run --example tui_hscroll --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> Result<(), quadraui::RunError> {
    quadraui::tui::run(common::HScrollEditor::new())
}
