//! GTK runner for the TextInput demo (#222 smoke test).
//!
//! Run with:
//!
//! ```sh
//! cargo run --example gtk_text_input --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::text_input_demo::TextInputDemo::new())
}
