//! `cargo run --example gtk_modal_occlusion --features gtk`
//!
//! Smallest app that makes the #455 "click falls through an open modal"
//! bug class visible: a clickable row list under a centred confirm
//! dialog. Clicking the dialog's own body text must never select the row
//! behind it — the app has no `if dialog_open` guard, so the guarantee is
//! entirely quadraui's `ModalStack` + `dispatch_click`.
//!
//! Driven headlessly by
//! `tests/conformance/scenarios/modal/dialog.blocks_click_through.scn.json`.
//!
//! Controls:
//! - click a row / `Open dialog` / `Cancel`
//! - `q` quit, `Esc` close the dialog (or quit)

#[path = "common/modal_occlusion_demo.rs"]
mod modal_occlusion_demo;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(modal_occlusion_demo::ModalOcclusionDemo::new())
}
