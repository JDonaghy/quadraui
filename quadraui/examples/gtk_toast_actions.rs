//! Toast-actions `AppLogic` + `quadraui::gtk::run` example (#1185).
//!
//! A VS Code-style two-action toast ("Install" primary + "Don't ask
//! again" secondary) driven by `ToastStackController` for keyboard
//! focus, alongside mouse hit-testing. Press `n` for a toast, `Tab` to
//! focus the stack, then Tab/Left/Right to cycle its buttons, Up/Down to
//! move between toasts, Enter to activate, Escape to dismiss, `q` to
//! quit.
//!
//! ```sh
//! cargo run --example gtk_toast_actions --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::ToastActionsApp::new())
}
