//! TUI runner for the `CellGridController` spreadsheet demo — a virtual
//! 100,000×200 sheet.
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
//! - Typing a character or pressing F2 opens an inline editor over the
//!   cursor cell; Enter/Tab/the arrow keys commit and move, Esc cancels.
//!   A leading `=` sums `+`-separated integers (e.g. `=1+2` → `3`).
//! - Click the formula bar (just above the grid) to open the same edit,
//!   seeded with the cursor cell's text, cursor at its end.
//! - Click a sheet tab to switch to it — each tab is its own
//!   independent sheet with its own cursor, selection, scroll position
//!   and edits.
//! - The status bar reports the selection in A1 notation, plus the sum
//!   of its numeric cells when the selection is small enough to be
//!   worth adding up every frame.
//! - Press `q` or Esc to quit (Esc cancels an edit first, if one is
//!   active).
#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::CellGridDemo::new())
}
