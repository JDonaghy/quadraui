//! Markdown adapter `AppLogic` + `quadraui::gtk::run` example.
//!
//! Renders a fixed markdown document through `render_markdown_to_styled`
//! into a `RichTextPopup`. Demonstrates heading scales, bold/italic/code
//! inline styling, and the flanking guard (`snake_case` identifiers and
//! `a * b * c` stay upright).
//!
//! - ↑/↓ scroll the popup
//! - q / Esc quits
//!
//! ```sh
//! cargo run --example gtk_markdown --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::MarkdownDemo::new())
}
