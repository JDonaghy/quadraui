//! TUI runner for the window-control demo (issue #950).
//!
//! `t`/`f`/`a`/`m`/`r`/`c` exercise `Backend::window()` /
//! `WindowControl`; the status bar shows each call's `ServiceResult`.
//! `t` (set_title) genuinely retitles the terminal tab via OSC 0/2 on a
//! terminal emulator that honours it — everything else reports
//! `Unsupported` on TUI (no OS window to control). Esc quits.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::window_control_demo::WindowControlDemo::new())
}
