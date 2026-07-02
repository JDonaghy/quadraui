//! Clipboard demo — manual smoke test for the native-clipboard-tool
//! fallback leg added to `TuiClipboard::write_text` (#398).
//!
//! `TuiClipboard::write_text` now writes via three independent legs:
//! arboard, OSC 52, and (new) a native command-line tool (`wl-copy` /
//! `xclip` / `xsel`). The bug this closes only reproduces on a local
//! X11 (or Wayland) session running *inside* an outer tmux — run this
//! demo there to verify a copy actually lands in the real system
//! clipboard:
//!
//! ```sh
//! tmux new -s clip-test
//! cargo run --example tui_clipboard --features tui
//! ```
//!
//! Press Ctrl-C to copy the line to the system clipboard, then — in
//! another pane/window — run `xclip -o -selection clipboard` (X11) or
//! `wl-paste` (Wayland) and confirm it prints the copied text. Ctrl-V
//! reads the system clipboard back into the app so a full round trip
//! (including a paste from *outside* this process, e.g. from a
//! browser) can be verified too.

use quadraui::{
    AppLogic, Backend, Color, Key, NamedKey, Reaction, Rect, StatusBar, StatusBarSegment,
    TextInput, UiEvent, WidgetId,
};

pub struct ClipboardDemo {
    input: TextInput,
    status: String,
}

impl ClipboardDemo {
    pub fn new() -> Self {
        let mut input = TextInput::new(WidgetId::new("clipboard-demo:input"));
        input.lines = vec!["Copy me to the system clipboard!".to_string()];
        input.cursor_col = input.lines[0].chars().count();
        input.has_focus = true;
        Self {
            input,
            status: "Ctrl-C copies · Ctrl-V pastes · Esc quits".to_string(),
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

    fn status(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("clipboard-demo:status"),
            left_segments: vec![StatusBarSegment {
                text: " Clipboard demo (#398) ".into(),
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

impl AppLogic for ClipboardDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let status_h = lh;
        let pad = lh;

        let status_rect = Rect::new(0.0, viewport.height - status_h, viewport.width, status_h);
        backend.draw_status_bar(status_rect, &self.status(), None, None);

        let input_rect = Rect::new(pad, pad, viewport.width - pad * 2.0, lh);
        backend.draw_text_input(input_rect, &self.input);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            // Ctrl-C: write the line to the system clipboard via all
            // three `TuiClipboard::write_text` legs (arboard, OSC 52,
            // native tool). No active text-selection is needed here —
            // this exercises the service directly, the same call the
            // runner's selection-copy path makes internally.
            UiEvent::KeyPressed {
                key: Key::Char('c') | Key::Char('C'),
                modifiers,
                ..
            } if modifiers.ctrl && !modifiers.alt && !modifiers.cmd => {
                let text = self.line().to_string();
                backend.services().clipboard().write_text(&text);
                self.status = format!("Copied {} chars — check `xclip -o -selection clipboard` (or wl-paste) in another pane", text.chars().count());
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
                self.insert_char(c);
                Reaction::Redraw
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
