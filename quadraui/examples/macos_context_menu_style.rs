//! `ContextMenuStyleDemo` `AppLogic` + `quadraui::macos::run` example
//! (issue #1187) — the one backend where `MenuStyle::Auto` actually
//! switches presentation.
//!
//! Right-click anywhere for a context menu. Press `m` to cycle
//! `MenuStyle`: under `Auto` (the default) or `Native` it's a real
//! `NSMenu`; under `Custom` it's the same in-window painted popup TUI
//! and GTK always use, via [`quadraui::ContextMenuController`] — same
//! app code, same `MouseDown { button: Right }` handler, no branching.
//!
//! ```sh
//! cargo run --example macos_context_menu_style --features macos
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::macos::run(common::ContextMenuStyleDemo::new())
}
