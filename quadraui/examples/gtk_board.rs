//! Board (kanban) `AppLogic` + `quadraui::gtk::run` example.
//!
//! A three-column kanban board with cards, badge icons showing CI / review
//! status, and a decision hint on selected cards.
//!
//! Controls:
//! - j / ↓  move selection down
//! - k / ↑  move selection up
//! - h / ←  jump to previous column
//! - l / →  jump to next column
//! - g       jump to first card in column
//! - G       jump to last card in column
//! - q / Esc quit
//!
//! ```sh
//! cargo run --example gtk_board --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::BoardApp::new())
}
