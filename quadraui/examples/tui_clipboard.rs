//! TUI runner for the Clipboard demo (#398 smoke test).
//!
//! Manually verifies the native-clipboard-tool fallback leg added to
//! `TuiClipboard::write_text`. Run this **inside tmux on a local X11
//! (or Wayland) session** — that's the setup where arboard and OSC 52
//! alone were shown to miss the real system clipboard:
//!
//! ```sh
//! tmux new -s clip-test
//! cargo run --example tui_clipboard --features tui
//! ```
//!
//! Then, from another pane: `xclip -o -selection clipboard` (X11) or
//! `wl-paste` (Wayland) after pressing Ctrl-C in the demo.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::clipboard_demo::ClipboardDemo::new())
}
