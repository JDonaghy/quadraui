//! TUI runner for the platform-services seam demo.
//!
//! `p`/`t`/`w`/`g`/`d`/`l`/`e`/`o`/`r`/`c`/`k`/`j`/`u` exercise
//! `Backend::services()` / `PlatformServices`; the status bar shows each
//! call's `ServiceResult`. TUI genuinely supports `app_path`, the
//! power-save-block round trip, and `login_item_enabled` — everything
//! else reports `Unsupported` on TUI (no OS registry, drag source,
//! updater, or desktop shell seam to call into). Esc quits.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::platform_seams_demo::PlatformSeamsDemo::new())
}
