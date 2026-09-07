//! The smallest quadraui app — one `AppLogic` impl, no shared
//! `examples/common/` module (quadraui#799: that module pulls in every
//! other example, so `tui_app.rs`'s "hello world" secretly compiled
//! thousands of lines nothing here needs).
//!
//! `cargo run --example hello --features tui`
//!
//! Press any key to bump the counter; `q` or Esc to quit.

use quadraui::prelude::*;

pub struct Hello {
    pub keys_pressed: u32,
}

impl AppLogic for Hello {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let bar = StatusBar {
            id: WidgetId::new("status:bar"),
            left_segments: vec![StatusBarSegment {
                text: format!(" Hello, quadraui! (keys: {}) ", self.keys_pressed),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: true,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " q to quit ".into(),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        };
        // Backend-native bar height instead of a hand-picked pixel
        // constant (quadraui#817): `backend.measure()` bundles
        // `char_width`/`line_height` in one call, so this scales to a
        // real 1-cell-tall bar on TUI and a proportionally-taller pixel
        // bar on GTK/macOS/Win — the same portable-sizing pattern
        // `docs/BACKEND_TRAIT_PROPOSAL.md` documents for `line_height`
        // alone (`backend.line_height() * 1.5`), just asking for both
        // metrics through the one bundled call.
        let vp = backend.viewport();
        let bar_h = backend.measure().line_height.max(1.0) * 1.4;
        let rect = Rect::new(0.0, vp.height - bar_h, vp.width, bar_h);
        let _ = backend.draw_status_bar_interactive(rect, &bar, &quadraui::InteractionState::new());
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed { key, .. } => {
                if matches!(key, Key::Char('q') | Key::Named(NamedKey::Escape)) {
                    return Reaction::Exit;
                }
                self.keys_pressed += 1;
                Reaction::Redraw
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}

fn main() -> std::io::Result<()> {
    quadraui::tui::run(Hello { keys_pressed: 0 })
}
