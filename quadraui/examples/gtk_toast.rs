//! Toast `AppLogic` + `quadraui::gtk::run` example.
//!
//! Toast stack with dismiss, action buttons, severity tints.
//! Press 1-4 to add toasts, `a` for action toast, `q` to quit.
//!
//! ```sh
//! cargo run --example gtk_toast --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::ToastApp::new())
}
