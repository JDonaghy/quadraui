//! `FindReplacePanel` `AppLogic` + `quadraui::tui::run` example
//! (quadraui#818).
//!
//! Click a toggle/nav/close button to see it reported in the status bar
//! — `FindReplacePanel::hit_test`'s first driver-tested consumer.
//!
//! - r        toggle the replace row
//! - q / Esc  quit
//!
//! ```sh
//! cargo run --example tui_find_replace --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::FindReplaceApp::new())
}
