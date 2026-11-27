//! Win-GUI runner for ShellView demo.
//!
//! Windows-only at runtime, but compiles on every host under plain
//! `cargo build --example win_appshell_demo --features win` — same
//! "compiles everywhere, only *works* on Windows" posture as the rest of
//! `src/win/` (see `win::run`'s module docs).
#[path = "common/mod.rs"]
mod common;

fn main() -> Result<(), quadraui::RunError> {
    let app = common::appshell_demo::AppShellDemo::new();
    let config = common::appshell_demo::AppShellDemo::config();
    quadraui::win::shell_runner::run_with_shell(app, config)
}
