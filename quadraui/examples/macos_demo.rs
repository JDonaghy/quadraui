//! `cargo run --example macos_demo --features macos`
//!
//! Smoke harness for the macOS backend through #35. Drives an
//! `AppLogic` implementation (`MiniApp` below) against
//! [`quadraui::macos::run`].
//!
//! `MiniApp` is intentionally trivial: its `render` is a no-op
//! (every `MacBackend::draw_*` is still `unimplemented!()` until
//! #38–#43 fill them in), and its `handle` returns `Reaction::Exit`
//! when Esc is pressed. The window's visible content for now comes
//! from `QuadraView::drawRect:` painting a debug background +
//! the #34 Menlo smoke label.
//!
//! Once the first chrome rasteriser ships (#38), this example will
//! switch to the shared `examples/common/AppState` so paired examples
//! across TUI / GTK / macOS demonstrate the milestone promise: one
//! `AppLogic` impl, every backend.
//!
//! Controls:
//! - **Esc**       — quit
//! - **Close button** — quit (via `QuadraAppDelegate`)

use quadraui::runner::{AppLogic, Reaction};
use quadraui::{Backend, Key, NamedKey, UiEvent};

struct MiniApp;

impl AppLogic for MiniApp {
    type AreaId = ();

    fn render(&self, _backend: &mut dyn Backend, _area: Self::AreaId) {
        // No-op: `MacBackend::draw_*` are all `unimplemented!` until
        // #38–#43 land. The visible content comes from
        // `QuadraView::drawRect:`'s debug paint + #34 smoke label.
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            _ => Reaction::Continue,
        }
    }
}

fn main() -> std::process::ExitCode {
    quadraui::macos::run(MiniApp)
}
