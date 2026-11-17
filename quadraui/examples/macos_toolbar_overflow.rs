//! `cargo run --example macos_toolbar_overflow --features macos`
//!
//! macOS port of `tui_toolbar_overflow.rs`. Same `ToolbarOverflowDemo`
//! `AppLogic` impl in `examples/common/toolbar_overflow_demo.rs`; only the
//! runner call differs. Cycle Clip -> Wrap -> Menu with `m` to see the
//! `MacBackend` `ToolbarOverflow::Wrap`/`ToolbarOverflow::Menu`
//! rasteriser paint extra rows / the trailing `[ » ]` affordance, and
//! click it in `Menu` mode to pop up the dropped items as a native
//! `NSMenu`.

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::macos::run(common::ToolbarOverflowDemo::new())
}
