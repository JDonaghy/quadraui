//! TUI runner for the caret-shape demo (issue #1015).
//!
//! `i`/`r`/`n` exercise `Backend::set_caret_shape`; TUI's implementation
//! genuinely steers the terminal emulator's hardware caret via DECSCUSR
//! (crossterm's `SetCursorStyle`) — the escape-sequence write this issue
//! moves out of consumer code (vimcode's old hand-rolled
//! `execute!(SetCursorStyle …)`) and into quadraui itself. Esc quits.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::caret_shape_demo::CaretShapeDemo::new())
}
