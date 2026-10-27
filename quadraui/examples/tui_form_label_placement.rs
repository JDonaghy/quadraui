//! `cargo run --example tui_form_label_placement --features tui`
//!
//! Form with per-field `label_position`/`label_align`/`label_width` and a
//! custom `tab_index` order on the TUI runner. The full `AppLogic` impl
//! lives in `examples/common/form_label_placement.rs`.
//!
//! Controls:
//! - `Tab` / `Shift+Tab`    → move focus in `tab_index` order
//! - typing (focused field) → edit the focused text field
//! - `Enter` (focused field) → commit the focused text field
//! - `q` / `Esc`            → quit

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::FormLabelPlacementApp::new())
}
