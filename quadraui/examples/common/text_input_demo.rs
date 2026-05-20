//! TextInput demo — visual smoke test for the new TextInput primitive.
//!
//! Renders a single multi-line `TextInput` filling most of the viewport.
//! Implements minimal editing (insert, backspace, arrow keys, Enter,
//! Home/End) so you can verify cursor positioning, line wrap on Enter,
//! scroll auto-clamp, and placeholder rendering.

use quadraui::{
    AppLogic, Backend, Color, Key, NamedKey, Reaction, Rect, StatusBar, StatusBarSegment,
    TextInput, UiEvent, WidgetId,
};

pub struct TextInputDemo {
    input: TextInput,
}

impl TextInputDemo {
    pub fn new() -> Self {
        let mut input = TextInput::new(WidgetId::new("demo:input"));
        input.placeholder =
            Some("Type something. Enter for newline. Arrows/Home/End to move. Esc to quit.".into());
        input.has_focus = true;
        Self { input }
    }

    fn cursor_byte(&self) -> usize {
        let line = self
            .input
            .lines
            .get(self.input.cursor_line)
            .map(String::as_str)
            .unwrap_or("");
        line.char_indices()
            .nth(self.input.cursor_col)
            .map(|(b, _)| b)
            .unwrap_or(line.len())
    }

    fn insert_char(&mut self, ch: char) {
        if self.input.lines.is_empty() {
            self.input.lines.push(String::new());
        }
        let byte = self.cursor_byte();
        let line = &mut self.input.lines[self.input.cursor_line];
        line.insert(byte, ch);
        self.input.cursor_col += 1;
    }

    fn insert_newline(&mut self) {
        let byte = self.cursor_byte();
        let line = self.input.lines[self.input.cursor_line].clone();
        let (head, tail) = line.split_at(byte);
        self.input.lines[self.input.cursor_line] = head.to_string();
        self.input
            .lines
            .insert(self.input.cursor_line + 1, tail.to_string());
        self.input.cursor_line += 1;
        self.input.cursor_col = 0;
    }

    fn backspace(&mut self) {
        if self.input.cursor_col > 0 {
            let byte = self.cursor_byte();
            let line = &mut self.input.lines[self.input.cursor_line];
            // Find byte boundary for the char before cursor.
            let prev_byte = line
                .char_indices()
                .nth(self.input.cursor_col - 1)
                .map(|(b, _)| b)
                .unwrap_or(0);
            line.replace_range(prev_byte..byte, "");
            self.input.cursor_col -= 1;
        } else if self.input.cursor_line > 0 {
            // Merge with previous line.
            let current = self.input.lines.remove(self.input.cursor_line);
            self.input.cursor_line -= 1;
            let prev = &mut self.input.lines[self.input.cursor_line];
            self.input.cursor_col = prev.chars().count();
            prev.push_str(&current);
        }
    }

    fn move_left(&mut self) {
        if self.input.cursor_col > 0 {
            self.input.cursor_col -= 1;
        } else if self.input.cursor_line > 0 {
            self.input.cursor_line -= 1;
            self.input.cursor_col = self.input.lines[self.input.cursor_line].chars().count();
        }
    }

    fn move_right(&mut self) {
        let line_len = self.input.lines[self.input.cursor_line].chars().count();
        if self.input.cursor_col < line_len {
            self.input.cursor_col += 1;
        } else if self.input.cursor_line + 1 < self.input.lines.len() {
            self.input.cursor_line += 1;
            self.input.cursor_col = 0;
        }
    }

    fn move_up(&mut self) {
        if self.input.cursor_line > 0 {
            self.input.cursor_line -= 1;
            let line_len = self.input.lines[self.input.cursor_line].chars().count();
            self.input.cursor_col = self.input.cursor_col.min(line_len);
        }
    }

    fn move_down(&mut self) {
        if self.input.cursor_line + 1 < self.input.lines.len() {
            self.input.cursor_line += 1;
            let line_len = self.input.lines[self.input.cursor_line].chars().count();
            self.input.cursor_col = self.input.cursor_col.min(line_len);
        }
    }

    fn move_home(&mut self) {
        self.input.cursor_col = 0;
    }

    fn move_end(&mut self) {
        self.input.cursor_col = self.input.lines[self.input.cursor_line].chars().count();
    }

    fn status(&self) -> StatusBar {
        let cursor = format!(
            " line {} col {} — Esc to quit ",
            self.input.cursor_line + 1,
            self.input.cursor_col + 1,
        );
        StatusBar {
            id: WidgetId::new("demo:status"),
            left_segments: vec![StatusBarSegment {
                text: " TextInput demo ".into(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: true,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: cursor,
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for TextInputDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for TextInputDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let status_h = lh;
        let pad = lh;

        let status_rect = Rect::new(0.0, viewport.height - status_h, viewport.width, status_h);
        backend.draw_status_bar(status_rect, &self.status(), None, None);

        let input_rect = Rect::new(
            pad,
            pad,
            viewport.width - pad * 2.0,
            viewport.height - status_h - pad * 2.0,
        );
        backend.draw_text_input(input_rect, &self.input);
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed { key, .. } => match key {
                Key::Named(NamedKey::Escape) => Reaction::Exit,
                Key::Named(NamedKey::Enter) => {
                    self.insert_newline();
                    Reaction::Redraw
                }
                Key::Named(NamedKey::Backspace) => {
                    self.backspace();
                    Reaction::Redraw
                }
                Key::Named(NamedKey::Left) => {
                    self.move_left();
                    Reaction::Redraw
                }
                Key::Named(NamedKey::Right) => {
                    self.move_right();
                    Reaction::Redraw
                }
                Key::Named(NamedKey::Up) => {
                    self.move_up();
                    Reaction::Redraw
                }
                Key::Named(NamedKey::Down) => {
                    self.move_down();
                    Reaction::Redraw
                }
                Key::Named(NamedKey::Home) => {
                    self.move_home();
                    Reaction::Redraw
                }
                Key::Named(NamedKey::End) => {
                    self.move_end();
                    Reaction::Redraw
                }
                Key::Char(c) => {
                    self.insert_char(c);
                    Reaction::Redraw
                }
                _ => Reaction::Continue,
            },
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
