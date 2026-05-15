//! macOS port of `tui_hscroll.rs` / `gtk_hscroll.rs`. Same
//! `HScrollEditor` `AppLogic` impl in `examples/common/hscroll_editor.rs`;
//! only the runner call differs. Horizontal-scroll smoke test.
//!
//! 500-char line editor — press `$` to jump to end, `0` to jump to start.
//!
//! ```sh
//! cargo run --example macos_hscroll --features macos
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::macos::run(common::HScrollEditor::new())
}
