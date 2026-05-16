//! macOS native NSMenu smoke (#184). Installs File / Edit / View in
//! the system menu bar at the top of the screen, with ⌘-shortcuts on
//! standard actions, two toggle items in View whose `✓` flips on
//! activation, and a nested "Appearance" submenu.
//!
//! ```sh
//! cargo run --example macos_native_menu --features macos
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::macos::run(common::NativeMenuApp::new())
}
