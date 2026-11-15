//! `Float` primitive `AppLogic` + `quadraui::gtk::run` example (issue
//! #1321).
//!
//! Same three floats as `tui_float` — hint, actions menu and the
//! transparent/opaque overlay banner — painted via Cairo through the
//! same `Backend::draw_float` call, with no per-backend app code at
//! all.
//!
//! ```sh
//! cargo run --example gtk_float --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::FloatApp::new())
}
