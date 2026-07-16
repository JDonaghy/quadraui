//! SplitTree `AppLogic` + `quadraui::tui::run` example.
//!
//! A 3-way nested split (`Split(H, Split(V, A, B), C)`) with every
//! divider draggable through `DragTarget::SplitDivider`. Press `r` to
//! reset ratios, `q` to quit.
//!
//! ```sh
//! cargo run --example tui_split_tree --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::SplitTreeApp::new())
}
