//! `SplitApp` driven through [`quadraui::tui::run_with`] with mouse capture
//! disabled — the runnable demo (and, paired with
//! `tests/tui_pty_smoke.rs`'s `tui_no_mouse_never_negotiates_mouse_capture`,
//! the black-box proof) for `RunConfig { mouse: false, .. }` (**`no-mouse`
//! mode**, quadraui#828).
//!
//! Every other TUI example goes through [`quadraui::tui::run`], which always
//! negotiates mouse capture. This one is the sole caller of `run_with` with
//! an explicit `RunConfig` anywhere in the tree — without it, the config
//! struct and the `if config.mouse { .. } else { .. }` branch it drives in
//! `src/tui/run.rs` compile clean but are never actually exercised end to
//! end: the escape sequences that toggle mouse capture reach a real
//! terminal (or, here, a real pty) only through this code path.
//!
//! [`SplitApp`] was chosen deliberately: its `[`/`]` keys are the documented
//! keyboard equivalent of dragging the split divider (quadraui#828's own
//! "every Tier-1 gesture needs a key path" rule), so this example is a live
//! demonstration that the app stays fully operable with the mouse never
//! negotiated at all — not just that the terminal setup skips a few escape
//! codes.
//!
//! ```sh
//! cargo run --example tui_no_mouse --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run_with(
        common::SplitApp::new(),
        quadraui::tui::RunConfig::no_mouse(),
    )
}
