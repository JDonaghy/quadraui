//! SidebarSystem search panel — Form (ToggleGroup) + Tree (Header rows).
//!
//! Click individual toggle items, click file headers to collapse,
//! click match rows to select. Status bar shows received events.
//!
//! ```sh
//! cargo run --example gtk_sidebar_search --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::SidebarSearchApp::new())
}
