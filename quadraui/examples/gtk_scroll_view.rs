//! GTK `ScrollView` demo: scroll the row list with the mouse wheel, or
//! drag the scrollbar thumb along the right edge.
//!
//! ```sh
//! cargo run --example gtk_scroll_view --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::ScrollViewDemo::new())
}
