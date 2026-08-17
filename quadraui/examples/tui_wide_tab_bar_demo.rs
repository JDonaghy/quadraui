//! `WideTabBarDemo` `AppLogic` + `quadraui::tui::run` example.
//!
//! One `TabBar` with a CJK ("日本語.rs") tab label and an ASCII control
//! tab — the conformance-matrix fixture for quadraui#555's TUI vt100
//! observer. `q` to quit.
//!
//! ```sh
//! cargo run --example tui_wide_tab_bar_demo --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::WideTabBarDemo::new())
}
