//! `cargo run --example gtk_multi_tree --features gtk`
//!
//! Debug-sidebar consumer pattern on the GTK runner. The full
//! `AppLogic` impl lives in `examples/common/multi_tree.rs` —
//! identical app code drives this example AND its TUI twin
//! (`msv_multi_tree.rs`). The only difference between this file and
//! `msv_multi_tree.rs` is the runner call.
//!
//! See `examples/common/multi_tree.rs` for the consumer pattern, and
//! `quadraui/CLAUDE.md` *Consumer patterns* for the design rationale.
//!
//! Controls:
//! - mouse click on header / body / scrollbar
//! - `Tab` / `Shift+Tab` cycle active section
//! - `↑` / `↓`            scroll active section
//! - `Enter`              select first row of active section
//! - `q` / `Esc`          quit

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::DebugSidebar::new())
}
