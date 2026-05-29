//! `ChatController` — composed chat overlay controller.
//!
//! Owns all interaction state for a chat overlay: a scrollable transcript,
//! a multi-line input box with history navigation, and a status strip.
//!
//! Apps push transcript turns per frame via
//! [`ChatController::set_transcript`], call
//! [`ChatController::render`] + [`ChatController::handle`], and match on
//! [`ChatControllerEvent`] for semantic actions.
//!
//! # Keyboard behaviour
//!
//! - `Ctrl+Enter` or `Alt+Enter` — submit the current input.
//!   `Alt+Enter` works on all terminals; `Ctrl+Enter` requires the Kitty
//!   keyboard protocol (supported by kitty, Alacritty ≥0.12, WezTerm, foot).
//! - `Enter` — insert a newline in the input.
//! - `Esc` — emit [`ChatControllerEvent::Cancelled`]; the app decides
//!   whether to close the overlay.
//! - `↑` (when cursor is on the first input line) — navigate to the
//!   previous history entry.
//! - `↓` (when cursor is on the last input line) — navigate to the next
//!   history entry or restore the saved input.
//! - `PageUp` / `PageDown` — scroll the transcript.
//! - `↑` / `↓` when the cursor is not on the boundary line — move the
//!   cursor within the input.
//! - `Ctrl+A` — move the cursor to the beginning of the current line
//!   (readline convention).
//! - `Ctrl+E` — move the cursor to the end of the current line
//!   (readline convention).
//!
//! # Scroll behaviour
//!
//! Mouse-wheel events (positive `delta.y` = scroll up) scroll the
//! transcript by 3 rows per tick. Backends normalise their native
//! scroll direction before emitting [`crate::UiEvent::Scroll`].

use crate::{
    Backend, ButtonMask, Color, Key, MessageList, MessageRow, Modifiers, MouseButton, NamedKey,
    Rect, Scrollbar, Spinner, StyledText, TextInput, TextInputHit, UiEvent, WidgetId,
};
use serde::{Deserialize, Serialize};

// ── Public types ───────────────────────────────────────────────────────────────

/// Role of a chat participant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatRole {
    /// A message from the end-user.
    User,
    /// A message from the AI assistant.
    Assistant,
    /// An informational system message (seed prompt label, tool output, etc.).
    System,
}

/// A single turn in the chat transcript.
///
/// `text` is a [`StyledText`] so rich markdown-derived colouring can be added
/// in a future pass. V1 rasterisers simply concatenate span text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatTurn {
    pub role: ChatRole,
    pub text: StyledText,
    /// Unix epoch seconds; `None` when not recorded.
    pub timestamp_unix: Option<f64>,
}

/// Events emitted by [`ChatController::handle`].
#[derive(Debug, Clone, PartialEq)]
pub enum ChatControllerEvent {
    /// User submitted a message (`Ctrl+Enter` or `Alt+Enter`). Contains the full input text
    /// (with embedded `\n` for multi-line messages). Apps should:
    /// 1. Append the text as a `User` turn to their own transcript.
    /// 2. Call [`ChatController::clear_input`].
    /// 3. Start their backend session continuation (subprocess, API call, etc.).
    Submit { text: String },
    /// Stream chunk (reserved for the backend-push path; apps that own
    /// the session pipe call [`ChatController::set_transcript`] directly).
    StreamChunk { text: String },
    /// User pressed `Esc`. The app decides whether to close the overlay,
    /// show a confirmation dialog, or ignore it.
    Cancelled,
    /// A key press that the chat controller did not consume. Apps can
    /// bind hotkeys here (e.g. `'c'` to copy the last assistant turn).
    KeyPressed { key: String, modifiers: Modifiers },
    /// Event was consumed (state changed, caller should redraw).
    Consumed,
    /// Event was not handled by the controller.
    Ignored,
}

// ── Internal types ─────────────────────────────────────────────────────────────

struct ScrollDrag {
    origin_y: f32,
    origin_offset: usize,
    travel: f32,
    max_offset: usize,
}

/// Pre-computed rect zones for one render/handle pass.
struct ChatLayout {
    status: Rect,
    transcript: Rect,
    scrollbar: Option<Rect>,
    /// Spinner rect within the status strip (rightmost `line_height` px).
    spinner: Option<Rect>,
    input: Rect,
}

// ── Controller ─────────────────────────────────────────────────────────────────

/// Cross-backend compose controller for a chat overlay.
///
/// Renders a 3-zone modal over its host rect: a 1-row status strip at the top,
/// a scrollable transcript in the middle, and a multi-line `TextInput` at the
/// bottom. All rendering is delegated to existing [`Backend`] trait methods —
/// `draw_message_list`, `draw_scrollbar`, `draw_text_input`, `draw_spinner` —
/// so no new trait method is needed.
///
/// State ownership follows the [`TreeController`](super::tree_controller::TreeController)
/// pattern: the app pushes transcript turns per frame via
/// [`set_transcript`](Self::set_transcript), and the controller owns only
/// interaction state (scroll position, input buffer, history ring).
///
/// # Wrapping
///
/// Transcript turns are **hard-wrapped at the column boundary**, not
/// word-wrapped: a turn is broken at exactly `floor(width / char_width)`
/// characters per row, so a long word can split mid-word (e.g. with a
/// 10-column budget `"implementation"` becomes `"implementa"` + `"tion"`).
/// Word-aware soft-wrap is deferred to a future pass.
pub struct ChatController {
    id: WidgetId,
    // ── Per-frame data pushed by the app ──────────────────────────────
    transcript: Vec<ChatTurn>,
    status_label: StyledText,
    busy: bool,
    model_label: String,
    spinner_frame: usize,
    // ── Input buffer ─────────────────────────────────────────────────
    /// Raw text with embedded `\n` separating logical lines.
    input_buf: String,
    /// Byte offset of the cursor inside `input_buf`.
    input_cursor: usize,
    /// Vertical scroll offset forwarded to [`TextInput::scroll_offset`].
    input_scroll_offset: usize,
    /// Horizontal scroll offset forwarded to [`TextInput::scroll_col`].
    input_scroll_col: usize,
    /// Whether the input has keyboard focus (controls cursor visibility).
    input_has_focus: bool,
    // ── Input history ─────────────────────────────────────────────────
    /// Past submitted messages (most-recent last).
    history: Vec<String>,
    /// Position in `history` when navigating, or `None` when not in
    /// history navigation mode.
    history_pos: Option<usize>,
    /// Input state saved when the user first pressed `↑` to enter history
    /// navigation. Restored on `↓` past the newest history entry.
    saved_input: Option<(String, usize)>,
    // ── Transcript scroll ─────────────────────────────────────────────
    transcript_scroll_top: usize,
    transcript_drag: Option<ScrollDrag>,
    // ── Config ────────────────────────────────────────────────────────
    /// Number of rows for the text input area. Default: `4`.
    input_height_rows: usize,
    /// Fixed scrollbar track width in surface units, or `None` to use
    /// `backend.line_height()` (same convention as `TreeController`).
    scrollbar_width: Option<f32>,
}

impl ChatController {
    /// Create a new controller. `id` is used to namespace widget IDs for all
    /// sub-primitives (transcript, scrollbar, input, spinner).
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: WidgetId::new(id),
            transcript: Vec::new(),
            status_label: StyledText::plain(""),
            busy: false,
            model_label: String::new(),
            spinner_frame: 0,
            input_buf: String::new(),
            input_cursor: 0,
            input_scroll_offset: 0,
            input_scroll_col: 0,
            input_has_focus: true,
            history: Vec::new(),
            history_pos: None,
            saved_input: None,
            transcript_scroll_top: 0,
            transcript_drag: None,
            input_height_rows: 4,
            scrollbar_width: None,
        }
    }

    // ── Per-frame data setters ────────────────────────────────────────

    /// Replace the transcript for the next render pass.
    ///
    /// This is the primary streaming hook: the app calls this on every
    /// chunk received from the assistant and then triggers a redraw.
    /// The controller does not hold a reference to the app's turns —
    /// it re-renders from the fresh slice each frame.
    pub fn set_transcript(&mut self, turns: Vec<ChatTurn>) {
        self.transcript = turns;
    }

    /// Set the status-strip context label (e.g. `"Refining issue #N"`).
    pub fn set_status(&mut self, label: StyledText) {
        self.status_label = label;
    }

    /// Enable or disable the busy spinner in the status strip.
    pub fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
    }

    /// Set the model chip label in the status strip (e.g. `"claude-opus-4-5"`).
    pub fn set_model_label(&mut self, label: &str) {
        self.model_label = label.to_string();
    }

    /// Advance the spinner animation frame. Apps increment this on their own
    /// ticker (~100 ms per frame for a braille-style spinner).
    pub fn set_spinner_frame(&mut self, frame: usize) {
        self.spinner_frame = frame;
    }

    // ── Input accessors ───────────────────────────────────────────────

    /// The current input buffer text, with `\n` separating lines.
    pub fn input_text(&self) -> &str {
        &self.input_buf
    }

    /// Clear the input buffer and reset cursor + scroll state.
    ///
    /// Apps call this after handling a [`ChatControllerEvent::Submit`].
    pub fn clear_input(&mut self) {
        self.input_buf.clear();
        self.input_cursor = 0;
        self.input_scroll_offset = 0;
        self.input_scroll_col = 0;
        self.history_pos = None;
        self.saved_input = None;
    }

    /// Whether the input area currently has keyboard focus.
    pub fn input_has_focus(&self) -> bool {
        self.input_has_focus
    }

    /// Set keyboard focus on the input area.
    pub fn set_input_has_focus(&mut self, focus: bool) {
        self.input_has_focus = focus;
    }

    /// Override the input area height (in text rows). Default: `4`.
    pub fn set_input_height_rows(&mut self, rows: usize) {
        self.input_height_rows = rows.max(1);
    }

    /// Override the scrollbar track width.
    ///
    /// Pass `Some(8.0)` for GTK, `Some(1.0)` for TUI (matching MSV).
    /// `None` (default) falls back to `backend.line_height()`.
    pub fn set_scrollbar_width(&mut self, width: Option<f32>) {
        self.scrollbar_width = width;
    }

    /// Current transcript scroll offset (first visible wrapped row).
    pub fn transcript_scroll_top(&self) -> usize {
        self.transcript_scroll_top
    }

    /// Programmatically override the transcript scroll position.
    pub fn set_transcript_scroll_top(&mut self, top: usize) {
        self.transcript_scroll_top = top;
    }

    // ── Render ────────────────────────────────────────────────────────

    /// Paint all three zones into `rect` using `backend`.
    ///
    /// Call this inside [`crate::AppLogic::render`] after resolving the
    /// overlay rect. The controller calls `begin_frame` / `end_frame`
    /// _around_ the draw calls if your app's render path wraps frames
    /// inside the overlay rect — otherwise just call it inline.
    pub fn render(&self, backend: &mut dyn Backend, rect: Rect) {
        let layout = self.compute_layout(backend, rect);

        // ── 1. Status strip ───────────────────────────────────────────
        let status_rows = self.build_status_rows();
        let status_list = MessageList {
            id: WidgetId::new(format!("{}-status", self.id.0)),
            rows: status_rows,
            scroll_top: 0,
        };
        backend.draw_message_list(layout.status, &status_list);

        // ── 2. Spinner (overlaid at the right end of the status strip) ─
        if self.busy {
            if let Some(sp_rect) = layout.spinner {
                let spinner = Spinner {
                    id: WidgetId::new(format!("{}-spinner", self.id.0)),
                    label: String::new(),
                    frame_idx: self.spinner_frame,
                    accent: None,
                };
                backend.draw_spinner(sp_rect, &spinner);
            }
        }

        // ── 3. Transcript ─────────────────────────────────────────────
        let col_budget = self.transcript_col_budget(backend.char_width(), layout.transcript);
        let wrapped_rows = self.build_transcript_rows(col_budget);
        let total_rows = wrapped_rows.len();
        let list = MessageList {
            id: WidgetId::new(format!("{}-transcript", self.id.0)),
            rows: wrapped_rows,
            scroll_top: self.transcript_scroll_top,
        };
        backend.draw_message_list(layout.transcript, &list);

        // ── 4. Scrollbar ──────────────────────────────────────────────
        if let Some(sb_rect) = layout.scrollbar {
            let visible_rows =
                Self::transcript_visible_rows_for(backend.line_height(), layout.transcript);
            let track_w = self.scrollbar_track_width(backend.line_height());
            let sb = build_scrollbar(
                format!("{}-sb", self.id.0),
                sb_rect,
                self.transcript_scroll_top,
                total_rows,
                visible_rows,
                self.transcript_drag.is_some(),
                track_w.max(1.0),
            );
            backend.draw_scrollbar(sb_rect, &sb);
        }

        // ── 5. Text input ─────────────────────────────────────────────
        let ti = self.build_text_input();
        backend.draw_text_input(layout.input, &ti);
    }

    // ── Handle ────────────────────────────────────────────────────────

    /// Dispatch a [`UiEvent`] to the controller.
    ///
    /// Keyboard events mutate the input buffer; scroll events mutate the
    /// transcript scroll position. Returns a semantic
    /// [`ChatControllerEvent`] for the app to act on.
    ///
    /// `backend` is used only for layout measurements (line_height,
    /// char_width, text_input_layout) — no drawing happens here.
    pub fn handle(
        &mut self,
        event: &UiEvent,
        backend: &dyn Backend,
        rect: Rect,
    ) -> ChatControllerEvent {
        let layout = self.compute_layout(backend, rect);
        match event {
            UiEvent::CharTyped(ch) => {
                self.input_insert_char(*ch);
                ChatControllerEvent::Consumed
            }

            UiEvent::ClipboardPaste(text) => {
                self.input_insert_str(text);
                ChatControllerEvent::Consumed
            }

            UiEvent::KeyPressed { key, modifiers, .. } => {
                self.handle_key(key, modifiers, backend, &layout)
            }

            UiEvent::Scroll { delta, .. } => {
                let visible =
                    Self::transcript_visible_rows_for(backend.line_height(), layout.transcript);
                let col_budget =
                    self.transcript_col_budget(backend.char_width(), layout.transcript);
                let total = self.build_transcript_rows(col_budget).len();
                // Positive delta.y = scroll content up (decrease offset).
                let rows: isize = if delta.y > 0.0 { -3 } else { 3 };
                self.scroll_transcript_by(rows, total, visible);
                ChatControllerEvent::Consumed
            }

            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => self.handle_click(backend, &layout, position.x, position.y),

            UiEvent::MouseMoved {
                position,
                buttons: ButtonMask { left: true, .. },
            } => self.handle_drag(position.y),

            UiEvent::MouseUp {
                button: MouseButton::Left,
                ..
            } => {
                self.transcript_drag = None;
                ChatControllerEvent::Ignored
            }

            _ => ChatControllerEvent::Ignored,
        }
    }

    // ── Transcript scroll (pub for external use) ───────────────────────

    /// Scroll the transcript by `delta` wrapped rows, clamped to valid range.
    ///
    /// Exposed so callers can drive transcript scrolling without synthesising
    /// a [`UiEvent::Scroll`].
    pub fn scroll_transcript_by(&mut self, delta: isize, total_rows: usize, visible_rows: usize) {
        let max = total_rows.saturating_sub(visible_rows) as isize;
        let cur = self.transcript_scroll_top as isize;
        let new = (cur + delta).max(0).min(max) as usize;
        self.transcript_scroll_top = new;
    }

    // ── Internal helpers ──────────────────────────────────────────────

    fn compute_layout(&self, backend: &dyn Backend, rect: Rect) -> ChatLayout {
        let lh = backend.line_height().max(1.0);
        let status_h = lh;
        // TextInput draws a 1-unit border on top and bottom plus content rows.
        let input_h = self.input_height_rows as f32 * lh + 2.0;
        let middle_h = (rect.height - status_h - input_h).max(0.0);

        let status = Rect::new(rect.x, rect.y, rect.width, status_h);
        let input = Rect::new(rect.x, rect.y + rect.height - input_h, rect.width, input_h);

        // Spinner rect: rightmost `lh × lh` square inside the status strip.
        let spinner = if self.busy && rect.width > lh {
            Some(Rect::new(rect.x + rect.width - lh, rect.y, lh, status_h))
        } else {
            None
        };

        // Show scrollbar when transcript has any content and there is room.
        let track_w = self.scrollbar_track_width(backend.line_height());
        let (transcript, scrollbar) = if !self.transcript.is_empty() && rect.width > track_w {
            let t = Rect::new(rect.x, rect.y + status_h, rect.width - track_w, middle_h);
            let s = Rect::new(
                rect.x + rect.width - track_w,
                rect.y + status_h,
                track_w,
                middle_h,
            );
            (t, Some(s))
        } else {
            let t = Rect::new(rect.x, rect.y + status_h, rect.width, middle_h);
            (t, None)
        };

        ChatLayout {
            status,
            transcript,
            scrollbar,
            spinner,
            input,
        }
    }

    fn scrollbar_track_width(&self, line_height: f32) -> f32 {
        self.scrollbar_width.unwrap_or(line_height)
    }

    fn transcript_visible_rows_for(line_height: f32, rect: Rect) -> usize {
        if line_height <= 0.0 {
            0
        } else {
            (rect.height / line_height).floor() as usize
        }
    }

    fn transcript_col_budget(&self, char_width: f32, rect: Rect) -> usize {
        if char_width <= 0.0 {
            80
        } else {
            ((rect.width / char_width).floor() as usize).max(1)
        }
    }

    fn build_status_rows(&self) -> Vec<MessageRow> {
        let label: String = self
            .status_label
            .spans
            .iter()
            .map(|s| s.text.as_str())
            .collect();
        let text = if self.model_label.is_empty() {
            label
        } else {
            format!("{label}  [{}]", self.model_label)
        };
        vec![MessageRow::new(text, Color::rgb(180, 180, 180), 0.0)]
    }

    fn build_transcript_rows(&self, col_budget: usize) -> Vec<MessageRow> {
        let mut rows = Vec::new();
        for turn in &self.transcript {
            let (role_label, role_fg, content_fg) = match turn.role {
                ChatRole::User => ("You", Color::rgb(100, 180, 255), Color::rgb(220, 220, 220)),
                ChatRole::Assistant => ("AI", Color::rgb(120, 220, 120), Color::rgb(210, 210, 210)),
                ChatRole::System => (
                    "System",
                    Color::rgb(200, 160, 80),
                    Color::rgb(190, 190, 190),
                ),
            };

            // Role header row (no indent).
            rows.push(MessageRow::new(role_label, role_fg, 0.0));

            // Content rows: wrap each raw line from the turn's plain text.
            let plain: String = turn.text.spans.iter().map(|s| s.text.as_str()).collect();
            for raw_line in plain.split('\n') {
                for wrapped in wrap_text(raw_line, col_budget.saturating_sub(2)) {
                    rows.push(MessageRow::new(wrapped, content_fg, 2.0));
                }
            }

            // Blank separator between turns.
            rows.push(MessageRow::new("", Color::rgb(50, 50, 50), 0.0));
        }
        rows
    }

    fn build_text_input(&self) -> TextInput {
        let lines: Vec<String> = if self.input_buf.is_empty() {
            vec![String::new()]
        } else {
            self.input_buf.split('\n').map(String::from).collect()
        };
        let (cursor_line, cursor_col) = cursor_byte_to_line_col(&self.input_buf, self.input_cursor);
        TextInput {
            id: WidgetId::new(format!("{}-input", self.id.0)),
            lines,
            cursor_line,
            cursor_col,
            placeholder: Some(
                "Type a message\u{2026} (Ctrl+Enter or Alt+Enter to send, Enter for newline, Esc to cancel)"
                    .into(),
            ),
            scroll_offset: self.input_scroll_offset,
            scroll_col: self.input_scroll_col,
            has_focus: self.input_has_focus,
        }
    }

    // ── Key handling ──────────────────────────────────────────────────

    fn handle_key(
        &mut self,
        key: &Key,
        modifiers: &Modifiers,
        backend: &dyn Backend,
        layout: &ChatLayout,
    ) -> ChatControllerEvent {
        match key {
            // ── Submit: Ctrl+Enter or Alt+Enter ───────────────────────
            // Alt+Enter works on all terminals; Ctrl+Enter needs Kitty protocol.
            Key::Named(NamedKey::Enter) if modifiers.ctrl || modifiers.alt => {
                if self.input_buf.is_empty() {
                    return ChatControllerEvent::Ignored;
                }
                let text = self.input_buf.clone();
                // Add to history if non-empty and not a duplicate of the last entry.
                if !text.trim().is_empty() && self.history.last() != Some(&text) {
                    self.history.push(text.clone());
                }
                self.history_pos = None;
                self.saved_input = None;
                ChatControllerEvent::Submit { text }
            }

            // ── Cancel: Esc ────────────────────────────────────────────
            Key::Named(NamedKey::Escape) => ChatControllerEvent::Cancelled,

            // ── Newline: plain Enter (no Ctrl) ─────────────────────────
            Key::Named(NamedKey::Enter) => {
                self.input_insert_char('\n');
                // Typing resets history navigation.
                self.history_pos = None;
                self.saved_input = None;
                ChatControllerEvent::Consumed
            }

            // ── Backspace ──────────────────────────────────────────────
            Key::Named(NamedKey::Backspace) => {
                self.input_backspace();
                ChatControllerEvent::Consumed
            }

            // ── Delete ─────────────────────────────────────────────────
            Key::Named(NamedKey::Delete) => {
                self.input_delete();
                ChatControllerEvent::Consumed
            }

            // ── Cursor movement ────────────────────────────────────────
            Key::Named(NamedKey::Left) => {
                self.input_move_left();
                ChatControllerEvent::Consumed
            }
            Key::Named(NamedKey::Right) => {
                self.input_move_right();
                ChatControllerEvent::Consumed
            }
            Key::Named(NamedKey::Home) => {
                self.input_move_home();
                ChatControllerEvent::Consumed
            }
            Key::Named(NamedKey::End) => {
                self.input_move_end();
                ChatControllerEvent::Consumed
            }

            // ── Up: history navigation or cursor up ────────────────────
            Key::Named(NamedKey::Up) => {
                let (cursor_line, _) = cursor_byte_to_line_col(&self.input_buf, self.input_cursor);
                if cursor_line == 0 {
                    self.history_prev()
                } else {
                    self.input_move_cursor_up();
                    ChatControllerEvent::Consumed
                }
            }

            // ── Down: history navigation or cursor down ────────────────
            Key::Named(NamedKey::Down) => {
                let (cursor_line, _) = cursor_byte_to_line_col(&self.input_buf, self.input_cursor);
                if cursor_line == self.input_last_line() {
                    self.history_next()
                } else {
                    self.input_move_cursor_down();
                    ChatControllerEvent::Consumed
                }
            }

            // ── PageUp / PageDown: scroll transcript ───────────────────
            Key::Named(NamedKey::PageUp) => {
                let visible =
                    Self::transcript_visible_rows_for(backend.line_height(), layout.transcript);
                let col_budget =
                    self.transcript_col_budget(backend.char_width(), layout.transcript);
                let total = self.build_transcript_rows(col_budget).len();
                let jump = (visible.max(1) - 1).max(1) as isize;
                self.scroll_transcript_by(-jump, total, visible);
                ChatControllerEvent::Consumed
            }
            Key::Named(NamedKey::PageDown) => {
                let visible =
                    Self::transcript_visible_rows_for(backend.line_height(), layout.transcript);
                let col_budget =
                    self.transcript_col_budget(backend.char_width(), layout.transcript);
                let total = self.build_transcript_rows(col_budget).len();
                let jump = (visible.max(1) - 1).max(1) as isize;
                self.scroll_transcript_by(jump, total, visible);
                ChatControllerEvent::Consumed
            }

            // ── Ctrl+A: move cursor to beginning of line (readline) ────
            Key::Char('a') if modifiers.ctrl => {
                self.input_move_home();
                ChatControllerEvent::Consumed
            }

            // ── Ctrl+E: move cursor to end of line (readline) ──────────
            Key::Char('e') if modifiers.ctrl => {
                self.input_move_end();
                ChatControllerEvent::Consumed
            }

            // ── Regular char: insert (no ctrl/alt) ────────────────────
            Key::Char(c) if !modifiers.ctrl && !modifiers.alt => {
                self.input_insert_char(*c);
                // Any typing resets history navigation.
                self.history_pos = None;
                self.saved_input = None;
                ChatControllerEvent::Consumed
            }

            // ── Anything else: pass to app ─────────────────────────────
            _ => ChatControllerEvent::KeyPressed {
                key: format!("{key:?}"),
                modifiers: *modifiers,
            },
        }
    }

    // ── History navigation ────────────────────────────────────────────

    fn history_prev(&mut self) -> ChatControllerEvent {
        if self.history.is_empty() {
            return ChatControllerEvent::Ignored;
        }
        match self.history_pos {
            None => {
                // Enter history: save current input, show newest history entry.
                self.saved_input = Some((self.input_buf.clone(), self.input_cursor));
                let idx = self.history.len() - 1;
                self.history_pos = Some(idx);
                let entry = self.history[idx].clone();
                self.input_cursor = entry.len();
                self.input_buf = entry;
                ChatControllerEvent::Consumed
            }
            Some(pos) if pos > 0 => {
                let idx = pos - 1;
                self.history_pos = Some(idx);
                let entry = self.history[idx].clone();
                self.input_cursor = entry.len();
                self.input_buf = entry;
                ChatControllerEvent::Consumed
            }
            Some(_) => {
                // Already at the oldest entry — do nothing.
                ChatControllerEvent::Consumed
            }
        }
    }

    fn history_next(&mut self) -> ChatControllerEvent {
        match self.history_pos {
            None => ChatControllerEvent::Ignored,
            Some(pos) => {
                if pos + 1 < self.history.len() {
                    let idx = pos + 1;
                    self.history_pos = Some(idx);
                    let entry = self.history[idx].clone();
                    self.input_cursor = entry.len();
                    self.input_buf = entry;
                } else {
                    // Restore saved input and exit history navigation.
                    let (text, cursor) = self.saved_input.take().unwrap_or_default();
                    self.input_buf = text;
                    self.input_cursor = cursor;
                    self.history_pos = None;
                }
                ChatControllerEvent::Consumed
            }
        }
    }

    // ── Mouse click / drag ────────────────────────────────────────────

    fn handle_click(
        &mut self,
        backend: &dyn Backend,
        layout: &ChatLayout,
        x: f32,
        y: f32,
    ) -> ChatControllerEvent {
        // Scrollbar click?
        if let Some(sb_rect) = layout.scrollbar {
            if rect_contains(sb_rect, x, y) {
                return self.click_scrollbar(backend, layout, sb_rect, y);
            }
        }

        // Transcript area click? (scrolls on page, focuses nothing)
        if rect_contains(layout.transcript, x, y) {
            return ChatControllerEvent::Consumed;
        }

        // Input area click?
        if rect_contains(layout.input, x, y) {
            self.input_has_focus = true;
            let ti = self.build_text_input();
            let til = backend.text_input_layout(layout.input, &ti);
            // Find the clicked visible line and update the cursor.
            let local_y = y - layout.input.y;
            for (r, hit) in &til.hit_regions {
                if local_y >= r.y && local_y < r.y + r.height {
                    if let TextInputHit::Line { line_idx } = hit {
                        let cw = backend.char_width().max(1.0);
                        let local_x = (x - layout.input.x - r.x).max(0.0);
                        let col = (local_x / cw).floor() as usize;
                        self.input_cursor = line_col_to_byte(&self.input_buf, *line_idx, col);
                    }
                    break;
                }
            }
            return ChatControllerEvent::Consumed;
        }

        ChatControllerEvent::Ignored
    }

    fn click_scrollbar(
        &mut self,
        backend: &dyn Backend,
        layout: &ChatLayout,
        sb_rect: Rect,
        y: f32,
    ) -> ChatControllerEvent {
        let visible = Self::transcript_visible_rows_for(backend.line_height(), layout.transcript);
        let col_budget = self.transcript_col_budget(backend.char_width(), layout.transcript);
        let total = self.build_transcript_rows(col_budget).len();
        let max_offset = total.saturating_sub(visible);
        if max_offset == 0 {
            return ChatControllerEvent::Ignored;
        }

        let track_w = self.scrollbar_track_width(backend.line_height());
        let sb = build_scrollbar(
            format!("{}-sb", self.id.0),
            sb_rect,
            self.transcript_scroll_top,
            total,
            visible,
            false,
            track_w.max(1.0),
        );
        let thumb_top = sb_rect.y + sb.thumb_start;
        let thumb_bottom = thumb_top + sb.thumb_len;

        if y >= thumb_top && y < thumb_bottom {
            let travel = (sb_rect.height - sb.thumb_len).max(0.0);
            self.transcript_drag = Some(ScrollDrag {
                origin_y: y,
                origin_offset: self.transcript_scroll_top,
                travel,
                max_offset,
            });
            ChatControllerEvent::Consumed
        } else if y < thumb_top {
            self.scroll_transcript_by(-(visible as isize), total, visible);
            ChatControllerEvent::Consumed
        } else {
            self.scroll_transcript_by(visible as isize, total, visible);
            ChatControllerEvent::Consumed
        }
    }

    fn handle_drag(&mut self, y: f32) -> ChatControllerEvent {
        let Some(drag) = &self.transcript_drag else {
            return ChatControllerEvent::Ignored;
        };
        if drag.travel <= 0.0 || drag.max_offset == 0 {
            return ChatControllerEvent::Ignored;
        }
        let dy = y - drag.origin_y;
        let drow = dy / drag.travel * drag.max_offset as f32;
        let new = (drag.origin_offset as f32 + drow).round() as i32;
        let new = new.max(0) as usize;
        let new = new.min(drag.max_offset);
        if new == self.transcript_scroll_top {
            return ChatControllerEvent::Ignored;
        }
        self.transcript_scroll_top = new;
        ChatControllerEvent::Consumed
    }

    // ── Input text buffer manipulation ────────────────────────────────

    fn input_last_line(&self) -> usize {
        self.input_buf.bytes().filter(|&b| b == b'\n').count()
    }

    /// Insert a single character at the cursor.  Also used by `CharTyped`.
    pub fn input_insert_char(&mut self, ch: char) {
        let cursor = snap_to_char_boundary(&self.input_buf, self.input_cursor);
        self.input_buf.insert(cursor, ch);
        self.input_cursor = cursor + ch.len_utf8();
    }

    /// Insert a string at the cursor (e.g. for clipboard paste).
    pub fn input_insert_str(&mut self, s: &str) {
        let cursor = snap_to_char_boundary(&self.input_buf, self.input_cursor);
        self.input_buf.insert_str(cursor, s);
        self.input_cursor = cursor + s.len();
    }

    fn input_backspace(&mut self) {
        if self.input_cursor == 0 {
            return;
        }
        let prev = prev_char_boundary(&self.input_buf, self.input_cursor);
        self.input_buf.replace_range(prev..self.input_cursor, "");
        self.input_cursor = prev;
    }

    fn input_delete(&mut self) {
        if self.input_cursor >= self.input_buf.len() {
            return;
        }
        let next = next_char_boundary(&self.input_buf, self.input_cursor);
        self.input_buf.replace_range(self.input_cursor..next, "");
    }

    fn input_move_left(&mut self) {
        self.input_cursor = prev_char_boundary(&self.input_buf, self.input_cursor);
    }

    fn input_move_right(&mut self) {
        self.input_cursor = next_char_boundary(&self.input_buf, self.input_cursor);
    }

    fn input_move_home(&mut self) {
        let before = &self.input_buf[..self.input_cursor];
        let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
        self.input_cursor = line_start;
    }

    fn input_move_end(&mut self) {
        let after = &self.input_buf[self.input_cursor..];
        let line_end = after
            .find('\n')
            .map(|i| self.input_cursor + i)
            .unwrap_or(self.input_buf.len());
        self.input_cursor = line_end;
    }

    fn input_move_cursor_up(&mut self) {
        let (line, col) = cursor_byte_to_line_col(&self.input_buf, self.input_cursor);
        if line == 0 {
            return;
        }
        self.input_cursor = line_col_to_byte(&self.input_buf, line - 1, col);
    }

    fn input_move_cursor_down(&mut self) {
        let (line, col) = cursor_byte_to_line_col(&self.input_buf, self.input_cursor);
        let last = self.input_last_line();
        if line >= last {
            return;
        }
        self.input_cursor = line_col_to_byte(&self.input_buf, line + 1, col);
    }
}

// ── Module-level helpers ───────────────────────────────────────────────────────

fn build_scrollbar(
    id: String,
    rect: Rect,
    scroll_top: usize,
    total_rows: usize,
    visible_rows: usize,
    is_dragging: bool,
    min_thumb: f32,
) -> Scrollbar {
    let mut sb = Scrollbar::vertical(
        id,
        rect,
        scroll_top as f32,
        total_rows as f32,
        visible_rows as f32,
        min_thumb,
    );
    sb.dragging = is_dragging;
    sb
}

/// Hard-wrap `text` at the column boundary: break every `col_budget`
/// characters regardless of word boundaries.
///
/// This is a deliberate v1 simplification — words can split mid-word (a
/// 10-column budget turns `"implementation"` into `"implementa"` + `"tion"`).
/// Word-aware soft-wrap is deferred to a future pass.
fn wrap_text(text: &str, col_budget: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    if col_budget == 0 {
        return vec![text.to_string()];
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= col_budget {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let end = (start + col_budget).min(chars.len());
        lines.push(chars[start..end].iter().collect());
        start = end;
    }
    lines
}

/// Convert a byte offset in `text` to `(line, char_col)`.
fn cursor_byte_to_line_col(text: &str, cursor: usize) -> (usize, usize) {
    let cursor = cursor.min(text.len());
    let before = &text[..cursor];
    let line = before.bytes().filter(|&b| b == b'\n').count();
    let col_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col = before[col_start..].chars().count();
    (line, col)
}

/// Convert `(line, char_col)` back to a byte offset in `text`.
///
/// Clamps to the end of the line if `target_col` exceeds the line length.
/// Clamps to `text.len()` if `target_line` doesn't exist.
fn line_col_to_byte(text: &str, target_line: usize, target_col: usize) -> usize {
    let mut current_line = 0usize;
    let mut line_start = 0usize;

    for (i, ch) in text.char_indices() {
        if current_line == target_line {
            // We're on the right line — scan forward `target_col` chars.
            let segment = &text[i..];
            let col_byte = segment
                .char_indices()
                .nth(target_col)
                .map(|(b, _)| b)
                .unwrap_or_else(|| {
                    // Clamp to end of this line (before the '\n').
                    segment.find('\n').unwrap_or(segment.len())
                });
            return i + col_byte;
        }
        if ch == '\n' {
            current_line += 1;
            line_start = i + 1;
        }
    }

    // Handle the case where we're at line 0 of an empty string, or
    // the target line is the last line (no trailing \n yet).
    if current_line == target_line {
        let segment = &text[line_start..];
        let col_byte = segment
            .char_indices()
            .nth(target_col)
            .map(|(b, _)| b)
            .unwrap_or_else(|| segment.find('\n').unwrap_or(segment.len()));
        return line_start + col_byte;
    }

    // Target line doesn't exist — clamp to end of text.
    text.len()
}

fn snap_to_char_boundary(s: &str, byte: usize) -> usize {
    let byte = byte.min(s.len());
    let mut i = byte;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn prev_char_boundary(s: &str, byte: usize) -> usize {
    if byte == 0 {
        return 0;
    }
    let mut i = byte - 1;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn next_char_boundary(s: &str, byte: usize) -> usize {
    let byte = byte.min(s.len());
    if byte >= s.len() {
        return s.len();
    }
    let mut i = byte + 1;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

fn rect_contains(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── MockBackend ──────────────────────────────────────────────────────────
    // Mirrors tree_controller::tests::MockBackend exactly, with two overrides:
    //  - draw_text_input / text_input_layout return a real layout (needed for
    //    click-routing tests that call handle()).
    //  - draw_spinner / spinner_layout return a trivial SpinnerLayout instead
    //    of panicking.

    struct MockBackend;

    impl crate::Backend for MockBackend {
        fn viewport(&self) -> crate::Viewport {
            crate::Viewport {
                width: 80.0,
                height: 24.0,
                scale: 1.0,
            }
        }
        fn begin_frame(&mut self, _v: crate::Viewport) {}
        fn end_frame(&mut self) {}
        fn poll_events(&mut self) -> Vec<UiEvent> {
            Vec::new()
        }
        fn wait_events(&mut self, _t: std::time::Duration) -> Vec<UiEvent> {
            Vec::new()
        }
        fn register_accelerator(&mut self, _a: &crate::Accelerator) {}
        fn unregister_accelerator(&mut self, _id: &crate::AcceleratorId) {}
        fn modal_stack_mut(&mut self) -> &mut crate::ModalStack {
            unimplemented!()
        }
        fn services(&self) -> &dyn crate::backend::PlatformServices {
            unimplemented!()
        }
        fn line_height(&self) -> f32 {
            1.0
        }
        fn char_width(&self) -> f32 {
            1.0
        }
        fn draw_tree(&mut self, _r: Rect, _t: &crate::TreeView) {}
        fn draw_list(&mut self, _r: Rect, _l: &crate::ListView) {}
        fn draw_data_table(
            &mut self,
            _r: Rect,
            _t: &crate::DataTable,
            _h: Option<usize>,
        ) -> crate::DataTableLayout {
            unimplemented!()
        }
        fn data_table_layout(&self, _r: Rect, _t: &crate::DataTable) -> crate::DataTableLayout {
            unimplemented!()
        }
        fn draw_form(&mut self, _r: Rect, _f: &crate::Form) {}
        fn draw_palette(&mut self, _r: Rect, _p: &crate::Palette) {}
        fn draw_status_bar(
            &mut self,
            _r: Rect,
            _b: &crate::primitives::status_bar::StatusBar,
            _hovered_id: Option<&WidgetId>,
            _pressed_id: Option<&WidgetId>,
        ) -> crate::StatusBarLayout {
            unimplemented!()
        }
        fn draw_tab_bar(
            &mut self,
            _r: Rect,
            _b: &crate::TabBar,
            _h: Option<usize>,
        ) -> crate::TabBarHits {
            unimplemented!()
        }
        fn draw_activity_bar(
            &mut self,
            _r: Rect,
            _b: &crate::primitives::activity_bar::ActivityBar,
            _h: Option<usize>,
        ) -> Vec<crate::primitives::activity_bar::ActivityBarRowHit> {
            unimplemented!()
        }
        fn draw_terminal(&mut self, _r: Rect, _t: &crate::Terminal) {}
        fn draw_text_display(&mut self, _r: Rect, _t: &crate::TextDisplay) {}
        fn draw_command_line(&mut self, _r: Rect, _c: &crate::CommandLine) {}
        fn status_bar_layout(&self, _r: Rect, _b: &crate::StatusBar) -> crate::StatusBarLayout {
            crate::StatusBarLayout {
                bar_width: 0.0,
                bar_height: 0.0,
                visible_segments: Vec::new(),
                hit_regions: Vec::new(),
                resolved_right_start: 0,
            }
        }
        fn tab_bar_layout(&self, _r: Rect, _b: &crate::TabBar) -> crate::TabBarHits {
            crate::TabBarHits::default()
        }
        fn activity_bar_layout(
            &self,
            _r: Rect,
            _b: &crate::primitives::activity_bar::ActivityBar,
        ) -> Vec<crate::ActivityBarRowHit> {
            Vec::new()
        }
        fn text_display_layout(
            &self,
            _r: Rect,
            _t: &crate::TextDisplay,
        ) -> crate::TextDisplayLayout {
            unimplemented!()
        }
        /// Override: return a real layout so click-routing tests don't panic.
        fn draw_text_input(&mut self, r: Rect, ti: &crate::TextInput) -> crate::TextInputLayout {
            ti.layout(
                r,
                crate::TextInputMeasure::new(self.line_height(), self.char_width()),
            )
        }
        /// Override: return a real layout so click-routing tests don't panic.
        fn text_input_layout(&self, r: Rect, ti: &crate::TextInput) -> crate::TextInputLayout {
            ti.layout(
                r,
                crate::TextInputMeasure::new(self.line_height(), self.char_width()),
            )
        }
        fn draw_tooltip(&mut self, _t: &crate::Tooltip, _l: &crate::TooltipLayout) {}
        fn draw_context_menu(
            &mut self,
            _m: &crate::ContextMenu,
            _l: &crate::ContextMenuLayout,
        ) -> Vec<(Rect, WidgetId)> {
            unimplemented!()
        }
        fn draw_dialog(&mut self, _d: &crate::Dialog, _l: &crate::DialogLayout) -> Vec<Rect> {
            unimplemented!()
        }
        fn draw_multi_section_view(&mut self, _r: Rect, _v: &crate::MultiSectionView) {}
        fn msv_layout(
            &self,
            _r: Rect,
            _v: &crate::MultiSectionView,
        ) -> crate::MultiSectionViewLayout {
            unimplemented!()
        }
        fn msv_metrics(&self) -> crate::primitives::multi_section_view::LayoutMetrics {
            unimplemented!()
        }
        fn tree_layout(
            &self,
            rect: Rect,
            tree: &crate::TreeView,
        ) -> crate::primitives::tree::TreeViewLayout {
            let lh = self.line_height();
            let visible: usize = if lh > 0.0 {
                (rect.height / lh).floor() as usize
            } else {
                0
            };
            let end = tree.scroll_offset + visible;
            let rows: Vec<crate::primitives::tree::VisibleTreeRow> = (tree.scroll_offset
                ..end.min(tree.rows.len()))
                .enumerate()
                .map(|(vi, ri)| crate::primitives::tree::VisibleTreeRow {
                    row_idx: ri,
                    bounds: Rect::new(0.0, vi as f32 * lh, rect.width, lh),
                })
                .collect();
            crate::primitives::tree::TreeViewLayout {
                viewport_width: rect.width,
                viewport_height: rect.height,
                visible_rows: rows,
                hit_regions: Vec::new(),
                resolved_scroll_offset: tree.scroll_offset,
            }
        }
        fn form_layout(&self, _r: Rect, _f: &crate::Form) -> crate::primitives::form::FormLayout {
            unimplemented!()
        }
        fn draw_editor(
            &mut self,
            _r: Rect,
            _e: &crate::primitives::editor::Editor,
        ) -> crate::backend::EditorPaintResult {
            Default::default()
        }
        fn draw_message_list(
            &mut self,
            _r: Rect,
            _l: &crate::primitives::message_list::MessageList,
        ) {
        }
        fn draw_rich_text_popup(
            &mut self,
            _p: &crate::RichTextPopup,
            _l: &crate::primitives::rich_text_popup::RichTextPopupLayout,
        ) {
        }
        fn draw_find_replace(
            &mut self,
            _r: Rect,
            _p: &crate::primitives::find_replace::FindReplacePanel,
        ) {
        }
        fn draw_completions(
            &mut self,
            _c: &crate::Completions,
            _l: &crate::primitives::completions::CompletionsLayout,
        ) {
        }
        fn draw_scrollbar(&mut self, _r: Rect, _s: &crate::Scrollbar) {}
        fn draw_drop_overlay(&mut self, _o: &crate::primitives::drop_zone::DropOverlay) {}
        fn draw_menu_bar(&mut self, _r: Rect, _b: &crate::MenuBar) -> crate::MenuBarLayout {
            unimplemented!()
        }
        fn menu_bar_layout(&self, _r: Rect, _b: &crate::MenuBar) -> crate::MenuBarLayout {
            unimplemented!()
        }
        fn draw_split(&mut self, _r: Rect, _s: &crate::Split) -> crate::SplitLayout {
            unimplemented!()
        }
        fn split_layout(&self, _r: Rect, _s: &crate::Split) -> crate::SplitLayout {
            unimplemented!()
        }
        fn draw_panel(&mut self, _r: Rect, _p: &crate::Panel) -> crate::PanelLayout {
            unimplemented!()
        }
        fn panel_layout(&self, _r: Rect, _p: &crate::Panel) -> crate::PanelLayout {
            unimplemented!()
        }
        fn draw_toast_stack(
            &mut self,
            _r: Rect,
            _s: &crate::ToastStack,
        ) -> crate::ToastStackLayout {
            unimplemented!()
        }
        fn toast_stack_layout(&self, _r: Rect, _s: &crate::ToastStack) -> crate::ToastStackLayout {
            unimplemented!()
        }
        fn draw_pipeline_view(
            &mut self,
            _r: Rect,
            _v: &crate::PipelineView,
        ) -> crate::PipelineViewLayout {
            unimplemented!()
        }
        fn pipeline_view_layout(
            &self,
            _r: Rect,
            _v: &crate::PipelineView,
        ) -> crate::PipelineViewLayout {
            unimplemented!()
        }
        fn draw_progress(&mut self, _r: Rect, _b: &crate::ProgressBar) -> crate::ProgressBarLayout {
            unimplemented!()
        }
        fn progress_layout(&self, _r: Rect, _b: &crate::ProgressBar) -> crate::ProgressBarLayout {
            unimplemented!()
        }
        /// Override: return a real layout so render tests don't panic.
        fn draw_spinner(&mut self, r: Rect, _s: &crate::Spinner) -> crate::SpinnerLayout {
            crate::SpinnerLayout { bounds: r }
        }
        /// Override: return a real layout so render tests don't panic.
        fn spinner_layout(&self, r: Rect, _s: &crate::Spinner) -> crate::SpinnerLayout {
            crate::SpinnerLayout { bounds: r }
        }
        fn draw_command_center(
            &mut self,
            _r: Rect,
            _c: &crate::CommandCenter,
        ) -> crate::CommandCenterLayout {
            unimplemented!()
        }
        fn command_center_layout(
            &self,
            _r: Rect,
            _c: &crate::CommandCenter,
        ) -> crate::CommandCenterLayout {
            unimplemented!()
        }
        fn draw_chart(
            &mut self,
            _r: Rect,
            _c: &crate::primitives::chart::Chart,
            _h: Option<(usize, usize)>,
            _x: Option<f64>,
        ) -> crate::primitives::chart::ChartLayout {
            unimplemented!()
        }
        fn chart_layout(
            &self,
            _r: Rect,
            _c: &crate::primitives::chart::Chart,
        ) -> crate::primitives::chart::ChartLayout {
            unimplemented!()
        }
        fn draw_toolbar(
            &mut self,
            _r: Rect,
            _b: &crate::primitives::toolbar::Toolbar,
            _h: Option<&crate::types::WidgetId>,
            _p: Option<&crate::types::WidgetId>,
        ) -> crate::primitives::toolbar::ToolbarLayout {
            unimplemented!()
        }
        fn toolbar_layout(
            &self,
            _r: Rect,
            _b: &crate::primitives::toolbar::Toolbar,
        ) -> crate::primitives::toolbar::ToolbarLayout {
            unimplemented!()
        }
        fn draw_sidebar_panel(
            &mut self,
            _r: Rect,
            _p: &crate::primitives::sidebar_panel::SidebarPanel,
            _h: Option<&crate::types::WidgetId>,
            _pr: Option<&crate::types::WidgetId>,
        ) -> crate::primitives::sidebar_panel::SidebarPanelLayout {
            unimplemented!()
        }
        fn sidebar_panel_layout(
            &self,
            _r: Rect,
            _p: &crate::primitives::sidebar_panel::SidebarPanel,
        ) -> crate::primitives::sidebar_panel::SidebarPanelLayout {
            unimplemented!()
        }
    }

    fn make_rect() -> Rect {
        Rect::new(0.0, 0.0, 80.0, 24.0)
    }

    fn make_turn(role: ChatRole, text: &str) -> ChatTurn {
        ChatTurn {
            role,
            text: StyledText::plain(text),
            timestamp_unix: None,
        }
    }

    // ── Construction ─────────────────────────────────────────────────

    #[test]
    fn new_starts_empty() {
        let cc = ChatController::new("chat");
        assert_eq!(cc.input_text(), "");
        assert_eq!(cc.transcript_scroll_top(), 0);
        assert!(cc.input_has_focus());
    }

    // ── Input insertion ───────────────────────────────────────────────

    #[test]
    fn input_insert_char_appends() {
        let mut cc = ChatController::new("c");
        cc.input_insert_char('h');
        cc.input_insert_char('i');
        assert_eq!(cc.input_text(), "hi");
    }

    #[test]
    fn input_insert_str_paste() {
        let mut cc = ChatController::new("c");
        cc.input_insert_str("hello\nworld");
        assert_eq!(cc.input_text(), "hello\nworld");
    }

    #[test]
    fn clear_input_resets_state() {
        let mut cc = ChatController::new("c");
        cc.input_insert_str("hello\nworld");
        cc.clear_input();
        assert_eq!(cc.input_text(), "");
        assert_eq!(cc.input_cursor, 0);
    }

    // ── Keyboard: submit ──────────────────────────────────────────────

    #[test]
    fn ctrl_enter_submits_and_emits_event() {
        let mut cc = ChatController::new("c");
        cc.input_insert_str("hello");
        let rect = make_rect();
        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
        };
        let ev = cc.handle(&event, &MockBackend, rect);
        assert_eq!(
            ev,
            ChatControllerEvent::Submit {
                text: "hello".into()
            }
        );
    }

    #[test]
    fn alt_enter_submits_and_emits_event() {
        let mut cc = ChatController::new("c");
        cc.input_insert_str("hello");
        let rect = make_rect();
        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers {
                alt: true,
                ..Default::default()
            },
            repeat: false,
        };
        let ev = cc.handle(&event, &MockBackend, rect);
        assert_eq!(
            ev,
            ChatControllerEvent::Submit {
                text: "hello".into()
            }
        );
    }

    #[test]
    fn ctrl_enter_on_empty_input_ignored() {
        let mut cc = ChatController::new("c");
        let rect = make_rect();
        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
        };
        let ev = cc.handle(&event, &MockBackend, rect);
        assert_eq!(ev, ChatControllerEvent::Ignored);
    }

    #[test]
    fn submit_adds_to_history() {
        let mut cc = ChatController::new("c");
        cc.input_insert_str("hello");
        let rect = make_rect();
        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
        };
        cc.handle(&event, &MockBackend, rect);
        assert_eq!(cc.history.len(), 1);
        assert_eq!(cc.history[0], "hello");
    }

    // ── Keyboard: Ctrl+A / Ctrl+E line motion ─────────────────────────

    fn ctrl_key(ch: char) -> UiEvent {
        UiEvent::KeyPressed {
            key: Key::Char(ch),
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
        }
    }

    #[test]
    fn ctrl_a_moves_cursor_to_line_start() {
        let mut cc = ChatController::new("c");
        cc.input_insert_str("hello");
        // Cursor sits at end of "hello" after insertion.
        assert_eq!(cc.input_cursor, 5);
        let ev = cc.handle(&ctrl_key('a'), &MockBackend, make_rect());
        assert_eq!(ev, ChatControllerEvent::Consumed);
        assert_eq!(cc.input_cursor, 0);
    }

    #[test]
    fn ctrl_e_moves_cursor_to_line_end() {
        let mut cc = ChatController::new("c");
        cc.input_insert_str("hello");
        // Move to the start first so Ctrl+E has somewhere to travel.
        cc.handle(&ctrl_key('a'), &MockBackend, make_rect());
        assert_eq!(cc.input_cursor, 0);
        let ev = cc.handle(&ctrl_key('e'), &MockBackend, make_rect());
        assert_eq!(ev, ChatControllerEvent::Consumed);
        assert_eq!(cc.input_cursor, 5);
    }

    #[test]
    fn ctrl_a_e_respect_current_line_in_multiline() {
        let mut cc = ChatController::new("c");
        cc.input_insert_str("hello\nworld");
        // Cursor is on the second line, at its end (byte index 11).
        assert_eq!(cc.input_cursor, 11);
        // Ctrl+A moves to the start of the *current* line, not the buffer.
        cc.handle(&ctrl_key('a'), &MockBackend, make_rect());
        // Cursor lands just after the '\n' (start of "world").
        assert_eq!(cc.input_cursor, 6);
        // Ctrl+E moves to the end of the current line (end of buffer here).
        cc.handle(&ctrl_key('e'), &MockBackend, make_rect());
        assert_eq!(cc.input_cursor, 11);
    }

    // ── Serde round-trips ─────────────────────────────────────────────

    #[test]
    fn chat_turn_serde_roundtrip() {
        let turn = ChatTurn {
            role: ChatRole::Assistant,
            text: StyledText::colored("hi there", Color::rgb(180, 230, 180)),
            timestamp_unix: Some(1_700_000_000.0),
        };
        let json = serde_json::to_string(&turn).expect("serialize ChatTurn");
        let decoded: ChatTurn = serde_json::from_str(&json).expect("deserialize ChatTurn");
        assert_eq!(decoded.role, turn.role);
        assert_eq!(decoded.text, turn.text);
        assert_eq!(decoded.timestamp_unix, turn.timestamp_unix);
    }

    #[test]
    fn chat_role_serde_roundtrip() {
        for role in [ChatRole::User, ChatRole::Assistant, ChatRole::System] {
            let json = serde_json::to_string(&role).expect("serialize ChatRole");
            let decoded: ChatRole = serde_json::from_str(&json).expect("deserialize ChatRole");
            assert_eq!(decoded, role);
        }
    }

    // ── Keyboard: cancel ──────────────────────────────────────────────

    #[test]
    fn esc_emits_cancelled() {
        let mut cc = ChatController::new("c");
        let rect = make_rect();
        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let ev = cc.handle(&event, &MockBackend, rect);
        assert_eq!(ev, ChatControllerEvent::Cancelled);
    }

    // ── Keyboard: enter inserts newline ───────────────────────────────

    #[test]
    fn plain_enter_inserts_newline() {
        let mut cc = ChatController::new("c");
        cc.input_insert_str("hello");
        let rect = make_rect();
        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let ev = cc.handle(&event, &MockBackend, rect);
        assert_eq!(ev, ChatControllerEvent::Consumed);
        assert_eq!(cc.input_text(), "hello\n");
    }

    // ── History navigation ────────────────────────────────────────────

    #[test]
    fn up_on_empty_history_returns_ignored() {
        let mut cc = ChatController::new("c");
        let ev = cc.history_prev();
        assert_eq!(ev, ChatControllerEvent::Ignored);
    }

    #[test]
    fn up_enters_history_with_newest_entry() {
        let mut cc = ChatController::new("c");
        cc.history.push("first".into());
        cc.history.push("second".into());
        let ev = cc.history_prev();
        assert_eq!(ev, ChatControllerEvent::Consumed);
        assert_eq!(cc.input_text(), "second");
        assert_eq!(cc.history_pos, Some(1));
    }

    #[test]
    fn up_twice_goes_to_older_entry() {
        let mut cc = ChatController::new("c");
        cc.history.push("first".into());
        cc.history.push("second".into());
        cc.history_prev(); // → "second"
        let ev = cc.history_prev();
        assert_eq!(ev, ChatControllerEvent::Consumed);
        assert_eq!(cc.input_text(), "first");
        assert_eq!(cc.history_pos, Some(0));
    }

    #[test]
    fn up_at_oldest_entry_stays() {
        let mut cc = ChatController::new("c");
        cc.history.push("only".into());
        cc.history_prev(); // at idx 0
        let ev = cc.history_prev(); // already at oldest
        assert_eq!(ev, ChatControllerEvent::Consumed);
        assert_eq!(cc.input_text(), "only"); // unchanged
    }

    #[test]
    fn down_after_history_nav_restores_saved_input() {
        let mut cc = ChatController::new("c");
        cc.input_insert_str("draft");
        cc.history.push("first".into());
        cc.history.push("second".into());
        cc.history_prev(); // → "second"
        cc.history_prev(); // → "first"
        cc.history_next(); // → "second"
        let ev = cc.history_next(); // → restore "draft"
        assert_eq!(ev, ChatControllerEvent::Consumed);
        assert_eq!(cc.input_text(), "draft");
        assert_eq!(cc.history_pos, None);
    }

    #[test]
    fn down_when_not_navigating_returns_ignored() {
        let mut cc = ChatController::new("c");
        let ev = cc.history_next();
        assert_eq!(ev, ChatControllerEvent::Ignored);
    }

    // ── History navigation via Up key (cursor on line 0) ─────────────

    #[test]
    fn up_key_on_line_0_enters_history() {
        let mut cc = ChatController::new("c");
        cc.history.push("prev".into());
        let rect = make_rect();
        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Up),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let ev = cc.handle(&event, &MockBackend, rect);
        assert_eq!(ev, ChatControllerEvent::Consumed);
        assert_eq!(cc.input_text(), "prev");
    }

    #[test]
    fn up_key_on_line_1_moves_cursor_up() {
        let mut cc = ChatController::new("c");
        cc.input_insert_str("line0\nline1");
        // Cursor is on line 1 after insert.
        let (line, _) = cursor_byte_to_line_col(cc.input_text(), cc.input_cursor);
        assert_eq!(line, 1);
        let rect = make_rect();
        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Up),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let ev = cc.handle(&event, &MockBackend, rect);
        assert_eq!(ev, ChatControllerEvent::Consumed);
        let (new_line, _) = cursor_byte_to_line_col(cc.input_text(), cc.input_cursor);
        assert_eq!(new_line, 0);
        // History should NOT have been entered.
        assert_eq!(cc.history_pos, None);
    }

    // ── Transcript scroll ─────────────────────────────────────────────

    #[test]
    fn scroll_transcript_clamps_to_bounds() {
        let mut cc = ChatController::new("c");
        cc.scroll_transcript_by(100, 10, 5); // total=10, visible=5, max=5
        assert_eq!(cc.transcript_scroll_top(), 5);
        cc.scroll_transcript_by(-100, 10, 5);
        assert_eq!(cc.transcript_scroll_top(), 0);
    }

    #[test]
    fn page_up_scrolls_transcript() {
        let mut cc = ChatController::new("c");
        cc.set_transcript_scroll_top(10);
        cc.scroll_transcript_by(-5, 20, 5);
        assert_eq!(cc.transcript_scroll_top(), 5);
    }

    // ── Transcript row building ───────────────────────────────────────

    #[test]
    fn build_transcript_rows_includes_role_header() {
        let mut cc = ChatController::new("c");
        cc.set_transcript(vec![make_turn(ChatRole::User, "hi")]);
        let rows = cc.build_transcript_rows(80);
        // Should have: role header ("You"), content row ("hi"), blank separator.
        assert!(rows.len() >= 3);
        assert_eq!(rows[0].text, "You");
        assert_eq!(rows[1].text, "hi");
    }

    #[test]
    fn build_transcript_rows_wraps_long_line() {
        let mut cc = ChatController::new("c");
        // A 10-char line. col_budget=7 → effective content budget=5 (after the
        // 2-unit indent subtract) → "12345" + "67890" = 2 content rows.
        cc.set_transcript(vec![make_turn(ChatRole::Assistant, "1234567890")]);
        let rows = cc.build_transcript_rows(7);
        let content: Vec<_> = rows
            .iter()
            .filter(|r| !r.text.is_empty() && r.indent > 0.0)
            .collect();
        assert_eq!(content.len(), 2);
    }

    // ── Wrap helper ───────────────────────────────────────────────────

    #[test]
    fn wrap_text_short_line_is_not_split() {
        let v = wrap_text("hello", 80);
        assert_eq!(v, vec!["hello".to_string()]);
    }

    #[test]
    fn wrap_text_splits_at_budget() {
        let v = wrap_text("abcde", 3);
        assert_eq!(v, vec!["abc", "de"]);
    }

    #[test]
    fn wrap_text_empty_returns_one_empty_row() {
        let v = wrap_text("", 80);
        assert_eq!(v, vec![String::new()]);
    }

    // ── Cursor byte conversion helpers ────────────────────────────────

    #[test]
    fn byte_to_line_col_single_line() {
        assert_eq!(cursor_byte_to_line_col("hello", 3), (0, 3));
    }

    #[test]
    fn byte_to_line_col_multiline() {
        assert_eq!(cursor_byte_to_line_col("ab\ncd", 4), (1, 1));
    }

    #[test]
    fn line_col_to_byte_first_line() {
        assert_eq!(line_col_to_byte("hello", 0, 3), 3);
    }

    #[test]
    fn line_col_to_byte_second_line() {
        assert_eq!(line_col_to_byte("ab\ncd", 1, 1), 4);
    }

    #[test]
    fn line_col_to_byte_clamps_to_end_of_line() {
        // "ab\ncd" line 0 has 2 chars; col 99 should clamp to 2 (before \n).
        assert_eq!(line_col_to_byte("ab\ncd", 0, 99), 2);
    }

    #[test]
    fn line_col_to_byte_nonexistent_line_clamps_to_end() {
        assert_eq!(line_col_to_byte("ab", 5, 0), 2);
    }

    // ── Input backspace / delete ──────────────────────────────────────

    #[test]
    fn backspace_deletes_previous_char() {
        let mut cc = ChatController::new("c");
        cc.input_insert_str("hi");
        cc.input_backspace();
        assert_eq!(cc.input_text(), "h");
        assert_eq!(cc.input_cursor, 1);
    }

    #[test]
    fn backspace_at_start_is_noop() {
        let mut cc = ChatController::new("c");
        cc.input_insert_str("x");
        cc.input_cursor = 0;
        cc.input_backspace();
        assert_eq!(cc.input_text(), "x");
    }

    #[test]
    fn delete_removes_char_at_cursor() {
        let mut cc = ChatController::new("c");
        cc.input_insert_str("ab");
        cc.input_cursor = 0;
        cc.input_delete();
        assert_eq!(cc.input_text(), "b");
    }

    // ── Layout zones ─────────────────────────────────────────────────

    #[test]
    fn layout_status_at_top() {
        let cc = ChatController::new("c");
        let layout = cc.compute_layout(&MockBackend, make_rect());
        assert_eq!(layout.status.y, 0.0);
        assert_eq!(layout.status.height, 1.0); // line_height = 1.0
    }

    #[test]
    fn layout_input_at_bottom() {
        let cc = ChatController::new("c");
        let layout = cc.compute_layout(&MockBackend, make_rect());
        // input_height_rows=4, lh=1, border=2 → input_h = 6
        let expected_input_y = make_rect().height - (4.0 * 1.0 + 2.0);
        assert_eq!(layout.input.y, expected_input_y);
    }

    #[test]
    fn layout_no_scrollbar_when_transcript_empty() {
        let cc = ChatController::new("c");
        let layout = cc.compute_layout(&MockBackend, make_rect());
        assert!(layout.scrollbar.is_none());
    }

    #[test]
    fn layout_scrollbar_present_when_transcript_nonempty() {
        let mut cc = ChatController::new("c");
        cc.set_transcript(vec![make_turn(ChatRole::User, "hello")]);
        let layout = cc.compute_layout(&MockBackend, make_rect());
        assert!(layout.scrollbar.is_some());
        // Transcript rect should be narrower than full width.
        assert!(layout.transcript.width < make_rect().width);
    }

    #[test]
    fn layout_spinner_present_when_busy() {
        let mut cc = ChatController::new("c");
        cc.set_busy(true);
        let layout = cc.compute_layout(&MockBackend, make_rect());
        assert!(layout.spinner.is_some());
    }

    #[test]
    fn layout_spinner_absent_when_not_busy() {
        let cc = ChatController::new("c");
        let layout = cc.compute_layout(&MockBackend, make_rect());
        assert!(layout.spinner.is_none());
    }

    // ── Backend rendering tests ───────────────────────────────────────
    //
    // The tests above exercise state/layout against a `MockBackend` whose
    // draw calls are no-ops. These two paint the controller through the
    // real `TuiBackend` / `GtkBackend` trait impls into a backend-owned
    // headless surface (a `ratatui::Buffer` / `cairo::ImageSurface`) and
    // assert that the actual `draw_message_list` / `draw_text_input` /
    // `draw_scrollbar` / `draw_spinner` rasterisers produced output. This
    // is the golden-style coverage required by the issue acceptance
    // criteria (TESTING.md "coordinate drift" row).

    /// TUI: paint a populated controller into a `ratatui::Buffer` via the
    /// `TuiBackend` trait path and assert the painted cells contain the
    /// expected glyphs — the status label + model chip, the user role
    /// header `"You"`, an assistant content row, and the input
    /// placeholder. This proves the real TUI rasterisers ran (not a mock).
    #[cfg(feature = "tui")]
    #[test]
    fn tui_render_paints_glyphs_into_buffer() {
        use crate::tui::TuiBackend;
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        const W: u16 = 48;
        const H: u16 = 14;

        let mut cc = ChatController::new("chat");
        cc.set_status(StyledText::plain("Refining issue #264"));
        cc.set_model_label("claude-opus-4-5");
        cc.set_transcript(vec![
            make_turn(ChatRole::User, "Hello there"),
            make_turn(ChatRole::Assistant, "General Kenobi"),
        ]);

        let mut terminal = Terminal::new(TestBackend::new(W, H)).expect("construct test terminal");
        let mut backend = TuiBackend::new();
        backend.begin_frame(crate::Viewport {
            width: W as f32,
            height: H as f32,
            scale: 1.0,
        });

        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        terminal
            .draw(|frame| {
                backend.enter_frame_scope(frame, |b| {
                    cc.render(b, rect);
                });
            })
            .expect("draw frame");

        // Flatten the painted buffer to a single string so assertions are
        // robust to exact cell coordinates.
        let buf = terminal.backend().buffer();
        let mut painted = String::new();
        for y in 0..H {
            for x in 0..W {
                painted.push(buf[(x, y)].symbol().chars().next().unwrap_or(' '));
            }
            painted.push('\n');
        }

        assert!(
            painted.contains("Refining issue #264"),
            "status label not painted:\n{painted}"
        );
        assert!(
            painted.contains("claude-opus-4-5"),
            "model chip not painted:\n{painted}"
        );
        assert!(
            painted.contains("You"),
            "user role header not painted:\n{painted}"
        );
        assert!(
            painted.contains("General Kenobi"),
            "assistant content row not painted:\n{painted}"
        );
        assert!(
            painted.contains("Type a message"),
            "input placeholder not painted:\n{painted}"
        );
    }

    /// GTK: paint a populated controller into a `cairo::ImageSurface` via
    /// the `GtkBackend` trait path against a white background, then assert
    /// (a) the render inked at least one non-white pixel — proving the GTK
    /// rasterisers actually drew — and (b) the backend's
    /// `text_input_layout` returns a non-empty layout for the input zone.
    #[cfg(feature = "gtk")]
    #[test]
    fn gtk_render_inks_surface_and_layout_nonempty() {
        use crate::gtk::GtkBackend;
        use pangocairo::cairo::{Context, Format, ImageSurface};

        const W: i32 = 360;
        const H: i32 = 220;

        let mut cc = ChatController::new("chat");
        cc.set_status(StyledText::plain("Plan review for #264"));
        cc.set_model_label("claude-opus-4-5");
        cc.set_transcript(vec![
            make_turn(ChatRole::User, "Hello there"),
            make_turn(ChatRole::Assistant, "General Kenobi"),
        ]);

        let mut backend = GtkBackend::new();
        // White background: any non-white pixel must have come from the
        // controller's foreground ink (text / borders / scrollbar).
        backend.set_theme(crate::Theme {
            background: crate::Color::rgb(255, 255, 255),
            ..crate::Theme::default()
        });

        let rect = Rect::new(0.0, 0.0, W as f32, H as f32);
        let mut surface = ImageSurface::create(Format::ARgb32, W, H).expect("create ImageSurface");
        {
            let cr = Context::new(&surface).expect("cairo Context");
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.paint().expect("paint white baseline");
            let layout = pangocairo::functions::create_layout(&cr);
            backend.enter_frame_scope(&cr, &layout, |b| {
                cc.render(b, rect);
            });
        }

        // (a) The render path inked at least one non-white pixel.
        let stride = surface.stride() as usize;
        let inked = {
            let data = surface.data().expect("surface data");
            let mut found = false;
            'scan: for y in 0..H {
                for x in 0..W {
                    let off = y as usize * stride + x as usize * 4;
                    // Cairo ARGB32 on little-endian is BGRA in memory.
                    let (r, g, b) = (data[off + 2], data[off + 1], data[off]);
                    if !(r == 255 && g == 255 && b == 255) {
                        found = true;
                        break 'scan;
                    }
                }
            }
            found
        };
        assert!(
            inked,
            "controller.render painted nothing into the GTK surface"
        );

        // (b) text_input_layout for the input zone yields a usable layout.
        let mut ti = TextInput::new(WidgetId::new("probe-input"));
        ti.lines = vec!["hello".to_string()];
        ti.cursor_col = 5;
        ti.has_focus = true;
        let input_rect = Rect::new(0.0, H as f32 - 80.0, W as f32, 80.0);
        let ti_layout = backend.text_input_layout(input_rect, &ti);
        assert!(
            !ti_layout.visible_lines.is_empty(),
            "text_input_layout returned no visible lines"
        );
    }
}
