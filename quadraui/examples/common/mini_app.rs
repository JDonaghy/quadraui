//! Backend-agnostic app code for the minimal "app" example
//! ([`tui_app`] / [`gtk_app`]).
//!
//! [`MiniApp`] is the smallest possible runner-driven app: a single
//! bottom-anchored [`StatusBar`] with a key counter and the last key
//! pressed. The same `AppLogic` impl drives both backends — the only
//! difference between `tui_app.rs` and `gtk_app.rs` is the runner
//! call.

use quadraui::{
    AppLogic, Backend, Color, Key, NamedKey, Reaction, Rect, StatusBar, StatusBarSegment, UiEvent,
    WidgetId,
};

pub struct MiniApp {
    pub keys_pressed: u32,
    pub last_key: Option<Key>,
}

impl MiniApp {
    pub fn new() -> Self {
        Self {
            keys_pressed: 0,
            last_key: None,
        }
    }

    fn status_bar(&self) -> StatusBar {
        let count = format!(" keys: {} ", self.keys_pressed);
        let last = match &self.last_key {
            Some(Key::Char(c)) => format!(" last: {c} "),
            Some(Key::Named(n)) => format!(" last: {n:?} "),
            None => " press any key — q to quit ".to_string(),
        };
        StatusBar {
            id: WidgetId::new("status:bar"),
            left_segments: vec![StatusBarSegment {
                text: " quadraui::run demo ".into(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: true,
                action_id: None,
            }],
            right_segments: vec![
                StatusBarSegment {
                    text: last,
                    fg: Color::rgb(220, 220, 220),
                    bg: Color::rgb(40, 80, 120),
                    bold: false,
                    action_id: None,
                },
                StatusBarSegment {
                    text: count,
                    fg: Color::rgb(255, 255, 255),
                    bg: Color::rgb(40, 80, 120),
                    bold: false,
                    action_id: None,
                },
            ],
        }
    }
}

impl Default for MiniApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for MiniApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let bar = self.status_bar();
        let viewport = backend.viewport();
        let row_h = 28.0;
        let rect = Rect::new(0.0, viewport.height - row_h, viewport.width, row_h);
        let _ = backend.draw_status_bar(rect, &bar);
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed { key, .. } => {
                if matches!(key, Key::Char('q')) || matches!(key, Key::Named(NamedKey::Escape)) {
                    return Reaction::Exit;
                }
                self.keys_pressed += 1;
                self.last_key = Some(key);
                Reaction::Redraw
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
