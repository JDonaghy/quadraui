//! Win-GUI runner for AppShell demo — proves `run_with_shell()` works on
//! the Win-GUI backend (quadraui#707, the Windows analogue of #465). Same
//! `AppShellDemo` `ShellApp` impl `tui_appshell_demo.rs` / `gtk_appshell_demo.rs`
//! / `macos_appshell_demo.rs` already share; only the runner call differs.
//!
//! Windows-only at runtime, but compiles on every host under plain
//! `cargo build --example win_appshell_demo --features win` — same
//! "compiles everywhere, only *works* on Windows" posture as the rest of
//! `src/win/` (see `win::run`'s module docs).
#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    let app = common::appshell_demo::AppShellDemo::new();
    let config = common::appshell_demo::AppShellDemo::config();
    quadraui::win::shell_runner::run_with_shell(app, config)
}
