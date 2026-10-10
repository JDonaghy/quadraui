//! macOS port of `tui_menu_bar.rs` / `gtk_menu_bar.rs`. Same
//! `MenuBarApp` `AppLogic` impl in `examples/common/menu_bar_app.rs`,
//! with zero macOS-specific code — only the runner call differs.
//!
//! On TUI/GTK this paints the in-window `MenuBar` primitive at the top
//! with a `StatusBar` at the bottom, same as ever. On macOS,
//! `Backend::draw_menu_bar` installs the real system
//! `NSMenu` instead — macOS has no Alt-mnemonic convention and a
//! painted `File Edit View` strip is the one that needs opting into
//! there, not the other way round. `MenuSystem::handle` routes the
//! resulting `UiEvent::MenuActivated` to the same `MenuEvent::Activated`
//! the painted dropdown path produces, so this `AppLogic` needed no
//! change to pick up the native menu bar — see `macos_native_menu.rs`
//! for an app that drives `Backend::install_menu_bar` directly instead
//! (checked-state toggles, submenus wired by hand), and
//! `macos_context_menu_style.rs` for the general `Backend::set_menu_style`
//! opt-out mechanism (demonstrated there for the context-menu path,
//! which this example's menu bar shares the same `MenuStyle` with).
//!
//! Click a menu item to activate. `q` or Esc to quit.
//!
//! ```sh
//! cargo run --example macos_menu_bar --features macos
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::macos::run(common::MenuBarApp::new())
}
