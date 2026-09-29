//! `SplitApp` driven through [`quadraui::gtk::GtkRunner`]'s non-blocking
//! `step`/`pump` entry points instead of the all-owning [`quadraui::gtk::run`]
//! (`Application::run()`, which blocks the calling thread until the app
//! quits) — the GTK counterpart of `examples/tui_embedded.rs` (issue #1100).
//!
//! This example's own `main` plays the "host loop" role: a bare `loop {}`
//! that does nothing but call `runner.pump(...)` every iteration. A real
//! host (Node's libuv reactor, a .NET `SynchronizationContext`, Python's
//! `asyncio` loop, a game engine's per-frame tick) would interleave its own
//! work between calls instead — that's the entire point of `step`/`pump`
//! returning after one bounded unit of work rather than looping internally
//! the way `run`/`run_with` do.
//!
//! `SplitApp` is unchanged from `gtk_split`/`tui_embedded`, kept identical
//! deliberately so this stays a minimal diff isolating only the runner
//! entry point, not a new shape.
//!
//! ```sh
//! cargo run --example gtk_embedded --features gtk
//! ```
//!
//! **Operator-run headless smoke** (real window, no CI — see
//! `quadraui/docs/TESTING.md`'s "Headless smoke mode" section):
//!
//! ```sh
//! quadraui/scripts/gtk_smoke.sh gtk_embedded gtk
//! ```
//!
//! `GtkRunner::new`/`new_with` honour `QUADRAUI_GTK_SMOKE_MS`/
//! `QUADRAUI_GTK_SMOKE_PASTE` exactly like `run`/`run_with` do (both funnel
//! through the same `activate` function), so the smoke script's forced
//! window close after `QUADRAUI_GTK_SMOKE_MS` still drives this example's
//! `pump` loop to [`quadraui::gtk::StepOutcome::Exited`] and a clean
//! process exit — proving the embedding path tears the window down the
//! same way the all-owning one does.

#[path = "common/mod.rs"]
mod common;

use std::time::Duration;

use quadraui::gtk::{GtkRunner, StepOutcome};

fn main() {
    let mut runner = GtkRunner::new(common::SplitApp::new());
    loop {
        // A real embedding host would do its own work here between turns
        // (service its own reactor, advance a game frame, etc.) — this
        // example just hands quadraui a bounded slice on every iteration,
        // the same shape a libuv idle handle or an `asyncio` task would
        // drive it from.
        if runner.pump(Duration::from_millis(50)) == StepOutcome::Exited {
            return;
        }
    }
}
