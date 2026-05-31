//! Backend-agnostic `AppLogic` for the AI-chat-transcript demo
//! ([`tui_ai_transcript`] / [`gtk_ai_transcript`]).
//!
//! This is the **headless-chat-server use case**: an upstream produces
//! markdown text (an AI assistant's reply) and the UI must render it as
//! styled, scrolling output. The demo wires the existing
//! [`render_markdown_to_styled`] adapter straight into a [`TextDisplay`]
//! — the append-only, auto-scrolling log primitive — instead of the
//! modal [`RichTextPopup`] used by `tui_markdown`. `TextDisplay` is the
//! better fit for a streaming transcript: lines arrive over time, the
//! view pins to the newest line, and a scrollbar lets the user page back.
//!
//! The bridge is tiny and is the reusable pattern a real chat client
//! would copy: [`markdown_to_display_lines`] maps each
//! `RenderedMarkdown` line's spans onto a [`TextDisplayLine`]. No new
//! primitive and no new `Backend` method — the same `AppLogic` drives
//! every backend.
//!
//! To make the streaming visible (and to mimic a server that emits a
//! reply token-by-token) the assistant's lines are queued and revealed
//! one per `tick`, after a short "thinking" delay.
//!
//! The canned replies deliberately include markdown the adapter does
//! **not** yet style — bulleted/numbered lists, fenced code blocks,
//! links — so the demo doubles as a visual record of the gap tracked in
//! the follow-up issue. Those lines render as plain (but readable) text.
//!
//! Controls:
//! - `Enter` / `s` — send the next canned prompt (cycles through a few).
//! - `↑` / `↓` — scroll one line (pauses auto-scroll).
//! - `PageUp` / `PageDown` — scroll a page (pauses auto-scroll).
//! - `End` — jump to the newest line and re-enable auto-scroll.
//! - `q` / `Esc` — quit.

use std::collections::VecDeque;

use quadraui::{
    render_markdown_to_styled, AppLogic, Backend, Color, Decoration, Key, NamedKey, Reaction, Rect,
    StyledSpan, StyledText, TextDisplay, TextDisplayLine, Theme, UiEvent, WidgetId,
};

/// Map a markdown string to a list of [`TextDisplayLine`]s via the
/// quadraui markdown adapter.
///
/// This is the headless-chat-server bridge: hand it the assistant's raw
/// markdown reply, get back styled lines ready to `append_line` into a
/// [`TextDisplay`]. `decoration` tags every produced line (e.g.
/// [`Decoration::Muted`] for system notes); inline bold/italic/code from
/// the markdown is preserved on the spans.
///
/// Per-line heading *scale* (`RenderedMarkdown::line_scales`) is dropped
/// here: `TextDisplay` has no per-line font-size knob (neither does the
/// TUI backend), so headings keep their bold weight but not larger type.
fn markdown_to_display_lines(
    md: &str,
    theme: &Theme,
    decoration: Decoration,
) -> Vec<TextDisplayLine> {
    render_markdown_to_styled(md, theme)
        .lines
        .into_iter()
        .map(|StyledText { spans }| TextDisplayLine {
            spans,
            decoration,
            timestamp: None,
        })
        .collect()
}

/// Canned (prompt, markdown-reply) pairs cycled through on each send.
/// The replies mix adapter-supported syntax (headings, **bold**,
/// *italic*, `code`) with not-yet-styled syntax (lists, fenced blocks,
/// links) so the demo shows both the feature and the gap.
const EXCHANGES: &[(&str, &str)] = &[
    (
        "How do I convert markdown to styled text?",
        "\
## Markdown → StyledText
Call `render_markdown_to_styled(input, &theme)`. It returns a
`RenderedMarkdown` with three length-aligned vectors:

- `lines` — one **StyledText** per source line
- `line_text` — plain text, for hit-tests and search
- `line_scales` — heading scale factors

Then feed `lines` into a `TextDisplay` for a *scrolling* transcript.",
    ),
    (
        "What markdown does it support today?",
        "\
# Supported
Inline **bold**, *italic*/_also_, and `inline_code` all render. Headings
`#`/`##`/`###` scale up in GTK.

# Not yet (the gap)
1. Bulleted and numbered lists
2. Fenced code blocks:
```rust
fn main() { println!(\"hi\"); }
```
3. Links like [quadraui](https://example.com), blockquotes, tables, images.

These pass through as plain text for now.",
    ),
    (
        "Anything else I should know?",
        "\
### Notes
Identifiers like foo_bar and baz_qux stay upright — the adapter honours
CommonMark flanking, so `snake_case` is **never** mistaken for emphasis.
Arithmetic like a * b * c is left alone too.",
    ),
];

/// Demo app rendering an AI chat transcript into a [`TextDisplay`].
pub struct AiTranscript {
    td: TextDisplay,
    /// Index of the next canned exchange to send.
    next_exchange: usize,
    /// Lines waiting to be revealed (simulating a streamed reply).
    pending: VecDeque<TextDisplayLine>,
    /// Ticks remaining before the queued reply starts streaming.
    thinking_ticks: usize,
}

impl AiTranscript {
    pub fn new() -> Self {
        let mut td = TextDisplay::new(WidgetId::new("ai:transcript"));
        td.title = Some(StyledText::plain(
            "AI transcript — Enter to send · ↑/↓/PgUp/PgDn scroll · End follow · q quit",
        ));
        td.show_scrollbar = true;
        td.auto_scroll = true;
        // Cap retention like a real long-running chat session.
        td.set_max_lines(2000);

        let mut app = Self {
            td,
            next_exchange: 0,
            pending: VecDeque::new(),
            thinking_ticks: 0,
        };
        // Seed with a greeting so the view isn't empty on launch.
        app.append_system("Connected to headless chat server. Ask away.");
        app
    }

    /// Append a dim system note (its own styled line, no markdown).
    fn append_system(&mut self, text: &str) {
        self.td.append_line(TextDisplayLine {
            spans: vec![StyledSpan::plain(text)],
            decoration: Decoration::Muted,
            timestamp: None,
        });
    }

    /// Append the user's prompt as a single highlighted line.
    fn append_user(&mut self, text: &str) {
        self.td.append_line(TextDisplayLine {
            spans: vec![StyledSpan {
                text: format!("You › {text}"),
                fg: Some(Color::rgb(120, 190, 255)),
                bg: None,
                bold: true,
                italic: false,
                underline: false,
            }],
            decoration: Decoration::Normal,
            timestamp: None,
        });
    }

    /// Send the next canned prompt: echo the user turn and queue the
    /// markdown reply to stream in over the next ticks.
    fn send_next(&mut self) {
        // Don't start a new turn mid-stream.
        if self.thinking_ticks > 0 || !self.pending.is_empty() {
            return;
        }
        let (prompt, reply_md) = EXCHANGES[self.next_exchange % EXCHANGES.len()];
        self.next_exchange += 1;

        self.append_user(prompt);
        // Re-enable follow so the streamed reply stays in view.
        self.td.auto_scroll = true;

        let theme = Theme::default();
        let mut lines = markdown_to_display_lines(reply_md, &theme, Decoration::Normal);
        self.pending.extend(lines.drain(..));
        self.thinking_ticks = 3;
    }
}

impl Default for AiTranscript {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for AiTranscript {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        let rect = Rect::new(0.0, 0.0, vp.width, vp.height);
        backend.draw_text_display(rect, &self.td);
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed { key, .. } => match key {
                Key::Char('q') | Key::Named(NamedKey::Escape) => Reaction::Exit,
                Key::Named(NamedKey::Enter) | Key::Char('s') => {
                    self.send_next();
                    Reaction::Redraw
                }
                Key::Named(NamedKey::Up) => {
                    self.td.auto_scroll = false;
                    self.td.scroll_offset = self.td.scroll_offset.saturating_sub(1);
                    Reaction::Redraw
                }
                Key::Named(NamedKey::Down) => {
                    self.td.auto_scroll = false;
                    self.td.scroll_offset = self.td.scroll_offset.saturating_add(1);
                    Reaction::Redraw
                }
                Key::Named(NamedKey::PageUp) => {
                    self.td.auto_scroll = false;
                    self.td.scroll_offset = self.td.scroll_offset.saturating_sub(10);
                    Reaction::Redraw
                }
                Key::Named(NamedKey::PageDown) => {
                    self.td.auto_scroll = false;
                    self.td.scroll_offset = self.td.scroll_offset.saturating_add(10);
                    Reaction::Redraw
                }
                Key::Named(NamedKey::End) => {
                    self.td.auto_scroll = true;
                    Reaction::Redraw
                }
                _ => Reaction::Continue,
            },
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }

    fn tick(&mut self, _backend: &mut dyn Backend) -> Reaction {
        if self.thinking_ticks > 0 {
            self.thinking_ticks -= 1;
            return Reaction::Redraw;
        }
        // Reveal one queued reply line per tick to simulate streaming.
        if let Some(line) = self.pending.pop_front() {
            self.td.append_line(line);
            return Reaction::Redraw;
        }
        Reaction::Continue
    }
}
