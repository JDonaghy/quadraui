//! Word-wrapped markdown in a `ListView` — TUI backend runner.
//!
//! Exercises [`render_markdown_to_styled_wrapped`]: renders a markdown
//! document that contains long paragraphs, bold/italic spans, a fenced
//! code block, and a bullet list, all word-wrapped to the current terminal
//! content width and displayed row-by-row in a [`ListView`].
//!
//! - `j` / `↓`  — scroll down
//! - `k` / `↑`  — scroll up
//! - `q` / `Esc` — quit
//!
//! ```sh
//! cargo run --example tui_markdown_wrap --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::MarkdownWrapDemo::new())
}
