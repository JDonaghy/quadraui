//! TUI runner for the `CellGridController` spreadsheet demo.
//!
//! Run with:
//!   cargo run --example tui_cell_grid --features tui
//!
//! Demonstrates:
//! - Arrows / Tab / Shift+Tab / Enter / Shift+Enter move the cursor.
//! - Page Up/Down page by the visible row count; Home/End jump within
//!   the row; Ctrl+Home / Ctrl+End jump to the sheet corners.
//! - Ctrl+Arrow jumps to a sheet edge; add Shift to any of the above to
//!   extend the selection instead of moving it.
//! - Click a cell to move the cursor, drag to select a rectangle,
//!   Shift+click to extend from the cursor.
//! - Click a column letter or row number to select that whole
//!   column/row; click the top-left corner (or press Ctrl+A) to select
//!   everything.
//! - The frozen header row and region column stay put as the body
//!   scrolls to follow the cursor.
//! - Press `q` or Esc to quit.
#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::CellGridDemo::new())
}
