//! `WorkspaceDemo` `ShellApp` + `quadraui::tui::shell_runner::run_with_shell`.
//!
//! `WorkspaceController` (quadraui#596) mounted **inside an AppShell
//! panel**: the controller paints its own tab strip into the sidebar's
//! content rect, and this app paints the active document's body into the
//! main content area. Click a tab to activate it, click its `×` to close
//! it, `Ctrl+Tab` / `Ctrl+PageDown` cycle, `o` opens another document
//! (enough of them overflow the strip), `q` quits.
//!
//! ```sh
//! cargo run --example tui_workspace --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() {
    let app = common::workspace_demo::WorkspaceDemo::new();
    let config = common::workspace_demo::WorkspaceDemo::config();
    quadraui::tui::shell_runner::run_with_shell(app, config);
}
