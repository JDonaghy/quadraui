//! `TextDisplayWrapDemo` `AppLogic` + `quadraui::gtk::run` example
//! (quadraui#905).
//!
//! A single `TextDisplay` with one line far wider than the viewport,
//! demonstrating that it word-wraps onto continuation rows (each prefixed
//! with "↳ ") instead of being silently truncated at the right edge.
//!
//! - `q` / `Esc` quits
//!
//! ```sh
//! cargo run --example gtk_text_display_wrap --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::TextDisplayWrapDemo::new())
}
