//! Win-GUI runner for the shell + MenuSystem regression demo (#411).
//!
//! Open the "File" menu and click an item — including the ones drawn
//! over the activity bar strip on the left, which `AppShell`'s chrome
//! hit-testing must not swallow.
//!
//! The menu bar lives in `AppShell`'s title-bar band
//! (`ShellConfig::with_title_bar`) — the same band `WinBackend::
//! nc_hit_test` reclassifies from `HTCAPTION` to `HTCLIENT` over anything
//! a host paints inside it, so a click on "File" reaches the app as an
//! ordinary `MouseDown` instead of being swallowed as a title-bar drag.
//!
//! Windows-only at runtime, but compiles on every host under plain
//! `cargo build --example win_shell_menu --features win` — same
//! "compiles everywhere, only *works* on Windows" posture as the rest of
//! `src/win/` (see `win::run`'s module docs).
//!
//! ```sh
//! cargo run --example win_shell_menu --features win
//! ```
//! (Windows only.)
#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    let app = common::ShellMenuDemo::new();
    let config = common::shell_menu_demo::ShellMenuDemo::config();
    quadraui::win::shell_runner::run_with_shell(app, config)
}
