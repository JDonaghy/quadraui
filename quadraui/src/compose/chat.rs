//! Chat: a transcript, a status strip and a multi-line input, as one
//! composite widget ([`Chat`]).
//!
//! [`Chat`] is a controlled descriptor: the transcript, the status strip,
//! the input buffer and caret, the transcript scroll position, the
//! focused turn and the collapsed turns are its fields, and every change
//! a gesture makes comes back as a [`ChatEvent`] the app writes onto
//! them ([`Chat::apply`] does that). [`ChatState`] holds only what no
//! app reads: the input history ring and the gesture in flight.
//!
//! Each part is its own widget, painted through its own `Primitive` and
//! driven through its own `Interactive`: the status strip, transcript
//! and hint are [`MessageList`]s, the busy indicator is a [`Spinner`],
//! the transcript scrollbar a [`Scrollbar`], the input a [`TextInput`]
//! and the Send/Stop segment a [`Button`].
//!
//! # Keyboard behaviour
//!
//! By default ([`Chat::submit_on_enter`] unset):
//!
//! - `Ctrl+S`, `Alt+Enter`, or `Ctrl+Enter` — submit the current input.
//!   `Ctrl+S` and `Alt+Enter` work on most terminals; `Ctrl+Enter` requires Kitty
//!   keyboard protocol (supported by kitty, Alacritty ≥0.12, WezTerm, foot).
//!   **Note**: `Ctrl+S` is the XON/XOFF flow-control suspend key in some
//!   terminal configurations (`stty -ixon` disables it). If `Ctrl+S` appears
//!   to freeze the terminal, run `stty -ixon` or use `Alt+Enter` instead.
//! - `Enter` — insert a newline in the input.
//!
//! With [`Chat::submit_on_enter`] set (Enter sends):
//!
//! - `Enter` (no modifiers) — submit the current input.
//! - `Shift+Enter` — insert a newline. `Alt+Enter` is accepted as a
//!   fallback for terminals that cannot report `Shift+Enter`.
//! - `Ctrl+S` / `Ctrl+Enter` still submit, same as the default mode.
//!
//! In both modes:
//!
//! - `Esc` — emit [`ChatEvent::Cancelled`]; the app decides whether to
//!   close the overlay.
//! - The input soft-wraps long lines to fit the box, and `↑`/`↓` move by
//!   **visual** row, not logical line.
//! - `↑` (when the cursor is on the first visual row of the whole
//!   buffer) — navigate to the previous history entry.
//! - `↓` (when the cursor is on the last visual row of the whole buffer)
//!   — navigate to the next history entry or restore the saved input.
//! - `PageUp` / `PageDown` — scroll the transcript.
//! - Every other editing key goes through the shared text-edit core
//!   ([`crate::behaviour::text_edit`]), the same one every other text
//!   widget uses: `Ctrl+Left`/`Ctrl+Right`/`Ctrl+Backspace`/
//!   `Ctrl+Delete` work by word (letters, digits and `_`), `Ctrl+A`
//!   selects all, `Ctrl+C`/`Ctrl+X` copy and cut a selection, and a
//!   press or drag in the input places the caret or selects.
//!
//! # Scroll behaviour
//!
//! Mouse-wheel events (positive `delta.y` = scroll up) scroll the
//! transcript through the transcript [`MessageList`]'s own wheel
//! handling. While [`Chat::stuck_to_bottom`] is set the transcript
//! follows its tail: the painted position is the last page, whatever
//! [`Chat::transcript_scroll_top`] says.
//!
//! # Send / Stop segment
//!
//! A [`Button`] is painted at the right of the input row. It reads
//! **Send** normally and **Stop** while [`Chat::busy`] is set. Clicking
//! it while idle behaves exactly like a keyboard submit (or is ignored
//! when the input is empty, where the button is disabled); while busy it
//! emits [`ChatEvent::StopRequested`] instead. The segment is omitted
//! (and the input takes the full row width) when the row is too narrow
//! to fit both.
//!
//! # Persistent hint line
//!
//! [`Chat::hint`] is an optional one-row hint painted directly above the
//! input box. Unlike the input placeholder it stays visible while typing.
//!
//! # Collapsible turns and per-turn hit regions
//!
//! Per-turn collapsed/summary state lives on the [`Chat`] descriptor,
//! keyed by transcript index, not on [`ChatTurn`]. A collapsed turn
//! paints as a one-line card — the role header with a `▸` fold indicator
//! plus [`Chat::turn_summary_line`] — instead of its full body.
//!
//! A click anywhere in the transcript resolves to the turn and in-turn row
//! it landed on: [`ChatEvent::TurnClicked`]`{ turn_idx, row_in_turn }`,
//! and also moves [`Chat::focused_turn`] to `Some(turn_idx)` so a
//! following keyboard `Enter` acts on the same turn. Each turn's row
//! block ends with the blank inter-turn separator row, folded into the
//! *preceding* turn. The widget does not toggle `collapsed` on click;
//! that decision is the app's.
//!
//! [`Chat::focused_turn`] is a keyboard cursor into the transcript: `Tab`
//! (and `Shift+Tab` / `BackTab`) cycle it through `None` (input has
//! focus) → turn `0` → … → the last turn → back to `None`. While a turn
//! is focused, plain `Enter` toggles that turn's collapsed state instead
//! of submitting or inserting a newline. Editing the input always
//! returns focus to the input.

use crate::a11y::{A11yInfo, A11yPrimitive, A11yRole};
use crate::behaviour::text_edit::{
    handle_lines_edit_event, Caret, EditIntent, EditView, FlatEditOutcome, LinesEditState,
};
use crate::compose::markdown::render_markdown_to_styled;
use crate::compose::widget_host::{self, ChildHost};
use crate::compose::Handled;
use crate::interaction::InteractionState;
use crate::interactive::{
    EventCx, FocusSpec, FocusStop, Interactive, KeyEvent, PointerKind, RoutedEvent, SubTarget,
    WidgetAction,
};
use crate::primitive::{LayoutCtx, PaintEnv, Primitive};
use crate::primitives::button::{Button, ButtonEvent, ButtonLayout};
use crate::primitives::message_list::{MessageListEvent, MessageListLayout};
use crate::primitives::scrollbar::{
    scroll_by_clamped, ScrollbarEvent, ScrollbarLayout, ScrollbarState,
};
use crate::primitives::spinner::SpinnerLayout;
use crate::primitives::text_input::TextInputPrimitiveLayout;
use crate::text_util::{wrap_spans, WrapPolicy};
use crate::theme::Theme;
use crate::types::StyledSpan;
use crate::{
    Color, EditOp, Key, MessageList, MessageListHit, MessageListMeasure, MessageRow, Modifiers,
    MouseButton, NamedKey, Point, Rect, Scrollbar, Spinner, StyledText, TextInput, UiEvent,
    WidgetId,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

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
/// When `line_scales` is non-empty (turns created via
/// [`Chat::push_turn_markdown`]), the transcript builds styled
/// [`MessageRow`]s with per-span fg/bold/italic and per-line heading
/// scale; otherwise it concatenates span text (the flat path).
///
/// Collapsed/summary state lives on [`Chat`], keyed by transcript index,
/// not here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatTurn {
    pub role: ChatRole,
    pub text: StyledText,
    /// Unix epoch seconds; `None` when not recorded.
    pub timestamp_unix: Option<f64>,
    /// Per-line font-scale factors, parallel to the logical lines in `text`.
    ///
    /// Non-empty only for turns created by [`Chat::push_turn_markdown`]
    /// (where the markdown adapter supplies one scale per input line).  `1.0` for
    /// body lines; `2.0` / `1.5` / `1.2` for H1 / H2 / H3 headings.
    #[serde(default)]
    pub line_scales: Vec<f32>,
}

/// What a [`Chat`] reports.
///
/// The first six are the chat's own semantic events. The rest are the
/// controlled-state write-backs: each names a descriptor field a gesture
/// changed, for the app to store ([`Chat::apply`] stores them all).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub enum ChatEvent {
    /// The input was submitted. Carries the full input text (with `\n`
    /// between lines). The app appends it to its transcript and clears
    /// the input.
    Submit { text: String },
    /// Stream chunk (reserved for a backend-push path; an app that owns
    /// the session pipe updates [`Chat::transcript`] directly).
    StreamChunk { text: String },
    /// `Esc` was pressed. The app decides whether to close the overlay.
    Cancelled,
    /// The Send/Stop segment was clicked while [`Chat::busy`] was set:
    /// the app stops its in-flight generation. The input is untouched.
    StopRequested,
    /// A key press the chat did not consume, for the app's own hotkeys.
    KeyPressed { key: String, modifiers: Modifiers },
    /// A click landed on transcript row `row_in_turn` of turn `turn_idx`
    /// (`0` = the role header row; the trailing blank separator row
    /// counts as part of the turn).
    TurnClicked { turn_idx: usize, row_in_turn: usize },
    /// The input buffer, caret or selection changed.
    InputChanged {
        lines: Vec<String>,
        cursor: Caret,
        anchor: Option<Caret>,
    },
    /// The transcript scrolled to `scroll_top`; `stuck_to_bottom` says
    /// whether it now follows the tail.
    TranscriptScrolled {
        scroll_top: usize,
        stuck_to_bottom: bool,
    },
    /// The keyboard focus cursor moved onto turn `turn`, or back to the
    /// input (`None`).
    FocusedTurnChanged { turn: Option<usize> },
    /// Turn `turn_idx` was collapsed or expanded from the keyboard.
    TurnCollapsed { turn_idx: usize, collapsed: bool },
}

impl WidgetAction for ChatEvent {
    fn into_ui_event(self, id: WidgetId) -> UiEvent {
        UiEvent::Chat(id, self)
    }
}

// ── Descriptor ─────────────────────────────────────────────────────────────────

/// A chat overlay: a 1-row status strip at the top, a scrollable
/// transcript in the middle, an optional hint row and a multi-line input
/// with a Send/Stop segment at the bottom.
///
/// Every visible piece of state is a field; see the module doc for the
/// controlled-state convention and the keyboard map.
///
/// # Wrapping
///
/// Plain-text turns are **word-wrapped** via
/// [`crate::text_util::word_wrap`]. Styled turns (from
/// [`Self::push_turn_markdown`]) wrap per-span at exactly the
/// display-width budget ([`crate::text_util::wrap_spans`] with
/// [`WrapPolicy::Char`]).
///
/// # Input soft-wrap and auto-grow
///
/// The painted [`TextInput`] is built from the *wrapped* buffer: the
/// logical [`Self::input_lines`] are soft-wrapped to the box's width, so
/// long lines wrap instead of scrolling horizontally. Wrapping is exact —
/// a visual row is always a contiguous char range of its logical line —
/// so caret positions map losslessly between the logical `(line, col)`
/// space [`Self::input_cursor`] is in and the visual rows painted.
///
/// The input box's height is the wrapped visual row count, clamped to
/// [`Self::input_rows`] (default `1..=8`); beyond the ceiling the box
/// stops growing and the input scrolls inside it.
#[derive(Debug, Clone)]
pub struct Chat {
    pub id: WidgetId,
    /// The transcript, oldest turn first.
    pub transcript: Vec<ChatTurn>,
    /// The status-strip context label (e.g. `"Refining issue #N"`).
    pub status: StyledText,
    /// Whether a generation is in flight: shows the spinner and turns
    /// the Send segment into Stop.
    pub busy: bool,
    /// The model chip in the status strip; empty for none.
    pub model_label: String,
    /// The spinner's animation frame.
    pub spinner_frame: usize,
    /// The input buffer's logical lines; never empty.
    pub input_lines: Vec<String>,
    /// The input caret, as a logical `(line, char column)`.
    pub input_cursor: Caret,
    /// The input selection's fixed end, if a selection is active.
    pub input_anchor: Option<Caret>,
    /// First visible visual row of the input box.
    pub input_scroll_offset: usize,
    /// Whether the input shows its caret.
    pub input_has_focus: bool,
    /// The transcript's keyboard focus cursor: `Some(turn)` while `Tab`
    /// or a transcript click parked focus on a turn.
    pub focused_turn: Option<usize>,
    /// Transcript indices of the collapsed turns.
    pub collapsed_turns: BTreeSet<usize>,
    /// Per-turn summary-line overrides, keyed by transcript index.
    pub turn_summaries: BTreeMap<usize, String>,
    /// First visible wrapped transcript row, while not following the tail.
    pub transcript_scroll_top: usize,
    /// Whether the transcript follows its tail.
    pub stuck_to_bottom: bool,
    /// The input box's auto-grow `(min, max)` visual row clamp. `min` is
    /// floored to `1` and `max` to `min`.
    pub input_rows: (usize, usize),
    /// Fixed scrollbar track width, or `None` for one line height.
    pub scrollbar_width: Option<f32>,
    /// Whether plain `Enter` submits (and `Shift+Enter` breaks the line).
    pub submit_on_enter: bool,
    /// The persistent hint row above the input; `None` omits the row.
    pub hint: Option<StyledText>,
}

impl Chat {
    /// An empty chat: no transcript, an empty focused input, following
    /// the tail.
    pub fn new(id: impl Into<WidgetId>) -> Self {
        Self {
            id: id.into(),
            transcript: Vec::new(),
            status: StyledText::plain(""),
            busy: false,
            model_label: String::new(),
            spinner_frame: 0,
            input_lines: vec![String::new()],
            input_cursor: (0, 0),
            input_anchor: None,
            input_scroll_offset: 0,
            input_has_focus: true,
            focused_turn: None,
            collapsed_turns: BTreeSet::new(),
            turn_summaries: BTreeMap::new(),
            transcript_scroll_top: 0,
            stuck_to_bottom: true,
            input_rows: (1, 8),
            scrollbar_width: None,
            submit_on_enter: false,
            hint: None,
        }
    }

    /// Store a write-back event's new field values. The semantic events
    /// change nothing.
    pub fn apply(&mut self, event: &ChatEvent) {
        match event {
            ChatEvent::InputChanged {
                lines,
                cursor,
                anchor,
            } => {
                self.input_lines = if lines.is_empty() {
                    vec![String::new()]
                } else {
                    lines.clone()
                };
                self.input_cursor = *cursor;
                self.input_anchor = *anchor;
            }
            ChatEvent::TranscriptScrolled {
                scroll_top,
                stuck_to_bottom,
            } => {
                self.transcript_scroll_top = *scroll_top;
                self.stuck_to_bottom = *stuck_to_bottom;
            }
            ChatEvent::FocusedTurnChanged { turn } => {
                self.focused_turn = *turn;
                self.input_has_focus = turn.is_none();
            }
            ChatEvent::TurnCollapsed {
                turn_idx,
                collapsed,
            } => {
                self.set_turn_collapsed(*turn_idx, *collapsed);
            }
            _ => {}
        }
    }

    /// The input buffer as one string, with `\n` between lines.
    pub fn input_text(&self) -> String {
        self.input_lines.join("\n")
    }

    /// Append a pre-styled turn.
    pub fn push_turn(&mut self, role: ChatRole, text: StyledText) {
        self.transcript.push(ChatTurn {
            role,
            text,
            timestamp_unix: None,
            line_scales: Vec::new(),
        });
    }

    /// Append a turn whose body is markdown, rendered through
    /// [`render_markdown_to_styled`]. The rendered lines are joined into
    /// one [`StyledText`] with `'\n'` separator spans, and their heading
    /// scales are kept as [`ChatTurn::line_scales`].
    pub fn push_turn_markdown(&mut self, role: ChatRole, markdown: &str, theme: &Theme) {
        let rendered = render_markdown_to_styled(markdown, theme);
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

    /// Whether `transcript[turn_idx]` is collapsed; `false` for an
    /// out-of-range index.
    pub fn is_turn_collapsed(&self, turn_idx: usize) -> bool {
        turn_idx < self.transcript.len() && self.collapsed_turns.contains(&turn_idx)
    }

    /// Override `transcript[turn_idx]`'s summary line; `None` falls back to
    /// its first non-blank line.
    pub fn set_turn_summary(&mut self, turn_idx: usize, summary: Option<String>) {
        match summary {
            Some(s) => {
                self.turn_summaries.insert(turn_idx, s);
            }
            None => {
                self.turn_summaries.remove(&turn_idx);
            }
        }
    }

    /// Flip `transcript[turn_idx]` between collapsed and expanded; a no-op
    /// out of range.
    pub fn toggle_turn_collapsed(&mut self, turn_idx: usize) {
        self.set_turn_collapsed(turn_idx, !self.is_turn_collapsed(turn_idx));
    }

    /// Empty the input: no text, the caret at the start, no selection and
    /// the box scrolled to its top. Apps call this after handling a
    /// [`ChatEvent::Submit`].
    pub fn clear_input(&mut self) {
        self.apply(&self.set_input("", None));
        self.input_scroll_offset = 0;
    }

    /// Collapse or expand `transcript[turn_idx]`; a no-op out of range.
    pub fn set_turn_collapsed(&mut self, turn_idx: usize, collapsed: bool) {
        if turn_idx >= self.transcript.len() {
            return;
        }
        if collapsed {
            self.collapsed_turns.insert(turn_idx);
        } else {
            self.collapsed_turns.remove(&turn_idx);
        }
    }

    /// The line a collapsed `transcript[turn_idx]` shows: its
    /// [`Self::turn_summaries`] override, else the first non-blank line of
    /// its plain text. Empty for an out-of-range index or a turn with no
    /// content.
    pub fn turn_summary_line(&self, turn_idx: usize) -> String {
        if let Some(s) = self.turn_summaries.get(&turn_idx) {
            return s.clone();
        }
        let Some(turn) = self.transcript.get(turn_idx) else {
            return String::new();
        };
        let plain: String = turn.text.spans.iter().map(|s| s.text.as_str()).collect();
        plain
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("")
            .to_string()
    }

    /// The scroll event that moves the transcript by `delta` wrapped rows
    /// from where it is painted, clamped to the last page; reaching the
    /// last page re-engages following the tail.
    pub fn scroll_transcript_by(
        &self,
        delta: isize,
        total_rows: usize,
        visible_rows: usize,
    ) -> ChatEvent {
        let max = transcript_max_offset(total_rows, visible_rows);
        let from = if self.stuck_to_bottom {
            max
        } else {
            self.transcript_scroll_top
        };
        let scroll_top = scroll_by_clamped(from, delta, max);
        ChatEvent::TranscriptScrolled {
            scroll_top,
            stuck_to_bottom: transcript_reached_bottom(scroll_top, max),
        }
    }

    /// The edit event `op` makes on the input, or `None` when it changes
    /// nothing.
    pub fn edit_input(&self, op: &EditOp) -> Option<ChatEvent> {
        let mut lines = self.input_lines.clone();
        let (mut line, mut col) = self.input_cursor;
        let mut anchor = self.input_anchor;
        let changed = EditView {
            lines: &mut lines,
            cursor_line: &mut line,
            cursor_col: &mut col,
            anchor: &mut anchor,
        }
        .apply(op);
        changed.then_some(ChatEvent::InputChanged {
            lines,
            cursor: (line, col),
            anchor,
        })
    }

    /// The event that replaces the whole input with `text`, the caret at
    /// `cursor` (clamped) or at the end, and no selection.
    pub fn set_input(&self, text: &str, cursor: Option<Caret>) -> ChatEvent {
        let lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        let last = lines.len() - 1;
        let cursor = match cursor {
            Some((line, col)) => {
                let line = line.min(last);
                (line, col.min(lines[line].chars().count()))
            }
            None => (last, lines[last].chars().count()),
        };
        ChatEvent::InputChanged {
            lines,
            cursor,
            anchor: None,
        }
    }

    // ── Row builders ──────────────────────────────────────────────────

    fn status_rows(&self) -> Vec<MessageRow> {
        let label: String = self.status.spans.iter().map(|s| s.text.as_str()).collect();
        let text = if self.model_label.is_empty() {
            label
        } else {
            format!("{label}  [{}]", self.model_label)
        };
        vec![MessageRow::new(text, Color::rgb(180, 180, 180), 0.0)]
    }

    /// The transcript's wrapped rows, plus the row each turn's block
    /// begins at: `turn_starts[i]..turn_starts[i + 1]` (or the end) is
    /// turn `i`'s row range.
    fn transcript_rows(&self, col_budget: usize) -> (Vec<MessageRow>, Vec<usize>) {
        let mut rows = Vec::new();
        let mut turn_starts = Vec::with_capacity(self.transcript.len());
        for (idx, turn) in self.transcript.iter().enumerate() {
            turn_starts.push(rows.len());

            let (role_label, role_fg, content_fg) = match turn.role {
                ChatRole::User => ("You", Color::rgb(100, 180, 255), Color::rgb(220, 220, 220)),
                ChatRole::Assistant => ("AI", Color::rgb(120, 220, 120), Color::rgb(210, 210, 210)),
                ChatRole::System => (
                    "System",
                    Color::rgb(200, 160, 80),
                    Color::rgb(190, 190, 190),
                ),
            };

            let collapsed = self.is_turn_collapsed(idx);
            let header = if collapsed {
                format!("{role_label} \u{25b8}")
            } else {
                role_label.to_string()
            };
            rows.push(MessageRow::new(header, role_fg, 0.0));

            if collapsed {
                rows.push(MessageRow::new(
                    self.turn_summary_line(idx),
                    content_fg,
                    2.0,
                ));
            } else {
                let content_budget = col_budget.saturating_sub(2);
                if !turn.line_scales.is_empty() {
                    // Styled path: per-line span lists with heading scales.
                    for (i, line_spans) in
                        split_spans_by_newline(&turn.text.spans).iter().enumerate()
                    {
                        let scale = turn.line_scales.get(i).copied().unwrap_or(1.0);
                        for group in wrap_spans(line_spans, content_budget, WrapPolicy::Char) {
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
                    // Flat path: plain text, word-wrapped.
                    let plain: String = turn.text.spans.iter().map(|s| s.text.as_str()).collect();
                    for raw_line in plain.split('\n') {
                        for wrapped in wrap_text(raw_line, content_budget) {
                            rows.push(MessageRow::new(wrapped, content_fg, 2.0));
                        }
                    }
                }
            }

            rows.push(MessageRow::new("", Color::rgb(50, 50, 50), 0.0));
        }
        (rows, turn_starts)
    }

    /// The painted input: `rows` (the wrapped buffer) with the caret
    /// mapped into visual space.
    fn text_input(&self, rows: &[InputRow]) -> TextInput {
        let (visual_row, visual_col) =
            input_logical_to_visual(rows, self.input_cursor.0, self.input_cursor.1);
        let mut ti = TextInput::new(WidgetId::new(format!("{}-input", self.id)));
        ti.lines = rows.iter().map(|r| r.text.clone()).collect();
        ti.cursor_line = visual_row;
        ti.cursor_col = visual_col;
        // The placeholder advertises the live submit binding.
        ti.placeholder = Some(
            if self.submit_on_enter {
                "Type a message\u{2026} (Enter to send, Shift+Enter for newline, Esc to cancel)"
            } else {
                "Type a message\u{2026} (Ctrl+S or Alt+Enter to send, Enter for newline, Esc to cancel)"
            }
            .into(),
        );
        ti.scroll_offset = self.input_scroll_offset;
        ti.scroll_col = 0;
        ti.has_focus = self.input_has_focus;
        ti
    }

    fn send_button(&self) -> Button {
        let mut button = Button::new(
            WidgetId::new(format!("{}-send", self.id)),
            if self.busy { "Stop" } else { "Send" },
        );
        button.disabled = !self.busy && self.input_text().trim().is_empty();
        button
    }

    fn scrollbar_track_width(&self, line_height: f32) -> f32 {
        self.scrollbar_width.unwrap_or(line_height)
    }
}

/// Width of the Send/Stop segment: 8 columns fits either label plus its
/// button chrome.
fn send_segment_width(char_width: f32) -> f32 {
    const SEND_SEGMENT_COLS: f32 = 8.0;
    SEND_SEGMENT_COLS * char_width.max(1.0)
}

fn transcript_col_budget(char_width: f32, rect: Rect) -> usize {
    if char_width <= 0.0 {
        80
    } else {
        ((rect.width / char_width).floor() as usize).max(1)
    }
}

fn transcript_visible_rows(line_height: f32, rect: Rect) -> usize {
    if line_height <= 0.0 {
        0
    } else {
        (rect.height / line_height).floor() as usize
    }
}

// ── State and layout ───────────────────────────────────────────────────────────

/// A pointer gesture the chat captured the pointer for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatDrag {
    /// A transcript scrollbar thumb drag.
    Scrollbar,
    /// A drag-select in the input.
    Input,
}

/// [`Interactive::State`] for [`Chat`]: the input history ring and the
/// gesture in flight. Everything painted is a [`Chat`] field.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct ChatState {
    /// Past submitted messages, oldest first.
    history: Vec<String>,
    /// The history entry the input shows, while navigating history.
    history_pos: Option<usize>,
    /// The input saved when history navigation began: `(text, line, col)`.
    saved_input: Option<(String, usize, usize)>,
    /// The shared text-edit core's per-focus state for the input.
    edit: LinesEditState,
    /// The transcript scrollbar's own state.
    scrollbar: ScrollbarState,
    drag: Option<ChatDrag>,
}

impl ChatState {
    /// Past submitted messages, oldest first.
    pub fn history(&self) -> &[String] {
        &self.history
    }

    /// The history entry the input shows, while navigating history.
    pub fn history_pos(&self) -> Option<usize> {
        self.history_pos
    }

    /// Forget history navigation: the next `↑` starts from the newest
    /// entry again.
    pub fn reset_history_navigation(&mut self) {
        self.history_pos = None;
        self.saved_input = None;
    }
}

/// The rects each part of a [`Chat`] occupies, in absolute coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChatZones {
    /// The 1-row status strip.
    pub status: Rect,
    /// The transcript rows.
    pub transcript: Rect,
    /// The transcript scrollbar, when the transcript is non-empty.
    pub scrollbar: Option<Rect>,
    /// The spinner, at the right of the status strip while busy.
    pub spinner: Option<Rect>,
    /// The hint row, when [`Chat::hint`] is set.
    pub hint: Option<Rect>,
    /// The input box.
    pub input: Rect,
    /// The Send/Stop segment, when the row is wide enough for it.
    pub send: Option<Rect>,
}

/// [`Primitive::Layout`] for [`Chat`]: every part's rect plus each child
/// widget laid out there.
#[derive(Debug, Clone)]
pub struct ChatLayout {
    /// The rect the chat was laid out into.
    pub area: Rect,
    /// Each part's rect.
    pub zones: ChatZones,
    /// The transcript row pitch.
    line_height: f32,
    status: (MessageList, MessageListLayout),
    spinner: Option<(Spinner, SpinnerLayout)>,
    transcript: (MessageList, MessageListLayout),
    turn_starts: Vec<usize>,
    visible_rows: usize,
    max_offset: usize,
    scrollbar: Option<(Scrollbar, ScrollbarLayout)>,
    hint: Option<(MessageList, MessageListLayout)>,
    input: (TextInput, TextInputPrimitiveLayout),
    input_rows: Vec<InputRow>,
    send: Option<(Button, ButtonLayout)>,
}

impl ChatLayout {
    /// The first transcript row painted — the last page while the chat
    /// follows its tail.
    pub fn transcript_scroll_top(&self) -> usize {
        self.transcript.0.scroll_top
    }

    /// The transcript's wrapped row count.
    pub fn transcript_rows(&self) -> usize {
        self.transcript.0.rows.len()
    }

    /// Transcript rows that fit the viewport.
    pub fn transcript_visible_rows(&self) -> usize {
        self.visible_rows
    }

    /// The painted transcript rows.
    pub fn transcript_list(&self) -> &MessageList {
        &self.transcript.0
    }

    /// The painted input.
    pub fn text_input(&self) -> &TextInput {
        &self.input.0
    }
}

// ── Sub-targets ────────────────────────────────────────────────────────────────

/// The transcript's focus stop and region.
const SUB_TRANSCRIPT: SubTarget = SubTarget(1);
/// The input's focus stop and region.
const SUB_INPUT: SubTarget = SubTarget(2);
/// The Send/Stop button's region.
const SUB_SEND: SubTarget = SubTarget(3);
/// The transcript scrollbar's region.
const SUB_SCROLLBAR: SubTarget = SubTarget(4);

// ── Primitive ──────────────────────────────────────────────────────────────────

impl A11yPrimitive for Chat {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new()
            .with_role(A11yRole::Group)
            .with_label("Chat")
    }
}

impl Primitive for Chat {
    type Layout = ChatLayout;
    type Event = ChatEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> ChatLayout {
        let lh = ctx.editor_line_height().max(1.0);
        let char_w = ctx.char_width;
        let status_h = lh;

        // The Send/Stop segment takes a fixed-width column at the right
        // of the input row when there is room for it beside the input.
        let send_w = send_segment_width(char_w);
        let show_send = rect.width > send_w * 2.0;
        let input_row_w = if show_send {
            rect.width - send_w
        } else {
            rect.width
        };

        // Auto-grow: the wrapped visual row count, clamped, plus the
        // input's one-unit border above and below.
        let col_budget = TextInput::content_cols(input_row_w, char_w);
        let input_rows = wrap_input_rows(&self.input_lines, col_budget);
        let (min_rows, max_rows) = self.input_rows;
        let min_rows = min_rows.max(1);
        let shown_rows = input_rows.len().clamp(min_rows, max_rows.max(min_rows));
        let input_h = shown_rows as f32 * ctx.line_height.max(1.0) + 2.0;
        let hint_h = if self.hint.is_some() { lh } else { 0.0 };
        let middle_h = (rect.height - status_h - input_h - hint_h).max(0.0);

        let status = Rect::new(rect.x, rect.y, rect.width, status_h);
        let input_y = rect.y + rect.height - input_h;
        let input = Rect::new(rect.x, input_y, input_row_w, input_h);
        let send = show_send.then(|| Rect::new(rect.x + input_row_w, input_y, send_w, input_h));
        let hint = (hint_h > 0.0).then(|| Rect::new(rect.x, input_y - hint_h, rect.width, hint_h));
        let spinner = (self.busy && rect.width > lh)
            .then(|| Rect::new(rect.x + rect.width - lh, rect.y, lh, status_h));
        let track_w = self.scrollbar_track_width(lh);
        let (transcript, scrollbar) = if !self.transcript.is_empty() && rect.width > track_w {
            (
                Rect::new(rect.x, rect.y + status_h, rect.width - track_w, middle_h),
                Some(Rect::new(
                    rect.x + rect.width - track_w,
                    rect.y + status_h,
                    track_w,
                    middle_h,
                )),
            )
        } else {
            (
                Rect::new(rect.x, rect.y + status_h, rect.width, middle_h),
                None,
            )
        };
        let zones = ChatZones {
            status,
            transcript,
            scrollbar,
            spinner,
            hint,
            input,
            send,
        };

        let status_list = MessageList {
            id: WidgetId::new(format!("{}-status", self.id)),
            rows: self.status_rows(),
            scroll_top: 0,
        };
        let status_layout = Primitive::layout(&status_list, ctx, status);

        let (rows, turn_starts) = self.transcript_rows(transcript_col_budget(char_w, transcript));
        let visible_rows = transcript_visible_rows(lh, transcript);
        let max_offset = transcript_max_offset(rows.len(), visible_rows);
        let scroll_top = if self.stuck_to_bottom {
            max_offset
        } else {
            self.transcript_scroll_top
        };
        let total_rows = rows.len();
        let transcript_list = MessageList {
            id: WidgetId::new(format!("{}-transcript", self.id)),
            rows,
            scroll_top,
        };
        let transcript_layout = Primitive::layout(&transcript_list, ctx, transcript);

        let scrollbar = scrollbar.map(|track| {
            let sb = Scrollbar::vertical(
                format!("{}-sb", self.id),
                track,
                scroll_top as f32,
                total_rows as f32,
                visible_rows as f32,
                track_w.max(1.0),
            );
            let layout = Primitive::layout(&sb, ctx, track);
            (sb, layout)
        });

        let spinner = spinner.map(|r| {
            let spinner = Spinner {
                id: WidgetId::new(format!("{}-spinner", self.id)),
                label: String::new(),
                frame_idx: self.spinner_frame,
                accent: None,
            };
            let layout = Primitive::layout(&spinner, ctx, r);
            (spinner, layout)
        });

        let hint = hint.map(|r| {
            let text: String = self
                .hint
                .as_ref()
                .map(|h| h.spans.iter().map(|s| s.text.as_str()).collect())
                .unwrap_or_default();
            let list = MessageList {
                id: WidgetId::new(format!("{}-hint", self.id)),
                rows: vec![MessageRow::new(text, Color::rgb(140, 140, 140), 0.0)],
                scroll_top: 0,
            };
            let layout = Primitive::layout(&list, ctx, r);
            (list, layout)
        });

        let text_input = self.text_input(&input_rows);
        let input_layout = Primitive::layout(&text_input, ctx, input);

        let send = send.map(|r| {
            let button = self.send_button();
            let layout = Primitive::layout(&button, ctx, r);
            (button, layout)
        });

        ChatLayout {
            area: rect,
            zones,
            line_height: lh,
            status: (status_list, status_layout),
            spinner,
            transcript: (transcript_list, transcript_layout),
            turn_starts,
            visible_rows,
            max_offset,
            scrollbar,
            hint,
            input: (text_input, input_layout),
            input_rows,
            send,
        }
    }

    fn paint(
        &self,
        layout: &ChatLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &PaintEnv,
    ) {
        let child = env.with_widget_state(None);
        let (status, status_layout) = &layout.status;
        status.paint(status_layout, surface, &child);
        if self.busy {
            if let Some((spinner, l)) = &layout.spinner {
                spinner.paint(l, surface, &child);
            }
        }
        let (transcript, transcript_layout) = &layout.transcript;
        transcript.paint(transcript_layout, surface, &child);
        if let Some((sb, l)) = &layout.scrollbar {
            sb.paint(l, surface, &child);
        }
        if let Some((hint, l)) = &layout.hint {
            hint.paint(l, surface, &child);
        }
        let (input, input_layout) = &layout.input;
        input.paint(input_layout, surface, &child);
        if let Some((button, l)) = &layout.send {
            let interaction = self.button_interaction(button, env);
            button.paint(l, surface, &child.with_interaction(&interaction));
        }
    }

    #[cfg(feature = "tui")]
    fn paint_cells(&self, layout: &ChatLayout, cells: &mut dyn crate::CellSurface, env: &PaintEnv) {
        let child = env.with_widget_state(None);
        let (status, status_layout) = &layout.status;
        status.paint_cells(status_layout, cells, &child);
        if self.busy {
            if let Some((spinner, l)) = &layout.spinner {
                spinner.paint_cells(l, cells, &child);
            }
        }
        let (transcript, transcript_layout) = &layout.transcript;
        transcript.paint_cells(transcript_layout, cells, &child);
        if let Some((sb, l)) = &layout.scrollbar {
            sb.paint_cells(l, cells, &child);
        }
        if let Some((hint, l)) = &layout.hint {
            hint.paint_cells(l, cells, &child);
        }
        let (input, input_layout) = &layout.input;
        input.paint_cells(input_layout, cells, &child);
        if let Some((button, l)) = &layout.send {
            let interaction = self.button_interaction(button, env);
            button.paint_cells(l, cells, &child.with_interaction(&interaction));
        }
    }
}

impl Chat {
    /// Hover and press on the Send/Stop region, re-addressed to the
    /// button so it tints itself.
    fn button_interaction(&self, button: &Button, env: &PaintEnv) -> InteractionState {
        let readdress = |target: Option<crate::focus::FocusAddress>| match target {
            Some((id, Some(SUB_SEND))) if id == self.id => Some((button.id, None)),
            _ => None,
        };
        InteractionState::from_targets(
            readdress(env.interaction.hovered_target()),
            readdress(env.interaction.pressed_target()),
        )
    }
}

// ── Interactive ────────────────────────────────────────────────────────────────

/// `rect` moved into the chat's local space.
fn local_rect(layout: &ChatLayout, rect: Rect) -> Rect {
    Rect::new(
        rect.x - layout.area.x,
        rect.y - layout.area.y,
        rect.width,
        rect.height,
    )
}

impl Chat {
    /// The event that returns keyboard focus to the input, when it is
    /// elsewhere.
    fn focus_input_event(&self) -> Option<ChatEvent> {
        (self.focused_turn.is_some() || !self.input_has_focus)
            .then_some(ChatEvent::FocusedTurnChanged { turn: None })
    }

    /// Report `primary`, emitting the input-focus write-back first when
    /// focus is elsewhere.
    fn with_input_focus(&self, primary: ChatEvent, cx: &mut EventCx) -> Handled<ChatEvent> {
        if let Some(focus) = self.focus_input_event() {
            cx.emit(focus.into_ui_event(self.id));
        }
        Handled::Event(primary)
    }

    fn submit(&self, state: &mut ChatState) -> Handled<ChatEvent> {
        let text = self.input_text();
        if text.is_empty() {
            return Handled::Ignored;
        }
        if !text.trim().is_empty() && state.history.last() != Some(&text) {
            state.history.push(text.clone());
        }
        state.reset_history_navigation();
        Handled::Event(ChatEvent::Submit { text })
    }

    fn history_prev(&self, state: &mut ChatState) -> Handled<ChatEvent> {
        if state.history.is_empty() {
            return Handled::Ignored;
        }
        let idx = match state.history_pos {
            None => {
                state.saved_input =
                    Some((self.input_text(), self.input_cursor.0, self.input_cursor.1));
                state.history.len() - 1
            }
            Some(pos) if pos > 0 => pos - 1,
            Some(_) => return Handled::Consumed,
        };
        state.history_pos = Some(idx);
        Handled::Event(self.set_input(&state.history[idx], None))
    }

    fn history_next(&self, state: &mut ChatState) -> Handled<ChatEvent> {
        let Some(pos) = state.history_pos else {
            return Handled::Ignored;
        };
        if pos + 1 < state.history.len() {
            state.history_pos = Some(pos + 1);
            return Handled::Event(self.set_input(&state.history[pos + 1], None));
        }
        let (text, line, col) = state.saved_input.take().unwrap_or_default();
        state.history_pos = None;
        Handled::Event(self.set_input(&text, Some((line, col))))
    }

    /// `↑`/`↓`: move the caret one visual row, or walk history from the
    /// first/last visual row of the buffer.
    fn vertical(
        &self,
        state: &mut ChatState,
        up: bool,
        extend: bool,
        layout: &ChatLayout,
    ) -> Handled<ChatEvent> {
        let rows = &layout.input_rows;
        let (row, col) = input_logical_to_visual(rows, self.input_cursor.0, self.input_cursor.1);
        if up && row == 0 {
            return self.history_prev(state);
        }
        if !up && row + 1 >= rows.len() {
            return self.history_next(state);
        }
        let target = if up { row - 1 } else { row + 1 };
        let (line, col) = input_visual_to_logical(rows, target, col);
        match self.edit_input(&EditOp::SetCursor { line, col, extend }) {
            Some(event) => Handled::Event(event),
            None => Handled::Consumed,
        }
    }

    fn page(&self, down: bool, layout: &ChatLayout) -> Handled<ChatEvent> {
        let visible = layout.visible_rows;
        let jump = (visible.max(1) - 1).max(1) as isize;
        Handled::Event(self.scroll_transcript_by(
            if down { jump } else { -jump },
            layout.transcript_rows(),
            visible,
        ))
    }

    /// Run `event` through the shared text-edit core against the input,
    /// reseeded from the descriptor's caret and selection.
    fn edit(
        &self,
        state: &mut ChatState,
        event: &RoutedEvent,
        layout: &ChatLayout,
        cx: &mut EventCx,
    ) -> Handled<ChatEvent> {
        state.edit.caret = Some(self.input_cursor);
        state.edit.anchor = self.input_anchor;
        let (ti_layout, rows) = (&layout.input.1, &layout.input_rows);
        let caret_at = |p: Point| {
            let (x, y) = (p.x + layout.area.x, p.y + layout.area.y);
            ti_layout
                .caret_at(x, y)
                .map(|(row, col)| input_visual_to_logical(rows, row, col))
        };
        let outcome = {
            let mut host = ChildHost::new(cx, Some(SUB_INPUT));
            let outcome = {
                let mut input_cx = EventCx::new(layout.input.0.id, &mut host);
                handle_lines_edit_event(
                    &self.input_lines,
                    self.input_cursor,
                    &mut state.edit,
                    event,
                    &caret_at,
                    &mut input_cx,
                )
            };
            match host.captured {
                Some(true) => state.drag = Some(ChatDrag::Input),
                Some(false) if state.drag == Some(ChatDrag::Input) => state.drag = None,
                _ => {}
            }
            outcome
        };
        let cursor = state.edit.caret.unwrap_or(self.input_cursor);
        let anchor = state.edit.anchor;
        match outcome {
            FlatEditOutcome::Changed(text) => {
                state.reset_history_navigation();
                self.with_input_focus(
                    ChatEvent::InputChanged {
                        lines: text.split('\n').map(str::to_string).collect(),
                        cursor,
                        anchor,
                    },
                    cx,
                )
            }
            FlatEditOutcome::Consumed
                if cursor != self.input_cursor || anchor != self.input_anchor =>
            {
                Handled::Event(ChatEvent::InputChanged {
                    lines: self.input_lines.clone(),
                    cursor,
                    anchor,
                })
            }
            FlatEditOutcome::Consumed => Handled::Consumed,
            FlatEditOutcome::Ignored => Handled::Ignored,
        }
    }

    fn handle_key(
        &self,
        state: &mut ChatState,
        event: &RoutedEvent,
        key: &KeyEvent,
        layout: &ChatLayout,
        cx: &mut EventCx,
    ) -> Handled<ChatEvent> {
        let m = key.modifiers;
        let turns = self.transcript.len();
        let step_focus = |forward: bool| {
            let turn = if turns == 0 {
                None
            } else if forward {
                match self.focused_turn {
                    None => Some(0),
                    Some(i) if i + 1 < turns => Some(i + 1),
                    Some(_) => None,
                }
            } else {
                match self.focused_turn {
                    None => Some(turns - 1),
                    Some(0) => None,
                    Some(i) => Some(i - 1),
                }
            };
            Handled::Event(ChatEvent::FocusedTurnChanged { turn })
        };
        match &key.key {
            Key::Named(NamedKey::Tab) if !m.ctrl && !m.alt => step_focus(!m.shift),
            Key::Named(NamedKey::BackTab) if !m.ctrl && !m.alt => step_focus(false),
            // Enter on a focused turn folds or unfolds it; Ctrl+Enter
            // still submits.
            Key::Named(NamedKey::Enter)
                if !m.ctrl && !self.input_has_focus && self.focused_turn.is_some() =>
            {
                let turn_idx = self.focused_turn.unwrap_or(0);
                Handled::Event(ChatEvent::TurnCollapsed {
                    turn_idx,
                    collapsed: !self.is_turn_collapsed(turn_idx),
                })
            }
            Key::Char('s') if m.ctrl => self.submit(state),
            Key::Named(NamedKey::Enter) if m.ctrl => self.submit(state),
            Key::Named(NamedKey::Enter) if self.submit_on_enter && (m.shift || m.alt) => {
                state.reset_history_navigation();
                match self.edit_input(&EditOp::InsertChar('\n')) {
                    Some(event) => self.with_input_focus(event, cx),
                    None => Handled::Consumed,
                }
            }
            Key::Named(NamedKey::Enter) if !self.submit_on_enter && m.alt => self.submit(state),
            Key::Named(NamedKey::Enter) if self.submit_on_enter => self.submit(state),
            Key::Named(NamedKey::Escape) => Handled::Event(ChatEvent::Cancelled),
            Key::Named(NamedKey::Up) => self.vertical(state, true, m.shift, layout),
            Key::Named(NamedKey::Down) => self.vertical(state, false, m.shift, layout),
            Key::Named(NamedKey::PageUp) => self.page(false, layout),
            Key::Named(NamedKey::PageDown) => self.page(true, layout),
            other => {
                // Copy and cut with nothing selected stay the app's keys.
                let clipboard = matches!(
                    EditIntent::from_key(other, m),
                    Some(EditIntent::Copy | EditIntent::Cut)
                );
                let selected = self
                    .input_anchor
                    .is_some_and(|anchor| anchor != self.input_cursor);
                let handled = if clipboard && !selected {
                    Handled::Ignored
                } else {
                    self.edit(state, event, layout, cx)
                };
                match handled {
                    Handled::Ignored => Handled::Event(ChatEvent::KeyPressed {
                        key: format!("{other:?}"),
                        modifiers: m,
                    }),
                    handled => handled,
                }
            }
        }
    }

    /// Hand a pointer event to the Send/Stop button.
    fn press_send(
        &self,
        state: &mut ChatState,
        event: &RoutedEvent,
        layout: &ChatLayout,
        cx: &mut EventCx,
    ) -> Handled<ChatEvent> {
        let Some((button, button_layout)) = &layout.send else {
            return Handled::Ignored;
        };
        let rect = local_rect(layout, button_layout.bounds);
        let routed = widget_host::child_event(event, Point::new(rect.x, rect.y), None);
        let mut host = ChildHost::new(cx, Some(SUB_SEND));
        let mut button_cx = EventCx::new(button.id, &mut host);
        match Interactive::handle(button, &mut (), &routed, button_layout, &mut button_cx) {
            Handled::Event(ButtonEvent::Activated { .. }) if self.busy => {
                Handled::Event(ChatEvent::StopRequested)
            }
            Handled::Event(ButtonEvent::Activated { .. }) => self.submit(state),
            _ => Handled::Ignored,
        }
    }

    /// Hand a pointer event to the transcript scrollbar.
    fn scrollbar_event(
        &self,
        state: &mut ChatState,
        event: &RoutedEvent,
        layout: &ChatLayout,
        cx: &mut EventCx,
    ) -> Handled<ChatEvent> {
        let Some((sb, sb_layout)) = &layout.scrollbar else {
            return Handled::Ignored;
        };
        if layout.max_offset == 0 {
            return Handled::Ignored;
        }
        let rect = local_rect(layout, sb_layout.track);
        let routed = widget_host::child_event(event, Point::new(rect.x, rect.y), None);
        let mut host = ChildHost::new(cx, Some(SUB_SCROLLBAR));
        let handled = {
            let mut sb_cx = EventCx::new(sb.id, &mut host);
            Interactive::handle(sb, &mut state.scrollbar, &routed, sb_layout, &mut sb_cx)
        };
        match host.captured {
            Some(true) => state.drag = Some(ChatDrag::Scrollbar),
            Some(false) => state.drag = None,
            None => {}
        }
        match handled {
            Handled::Event(ScrollbarEvent::Moved { fraction }) => {
                let max = layout.max_offset;
                let scroll_top = ((fraction * max as f32).round().max(0.0) as usize).min(max);
                if scroll_top == layout.transcript_scroll_top() {
                    return Handled::Consumed;
                }
                Handled::Event(ChatEvent::TranscriptScrolled {
                    scroll_top,
                    stuck_to_bottom: transcript_reached_bottom(scroll_top, max),
                })
            }
            Handled::Consumed => Handled::Consumed,
            _ => Handled::Ignored,
        }
    }

    /// A press on the transcript: the turn and in-turn row it landed on.
    fn press_transcript(
        &self,
        abs: Point,
        layout: &ChatLayout,
        cx: &mut EventCx,
    ) -> Handled<ChatEvent> {
        let (list, _) = &layout.transcript;
        let measure = MessageListMeasure::new(layout.line_height);
        match list.hit_test(layout.zones.transcript, measure, abs.x, abs.y) {
            MessageListHit::Row(row_idx) => {
                let turn_idx = match layout.turn_starts.binary_search(&row_idx) {
                    Ok(i) => i,
                    Err(i) => i.saturating_sub(1),
                };
                let row_in_turn = row_idx - layout.turn_starts[turn_idx];
                if self.focused_turn != Some(turn_idx) || self.input_has_focus {
                    cx.emit(
                        ChatEvent::FocusedTurnChanged {
                            turn: Some(turn_idx),
                        }
                        .into_ui_event(self.id),
                    );
                }
                Handled::Event(ChatEvent::TurnClicked {
                    turn_idx,
                    row_in_turn,
                })
            }
            MessageListHit::Empty => Handled::Consumed,
        }
    }

    fn wheel(
        &self,
        event: &RoutedEvent,
        layout: &ChatLayout,
        cx: &mut EventCx,
    ) -> Handled<ChatEvent> {
        let (list, list_layout) = &layout.transcript;
        let rect = local_rect(layout, layout.zones.transcript);
        let routed = widget_host::child_event(event, Point::new(rect.x, rect.y), None);
        let mut host = ChildHost::new(cx, Some(SUB_TRANSCRIPT));
        let mut list_cx = EventCx::new(list.id, &mut host);
        match Interactive::handle(list, &mut (), &routed, list_layout, &mut list_cx) {
            Handled::Event(MessageListEvent::Scrolled {
                scroll_top,
                at_bottom,
            }) => Handled::Event(ChatEvent::TranscriptScrolled {
                scroll_top,
                stuck_to_bottom: at_bottom,
            }),
            Handled::Ignored => Handled::Ignored,
            _ => Handled::Consumed,
        }
    }
}

impl Interactive for Chat {
    type State = ChatState;
    type Action = ChatEvent;

    /// The transcript (while it has turns), then the input.
    fn focus(&self) -> FocusSpec {
        let mut stops = Vec::new();
        if !self.transcript.is_empty() {
            stops.push(FocusStop {
                sub: SUB_TRANSCRIPT,
            });
        }
        stops.push(FocusStop { sub: SUB_INPUT });
        FocusSpec::Children(stops)
    }

    /// The Send/Stop button, the scrollbar, the transcript and the input,
    /// in the chat's local space.
    fn sub_regions(&self, layout: &ChatLayout) -> Vec<(Rect, SubTarget)> {
        let z = &layout.zones;
        let mut out = Vec::new();
        if let Some(send) = z.send {
            out.push((local_rect(layout, send), SUB_SEND));
        }
        if let Some(sb) = z.scrollbar {
            out.push((local_rect(layout, sb), SUB_SCROLLBAR));
        }
        out.push((local_rect(layout, z.transcript), SUB_TRANSCRIPT));
        out.push((local_rect(layout, z.input), SUB_INPUT));
        out
    }

    fn handle(
        &self,
        state: &mut ChatState,
        event: &RoutedEvent,
        layout: &ChatLayout,
        cx: &mut EventCx,
    ) -> Handled<ChatEvent> {
        match event {
            RoutedEvent::Key(key) => self.handle_key(state, event, key, layout, cx),
            RoutedEvent::Text(_) => self.edit(state, event, layout, cx),
            RoutedEvent::Wheel { .. } => self.wheel(event, layout, cx),
            RoutedEvent::Pointer { kind, local, .. } => {
                match (state.drag, kind) {
                    (Some(ChatDrag::Scrollbar), PointerKind::Move | PointerKind::Up(_)) => {
                        return self.scrollbar_event(state, event, layout, cx);
                    }
                    (Some(ChatDrag::Input), PointerKind::Move | PointerKind::Up(_)) => {
                        return self.edit(state, event, layout, cx);
                    }
                    _ => {}
                }
                if *kind != PointerKind::Down(MouseButton::Left) {
                    return Handled::Ignored;
                }
                let abs = Point::new(local.x + layout.area.x, local.y + layout.area.y);
                let z = &layout.zones;
                if z.send.is_some_and(|r| r.contains(abs)) {
                    self.press_send(state, event, layout, cx)
                } else if z.scrollbar.is_some_and(|r| r.contains(abs)) {
                    self.scrollbar_event(state, event, layout, cx)
                } else if z.transcript.contains(abs) {
                    self.press_transcript(abs, layout, cx)
                } else if z.input.contains(abs) {
                    if let Some(focus) = self.focus_input_event() {
                        cx.emit(focus.into_ui_event(self.id));
                    }
                    match self.edit(state, event, layout, cx) {
                        Handled::Ignored => Handled::Consumed,
                        handled => handled,
                    }
                } else {
                    Handled::Ignored
                }
            }
            _ => Handled::Ignored,
        }
    }
}

// ── Module-level helpers ───────────────────────────────────────────────────────

/// Last valid transcript scroll offset for `total_rows` wrapped rows in a
/// `visible_rows`-tall viewport: the "last page" position following the
/// tail paints, and the threshold every scroll path re-engages following
/// at.
fn transcript_max_offset(total_rows: usize, visible_rows: usize) -> usize {
    total_rows.saturating_sub(visible_rows)
}

/// Whether a transcript scrolled to `scroll_top` (out of `max_offset`,
/// from [`transcript_max_offset`]) has reached or passed the last page.
fn transcript_reached_bottom(scroll_top: usize, max_offset: usize) -> bool {
    scroll_top >= max_offset
}

/// Word-aware soft-wrap of `text` to `col_budget` characters per row.
///
/// Uses [`crate::text_util::word_wrap`] to break at whitespace where possible.
/// Only a single word wider than `col_budget` hard-breaks mid-word (there is
/// no other way to fit it); everything else wraps at word boundaries.
fn wrap_text(text: &str, col_budget: usize) -> Vec<String> {
    crate::text_util::word_wrap(text, col_budget)
}

/// Split a flat span list (where `\n`-only spans act as line delimiters)
/// into per-logical-line span lists.
///
/// This is the inverse of the encoding used by
/// [`Chat::push_turn_markdown`], which concatenates per-line
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

// ── Input soft-wrap ─────────────────────────────────────────────────────

/// One soft-wrapped visual row of the chat input buffer.
///
/// Wrapping here is deliberately **exact** — a row's `text` is always a
/// contiguous char range of its owning logical line, with no whitespace
/// collapsed or dropped (unlike [`crate::text_util::word_wrap`]'s
/// transcript wrapping, which *is* lossy at wrap points). Concatenating
/// every row for a given `logical_line`, in order, reconstructs that
/// line's text char-for-char. That's what lets
/// [`input_logical_to_visual`] / [`input_visual_to_logical`] round-trip
/// cursor positions losslessly between logical ([`Chat::input_cursor`]'s
/// `(line, char column)`) and visual (row, char column) space.
#[derive(Debug, Clone)]
struct InputRow {
    /// Index into [`Chat::input_lines`].
    logical_line: usize,
    /// Char-column offset into `logical_line` where this row starts.
    col_offset: usize,
    /// This row's text — a char-exact slice of `logical_line`.
    text: String,
}

/// Soft-wrap `lines` (the input editor's logical lines) to `col_budget`
/// char columns per visual row. See [`InputRow`] for the exactness
/// guarantee that makes cursor-position round-tripping possible.
///
/// Always returns at least one row per logical line (an empty logical
/// line yields one empty row), so `rows.len() >= 1` for any input,
/// including a single empty line.
fn wrap_input_rows(lines: &[String], col_budget: usize) -> Vec<InputRow> {
    let mut rows = Vec::new();
    for (logical_line, line) in lines.iter().enumerate() {
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
    let total_width: usize = chars
        .iter()
        .map(|&c| crate::text_util::char_cell_width(c) as usize)
        .sum();
    if col_budget == 0 || total_width <= col_budget {
        return vec![(0, line.to_string())];
    }
    let mut rows = Vec::new();
    let mut start = 0usize;
    while start < chars.len() {
        // Grow `end` by display width, not char count, so a row of CJK
        // or emoji never exceeds `col_budget` cells — each of those
        // chars claims two columns, not one.
        let mut width = 0usize;
        let mut end = start;
        while end < chars.len() {
            let w = crate::text_util::char_cell_width(chars[end]) as usize;
            if width + w > col_budget {
                break;
            }
            width += w;
            end += 1;
        }
        if end == start {
            // Not even one (possibly double-width) char fits the
            // budget — take it anyway so the loop always makes forward
            // progress, same as every other row-budget clamp in this
            // file.
            end = start + 1;
        }
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

/// Map a logical `(line, char_col)` cursor position — [`Chat::input_cursor`]'s
/// own coordinate space — to `(visual_row_idx, visual_col)` within `rows`
/// — the inverse of [`input_visual_to_logical`].
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
/// logical `(line, char_col)` position in [`Chat::input_cursor`]'s coordinate
/// space — the inverse of [`input_logical_to_visual`]. Both indices are
/// clamped, so this never panics on an out-of-range row or column;
/// `rows` empty returns `(0, 0)`.
fn input_visual_to_logical(rows: &[InputRow], row_idx: usize, col: usize) -> (usize, usize) {
    let Some(last) = rows.len().checked_sub(1) else {
        return (0, 0);
    };
    let r = &rows[row_idx.min(last)];
    let len = r.text.chars().count();
    (r.logical_line, r.col_offset + col.min(len))
}

// ── Tests ──────────────────────────────────────────────────────────────────────

/// A host that keeps a [`Chat`] and its [`ChatState`], drives the chat
/// through its own [`Interactive`] with raw [`UiEvent`]s, and applies
/// every write-back the chat reports, the way an app does. A write-back
/// answers [`Handled::Consumed`]; the semantic events are returned.
#[cfg(test)]
use crate::Backend;

#[cfg(test)]
struct TestHost {
    chat: Chat,
    state: ChatState,
    /// The transcript row last painted.
    painted_scroll_top: std::cell::Cell<usize>,
}

#[cfg(test)]
impl std::ops::Deref for TestHost {
    type Target = Chat;

    fn deref(&self) -> &Chat {
        &self.chat
    }
}

#[cfg(test)]
impl std::ops::DerefMut for TestHost {
    fn deref_mut(&mut self) -> &mut Chat {
        &mut self.chat
    }
}

#[cfg(test)]
impl TestHost {
    fn new(id: &str) -> Self {
        Self {
            chat: Chat::new(WidgetId::new(id)),
            state: ChatState::default(),
            painted_scroll_top: std::cell::Cell::new(0),
        }
    }

    /// Store `event`'s write-back; a semantic event is returned instead.
    fn settle(&mut self, event: ChatEvent) -> Handled<ChatEvent> {
        self.chat.apply(&event);
        match event {
            ChatEvent::InputChanged { .. }
            | ChatEvent::TranscriptScrolled { .. }
            | ChatEvent::FocusedTurnChanged { .. }
            | ChatEvent::TurnCollapsed { .. } => Handled::Consumed,
            semantic => Handled::Event(semantic),
        }
    }

    fn handle(&mut self, event: &UiEvent, backend: &dyn Backend, rect: Rect) -> Handled<ChatEvent> {
        let Some(driven) =
            crate::compose::drive_ui_event(&self.chat, &mut self.state, event, backend, rect)
        else {
            return Handled::Ignored;
        };
        for emitted in driven.emitted {
            if let UiEvent::Chat(id, event) = emitted {
                if id == self.chat.id {
                    self.chat.apply(&event);
                }
            }
        }
        match driven.handled {
            Handled::Event(event) => self.settle(event),
            other => other,
        }
    }

    fn render(&self, backend: &mut dyn Backend, rect: Rect) {
        use crate::backend::DrawsPrimitives;
        let layout = backend.draw_dyn(rect, &self.chat, &());
        if let Some(layout) = layout.downcast_ref::<ChatLayout>() {
            self.painted_scroll_top.set(layout.transcript_scroll_top());
        }
    }

    fn compute_layout(&self, backend: &dyn Backend, rect: Rect) -> ChatZones {
        use crate::backend::DrawsPrimitives;
        backend.layout_primitive(rect, &self.chat).zones
    }

    fn build_transcript_rows(&self, col_budget: usize) -> Vec<MessageRow> {
        self.chat.transcript_rows(col_budget).0
    }

    fn apply_input_op(&mut self, op: EditOp) {
        if let Some(event) = self.chat.edit_input(&op) {
            self.chat.apply(&event);
        }
    }

    fn input_insert_char(&mut self, ch: char) {
        self.apply_input_op(EditOp::InsertChar(ch));
    }

    fn input_insert_str(&mut self, s: &str) {
        self.apply_input_op(EditOp::InsertText(s.to_string()));
    }

    /// The app's clear after a submit: the descriptor's input, plus the
    /// widget state's history cursor.
    fn clear_input(&mut self) {
        self.chat.clear_input();
        self.state.reset_history_navigation();
    }

    fn history_prev(&mut self) -> Handled<ChatEvent> {
        match self.chat.history_prev(&mut self.state) {
            Handled::Event(event) => self.settle(event),
            other => other,
        }
    }

    fn history_next(&mut self) -> Handled<ChatEvent> {
        match self.chat.history_next(&mut self.state) {
            Handled::Event(event) => self.settle(event),
            other => other,
        }
    }

    fn selection_range(&self) -> Option<(Caret, Caret)> {
        crate::behaviour::text_edit::selection_range(self.chat.input_cursor, self.chat.input_anchor)
    }

    fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection_range()?;
        Some(crate::behaviour::text_edit::text_in_range(
            &self.chat.input_lines,
            start,
            end,
        ))
    }

    /// The first transcript row shown: the row last painted while the
    /// transcript follows its tail.
    fn transcript_scroll_top(&self) -> usize {
        if self.chat.stuck_to_bottom {
            self.painted_scroll_top.get()
        } else {
            self.chat.transcript_scroll_top
        }
    }

    fn set_transcript_scroll_top(&mut self, top: usize) {
        self.chat.transcript_scroll_top = top;
        self.chat.stuck_to_bottom = false;
        self.painted_scroll_top.set(top);
    }

    /// Scroll from the row shown, the way the chat's own scroll gestures
    /// do.
    fn scroll_transcript_by(&mut self, delta: isize, total_rows: usize, visible_rows: usize) {
        let mut probe = self.chat.clone();
        probe.stuck_to_bottom = false;
        probe.transcript_scroll_top = self.transcript_scroll_top();
        let event = probe.scroll_transcript_by(delta, total_rows, visible_rows);
        if let ChatEvent::TranscriptScrolled { scroll_top, .. } = event {
            self.painted_scroll_top.set(scroll_top);
        }
        self.chat.apply(&event);
    }

    fn set_focused_turn(&mut self, turn: Option<usize>) {
        self.chat.focused_turn = turn;
        if turn.is_some() {
            self.chat.input_has_focus = false;
        }
    }

    fn focused_turn(&self) -> Option<usize> {
        self.chat.focused_turn
    }

    fn input_has_focus(&self) -> bool {
        self.chat.input_has_focus
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::RecordingBackend;

    // Tests use `RecordingBackend` for its `draw_text_input`/`text_input_layout`
    // and `draw_spinner`/`spinner_layout` implementations, which return real layouts
    // rather than panicking, as required by click-routing and render tests.

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
        let cc = TestHost::new("chat");
        assert_eq!(cc.input_text(), "");
        assert_eq!(cc.transcript_scroll_top(), 0);
        assert!(cc.input_has_focus());
    }

    // ── Input insertion ───────────────────────────────────────────────

    #[test]
    fn input_insert_char_appends() {
        let mut cc = TestHost::new("c");
        cc.input_insert_char('h');
        cc.input_insert_char('i');
        assert_eq!(cc.input_text(), "hi");
    }

    #[test]
    fn input_insert_str_paste() {
        let mut cc = TestHost::new("c");
        cc.input_insert_str("hello\nworld");
        assert_eq!(cc.input_text(), "hello\nworld");
    }

    #[test]
    fn clear_input_resets_state() {
        let mut cc = TestHost::new("c");
        cc.input_insert_str("hello\nworld");
        cc.clear_input();
        assert_eq!(cc.input_text(), "");
        assert_eq!(cc.chat.input_cursor.0, 0);
        assert_eq!(cc.chat.input_cursor.1, 0);
    }

    // ── Keyboard: submit ──────────────────────────────────────────────

    #[test]
    fn ctrl_enter_submits_and_emits_event() {
        let mut cc = TestHost::new("c");
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
            Handled::Event(ChatEvent::Submit {
                text: "hello".into()
            })
        );
    }

    #[test]
    fn alt_enter_submits_and_emits_event() {
        let mut cc = TestHost::new("c");
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
            Handled::Event(ChatEvent::Submit {
                text: "hello".into()
            })
        );
    }

    #[test]
    fn ctrl_enter_on_empty_input_ignored() {
        let mut cc = TestHost::new("c");
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
        assert_eq!(ev, Handled::Ignored);
    }

    #[test]
    fn ctrl_s_submits_and_emits_event() {
        let mut cc = TestHost::new("c");
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
            Handled::Event(ChatEvent::Submit {
                text: "hello".into()
            })
        );
    }

    #[test]
    fn ctrl_s_on_empty_input_ignored() {
        let mut cc = TestHost::new("c");
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
        assert_eq!(ev, Handled::Ignored);
    }

    #[test]
    fn submit_adds_to_history() {
        let mut cc = TestHost::new("c");
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
        let _ = cc.handle(&event, &RecordingBackend::new(), rect);
        assert_eq!(cc.state.history.len(), 1);
        assert_eq!(cc.state.history[0], "hello");
    }

    // ── Keyboard: chorded editing keys ────────────────────────────────

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
    fn ctrl_a_selects_the_whole_input() {
        let mut cc = TestHost::new("c");
        cc.input_insert_str("hello\nworld");
        let ev = cc.handle(&ctrl_key('a'), &RecordingBackend::new(), make_rect());
        assert_eq!(ev, Handled::Consumed);
        assert_eq!(cc.selected_text().as_deref(), Some("hello\nworld"));
    }

    #[test]
    fn ctrl_e_passes_through_as_key_pressed() {
        let mut cc = TestHost::new("c");
        cc.input_insert_str("hello");
        let ev = cc.handle(&ctrl_key('e'), &RecordingBackend::new(), make_rect());
        match ev {
            Handled::Event(ChatEvent::KeyPressed { key, modifiers }) => {
                assert_eq!(key, "Char('e')");
                assert!(modifiers.ctrl);
            }
            other => panic!("expected KeyPressed fallthrough, got {other:?}"),
        }
        assert_eq!(cc.chat.input_cursor, (0, 5));
    }

    #[test]
    fn ctrl_backspace_stops_at_punctuation_like_every_text_widget() {
        let mut cc = TestHost::new("c");
        cc.input_insert_str("path.to_file");
        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Backspace),
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
        };
        let _ = cc.handle(&event, &RecordingBackend::new(), make_rect());
        assert_eq!(cc.input_text(), "path.");
    }

    #[test]
    fn ctrl_c_with_a_selection_copies_it() {
        let mut cc = TestHost::new("c");
        cc.input_insert_str("hello");
        let _ = cc.handle(&ctrl_key('a'), &RecordingBackend::new(), make_rect());
        let probe = cc.chat.clone();
        let layout = {
            use crate::backend::DrawsPrimitives;
            RecordingBackend::new().layout_primitive(make_rect(), &probe)
        };
        let event = RoutedEvent::Key(KeyEvent {
            key: Key::Char('c'),
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
        });
        let driven = widget_host::drive(
            &probe,
            probe.id,
            &mut cc.state,
            &event,
            &layout,
            crate::primitive::Units::Cells,
        );
        assert_eq!(driven.handled, Handled::Consumed);
        assert_eq!(
            driven.emitted,
            vec![UiEvent::TextCopied("hello".to_string())]
        );
    }

    // macOS delivers a Cmd-modified letter as a plain `Key::Char` with
    // `modifiers.cmd` set, the same shape Ctrl uses on every other
    // backend. It must fall through to `KeyPressed` for the app's own
    // hotkey dispatch rather than being typed into the input buffer.
    #[test]
    fn cmd_char_is_not_inserted_and_passes_through_as_key_pressed() {
        let mut cc = TestHost::new("c");
        let event = UiEvent::KeyPressed {
            key: Key::Char('x'),
            modifiers: Modifiers {
                cmd: true,
                ..Default::default()
            },
            repeat: false,
        };
        let ev = cc.handle(&event, &RecordingBackend::new(), make_rect());
        assert_eq!(cc.input_text(), "");
        match ev {
            Handled::Event(ChatEvent::KeyPressed { key, modifiers }) => {
                assert_eq!(key, "Char('x')");
                assert!(modifiers.cmd);
            }
            other => panic!("expected KeyPressed fallthrough, got {other:?}"),
        }
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
        let mut cc = TestHost::new("c");
        let rect = make_rect();
        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let ev = cc.handle(&event, &RecordingBackend::new(), rect);
        assert_eq!(ev, Handled::Event(ChatEvent::Cancelled));
    }

    // ── Keyboard: enter inserts newline ───────────────────────────────

    #[test]
    fn plain_enter_inserts_newline() {
        let mut cc = TestHost::new("c");
        cc.input_insert_str("hello");
        let rect = make_rect();
        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let ev = cc.handle(&event, &RecordingBackend::new(), rect);
        assert_eq!(ev, Handled::Consumed);
        assert_eq!(cc.input_text(), "hello\n");
    }

    // ── Keyboard: submit_on_enter mode ─────────────────────────────────

    #[test]
    fn submit_on_enter_defaults_to_false() {
        let cc = TestHost::new("c");
        assert!(!cc.submit_on_enter);
    }

    #[test]
    fn submit_on_enter_plain_enter_submits() {
        let mut cc = TestHost::new("c");
        cc.submit_on_enter = true;
        cc.input_insert_str("hello");
        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let ev = cc.handle(&event, &RecordingBackend::new(), make_rect());
        assert_eq!(
            ev,
            Handled::Event(ChatEvent::Submit {
                text: "hello".into()
            })
        );
    }

    #[test]
    fn submit_on_enter_shift_enter_inserts_newline() {
        let mut cc = TestHost::new("c");
        cc.submit_on_enter = true;
        cc.input_insert_str("hello");
        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers {
                shift: true,
                ..Default::default()
            },
            repeat: false,
        };
        let ev = cc.handle(&event, &RecordingBackend::new(), make_rect());
        assert_eq!(ev, Handled::Consumed);
        assert_eq!(cc.input_text(), "hello\n");
    }

    #[test]
    fn submit_on_enter_alt_enter_inserts_newline_fallback() {
        // Alt+Enter is the fallback for terminals that can't report
        // Shift+Enter — it must insert a newline in submit_on_enter mode,
        // the opposite of its meaning in the default mode.
        let mut cc = TestHost::new("c");
        cc.submit_on_enter = true;
        cc.input_insert_str("hello");
        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers {
                alt: true,
                ..Default::default()
            },
            repeat: false,
        };
        let ev = cc.handle(&event, &RecordingBackend::new(), make_rect());
        assert_eq!(ev, Handled::Consumed);
        assert_eq!(cc.input_text(), "hello\n");
    }

    #[test]
    fn submit_on_enter_ctrl_enter_still_submits() {
        let mut cc = TestHost::new("c");
        cc.submit_on_enter = true;
        cc.input_insert_str("hello");
        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
        };
        let ev = cc.handle(&event, &RecordingBackend::new(), make_rect());
        assert_eq!(
            ev,
            Handled::Event(ChatEvent::Submit {
                text: "hello".into()
            })
        );
    }

    #[test]
    fn submit_on_enter_plain_enter_on_empty_input_ignored() {
        let mut cc = TestHost::new("c");
        cc.submit_on_enter = true;
        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let ev = cc.handle(&event, &RecordingBackend::new(), make_rect());
        assert_eq!(ev, Handled::Ignored);
    }

    // ── History navigation ────────────────────────────────────────────

    #[test]
    fn up_on_empty_history_returns_ignored() {
        let mut cc = TestHost::new("c");
        let ev = cc.history_prev();
        assert_eq!(ev, Handled::Ignored);
    }

    #[test]
    fn up_enters_history_with_newest_entry() {
        let mut cc = TestHost::new("c");
        cc.state.history.push("first".into());
        cc.state.history.push("second".into());
        let ev = cc.history_prev();
        assert_eq!(ev, Handled::Consumed);
        assert_eq!(cc.input_text(), "second");
        assert_eq!(cc.state.history_pos, Some(1));
    }

    #[test]
    fn up_twice_goes_to_older_entry() {
        let mut cc = TestHost::new("c");
        cc.state.history.push("first".into());
        cc.state.history.push("second".into());
        let _ = cc.history_prev(); // → "second"
        let ev = cc.history_prev();
        assert_eq!(ev, Handled::Consumed);
        assert_eq!(cc.input_text(), "first");
        assert_eq!(cc.state.history_pos, Some(0));
    }

    #[test]
    fn up_at_oldest_entry_stays() {
        let mut cc = TestHost::new("c");
        cc.state.history.push("only".into());
        let _ = cc.history_prev(); // at idx 0
        let ev = cc.history_prev(); // already at oldest
        assert_eq!(ev, Handled::Consumed);
        assert_eq!(cc.input_text(), "only"); // unchanged
    }

    #[test]
    fn down_after_history_nav_restores_saved_input() {
        let mut cc = TestHost::new("c");
        cc.input_insert_str("draft");
        cc.state.history.push("first".into());
        cc.state.history.push("second".into());
        let _ = cc.history_prev(); // → "second"
        let _ = cc.history_prev(); // → "first"
        let _ = cc.history_next(); // → "second"
        let ev = cc.history_next(); // → restore "draft"
        assert_eq!(ev, Handled::Consumed);
        assert_eq!(cc.input_text(), "draft");
        assert_eq!(cc.state.history_pos, None);
    }

    #[test]
    fn down_when_not_navigating_returns_ignored() {
        let mut cc = TestHost::new("c");
        let ev = cc.history_next();
        assert_eq!(ev, Handled::Ignored);
    }

    // ── History navigation via Up key (cursor on line 0) ─────────────

    #[test]
    fn up_key_on_line_0_enters_history() {
        let mut cc = TestHost::new("c");
        cc.state.history.push("prev".into());
        let rect = make_rect();
        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Up),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let ev = cc.handle(&event, &RecordingBackend::new(), rect);
        assert_eq!(ev, Handled::Consumed);
        assert_eq!(cc.input_text(), "prev");
    }

    #[test]
    fn up_key_on_line_1_moves_cursor_up() {
        let mut cc = TestHost::new("c");
        cc.input_insert_str("line0\nline1");
        // Cursor is on line 1 after insert.
        assert_eq!(cc.chat.input_cursor.0, 1);
        let rect = make_rect();
        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Up),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let ev = cc.handle(&event, &RecordingBackend::new(), rect);
        assert_eq!(ev, Handled::Consumed);
        assert_eq!(cc.chat.input_cursor.0, 0);
        // History should NOT have been entered.
        assert_eq!(cc.state.history_pos, None);
    }

    // ── Transcript scroll ─────────────────────────────────────────────

    #[test]
    fn transcript_max_offset_is_total_minus_visible() {
        assert_eq!(transcript_max_offset(10, 4), 6);
        assert_eq!(
            transcript_max_offset(4, 10),
            0,
            "fits entirely: no overflow"
        );
        assert_eq!(transcript_max_offset(0, 0), 0);
    }

    #[test]
    fn transcript_reached_bottom_is_at_or_past_max_offset() {
        assert!(!transcript_reached_bottom(4, 6));
        assert!(transcript_reached_bottom(6, 6));
        assert!(transcript_reached_bottom(7, 6));
    }

    #[test]
    fn scroll_transcript_clamps_to_bounds() {
        let mut cc = TestHost::new("c");
        cc.scroll_transcript_by(100, 10, 5); // total=10, visible=5, max=5
        assert_eq!(cc.transcript_scroll_top(), 5);
        cc.scroll_transcript_by(-100, 10, 5);
        assert_eq!(cc.transcript_scroll_top(), 0);
    }

    #[test]
    fn page_up_scrolls_transcript() {
        let mut cc = TestHost::new("c");
        cc.set_transcript_scroll_top(10);
        cc.scroll_transcript_by(-5, 20, 5);
        assert_eq!(cc.transcript_scroll_top(), 5);
    }

    /// A two-notch wheel gesture scrolls twice as far as a one-notch one:
    /// the handler consumes the delta's magnitude, not only its sign.
    #[test]
    fn wheel_scroll_consumes_the_delta_magnitude_not_just_its_sign() {
        let mut cc = TestHost::new("c");
        cc.transcript = (0..60)
            .map(|i| make_turn(ChatRole::User, &format!("line {i}")))
            .collect();
        cc.set_transcript_scroll_top(40);

        let ev = UiEvent::Scroll {
            widget: None,
            delta: crate::ScrollDelta::new(0.0, 2.0),
            position: crate::Point::new(5.0, 5.0),
        };
        assert_eq!(
            cc.handle(&ev, &RecordingBackend::new(), make_rect()),
            Handled::Consumed
        );
        assert_eq!(
            cc.transcript_scroll_top(),
            40 - 2 * 3,
            "two notches move two notches' worth of rows"
        );
    }

    /// A fractional trackpad delta still moves the transcript: rounding the
    /// magnitude must never swallow a slow gesture entirely.
    #[test]
    fn wheel_scroll_still_moves_for_a_sub_notch_gesture() {
        let mut cc = TestHost::new("c");
        cc.transcript = (0..60)
            .map(|i| make_turn(ChatRole::User, &format!("line {i}")))
            .collect();
        cc.set_transcript_scroll_top(40);

        let ev = UiEvent::Scroll {
            widget: None,
            delta: crate::ScrollDelta::new(0.0, 0.1),
            position: crate::Point::new(5.0, 5.0),
        };
        let _ = cc.handle(&ev, &RecordingBackend::new(), make_rect());
        assert!(
            cc.transcript_scroll_top() < 40,
            "a sub-notch gesture still scrolls at least one row"
        );
    }

    // ── Follow-tail (stick-to-bottom) ─────────────────────────────────

    /// A brand-new chat must start stuck to the bottom so the first
    /// streamed reply is visible without the user having to scroll.
    #[test]
    fn new_chat_is_stuck_to_bottom() {
        let cc = TestHost::new("c");
        assert!(
            cc.chat.stuck_to_bottom,
            "new chat must start stuck_to_bottom=true"
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
        let mut cc = TestHost::new("c");
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
        let mut cc = TestHost::new("c");
        // Scroll up from 0 → disengages follow-tail (max = 20−5 = 15; 0 < 15).
        cc.scroll_transcript_by(-5, 20, 5); // clamped to 0, stuck=false
                                            // Scroll to a mid position.
        cc.scroll_transcript_by(3, 20, 5); // scroll_top=3, stuck=false
        assert!(
            !cc.chat.stuck_to_bottom,
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
        let mut cc = TestHost::new("c");
        // Disengage by scrolling up.
        cc.scroll_transcript_by(-5, 20, 5); // clamped to 0, max=15, stuck=false
        cc.scroll_transcript_by(3, 20, 5); // scroll_top=3, stuck=false
        assert!(!cc.chat.stuck_to_bottom);
        // Scroll to the bottom (max = 20−5 = 15).
        cc.scroll_transcript_by(100, 20, 5); // clamped to 15, stuck=(15≥15)=true
        assert!(
            cc.chat.stuck_to_bottom,
            "scrolling back to the last page must re-engage follow-tail"
        );
    }

    // ── push_turn / push_turn_markdown ───────────────────────────────

    #[test]
    fn push_turn_appends_turn_to_transcript() {
        let mut cc = TestHost::new("c");
        cc.push_turn(ChatRole::User, StyledText::plain("hello"));
        assert_eq!(cc.chat.transcript.len(), 1);
        assert_eq!(cc.chat.transcript[0].role, ChatRole::User);
        let text: String = cc.chat.transcript[0]
            .text
            .spans
            .iter()
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(text, "hello");
    }

    #[test]
    fn push_turn_multiple_turns_accumulate() {
        let mut cc = TestHost::new("c");
        cc.push_turn(ChatRole::User, StyledText::plain("first"));
        cc.push_turn(ChatRole::Assistant, StyledText::plain("second"));
        assert_eq!(cc.chat.transcript.len(), 2);
        assert_eq!(cc.chat.transcript[0].role, ChatRole::User);
        assert_eq!(cc.chat.transcript[1].role, ChatRole::Assistant);
    }

    #[test]
    fn push_turn_markdown_joins_lines_with_newline() {
        let mut cc = TestHost::new("c");
        let theme = crate::Theme::default();
        cc.push_turn_markdown(ChatRole::Assistant, "line one\nline two", &theme);
        assert_eq!(cc.chat.transcript.len(), 1);
        assert_eq!(cc.chat.transcript[0].role, ChatRole::Assistant);
        // Span texts joined must contain both lines with a '\n' separator.
        let text: String = cc.chat.transcript[0]
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
        // The chat renders transcript rows by concatenating span
        // text — structural glyphs (bullets, blockquote bars) must survive
        // so they appear as text cues in the rendered transcript.
        let mut cc = TestHost::new("c");
        let theme = crate::Theme::default();
        cc.push_turn_markdown(ChatRole::Assistant, "- item one\n- item two", &theme);
        let text: String = cc.chat.transcript[0]
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
        let mut cc = TestHost::new("c");
        let theme = crate::Theme::default();
        // Empty markdown must not panic and must append exactly one turn.
        cc.push_turn_markdown(ChatRole::System, "", &theme);
        assert_eq!(cc.chat.transcript.len(), 1);
    }

    // ── Transcript row building ───────────────────────────────────────

    #[test]
    fn build_transcript_rows_includes_role_header() {
        let mut cc = TestHost::new("c");
        cc.transcript = vec![make_turn(ChatRole::User, "hi")];
        let rows = cc.build_transcript_rows(80);
        // Should have: role header ("You"), content row ("hi"), blank separator.
        assert!(rows.len() >= 3);
        assert_eq!(rows[0].text, "You");
        assert_eq!(rows[1].text, "hi");
    }

    #[test]
    fn build_transcript_rows_wraps_long_line() {
        let mut cc = TestHost::new("c");
        // A 10-char line. col_budget=7 → effective content budget=5 (after the
        // 2-unit indent subtract) → "12345" + "67890" = 2 content rows.
        cc.transcript = vec![make_turn(ChatRole::Assistant, "1234567890")];
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
        let mut cc = TestHost::new("c");
        let theme = crate::Theme::default();
        cc.push_turn_markdown(ChatRole::Assistant, "# Heading\nbody", &theme);
        let turn = &cc.chat.transcript[0];
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
        let mut cc = TestHost::new("c");
        cc.push_turn(ChatRole::User, StyledText::plain("plain text"));
        assert!(
            cc.chat.transcript[0].line_scales.is_empty(),
            "push_turn must leave line_scales empty"
        );
    }

    #[test]
    fn build_transcript_rows_flat_path_unchanged() {
        // Turns with empty line_scales must produce rows with empty spans
        // — bit-for-bit identical to the pre-styled-row behaviour.
        let mut cc = TestHost::new("c");
        cc.transcript = vec![make_turn(ChatRole::User, "hello")];
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
        let mut cc = TestHost::new("c");
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
        let mut cc = TestHost::new("c");
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
        // they are inserted by the chat, not from user content.
        let mut cc = TestHost::new("c");
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
        let mut cc = TestHost::new("c");
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
        // `WrapPolicy::Char`, budgeting by display width. 6 CJK
        // characters are 6 chars but 12 display cells — a
        // `chars().count()`-based budget would wrongly let them all
        // through on one row at content_budget=6; the display-width-aware
        // wrapper must split after 3 characters (6 cells).
        let mut cc = TestHost::new("c");
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

    // `wrap_spans`/`WrapPolicy` are unit-tested in `text_util.rs`; this
    // module only tests the `WrapPolicy::Char` call site.

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

    // ── Input soft-wrap ────────────────────────────────────────────────

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
    fn wrap_line_exact_never_exceeds_budget_with_wide_glyphs() {
        // 4 CJK chars (8 display columns) at a budget of 4 cells must
        // split into rows that each measure <= 4 cells — a char-count
        // budget would instead put all 4 chars (8 cols) on one row.
        let rows = wrap_line_exact("日本語日", 4);
        for (_, text) in &rows {
            assert!(
                crate::text_util::display_width(text) <= 4,
                "row {text:?} exceeds the 4-cell budget"
            );
        }
        // Still an exact, lossless reconstruction.
        let joined: String = rows.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(joined, "日本語日");
    }

    #[test]
    fn wrap_line_exact_single_wide_glyph_overflows_a_degenerate_one_cell_budget() {
        // At `col_budget == 1`, a single 2-cell-wide glyph can never fit
        // without being dropped or split mid-glyph — neither of which
        // `wrap_line_exact` will do, since lossless reconstruction (every
        // row's text concatenates back to the exact input) always wins
        // over the cell budget here. The forward-progress fallback takes
        // the whole glyph anyway, so this row is intentionally 2 cells
        // wide despite the 1-cell budget. Pinned so a future reader sees
        // this is deliberate overflow, not an untested gap, and doesn't
        // "fix" it into a wrap that silently drops or mangles the glyph.
        let rows = wrap_line_exact("日", 1);
        assert_eq!(rows, vec![(0, "日".to_string())]);
        assert_eq!(crate::text_util::display_width(&rows[0].1), 2);
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

    fn lines(strs: &[&str]) -> Vec<String> {
        strs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn wrap_input_rows_one_row_per_short_logical_line() {
        let rows = wrap_input_rows(&lines(&["ab", "cd"]), 80);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].logical_line, 0);
        assert_eq!(rows[1].logical_line, 1);
    }

    #[test]
    fn wrap_input_rows_empty_buffer_yields_one_empty_row() {
        let rows = wrap_input_rows(&lines(&[""]), 80);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text, "");
    }

    #[test]
    fn logical_to_visual_round_trips_through_visual_to_logical() {
        let rows = wrap_input_rows(&lines(&["hello world", "second line"]), 6);
        assert!(rows.len() > 2, "expected wrapping to produce >2 rows");

        // Logical (0, 8) -> "hello wo|rld...".
        let (row_idx, visual_col) = input_logical_to_visual(&rows, 0, 8);
        let logical = input_visual_to_logical(&rows, row_idx, visual_col);
        assert_eq!(
            logical,
            (0, 8),
            "round trip through visual space must be lossless"
        );
    }

    #[test]
    fn logical_to_visual_end_of_line_lands_on_last_visual_row() {
        let line = "hello world";
        let rows = wrap_input_rows(&lines(&[line]), 6); // wraps into 2 rows
        let last_row = rows.len() - 1;
        let (row_idx, _) = input_logical_to_visual(&rows, 0, line.chars().count());
        assert_eq!(row_idx, last_row);
    }

    // ── Up/Down move by VISUAL row when wrapped ────────────────────────

    #[test]
    fn up_within_wrapped_paragraph_moves_cursor_not_history() {
        let mut cc = TestHost::new("c");
        cc.input_rows = (1, 8);
        cc.input_insert_str("hello world this wraps");
        cc.state.history.push("prev".into());
        // Narrow rect forces wrapping: content width well under the text length.
        let rect = Rect::new(0.0, 0.0, 12.0, 24.0);
        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Up),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let ev = cc.handle(&event, &RecordingBackend::new(), rect);
        assert_eq!(ev, Handled::Consumed);
        // History must NOT have been entered — the cursor started on a
        // later visual row of the wrapped paragraph, not the first one.
        assert_eq!(cc.state.history_pos, None);
        assert_eq!(cc.input_text(), "hello world this wraps");
    }

    #[test]
    fn up_on_first_visual_row_of_wrapped_paragraph_enters_history() {
        let mut cc = TestHost::new("c");
        cc.input_insert_str("hello world this wraps");
        cc.chat.input_cursor.1 = 3; // still within the first wrapped row
        cc.state.history.push("prev".into());
        let rect = Rect::new(0.0, 0.0, 12.0, 24.0);
        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Up),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let ev = cc.handle(&event, &RecordingBackend::new(), rect);
        assert_eq!(ev, Handled::Consumed);
        assert_eq!(cc.input_text(), "prev", "should have entered history");
    }

    #[test]
    fn down_on_last_visual_row_of_wrapped_paragraph_enters_history_next() {
        let mut cc = TestHost::new("c");
        // A wrapped paragraph sitting in `saved_input`, restored once Down
        // walks off the newest history entry.
        cc.input_insert_str("hello world this wraps");
        cc.state.history.push("first".into());
        cc.state.history.push("second".into());
        let rect = Rect::new(0.0, 0.0, 12.0, 24.0);
        // Drive `history_prev` directly (bypassing the Up key) to reach a
        // deterministic "navigating the newest entry" state.
        let _ = cc.history_prev();
        assert_eq!(cc.input_text(), "second");

        let event = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Down),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let ev = cc.handle(&event, &RecordingBackend::new(), rect);
        assert_eq!(ev, Handled::Consumed);
        // "second" fits on one visual row even at width 12, so Down from
        // its only (first==last) row walks off the newest history entry
        // and restores the saved wrapped paragraph.
        assert_eq!(cc.state.history_pos, None);
        assert_eq!(cc.input_text(), "hello world this wraps");
    }

    // ── Input backspace / delete ──────────────────────────────────────

    #[test]
    fn backspace_deletes_previous_char() {
        let mut cc = TestHost::new("c");
        cc.input_insert_str("hi");
        cc.apply_input_op(EditOp::DeleteBackward);
        assert_eq!(cc.input_text(), "h");
        assert_eq!(cc.chat.input_cursor.1, 1);
    }

    #[test]
    fn backspace_at_start_is_noop() {
        let mut cc = TestHost::new("c");
        cc.input_insert_str("x");
        cc.chat.input_cursor.1 = 0;
        cc.apply_input_op(EditOp::DeleteBackward);
        assert_eq!(cc.input_text(), "x");
    }

    #[test]
    fn delete_removes_char_at_cursor() {
        let mut cc = TestHost::new("c");
        cc.input_insert_str("ab");
        cc.chat.input_cursor.1 = 0;
        cc.apply_input_op(EditOp::DeleteForward);
        assert_eq!(cc.input_text(), "b");
    }

    // ── Selection via Shift+Arrow ────────────────────────────────────

    fn named_key(key: NamedKey, modifiers: Modifiers) -> UiEvent {
        UiEvent::KeyPressed {
            key: Key::Named(key),
            modifiers,
            repeat: false,
        }
    }

    fn shift() -> Modifiers {
        Modifiers {
            shift: true,
            ..Default::default()
        }
    }

    #[test]
    fn shift_left_extends_selection() {
        let mut cc = TestHost::new("c");
        cc.input_insert_str("hello");
        // Cursor is at the end (col 5); two Shift+Left presses select "lo".
        let _ = cc.handle(
            &named_key(NamedKey::Left, shift()),
            &RecordingBackend::new(),
            make_rect(),
        );
        let _ = cc.handle(
            &named_key(NamedKey::Left, shift()),
            &RecordingBackend::new(),
            make_rect(),
        );
        assert_eq!(cc.selected_text().as_deref(), Some("lo"));
    }

    #[test]
    fn typing_over_a_selection_replaces_it() {
        let mut cc = TestHost::new("c");
        cc.input_insert_str("hello");
        let _ = cc.handle(
            &named_key(NamedKey::Left, shift()),
            &RecordingBackend::new(),
            make_rect(),
        );
        let _ = cc.handle(
            &named_key(NamedKey::Left, shift()),
            &RecordingBackend::new(),
            make_rect(),
        );
        cc.input_insert_char('X'); // "lo" is selected; typing replaces it.
        assert_eq!(cc.input_text(), "helX");
    }

    #[test]
    fn plain_left_after_shift_select_collapses_without_extending() {
        // An unmodified arrow after a selection collapses to the
        // selection's near edge instead of also taking a step — see
        // `EditOp::MoveLeft`'s own doc.
        let mut cc = TestHost::new("c");
        cc.input_insert_str("hello");
        let _ = cc.handle(
            &named_key(NamedKey::Left, shift()),
            &RecordingBackend::new(),
            make_rect(),
        );
        let _ = cc.handle(
            &named_key(NamedKey::Left, Modifiers::default()),
            &RecordingBackend::new(),
            make_rect(),
        );
        assert_eq!(cc.selection_range(), None);
        // One Shift+Left selects col 4..5; the plain Left lands on the
        // selection's near edge (4), not one further step back (3).
        assert_eq!(cc.chat.input_cursor.1, 4);
    }

    // ── Word ops: Ctrl+Backspace / Ctrl+Left/Right ────────────────────

    #[test]
    fn ctrl_backspace_deletes_previous_word() {
        let mut cc = TestHost::new("c");
        cc.input_insert_str("hello world");
        let ev = cc.handle(
            &named_key(
                NamedKey::Backspace,
                Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &RecordingBackend::new(),
            make_rect(),
        );
        assert_eq!(ev, Handled::Consumed);
        assert_eq!(cc.input_text(), "hello ");
    }

    #[test]
    fn ctrl_backspace_at_line_start_merges_with_previous_line() {
        let mut cc = TestHost::new("c");
        cc.input_insert_str("hello\nworld");
        cc.apply_input_op(EditOp::MoveLineStart { extend: false });
        let _ = cc.handle(
            &named_key(
                NamedKey::Backspace,
                Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &RecordingBackend::new(),
            make_rect(),
        );
        assert_eq!(cc.input_text(), "helloworld");
    }

    #[test]
    fn ctrl_backspace_with_active_selection_deletes_only_the_selection() {
        let mut cc = TestHost::new("c");
        cc.input_insert_str("hello world");
        let _ = cc.handle(
            &named_key(NamedKey::Left, shift()),
            &RecordingBackend::new(),
            make_rect(),
        );
        let _ = cc.handle(
            &named_key(NamedKey::Left, shift()),
            &RecordingBackend::new(),
            make_rect(),
        ); // "ld" selected.
        let _ = cc.handle(
            &named_key(
                NamedKey::Backspace,
                Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &RecordingBackend::new(),
            make_rect(),
        );
        assert_eq!(cc.input_text(), "hello wor");
    }

    #[test]
    fn ctrl_left_moves_cursor_to_previous_word_start() {
        let mut cc = TestHost::new("c");
        cc.input_insert_str("hello world");
        let _ = cc.handle(
            &named_key(
                NamedKey::Left,
                Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &RecordingBackend::new(),
            make_rect(),
        );
        assert_eq!(cc.chat.input_cursor.1, 6);
    }

    #[test]
    fn ctrl_shift_left_selects_the_previous_word() {
        let mut cc = TestHost::new("c");
        cc.input_insert_str("hello world");
        let _ = cc.handle(
            &named_key(
                NamedKey::Left,
                Modifiers {
                    ctrl: true,
                    shift: true,
                    ..Default::default()
                },
            ),
            &RecordingBackend::new(),
            make_rect(),
        );
        assert_eq!(cc.selected_text().as_deref(), Some("world"));
    }

    // ── Layout zones ─────────────────────────────────────────────────

    #[test]
    fn layout_status_at_top() {
        let cc = TestHost::new("c");
        let layout = cc.compute_layout(&RecordingBackend::new(), make_rect());
        assert_eq!(layout.status.y, 0.0);
        assert_eq!(layout.status.height, 1.0); // line_height = 1.0
    }

    #[test]
    fn layout_input_at_bottom() {
        let cc = TestHost::new("c");
        let layout = cc.compute_layout(&RecordingBackend::new(), make_rect());
        // Auto-grow: empty input wraps to 1 visual row, clamped
        // to the default input_min_rows=1. lh=1, border=2 → input_h = 3.
        let expected_input_y = make_rect().height - (1.0 * 1.0 + 2.0);
        assert_eq!(layout.input.y, expected_input_y);
    }

    #[test]
    fn layout_input_grows_with_wrapped_row_count() {
        let mut cc = TestHost::new("c");
        cc.input_insert_str("line one\nline two\nline three");
        let layout = cc.compute_layout(&RecordingBackend::new(), make_rect());
        // 3 short logical lines, wide rect → 3 visual rows, no wrapping.
        // lh=1, border=2 → input_h = 5.
        let expected_input_y = make_rect().height - (3.0 * 1.0 + 2.0);
        assert_eq!(layout.input.y, expected_input_y);
    }

    #[test]
    fn layout_input_clamps_to_max_rows() {
        let mut cc = TestHost::new("c");
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
    fn input_rows_override_the_default_clamp() {
        let mut cc = TestHost::new("c");
        cc.input_rows = (2, 3);
        let layout = cc.compute_layout(&RecordingBackend::new(), make_rect());
        // Empty input wraps to 1 visual row, clamped up to the new min=2.
        let expected_input_y = make_rect().height - (2.0 * 1.0 + 2.0);
        assert_eq!(layout.input.y, expected_input_y);
    }

    #[test]
    fn input_rows_floor_max_to_min() {
        let mut cc = TestHost::new("c");
        cc.input_rows = (5, 2); // max < min — max floored up to min.
        let layout = cc.compute_layout(&RecordingBackend::new(), make_rect());
        let expected_input_y = make_rect().height - (5.0 * 1.0 + 2.0);
        assert_eq!(layout.input.y, expected_input_y);
    }

    #[test]
    fn input_rows_floor_min_to_one() {
        let mut cc = TestHost::new("c");
        cc.input_rows = (0, 0);
        let layout = cc.compute_layout(&RecordingBackend::new(), make_rect());
        let expected_input_y = make_rect().height - (1.0 * 1.0 + 2.0);
        assert_eq!(layout.input.y, expected_input_y);
    }

    #[test]
    fn layout_no_scrollbar_when_transcript_empty() {
        let cc = TestHost::new("c");
        let layout = cc.compute_layout(&RecordingBackend::new(), make_rect());
        assert!(layout.scrollbar.is_none());
    }

    #[test]
    fn layout_scrollbar_present_when_transcript_nonempty() {
        let mut cc = TestHost::new("c");
        cc.transcript = vec![make_turn(ChatRole::User, "hello")];
        let layout = cc.compute_layout(&RecordingBackend::new(), make_rect());
        assert!(layout.scrollbar.is_some());
        // Transcript rect should be narrower than full width.
        assert!(layout.transcript.width < make_rect().width);
    }

    #[test]
    fn layout_spinner_present_when_busy() {
        let mut cc = TestHost::new("c");
        cc.busy = true;
        let layout = cc.compute_layout(&RecordingBackend::new(), make_rect());
        assert!(layout.spinner.is_some());
    }

    #[test]
    fn layout_spinner_absent_when_not_busy() {
        let cc = TestHost::new("c");
        let layout = cc.compute_layout(&RecordingBackend::new(), make_rect());
        assert!(layout.spinner.is_none());
    }

    // ── Send/Stop segment + hint layout ────────────────────────────────

    #[test]
    fn layout_send_segment_present_when_row_wide_enough() {
        let cc = TestHost::new("c");
        let layout = cc.compute_layout(&RecordingBackend::new(), make_rect());
        assert!(layout.send.is_some());
        // The TextInput's own rect should be narrower than the full row
        // to make room for the segment.
        assert!(layout.input.width < make_rect().width);
    }

    #[test]
    fn layout_send_segment_absent_when_row_too_narrow() {
        let cc = TestHost::new("c");
        let narrow = Rect::new(0.0, 0.0, 10.0, 24.0);
        let layout = cc.compute_layout(&RecordingBackend::new(), narrow);
        assert!(layout.send.is_none());
        // TextInput falls back to the full row width.
        assert_eq!(layout.input.width, narrow.width);
    }

    #[test]
    fn layout_send_segment_spans_full_input_height() {
        let cc = TestHost::new("c");
        let layout = cc.compute_layout(&RecordingBackend::new(), make_rect());
        let send = layout.send.expect("send segment present");
        assert_eq!(send.y, layout.input.y);
        assert_eq!(send.height, layout.input.height);
    }

    #[test]
    fn layout_hint_absent_by_default() {
        let cc = TestHost::new("c");
        let layout = cc.compute_layout(&RecordingBackend::new(), make_rect());
        assert!(layout.hint.is_none());
    }

    #[test]
    fn layout_hint_present_when_set() {
        let mut cc = TestHost::new("c");
        cc.hint = Some(StyledText::plain("Enter to send, Shift+Enter for newline"));
        let layout = cc.compute_layout(&RecordingBackend::new(), make_rect());
        let hint = layout.hint.expect("hint row present");
        // The hint row sits directly above the input box.
        assert_eq!(hint.y + hint.height, layout.input.y);
    }

    #[test]
    fn layout_hint_cleared_by_set_hint_none() {
        let mut cc = TestHost::new("c");
        cc.hint = Some(StyledText::plain("hint"));
        cc.hint = None;
        let layout = cc.compute_layout(&RecordingBackend::new(), make_rect());
        assert!(layout.hint.is_none());
    }

    // ── Send/Stop segment click routing ────────────────────────────────

    fn click_at(cc: &mut TestHost, rect: Rect, x: f32, y: f32) -> Handled<ChatEvent> {
        let event = crate::mouse_down(MouseButton::Left, x, y, Modifiers::default());
        cc.handle(&event, &RecordingBackend::new(), rect)
    }

    #[test]
    fn click_send_segment_while_idle_submits() {
        let mut cc = TestHost::new("c");
        cc.input_insert_str("hello");
        let rect = make_rect();
        let send = cc
            .compute_layout(&RecordingBackend::new(), rect)
            .send
            .expect("send segment present");
        let ev = click_at(&mut cc, rect, send.x + 1.0, send.y + 1.0);
        assert_eq!(
            ev,
            Handled::Event(ChatEvent::Submit {
                text: "hello".into()
            })
        );
    }

    #[test]
    fn click_send_segment_while_idle_and_empty_is_ignored() {
        let mut cc = TestHost::new("c");
        let rect = make_rect();
        let send = cc
            .compute_layout(&RecordingBackend::new(), rect)
            .send
            .expect("send segment present");
        let ev = click_at(&mut cc, rect, send.x + 1.0, send.y + 1.0);
        assert_eq!(ev, Handled::Ignored);
    }

    #[test]
    fn click_send_segment_while_busy_emits_stop_requested() {
        let mut cc = TestHost::new("c");
        cc.input_insert_str("hello");
        cc.busy = true;
        let rect = make_rect();
        let send = cc
            .compute_layout(&RecordingBackend::new(), rect)
            .send
            .expect("send segment present");
        let ev = click_at(&mut cc, rect, send.x + 1.0, send.y + 1.0);
        assert_eq!(ev, Handled::Event(ChatEvent::StopRequested));
        // The input buffer is left untouched by a stop request.
        assert_eq!(cc.input_text(), "hello");
    }

    // ── Collapsible turns and per-turn hit regions ──────────────────────

    #[test]
    fn turn_summary_line_defaults_to_first_nonblank_line() {
        let mut cc = TestHost::new("c");
        cc.transcript = vec![make_turn(ChatRole::Assistant, "\n  \nreal content\nmore")];
        assert_eq!(cc.turn_summary_line(0), "real content");
    }

    #[test]
    fn turn_summary_line_prefers_explicit_summary() {
        let mut cc = TestHost::new("c");
        cc.transcript = vec![make_turn(ChatRole::Assistant, "full body text")];
        cc.set_turn_summary(0, Some("custom summary".to_string()));
        assert_eq!(cc.turn_summary_line(0), "custom summary");
    }

    #[test]
    fn turn_summary_line_empty_for_turn_with_no_content() {
        let mut cc = TestHost::new("c");
        cc.transcript = vec![make_turn(ChatRole::Assistant, "")];
        assert_eq!(cc.turn_summary_line(0), "");
    }

    #[test]
    fn turn_summary_line_empty_for_out_of_range_turn_idx() {
        let cc = TestHost::new("c");
        assert_eq!(cc.turn_summary_line(0), "");
    }

    #[test]
    fn set_turn_summary_none_clears_the_override() {
        let mut cc = TestHost::new("c");
        cc.transcript = vec![make_turn(ChatRole::Assistant, "fallback line")];
        cc.set_turn_summary(0, Some("override".to_string()));
        assert_eq!(cc.turn_summary_line(0), "override");
        cc.set_turn_summary(0, None);
        assert_eq!(cc.turn_summary_line(0), "fallback line");
    }

    #[test]
    fn new_turns_start_expanded() {
        let mut cc = TestHost::new("c");
        cc.transcript = vec![make_turn(ChatRole::User, "hi")];
        assert!(!cc.is_turn_collapsed(0));
    }

    #[test]
    fn toggle_turn_collapsed_flips_state() {
        let mut cc = TestHost::new("c");
        cc.transcript = vec![make_turn(ChatRole::User, "hi")];
        cc.toggle_turn_collapsed(0);
        assert!(cc.is_turn_collapsed(0));
        cc.toggle_turn_collapsed(0);
        assert!(!cc.is_turn_collapsed(0));
    }

    #[test]
    fn set_turn_collapsed_out_of_range_is_a_noop() {
        let mut cc = TestHost::new("c");
        cc.set_turn_collapsed(5, true);
        cc.toggle_turn_collapsed(5);
        assert!(!cc.is_turn_collapsed(5));
    }

    #[test]
    fn build_transcript_rows_collapsed_turn_emits_header_and_summary_only() {
        let mut cc = TestHost::new("c");
        cc.transcript = vec![make_turn(ChatRole::Assistant, "first line\nsecond line")];
        cc.set_turn_collapsed(0, true);

        let rows = cc.build_transcript_rows(80);
        // Header row + one summary row + the blank inter-turn separator —
        // not the full two-line body.
        assert_eq!(rows.len(), 3);
        assert!(
            rows[0].text.contains("AI"),
            "header row should still show the role label: {:?}",
            rows[0].text
        );
        assert_eq!(rows[1].text, "first line");
    }

    #[test]
    fn build_transcript_rows_expanded_turn_unchanged_by_collapsed_field() {
        // `collapsed` defaulting to `false` must not perturb the existing
        // flat-path row output at all.
        let mut cc = TestHost::new("c");
        cc.transcript = vec![make_turn(ChatRole::User, "hello")];
        let rows = cc.build_transcript_rows(80);
        assert_eq!(rows[0].text, "You");
        assert_eq!(rows[1].text, "hello");
        assert_eq!(rows[1].indent, 2.0);
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn transcript_click_on_first_turn_header_returns_turn_clicked() {
        let mut cc = TestHost::new("c");
        cc.transcript = vec![
            make_turn(ChatRole::User, "Hello"),
            make_turn(ChatRole::Assistant, "Hi"),
        ];
        let rect = make_rect();
        let t = cc.compute_layout(&RecordingBackend::new(), rect).transcript;
        // Row 0 (turn 0's header row) sits at the very top of the
        // transcript rect (line_height == 1.0 on `RecordingBackend`).
        let ev = click_at(&mut cc, rect, t.x + 1.0, t.y + 0.5);
        assert_eq!(
            ev,
            Handled::Event(ChatEvent::TurnClicked {
                turn_idx: 0,
                row_in_turn: 0
            })
        );
        // A click also moves the keyboard cursor onto the clicked turn so
        // mouse and keyboard interactions stay in sync.
        assert_eq!(cc.focused_turn(), Some(0));
    }

    #[test]
    fn transcript_click_on_second_turn_resolves_its_turn_idx() {
        let mut cc = TestHost::new("c");
        cc.transcript = vec![
            make_turn(ChatRole::User, "Hello"),
            make_turn(ChatRole::Assistant, "Hi"),
        ];
        let rect = make_rect();
        let t = cc.compute_layout(&RecordingBackend::new(), rect).transcript;
        // Turn 0 occupies rows 0-2 (header, content, blank separator);
        // turn 1's header starts at row 3.
        let ev = click_at(&mut cc, rect, t.x + 1.0, t.y + 3.5);
        assert_eq!(
            ev,
            Handled::Event(ChatEvent::TurnClicked {
                turn_idx: 1,
                row_in_turn: 0
            })
        );
        assert_eq!(cc.focused_turn(), Some(1));
    }

    #[test]
    fn click_then_enter_toggles_the_clicked_turn_not_a_stale_focus() {
        // Pressing `Enter` after a mouse click must toggle the clicked turn,
        // using the focused_turn set by the click.
        let mut cc = TestHost::new("c");
        cc.transcript = vec![
            make_turn(ChatRole::User, "Hello"),
            make_turn(ChatRole::Assistant, "Hi"),
        ];
        let rect = make_rect();
        let backend = RecordingBackend::new();
        let t = cc.compute_layout(&backend, rect).transcript;

        let _ = click_at(&mut cc, rect, t.x + 1.0, t.y + 3.5);
        assert_eq!(cc.focused_turn(), Some(1));
        assert!(!cc.is_turn_collapsed(1));

        let enter = key_event(Key::Named(NamedKey::Enter), Modifiers::default());
        let ev = cc.handle(&enter, &backend, rect);
        assert_eq!(ev, Handled::Consumed);
        assert!(cc.is_turn_collapsed(1));
        assert!(!cc.is_turn_collapsed(0));
    }

    #[test]
    fn transcript_click_below_last_row_is_consumed_not_turn_clicked() {
        let mut cc = TestHost::new("c");
        cc.transcript = vec![make_turn(ChatRole::User, "Hi")];
        let rect = make_rect();
        let t = cc.compute_layout(&RecordingBackend::new(), rect).transcript;
        // Far below the 3 painted rows, but still inside the transcript
        // rect's blank tail.
        let ev = click_at(&mut cc, rect, t.x + 1.0, t.y + t.height - 0.5);
        assert_eq!(ev, Handled::Consumed);
    }

    // ── Focused-turn keyboard cursor ─────────────────────────────────────

    fn key_event(key: Key, modifiers: Modifiers) -> UiEvent {
        UiEvent::KeyPressed {
            key,
            modifiers,
            repeat: false,
        }
    }

    #[test]
    fn tab_cycles_focused_turn_then_back_to_input() {
        let mut cc = TestHost::new("c");
        cc.transcript = vec![
            make_turn(ChatRole::User, "one"),
            make_turn(ChatRole::Assistant, "two"),
        ];
        let rect = make_rect();
        let backend = RecordingBackend::new();
        let tab = key_event(Key::Named(NamedKey::Tab), Modifiers::default());

        assert_eq!(cc.focused_turn(), None);
        assert!(cc.input_has_focus());

        let _ = cc.handle(&tab, &backend, rect);
        assert_eq!(cc.focused_turn(), Some(0));
        assert!(!cc.input_has_focus());

        let _ = cc.handle(&tab, &backend, rect);
        assert_eq!(cc.focused_turn(), Some(1));

        let _ = cc.handle(&tab, &backend, rect);
        assert_eq!(cc.focused_turn(), None);
        assert!(cc.input_has_focus());
    }

    #[test]
    fn shift_tab_cycles_focused_turn_backward() {
        let mut cc = TestHost::new("c");
        cc.transcript = vec![
            make_turn(ChatRole::User, "one"),
            make_turn(ChatRole::Assistant, "two"),
        ];
        let rect = make_rect();
        let backend = RecordingBackend::new();
        let shift_tab = key_event(
            Key::Named(NamedKey::Tab),
            Modifiers {
                shift: true,
                ..Default::default()
            },
        );

        let _ = cc.handle(&shift_tab, &backend, rect);
        assert_eq!(cc.focused_turn(), Some(1));
        let _ = cc.handle(&shift_tab, &backend, rect);
        assert_eq!(cc.focused_turn(), Some(0));
        let _ = cc.handle(&shift_tab, &backend, rect);
        assert_eq!(cc.focused_turn(), None);
    }

    #[test]
    fn tab_with_empty_transcript_is_a_noop() {
        let mut cc = TestHost::new("c");
        let rect = make_rect();
        let backend = RecordingBackend::new();
        let tab = key_event(Key::Named(NamedKey::Tab), Modifiers::default());
        let _ = cc.handle(&tab, &backend, rect);
        assert_eq!(cc.focused_turn(), None);
        assert!(cc.input_has_focus());
    }

    #[test]
    fn enter_while_turn_focused_toggles_collapsed_instead_of_submitting() {
        let mut cc = TestHost::new("c");
        cc.transcript = vec![make_turn(ChatRole::Assistant, "body")];
        cc.set_focused_turn(Some(0));
        let rect = make_rect();
        let backend = RecordingBackend::new();
        let enter = key_event(Key::Named(NamedKey::Enter), Modifiers::default());

        assert!(!cc.is_turn_collapsed(0));
        let ev = cc.handle(&enter, &backend, rect);
        assert_eq!(ev, Handled::Consumed);
        assert!(cc.is_turn_collapsed(0));

        // Pressing Enter again expands it back.
        let ev2 = cc.handle(&enter, &backend, rect);
        assert_eq!(ev2, Handled::Consumed);
        assert!(!cc.is_turn_collapsed(0));
    }

    #[test]
    fn ctrl_enter_still_submits_while_a_turn_is_focused() {
        let mut cc = TestHost::new("c");
        cc.transcript = vec![make_turn(ChatRole::Assistant, "body")];
        cc.set_focused_turn(Some(0));
        cc.input_insert_str("hello");
        let rect = make_rect();
        let backend = RecordingBackend::new();
        let ctrl_enter = key_event(
            Key::Named(NamedKey::Enter),
            Modifiers {
                ctrl: true,
                ..Default::default()
            },
        );
        let ev = cc.handle(&ctrl_enter, &backend, rect);
        assert_eq!(
            ev,
            Handled::Event(ChatEvent::Submit {
                text: "hello".into()
            })
        );
        // The universal submit chord bypasses the toggle — the turn stays
        // expanded.
        assert!(!cc.is_turn_collapsed(0));
    }

    #[test]
    fn typing_returns_focus_to_input_from_a_focused_turn() {
        let mut cc = TestHost::new("c");
        cc.transcript = vec![make_turn(ChatRole::User, "one")];
        cc.set_focused_turn(Some(0));
        assert!(!cc.input_has_focus());

        let rect = make_rect();
        let backend = RecordingBackend::new();
        let _ = cc.handle(&UiEvent::CharTyped('x'), &backend, rect);

        assert_eq!(cc.focused_turn(), None);
        assert!(cc.input_has_focus());
        assert_eq!(cc.input_text(), "x");
    }

    // ── Backend rendering tests ───────────────────────────────────────
    //
    // The tests above exercise state/layout against a `RecordingBackend` whose
    // draw calls are no-ops. These two paint the chat through the
    // real `TuiBackend` / `GtkBackend` trait impls into a backend-owned
    // headless surface (a `ratatui::Buffer` / `cairo::ImageSurface`) and
    // assert that the message list, text input, scrollbar and spinner
    // paints produced output. This
    // is the golden-style coverage required by the issue acceptance
    // criteria (TESTING.md "coordinate drift" row).

    /// TUI: paint a populated chat into a `ratatui::Buffer` via the
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

        let mut cc = TestHost::new("chat");
        cc.status = StyledText::plain("Refining issue #264");
        cc.model_label = "claude-opus-4-5".to_string();
        cc.transcript = vec![
            make_turn(ChatRole::User, "Hello there"),
            make_turn(ChatRole::Assistant, "General Kenobi"),
        ];

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

    /// TUI: the Send/Stop segment paints "Send" while idle and
    /// "Stop" while busy, and the persistent hint row paints its text —
    /// through the real `Form` cell painter and the message list's own
    /// cell paint, not a mock.
    #[cfg(feature = "tui")]
    #[test]
    fn tui_render_paints_send_stop_segment_and_hint() {
        use crate::tui::TuiBackend;
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        const W: u16 = 48;
        const H: u16 = 14;

        fn paint(cc: &TestHost) -> String {
            let mut terminal =
                Terminal::new(TestBackend::new(W, H)).expect("construct test terminal");
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
            let buf = terminal.backend().buffer();
            let mut painted = String::new();
            for y in 0..H {
                for x in 0..W {
                    painted.push(buf[(x, y)].symbol().chars().next().unwrap_or(' '));
                }
                painted.push('\n');
            }
            painted
        }

        let mut cc = TestHost::new("chat");
        cc.hint = Some(StyledText::plain("Enter to send"));

        let idle = paint(&cc);
        assert!(idle.contains("Send"), "Send label not painted:\n{idle}");
        assert!(
            idle.contains("Enter to send"),
            "hint row not painted:\n{idle}"
        );

        cc.busy = true;
        let busy = paint(&cc);
        assert!(busy.contains("Stop"), "Stop label not painted:\n{busy}");
    }

    /// TUI: the input placeholder advertises the binding that is actually live.
    ///
    /// `set_submit_on_enter(true)` inverts the meaning of `Enter` and
    /// `Shift+Enter`, so the placeholder must change to match. The default
    /// placeholder ("Alt+Enter to send, Enter for newline") would instruct the
    /// user to do the opposite of what the keymap does. Tested with the real
    /// `TuiBackend::draw_text_input` rasteriser, wide enough that the full
    /// string fits without truncation.
    #[cfg(feature = "tui")]
    #[test]
    fn tui_placeholder_tracks_submit_on_enter_mode() {
        use crate::tui::TuiBackend;
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        // Wide enough that the whole placeholder fits inside the input box
        // (box borders + the Send segment eat ~10 columns).
        const W: u16 = 110;
        const H: u16 = 14;

        fn paint(cc: &TestHost) -> String {
            let mut terminal =
                Terminal::new(TestBackend::new(W, H)).expect("construct test terminal");
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
            let buf = terminal.backend().buffer();
            let mut painted = String::new();
            for y in 0..H {
                for x in 0..W {
                    painted.push(buf[(x, y)].symbol().chars().next().unwrap_or(' '));
                }
                painted.push('\n');
            }
            painted
        }

        // Default mode: Alt+Enter sends, plain Enter breaks the line.
        let mut cc = TestHost::new("chat");
        let default_mode = paint(&cc);
        assert!(
            default_mode.contains("Alt+Enter to send"),
            "default-mode placeholder should advertise Alt+Enter as submit:\n{default_mode}"
        );
        assert!(
            default_mode.contains("Enter for newline"),
            "default-mode placeholder should advertise plain Enter as newline:\n{default_mode}"
        );

        // submit_on_enter: the two bindings swap, and so must the hint.
        cc.submit_on_enter = true;
        let enter_mode = paint(&cc);
        assert!(
            enter_mode.contains("Enter to send"),
            "submit_on_enter placeholder should advertise Enter as submit:\n{enter_mode}"
        );
        assert!(
            enter_mode.contains("Shift+Enter for newline"),
            "submit_on_enter placeholder should advertise Shift+Enter as newline:\n{enter_mode}"
        );
        assert!(
            !enter_mode.contains("Alt+Enter to send"),
            "submit_on_enter placeholder must not still claim Alt+Enter submits — in this \
             mode Alt+Enter inserts a newline:\n{enter_mode}"
        );
    }

    /// GTK: paint a populated chat into a `cairo::ImageSurface` via
    /// the `GtkBackend` trait path against a white background, then assert
    /// (a) the render inked at least one non-white pixel — proving the GTK
    /// rasterisers actually drew — and (b) the backend's
    /// `text_input_layout` returns a non-empty layout for the input zone.
    #[cfg(feature = "gtk")]
    #[test]
    fn gtk_render_inks_surface_and_layout_nonempty() {
        use crate::backend::DrawsPrimitives;
        use crate::gtk::GtkBackend;
        use pangocairo::cairo::{Context, Format, ImageSurface};

        const W: i32 = 360;
        const H: i32 = 220;

        let mut cc = TestHost::new("chat");
        cc.status = StyledText::plain("Plan review for #264");
        cc.model_label = "claude-opus-4-5".to_string();
        cc.transcript = vec![
            make_turn(ChatRole::User, "Hello there"),
            make_turn(ChatRole::Assistant, "General Kenobi"),
        ];

        let mut backend = GtkBackend::new();
        // White background: any non-white pixel must have come from the
        // chat's foreground ink (text / borders / scrollbar).
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
        assert!(inked, "the chat painted nothing into the GTK surface");

        // (b) the input zone's widget-contract layout yields usable rows.
        let mut ti = TextInput::new(WidgetId::new("probe-input"));
        ti.lines = vec!["hello".to_string()];
        ti.cursor_col = 5;
        ti.has_focus = true;
        let input_rect = Rect::new(0.0, H as f32 - 80.0, W as f32, 80.0);
        let ti_layout = backend.layout_primitive(input_rect, &ti);
        assert!(
            !ti_layout.layout.visible_lines.is_empty(),
            "the input's resolved layout has no visible lines"
        );
    }
}

#[cfg(test)]
mod handle_event_tests {
    use super::*;
    use crate::testing::RecordingBackend;

    fn make_rect() -> Rect {
        Rect::new(0.0, 0.0, 80.0, 20.0)
    }

    #[test]
    fn handle_reports_submit_as_its_semantic_event() {
        let mut cc = TestHost::new("c");
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
        let outcome = cc.handle(&event, &RecordingBackend::new(), rect);
        assert_eq!(
            outcome,
            Handled::Event(ChatEvent::Submit {
                text: "hello".into()
            })
        );
    }

    #[test]
    fn handle_event_returns_consumed_for_a_typed_char() {
        let mut cc = TestHost::new("c");
        let rect = make_rect();
        let outcome = cc.handle(&UiEvent::CharTyped('x'), &RecordingBackend::new(), rect);
        assert_eq!(outcome, Handled::Consumed);
    }

    #[test]
    fn handle_event_returns_ignored_for_an_unrelated_mouse_up() {
        let mut cc = TestHost::new("c");
        let rect = make_rect();
        let event = UiEvent::MouseUp {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(0.0, 0.0),
        };
        let outcome = cc.handle(&event, &RecordingBackend::new(), rect);
        assert_eq!(outcome, Handled::Ignored);
    }
}
