//! Win-GUI port of `tui_hscroll.rs` / `gtk_hscroll.rs` /
//! `macos_hscroll.rs`. Same `HScrollEditor` `AppLogic` impl in
//! `examples/common/hscroll_editor.rs`; only the runner call differs.
//! Horizontal-scroll smoke test.
//!
//! 500-char line editor — press `$` to jump to end, `0` to jump to
//! start.
//!
//! ```sh
//! cargo run --example win_hscroll --features win
//! ```
//! (Windows only.)

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::win::run(common::HScrollEditor::new())
}
