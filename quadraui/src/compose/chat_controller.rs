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
//! - `Ctrl+S`, `Alt+Enter`, or `Ctrl+Enter` — submit the current input.
//!   `Ctrl+S` and `Alt+Enter` work on most terminals; `Ctrl+Enter` requires Kitty
//!   keyboard protocol (supported by kitty, Alacritty ≥0.12, WezTerm, foot).
//!   **Note**: `Ctrl+S` is the XON/XOFF flow-control suspend key in some
//!   terminal configurations (`stty -ixon` disables it). If `Ctrl+S` appears
//!   to freeze the terminal, run `stty -ixon` or use `Alt+Enter` instead.
//! - `Enter` — insert a newline in the input.
//! - `Esc` — emit [`ChatControllerEvent::Cancelled`]; the app decides
//!   whether to close the overlay.
//! - The input soft-wraps long lines to fit the box (#1136), and `↑`/`↓`
//!   move by **visual** row, not logical line — see this module's *Input
//!   soft-wrap and auto-grow* section on [`ChatController`].
//! - `↑` (when the cursor is on the first visual row of the whole
//!   buffer) — navigate to the previous history entry.
//! - `↓` (when the cursor is on the last visual row of the whole buffer)
//!   — navigate to the next history entry or restore the saved input.
//! - `PageUp` / `PageDown` — scroll the transcript.
//! - `↑` / `↓` when the cursor is not on the first/last visual row — move
//!   the cursor within the input (by visual row, so it can move within a
//!   single wrapped logical line).
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

use crate::compose::markdown::render_markdown_to_styled;
use crate::text_util::{
    next_char_boundary, prev_char_boundary, safe_prefix, snap_to_char_boundary, wrap_spans,
    WrapPolicy,
};
use crate::theme::Theme;
use crate::types::StyledSpan;
use crate::{
    Backend, ButtonMask, Color, Key, MessageList, MessageRow, Modifiers, MouseButton, NamedKey,
    Rect, Scrollbar, Spinner, StyledText, TextInput, TextInputHit, UiEvent, WidgetId,
};
use serde::{Deserialize, Serialize};
use std::cell::Cell;

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
/// in a future pass. When `line_scales` is non-empty (turns created via
/// [`ChatController::push_turn_markdown`]), the transcript renderer builds
/// styled [`MessageRow`]s with per-span fg/bold/italic and per-line heading
/// scale; otherwise it falls back to concatenating span text (the flat path).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatTurn {
    pub role: ChatRole,
    pub text: StyledText,
    /// Unix epoch seconds; `None` when not recorded.
    pub timestamp_unix: Option<f64>,
    /// Per-line font-scale factors, parallel to the logical lines in `text`.
    ///
    /// Non-empty only for turns created by [`ChatController::push_turn_markdown`]
    /// (where the markdown adapter supplies one scale per input line).  `1.0` for
    /// body lines; `2.0` / `1.5` / `1.2` for H1 / H2 / H3 headings.
    ///
    /// When empty the transcript renderer uses the flat (legacy) path so that
    /// turns created via [`ChatController::push_turn`] or
    /// [`ChatController::set_transcript`] are visually unchanged.
    #[serde(default)]
    pub line_scales: Vec<f32>,
}

/// Events emitted by [`ChatController::handle`].
#[derive(Debug, Clone, PartialEq)]
pub enum ChatControllerEvent {
    /// User submitted a message (`Ctrl+S`, `Alt+Enter`, or `Ctrl+Enter`). Contains the full input text
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
/// Plain-text turns (from [`push_turn`](Self::push_turn) /
/// [`set_transcript`](Self::set_transcript)) are **word-wrapped** via
/// [`crate::text_util::word_wrap`] (#474): rows break at whitespace where
/// possible, and only a single word wider than the column budget falls
/// back to a mid-word hard break (e.g. with a 10-column budget
/// `"implementation"` becomes `"implementa"` + `"tion"`).
///
/// Styled turns (from [`push_turn_markdown`](Self::push_turn_markdown))
/// wrap per-span at exactly the display-width budget, ignoring word
/// boundaries — [`crate::text_util::wrap_spans`] with
/// [`crate::text_util::WrapPolicy::Char`]. `Word` wrapping is available
/// on the same function (markdown rendering uses it — see
/// [`crate::compose::markdown`]) but is intentionally not used here yet;
/// switching this call site's policy is a follow-up, not a capability
/// gap. See issue #821.
///
/// # Input soft-wrap and auto-grow (#1136)
///
/// The input box's `TextInput` is built from the *wrapped* buffer, not the
/// raw one: [`build_text_input`](Self::build_text_input) soft-wraps
/// `input_buf` to [`TextInput::content_cols`] before handing it to the
/// primitive, so long lines wrap inside the box instead of scrolling
/// horizontally. Wrapping is exact — a visual row is always a contiguous
/// byte range of its logical line, with no whitespace collapsed or
/// dropped (unlike [`crate::text_util::word_wrap`]'s transcript wrapping,
/// which *is* lossy at wrap points) — so cursor positions map losslessly
/// between logical `(line, byte offset)` space (what `input_buf`/
/// `input_cursor` store) and visual `(row, char column)` space (what the
/// painted `TextInput` shows).
///
/// `↑`/`↓` move the cursor by **visual** row, not logical line: pressing
/// `↑` partway through a wrapped paragraph moves to the previous visual
/// row within it; history recall only triggers at the *first* / *last*
/// visual row of the whole buffer (see the `Key::Named(NamedKey::Up)` /
/// `Down` arms of [`Self::handle_key`]).
///
/// The input box's painted height auto-grows with content: it's the
/// wrapped visual row count, clamped to
/// [`input_min_rows`, `input_max_rows`](Self::set_input_height_range)
/// (default `1..=8`). Once the row count exceeds `input_max_rows` the box
/// stops growing and [`TextInput::layout`]'s own vertical auto-scroll
/// keeps the cursor in view, same as any other overflowing `TextInput`.
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
    /// First visible wrapped row.
    ///
    /// Stored in a [`Cell`] so that [`render`](Self::render) (which takes
    /// `&self` to satisfy the [`AppLogic`] trait contract) can update the
    /// position when follow-tail is engaged — without requiring the caller to
    /// hold a `&mut` reference.
    transcript_scroll_top: Cell<usize>,
    transcript_drag: Option<ScrollDrag>,
    /// When `true` the viewport is pinned to the tail: each [`render`] call
    /// drives `transcript_scroll_top` to `total_rows − visible_rows` so
    /// freshly-appended content stays visible.
    ///
    /// Starts `true` on construction.  Disengaged whenever the user scrolls
    /// up (via wheel, PageUp, or scrollbar drag); re-engaged whenever the
    /// user scrolls back down to the bottom row.
    ///
    /// [`render`]: Self::render
    stuck_to_bottom: bool,
    // ── Config ────────────────────────────────────────────────────────
    /// Auto-grow floor: the input area is never shorter than this many
    /// rows, even when empty. Default: `1`. See
    /// [`set_input_height_range`](Self::set_input_height_range).
    input_min_rows: usize,
    /// Auto-grow ceiling: the input area stops growing at this many rows;
    /// beyond it, content scrolls inside a fixed-height box instead.
    /// Default: `8`. See
    /// [`set_input_height_range`](Self::set_input_height_range).
    input_max_rows: usize,
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
            input_has_focus: true,
            history: Vec::new(),
            history_pos: None,
            saved_input: None,
            transcript_scroll_top: Cell::new(0),
            transcript_drag: None,
            stuck_to_bottom: true,
            input_min_rows: 1,
            input_max_rows: 8,
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

    /// Configure the input area's auto-grow row-count clamp (#1136).
    ///
    /// The input box's painted height is the wrapped visual row count of
    /// the current buffer, clamped to `[min, max]`, plus the fixed 2-row
    /// border (see [`Self::compute_layout`]). `min` is floored to `1`
    /// (the box never fully collapses); `max` is floored to `min`.
    /// Default: `1..=8`.
    pub fn set_input_height_range(&mut self, min: usize, max: usize) {
        let min = min.max(1);
        let max = max.max(min);
        self.input_min_rows = min;
        self.input_max_rows = max;
    }

    /// Override the scrollbar track width.
    ///
    /// Pass `Some(8.0)` for GTK, `Some(1.0)` for TUI (matching MSV).
    /// `None` (default) falls back to `backend.line_height()`.
    pub fn set_scrollbar_width(&mut self, width: Option<f32>) {
        self.scrollbar_width = width;
    }

    // ── Transcript push helpers ───────────────────────────────────────

    /// Append a pre-styled turn to the controller's internal transcript.
    ///
    /// This is an alternative to [`set_transcript`] for apps that build the
    /// transcript incrementally rather than replacing it each frame.  Do not
    /// mix `push_turn` and `set_transcript` on the same controller — use one
    /// model consistently.
    ///
    /// For markdown-formatted assistant messages, prefer
    /// [`push_turn_markdown`].
    pub fn push_turn(&mut self, role: ChatRole, text: StyledText) {
        self.transcript.push(ChatTurn {
            role,
            text,
            timestamp_unix: None,
            line_scales: Vec::new(),
        });
    }

    /// Append a turn whose body is markdown text.
    ///
    /// This is the **recommended API for assistant-role messages**.  The
    /// markdown adapter ([`render_markdown_to_styled`]) converts headings,
    /// bold, italic, inline code, bulleted and numbered lists, blockquotes,
    /// links, and fenced code blocks to a [`StyledText`] with colours and
    /// decorations baked in.  The resulting [`ChatTurn`] is appended to the
    /// controller's internal transcript.
    ///
    /// Use [`set_transcript`] instead if your app maintains its own
    /// transcript vector and passes it each frame.  Do not mix `push_turn*`
    /// and `set_transcript` on the same controller.
    ///
    /// Markdown lines are interleaved into a single [`StyledText`] with
    /// `'\n'` separator spans so the transcript renderer can split them for
    /// wrapping, preserving the structure produced by the adapter (list
    /// markers, blockquote rules, etc.) as plain-text cues.
    pub fn push_turn_markdown(&mut self, role: ChatRole, markdown: &str, theme: &Theme) {
        let rendered = render_markdown_to_styled(markdown, theme);
        // Preserve line-scale metadata so the transcript renderer can build
        // styled rows with heading sizes.
        let line_scales = rendered.line_scales.clone();
        let mut spans: Vec<StyledSpan> = Vec::new();
        for (i, line) in rendered.lines.into_iter().enumerate() {
            if i > 0 {
                spans.push(StyledSpan::plain("\n"));
            }
            spans.extend(line.spans);
        }
        self.transcript.push(ChatTurn {
            role,
            text: StyledText { spans },
            timestamp_unix: None,
            line_scales,
        });
    }

    /// Current transcript scroll offset (first visible wrapped row).
    ///
    /// After [`render`](Self::render) runs with follow-tail engaged, this
    /// reflects the position that was actually painted (tail row).
    pub fn transcript_scroll_top(&self) -> usize {
        self.transcript_scroll_top.get()
    }

    /// Programmatically override the transcript scroll position.
    ///
    /// This does **not** change the [`stuck_to_bottom`](Self) flag.  If you
    /// want to re-engage follow-tail, call
    /// [`scroll_transcript_by`](Self::scroll_transcript_by) with a large
    /// positive delta so the flag is recomputed against a known `total_rows`
    /// and `visible_rows`.
    pub fn set_transcript_scroll_top(&mut self, top: usize) {
        self.transcript_scroll_top.set(top);
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
        // Hoist visible_rows so follow-tail can use it before building MessageList.
        let visible_rows =
            Self::transcript_visible_rows_for(backend.line_height(), layout.transcript);

        // Follow-tail: if stuck to the bottom, pin scroll_top to the last page
        // so newly-appended rows are always visible.  This is a no-op when the
        // content fits entirely in the viewport (total ≤ visible → max = 0).
        // `transcript_scroll_top` is a `Cell<usize>` so we can update it here
        // while `render` takes `&self` (required by the `AppLogic` trait).
        if self.stuck_to_bottom {
            self.transcript_scroll_top
                .set(total_rows.saturating_sub(visible_rows));
        }

        let list = MessageList {
            id: WidgetId::new(format!("{}-transcript", self.id.0)),
            rows: wrapped_rows,
            scroll_top: self.transcript_scroll_top.get(),
        };
        backend.draw_message_list(layout.transcript, &list);

        // ── 4. Scrollbar ──────────────────────────────────────────────
        if let Some(sb_rect) = layout.scrollbar {
            let track_w = self.scrollbar_track_width(backend.line_height());
            let sb = build_scrollbar(
                format!("{}-sb", self.id.0),
                sb_rect,
                self.transcript_scroll_top.get(),
                total_rows,
                visible_rows,
                self.transcript_drag.is_some(),
                track_w.max(1.0),
            );
            backend.draw_scrollbar(sb_rect, &sb);
        }

        // ── 5. Text input ─────────────────────────────────────────────
        let col_budget = TextInput::content_cols(layout.input.width, backend.char_width());
        let ti = self.build_text_input(col_budget);
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
    ///
    /// After clamping, [`stuck_to_bottom`](Self) is recomputed: scrolling up
    /// disengages follow-tail; scrolling back to the last page re-engages it.
    pub fn scroll_transcript_by(&mut self, delta: isize, total_rows: usize, visible_rows: usize) {
        let max = total_rows.saturating_sub(visible_rows) as isize;
        let cur = self.transcript_scroll_top.get() as isize;
        let new = (cur + delta).max(0).min(max) as usize;
        self.transcript_scroll_top.set(new);
        // Re-engage follow-tail when scrolled to (or past) the last page;
        // disengage it when scrolled up from there.
        self.stuck_to_bottom =
            self.transcript_scroll_top.get() >= total_rows.saturating_sub(visible_rows);
    }

    // ── Internal helpers ──────────────────────────────────────────────

    fn compute_layout(&self, backend: &dyn Backend, rect: Rect) -> ChatLayout {
        let lh = backend.line_height().max(1.0);
        let status_h = lh;
        // Auto-grow (#1136): height tracks the wrapped visual row count of
        // the current buffer, clamped to [input_min_rows, input_max_rows].
        // TextInput draws a 1-unit border on top and bottom plus content rows.
        let col_budget = TextInput::content_cols(rect.width, backend.char_width());
        let visual_rows = wrap_input_rows(&self.input_buf, col_budget).len().max(1);
        let input_rows = visual_rows.clamp(self.input_min_rows, self.input_max_rows);
        let input_h = input_rows as f32 * lh + 2.0;
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

            let content_budget = col_budget.saturating_sub(2);

            if !turn.line_scales.is_empty() {
                // ── Styled path ──────────────────────────────────────────
                // Turns created by `push_turn_markdown` carry per-line span
                // lists (separated by plain `\n` spans) and matching scale
                // factors.  Build styled MessageRows so rasterisers can apply
                // per-span fg/bold/italic and heading font sizes.
                let lines_spans = split_spans_by_newline(&turn.text.spans);
                for (i, line_spans) in lines_spans.iter().enumerate() {
                    let scale = turn.line_scales.get(i).copied().unwrap_or(1.0);
                    let wrapped_groups = wrap_spans(line_spans, content_budget, WrapPolicy::Char);
                    for group in wrapped_groups {
                        let text: String = group.iter().map(|s| s.text.as_str()).collect();
                        rows.push(MessageRow {
                            text,
                            fg: content_fg,
                            indent: 2.0,
                            spans: group,
                            scale,
                        });
                    }
                }
            } else {
                // ── Flat path (unchanged) ────────────────────────────────
                // Turns created by `push_turn` or `set_transcript` have
                // empty `line_scales`.  Output is byte-for-byte identical
                // to the pre-styled-row behaviour — existing callers
                // (vimcode debug / AI sidebar) are visually unchanged.
                let plain: String = turn.text.spans.iter().map(|s| s.text.as_str()).collect();
                for raw_line in plain.split('\n') {
                    for wrapped in wrap_text(raw_line, content_budget) {
                        rows.push(MessageRow::new(wrapped, content_fg, 2.0));
                    }
                }
            }

            // Blank separator between turns.
            rows.push(MessageRow::new("", Color::rgb(50, 50, 50), 0.0));
        }
        rows
    }

    /// Soft-wrap `input_buf` to `col_budget` and build the `TextInput` the
    /// rasterisers paint (#1136) — see this struct's *Input soft-wrap and
    /// auto-grow* doc section.
    fn build_text_input(&self, col_budget: usize) -> TextInput {
        let rows = wrap_input_rows(&self.input_buf, col_budget);
        self.text_input_from_rows(&rows)
    }

    /// Build the paint-time `TextInput` from an already-wrapped `rows`
    /// (shared by [`Self::build_text_input`] and
    /// [`Self::handle_click`], which both need the same wrap pass — the
    /// click handler additionally uses `rows` to map the click back to a
    /// buffer byte offset).
    fn text_input_from_rows(&self, rows: &[InputRow]) -> TextInput {
        let lines: Vec<String> = rows.iter().map(|r| r.text.clone()).collect();
        let (logical_line, logical_col) =
            cursor_byte_to_line_col(&self.input_buf, self.input_cursor);
        let (visual_row, visual_col) = input_logical_to_visual(rows, logical_line, logical_col);
        let mut ti = TextInput::new(WidgetId::new(format!("{}-input", self.id.0)));
        ti.lines = lines;
        ti.cursor_line = visual_row;
        ti.cursor_col = visual_col;
        ti.placeholder = Some(
            "Type a message\u{2026} (Ctrl+S or Alt+Enter to send, Enter for newline, Esc to cancel)"
                .into(),
        );
        ti.scroll_offset = self.input_scroll_offset;
        // No horizontal scroll: `rows` is already wrapped to fit the box's
        // width, so every visual row fits within `col_budget` columns.
        ti.scroll_col = 0;
        ti.has_focus = self.input_has_focus;
        ti
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
            // ── Submit: Ctrl+S, Alt+Enter, or Ctrl+Enter ─────────────────
            // Ctrl+S works on all terminals.
            // Alt+Enter works on all terminals (ESC+Enter escape sequence).
            // Ctrl+Enter needs Kitty protocol — unreliable as the primary affordance.
            Key::Char('s') if modifiers.ctrl => {
                if self.input_buf.is_empty() {
                    return ChatControllerEvent::Ignored;
                }
                let text = self.input_buf.clone();
                if !text.trim().is_empty() && self.history.last() != Some(&text) {
                    self.history.push(text.clone());
                }
                self.history_pos = None;
                self.saved_input = None;
                ChatControllerEvent::Submit { text }
            }
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

            // ── Up: history navigation or cursor up (by VISUAL row —
            //    #1136: a wrapped paragraph's interior rows move the
            //    cursor; history recall only fires on the first visual
            //    row of the whole buffer) ─────────────────────────────
            Key::Named(NamedKey::Up) => {
                let col_budget = TextInput::content_cols(layout.input.width, backend.char_width());
                let rows = wrap_input_rows(&self.input_buf, col_budget);
                let (logical_line, logical_col) =
                    cursor_byte_to_line_col(&self.input_buf, self.input_cursor);
                let (row_idx, col) = input_logical_to_visual(&rows, logical_line, logical_col);
                if row_idx == 0 {
                    self.history_prev()
                } else {
                    self.input_cursor =
                        input_visual_to_byte(&self.input_buf, &rows, row_idx - 1, col);
                    ChatControllerEvent::Consumed
                }
            }

            // ── Down: history navigation or cursor down (by VISUAL row —
            //    see the `Up` arm above) ─────────────────────────────────
            Key::Named(NamedKey::Down) => {
                let col_budget = TextInput::content_cols(layout.input.width, backend.char_width());
                let rows = wrap_input_rows(&self.input_buf, col_budget);
                let (logical_line, logical_col) =
                    cursor_byte_to_line_col(&self.input_buf, self.input_cursor);
                let (row_idx, col) = input_logical_to_visual(&rows, logical_line, logical_col);
                if row_idx + 1 >= rows.len() {
                    self.history_next()
                } else {
                    self.input_cursor =
                        input_visual_to_byte(&self.input_buf, &rows, row_idx + 1, col);
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
            let col_budget = TextInput::content_cols(layout.input.width, backend.char_width());
            let rows = wrap_input_rows(&self.input_buf, col_budget);
            let ti = self.text_input_from_rows(&rows);
            let til = backend.text_input_layout(layout.input, &ti);
            // Find the clicked visible line and update the cursor.
            // `line_idx` here is a VISUAL row index (into `rows`), not a
            // logical line — map it back through `rows` (#1136).
            let local_y = y - layout.input.y;
            for (r, hit) in &til.hit_regions {
                if local_y >= r.y && local_y < r.y + r.height {
                    if let TextInputHit::Line { line_idx } = hit {
                        let cw = backend.char_width().max(1.0);
                        let local_x = (x - layout.input.x - r.x).max(0.0);
                        let col = (local_x / cw).floor() as usize;
                        self.input_cursor =
                            input_visual_to_byte(&self.input_buf, &rows, *line_idx, col);
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
            self.transcript_scroll_top.get(),
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
                origin_offset: self.transcript_scroll_top.get(),
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
        // Copy max_offset before the immutable borrow of `drag` ends so we can
        // use it when updating `stuck_to_bottom` after mutating `self`.
        let max_offset = drag.max_offset;
        let new = new.min(max_offset);
        if new == self.transcript_scroll_top.get() {
            return ChatControllerEvent::Ignored;
        }
        self.transcript_scroll_top.set(new);
        self.stuck_to_bottom = new >= max_offset;
        ChatControllerEvent::Consumed
    }

    // ── Input text buffer manipulation ────────────────────────────────

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

/// Word-aware soft-wrap of `text` to `col_budget` characters per row.
///
/// Thin wrapper over [`crate::text_util::word_wrap`] (#474) — this used to
/// be a private hard char-break (splitting `"implementation"` into
/// `"implementa"` + `"tion"` regardless of word boundaries) with "word-aware
/// soft-wrap is deferred to a future pass" as a known v1 gap. That gap is
/// now closed: only a single word wider than `col_budget` still hard-breaks
/// (there's no other way to fit it), everything else wraps at whitespace.
fn wrap_text(text: &str, col_budget: usize) -> Vec<String> {
    crate::text_util::word_wrap(text, col_budget)
}

/// Split a flat span list (where `\n`-only spans act as line delimiters)
/// into per-logical-line span lists.
///
/// This is the inverse of the encoding used by
/// [`ChatController::push_turn_markdown`], which concatenates per-line
/// span vecs with [`StyledSpan::plain`]`("\n")` separators.  The
/// resulting `Vec<Vec<StyledSpan>>` has one entry per logical line
/// (including an empty entry for blank lines).
fn split_spans_by_newline(spans: &[StyledSpan]) -> Vec<Vec<StyledSpan>> {
    let mut lines: Vec<Vec<StyledSpan>> = Vec::new();
    let mut current: Vec<StyledSpan> = Vec::new();
    for span in spans {
        if span.text == "\n" {
            lines.push(std::mem::take(&mut current));
        } else {
            current.push(span.clone());
        }
    }
    lines.push(current);
    lines
}

/// Convert a byte offset in `text` to `(line, char_col)`.
fn cursor_byte_to_line_col(text: &str, cursor: usize) -> (usize, usize) {
    let before = safe_prefix(text, cursor);
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

// ── Input soft-wrap (#1136) ─────────────────────────────────────────────

/// One soft-wrapped visual row of the chat input buffer.
///
/// Wrapping here is deliberately **exact** — a row's `text` is always a
/// contiguous char range of its owning logical line, with no whitespace
/// collapsed or dropped (unlike [`crate::text_util::word_wrap`]'s
/// transcript wrapping, which *is* lossy at wrap points). Concatenating
/// every row for a given `logical_line`, in order, reconstructs that
/// line's text byte-for-byte. That's what lets
/// [`input_logical_to_visual`] / [`input_visual_to_byte`] round-trip
/// cursor positions losslessly between logical (buffer byte offset) and
/// visual (row, char column) space.
struct InputRow {
    /// Index into the logical (`\n`-separated) lines of the buffer.
    logical_line: usize,
    /// Char-column offset into `logical_line` where this row starts.
    col_offset: usize,
    /// This row's text — a char-exact slice of `logical_line`.
    text: String,
}

/// Soft-wrap `text` (the whole input buffer, `\n`-separated logical
/// lines) to `col_budget` char columns per visual row. See [`InputRow`]
/// for the exactness guarantee that makes cursor-position round-tripping
/// possible.
///
/// Always returns at least one row per logical line (an empty logical
/// line yields one empty row), so `rows.len() >= 1` for any input,
/// including `""`.
fn wrap_input_rows(text: &str, col_budget: usize) -> Vec<InputRow> {
    let mut rows = Vec::new();
    for (logical_line, line) in text.split('\n').enumerate() {
        for (col_offset, row_text) in wrap_line_exact(line, col_budget) {
            rows.push(InputRow {
                logical_line,
                col_offset,
                text: row_text,
            });
        }
    }
    rows
}

/// Break one logical `line` (no `\n`) into `(col_offset, text)` visual
/// rows of at most `col_budget` char columns, breaking after the last
/// space at or before the budget where possible (an unbroken run longer
/// than `col_budget` hard-breaks at the budget). Never collapses or
/// drops characters — see [`InputRow`].
///
/// `col_budget == 0` or a line that already fits returns the line
/// unmodified as a single row, matching
/// [`crate::text_util::word_wrap`]'s convention.
fn wrap_line_exact(line: &str, col_budget: usize) -> Vec<(usize, String)> {
    let chars: Vec<char> = line.chars().collect();
    if col_budget == 0 || chars.len() <= col_budget {
        return vec![(0, line.to_string())];
    }
    let mut rows = Vec::new();
    let mut start = 0usize;
    while start < chars.len() {
        let mut end = (start + col_budget).min(chars.len());
        if end < chars.len() {
            if let Some(space_rel) = chars[start..end].iter().rposition(|&c| c == ' ') {
                let space_idx = start + space_rel;
                if space_idx > start {
                    // Break just after the space so it stays with this
                    // row (matching normal word-wrap convention) rather
                    // than becoming a leading space on the next row.
                    end = space_idx + 1;
                }
            }
        }
        rows.push((start, chars[start..end].iter().collect()));
        start = end;
    }
    rows
}

/// Map a logical `(line, char_col)` cursor position (as produced by
/// [`cursor_byte_to_line_col`]) to `(visual_row_idx, visual_col)` within
/// `rows` — the inverse of [`input_visual_to_byte`].
fn input_logical_to_visual(rows: &[InputRow], line: usize, col: usize) -> (usize, usize) {
    let mut last_for_line: Option<usize> = None;
    for (i, r) in rows.iter().enumerate() {
        if r.logical_line != line {
            continue;
        }
        last_for_line = Some(i);
        let len = r.text.chars().count();
        if col >= r.col_offset && col < r.col_offset + len {
            return (i, col - r.col_offset);
        }
    }
    // `col` is at (or past) the end of the logical line — land on the end
    // of its last visual row.
    match last_for_line {
        Some(i) => {
            let r = &rows[i];
            let len = r.text.chars().count();
            (i, col.saturating_sub(r.col_offset).min(len))
        }
        None => (0, 0),
    }
}

/// Map a `(visual_row_idx, visual_col)` position within `rows` back to a
/// byte offset in `text` (the whole input buffer) — the inverse of
/// [`input_logical_to_visual`]. Both indices are clamped, so this never
/// panics on an out-of-range row or column; `rows` empty returns `0`.
fn input_visual_to_byte(text: &str, rows: &[InputRow], row_idx: usize, col: usize) -> usize {
    let Some(last) = rows.len().checked_sub(1) else {
        return 0;
    };
    let r = &rows[row_idx.min(last)];
    let len = r.text.chars().count();
    line_col_to_byte(text, r.logical_line, r.col_offset + col.min(len))
}

fn rect_contains(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::RecordingBackend;

    // Tests below drive `ChatController` against `RecordingBackend`
    // (quadraui#799, replacing a private `MockBackend` copy of the same
    // shape) — its `draw_text_input`/`text_input_layout` and
    // `draw_spinner`/`spinner_layout` all return real layouts rather than
    // panicking, which the click-routing and render tests here depend on.

    fn make_rect() -> Rect {
        Rect::new(0.0, 0.0, 80.0, 24.0)
    }

    fn make_turn(role: ChatRole, text: &str) -> ChatTurn {
        ChatTurn {
            role,
            text: StyledText::plain(text),
            timestamp_unix: None,
            line_scales: Vec::new(),
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
        let ev = cc.handle(&event, &RecordingBackend::new(), rect);
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
        let ev = cc.handle(&event, &RecordingBackend::new(), rect);
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
        let ev = cc.handle(&event, &RecordingBackend::new(), rect);
        assert_eq!(ev, ChatControllerEvent::Ignored);
    }

    #[test]
    fn ctrl_s_submits_and_emits_event() {
        let mut cc = ChatController::new("c");
        cc.input_insert_str("hello");
        let rect = make_rect();
        let event = UiEvent::KeyPressed {
            key: Key::Char('s'),
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
        };
        let ev = cc.handle(&event, &RecordingBackend::new(), rect);
        assert_eq!(
            ev,
            ChatControllerEvent::Submit {
                text: "hello".into()
            }
        );
    }

    #[test]
    fn ctrl_s_on_empty_input_ignored() {
        let mut cc = ChatController::new("c");
        let rect = make_rect();
        let event = UiEvent::KeyPressed {
            key: Key::Char('s'),
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
        };
        let ev = cc.handle(&event, &RecordingBackend::new(), rect);
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
        cc.handle(&event, &RecordingBackend::new(), rect);
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
        let ev = cc.handle(&ctrl_key('a'), &RecordingBackend::new(), make_rect());
        assert_eq!(ev, ChatControllerEvent::Consumed);
        assert_eq!(cc.input_cursor, 0);
    }

    #[test]
    fn ctrl_e_moves_cursor_to_line_end() {
        let mut cc = ChatController::new("c");
        cc.input_insert_str("hello");
        // Move to the start first so Ctrl+E has somewhere to travel.
        cc.handle(&ctrl_key('a'), &RecordingBackend::new(), make_rect());
        assert_eq!(cc.input_cursor, 0);
        let ev = cc.handle(&ctrl_key('e'), &RecordingBackend::new(), make_rect());
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
        cc.handle(&ctrl_key('a'), &RecordingBackend::new(), make_rect());
        // Cursor lands just after the '\n' (start of "world").
        assert_eq!(cc.input_cursor, 6);
        // Ctrl+E moves to the end of the current line (end of buffer here).
        cc.handle(&ctrl_key('e'), &RecordingBackend::new(), make_rect());
        assert_eq!(cc.input_cursor, 11);
    }

    // ── Serde round-trips ─────────────────────────────────────────────

    #[test]
    fn chat_turn_serde_roundtrip() {
        let turn = ChatTurn {
            role: ChatRole::Assistant,
            text: StyledText::colored("hi there", Color::rgb(180, 230, 180)),
            timestamp_unix: Some(1_700_000_000.0),
            line_scales: Vec::new(),
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
        let ev = cc.handle(&event, &RecordingBackend::new(), rect);
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
        let ev = cc.handle(&event, &RecordingBackend::new(), rect);
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
        let ev = cc.handle(&event, &RecordingBackend::new(), rect);
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
        let ev = cc.handle(&event, &RecordingBackend::new(), rect);
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

    // ── Follow-tail (stick-to-bottom) ─────────────────────────────────

    /// A brand-new controller must start stuck to the bottom so the first
    /// streamed reply is visible without the user having to scroll.
    #[test]
    fn new_controller_is_stuck_to_bottom() {
        let cc = ChatController::new("c");
        assert!(
            cc.stuck_to_bottom,
            "new controller must start stuck_to_bottom=true"
        );
    }

    /// When `stuck_to_bottom` is true and new turns push the total row-count
    /// beyond the visible window, `render()` must advance `transcript_scroll_top`
    /// so the newest rows are visible.
    ///
    /// Geometry with `RecordingBackend` (line_height=1, char_width=1) + `make_rect`
    /// (80 × 24):
    ///   status_h = 1, input_h = 4×1+2 = 6, middle_h = 17,
    ///   scrollbar_width = 1  →  transcript_rect = 79 × 17,
    ///   visible_rows = 17, col_budget = 79.
    ///
    /// Each plain turn → 1 header + 1 content + 1 blank = 3 rows.
    /// 7 turns → 21 rows.  max_scroll = 21 − 17 = 4.
    #[test]
    fn follow_tail_advances_scroll_top_on_new_content() {
        let mut cc = ChatController::new("c");
        for i in 0..7 {
            cc.push_turn(ChatRole::User, StyledText::plain(&format!("m{i}")));
        }
        assert_eq!(cc.transcript_scroll_top(), 0, "scroll_top must start at 0");
        cc.render(&mut RecordingBackend::new(), make_rect());
        assert!(
            cc.transcript_scroll_top() > 0,
            "follow-tail must advance scroll_top when content exceeds the visible window; \
             scroll_top={}",
            cc.transcript_scroll_top()
        );
    }

    /// When the user has scrolled up (disengaging follow-tail), appending new
    /// content must NOT yank the viewport to the bottom.
    #[test]
    fn scrolled_up_position_preserved_across_new_content() {
        let mut cc = ChatController::new("c");
        // Scroll up from 0 → disengages follow-tail (max = 20−5 = 15; 0 < 15).
        cc.scroll_transcript_by(-5, 20, 5); // clamped to 0, stuck=false
                                            // Scroll to a mid position.
        cc.scroll_transcript_by(3, 20, 5); // scroll_top=3, stuck=false
        assert!(
            !cc.stuck_to_bottom,
            "scroll_transcript_by up must disengage follow-tail"
        );
        assert_eq!(cc.transcript_scroll_top(), 3);
        // Append new content.
        cc.push_turn(ChatRole::Assistant, StyledText::plain("new reply"));
        // render() must leave scroll_top unchanged because follow-tail is off.
        cc.render(&mut RecordingBackend::new(), make_rect());
        assert_eq!(
            cc.transcript_scroll_top(),
            3,
            "scroll position must be preserved when follow-tail is disengaged; \
             scroll_top={}",
            cc.transcript_scroll_top()
        );
    }

    /// After scrolling up (disengaging follow-tail), scrolling back to the very
    /// last page must re-engage follow-tail automatically.
    #[test]
    fn scroll_to_bottom_reengages_follow_tail() {
        let mut cc = ChatController::new("c");
        // Disengage by scrolling up.
        cc.scroll_transcript_by(-5, 20, 5); // clamped to 0, max=15, stuck=false
        cc.scroll_transcript_by(3, 20, 5); // scroll_top=3, stuck=false
        assert!(!cc.stuck_to_bottom);
        // Scroll to the bottom (max = 20−5 = 15).
        cc.scroll_transcript_by(100, 20, 5); // clamped to 15, stuck=(15≥15)=true
        assert!(
            cc.stuck_to_bottom,
            "scrolling back to the last page must re-engage follow-tail"
        );
    }

    // ── push_turn / push_turn_markdown ───────────────────────────────

    #[test]
    fn push_turn_appends_turn_to_transcript() {
        let mut cc = ChatController::new("c");
        cc.push_turn(ChatRole::User, StyledText::plain("hello"));
        assert_eq!(cc.transcript.len(), 1);
        assert_eq!(cc.transcript[0].role, ChatRole::User);
        let text: String = cc.transcript[0]
            .text
            .spans
            .iter()
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(text, "hello");
    }

    #[test]
    fn push_turn_multiple_turns_accumulate() {
        let mut cc = ChatController::new("c");
        cc.push_turn(ChatRole::User, StyledText::plain("first"));
        cc.push_turn(ChatRole::Assistant, StyledText::plain("second"));
        assert_eq!(cc.transcript.len(), 2);
        assert_eq!(cc.transcript[0].role, ChatRole::User);
        assert_eq!(cc.transcript[1].role, ChatRole::Assistant);
    }

    #[test]
    fn push_turn_markdown_joins_lines_with_newline() {
        let mut cc = ChatController::new("c");
        let theme = crate::Theme::default();
        cc.push_turn_markdown(ChatRole::Assistant, "line one\nline two", &theme);
        assert_eq!(cc.transcript.len(), 1);
        assert_eq!(cc.transcript[0].role, ChatRole::Assistant);
        // Span texts joined must contain both lines with a '\n' separator.
        let text: String = cc.transcript[0]
            .text
            .spans
            .iter()
            .map(|s| s.text.as_str())
            .collect();
        assert!(
            text.contains("line one"),
            "first line must be present; got: {text:?}"
        );
        assert!(
            text.contains("line two"),
            "second line must be present; got: {text:?}"
        );
        assert!(
            text.contains('\n'),
            "lines must be joined with a newline separator; got: {text:?}"
        );
    }

    #[test]
    fn push_turn_markdown_structural_glyphs_survive_in_plain_text() {
        // The ChatController renders transcript rows by concatenating span
        // text — structural glyphs (bullets, blockquote bars) must survive
        // so they appear as text cues in the rendered transcript.
        let mut cc = ChatController::new("c");
        let theme = crate::Theme::default();
        cc.push_turn_markdown(ChatRole::Assistant, "- item one\n- item two", &theme);
        let text: String = cc.transcript[0]
            .text
            .spans
            .iter()
            .map(|s| s.text.as_str())
            .collect();
        assert!(
            text.contains('\u{2022}'),
            "bullet glyph must survive as a structural text cue; got: {text:?}"
        );
        assert!(
            text.contains("item one"),
            "first item content must be present; got: {text:?}"
        );
    }

    #[test]
    fn push_turn_markdown_empty_string_produces_one_turn_no_panic() {
        let mut cc = ChatController::new("c");
        let theme = crate::Theme::default();
        // Empty markdown must not panic and must append exactly one turn.
        cc.push_turn_markdown(ChatRole::System, "", &theme);
        assert_eq!(cc.transcript.len(), 1);
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

    // ── Styled transcript rows ────────────────────────────────────────

    #[test]
    fn push_turn_markdown_stores_line_scales() {
        let mut cc = ChatController::new("c");
        let theme = crate::Theme::default();
        cc.push_turn_markdown(ChatRole::Assistant, "# Heading\nbody", &theme);
        let turn = &cc.transcript[0];
        // Markdown adapter produces 2 lines → 2 scales.
        assert_eq!(
            turn.line_scales.len(),
            2,
            "line_scales must have one entry per markdown line; got {:?}",
            turn.line_scales
        );
        // H1 → scale 2.0.
        assert!(
            (turn.line_scales[0] - 2.0).abs() < f32::EPSILON,
            "H1 line_scale must be 2.0; got {}",
            turn.line_scales[0]
        );
        // Body → scale 1.0.
        assert!(
            (turn.line_scales[1] - 1.0).abs() < f32::EPSILON,
            "body line_scale must be 1.0; got {}",
            turn.line_scales[1]
        );
    }

    #[test]
    fn push_turn_has_empty_line_scales() {
        // Turns created via push_turn must have empty line_scales so the
        // flat render path is used — invariant for existing callers.
        let mut cc = ChatController::new("c");
        cc.push_turn(ChatRole::User, StyledText::plain("plain text"));
        assert!(
            cc.transcript[0].line_scales.is_empty(),
            "push_turn must leave line_scales empty"
        );
    }

    #[test]
    fn build_transcript_rows_flat_path_unchanged() {
        // Turns with empty line_scales must produce rows with empty spans
        // — bit-for-bit identical to the pre-styled-row behaviour.
        let mut cc = ChatController::new("c");
        cc.set_transcript(vec![make_turn(ChatRole::User, "hello")]);
        let rows = cc.build_transcript_rows(80);
        // Content row at index 1 (after the "You" header).
        let content_row = rows.iter().find(|r| r.text == "hello").unwrap();
        assert!(
            content_row.spans.is_empty(),
            "flat-path rows must have empty spans; got {:?}",
            content_row.spans
        );
        assert!(
            (content_row.scale - 1.0).abs() < f32::EPSILON,
            "flat-path rows must have scale 1.0"
        );
    }

    #[test]
    fn build_transcript_rows_styled_path_produces_spans() {
        // Turns created by push_turn_markdown must produce rows with non-empty
        // spans so rasterisers can apply per-span styling.
        let mut cc = ChatController::new("c");
        let theme = crate::Theme::default();
        cc.push_turn_markdown(ChatRole::Assistant, "**bold** text", &theme);
        let rows = cc.build_transcript_rows(80);
        // Skip the "AI" role header row, find the content row.
        let content_rows: Vec<_> = rows.iter().filter(|r| r.indent > 0.0).collect();
        assert!(
            !content_rows.is_empty(),
            "expected at least one content row"
        );
        let first = &content_rows[0];
        assert!(
            !first.spans.is_empty(),
            "markdown turns must produce rows with non-empty spans; got empty"
        );
        // The bold span must appear somewhere in the row spans.
        let has_bold = first.spans.iter().any(|s| s.bold);
        assert!(
            has_bold,
            "expected a bold span in the row; spans: {:?}",
            first.spans
        );
    }

    #[test]
    fn build_transcript_rows_heading_carries_scale() {
        // H1 markdown line → content rows must have scale 2.0.
        let mut cc = ChatController::new("c");
        let theme = crate::Theme::default();
        cc.push_turn_markdown(ChatRole::Assistant, "# Title", &theme);
        let rows = cc.build_transcript_rows(80);
        let content_rows: Vec<_> = rows.iter().filter(|r| r.indent > 0.0).collect();
        assert!(!content_rows.is_empty(), "expected content rows");
        let heading_row = &content_rows[0];
        assert!(
            (heading_row.scale - 2.0).abs() < f32::EPSILON,
            "H1 content row must have scale 2.0; got {}",
            heading_row.scale
        );
    }

    #[test]
    fn build_transcript_rows_plain_role_header_has_no_spans() {
        // Role headers ("You", "AI", "System") must always be flat rows —
        // they are inserted by the controller, not from user content.
        let mut cc = ChatController::new("c");
        let theme = crate::Theme::default();
        cc.push_turn_markdown(ChatRole::User, "**bold**", &theme);
        let rows = cc.build_transcript_rows(80);
        let header = rows.iter().find(|r| r.text == "You").unwrap();
        assert!(
            header.spans.is_empty(),
            "role header must be a flat row; spans: {:?}",
            header.spans
        );
    }

    #[test]
    fn build_transcript_rows_styled_wraps_across_budget() {
        // A styled turn wider than the column budget must produce multiple rows.
        let mut cc = ChatController::new("c");
        let theme = crate::Theme::default();
        // 10 plain chars; col_budget=7 → content_budget=5 → 2 rows.
        cc.push_turn_markdown(ChatRole::Assistant, "1234567890", &theme);
        let rows = cc.build_transcript_rows(7);
        let content_rows: Vec<_> = rows.iter().filter(|r| r.indent > 0.0).collect();
        assert_eq!(
            content_rows.len(),
            2,
            "10-char styled line at budget=5 must wrap to 2 rows; got {}",
            content_rows.len()
        );
    }

    #[test]
    fn build_transcript_rows_styled_wraps_cjk_at_display_width() {
        // Styled (markdown) turns wrap via `text_util::wrap_spans` with
        // `WrapPolicy::Char`, budgeting by display width (#821). 6 CJK
        // characters are 6 chars but 12 display cells — a
        // `chars().count()`-based budget would wrongly let them all
        // through on one row at content_budget=6; the display-width-aware
        // wrapper must split after 3 characters (6 cells).
        let mut cc = ChatController::new("c");
        let theme = crate::Theme::default();
        cc.push_turn_markdown(ChatRole::Assistant, "**中文中文中文**", &theme);
        let rows = cc.build_transcript_rows(8); // content_budget = 8 - 2 = 6
        let content_rows: Vec<_> = rows.iter().filter(|r| r.indent > 0.0).collect();
        assert_eq!(
            content_rows.len(),
            2,
            "6 CJK chars (12 cells) at a 6-cell content budget must wrap to \
             2 rows, not 1; got {}",
            content_rows.len()
        );
        for row in &content_rows {
            assert!(
                crate::text_util::display_width(&row.text) <= 6,
                "row {:?} exceeds the 6-cell budget",
                row.text
            );
        }
    }

    // ── split_spans_by_newline ────────────────────────────────────────

    #[test]
    fn split_spans_by_newline_single_line() {
        let spans = vec![StyledSpan::plain("hello")];
        let lines = split_spans_by_newline(&spans);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), 1);
        assert_eq!(lines[0][0].text, "hello");
    }

    #[test]
    fn split_spans_by_newline_two_lines() {
        let spans = vec![
            StyledSpan::plain("line1"),
            StyledSpan::plain("\n"),
            StyledSpan::plain("line2"),
        ];
        let lines = split_spans_by_newline(&spans);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0][0].text, "line1");
        assert_eq!(lines[1][0].text, "line2");
    }

    #[test]
    fn split_spans_by_newline_empty_produces_one_empty_line() {
        let lines = split_spans_by_newline(&[]);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].is_empty());
    }

    // `wrap_spans`/`WrapPolicy` themselves are unit-tested in
    // `text_util.rs` (#821) — this module no longer owns a wrap
    // implementation, only the `WrapPolicy::Char` call site exercised by
    // `build_transcript_rows_styled_wraps_across_budget` and
    // `build_transcript_rows_styled_wraps_cjk_at_display_width` below.

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

    // ── Input soft-wrap (#1136) ────────────────────────────────────────

    #[test]
    fn wrap_line_exact_short_line_not_split() {
        assert_eq!(wrap_line_exact("hi", 10), vec![(0, "hi".to_string())]);
    }

    #[test]
    fn wrap_line_exact_breaks_at_space() {
        // "hello world" (11 chars) at budget 7 breaks after "hello " (the
        // space stays with the first row) leaving "world" on the second.
        assert_eq!(
            wrap_line_exact("hello world", 7),
            vec![(0, "hello ".to_string()), (6, "world".to_string())]
        );
    }

    #[test]
    fn wrap_line_exact_hard_breaks_a_single_long_word() {
        assert_eq!(
            wrap_line_exact("abcdefgh", 3),
            vec![
                (0, "abc".to_string()),
                (3, "def".to_string()),
                (6, "gh".to_string())
            ]
        );
    }

    #[test]
    fn wrap_line_exact_reconstructs_original_text() {
        // The defining property: concatenating every row's text
        // reconstructs the input exactly — no whitespace collapsed or
        // dropped (unlike `word_wrap`), so cursor round-trips stay exact.
        let line = "the quick brown fox jumps over the lazy dog";
        for budget in 1..line.chars().count() + 2 {
            let rows = wrap_line_exact(line, budget);
            let joined: String = rows.iter().map(|(_, t)| t.as_str()).collect();
            assert_eq!(joined, line, "budget={budget}");
        }
    }

    #[test]
    fn wrap_input_rows_one_row_per_short_logical_line() {
        let rows = wrap_input_rows("ab\ncd", 80);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].logical_line, 0);
        assert_eq!(rows[1].logical_line, 1);
    }

    #[test]
    fn wrap_input_rows_empty_buffer_yields_one_empty_row() {
        let rows = wrap_input_rows("", 80);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text, "");
    }

    #[test]
    fn logical_to_visual_round_trips_through_visual_to_byte() {
        let text = "hello world\nsecond line";
        let rows = wrap_input_rows(text, 6); // forces "hello world" to wrap
        assert!(rows.len() > 2, "expected wrapping to produce >2 rows");

        // Cursor at byte offset 8 ("hello wo|rld...") -> logical (0, 8).
        let (line, col) = cursor_byte_to_line_col(text, 8);
        assert_eq!((line, col), (0, 8));

        let (row_idx, visual_col) = input_logical_to_visual(&rows, line, col);
        let byte = input_visual_to_byte(text, &rows, row_idx, visual_col);
        assert_eq!(byte, 8, "round trip through visual space must be lossless");
    }

    #[test]
    fn logical_to_visual_end_of_line_lands_on_last_visual_row() {
        let text = "hello world";
        let rows = wrap_input_rows(text, 6); // wraps into 2 rows
        let last_row = rows.len() - 1;
        let (line, col) = cursor_byte_to_line_col(text, text.len());
        let (row_idx, _) = input_logical_to_visual(&rows, line, col);
        assert_eq!(row_idx, last_row);
    }

    // ── Up/Down move by VISUAL row when wrapped (#1136) ────────────────

    #[test]
    fn up_within_wrapped_paragraph_moves_cursor_not_history() {
        let mut cc = ChatController::new("c");
        cc.set_input_height_range(1, 8);
        cc.input_insert_str("hello world this wraps");
        cc.history.push("prev".into());
        // Narrow rect forces wrapping: content width well under the text length.
        let rect = Rect::new(0.0, 0.0, 12.0, 24.0);
        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Up),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let ev = cc.handle(&event, &RecordingBackend::new(), rect);
        assert_eq!(ev, ChatControllerEvent::Consumed);
        // History must NOT have been entered — the cursor started on a
        // later visual row of the wrapped paragraph, not the first one.
        assert_eq!(cc.history_pos, None);
        assert_eq!(cc.input_text(), "hello world this wraps");
    }

    #[test]
    fn up_on_first_visual_row_of_wrapped_paragraph_enters_history() {
        let mut cc = ChatController::new("c");
        cc.input_insert_str("hello world this wraps");
        cc.input_cursor = 3; // still within the first wrapped row
        cc.history.push("prev".into());
        let rect = Rect::new(0.0, 0.0, 12.0, 24.0);
        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Up),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let ev = cc.handle(&event, &RecordingBackend::new(), rect);
        assert_eq!(ev, ChatControllerEvent::Consumed);
        assert_eq!(cc.input_text(), "prev", "should have entered history");
    }

    #[test]
    fn down_on_last_visual_row_of_wrapped_paragraph_enters_history_next() {
        let mut cc = ChatController::new("c");
        // A wrapped paragraph sitting in `saved_input`, restored once Down
        // walks off the newest history entry.
        cc.input_insert_str("hello world this wraps");
        cc.history.push("first".into());
        cc.history.push("second".into());
        let rect = Rect::new(0.0, 0.0, 12.0, 24.0);
        // Drive `history_prev` directly (bypassing the Up key) to reach a
        // deterministic "navigating the newest entry" state.
        cc.history_prev();
        assert_eq!(cc.input_text(), "second");

        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Down),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let ev = cc.handle(&event, &RecordingBackend::new(), rect);
        assert_eq!(ev, ChatControllerEvent::Consumed);
        // "second" fits on one visual row even at width 12, so Down from
        // its only (first==last) row walks off the newest history entry
        // and restores the saved wrapped paragraph.
        assert_eq!(cc.history_pos, None);
        assert_eq!(cc.input_text(), "hello world this wraps");
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

    /// Regression for issue #503: `cursor_byte_to_line_col` used to
    /// clamp `cursor` with only `.min(text.len())`, not a char-boundary
    /// snap — a cursor byte offset landing mid-multibyte-character
    /// (here, inside "é") panicked `&text[..cursor]`.
    #[test]
    fn byte_to_line_col_multibyte_mid_char_offset_does_not_panic() {
        let text = "héllo";
        assert!(!text.is_char_boundary(2)); // inside the 2-byte 'é'
        let (line, col) = cursor_byte_to_line_col(text, 2);
        assert_eq!((line, col), (0, 1));
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
        let layout = cc.compute_layout(&RecordingBackend::new(), make_rect());
        assert_eq!(layout.status.y, 0.0);
        assert_eq!(layout.status.height, 1.0); // line_height = 1.0
    }

    #[test]
    fn layout_input_at_bottom() {
        let cc = ChatController::new("c");
        let layout = cc.compute_layout(&RecordingBackend::new(), make_rect());
        // Auto-grow (#1136): empty input wraps to 1 visual row, clamped
        // to the default input_min_rows=1. lh=1, border=2 → input_h = 3.
        let expected_input_y = make_rect().height - (1.0 * 1.0 + 2.0);
        assert_eq!(layout.input.y, expected_input_y);
    }

    #[test]
    fn layout_input_grows_with_wrapped_row_count() {
        let mut cc = ChatController::new("c");
        cc.input_insert_str("line one\nline two\nline three");
        let layout = cc.compute_layout(&RecordingBackend::new(), make_rect());
        // 3 short logical lines, wide rect → 3 visual rows, no wrapping.
        // lh=1, border=2 → input_h = 5.
        let expected_input_y = make_rect().height - (3.0 * 1.0 + 2.0);
        assert_eq!(layout.input.y, expected_input_y);
    }

    #[test]
    fn layout_input_clamps_to_max_rows() {
        let mut cc = ChatController::new("c");
        for _ in 0..20 {
            cc.input_insert_char('\n');
        }
        // 21 logical lines (20 newlines) would need 21 visual rows, but
        // the default max is 8.
        let layout = cc.compute_layout(&RecordingBackend::new(), make_rect());
        let expected_input_y = make_rect().height - (8.0 * 1.0 + 2.0);
        assert_eq!(layout.input.y, expected_input_y);
    }

    #[test]
    fn set_input_height_range_overrides_default_clamp() {
        let mut cc = ChatController::new("c");
        cc.set_input_height_range(2, 3);
        let layout = cc.compute_layout(&RecordingBackend::new(), make_rect());
        // Empty input wraps to 1 visual row, clamped up to the new min=2.
        let expected_input_y = make_rect().height - (2.0 * 1.0 + 2.0);
        assert_eq!(layout.input.y, expected_input_y);
    }

    #[test]
    fn set_input_height_range_floors_max_to_min() {
        let mut cc = ChatController::new("c");
        cc.set_input_height_range(5, 2); // max < min — max floored up to min.
        assert_eq!(cc.input_min_rows, 5);
        assert_eq!(cc.input_max_rows, 5);
    }

    #[test]
    fn layout_no_scrollbar_when_transcript_empty() {
        let cc = ChatController::new("c");
        let layout = cc.compute_layout(&RecordingBackend::new(), make_rect());
        assert!(layout.scrollbar.is_none());
    }

    #[test]
    fn layout_scrollbar_present_when_transcript_nonempty() {
        let mut cc = ChatController::new("c");
        cc.set_transcript(vec![make_turn(ChatRole::User, "hello")]);
        let layout = cc.compute_layout(&RecordingBackend::new(), make_rect());
        assert!(layout.scrollbar.is_some());
        // Transcript rect should be narrower than full width.
        assert!(layout.transcript.width < make_rect().width);
    }

    #[test]
    fn layout_spinner_present_when_busy() {
        let mut cc = ChatController::new("c");
        cc.set_busy(true);
        let layout = cc.compute_layout(&RecordingBackend::new(), make_rect());
        assert!(layout.spinner.is_some());
    }

    #[test]
    fn layout_spinner_absent_when_not_busy() {
        let cc = ChatController::new("c");
        let layout = cc.compute_layout(&RecordingBackend::new(), make_rect());
        assert!(layout.spinner.is_none());
    }

    // ── Backend rendering tests ───────────────────────────────────────
    //
    // The tests above exercise state/layout against a `RecordingBackend` whose
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
