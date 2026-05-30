//! Markdown adapter `AppLogic` + `quadraui::tui::run` example.
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
//! cargo run --example tui_markdown --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::MarkdownDemo::new())
}
