//! `TabIconsDemo` `AppLogic` + `quadraui::macos::run` example.
//!
//! macOS twin of `tui_tab_icons` / `gtk_tab_icons` — the same
//! `AppLogic`, so the icon sidecar (quadraui#620) is exercised through
//! the CoreText/Core Graphics rasteriser instead of the cell grid or
//! Pango. `i` toggles the sidecar: with #926's CoreText icon-width pass
//! the labels shift by exactly the icon reservation, the close `×`
//! glyphs shift with them, and a click still closes the tab it landed
//! on. `tab` cycles the active tab, `q` quits.
//!
//! ```sh
//! cargo run --example macos_tab_icons --features macos
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::macos::run(common::TabIconsDemo::new())
}
