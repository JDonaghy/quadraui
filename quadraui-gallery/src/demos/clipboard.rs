//! `Clipboard` demo — adapted from
//! `quadraui/examples/common/clipboard_demo.rs`.
//!
//! `Ctrl-C` writes the current text to the system clipboard via
//! `PlatformServices::clipboard()` (arboard, OSC 52, and a native
//! command-line tool fallback, all three legs tried every time);
//! `Ctrl-V` reads it back, including a paste made from *outside* this
//! process — a full round trip a human can verify by running the real
//! TUI binary and pasting into another application.
//!
//! Three variants seed the field with content that exercises different
//! clipboard-encoding paths: plain ASCII, multi-line text, and Unicode
//! (accents, CJK, emoji).

use quadraui::{
    Backend, BackendCaps, Color, EditOp, InteractionState, Key, NamedKey, Reaction, Rect,
    StatusBar, StatusBarSegment, TextEditor, TextInput, UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("clipboard.rs");

// gallery:begin
const SEEDS: [&str; 3] = [
    "Copy me to the system clipboard!",
    "Line one\nLine two\nLine three",
    "café façade 日本語 🎉",
];

pub struct ClipboardDemo {
    editors: Vec<TextEditor>,
    statuses: Vec<String>,
}

impl ClipboardDemo {
    pub fn new() -> Self {
        let editors = SEEDS
            .iter()
            .enumerate()
            .map(|(i, seed)| {
                let mut input = TextInput::new(WidgetId::new(format!("gallery:clipboard:{i}")));
                input.lines = seed.lines().map(str::to_string).collect();
                input.has_focus = true;
                let last_line = input.lines.len().saturating_sub(1);
                input.cursor_line = last_line;
                input.cursor_col = input.lines.get(last_line).map_or(0, |l| l.chars().count());
                TextEditor::new(input)
            })
            .collect();
        Self {
            editors,
            statuses: vec!["Ctrl-C copies - Ctrl-V pastes".to_string(); SEEDS.len()],
        }
    }

    fn text(&self, variant: usize) -> String {
        self.editors[variant].input.lines.join("\n")
    }

    fn status_bar(&self, variant: usize) -> StatusBar {
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
                text: format!(" {} ", self.statuses[variant]),
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

    fn variants(&self) -> &'static [&'static str] {
        &["Plain text", "Multi-line", "Unicode"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let lh = backend.line_height();
        let pad = lh;

        let status_rect = Rect::new(area.x, area.y + area.height - lh, area.width, lh);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status_bar(variant),
            &InteractionState::new(),
        );

        let input_rect = Rect::new(
            area.x + pad,
            area.y + pad,
            (area.width - pad * 2.0).max(0.0),
            (area.height - pad * 2.0 - lh).max(lh),
        );
        backend.draw_text_input(input_rect, &self.editors[variant].input);
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        _area: Rect,
    ) -> Reaction {
        match event {
            // Ctrl-C: write the whole field to the system clipboard via
            // all three `Clipboard::write_text` legs. No active
            // text-selection is needed here — this exercises the
            // service directly.
            UiEvent::KeyPressed {
                key: Key::Char('c') | Key::Char('C'),
                modifiers,
                ..
            } if modifiers.ctrl && !modifiers.alt && !modifiers.cmd => {
                let text = self.text(variant);
                backend.services().clipboard().write_text(&text);
                self.statuses[variant] = format!("Copied {} chars", text.chars().count());
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
                        let count = text.chars().count();
                        self.editors[variant].apply(EditOp::MoveDocEnd { extend: false });
                        self.editors[variant].apply(EditOp::InsertText(text));
                        self.statuses[variant] = format!("Pasted {count} chars");
                    }
                    None => self.statuses[variant] = "Clipboard read returned nothing".to_string(),
                }
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Backspace),
                ..
            } => {
                self.editors[variant].apply(EditOp::DeleteBackward);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char(c),
                modifiers,
                ..
            } if !modifiers.ctrl && !modifiers.alt && !modifiers.cmd => {
                self.editors[variant].apply(EditOp::InsertChar(*c));
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
            "text": self.text(variant),
            "status": self.statuses[variant],
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
        let before = demo.text(0);
        demo.editors[0].apply(EditOp::MoveDocEnd { extend: false });
        demo.editors[0].apply(EditOp::InsertChar('!'));
        assert_eq!(demo.text(0), format!("{before}!"));
        demo.editors[0].apply(EditOp::DeleteBackward);
        assert_eq!(demo.text(0), before);
    }

    #[test]
    fn each_variant_starts_with_its_own_seed() {
        let demo = ClipboardDemo::new();
        for (i, seed) in SEEDS.iter().enumerate() {
            assert_eq!(demo.text(i), *seed);
        }
    }
}
