//! GridApp `AppLogic` + `quadraui::gtk::run` example.
//!
//! Six metric tiles laid out into a 2-row-by-3-column dashboard via
//! `quadraui::layout::resolve_grid_tracks`.
//!
//! - arrow keys move 2-D focus between tiles
//! - Enter activates the focused tile
//! - click a tile to focus + activate it
//! - q / Esc quits
//!
//! ```sh
//! cargo run --example gtk_grid --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::GridApp::new())
}
