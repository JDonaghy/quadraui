//! `SidebarPanelBody` (#1041) `AppLogic` + `quadraui::gtk::run`
//! example. Same logic as `tui_sidebar_panel_body`, paired by shape —
//! see `examples/common/sidebar_panel_body_demo.rs`.
//!
//! ```sh
//! cargo run --example gtk_sidebar_panel_body --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::SidebarPanelBodyDemo::new())
}
