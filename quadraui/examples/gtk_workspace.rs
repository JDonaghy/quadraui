//! `WorkspaceDemo` `ShellApp` + `quadraui::gtk::shell_runner::run_with_shell`.
//!
//! GTK twin of `tui_workspace` — the *same* `ShellApp`, so `Workspace`
//! is exercised through the Pango/Cairo tab-bar rasteriser instead of the
//! cell grid, including its pixel-measured scroll-offset correction. Click a tab to
//! activate it, click its `×` to close it, `Ctrl+Tab` / `Ctrl+PageDown`
//! cycle, `o` opens another document, `q` quits.
//!
//! ```sh
//! cargo run --example gtk_workspace --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> Result<(), quadraui::RunError> {
    let app = common::workspace_demo::WorkspaceDemo::new();
    let config = common::workspace_demo::WorkspaceDemo::config();
    quadraui::gtk::shell_runner::run_with_shell(app, config)
}
