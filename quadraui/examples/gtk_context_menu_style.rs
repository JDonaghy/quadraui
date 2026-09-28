//! `ContextMenuStyleDemo` `AppLogic` + `quadraui::gtk::run` example.
//!
//! Right-click anywhere for a painted context menu; `m` cycles
//! `MenuStyle` (`Auto`/`Native`/`Custom`) at runtime. GTK always resolves
//! `Custom` (no `native_menu` capability), so the visible menu doesn't
//! change, but the status bar's `effective=` readout does.
//!
//! ```sh
//! cargo run --example gtk_context_menu_style --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::ContextMenuStyleDemo::new())
}
