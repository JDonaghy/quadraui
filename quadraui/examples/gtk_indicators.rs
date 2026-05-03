//! Progress + Spinner `AppLogic` + `quadraui::gtk::run` example.
//!
//! Progress bar + spinner indicator demo.
//! Press space to advance, `i` for indeterminate, `c` for cancel, `q` to quit.
//!
//! ```sh
//! cargo run --example gtk_indicators --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::IndicatorsApp::new())
}
