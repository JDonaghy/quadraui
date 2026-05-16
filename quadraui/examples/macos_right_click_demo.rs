//! macOS native right-click context-menu smoke (#185). Right-click
//! anywhere in the window to open a native AppKit pop-up menu with
//! Cut / Copy / Paste / Select All / About. The menu picks up system
//! fonts, accent colour, Dark Mode, ⌘-shortcuts, and AppKit-managed
//! dismissal.
//!
//! ```sh
//! cargo run --example macos_right_click_demo --features macos
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::macos::run(common::RightClickDemo::new())
}
