//! Activity-bar keyboard navigation demo — TUI backend.
//!
//! Demonstrates [`quadraui::ActivityBar::is_keyboard_focused`]: the backend
//! emits `UiEvent::ActivityBar(id, ActivityBarEvent::KeyPressed { … })`
//! instead of a raw `UiEvent::KeyPressed` when a bar declares keyboard focus,
//! so navigation logic stays entirely in app code.
//!
//! Controls:
//! - **Tab**          → focus the activity bar (cursor appears on first item)
//! - **j** / **↓**   → move cursor down
//! - **k** / **↑**   → move cursor up
//! - **l** / **Enter**→ activate highlighted item, return focus
//! - **Esc** / **h**  → return focus without activating
//! - **q** / **Esc** (editor area) → quit
//!
//! ```sh
//! cargo run --example tui_activity_nav --features tui
//! ```

#[path = "common/mod.rs"]
mod common;

fn main() -> std::io::Result<()> {
    quadraui::tui::run(common::ActivityNavApp::new())
}
