//! `cargo run --example tui_modal_occlusion --features tui`
//!
//! A clickable row list under a centred confirm dialog. Clicking the
//! dialog's own body text must never select the row behind it — the app
//! has no `if dialog_open` guard, so the guarantee is entirely quadraui's
//! `ModalStack` + `dispatch_click`.
//!
//! Controls:
//! - click a row / `Open dialog` / `Cancel`
//! - `q` quit, `Esc` close the dialog (or quit)

#[path = "common/modal_occlusion_demo.rs"]
mod modal_occlusion_demo;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(modal_occlusion_demo::ModalOcclusionDemo::new())
}
