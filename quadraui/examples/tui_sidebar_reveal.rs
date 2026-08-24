//! `SidebarSystem::reveal` (#595) `AppLogic` + `quadraui::tui::run`
//! example. See `examples/common/sidebar_reveal_demo.rs` for the
//! consumer pattern.
//!
//! - `↑` / `↓`     move selection (interactive nav)
//! - `z`           toggle collapse of the section
//! - `g`           reveal: select + expand + scroll the last row into
//!                 view, without interactive nav
//! - `q` / `Esc`   quit
//!
//! ```sh
//! cargo run --example tui_sidebar_reveal --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::SidebarRevealDemo::new())
}
