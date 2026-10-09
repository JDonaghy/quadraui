//! `Chat` demo — adapted from `quadraui/examples/common/chat_demo.rs` and
//! `ai_transcript.rs`.
//!
//! Both source files drive a [`ChatController`]; the two variants below
//! keep both usages since they exercise different parts of its surface:
//! a fully interactive session (submit, simulated "thinking" reply,
//! collapsible turns) and [`ChatController::push_turn_markdown`] (the
//! recommended way to render assistant replies written in markdown).

use std::time::Duration;

use quadraui::{
    Backend, ChatController, ChatControllerEvent, ChatRole, ChatTurn, Color, Reaction, Rect,
    StyledText, Theme, UiEvent,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("chat.rs");

// gallery:begin
/// Cadence for the "thinking" spinner's animation frame while
/// `thinking_ticks > 0` — see [`InteractiveState::tick`].
const THINKING_TICK_INTERVAL: Duration = Duration::from_millis(100);

/// Canned (prompt, markdown-reply) pairs cycled through by the
/// "Markdown transcript" variant.
const EXCHANGES: &[(&str, &str)] = &[
    (
        "How do I convert markdown to styled text?",
        "\
## Markdown \u{2192} StyledText

Call `render_markdown_to_styled(input, &theme)`. It returns:

- `lines` — one **StyledText** per source line
- `line_text` — plain text, for hit-tests and search

> `snake_case` identifiers like render_markdown_to_styled stay upright.",
    ),
    (
        "What markdown does it support today?",
        "\
# Supported constructs

Inline **bold**, *italic*, and `inline_code` all render with correct spans.

1. Numbered items render with a styled marker
2. Both dash and asterisk bullet syntax work

> This is a blockquote with a `\u{2502}` left-bar decoration.",
    ),
];

/// Variant 0 state: interactive session — echoes the user's message back
/// as a simulated assistant reply after a short "thinking" delay.
struct InteractiveState {
    controller: ChatController,
    turns: Vec<ChatTurn>,
    thinking_ticks: usize,
    pending_reply: Option<String>,
    spinner_frame: usize,
}

impl InteractiveState {
    fn new() -> Self {
        let mut controller = ChatController::new("gallery:chat:interactive");
        controller.set_status(StyledText::plain(
            "Enter to send, Ctrl+C to quit demo input",
        ));
        controller.set_model_label("claude-opus-4-5");
        controller.set_submit_on_enter(true);
        controller.set_hint(Some(StyledText::plain(
            "Enter to send \u{b7} Shift+Enter for a newline",
        )));
        Self {
            controller,
            turns: Vec::new(),
            thinking_ticks: 0,
            pending_reply: None,
            spinner_frame: 0,
        }
    }

    fn sync(&mut self) {
        self.controller.set_transcript(self.turns.clone());
        self.controller.set_busy(self.thinking_ticks > 0);
    }

    fn handle(&mut self, event: &UiEvent, backend: &mut dyn Backend, area: Rect) -> Reaction {
        let ev = self.controller.handle(event, backend, area);
        match ev {
            ChatControllerEvent::Submit { text } => {
                self.turns.push(ChatTurn {
                    role: ChatRole::User,
                    text: StyledText::colored(text.clone(), Color::rgb(220, 220, 220)),
                    timestamp_unix: None,
                    line_scales: Vec::new(),
                });
                self.controller.clear_input();
                self.pending_reply = Some(format!("Echo: {text}"));
                self.thinking_ticks = 5;
                backend.request_frame_in(THINKING_TICK_INTERVAL);
                self.sync();
                Reaction::Redraw
            }
            ChatControllerEvent::StopRequested => {
                self.thinking_ticks = 0;
                self.pending_reply = None;
                self.sync();
                Reaction::Redraw
            }
            ChatControllerEvent::TurnClicked {
                turn_idx,
                row_in_turn: 0,
            } => {
                self.controller.toggle_turn_collapsed(turn_idx);
                self.sync();
                Reaction::Redraw
            }
            ChatControllerEvent::Consumed => Reaction::Redraw,
            _ => {
                if matches!(event, UiEvent::WindowResized { .. }) {
                    Reaction::Redraw
                } else {
                    Reaction::Continue
                }
            }
        }
    }

    fn tick(&mut self, backend: &mut dyn Backend) -> Reaction {
        if self.thinking_ticks == 0 {
            return Reaction::Continue;
        }
        self.thinking_ticks -= 1;
        self.spinner_frame = self.spinner_frame.wrapping_add(1);
        self.controller.set_spinner_frame(self.spinner_frame);
        if self.thinking_ticks == 0 {
            if let Some(reply) = self.pending_reply.take() {
                self.turns.push(ChatTurn {
                    role: ChatRole::Assistant,
                    text: StyledText::colored(reply, Color::rgb(180, 230, 180)),
                    timestamp_unix: None,
                    line_scales: Vec::new(),
                });
            }
        } else {
            backend.request_frame_in(THINKING_TICK_INTERVAL);
        }
        self.sync();
        Reaction::Redraw
    }
}

/// Variant 1 state: canned markdown exchanges pushed via
/// `push_turn_markdown`.
struct TranscriptState {
    controller: ChatController,
    next_exchange: usize,
}

impl TranscriptState {
    fn new() -> Self {
        let mut controller = ChatController::new("gallery:chat:transcript");
        controller.set_status(StyledText::plain(
            "AI transcript \u{b7} Ctrl+S to send \u{b7} PgUp/PgDn scroll",
        ));
        controller.set_scrollbar_width(Some(1.0));
        controller.push_turn(
            ChatRole::System,
            StyledText::plain("Connected. Press Ctrl+S or Alt+Enter to send the next prompt."),
        );
        Self {
            controller,
            next_exchange: 0,
        }
    }

    fn send_next(&mut self) {
        let (prompt, reply_md) = EXCHANGES[self.next_exchange % EXCHANGES.len()];
        self.next_exchange += 1;
        self.controller
            .push_turn(ChatRole::User, StyledText::plain(prompt));
        let theme = Theme::default();
        self.controller
            .push_turn_markdown(ChatRole::Assistant, reply_md, &theme);
    }

    /// Total turns pushed so far: the one `System` turn from [`Self::new`]
    /// plus a (user, assistant) pair per completed [`Self::send_next`]
    /// call. `ChatController` has no public turn-count accessor to read
    /// this back from directly, so it's derived here instead of (wrongly)
    /// reporting `transcript_scroll_top()` as if it were one.
    fn turn_count(&self) -> usize {
        1 + self.next_exchange * 2
    }

    fn handle(&mut self, event: &UiEvent, backend: &mut dyn Backend, area: Rect) -> Reaction {
        match self.controller.handle(event, backend, area) {
            ChatControllerEvent::Submit { .. } => {
                self.send_next();
                Reaction::Redraw
            }
            ChatControllerEvent::Consumed => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}

pub struct ChatDemo {
    interactive: InteractiveState,
    transcript: TranscriptState,
}

impl ChatDemo {
    pub fn new() -> Self {
        Self {
            interactive: InteractiveState::new(),
            transcript: TranscriptState::new(),
        }
    }
}

impl Default for ChatDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for ChatDemo {
    fn name(&self) -> &'static str {
        "Chat"
    }

    fn group(&self) -> &'static str {
        "Content"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Interactive", "Markdown transcript"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        match variant {
            1 => self.transcript.controller.render(backend, area),
            _ => self.interactive.controller.render(backend, area),
        }
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        match variant {
            1 => self.transcript.handle(event, backend, area),
            _ => self.interactive.handle(event, backend, area),
        }
    }

    fn tick(&mut self, variant: usize, backend: &mut dyn Backend) -> Reaction {
        match variant {
            1 => Reaction::Continue,
            _ => self.interactive.tick(backend),
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        match variant {
            1 => serde_json::json!({
                "turns": self.transcript.turn_count(),
                "next_exchange": self.transcript.next_exchange,
            }),
            _ => serde_json::json!({
                "turns": self.interactive.turns.len(),
                "thinking_ticks": self.interactive.thinking_ticks,
                "input": self.interactive.controller.input_text(),
            }),
        }
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;
    use quadraui::Key;

    #[test]
    fn transcript_submit_pushes_the_next_canned_exchange() {
        let mut demo = ChatDemo::new();
        let mut backend = quadraui::testing::RecordingBackend::new();
        let area = Rect::new(0.0, 0.0, 60.0, 20.0);
        assert_eq!(demo.transcript.next_exchange, 0);
        // `ChatController::try_submit` ignores Ctrl+S on an empty input
        // buffer — type a character first, same as a real user would.
        demo.handle(1, &UiEvent::CharTyped('?'), &mut backend, area);
        let reaction = demo.handle(
            1,
            &UiEvent::KeyPressed {
                key: Key::Char('s'),
                modifiers: quadraui::Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
                repeat: false,
            },
            &mut backend,
            area,
        );
        assert!(matches!(reaction, Reaction::Redraw));
        assert_eq!(demo.transcript.next_exchange, 1);
    }

    #[test]
    fn interactive_tick_delivers_the_reply_after_the_countdown() {
        let mut demo = ChatDemo::new();
        let mut backend = quadraui::testing::RecordingBackend::new();
        demo.interactive.thinking_ticks = 1;
        demo.interactive.pending_reply = Some("Echo: hi".to_string());
        let reaction = demo.tick(0, &mut backend);
        assert!(matches!(reaction, Reaction::Redraw));
        assert_eq!(demo.interactive.turns.len(), 1);
        assert_eq!(demo.interactive.turns[0].role, ChatRole::Assistant);
    }
}
