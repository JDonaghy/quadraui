//! `MessageList` demo — adapted from
//! `quadraui/examples/common/message_list_demo.rs`.
//!
//! Two variants: a plain scrollable row list (alternating colours) and
//! a "log" variant where rows carry severity colours (info/warn/error)
//! — the same [`MessageList::hit_test`] click routing on both. `j`/`k`
//! scroll; clicking a row reports its index and text.

use quadraui::{
    Backend, BackendCaps, Color, InteractionState, Key, MessageList, MessageListHit,
    MessageListMeasure, MessageRow, MouseButton, Reaction, Rect, StatusBar, StatusBarSegment,
    UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("message_list.rs");

// gallery:begin
pub struct MessageListDemo {
    rows: Vec<Vec<MessageRow>>,
    scroll_top: Vec<usize>,
    last_click: Vec<Option<String>>,
}

impl MessageListDemo {
    pub fn new() -> Self {
        let plain = (0..30)
            .map(|i| {
                let fg = if i % 2 == 0 {
                    Color::rgb(220, 220, 220)
                } else {
                    Color::rgb(140, 200, 255)
                };
                MessageRow::new(format!("row {i}: quadraui MessageList demo"), fg, 0.0)
            })
            .collect();
        let log = (0..30)
            .map(|i| {
                let (fg, label) = match i % 3 {
                    0 => (Color::rgb(220, 220, 220), "INFO"),
                    1 => (Color::rgb(230, 190, 60), "WARN"),
                    _ => (Color::rgb(230, 90, 90), "ERROR"),
                };
                MessageRow::new(format!("[{label}] event #{i} processed"), fg, 0.0)
            })
            .collect();
        Self {
            rows: vec![plain, log],
            scroll_top: vec![0, 0],
            last_click: vec![None, None],
        }
    }

    fn list_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0))
    }

    fn status_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y + area.height - lh, area.width, lh)
    }

    fn list(&self, variant: usize) -> MessageList {
        MessageList {
            id: WidgetId::new("gallery:message-list"),
            rows: self.rows[variant].clone(),
            scroll_top: self.scroll_top[variant],
        }
    }

    fn status(&self, variant: usize) -> StatusBar {
        let msg = match &self.last_click[variant] {
            Some(m) => format!(" {m} "),
            None => " click a row — j/k scroll ".into(),
        };
        StatusBar {
            id: WidgetId::new("gallery:message-list:status"),
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

impl Default for MessageListDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for MessageListDemo {
    fn name(&self) -> &'static str {
        "Message List"
    }

    fn group(&self) -> &'static str {
        "Data"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Plain rows", "Severity log"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let rect = Self::list_rect(area, backend);
        backend.draw_message_list(rect, &self.list(variant));

        let status_rect = Self::status_rect(area, backend);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status(variant),
            &InteractionState::new(),
        );
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        match event {
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => {
                let rect = Self::list_rect(area, backend);
                let measure = MessageListMeasure::from_metrics(&backend.measure());
                let list = self.list(variant);
                self.last_click[variant] =
                    Some(match list.hit_test(rect, measure, position.x, position.y) {
                        MessageListHit::Row(idx) => {
                            format!("clicked row {idx}: {}", self.rows[variant][idx].text)
                        }
                        MessageListHit::Empty => "clicked empty area".into(),
                    });
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('j'),
                ..
            } => {
                let max = self.rows[variant].len().saturating_sub(1);
                self.scroll_top[variant] = (self.scroll_top[variant] + 1).min(max);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('k'),
                ..
            } => {
                self.scroll_top[variant] = self.scroll_top[variant].saturating_sub(1);
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        serde_json::json!({
            "row_count": self.rows[variant].len(),
            "scroll_top": self.scroll_top[variant],
            "last_click": self.last_click[variant],
        })
    }

    fn caps_note(&self, _variant: usize, _caps: &BackendCaps) -> Option<String> {
        None
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_variant_labels_rows_by_index_mod_three() {
        let demo = MessageListDemo::new();
        assert!(demo.rows[1][0].text.contains("INFO"));
        assert!(demo.rows[1][1].text.contains("WARN"));
        assert!(demo.rows[1][2].text.contains("ERROR"));
    }

    #[test]
    fn j_key_advances_scroll_top() {
        let mut demo = MessageListDemo::new();
        let mut backend = quadraui::testing::RecordingBackend::new();
        let area = Rect::new(0.0, 0.0, 60.0, 20.0);
        let event = UiEvent::KeyPressed {
            key: Key::Char('j'),
            modifiers: Default::default(),
            repeat: false,
        };
        let reaction = demo.handle(0, &event, &mut backend, area);
        assert!(matches!(reaction, Reaction::Redraw));
        assert_eq!(demo.scroll_top[0], 1);
    }
}

#[cfg(all(test, feature = "tui"))]
mod tui_tests {
    use super::*;

    #[test]
    fn clicking_the_first_row_reports_its_text() {
        let mut demo = MessageListDemo::new();
        let mut backend = quadraui::tui::TuiBackend::new();
        let area = Rect::new(0.0, 0.0, 60.0, 20.0);
        let rect = MessageListDemo::list_rect(area, &backend);
        let event = UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: quadraui::Point::new(rect.x, rect.y),
            modifiers: Default::default(),
        };
        let reaction = demo.handle(0, &event, &mut backend, area);
        assert!(matches!(reaction, Reaction::Redraw));
        assert!(demo.last_click[0]
            .as_ref()
            .unwrap()
            .contains("quadraui MessageList demo"));
    }
}
