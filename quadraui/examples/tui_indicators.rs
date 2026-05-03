//! Progress + Spinner `AppLogic` + `quadraui::tui::run` example.
//!
//! Progress bar + spinner indicator demo.
//! Press space to advance, `i` for indeterminate, `c` for cancel, `q` to quit.
//!
//! ```sh
//! cargo run --example tui_indicators --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::IndicatorsApp::new())
}
