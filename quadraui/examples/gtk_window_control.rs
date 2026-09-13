//! GTK runner for the window-control demo (issue #950).
//!
//! Same `AppLogic` as `tui_window_control`, proving the
//! `Backend::window()` dispatch is backend-generic. On GTK, `t`/`f`/`m`/
//! `r`/`c` genuinely retitle/fullscreen/minimize/restore/center the real
//! `gtk4::ApplicationWindow`; `a` (always-on-top) reports `Unsupported`
//! — GTK4/Wayland has no client-requestable always-on-top protocol (see
//! `WindowControl::set_always_on_top`'s doc).

#[path = "common/mod.rs"]
mod common;

fn main() {
    quadraui::gtk::run(common::window_control_demo::WindowControlDemo::new());
}
