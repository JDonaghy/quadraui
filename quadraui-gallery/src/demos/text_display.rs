//! `Text Display` demo — adapted from
//! `quadraui/examples/common/text_display_wrap_demo.rs`.
//!
//! `TextDisplay` is the scrollable, append-only log viewer (distinct
//! from `Terminal`, which is VT100-aware, and `Editor`, which has a
//! cursor). Two variants cover its range: a decorated log tail (per-line
//! severity tint + timestamp, manual scroll disabling auto-scroll) and
//! an over-wide line that must word-wrap rather than silently truncate.

use quadraui::{
    Backend, Color, Decoration, InteractionState, Key, NamedKey, Reaction, Rect, StatusBar,
    StatusBarSegment, StyledSpan, TextDisplay, TextDisplayLine, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("text_display.rs");

// gallery:begin
/// Distinctive word placed at the very end of the long line in the
/// word-wrap variant. Never painted anywhere on screen if `TextDisplay`
/// truncates instead of wrapping.
const TAIL_MARKER: &str = "TAILMARKER";

fn long_line_text() -> String {
    let mut s = String::from(
        "This one line of prose is deliberately far wider than the viewport \
         so that it cannot possibly fit on a single row no matter how the \
         backend measures character width, which is exactly the scenario \
         a pane that silently truncated long lines at its right edge \
         instead of wrapping them onto continuation rows the reader could \
         actually scroll to and read, ",
    );
    s.push_str(TAIL_MARKER);
    s
}

fn log_lines() -> Vec<TextDisplayLine> {
    vec![
        TextDisplayLine {
            spans: vec![StyledSpan::plain("server started on :8080")],
            decoration: Decoration::Normal,
            timestamp: Some("12:00:00".into()),
        },
        TextDisplayLine {
            spans: vec![StyledSpan::plain("accepted connection from 10.0.0.4")],
            decoration: Decoration::Normal,
            timestamp: Some("12:00:02".into()),
        },
        TextDisplayLine {
            spans: vec![StyledSpan::plain("slow query: 840ms")],
            decoration: Decoration::Warning,
            timestamp: Some("12:00:05".into()),
        },
        TextDisplayLine {
            spans: vec![StyledSpan::plain("connection reset by peer")],
            decoration: Decoration::Error,
            timestamp: Some("12:00:06".into()),
        },
        TextDisplayLine {
            spans: vec![StyledSpan::plain("retry succeeded")],
            decoration: Decoration::Muted,
            timestamp: Some("12:00:07".into()),
        },
    ]
}

fn wrap_lines() -> Vec<TextDisplayLine> {
    vec![
        TextDisplayLine {
            spans: vec![StyledSpan::plain(long_line_text())],
            decoration: Decoration::Normal,
            timestamp: None,
        },
        TextDisplayLine {
            spans: vec![StyledSpan::plain("A short line follows.")],
            decoration: Decoration::Normal,
            timestamp: None,
        },
    ]
}

pub struct TextDisplayDemo {
    scroll_offset: usize,
    auto_scroll: bool,
}

impl TextDisplayDemo {
    pub fn new() -> Self {
        Self {
            scroll_offset: 0,
            auto_scroll: true,
        }
    }

    fn build(&self, variant: usize) -> TextDisplay {
        let lines = if variant == 1 {
            wrap_lines()
        } else {
            log_lines()
        };
        let mut display = TextDisplay::new(WidgetId::new("gallery:text-display"));
        display.lines = lines;
        display.scroll_offset = self.scroll_offset;
        display.auto_scroll = self.auto_scroll;
        display.show_scrollbar = true;
        display
    }
}

impl Default for TextDisplayDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for TextDisplayDemo {
    fn name(&self) -> &'static str {
        "Text Display"
    }

    fn group(&self) -> &'static str {
        "Content"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Log tail (decorations)", "Word-wrap (long line)"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let lh = backend.line_height();
        let body = Rect::new(area.x, area.y, area.width, (area.height - lh).max(0.0));
        let display = self.build(variant);
        backend.draw_text_display(body, &display);

        let status_rect = Rect::new(area.x, area.y + body.height, area.width, lh);
        let text = format!(
            " auto_scroll: {}  scroll_offset: {}  (k/j or \u{2191}/\u{2193} scroll, G return to live) ",
            self.auto_scroll, self.scroll_offset
        );
        let bar = StatusBar {
            id: WidgetId::new("gallery:text-display:status"),
            left_segments: vec![StatusBarSegment {
                text,
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        };
        let _ = backend.draw_status_bar_interactive(status_rect, &bar, &InteractionState::new());
    }

    fn handle(
        &mut self,
        _variant: usize,
        event: &UiEvent,
        _backend: &mut dyn Backend,
        _area: Rect,
    ) -> Reaction {
        match event {
            UiEvent::KeyPressed { key, .. } => match key {
                Key::Char('j') | Key::Named(NamedKey::Down) => {
                    self.scroll_offset = self.scroll_offset.saturating_add(1);
                    self.auto_scroll = false;
                    Reaction::Redraw
                }
                Key::Char('k') | Key::Named(NamedKey::Up) => {
                    self.scroll_offset = self.scroll_offset.saturating_sub(1);
                    self.auto_scroll = false;
                    Reaction::Redraw
                }
                Key::Char('G') | Key::Named(NamedKey::End) => {
                    self.auto_scroll = true;
                    Reaction::Redraw
                }
                _ => Reaction::Continue,
            },
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        serde_json::json!({
            "variant": variant,
            "scroll_offset": self.scroll_offset,
            "auto_scroll": self.auto_scroll,
            "line_count": self.build(variant).lines.len(),
        })
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn down_key_scrolls_and_disables_auto_scroll() {
        let mut demo = TextDisplayDemo::new();
        let mut backend = quadraui::testing::RecordingBackend::new();
        assert!(demo.auto_scroll);
        let reaction = demo.handle(
            0,
            &UiEvent::KeyPressed {
                key: Key::Char('j'),
                modifiers: Default::default(),
                repeat: false,
            },
            &mut backend,
            Rect::new(0.0, 0.0, 40.0, 10.0),
        );
        assert!(matches!(reaction, Reaction::Redraw));
        assert_eq!(demo.scroll_offset, 1);
        assert!(!demo.auto_scroll);
    }

    #[test]
    fn shift_g_returns_to_live_view() {
        let mut demo = TextDisplayDemo::new();
        let mut backend = quadraui::testing::RecordingBackend::new();
        demo.auto_scroll = false;
        let reaction = demo.handle(
            0,
            &UiEvent::KeyPressed {
                key: Key::Char('G'),
                modifiers: Default::default(),
                repeat: false,
            },
            &mut backend,
            Rect::new(0.0, 0.0, 40.0, 10.0),
        );
        assert!(matches!(reaction, Reaction::Redraw));
        assert!(demo.auto_scroll);
    }

    /// Sanity check on the fixture itself — that `TAIL_MARKER` actually
    /// ends up in the long line `render(1)` hands the backend. The
    /// property worth testing is whether `TextDisplay` actually *wraps*
    /// that line onto a continuation row rather than truncating it
    /// (`TAIL_MARKER`'s own doc) — that needs a painted screen to
    /// observe, which is what
    /// `text_display_wrap_variant_scrolls_the_tail_marker_into_view` in
    /// `tests/gallery_driver.rs` checks.
    #[test]
    fn long_line_text_ends_with_the_tail_marker() {
        let demo = TextDisplayDemo::new();
        let display = demo.build(1);
        let joined: String = display
            .lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.text.clone())
            .collect();
        assert!(joined.contains(TAIL_MARKER));
    }
}
