//! Backend-agnostic `AppLogic` for the [`Chat`] demo
//! ([`tui_chat`] / [`gtk_chat`]).
//!
//! Demonstrates a self-contained chat overlay that:
//! - Paints a [`Chat`] covering the full viewport and registers it in the
//!   frame tree, so the runtime routes every key, click and wheel to it.
//! - Echoes submitted messages as `User` turns.
//! - Responds with a simulated `Assistant` reply after a few ticks.
//! - Tracks a rotating spinner frame while the "assistant" is thinking.
//! - Supports `↑`/`↓` input-history navigation.
//! - Shows a status strip with context label and model chip.
//! - Opts into `submit_on_enter` plus a persistent hint line, and a
//!   clickable Send/Stop segment at the right of the input row.
//!
//! Controls:
//! - Type any text; `Enter` submits it.
//! - `Shift+Enter` (or `Alt+Enter` as a terminal-compatibility fallback)
//!   inserts a newline instead.
//! - Click the **Send** segment at the right of the input row to submit
//!   with the mouse instead of the keyboard; it reads **Stop** while the
//!   simulated assistant is "thinking" — click it to cancel the reply.
//! - `↑` / `↓` — history navigation (when cursor is on the first/last
//!   *visual* row of the wrapped, auto-growing input box).
//! - `PageUp` / `PageDown` — scroll the transcript.
//! - Click a turn's role-header row (or press `Enter` while `Tab`/
//!   `Shift+Tab` has focused it) to collapse it to a one-line summary
//!   card; click/`Enter` again to expand it back.
//! - `Esc` — clear the input, or quit when the input is already empty.
//! - `Ctrl+C` with nothing selected — quit immediately.
//!
//! The "thinking" spinner's per-frame countdown (`Self::tick`) uses
//! [`Backend::request_frame_in`] to re-arm itself at a precise ~100ms
//! cadence rather than relying on automatic ticks, enabling continued
//! animation when no other work is scheduled (see [`Backend::request_frame_in`]
//! and `Spinner`'s module doc for why frame timing stays app-driven rather
//! than timer-owned).

use quadraui::{
    AppLogic, Backend, Chat, ChatEvent, ChatRole, ChatTurn, Color, FrameTree, Reaction, Rect,
    ScreenLayout, StyledText, Surface, UiEvent, WidgetId,
};
use std::cell::Cell;
use std::time::Duration;

/// Cadence for the "thinking" spinner's animation frame while
/// `thinking_ticks > 0` — see [`ChatDemo::tick`].
const THINKING_TICK_INTERVAL: Duration = Duration::from_millis(100);

/// Demo app owning a [`Chat`].
///
/// The chat descriptor holds every visible field; the app applies each
/// [`ChatEvent`] the chat reports and owns the transcript.
pub struct ChatDemo {
    chat: Chat,
    /// Where the chat painted last frame, for `frame_tree` to register.
    area: Cell<Rect>,
    /// Non-zero while simulating an assistant "thinking" delay.
    thinking_ticks: usize,
    /// Pending assistant reply text (set when the user submits, delivered after
    /// the thinking delay).
    pending_reply: Option<String>,
}

impl ChatDemo {
    pub fn new() -> Self {
        let mut chat = Chat::new(WidgetId::new("demo:chat"));
        chat.status = StyledText::plain("Chat demo — Enter to send, Esc to quit");
        chat.model_label = "claude-opus-4-5".into();
        // Zed-style "Enter to send" plus a persistent hint line
        // (unlike the placeholder, this stays visible while typing).
        chat.submit_on_enter = true;
        chat.hint = Some(StyledText::plain(
            "Enter to send · Shift+Enter for a newline",
        ));
        Self {
            chat,
            area: Cell::new(Rect::new(0.0, 0.0, 0.0, 0.0)),
            thinking_ticks: 0,
            pending_reply: None,
        }
    }

    fn react(&mut self, event: ChatEvent, backend: &mut dyn Backend) -> Reaction {
        self.chat.apply(&event);
        match event {
            ChatEvent::Submit { text } => {
                self.chat.transcript.push(ChatTurn {
                    role: ChatRole::User,
                    text: StyledText::colored(text.clone(), Color::rgb(220, 220, 220)),
                    timestamp_unix: None,
                    line_scales: Vec::new(),
                });
                self.chat.clear_input();
                // Queue a simulated reply (delivered after a few ticks).
                self.pending_reply = Some(format!("Echo: {text}"));
                self.thinking_ticks = 5;
                self.chat.busy = true;
                // Arm the first precise wake here rather than waiting on
                // the runner's coarse idle-poll fallback for it — see
                // `Self::tick`'s matching call.
                backend.request_frame_in(THINKING_TICK_INTERVAL);
                Reaction::Redraw
            }
            ChatEvent::Cancelled => {
                if self.chat.input_text().is_empty() {
                    Reaction::Exit
                } else {
                    self.chat.clear_input();
                    Reaction::Redraw
                }
            }
            // Clicking the Send segment while it reads "Stop" interrupts
            // the simulated "thinking" reply — a real app would cancel its
            // backend session continuation here instead.
            ChatEvent::StopRequested => {
                self.thinking_ticks = 0;
                self.pending_reply = None;
                self.chat.busy = false;
                Reaction::Redraw
            }
            // Clicking a turn's role-header row (row 0 of its block)
            // toggles it between its full body and a one-line summary
            // card. A click elsewhere in the body is ignored here — a real
            // consumer (e.g. vimcode's tool-call cards) would resolve
            // `row_in_turn` to a `path:line` location instead.
            ChatEvent::TurnClicked {
                turn_idx,
                row_in_turn: 0,
            } => {
                self.chat.toggle_turn_collapsed(turn_idx);
                Reaction::Redraw
            }
            ChatEvent::KeyPressed { key, modifiers } if modifiers.ctrl && key == "Char('c')" => {
                Reaction::Exit
            }
            ChatEvent::KeyPressed { .. } | ChatEvent::TurnClicked { .. } => Reaction::Continue,
            _ => Reaction::Redraw,
        }
    }
}

impl Default for ChatDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for ChatDemo {
    type AreaId = ();

    fn setup(&mut self, backend: &mut dyn Backend) {
        backend.set_focus(Some(self.chat.id));
    }

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        let rect = Rect::new(0.0, 0.0, vp.width, vp.height);
        self.area.set(rect);
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect,
            widget: Box::new(self.chat.clone()),
        });
        frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.chat.id, self.area.get(), Box::new(self.chat.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::Chat(id, event) if id == self.chat.id => self.react(event, backend),
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }

    fn tick(&mut self, backend: &mut dyn Backend) -> Reaction {
        if self.thinking_ticks == 0 {
            return Reaction::Continue;
        }
        self.thinking_ticks -= 1;
        // Advance the spinner animation by one frame each tick.
        self.chat.spinner_frame = self.chat.spinner_frame.wrapping_add(1);
        if self.thinking_ticks == 0 {
            // Deliver the simulated reply.
            if let Some(reply) = self.pending_reply.take() {
                self.chat.transcript.push(ChatTurn {
                    role: ChatRole::Assistant,
                    text: StyledText::colored(reply, Color::rgb(180, 230, 180)),
                    timestamp_unix: None,
                    line_scales: Vec::new(),
                });
            }
            self.chat.busy = false;
        } else {
            // Still counting down — ask to be woken again at a precise
            // interval instead of relying on whatever idle cadence (if
            // any) the runner happens to poll at.
            backend.request_frame_in(THINKING_TICK_INTERVAL);
        }
        Reaction::Redraw
    }
}
