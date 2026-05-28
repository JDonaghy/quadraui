//! Backend-agnostic `AppLogic` for the `ChatController` demo
//! ([`tui_chat`] / [`gtk_chat`]).
//!
//! Demonstrates a self-contained chat overlay that:
//! - Renders a `ChatController` covering the full viewport.
//! - Echoes submitted messages as `User` turns.
//! - Responds immediately with a simulated `Assistant` reply after a few ticks.
//! - Tracks a rotating spinner frame while the "assistant" is thinking.
//! - Supports `↑`/`↓` input-history navigation.
//! - Shows a status strip with context label and model chip.
//! - Exits on `q` / `Ctrl+C`.
//!
//! Controls:
//! - Type any text; press `Enter` for newlines in the input.
//! - `Ctrl+Enter` or `Alt+Enter` — submit the message.
//! - `↑` / `↓` — history navigation (when cursor is on the first/last line).
//! - `PageUp` / `PageDown` — scroll the transcript.
//! - `Esc` — clear the input, or quit when the input is already empty.
//! - `q` / `Ctrl+C` — quit immediately.

use quadraui::{
    AppLogic, Backend, ChatController, ChatControllerEvent, ChatRole, ChatTurn, Color, Key,
    Reaction, Rect, StyledText, UiEvent,
};

/// Demo app wrapping a `ChatController`.
///
/// The controller owns all input and layout state. The app owns only the
/// transcript (a `Vec<ChatTurn>`) and updates the controller's snapshot via
/// `set_transcript` on every event that changes the conversation.
pub struct ChatDemo {
    controller: ChatController,
    /// Accumulated transcript. A clone is pushed to the controller after each
    /// mutation so the controller's render pass sees the latest state.
    turns: Vec<ChatTurn>,
    /// Non-zero while simulating an assistant "thinking" delay.
    thinking_ticks: usize,
    /// Pending assistant reply text (set when the user submits, delivered after
    /// the thinking delay).
    pending_reply: Option<String>,
    /// Monotonically advancing spinner animation frame. Incremented every tick
    /// while the assistant is "thinking" so the spinner visibly rotates.
    spinner_frame: usize,
}

impl ChatDemo {
    pub fn new() -> Self {
        let mut controller = ChatController::new("demo:chat");
        controller.set_status(StyledText::plain(
            "Chat demo — Ctrl+Enter or Alt+Enter to send, q to quit",
        ));
        controller.set_model_label("claude-opus-4-5");
        Self {
            controller,
            turns: Vec::new(),
            thinking_ticks: 0,
            pending_reply: None,
            spinner_frame: 0,
        }
    }

    /// Push the current transcript + busy state into the controller so the
    /// next `render()` call sees the latest snapshot.
    fn sync_controller(&mut self) {
        self.controller.set_transcript(self.turns.clone());
        self.controller.set_busy(self.thinking_ticks > 0);
    }
}

impl Default for ChatDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for ChatDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        let rect = Rect::new(0.0, 0.0, vp.width, vp.height);
        self.controller.render(backend, rect);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        // Global quit keys bypass the controller.
        if let UiEvent::KeyPressed {
            ref key,
            ref modifiers,
            ..
        } = event
        {
            match key {
                Key::Char('q') => return Reaction::Exit,
                Key::Char('c') if modifiers.ctrl => return Reaction::Exit,
                _ => {}
            }
        }

        let vp = backend.viewport();
        let rect = Rect::new(0.0, 0.0, vp.width, vp.height);
        let ev = self.controller.handle(&event, backend, rect);

        match ev {
            ChatControllerEvent::Submit { text } => {
                // Append the user turn.
                self.turns.push(ChatTurn {
                    role: ChatRole::User,
                    text: StyledText::colored(text.clone(), Color::rgb(220, 220, 220)),
                    timestamp_unix: None,
                });
                self.controller.clear_input();
                // Queue a simulated reply (delivered after a few ticks).
                self.pending_reply = Some(format!("Echo: {text}"));
                self.thinking_ticks = 5;
                self.sync_controller();
                Reaction::Redraw
            }
            ChatControllerEvent::Cancelled => {
                if self.controller.input_text().is_empty() {
                    Reaction::Exit
                } else {
                    self.controller.clear_input();
                    Reaction::Redraw
                }
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

    fn tick(&mut self, _backend: &mut dyn Backend) -> Reaction {
        if self.thinking_ticks > 0 {
            self.thinking_ticks -= 1;
            // Advance spinner animation by one frame each tick.
            self.spinner_frame = self.spinner_frame.wrapping_add(1);
            self.controller.set_spinner_frame(self.spinner_frame);
            if self.thinking_ticks == 0 {
                // Deliver the simulated reply.
                if let Some(reply) = self.pending_reply.take() {
                    self.turns.push(ChatTurn {
                        role: ChatRole::Assistant,
                        text: StyledText::colored(reply, Color::rgb(180, 230, 180)),
                        timestamp_unix: None,
                    });
                }
            }
            self.sync_controller();
            return Reaction::Redraw;
        }
        Reaction::Continue
    }
}
