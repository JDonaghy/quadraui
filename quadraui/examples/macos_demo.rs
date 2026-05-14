//! `cargo run --example macos_demo --features macos`
//!
//! Smoke-test for the macOS backend bootstrap (issue #32). Opens an
//! AppKit window, paints a flat dark grey through Core Graphics on
//! every `drawRect:`, and proves the Retina backing factor flows into
//! `Viewport::scale`.
//!
//! Once #35 lands this example will be rewritten to share the
//! `common::AppState` AppLogic that the TUI and GTK demos already
//! drive, matching the milestone promise that every `AppLogic`
//! example runs unchanged across backends.

fn main() -> std::process::ExitCode {
    quadraui::macos::run()
}
