//! PipelineView `AppLogic` + `quadraui::gtk::run` example.
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
//! cargo run --example gtk_pipeline --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::PipelineApp::new())
}
