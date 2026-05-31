//! AI-chat-transcript `AppLogic` + `quadraui::gtk::run` example.
//!
//! The headless-chat-server use case: markdown replies are rendered
//! through `render_markdown_to_styled` into a scrolling, auto-following
//! `TextDisplay`. Press Enter to send a canned prompt and watch the
//! markdown reply stream in.
//!
//! ```sh
//! cargo run --example gtk_ai_transcript --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::AiTranscript::new())
}
