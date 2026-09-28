//! Toast-actions `AppLogic` + `quadraui::tui::run` example (#1185).
//!
//! A VS Code-style two-action toast ("Install" primary + "Don't ask
//! again" secondary) driven by `ToastStackController` for keyboard
//! focus, alongside mouse hit-testing. Press `n` for a toast, `Tab` to
//! focus the stack, then Tab/Left/Right to cycle its buttons, Up/Down to
//! move between toasts, Enter to activate, Escape to dismiss, `q` to
//! quit.
//!
//! ```sh
//! cargo run --example tui_toast_actions --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::ToastActionsApp::new())
}
