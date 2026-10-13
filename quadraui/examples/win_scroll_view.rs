//! Win-GUI port of `gtk_scroll_view.rs`. Same `ScrollViewDemo` `AppLogic`
//! impl in `examples/common/scroll_view_demo.rs`; only the runner call
//! differs: scroll the row list with the mouse wheel, or drag the
//! scrollbar thumb along the right edge.
//!
//! ```sh
//! cargo run --example win_scroll_view --features win
//! ```
//! (Windows only.)

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::win::run(common::ScrollViewDemo::new())
}
