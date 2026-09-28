//! `ContextMenuStyleDemo` `AppLogic` + `quadraui::tui::run` example.
//!
//! Right-click anywhere for a painted context menu; `m` cycles
//! `MenuStyle` (`Auto`/`Native`/`Custom`) at runtime. TUI always resolves
//! `Custom` (no `native_menu` capability), so the visible menu doesn't
//! change, but the status bar's `effective=` readout does — see
//! `examples/macos_context_menu_style.rs` for where `Auto` actually
//! switches presentation.
//!
//! ```sh
//! cargo run --example tui_context_menu_style --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::ContextMenuStyleDemo::new())
}
