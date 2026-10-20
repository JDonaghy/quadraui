//! `SplitApp` driven through [`quadraui::macos::MacRunner`]'s non-blocking
//! `step`/`pump` entry points instead of the all-owning [`quadraui::macos::run`]
//! (`NSApplication::run()`, which blocks the calling thread until the app
//! quits) — the macOS counterpart of `examples/tui_embedded.rs` /
//! `examples/gtk_embedded.rs`.
//!
//! This example's own `main` plays the "host loop" role: a bare `loop {}`
//! that does nothing but call `runner.pump(...)` every iteration. A real
//! host (Node's libuv reactor, a .NET `SynchronizationContext`, Python's
//! `asyncio` loop, a game engine's per-frame tick) would interleave its own
//! work between calls instead — that's the entire point of `step`/`pump`
//! returning after one bounded unit of work rather than looping internally
//! the way `run`/`run_with` do.
//!
//! `SplitApp` is unchanged from `macos_split`/`tui_embedded`, kept identical
//! deliberately so this stays a minimal diff isolating only the runner
//! entry point, not a new shape.
//!
//! ```sh
//! cargo run --example macos_embedded --features macos
//! ```

#[path = "common/mod.rs"]
mod common;

use std::time::Duration;

use quadraui::macos::{MacRunner, StepOutcome};

fn main() -> std::process::ExitCode {
    let mut runner = MacRunner::new(common::SplitApp::new());
    loop {
        // A real embedding host would do its own work here between turns
        // (service its own reactor, advance a game frame, etc.) — this
        // example just hands quadraui a bounded slice on every iteration,
        // the same shape a libuv idle handle or an `asyncio` task would
        // drive it from.
        // `Exited` means the window was *closed*, not merely taken
        // offscreen: minimizing (Cmd-M) or hiding the app (Cmd-H) keeps
        // this loop pumping, which is what lets the window come back.
        if runner.pump(Duration::from_millis(50)) == StepOutcome::Exited {
            return std::process::ExitCode::SUCCESS;
        }
    }
}
