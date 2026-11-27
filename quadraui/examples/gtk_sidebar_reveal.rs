//! `Sidebar::reveal` `AppLogic` + `quadraui::gtk::run`
//! example. Same logic as `tui_sidebar_reveal`, paired by shape — see
//! `examples/common/sidebar_reveal_demo.rs`.
//!
//! ```sh
//! cargo run --example gtk_sidebar_reveal --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> Result<(), quadraui::RunError> {
    quadraui::gtk::run(common::SidebarRevealDemo::new())
}
