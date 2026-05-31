//! AI-chat-transcript `AppLogic` + `quadraui::tui::run` example.
//!
//! The headless-chat-server use case: markdown replies are rendered
//! through `render_markdown_to_styled` into a scrolling, auto-following
//! `TextDisplay`. Press Enter to send a canned prompt and watch the
//! markdown reply stream in.
//!
//! ```sh
//! cargo run --example tui_ai_transcript --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::AiTranscript::new())
}
