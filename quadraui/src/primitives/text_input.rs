//! [`TextInput`] — standalone multi-line text input primitive.
//!
//! Used for free-form text entry (commit messages, multi-line search,
//! note-taking). Stores text as a `Vec<String>` of lines, with cursor
//! position tracked as `(line, col)`. The primitive handles vertical
//! scroll auto-clamp (cursor stays in viewport) and emits hit regions
//! per visible line so consumers route clicks back to text positions.
//!
//! V1 is line-based (consumer pre-splits long input into lines). Word
//! wrap inside the primitive is a future extension; today consumers
//! split on `\n` or pre-wrap at their preferred width.
//!
//! ## Editing — [`EditOp`] / [`TextInput::apply`]
//!
//! Before issue #833, `TextInput` had no editing behaviour at all: apps
//! (e.g. `examples/common/text_input_demo.rs`) had to hand-roll their own
//! insert/backspace/arrow-key logic against `lines`/`cursor_line`/
//! `cursor_col` directly. [`EditOp`] is a closed set of edit intents
//! (insert, delete, cursor movement, selection, undo/redo);
//! [`TextInput::apply`] is the single mutator that turns one into the
//! next buffer/cursor/selection state, so every consumer gets the same
//! (correct, multibyte-safe) editing behaviour instead of an ad hoc copy.
//! [`EditOp::from_key`] maps a plain keypress to the `EditOp` it means;
//! [`EditOp::from_key_binding`] maps the universal
//! [`crate::accelerator::KeyBinding::Undo`] /
//! [`crate::accelerator::KeyBinding::Redo`] /
//! [`crate::accelerator::KeyBinding::SelectAll`] accelerators the same
//! way, wiring those long-declared-but-unused names (see
//! `accelerator.rs`'s module doc) to real behaviour.
//!
//! Undo/redo is a [`crate::undo::UndoStack`] of buffer snapshots, private
//! to `TextInput` and excluded from (de)serialization — history is
//! runtime-only, not part of a `TextInput`'s durable value. Every
//! content-mutating op records the pre-mutation snapshot; pure cursor
//! movement does not, so moving the cursor between two edits doesn't
//! split them into separate undo steps.

use serde::{Deserialize, Serialize};

use crate::accelerator::KeyBinding;
use crate::event::{Key, NamedKey, Rect};
use crate::types::{Modifiers, WidgetId};
use crate::undo::UndoStack;

/// Multi-line text input. Text is stored as one entry per line — empty
/// lines are empty strings. Cursor is `(line, col)` in *char columns*
/// (not bytes); the rasterisers convert as needed.
///
/// `PartialEq`/`Eq` are hand-implemented (below) rather than derived —
/// they compare content/cursor/selection only, excluding `undo_stack`
/// (runtime-only history; see the field doc).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextInput {
    pub id: WidgetId,
    /// One entry per logical line. Newlines are implicit between
    /// consecutive entries. Empty input is `vec![String::new()]`.
    pub lines: Vec<String>,
    /// Cursor line index. Clamped to `lines.len().saturating_sub(1)`
    /// by [`Self::layout`].
    #[serde(default)]
    pub cursor_line: usize,
    /// Cursor column within the cursor line, in *char columns*.
    /// Clamped to the line's char count by [`Self::layout`].
    #[serde(default)]
    pub cursor_col: usize,
    /// Optional placeholder shown when `lines` is empty or contains a
    /// single empty string. Rendered in muted color.
    #[serde(default)]
    pub placeholder: Option<String>,
    /// First visible line index. The primitive clamps this so the
    /// cursor stays inside the viewport.
    #[serde(default)]
    pub scroll_offset: usize,
    /// Horizontal scroll, in char columns. Applied to *every* line.
    /// The primitive clamps this so the cursor stays in view.
    #[serde(default)]
    pub scroll_col: usize,
    /// Whether the input has keyboard focus. Controls cursor visibility
    /// and border color (rasteriser-defined).
    ///
    /// This is set by the app, not derived automatically — see
    /// [`crate::focus`]'s module doc ("representation 5") for why this
    /// field (and the identical convention on several other primitives)
    /// is left as-is rather than collapsed onto `FocusManager` in #830.
    /// An app that *does* drive this `TextInput` through
    /// [`crate::runner::AppLogic::tab_stops`] should set it from
    /// `backend.focus_manager().is_focused(&input.id)` each frame —
    /// [`crate::focus::FocusManager::is_focused`] is the read path such
    /// a migration converges on.
    #[serde(default)]
    pub has_focus: bool,
    /// Selection anchor `(line, col)`, in the same char-column space as
    /// [`Self::cursor_line`]/[`Self::cursor_col`]. The cursor is the
    /// *moving* end of a selection; this is the *fixed* end. `None`
    /// means no active selection. A selection exists only when this is
    /// `Some` **and** differs from the current cursor position — see
    /// [`Self::selection_range`].
    #[serde(default)]
    pub selection_anchor: Option<(usize, usize)>,
    /// Undo/redo history — see the module doc and [`EditOp`]. Runtime-only:
    /// skipped by (de)serialization (a freshly deserialized `TextInput`
    /// starts with empty history, same as [`Self::new`]), and excluded
    /// from equality (two buffers with identical content/cursor/selection
    /// are equal regardless of how their history was built up).
    #[serde(skip)]
    undo_stack: UndoStack<TextInputSnapshot>,
}

impl PartialEq for TextInput {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.lines == other.lines
            && self.cursor_line == other.cursor_line
            && self.cursor_col == other.cursor_col
            && self.placeholder == other.placeholder
            && self.scroll_offset == other.scroll_offset
            && self.scroll_col == other.scroll_col
            && self.has_focus == other.has_focus
            && self.selection_anchor == other.selection_anchor
    }
}

impl Eq for TextInput {}

impl TextInput {
    pub fn new(id: WidgetId) -> Self {
        Self {
            id,
            lines: vec![String::new()],
            cursor_line: 0,
            cursor_col: 0,
            placeholder: None,
            scroll_offset: 0,
            scroll_col: 0,
            has_focus: false,
            selection_anchor: None,
            undo_stack: UndoStack::new(),
        }
    }
}

/// An edit intent for [`TextInput::apply`] — see the module doc.
///
/// Every variant that moves the cursor (`Move*`, `SetCursor`) carries an
/// `extend` flag: `false` collapses any active selection and moves the
/// cursor normally (a plain arrow-key press); `true` starts a selection
/// at the cursor's pre-move position if none is active yet, then moves
/// only the cursor (a shift-held arrow key, or a mouse drag).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditOp {
    /// Insert one character at the cursor. `'\n'` splits the current
    /// line. If a selection is active, it is deleted first (so this also
    /// implements "type over the selection").
    InsertChar(char),
    /// Insert a (possibly multi-line) string at the cursor — e.g. a
    /// clipboard paste. Same selection-replace behaviour as
    /// [`Self::InsertChar`].
    InsertText(String),
    /// Backspace: delete the selection if one is active, else the one
    /// character before the cursor (merging with the previous line at
    /// column 0).
    DeleteBackward,
    /// Delete (forward-delete): delete the selection if one is active,
    /// else the one character after the cursor (merging with the next
    /// line at end-of-line).
    DeleteForward,
    /// Delete the active selection with no replacement. A no-op (returns
    /// `false` from [`TextInput::apply`]) when no selection is active —
    /// callers doing "Cut" should read [`TextInput::selected_text`]
    /// first, then apply this.
    DeleteSelection,
    /// Move the cursor one character left (or to the end of the previous
    /// line, if already at column 0).
    MoveLeft { extend: bool },
    /// Move the cursor one character right (or to the start of the next
    /// line, if already at end-of-line).
    MoveRight { extend: bool },
    /// Move the cursor up one line, clamping the column to the target
    /// line's length.
    MoveUp { extend: bool },
    /// Move the cursor down one line, clamping the column to the target
    /// line's length.
    MoveDown { extend: bool },
    /// Move the cursor to column 0 of the current line.
    MoveLineStart { extend: bool },
    /// Move the cursor to the end of the current line.
    MoveLineEnd { extend: bool },
    /// Move the cursor to `(0, 0)`.
    MoveDocStart { extend: bool },
    /// Move the cursor to the end of the last line.
    MoveDocEnd { extend: bool },
    /// Jump the cursor to an arbitrary `(line, col)` — e.g. from
    /// [`TextInputLayout::hit_test`] resolving a mouse click, or a drag
    /// extending a selection to the drag position.
    SetCursor {
        line: usize,
        col: usize,
        extend: bool,
    },
    /// Select the entire buffer.
    SelectAll,
    /// Undo the last content-mutating op. No-op if there's nothing to
    /// undo.
    Undo,
    /// Redo the last undone op. No-op if there's nothing to redo (or a
    /// new edit was made since the last undo).
    Redo,
}

impl EditOp {
    /// Map a plain keypress to the [`EditOp`] it means, for widgets
    /// wiring [`crate::event::UiEvent::KeyPressed`] straight into
    /// [`TextInput::apply`]. Returns `None` for keys `TextInput` doesn't
    /// interpret (function keys, Escape, Tab, ...) — callers handle
    /// those themselves (Escape to blur, Tab to move focus, etc.).
    ///
    /// `Ctrl+Home`/`Ctrl+End` map to [`EditOp::MoveDocStart`]/
    /// [`EditOp::MoveDocEnd`]; plain `Home`/`End` map to
    /// [`EditOp::MoveLineStart`]/[`EditOp::MoveLineEnd`]. `modifiers.shift`
    /// becomes every movement op's `extend` flag.
    pub fn from_key(key: &Key, modifiers: Modifiers) -> Option<EditOp> {
        let extend = modifiers.shift;
        match key {
            Key::Char(c) => Some(EditOp::InsertChar(*c)),
            Key::Named(NamedKey::Enter) => Some(EditOp::InsertChar('\n')),
            Key::Named(NamedKey::Backspace) => Some(EditOp::DeleteBackward),
            Key::Named(NamedKey::Delete) => Some(EditOp::DeleteForward),
            Key::Named(NamedKey::Left) => Some(EditOp::MoveLeft { extend }),
            Key::Named(NamedKey::Right) => Some(EditOp::MoveRight { extend }),
            Key::Named(NamedKey::Up) => Some(EditOp::MoveUp { extend }),
            Key::Named(NamedKey::Down) => Some(EditOp::MoveDown { extend }),
            Key::Named(NamedKey::Home) if modifiers.ctrl => Some(EditOp::MoveDocStart { extend }),
            Key::Named(NamedKey::End) if modifiers.ctrl => Some(EditOp::MoveDocEnd { extend }),
            Key::Named(NamedKey::Home) => Some(EditOp::MoveLineStart { extend }),
            Key::Named(NamedKey::End) => Some(EditOp::MoveLineEnd { extend }),
            _ => None,
        }
    }

    /// Map a universal [`KeyBinding`] accelerator to the [`EditOp`] it
    /// represents — wires [`KeyBinding::Undo`]/[`KeyBinding::Redo`]/
    /// [`KeyBinding::SelectAll`] (declared in `accelerator.rs` since
    /// before #833, previously with no consumer) to real `TextInput`
    /// behaviour. Apps register an [`crate::accelerator::Accelerator`]
    /// with one of these bindings and, on the matching
    /// [`crate::event::UiEvent::Accelerator`], call
    /// `text_input.apply(op)` with the result.
    ///
    /// Returns `None` for every other binding: `Copy`/`Cut`/`Paste` need
    /// clipboard content that arrives via
    /// [`crate::event::UiEvent::ClipboardPaste`]/`TextCopied`, not the
    /// accelerator alone (see `accelerator.rs`'s module doc on that
    /// event split), so they're not represented as an `EditOp` — callers
    /// wire `Cut`/`Copy` through [`TextInput::selected_text`] plus
    /// (for `Cut`) [`EditOp::DeleteSelection`], and `Paste` through
    /// [`EditOp::InsertText`] fed by `ClipboardPaste`'s payload.
    pub fn from_key_binding(binding: &KeyBinding) -> Option<EditOp> {
        match binding {
            KeyBinding::Undo => Some(EditOp::Undo),
            KeyBinding::Redo => Some(EditOp::Redo),
            KeyBinding::SelectAll => Some(EditOp::SelectAll),
            _ => None,
        }
    }
}

/// Snapshot of the editable state — everything [`EditOp::Undo`]/
/// [`EditOp::Redo`] restore. Deliberately excludes `id`/`placeholder`/
/// `scroll_offset`/`scroll_col`/`has_focus`: those aren't buffer content,
/// so undoing an edit shouldn't also snap the viewport back.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TextInputSnapshot {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
    selection_anchor: Option<(usize, usize)>,
}

/// Byte offset of the `col`-th char boundary in `line` (`col` in char
/// columns), clamped to `line.len()` if `col` exceeds the line's char
/// count.
fn byte_offset_for_col(line: &str, col: usize) -> usize {
    line.char_indices()
        .nth(col)
        .map(|(b, _)| b)
        .unwrap_or(line.len())
}

impl TextInput {
    fn snapshot(&self) -> TextInputSnapshot {
        TextInputSnapshot {
            lines: self.lines.clone(),
            cursor_line: self.cursor_line,
            cursor_col: self.cursor_col,
            selection_anchor: self.selection_anchor,
        }
    }

    fn restore(&mut self, snap: TextInputSnapshot) {
        self.lines = snap.lines;
        self.cursor_line = snap.cursor_line;
        self.cursor_col = snap.cursor_col;
        self.selection_anchor = snap.selection_anchor;
    }

    /// Run a content-mutating closure, recording an undo snapshot first
    /// — but only keeping it if `f` actually reports a change, so a
    /// no-op edit (backspace at the very start of the buffer, delete at
    /// the very end) doesn't waste an undo step.
    fn record_and(&mut self, f: impl FnOnce(&mut Self) -> bool) -> bool {
        let snap = self.snapshot();
        let changed = f(self);
        if changed {
            self.undo_stack.record(snap);
        }
        changed
    }

    /// The selection as an ordered `(start, end)` pair of `(line, col)`
    /// positions, or `None` if no selection is active (`selection_anchor`
    /// is `None`, or equals the current cursor position).
    pub fn selection_range(&self) -> Option<((usize, usize), (usize, usize))> {
        let anchor = self.selection_anchor?;
        let cursor = (self.cursor_line, self.cursor_col);
        if anchor == cursor {
            return None;
        }
        Some(if anchor <= cursor {
            (anchor, cursor)
        } else {
            (cursor, anchor)
        })
    }

    /// The text currently selected, or `None` if no selection is active.
    /// Multi-line selections join lines with `'\n'`, mirroring how
    /// [`EditOp::InsertText`] would re-insert the same text verbatim.
    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection_range()?;
        Some(self.text_in_range(start, end))
    }

    fn text_in_range(&self, start: (usize, usize), end: (usize, usize)) -> String {
        let (sl, sc) = start;
        let (el, ec) = end;
        if sl == el {
            let line = self.lines.get(sl).map(String::as_str).unwrap_or("");
            let sb = byte_offset_for_col(line, sc);
            let eb = byte_offset_for_col(line, ec);
            return line[sb.min(eb)..sb.max(eb)].to_string();
        }
        let mut out = String::new();
        let first = self.lines.get(sl).map(String::as_str).unwrap_or("");
        out.push_str(&first[byte_offset_for_col(first, sc)..]);
        for line in &self.lines[sl + 1..el] {
            out.push('\n');
            out.push_str(line);
        }
        out.push('\n');
        let last = self.lines.get(el).map(String::as_str).unwrap_or("");
        out.push_str(&last[..byte_offset_for_col(last, ec)]);
        out
    }

    fn line_len(&self, line: usize) -> usize {
        self.lines.get(line).map_or(0, |l| l.chars().count())
    }

    fn delete_range(&mut self, start: (usize, usize), end: (usize, usize)) {
        let (sl, sc) = start;
        let (el, ec) = end;
        if sl == el {
            if let Some(line) = self.lines.get_mut(sl) {
                let sb = byte_offset_for_col(line, sc);
                let eb = byte_offset_for_col(line, ec);
                line.replace_range(sb..eb, "");
            }
        } else {
            let head = {
                let line = self.lines.get(sl).map(String::as_str).unwrap_or("");
                line[..byte_offset_for_col(line, sc)].to_string()
            };
            let tail = {
                let line = self.lines.get(el).map(String::as_str).unwrap_or("");
                line[byte_offset_for_col(line, ec)..].to_string()
            };
            self.lines.splice(sl..=el, [format!("{head}{tail}")]);
        }
        self.cursor_line = sl;
        self.cursor_col = sc;
        self.selection_anchor = None;
    }

    /// Delete the active selection, if any. Returns whether anything was
    /// deleted. Does **not** record undo itself — callers wrap this in
    /// [`Self::record_and`].
    fn delete_selection_raw(&mut self) -> bool {
        match self.selection_range() {
            Some((start, end)) => {
                self.delete_range(start, end);
                true
            }
            None => false,
        }
    }

    fn insert_char_raw(&mut self, ch: char) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        if ch == '\n' {
            let line = self.lines[self.cursor_line].as_str();
            let byte = byte_offset_for_col(line, self.cursor_col);
            let rest = self.lines[self.cursor_line][byte..].to_string();
            self.lines[self.cursor_line].truncate(byte);
            self.lines.insert(self.cursor_line + 1, rest);
            self.cursor_line += 1;
            self.cursor_col = 0;
        } else {
            let line = &mut self.lines[self.cursor_line];
            let byte = byte_offset_for_col(line, self.cursor_col);
            line.insert(byte, ch);
            self.cursor_col += 1;
        }
    }

    fn insert_text_raw(&mut self, text: &str) {
        for ch in text.chars() {
            self.insert_char_raw(ch);
        }
    }

    /// Replace the selection (if any) then insert `text`. Shared by
    /// [`EditOp::InsertChar`]/[`EditOp::InsertText`].
    fn replace_selection_and_insert(&mut self, text: &str) -> bool {
        self.delete_selection_raw();
        self.insert_text_raw(text);
        true
    }

    fn delete_backward_raw(&mut self) -> bool {
        if self.cursor_col > 0 {
            let line = &mut self.lines[self.cursor_line];
            let end = byte_offset_for_col(line, self.cursor_col);
            let start = byte_offset_for_col(line, self.cursor_col - 1);
            line.replace_range(start..end, "");
            self.cursor_col -= 1;
            true
        } else if self.cursor_line > 0 {
            let current = self.lines.remove(self.cursor_line);
            self.cursor_line -= 1;
            let prev_len = self.line_len(self.cursor_line);
            self.lines[self.cursor_line].push_str(&current);
            self.cursor_col = prev_len;
            true
        } else {
            false
        }
    }

    fn delete_forward_raw(&mut self) -> bool {
        let len = self.line_len(self.cursor_line);
        if self.cursor_col < len {
            let line = &mut self.lines[self.cursor_line];
            let start = byte_offset_for_col(line, self.cursor_col);
            let end = byte_offset_for_col(line, self.cursor_col + 1);
            line.replace_range(start..end, "");
            true
        } else if self.cursor_line + 1 < self.lines.len() {
            let next = self.lines.remove(self.cursor_line + 1);
            self.lines[self.cursor_line].push_str(&next);
            true
        } else {
            false
        }
    }

    /// Move the cursor to `pos`, updating (or clearing) the selection
    /// anchor per `extend`. Returns whether the cursor actually moved.
    fn move_cursor_to(&mut self, pos: (usize, usize), extend: bool) -> bool {
        let before = (self.cursor_line, self.cursor_col);
        let anchor_before = self.selection_anchor;
        if extend {
            if self.selection_anchor.is_none() {
                self.selection_anchor = Some(before);
            }
        } else {
            self.selection_anchor = None;
        }
        self.cursor_line = pos.0;
        self.cursor_col = pos.1;
        before != pos || self.selection_anchor != anchor_before
    }

    fn clamp_pos(&self, line: usize, col: usize) -> (usize, usize) {
        let line = line.min(self.lines.len().saturating_sub(1));
        let col = col.min(self.line_len(line));
        (line, col)
    }

    /// Apply one [`EditOp`], mutating this `TextInput` in place. Returns
    /// whether anything actually changed (useful for deciding whether a
    /// redraw is needed).
    pub fn apply(&mut self, op: EditOp) -> bool {
        match op {
            EditOp::InsertChar(ch) => self.record_and(|s| {
                s.delete_selection_raw();
                s.insert_char_raw(ch);
                true
            }),
            EditOp::InsertText(text) => self.record_and(|s| s.replace_selection_and_insert(&text)),
            EditOp::DeleteBackward => self.record_and(|s| {
                if s.delete_selection_raw() {
                    true
                } else {
                    s.delete_backward_raw()
                }
            }),
            EditOp::DeleteForward => self.record_and(|s| {
                if s.delete_selection_raw() {
                    true
                } else {
                    s.delete_forward_raw()
                }
            }),
            EditOp::DeleteSelection => self.record_and(|s| s.delete_selection_raw()),
            EditOp::MoveLeft { extend } => {
                let (line, col) = (self.cursor_line, self.cursor_col);
                let target = if col > 0 {
                    (line, col - 1)
                } else if line > 0 {
                    (line - 1, self.line_len(line - 1))
                } else {
                    (line, col)
                };
                self.move_cursor_to(target, extend)
            }
            EditOp::MoveRight { extend } => {
                let (line, col) = (self.cursor_line, self.cursor_col);
                let len = self.line_len(line);
                let target = if col < len {
                    (line, col + 1)
                } else if line + 1 < self.lines.len() {
                    (line + 1, 0)
                } else {
                    (line, col)
                };
                self.move_cursor_to(target, extend)
            }
            EditOp::MoveUp { extend } => {
                let target = if self.cursor_line > 0 {
                    self.clamp_pos(self.cursor_line - 1, self.cursor_col)
                } else {
                    (self.cursor_line, self.cursor_col)
                };
                self.move_cursor_to(target, extend)
            }
            EditOp::MoveDown { extend } => {
                let target = if self.cursor_line + 1 < self.lines.len() {
                    self.clamp_pos(self.cursor_line + 1, self.cursor_col)
                } else {
                    (self.cursor_line, self.cursor_col)
                };
                self.move_cursor_to(target, extend)
            }
            EditOp::MoveLineStart { extend } => self.move_cursor_to((self.cursor_line, 0), extend),
            EditOp::MoveLineEnd { extend } => {
                let len = self.line_len(self.cursor_line);
                self.move_cursor_to((self.cursor_line, len), extend)
            }
            EditOp::MoveDocStart { extend } => self.move_cursor_to((0, 0), extend),
            EditOp::MoveDocEnd { extend } => {
                let last = self.lines.len().saturating_sub(1);
                let len = self.line_len(last);
                self.move_cursor_to((last, len), extend)
            }
            EditOp::SetCursor { line, col, extend } => {
                let target = self.clamp_pos(line, col);
                self.move_cursor_to(target, extend)
            }
            EditOp::SelectAll => {
                let last = self.lines.len().saturating_sub(1);
                let end = (last, self.line_len(last));
                if end == (0, 0) {
                    false
                } else {
                    self.selection_anchor = Some((0, 0));
                    self.cursor_line = end.0;
                    self.cursor_col = end.1;
                    true
                }
            }
            EditOp::Undo => {
                let current = self.snapshot();
                match self.undo_stack.undo(current) {
                    Some(prev) => {
                        self.restore(prev);
                        true
                    }
                    None => false,
                }
            }
            EditOp::Redo => {
                let current = self.snapshot();
                match self.undo_stack.redo(current) {
                    Some(next) => {
                        self.restore(next);
                        true
                    }
                    None => false,
                }
            }
        }
    }
}

/// Hit-test classification for clicks inside a `TextInput`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextInputHit {
    /// Click landed on a text line — the consumer maps `(line, col)`
    /// to a new cursor position.
    Line { line_idx: usize },
    /// Click landed in the content area but past the last line.
    EmptyArea,
}

/// One visible line resolved by layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleTextInputLine {
    /// Index into [`TextInput::lines`].
    pub line_idx: usize,
    pub bounds: Rect,
}

/// Fully-resolved layout.
#[derive(Debug, Clone, PartialEq)]
pub struct TextInputLayout {
    /// Outer bounds (matches the `rect` argument to layout).
    pub bounds: Rect,
    /// Content bounds (inside any border/padding the rasteriser draws).
    pub content_bounds: Rect,
    pub visible_lines: Vec<VisibleTextInputLine>,
    /// Cursor bounds in viewport pixels/cells when the cursor is inside
    /// the visible window, otherwise `None`. Rasterisers paint a cursor
    /// glyph (TUI) or vertical bar (GTK) at this rect.
    pub cursor_bounds: Option<Rect>,
    /// Vertical scroll offset after auto-clamp. The primitive
    /// guarantees the cursor is visible by adjusting this.
    pub resolved_scroll_offset: usize,
    /// Horizontal scroll offset (char columns) after auto-clamp.
    /// Rasterisers slice each line `[resolved_scroll_col..]` before
    /// painting.
    pub resolved_scroll_col: usize,
    /// Per-region hit map for click routing.
    pub hit_regions: Vec<(Rect, TextInputHit)>,
    /// True when the placeholder was rendered (lines empty / single
    /// empty line). Rasterisers consult this to draw the placeholder
    /// in a muted color.
    pub placeholder_active: bool,
}

impl TextInputLayout {
    /// Hit-test a click at surface-native coordinates `(x, y)` against
    /// this layout's `hit_regions`.
    ///
    /// Coordinate frame: **ABSOLUTE** — matches [`TextInputLayout`]'s own
    /// convention (`bounds`/`content_bounds`/`hit_regions` all carry
    /// `rect.x`/`rect.y`, per `PRIMITIVE_RULES.md`'s coordinate-frame
    /// table). Returns [`TextInputHit::EmptyArea`] when `(x, y)` falls
    /// outside every region (quadraui#818).
    pub fn hit_test(&self, x: f32, y: f32) -> TextInputHit {
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        TextInputHit::EmptyArea
    }
}

/// Per-line measurement supplied by the backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextInputMeasure {
    /// Height of one row in surface-native units (TUI: 1.0 cells;
    /// GTK: line_height pixels).
    pub row_height: f32,
    /// Width of one character column. TUI: 1.0 (cells). GTK: the
    /// monospace `char_width` from the backend — used to position the
    /// cursor and route clicks back to columns.
    pub char_width: f32,
}

impl TextInputMeasure {
    pub fn new(row_height: f32, char_width: f32) -> Self {
        Self {
            row_height,
            char_width,
        }
    }

    /// Build from the backend's own [`crate::backend::Metrics`]
    /// (`backend.measure()`) instead of hand-threading
    /// `backend.line_height()` / `backend.char_width()` through
    /// [`Self::new`] — the two are identical, this just names the
    /// intent (quadraui#817).
    pub fn from_metrics(m: &crate::backend::Metrics) -> Self {
        Self::new(m.line_height, m.char_width)
    }
}

impl TextInput {
    /// Compute layout for `rect`. `measure` supplies row height + char
    /// width in the backend's native units.
    pub fn layout(&self, rect: Rect, measure: TextInputMeasure) -> TextInputLayout {
        let row_h = measure.row_height.max(1.0);
        let char_w = measure.char_width.max(1.0);

        // Border (1 cell / 1 px) + horizontal padding (1 char column).
        // Vertical padding is zero so each row neatly fills `row_h`.
        let border = 1.0;
        let pad_x = char_w;
        let content_x = rect.x + border + pad_x;
        let content_y = rect.y + border;
        let content_w = (rect.width - (border + pad_x) * 2.0).max(0.0);
        let content_h = (rect.height - border * 2.0).max(0.0);
        let content_bounds = Rect::new(content_x, content_y, content_w, content_h);

        let total_lines = self.lines.len().max(1);
        let max_rows = ((content_h / row_h).floor() as usize).max(1);
        let visible_cols = ((content_w / char_w).floor() as usize).max(1);

        // Clamp cursor line/col to text bounds.
        let cursor_line = self.cursor_line.min(total_lines.saturating_sub(1));
        let cursor_col = {
            let line = self.lines.get(cursor_line).map_or("", String::as_str);
            self.cursor_col.min(line.chars().count())
        };

        // Vertical scroll auto-clamp.
        let max_scroll = total_lines.saturating_sub(max_rows);
        let mut scroll = self.scroll_offset.min(max_scroll);
        if cursor_line < scroll {
            scroll = cursor_line;
        } else if cursor_line >= scroll + max_rows {
            scroll = cursor_line + 1 - max_rows;
        }
        scroll = scroll.min(max_scroll);

        // Horizontal scroll auto-clamp — keep cursor inside [scroll_col,
        // scroll_col + visible_cols). Reserve one column of slack so the
        // cursor itself stays visible (not flush against the right edge).
        let mut scroll_col = self.scroll_col;
        if cursor_col < scroll_col {
            scroll_col = cursor_col;
        } else if cursor_col >= scroll_col + visible_cols {
            scroll_col = cursor_col + 1 - visible_cols;
        }

        let visible_count = total_lines.saturating_sub(scroll).min(max_rows);
        let mut visible_lines: Vec<VisibleTextInputLine> = Vec::with_capacity(visible_count);
        let mut hit_regions: Vec<(Rect, TextInputHit)> = Vec::with_capacity(visible_count + 1);

        let placeholder_active = self.placeholder.is_some()
            && (self.lines.is_empty() || (self.lines.len() == 1 && self.lines[0].is_empty()));

        for i in 0..visible_count {
            let line_idx = scroll + i;
            let row_y = content_y + i as f32 * row_h;
            let row_bounds = Rect::new(content_x, row_y, content_w, row_h);
            visible_lines.push(VisibleTextInputLine {
                line_idx,
                bounds: row_bounds,
            });
            hit_regions.push((row_bounds, TextInputHit::Line { line_idx }));
        }

        // Empty-area hit zone: any vertical space below the last
        // visible line.
        let used_h = visible_count as f32 * row_h;
        if used_h < content_h {
            hit_regions.push((
                Rect::new(content_x, content_y + used_h, content_w, content_h - used_h),
                TextInputHit::EmptyArea,
            ));
        }

        // Cursor bounds — only when in the visible window.
        let cursor_bounds = if cursor_line >= scroll && cursor_line < scroll + visible_count {
            let row_off = cursor_line - scroll;
            let col_off = cursor_col.saturating_sub(scroll_col);
            let cursor_x = content_x + col_off as f32 * char_w;
            let cursor_y = content_y + row_off as f32 * row_h;
            Some(Rect::new(cursor_x, cursor_y, char_w, row_h))
        } else {
            None
        };

        TextInputLayout {
            bounds: rect,
            content_bounds,
            visible_lines,
            cursor_bounds,
            resolved_scroll_offset: scroll,
            resolved_scroll_col: scroll_col,
            hit_regions,
            placeholder_active,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(lines: Vec<&str>) -> TextInput {
        let mut ti = TextInput::new(WidgetId::new("ti"));
        ti.lines = lines.into_iter().map(String::from).collect();
        ti.has_focus = true;
        ti
    }

    fn measure() -> TextInputMeasure {
        TextInputMeasure::new(1.0, 1.0)
    }

    fn rect(w: f32, h: f32) -> Rect {
        Rect::new(0.0, 0.0, w, h)
    }

    #[test]
    fn empty_input_renders_one_row() {
        let ti = TextInput::new(WidgetId::new("ti"));
        let l = ti.layout(rect(20.0, 10.0), measure());
        assert_eq!(l.visible_lines.len(), 1);
        assert_eq!(l.visible_lines[0].line_idx, 0);
    }

    #[test]
    fn placeholder_active_when_empty() {
        let mut ti = TextInput::new(WidgetId::new("ti"));
        ti.placeholder = Some("type here".into());
        let l = ti.layout(rect(20.0, 10.0), measure());
        assert!(l.placeholder_active);
    }

    #[test]
    fn placeholder_inactive_with_content() {
        let mut ti = input(vec!["hello"]);
        ti.placeholder = Some("type here".into());
        let l = ti.layout(rect(20.0, 10.0), measure());
        assert!(!l.placeholder_active);
    }

    #[test]
    fn cursor_bounds_at_origin_when_empty() {
        let ti = TextInput::new(WidgetId::new("ti"));
        let l = ti.layout(rect(20.0, 10.0), measure());
        let cb = l.cursor_bounds.unwrap();
        // Border (1) + horizontal padding (1 char_width) -> content starts at x=2.
        // Vertical padding is zero so content_y = border = 1.
        assert_eq!(cb.x, 2.0);
        assert_eq!(cb.y, 1.0);
        assert_eq!(cb.width, 1.0);
        assert_eq!(cb.height, 1.0);
    }

    #[test]
    fn cursor_bounds_track_col() {
        let mut ti = input(vec!["hello"]);
        ti.cursor_col = 3;
        let l = ti.layout(rect(20.0, 10.0), measure());
        assert_eq!(l.cursor_bounds.unwrap().x, 2.0 + 3.0); // content_x + col
    }

    #[test]
    fn cursor_col_clamped_to_line_length() {
        let mut ti = input(vec!["hi"]);
        ti.cursor_col = 99;
        let l = ti.layout(rect(20.0, 10.0), measure());
        assert_eq!(l.cursor_bounds.unwrap().x, 2.0 + 2.0); // clamped to 2
    }

    #[test]
    fn cursor_line_clamped_to_total() {
        let mut ti = input(vec!["a", "b", "c"]);
        ti.cursor_line = 99;
        let l = ti.layout(rect(20.0, 10.0), measure());
        assert!(l.cursor_bounds.is_some());
        // Cursor lands on line index 2 (last available).
        let cb = l.cursor_bounds.unwrap();
        assert_eq!(cb.y, 1.0 + 2.0); // border + 2 rows
    }

    // ── Horizontal scroll ────────────────────────────────────────────

    #[test]
    fn h_scroll_auto_pulls_cursor_into_view_rightward() {
        // Wide line, narrow viewport.
        let mut ti = input(vec!["abcdefghijklmnopqrstuvwxyz"]);
        // content_w = 10 - 2(border) - 2(pad) = 6 -> visible_cols = 6.
        ti.cursor_col = 20;
        let l = ti.layout(rect(10.0, 5.0), measure());
        // Cursor should be visible: scroll_col = 20 + 1 - 6 = 15.
        assert_eq!(l.resolved_scroll_col, 15);
        // Cursor x = content_x + (cursor_col - scroll_col) * char_w = 2 + 5 = 7.
        assert_eq!(l.cursor_bounds.unwrap().x, 7.0);
    }

    #[test]
    fn h_scroll_auto_pulls_cursor_into_view_leftward() {
        let mut ti = input(vec!["abcdefghijklmnopqrstuvwxyz"]);
        ti.cursor_col = 2;
        ti.scroll_col = 20; // stale — cursor is left of view
        let l = ti.layout(rect(10.0, 5.0), measure());
        assert_eq!(l.resolved_scroll_col, 2);
    }

    #[test]
    fn h_scroll_zero_when_line_fits() {
        let mut ti = input(vec!["short"]);
        ti.cursor_col = 5;
        let l = ti.layout(rect(20.0, 5.0), measure());
        assert_eq!(l.resolved_scroll_col, 0);
    }

    #[test]
    fn scroll_auto_pulls_cursor_into_view_downward() {
        // 10 lines, viewport fits 3, cursor on line 7.
        let mut ti = input(vec!["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"]);
        ti.cursor_line = 7;
        // content height = 5 - 2 (border) = 3, max_rows = 3.
        let l = ti.layout(rect(20.0, 5.0), measure());
        // Cursor should be visible: scroll = 7 + 1 - 3 = 5
        assert_eq!(l.resolved_scroll_offset, 5);
        assert!(l.cursor_bounds.is_some());
    }

    #[test]
    fn scroll_auto_pulls_cursor_into_view_upward() {
        let mut ti = input(vec!["0", "1", "2", "3", "4", "5"]);
        ti.cursor_line = 1;
        ti.scroll_offset = 4; // stale — cursor is above viewport
        let l = ti.layout(rect(20.0, 5.0), measure());
        assert_eq!(l.resolved_scroll_offset, 1);
    }

    #[test]
    fn scroll_clamped_when_cursor_in_view() {
        let mut ti = input(vec!["0", "1", "2", "3"]);
        ti.cursor_line = 0;
        ti.scroll_offset = 99; // wildly stale
        let l = ti.layout(rect(20.0, 10.0), measure());
        // max_scroll = 4 - (10-2)/1 = 4 - 8 = 0, so clamp to 0
        assert_eq!(l.resolved_scroll_offset, 0);
    }

    #[test]
    fn hit_regions_one_per_visible_line() {
        let ti = input(vec!["a", "b", "c"]);
        let l = ti.layout(rect(20.0, 10.0), measure());
        let line_hits: Vec<_> = l
            .hit_regions
            .iter()
            .filter(|(_, h)| matches!(h, TextInputHit::Line { .. }))
            .collect();
        assert_eq!(line_hits.len(), 3);
    }

    #[test]
    fn empty_area_hit_region_below_last_line() {
        let ti = input(vec!["only"]);
        let l = ti.layout(rect(20.0, 10.0), measure());
        let empty = l
            .hit_regions
            .iter()
            .find(|(_, h)| matches!(h, TextInputHit::EmptyArea));
        assert!(empty.is_some());
    }

    #[test]
    fn hit_test_line_returns_line_idx() {
        let ti = input(vec!["a", "b", "c"]);
        let l = ti.layout(rect(20.0, 10.0), measure());
        // Row 1 (0-indexed) is line_idx 1, at content_y=1.0 + 1*row_h=1.0.
        assert_eq!(l.hit_test(5.0, 2.0), TextInputHit::Line { line_idx: 1 });
    }

    #[test]
    fn hit_test_below_last_line_returns_empty_area() {
        let ti = input(vec!["only"]);
        let l = ti.layout(rect(20.0, 10.0), measure());
        assert_eq!(l.hit_test(5.0, 8.0), TextInputHit::EmptyArea);
    }

    #[test]
    fn hit_test_outside_bounds_returns_empty_area() {
        let ti = input(vec!["only"]);
        let l = ti.layout(rect(20.0, 10.0), measure());
        assert_eq!(l.hit_test(-5.0, -5.0), TextInputHit::EmptyArea);
    }

    #[test]
    fn cursor_bounds_none_when_off_screen() {
        // Force scroll_offset stale by manipulating cursor_line.
        // Build 10 lines, cursor on 0, but max_rows shows 3 — should
        // auto-scroll to keep cursor in view (so this is harder to test).
        // Test the literal off-screen case: cursor_line clamped to 2,
        // scroll forced to 0, max_rows = 1.
        let mut ti = input(vec!["a", "b", "c"]);
        ti.cursor_line = 2;
        ti.scroll_offset = 0;
        let l = ti.layout(rect(20.0, 3.0), measure()); // content_h = 1, max_rows = 1
                                                       // Auto-scroll keeps cursor in view, so it's still visible:
        assert!(l.cursor_bounds.is_some());
        assert_eq!(l.resolved_scroll_offset, 2);
    }

    // ── apply(EditOp) — insert / delete / cursor / selection / undo ─────

    #[test]
    fn insert_char_advances_cursor() {
        let mut ti = TextInput::new(WidgetId::new("ti"));
        assert!(ti.apply(EditOp::InsertChar('h')));
        assert!(ti.apply(EditOp::InsertChar('i')));
        assert_eq!(ti.lines, vec!["hi".to_string()]);
        assert_eq!((ti.cursor_line, ti.cursor_col), (0, 2));
    }

    #[test]
    fn insert_char_newline_splits_line() {
        let mut ti = input(vec!["helloworld"]);
        ti.cursor_col = 5;
        assert!(ti.apply(EditOp::InsertChar('\n')));
        assert_eq!(ti.lines, vec!["hello".to_string(), "world".to_string()]);
        assert_eq!((ti.cursor_line, ti.cursor_col), (1, 0));
    }

    #[test]
    fn insert_char_is_multibyte_safe() {
        // Cursor sits after 'h' + é (a 2-byte char) — inserting must not
        // panic slicing mid-char, and must land after the é (char column
        // 2), not the byte offset.
        let mut ti = input(vec!["héllo"]);
        ti.cursor_col = 2; // after 'h', 'é'
        assert!(ti.apply(EditOp::InsertChar('X')));
        assert_eq!(ti.lines, vec!["héXllo".to_string()]);
    }

    #[test]
    fn insert_text_with_embedded_newline_creates_lines() {
        let mut ti = TextInput::new(WidgetId::new("ti"));
        assert!(ti.apply(EditOp::InsertText("ab\ncd".to_string())));
        assert_eq!(ti.lines, vec!["ab".to_string(), "cd".to_string()]);
        assert_eq!((ti.cursor_line, ti.cursor_col), (1, 2));
    }

    #[test]
    fn delete_backward_removes_prior_char() {
        let mut ti = input(vec!["hi"]);
        ti.cursor_col = 2;
        assert!(ti.apply(EditOp::DeleteBackward));
        assert_eq!(ti.lines, vec!["h".to_string()]);
        assert_eq!(ti.cursor_col, 1);
    }

    #[test]
    fn delete_backward_at_line_start_merges_with_previous_line() {
        let mut ti = input(vec!["foo", "bar"]);
        ti.cursor_line = 1;
        ti.cursor_col = 0;
        assert!(ti.apply(EditOp::DeleteBackward));
        assert_eq!(ti.lines, vec!["foobar".to_string()]);
        assert_eq!((ti.cursor_line, ti.cursor_col), (0, 3));
    }

    #[test]
    fn delete_backward_at_doc_start_is_a_no_op() {
        let mut ti = input(vec!["hi"]);
        assert!(!ti.apply(EditOp::DeleteBackward));
        assert_eq!(ti.lines, vec!["hi".to_string()]);
    }

    #[test]
    fn delete_forward_removes_next_char() {
        let mut ti = input(vec!["hi"]);
        assert!(ti.apply(EditOp::DeleteForward));
        assert_eq!(ti.lines, vec!["i".to_string()]);
        assert_eq!(ti.cursor_col, 0);
    }

    #[test]
    fn delete_forward_at_line_end_merges_next_line() {
        let mut ti = input(vec!["foo", "bar"]);
        ti.cursor_col = 3; // end of "foo"
        assert!(ti.apply(EditOp::DeleteForward));
        assert_eq!(ti.lines, vec!["foobar".to_string()]);
    }

    #[test]
    fn delete_forward_at_doc_end_is_a_no_op() {
        let mut ti = input(vec!["hi"]);
        ti.cursor_col = 2;
        assert!(!ti.apply(EditOp::DeleteForward));
    }

    #[test]
    fn move_right_then_left_round_trips() {
        let mut ti = input(vec!["hi"]);
        assert!(ti.apply(EditOp::MoveRight { extend: false }));
        assert_eq!(ti.cursor_col, 1);
        assert!(ti.apply(EditOp::MoveLeft { extend: false }));
        assert_eq!(ti.cursor_col, 0);
    }

    #[test]
    fn move_right_at_line_end_wraps_to_next_line() {
        let mut ti = input(vec!["ab", "cd"]);
        ti.cursor_col = 2;
        assert!(ti.apply(EditOp::MoveRight { extend: false }));
        assert_eq!((ti.cursor_line, ti.cursor_col), (1, 0));
    }

    #[test]
    fn move_left_at_line_start_wraps_to_previous_line_end() {
        let mut ti = input(vec!["ab", "cd"]);
        ti.cursor_line = 1;
        assert!(ti.apply(EditOp::MoveLeft { extend: false }));
        assert_eq!((ti.cursor_line, ti.cursor_col), (0, 2));
    }

    #[test]
    fn move_up_down_clamp_column_to_shorter_line() {
        let mut ti = input(vec!["abcdef", "xy"]);
        ti.cursor_col = 5;
        assert!(ti.apply(EditOp::MoveDown { extend: false }));
        assert_eq!((ti.cursor_line, ti.cursor_col), (1, 2)); // clamped to "xy"'s length
        assert!(ti.apply(EditOp::MoveUp { extend: false }));
        assert_eq!((ti.cursor_line, ti.cursor_col), (0, 2)); // clamped col preserved
    }

    #[test]
    fn move_line_start_end_and_doc_start_end() {
        let mut ti = input(vec!["abc", "de"]);
        ti.cursor_line = 1;
        ti.cursor_col = 1;
        assert!(ti.apply(EditOp::MoveLineEnd { extend: false }));
        assert_eq!(ti.cursor_col, 2);
        assert!(ti.apply(EditOp::MoveLineStart { extend: false }));
        assert_eq!(ti.cursor_col, 0);
        assert!(ti.apply(EditOp::MoveDocEnd { extend: false }));
        assert_eq!((ti.cursor_line, ti.cursor_col), (1, 2));
        assert!(ti.apply(EditOp::MoveDocStart { extend: false }));
        assert_eq!((ti.cursor_line, ti.cursor_col), (0, 0));
    }

    #[test]
    fn set_cursor_clamps_out_of_range_position() {
        let mut ti = input(vec!["abc"]);
        assert!(ti.apply(EditOp::SetCursor {
            line: 99,
            col: 99,
            extend: false
        }));
        assert_eq!((ti.cursor_line, ti.cursor_col), (0, 3));
    }

    // ── selection ─────────────────────────────────────────────────────

    #[test]
    fn extend_move_creates_selection_and_selected_text() {
        let mut ti = input(vec!["hello"]);
        ti.apply(EditOp::MoveRight { extend: true });
        ti.apply(EditOp::MoveRight { extend: true });
        assert_eq!(ti.selection_range(), Some(((0, 0), (0, 2))));
        assert_eq!(ti.selected_text().as_deref(), Some("he"));
    }

    #[test]
    fn plain_move_after_extend_collapses_selection() {
        let mut ti = input(vec!["hello"]);
        ti.apply(EditOp::MoveRight { extend: true });
        ti.apply(EditOp::MoveRight { extend: false });
        assert_eq!(ti.selection_range(), None);
    }

    #[test]
    fn select_all_selects_entire_buffer() {
        let mut ti = input(vec!["ab", "cde"]);
        assert!(ti.apply(EditOp::SelectAll));
        assert_eq!(ti.selected_text().as_deref(), Some("ab\ncde"));
    }

    #[test]
    fn insert_over_selection_replaces_it() {
        let mut ti = input(vec!["hello"]);
        ti.apply(EditOp::MoveRight { extend: true });
        ti.apply(EditOp::MoveRight { extend: true }); // selects "he"
        assert!(ti.apply(EditOp::InsertChar('X')));
        assert_eq!(ti.lines, vec!["Xllo".to_string()]);
        assert_eq!(ti.selection_range(), None);
    }

    #[test]
    fn delete_backward_over_selection_deletes_selection_not_one_char() {
        let mut ti = input(vec!["hello"]);
        ti.apply(EditOp::MoveRight { extend: true });
        ti.apply(EditOp::MoveRight { extend: true }); // selects "he"
        assert!(ti.apply(EditOp::DeleteBackward));
        assert_eq!(ti.lines, vec!["llo".to_string()]);
    }

    #[test]
    fn delete_selection_op_clears_selection_with_no_replacement() {
        let mut ti = input(vec!["hello"]);
        ti.apply(EditOp::MoveRight { extend: true });
        ti.apply(EditOp::MoveRight { extend: true });
        assert!(ti.apply(EditOp::DeleteSelection));
        assert_eq!(ti.lines, vec!["llo".to_string()]);
        assert_eq!(ti.selection_range(), None);
    }

    #[test]
    fn delete_selection_with_no_selection_is_a_no_op() {
        let mut ti = input(vec!["hello"]);
        assert!(!ti.apply(EditOp::DeleteSelection));
    }

    #[test]
    fn multiline_selection_spans_lines() {
        let mut ti = input(vec!["abc", "def"]);
        ti.apply(EditOp::MoveDown { extend: true });
        ti.apply(EditOp::MoveLineEnd { extend: true });
        assert_eq!(ti.selected_text().as_deref(), Some("abc\ndef"));
    }

    // ── undo / redo ───────────────────────────────────────────────────

    #[test]
    fn undo_reverts_last_insert() {
        let mut ti = TextInput::new(WidgetId::new("ti"));
        ti.apply(EditOp::InsertChar('h'));
        ti.apply(EditOp::InsertChar('i'));
        assert_eq!(ti.lines, vec!["hi".to_string()]);
        assert!(ti.apply(EditOp::Undo));
        assert_eq!(ti.lines, vec!["h".to_string()]);
        assert!(ti.apply(EditOp::Undo));
        assert_eq!(ti.lines, vec!["".to_string()]);
    }

    #[test]
    fn undo_with_no_history_is_a_no_op() {
        let mut ti = TextInput::new(WidgetId::new("ti"));
        assert!(!ti.apply(EditOp::Undo));
    }

    #[test]
    fn redo_restores_undone_edit() {
        let mut ti = TextInput::new(WidgetId::new("ti"));
        ti.apply(EditOp::InsertChar('h'));
        ti.apply(EditOp::Undo);
        assert_eq!(ti.lines, vec!["".to_string()]);
        assert!(ti.apply(EditOp::Redo));
        assert_eq!(ti.lines, vec!["h".to_string()]);
    }

    #[test]
    fn new_edit_after_undo_clears_redo_history() {
        let mut ti = TextInput::new(WidgetId::new("ti"));
        ti.apply(EditOp::InsertChar('a'));
        ti.apply(EditOp::Undo);
        ti.apply(EditOp::InsertChar('b'));
        assert!(
            !ti.apply(EditOp::Redo),
            "redo history should have been invalidated by the new edit"
        );
        assert_eq!(ti.lines, vec!["b".to_string()]);
    }

    #[test]
    fn cursor_movement_does_not_create_an_undo_step() {
        // Typing, then moving, then undoing should undo the typing — not
        // a no-op "undo the move" step.
        let mut ti = TextInput::new(WidgetId::new("ti"));
        ti.apply(EditOp::InsertChar('a'));
        ti.apply(EditOp::MoveLeft { extend: false });
        assert!(ti.apply(EditOp::Undo));
        assert_eq!(ti.lines, vec!["".to_string()]);
    }

    #[test]
    fn undo_restores_selection_state() {
        let mut ti = input(vec!["hello"]);
        ti.apply(EditOp::MoveRight { extend: true });
        ti.apply(EditOp::MoveRight { extend: true }); // selects "he"
        ti.apply(EditOp::DeleteSelection);
        assert_eq!(ti.lines, vec!["llo".to_string()]);
        assert!(ti.apply(EditOp::Undo));
        assert_eq!(ti.lines, vec!["hello".to_string()]);
        assert_eq!(ti.selection_range(), Some(((0, 0), (0, 2))));
    }

    // ── EditOp::from_key / from_key_binding ──────────────────────────

    #[test]
    fn from_key_maps_plain_char_to_insert() {
        assert_eq!(
            EditOp::from_key(&Key::Char('x'), Modifiers::default()),
            Some(EditOp::InsertChar('x'))
        );
    }

    #[test]
    fn from_key_maps_arrows_with_shift_to_extend() {
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        assert_eq!(
            EditOp::from_key(&Key::Named(NamedKey::Right), shift),
            Some(EditOp::MoveRight { extend: true })
        );
        assert_eq!(
            EditOp::from_key(&Key::Named(NamedKey::Right), Modifiers::default()),
            Some(EditOp::MoveRight { extend: false })
        );
    }

    #[test]
    fn from_key_ctrl_home_end_map_to_doc_start_end() {
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        assert_eq!(
            EditOp::from_key(&Key::Named(NamedKey::Home), ctrl),
            Some(EditOp::MoveDocStart { extend: false })
        );
        assert_eq!(
            EditOp::from_key(&Key::Named(NamedKey::End), ctrl),
            Some(EditOp::MoveDocEnd { extend: false })
        );
        assert_eq!(
            EditOp::from_key(&Key::Named(NamedKey::Home), Modifiers::default()),
            Some(EditOp::MoveLineStart { extend: false })
        );
    }

    #[test]
    fn from_key_unmapped_key_returns_none() {
        assert_eq!(
            EditOp::from_key(&Key::Named(NamedKey::Escape), Modifiers::default()),
            None
        );
    }

    #[test]
    fn from_key_binding_wires_undo_redo_select_all() {
        assert_eq!(
            EditOp::from_key_binding(&KeyBinding::Undo),
            Some(EditOp::Undo)
        );
        assert_eq!(
            EditOp::from_key_binding(&KeyBinding::Redo),
            Some(EditOp::Redo)
        );
        assert_eq!(
            EditOp::from_key_binding(&KeyBinding::SelectAll),
            Some(EditOp::SelectAll)
        );
        assert_eq!(EditOp::from_key_binding(&KeyBinding::Copy), None);
    }

    // ── TextInput value equality ignores undo history ────────────────

    #[test]
    fn equality_ignores_undo_history() {
        let mut a = TextInput::new(WidgetId::new("ti"));
        let mut b = TextInput::new(WidgetId::new("ti"));
        // `a` reaches "x" directly; `b` reaches "x" via an extra
        // insert-then-undo round trip, leaving `b`'s redo stack non-empty
        // (unlike `a`'s). Content is identical; history is not.
        a.apply(EditOp::InsertChar('x'));
        b.apply(EditOp::InsertChar('x'));
        b.apply(EditOp::InsertChar('y'));
        b.apply(EditOp::Undo);
        assert_eq!(a.lines, b.lines);
        assert_eq!(a, b, "equality must compare content, not undo history");
    }
}
