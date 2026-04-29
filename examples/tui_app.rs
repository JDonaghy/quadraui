//! Minimal `AppLogic` + `quadraui::tui::run` example.
//!
//! ~10 lines of `main()` — everything else (the `MiniApp` state, the
//! `AppLogic` impl, terminal setup, frame loop, event drain,
//! tear-down) lives in the runner crate or `examples/common/mod.rs`.
//! See `gtk_app.rs` for the GTK twin — same `MiniApp`, different
//! runner.
//!
//! Run with:
//!
//! ```sh
//! cargo run --example tui_app --features tui
//! ```
//!
//! Press any key to bump the counter; `q` or Esc to quit.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::MiniApp::new())
}
