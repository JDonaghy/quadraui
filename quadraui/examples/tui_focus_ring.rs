//! TUI runner for `FocusDemo` — runner-owned `FocusManager` + focus ring (#830).
//!
//! Tab / Shift+Tab cycle focus through the two lists and the status bar;
//! the ring follows, and the status bar echoes `UiEvent::FocusChanged`.
#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::FocusDemo::new())
}
