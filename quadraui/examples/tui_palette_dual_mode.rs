//! TUI runner for the dual-mode palette demo.
//!
//! Demonstrates [`quadraui::DualModePaletteController`] — a palette that
//! toggles between:
//!
//! - **List mode** (default): fuzzy-search existing Git branches, `Enter` to switch.
//! - **Input mode**: type a new branch name, `Enter` to create.
//!
//! Press `Tab` to switch modes. The mode badge `[L]` / `[I]` is visible in
//! the palette title bar.
//!
//! Run:
//! ```text
//! cargo run --example tui_palette_dual_mode --features tui
//! ```

#[path = "common/palette_dual_mode_app.rs"]
mod palette_dual_mode_app;

use palette_dual_mode_app::PaletteDualModeApp;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(PaletteDualModeApp::new())
}
