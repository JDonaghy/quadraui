//! `MessageListApp` — minimal visual smoke test for the `MessageList`
//! primitive's [`MessageList::hit_test`] (quadraui#818).
//!
//! Renders a scrollable list of styled rows filling most of the
//! viewport. Clicking a row reports its index and text in the status
//! bar — the primitive's first click-routed consumer (before #818,
//! `MessageList` had no `hit_test` at all, so no consumer could route a
//! click to a specific row without hand-rolling the same row-height
//! arithmetic `tui::draw_message_list` / `gtk::draw_message_list` use).
//!
//! ## Key bindings
//!
//! | Key            | Action           |
//! |----------------|------------------|
//! | `j` / Down     | Scroll down      |
//! | `k` / Up       | Scroll up        |
//! | `q` / Esc      | Quit             |

use quadraui::{
    AppLogic, Backend, Color, InteractionState, Key, MessageList, MessageListHit,
    MessageListMeasure, MessageRow, MouseButton, NamedKey, Reaction, Rect, StatusBar,
    StatusBarSegment, UiEvent, WidgetId,
};

pub struct MessageListApp {
    rows: Vec<MessageRow>,
    scroll_top: usize,
    last_click: Option<String>,
}

impl MessageListApp {
    pub fn new() -> Self {
        let rows = (0..30)
            .map(|i| {
                let fg = if i % 2 == 0 {
                    Color::rgb(220, 220, 220)
                } else {
                    Color::rgb(140, 200, 255)
                };
                MessageRow::new(format!("row {i}: quadraui MessageList demo"), fg, 0.0)
            })
            .collect();
        Self {
            rows,
            scroll_top: 0,
            last_click: None,
        }
    }

    /// Same rect `render` paints the list into — shared so `handle`'s
    /// click routing can't drift from where the primitive was actually
    /// painted (quadraui#818, matching every other demo in this module).
    fn list_rect(backend: &dyn Backend) -> Rect {
        let vp = backend.viewport();
        let lh = backend.line_height();
        Rect::new(0.0, 0.0, vp.width, (vp.height - lh).max(0.0))
    }

    fn list(&self) -> MessageList {
        MessageList {
            id: WidgetId::new("message-list-demo"),
            rows: self.rows.clone(),
            scroll_top: self.scroll_top,
        }
    }

    fn status_bar(&self) -> StatusBar {
        let msg = match &self.last_click {
            Some(m) => format!(" {m} "),
            None => " click a row — j/k scroll, q quits ".into(),
        };
        StatusBar {
            id: WidgetId::new("message-list-status"),
            left_segments: vec![StatusBarSegment {
                text: msg,
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }
}

impl Default for MessageListApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for MessageListApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        let rect = Self::list_rect(backend);
        backend.draw_message_list(rect, &self.list());

        let status_rect = Rect::new(0.0, rect.height, vp.width, vp.height - rect.height);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status_bar(),
            &InteractionState::new(),
        );
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            // Click routing (#818): resolve the click through
            // `MessageList::hit_test` — the same geometry `render` just
            // painted from — and report which row was hit.
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => {
                let rect = Self::list_rect(backend);
                let measure = MessageListMeasure::from_metrics(&backend.measure());
                self.last_click = Some(
                    match self.list().hit_test(rect, measure, position.x, position.y) {
                        MessageListHit::Row(idx) => {
                            format!("clicked row {idx}: {}", self.rows[idx].text)
                        }
                        MessageListHit::Empty => "clicked empty area".into(),
                    },
                );
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('j'),
                ..
            }
            | UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                ..
            } => {
                self.scroll_top = (self.scroll_top + 1).min(self.rows.len().saturating_sub(1));
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('k'),
                ..
            }
            | UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Up),
                ..
            } => {
                self.scroll_top = self.scroll_top.saturating_sub(1);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            }
            | UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
