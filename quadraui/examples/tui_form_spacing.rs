//! `cargo run --example tui_form_spacing --features tui`
//!
//! Static [`quadraui::Form`] exercising [`quadraui::FormSpacing`]
//! (padding + `item_gap`) and [`quadraui::RowAlign`] on the TUI runner.
//! The full `AppLogic` impl lives in `examples/common/form_spacing.rs`
//! — identical app code drives this example and any future GTK twin.
//!
//! Controls:
//! - `q` / `Esc` → quit

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::FormSpacingApp::new())
}
