//! Tooltip `AppLogic` + `quadraui::gtk::run` example.
//!
//! Cycle through `Tooltip`'s border vocabulary (#541): side bars, a
//! closed box, or no chrome at all, plus a title embedded in the box's
//! top border row. Press 1/2/3 for Sides/Full/None, `t` to toggle the
//! title, `q` to quit.
//!
//! ```sh
//! cargo run --example gtk_tooltip --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::TooltipDemo::new())
}
