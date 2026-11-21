//! SplitTree `AppLogic` + `quadraui::gtk::run` example.
//!
//! A 3-way nested split (`Split(H, Split(V, A, B), C)`) with every
//! divider draggable through `DragTarget::SplitDivider`. Press `r` to
//! reset ratios, `q` to quit.
//!
//! ```sh
//! cargo run --example gtk_split_tree --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> Result<(), quadraui::RunError> {
    quadraui::gtk::run(common::SplitTreeApp::new())
}
