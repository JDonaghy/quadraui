//! `FocusTree` modal-trapping / fallback-on-disappear `AppLogic` +
//! `quadraui::tui::run` example.
//!
//! Three rows plus a confirm dialog: `m` opens the dialog and traps
//! Tab/Shift+Tab inside its two buttons via `FocusTree::restrict`; `d`
//! deletes the focused row and shows focus fall back to "none" rather
//! than a dangling address. See `examples/common/modal_focus_demo.rs`'s
//! module doc for the full control list.
//! ```sh
//! cargo run --example tui_modal_focus_demo --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> Result<(), quadraui::RunError> {
    quadraui::tui::run(common::ModalFocusDemo::new())
}
