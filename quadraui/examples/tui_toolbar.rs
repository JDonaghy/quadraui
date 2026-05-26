//! Toolbar `AppLogic` + `quadraui::tui::run` example.
//!
//! Exercises the `Toolbar` primitive's full surface: enabled actions
//! with icon + label + key hint, a toggle action that flips `is_active`,
//! a permanently disabled action, a separator, a live `Label` showing
//! state, and hover / pressed highlighting that tracks the mouse.
//!
//! - Click a button         fire its action
//! - 1 / 2 / 3 / 4          keyboard shortcuts for the enabled actions
//! - q / Esc                quit
//!
//! ```sh
//! cargo run --example tui_toolbar --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::ToolbarApp::new())
}
