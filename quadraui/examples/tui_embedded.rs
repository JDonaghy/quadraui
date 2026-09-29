//! `SplitApp` driven through [`quadraui::tui::TuiRunner`]'s non-blocking
//! `step`/`pump` entry points instead of the all-owning [`quadraui::tui::run`]
//! loop — the runnable demo (and, paired with `tests/tui_pty_smoke.rs`'s
//! `embedded_runner` module, the black-box proof) for issue #1100: a host
//! that already owns its own event loop (Node's libuv reactor, a .NET
//! `SynchronizationContext`, Python's `asyncio` loop, a game engine's
//! per-frame tick) can still embed a quadraui TUI app by calling
//! `TuiRunner::pump(timeout)`/`step()` from wherever *it* schedules
//! quadraui's turn, instead of handing the whole process over to
//! `quadraui::tui::run`.
//!
//! This example's own `main` plays the "host loop" role: a bare `loop {}`
//! that does nothing but call `runner.pump(...)` every iteration. A real
//! host would interleave its own work between calls — that's the entire
//! point of `step`/`pump` returning after one bounded unit of work instead
//! of looping internally the way `run`/`run_with` do.
//!
//! `SplitApp` itself is unchanged from `tui_split`/`tui_no_mouse`, chosen
//! for the same reason `tui_no_mouse` picked it: its `[`/`]` keyboard
//! resize path is a Tier-1 gesture with no mouse dependency, so this
//! example stays a minimal diff from its siblings rather than a new shape.
//!
//! ```sh
//! cargo run --example tui_embedded --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

use std::time::Duration;

use quadraui::tui::{StepOutcome, TuiRunner};

fn main() -> std::io::Result<()> {
    let mut runner = TuiRunner::new(common::SplitApp::new())?;
    loop {
        // A real embedding host would do its own work here between turns
        // (service its own reactor, advance a game frame, etc.) — this
        // example just hands quadraui a bounded slice on every iteration,
        // the same shape a libuv idle handle or an `asyncio` task would
        // drive it from.
        if runner.pump(Duration::from_millis(50))? == StepOutcome::Exited {
            return Ok(());
        }
    }
}
