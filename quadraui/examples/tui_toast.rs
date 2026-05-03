//! Toast `AppLogic` + `quadraui::tui::run` example.
//!
//! Toast stack with dismiss, action buttons, severity tints.
//! Press 1-4 to add toasts, `a` for action toast, `q` to quit.
//!
//! ```sh
//! cargo run --example tui_toast --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::ToastApp::new())
}
