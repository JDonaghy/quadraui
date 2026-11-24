//! `WideTabBarDemo` `AppLogic` + `quadraui::tui::run` example.
//!
//! One `TabBar` with a CJK ("日本語.rs") tab label and an ASCII control
//! tab — the conformance-matrix fixture for quadraui#555's TUI vt100
//! observer. The bar is pushed as a `Surface::Widget` and routed through
//! `AppLogic::frame_tree`, so clicking a tab or walking it with
//! Left/Right + Enter activates it. `q` to quit.
//!
//! ```sh
//! cargo run --example tui_wide_tab_bar_demo --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> Result<(), quadraui::RunError> {
    quadraui::tui::run(common::WideTabBarDemo::new())
}
