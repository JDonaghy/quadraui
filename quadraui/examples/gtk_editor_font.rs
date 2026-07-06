//! GTK runner for the editor-font-override demo (#422).
//!
//! Proves `ShellConfig::with_editor_font()` / `Backend::set_editor_font`:
//! the editor renders at a large custom font instead of the runner's
//! hardcoded `"Monospace 11"` default.
//!
//! ```sh
//! cargo run --example gtk_editor_font --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() {
    let app = common::EditorFontDemo::new();
    let config = common::EditorFontDemo::config();
    quadraui::gtk::shell_runner::run_with_shell(app, config);
}
