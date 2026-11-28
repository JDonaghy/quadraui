//! The hero counter in a native window titled "Counter": the identical
//! `Counter` from `src/main.rs`, handed to `run_native_with`, which picks
//! the native backend feature this build enables (`--features tui,macos`,
//! `tui,gtk` or `tui,win`).

#[allow(dead_code)]
#[path = "../main.rs"]
mod hero;

fn main() -> Result<(), quadraui::RunError> {
    use quadraui::app::{run_native_with, NativeOptions};
    run_native_with(hero::Counter::default(), NativeOptions::new("Counter"))
}
