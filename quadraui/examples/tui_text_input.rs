//! TUI runner for the TextInput demo (#222 smoke test).
//!
//! Run with:
//!
//! ```sh
//! cargo run --example tui_text_input --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::text_input_demo::TextInputDemo::new())
}
