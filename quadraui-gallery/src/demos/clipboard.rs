//! `Clipboard` demo — adapted from
//! `quadraui/examples/common/clipboard_demo.rs`.
//!
//! `Ctrl-C` writes the current line to the system clipboard via
//! `PlatformServices::clipboard()` (arboard, OSC 52, and a native
//! command-line tool fallback — quadraui#398, all three legs tried
//! every time); `Ctrl-V` reads it back, including a paste made from
//! *outside* this process — a full round trip a human can verify by
//! running the real TUI binary and pasting into another application.

use quadraui::{
    Backend, BackendCaps, Color, InteractionState, Key, NamedKey, Reaction, Rect, StatusBar,
    StatusBarSegment, TextInput, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("clipboard.rs");

// gallery:begin
const SEED: &str = "Copy me to the system clipboard!";

pub struct ClipboardDemo {
    input: TextInput,
    status: String,
}

impl ClipboardDemo {
    pub fn new() -> Self {
        let mut input = TextInput::new(WidgetId::new("gallery:clipboard:input"));
        input.lines = vec![SEED.to_string()];
        input.cursor_col = input.lines[0].chars().count();
        input.has_focus = true;
        Self {
            input,
            status: "Ctrl-C copies - Ctrl-V pastes".to_string(),
        }
    }

    fn line(&self) -> &str {
        self.input.lines.first().map(String::as_str).unwrap_or("")
    }

    fn cursor_byte(&self) -> usize {
        self.line()
            .char_indices()
            .nth(self.input.cursor_col)
            .map(|(b, _)| b)
            .unwrap_or_else(|| self.line().len())
    }

    fn insert_char(&mut self, ch: char) {
        if self.input.lines.is_empty() {
            self.input.lines.push(String::new());
        }
        let byte = self.cursor_byte();
        self.input.lines[0].insert(byte, ch);
        self.input.cursor_col += 1;
    }

    fn backspace(&mut self) {
        if self.input.cursor_col == 0 {
            return;
        }
        let byte = self.cursor_byte();
        let prev_byte = self
            .line()
            .char_indices()
            .nth(self.input.cursor_col - 1)
            .map(|(b, _)| b)
            .unwrap_or(0);
        self.input.lines[0].replace_range(prev_byte..byte, "");
        self.input.cursor_col -= 1;
    }

    fn status_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("gallery:clipboard:status"),
            left_segments: vec![StatusBarSegment {
                text: " Clipboard demo ".into(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: true,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.status),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for ClipboardDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for ClipboardDemo {
    fn name(&self) -> &'static str {
        "Clipboard"
    }

    fn group(&self) -> &'static str {
        "Overlays"
    }

    fn render(&self, _variant: usize, backend: &mut dyn Backend, area: Rect) {
        let lh = backend.line_height();
        let pad = lh;

        let status_rect = Rect::new(area.x, area.y + area.height - lh, area.width, lh);
        backend.draw_status_bar_interactive(
            status_rect,
            &self.status_bar(),
            &InteractionState::new(),
        );

        let input_rect = Rect::new(
            area.x + pad,
            area.y + pad,
            (area.width - pad * 2.0).max(0.0),
            lh,
        );
        backend.draw_text_input(input_rect, &self.input);
    }

    fn handle(
        &mut self,
        _variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        _area: Rect,
    ) -> Reaction {
        match event {
            // Ctrl-C: write the line to the system clipboard via all
            // three `Clipboard::write_text` legs. No active
            // text-selection is needed here — this exercises the
            // service directly.
            UiEvent::KeyPressed {
                key: Key::Char('c') | Key::Char('C'),
                modifiers,
                ..
            } if modifiers.ctrl && !modifiers.alt && !modifiers.cmd => {
                let text = self.line().to_string();
                backend.services().clipboard().write_text(&text);
                self.status = format!("Copied {} chars", text.chars().count());
                Reaction::Redraw
            }
            // Ctrl-V: read the system clipboard back — proves a round
            // trip through whichever leg actually landed the write,
            // including a paste made from outside this process.
            UiEvent::KeyPressed {
                key: Key::Char('v') | Key::Char('V'),
                modifiers,
                ..
            } if modifiers.ctrl && !modifiers.alt && !modifiers.cmd => {
                match backend.services().clipboard().read_text() {
                    Some(text) => {
                        self.input.lines = vec![text];
                        self.input.cursor_col = self.line().chars().count();
                        self.status = "Pasted from system clipboard".to_string();
                    }
                    None => self.status = "Clipboard read returned nothing".to_string(),
                }
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Backspace),
                ..
            } => {
                self.backspace();
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char(c),
                modifiers,
                ..
            } if !modifiers.ctrl && !modifiers.alt && !modifiers.cmd => {
                self.insert_char(*c);
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, _variant: usize) -> serde_json::Value {
        serde_json::json!({
            "line": self.line(),
            "status": self.status,
        })
    }

    fn caps_note(&self, _variant: usize, _caps: &BackendCaps) -> Option<String> {
        // `PlatformServices::clipboard()` has no no-op default and no
        // `BackendCaps` flag — every backend implements a real
        // read/write path (TUI's own three-leg fallback included), so
        // there's no gap to report.
        None
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_backspace_round_trip() {
        let mut demo = ClipboardDemo::new();
        let before = demo.line().to_string();
        demo.insert_char('!');
        assert_eq!(demo.line(), format!("{before}!"));
        demo.backspace();
        assert_eq!(demo.line(), before);
    }

    #[test]
    fn starts_with_the_seed_line() {
        let demo = ClipboardDemo::new();
        assert_eq!(demo.line(), SEED);
    }
}
