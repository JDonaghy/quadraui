//! Activity-bar keyboard navigation demo — GTK backend.
//!
//! Demonstrates [`quadraui::ActivityBar::is_keyboard_focused`]: the runner's
//! window-level `EventControllerKey` intercepts key events and emits
//! `UiEvent::ActivityBar(id, ActivityBarEvent::KeyPressed { … })` when a bar
//! declares keyboard focus. This avoids the `grab_focus()` / sibling-DA
//! focus-stealing problem described in vimcode#494.
//!
//! Controls:
//! - **Tab**          → focus the activity bar (cursor appears on first item)
//! - **j** / **↓**   → move cursor down
//! - **k** / **↑**   → move cursor up
//! - **l** / **Enter**→ activate highlighted item, return focus
//! - **Esc** / **h**  → return focus without activating
//!
//! Click an activity bar item directly to activate it without keyboard focus.
//!
//! ```sh
//! cargo run --example gtk_activity_nav --features gtk
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::process::ExitCode {
    quadraui::gtk::run(common::ActivityNavApp::new())
}
