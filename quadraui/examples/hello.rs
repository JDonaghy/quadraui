//! The smallest quadraui app on the canonical `ShellApp` path (quadraui#1342:
//! the old version computed a `Rect` by hand from `backend.viewport()` and
//! `backend.measure()` and called `Backend::draw_status_bar_interactive`
//! directly — `Backend::draw_*` is the *low-level* entry point
//! (`docs/decisions/DECISIONS.md` D-006). `ShellApp` is canonical instead:
//! [`AppShellLayout::status_bar_bounds`] below is computed for us, so there
//! is no viewport arithmetic anywhere in this file, and painting goes
//! through [`ScreenLayout`]/[`Surface`] — the same declarative frame-list
//! path a multi-primitive screen uses — rather than a raw `draw_*` call.
//!
//! No shared `examples/common/` module (quadraui#799: that module pulls in
//! every other example, so this "hello world" would secretly compile
//! thousands of lines nothing here needs).
//!
//! `cargo run --example hello --features tui`
//!
//! Press any key to bump the counter; `q` or Esc to quit.

use quadraui::prelude::*;
use quadraui::{AppShellLayout, ScreenLayout, Surface};

pub struct Hello {
    pub keys_pressed: u32,
}

impl Hello {
    /// No sidebar panels and no activity bar — this app has nothing to put
    /// in either — just the one status-bar band it actually uses.
    pub fn config() -> ShellConfig {
        ShellConfig::new("Hello", Vec::new())
            .with_status_bar()
            .with_activity_bar_width(0.0)
    }
}

impl ShellApp for Hello {
    fn render_content(&self, backend: &mut dyn Backend, layout: &AppShellLayout) {
        let Some(rect) = layout.status_bar_bounds else {
            return;
        };
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
        let mut frame = ScreenLayout::new();
        frame.push(Surface::StatusBar {
            rect,
            bar: &bar,
            hovered: None,
            pressed: None,
        });
        frame.draw(backend);
    }

    fn handle(
        &mut self,
        event: UiEvent,
        _backend: &mut dyn Backend,
        _ctx: &ShellContext,
    ) -> Reaction {
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

fn main() {
    quadraui::tui::shell_runner::run_with_shell(Hello { keys_pressed: 0 }, Hello::config());
}
