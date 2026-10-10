//! `{{project-name}}` — a quadraui app.
//!
//! [`App`] is the whole UI: one [`quadraui::ShellApp`] implementation, with
//! no backend-specific code anywhere in this file. `src/main.rs` picks
//! which backend actually runs it based on which Cargo feature is enabled
//! — see that file's `fn main` bodies. This mirrors `quadraui`'s own
//! canonical onboarding example, `quadraui/examples/hello.rs`.
//!
//! Painting goes through [`quadraui::ScreenLayout`]/[`quadraui::Surface`]
//! — the same declarative frame-list path a multi-primitive screen uses —
//! rather than a raw `Backend::draw_*` call, and
//! [`quadraui::AppShellLayout::status_bar_bounds`] below is computed for
//! us, so there is no viewport arithmetic anywhere in this file either.

use quadraui::prelude::*;
use quadraui::{AppShellLayout, ScreenLayout, Surface};

/// The whole app: one counter of how many keys have been pressed.
pub struct App {
    pub keys_pressed: u32,
}

impl App {
    pub fn new() -> Self {
        App { keys_pressed: 0 }
    }

    /// No sidebar panels and no activity bar — this app has nothing to put
    /// in either — just the one status-bar band it actually uses.
    pub fn config() -> ShellConfig {
        ShellConfig::new("{{project-name}}", Vec::new())
            .with_status_bar()
            .with_activity_bar_width(0.0)
    }
}

impl Default for App {
    fn default() -> Self {
        App::new()
    }
}

impl ShellApp for App {
    fn render_content(&self, backend: &mut dyn Backend, layout: &AppShellLayout) {
        let Some(rect) = layout.status_bar_bounds else {
            return;
        };
        let bar = StatusBar {
            id: WidgetId::new("status:bar"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {{project-name}} (keys: {}) ", self.keys_pressed),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_has_a_title_and_a_status_bar() {
        let config = App::config();
        assert_eq!(config.title, "{{project-name}}");
        assert!(config.has_status_bar);
    }

    #[test]
    fn new_app_starts_with_zero_keys_pressed() {
        assert_eq!(App::new().keys_pressed, 0);
    }
}
