//! `Float` primitive `AppLogic` + `quadraui::tui::run` example (issue
//! #1321).
//!
//! Two floats: a non-focusable which-key hint and a focusable actions
//! menu anchored to the selected command row, auto-flipping above it
//! near the bottom of the list. See `examples/common/float_app.rs`'s
//! module doc for the full control list and what each float
//! demonstrates.
//!
//! ```sh
//! cargo run --example tui_float --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::FloatApp::new())
}
