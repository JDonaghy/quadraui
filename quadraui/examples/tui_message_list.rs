//! `MessageList` `AppLogic` + `quadraui::tui::run` example (quadraui#818).
//!
//! Click a row to see it reported in the status bar —
//! `MessageList::hit_test`'s first driver-tested consumer.
//!
//! - j / ↓    scroll down
//! - k / ↑    scroll up
//! - q / Esc  quit
//!
//! ```sh
//! cargo run --example tui_message_list --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::MessageListApp::new())
}
