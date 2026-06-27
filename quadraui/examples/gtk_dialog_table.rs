//! `cargo run --example gtk_dialog_table --features gtk`
//!
//! Renders a `Dialog` with a [`DialogTable`] — a two-column keybindings
//! grid with headers and a `──────` separator row. Demonstrates the new
//! table-layout slot (issue #225).
//!
//! Controls:
//! - `q` / `Esc`    quit

#[path = "common/dialog_table_demo.rs"]
mod dialog_table_demo;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(dialog_table_demo::DialogTableDemo::new())
}
