//! GTK horizontal-scroll smoke test.
//!
//! 500-char line editor — press `$` to jump to end, `0` to jump to start.
//!
//! ```sh
//! cargo run --example gtk_hscroll --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::HScrollEditor::new())
}
