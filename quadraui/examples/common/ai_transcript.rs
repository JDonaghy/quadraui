//! Backend-agnostic `AppLogic` for the AI-chat-transcript demo
//! ([`tui_ai_transcript`] / [`gtk_ai_transcript`]).
//!
//! Demonstrates [`ChatController::push_turn_markdown`]: the recommended API
//! for assistant-role messages.  Each canned reply is passed as raw markdown
//! to `push_turn_markdown`; the adapter converts headings, **bold**, *italic*,
//! `inline_code`, bulleted / numbered lists, fenced code blocks, links, and
//! blockquotes to a [`StyledText`] stored inside the [`ChatTurn`].
//!
//! The `ChatController` renders the structured plain text in the transcript.
//! Structural glyphs — list bullets `•`, blockquote bars `│`, and code-block
//! content — survive as literal Unicode text cues in every backend.
//! Per-span attributes (bold, italic, underline, foreground colour) are **not**
//! preserved in the transcript renderer: `build_transcript_rows` extracts
//! uniform plain text and applies a single `content_fg` per role.  Per-span
//! colour rendering in the transcript is a planned future improvement.
//!
//! Controls:
//! - `Ctrl+S` or `Alt+Enter` — submit (sends the next canned prompt).
//! - `Enter` — insert a newline in the input box.
//! - `PageUp` / `PageDown` / `↑` / `↓` — scroll the transcript.
//! - `Esc` — quit.

use quadraui::{
    AppLogic, Backend, ChatController, ChatControllerEvent, ChatRole, Reaction, Rect, StyledText,
    Theme, UiEvent,
};

/// Canned (prompt, markdown-reply) pairs cycled through on each send.
///
/// The replies deliberately exercise every construct the adapter now supports:
/// headings, bold/italic/code, bulleted lists, numbered lists, fenced code
/// blocks, links, and blockquotes.  They also include `snake_case` identifiers
/// to confirm the flanking guard keeps them upright.
const EXCHANGES: &[(&str, &str)] = &[
    (
        "How do I convert markdown to styled text?",
        "\
## Markdown → StyledText

Call `render_markdown_to_styled(input, &theme)`. It returns a
`RenderedMarkdown` with three length-aligned vectors:

- `lines` — one **StyledText** per source line
- `line_text` — plain text, for hit-tests and search
- `line_scales` — heading scale factors (H1=2.0, H2=1.5, H3=1.2)

Then pass the markdown string to `ChatController::push_turn_markdown` and the
controller stores the styled result as a [`ChatTurn`].

> **Tip**: `snake_case` identifiers like render_markdown_to_styled stay
> upright — the adapter honours CommonMark flanking rules.",
    ),
    (
        "What markdown does it support today?",
        "\
# Supported constructs

Inline **bold**, *italic*, and `inline_code` all render with correct spans.
Headings `#` / `##` / `###` scale up in the GTK backend.

## Lists

1. Numbered items render with a styled marker
2. Both dash and asterisk bullet syntax work

- First bullet item
- Second bullet item with **bold** text
* Third item — asterisk syntax

## Fenced code blocks

```rust
pub fn push_turn_markdown(
    &mut self,
    role: ChatRole,
    markdown: &str,
    theme: &Theme,
) {
    // internally calls render_markdown_to_styled
}
```

## Links and blockquotes

See [the quadraui docs](https://github.com/example/quadraui) for details.

> This is a blockquote.  It renders with a `│` left-bar decoration.
> You can put **bold** and `code` inside a blockquote too.",
    ),
    (
        "Anything else I should know?",
        "\
### A few rules to remember

- `snake_case` identifiers stay upright — the flanking guard prevents
  `foo_bar_baz` from being parsed as italic.
- Arithmetic like a * b * c is left alone (whitespace-flanked `*`).
- Nested lists are deferred — single level only for now.

> The same `AppLogic` impl drives both TUI and GTK backends.  The markdown
> adapter runs once; both backends consume `RenderedMarkdown` unchanged.",
    ),
];

/// Demo app that drives a [`ChatController`] with `push_turn_markdown`.
pub struct AiTranscript {
    controller: ChatController,
    next_exchange: usize,
}

impl AiTranscript {
    pub fn new() -> Self {
        let mut controller = ChatController::new("ai-transcript");
        controller.set_status(StyledText::plain(
            "AI transcript  ·  Ctrl+S to send  ·  PgUp/PgDn scroll  ·  Esc quit",
        ));
        // TUI uses 1-cell-wide scrollbar; GTK uses ~8px.  Set to 1 so the
        // demo looks right in TUI without further config.
        controller.set_scrollbar_width(Some(1.0));

        // Seed with a system greeting so the view is not empty on launch.
        controller.push_turn(
            ChatRole::System,
            StyledText::plain(
                "Connected. Press Ctrl+S or Alt+Enter to send the next canned prompt.",
            ),
        );

        Self {
            controller,
            next_exchange: 0,
        }
    }

    /// Send the next canned prompt: echo the user turn, then use
    /// `push_turn_markdown` to render and store the assistant reply.
    fn send_next(&mut self) {
        let (prompt, reply_md) = EXCHANGES[self.next_exchange % EXCHANGES.len()];
        self.next_exchange += 1;

        // User turn — plain text.
        self.controller
            .push_turn(ChatRole::User, StyledText::plain(prompt));

        // Assistant turn — markdown rendered via the adapter.
        let theme = Theme::default();
        self.controller
            .push_turn_markdown(ChatRole::Assistant, reply_md, &theme);
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
        self.controller.render(backend, rect);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        let vp = backend.viewport();
        let rect = Rect::new(0.0, 0.0, vp.width, vp.height);
        match self.controller.handle(&event, backend, rect) {
            ChatControllerEvent::Submit { .. } => {
                self.send_next();
                Reaction::Redraw
            }
            ChatControllerEvent::Cancelled => Reaction::Exit,
            ChatControllerEvent::Consumed => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
