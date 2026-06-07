//! Mouse-drag text-selection + Ctrl-C copy — TUI example.
//!
//! Exercises the selection pipeline through `run_with_shell`, proving that
//! the pipeline works for shell apps (coord-tui's entry point) just as it
//! does for direct `tui::run` apps. This is the acceptance demo for issue #283.
//!
//! ## Controls
//!
//! | Input                     | Action                                  |
//! |---------------------------|-----------------------------------------|
//! | Click-drag content lines  | Start / extend text selection           |
//! | Ctrl-A                    | Select all lines in the content area    |
//! | Ctrl-C (with selection)   | Copy selection; shows a preview         |
//! | q / Esc                   | Quit                                    |
//!
//! ```sh
//! cargo run --example tui_selection --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() {
    quadraui::tui::shell_runner::run_with_shell(
        common::SelectionDemo::new(),
        common::SelectionDemo::config(),
    );
}
