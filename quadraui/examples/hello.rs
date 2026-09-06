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
        let vp = backend.viewport();
        let rect = Rect::new(0.0, vp.height - 28.0, vp.width, 28.0);
        let _ = backend.draw_status_bar(rect, &bar, None, None);
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
