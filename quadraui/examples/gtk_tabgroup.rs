//! `TabGroupController` demo (GTK backend).
//!
//! Two split panes, each with its own tab bar. Demonstrates tab
//! switching, closing, new-tab, pane focus, and divider dragging.
//!
//! ```sh
//! cargo run --example gtk_tabgroup --features gtk
//! ```
//!
//! Controls:
//! - click tab label     — activate that tab
//! - click `×`          — close the tab (last tab collapses the pane)
//! - click `+`          — add an untitled tab
//! - click content area — focus that pane
//! - drag divider       — resize panes
//! - Tab / Shift+Tab    — cycle focus
//! - q / Esc            — quit

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::TabGroupDemo::new())
}
