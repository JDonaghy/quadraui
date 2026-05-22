//! PipelineView `AppLogic` + `quadraui::tui::run` example.
//!
//! A five-stage CI/CD pipeline with Done/Active/Pending stages, keyboard
//! navigation, and clickable action buttons.
//!
//! - ←/→ arrows move keyboard focus
//! - Enter fires the focused stage's action
//! - r resets the pipeline
//! - q / Esc quits
//!
//! ```sh
//! cargo run --example tui_pipeline --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::PipelineApp::new())
}
