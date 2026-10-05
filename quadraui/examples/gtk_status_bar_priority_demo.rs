//! StatusBarPriorityDemo `AppLogic` + `quadraui::gtk::run` example.
//!
//! Demonstrates that a `StatusBar` whose `right_segments` follow the
//! documented least-important-first / cursor-last convention keeps its
//! cursor-position segment visible once the left segment grows (e.g. an
//! editor's unsaved-changes badge), even when that growth forces the
//! bar's priority-drop to shed a lower-priority right segment.
//!
//! - d       toggle the simulated "dirty" state
//! - q / Esc quits
//!
//! ```sh
//! cargo run --example gtk_status_bar_priority_demo --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::StatusBarPriorityDemo::new())
}
