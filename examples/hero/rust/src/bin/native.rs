//! The hero counter in a native window: the identical `Counter` from
//! `src/main.rs`, handed to `run_native`, which picks the native backend
//! feature this build enables (`--features tui,macos`, `tui,gtk` or
//! `tui,win`).

#[allow(dead_code)]
#[path = "../main.rs"]
mod hero;

fn main() -> Result<(), quadraui::RunError> {
    quadraui::app::run_native(hero::Counter::default())
}
