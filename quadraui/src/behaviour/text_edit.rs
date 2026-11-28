//! `text_edit` — the one text-editing core every editable text widget
//! shares: caret movement, selection, word motion and the buffer
//! mutations an [`EditOp`] names.
//!
//! # Why one core
//!
//! [`crate::TextInput`] (multi-line), [`crate::TextField`] (single-line,
//! bracketed) and [`crate::CommandLine`] (single-line, bottom-row) all
//! need the same answers to the same questions: where does Left go when
//! the caret sits at column 0, what does Backspace do with an active
//! selection, where does Ctrl-Left stop inside `foo_bar baz`. Three
//! copies of that arithmetic would drift. [`EditView`] is the single
//! mutator all three route through, and [`EditOp`] the single intent
//! vocabulary a keystroke is mapped into (via [`EditOp::from_key`]) before
//! it reaches one.
//!
//! # Shapes
//!
//! * [`EditView`] — a *borrowed* view over `(lines, caret, anchor)`. A
//!   widget that already owns those three pieces (`TextInput` through
//!   [`crate::TextEditor`]) lends them for the duration of one
//!   [`EditView::apply`] call, so nothing is cloned per keystroke.
//! * [`apply_flat_edit_op`] — the same core against a flat `String` plus
//!   *byte* offsets, the shape `TextField`, `CommandLine` and
//!   [`crate::primitives::form::FieldKind::TextInput`] store their value
//!   in. It converts to char-column coordinates, applies one op, and
//!   converts back.
//!
//! Undo/redo needs a history that outlives a single op, which is
//! per-widget state, so [`EditOp::Undo`]/[`EditOp::Redo`] are inert in
//! [`EditView::apply`] alone. [`EditHistory::apply`] is the one place
//! they are served: it runs an op through an [`EditView`], recording a
//! pre-mutation [`EditSnapshot`] for every op that changes content
//! ([`mutates_content`]). [`crate::TextEditor`], [`LinesEditState`] and
//! [`FlatEditState`] each own one, so every editable text widget undoes
//! the same way.
//!
//! Clipboard is likewise not an [`EditOp`]: a copy or cut has to reach the
//! host (as [`crate::UiEvent::TextCopied`]) and a paste arrives from it (as
//! [`crate::UiEvent::ClipboardPaste`]), so a widget reads
//! [`EditView::selected_text`] and emits, or applies
//! [`EditOp::InsertText`] with what arrived.

use crate::accelerator::KeyBinding;
use crate::event::{Key, NamedKey};
use crate::types::Modifiers;
use crate::undo::UndoStack;

/// A caret (or selection-anchor) position: `(line, char_column)`.
///
/// Columns count **characters**, not bytes — the same space
/// [`crate::TextInput::cursor_col`] documents, so a multibyte line never
/// needs a caller to think about UTF-8 boundaries.
pub type Caret = (usize, usize);

/// An edit intent for [`EditView::apply`] — see the module doc.
///
/// Every variant that moves the caret (`Move*`, `SetCursor`) carries an
/// `extend` flag: `false` collapses any active selection and moves the
/// caret normally (a plain arrow-key press); `true` starts a selection
/// at the caret's pre-move position if none is active yet, then moves
/// only the caret (a shift-held arrow key, or a mouse drag).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditOp {
    /// Insert one character at the caret. `'\n'` splits the current
    /// line. If a selection is active, it is deleted first (so this also
    /// implements "type over the selection").
    InsertChar(char),
    /// Insert a (possibly multi-line) string at the caret — e.g. a
    /// clipboard paste. Same selection-replace behaviour as
    /// [`Self::InsertChar`].
    InsertText(String),
    /// Backspace: delete the selection if one is active, else the one
    /// character before the caret (merging with the previous line at
    /// column 0).
    DeleteBackward,
    /// Delete (forward-delete): delete the selection if one is active,
    /// else the one character after the caret (merging with the next
    /// line at end-of-line).
    DeleteForward,
    /// Delete the active selection with no replacement. A no-op (returns
    /// `false` from [`EditView::apply`]) when no selection is active —
    /// callers doing "Cut" should read [`EditView::selected_text`]
    /// first, then apply this.
    DeleteSelection,
    /// Delete the selection if one is active, else from the caret back
    /// to the previous word boundary — Ctrl-Backspace.
    DeleteWordBackward,
    /// Delete the selection if one is active, else from the caret
    /// forward to the next word boundary — Ctrl-Delete.
    DeleteWordForward,
    /// Move the caret one character left (or to the end of the previous
    /// line, if already at column 0).
    MoveLeft { extend: bool },
    /// Move the caret one character right (or to the start of the next
    /// line, if already at end-of-line).
    MoveRight { extend: bool },
    /// Move the caret up one line, clamping the column to the target
    /// line's length.
    MoveUp { extend: bool },
    /// Move the caret down one line, clamping the column to the target
    /// line's length.
    MoveDown { extend: bool },
    /// Move the caret left to the previous word boundary — Ctrl-Left.
    /// Crosses into the previous line's end when already at column 0.
    MoveWordLeft { extend: bool },
    /// Move the caret right to the next word boundary — Ctrl-Right.
    /// Crosses into the next line's start when already at end-of-line.
    MoveWordRight { extend: bool },
    /// Move the caret to column 0 of the current line.
    MoveLineStart { extend: bool },
    /// Move the caret to the end of the current line.
    MoveLineEnd { extend: bool },
    /// Move the caret to `(0, 0)`.
    MoveDocStart { extend: bool },
    /// Move the caret to the end of the last line.
    MoveDocEnd { extend: bool },
    /// Jump the caret to an arbitrary `(line, col)` — e.g. from
    /// [`crate::TextInputLayout::hit_test`] resolving a mouse click, or a
    /// drag extending a selection to the drag position.
    SetCursor {
        /// Target line index, clamped to the buffer's last line.
        line: usize,
        /// Target char column, clamped to that line's char count.
        col: usize,
        /// Whether this extends the active selection.
        extend: bool,
    },
    /// Select the entire buffer.
    SelectAll,
    /// Undo the last content-mutating op. Served by the widget's own
    /// history, not by [`EditView::apply`] — see the module doc.
    Undo,
    /// Redo the last undone op. Served by the widget's own history, not
    /// by [`EditView::apply`] — see the module doc.
    Redo,
}

impl EditOp {
    /// Map a plain keypress to the [`EditOp`] it means, for widgets
    /// wiring [`crate::event::UiEvent::KeyPressed`] straight into
    /// [`EditView::apply`]. Returns `None` for keys this core doesn't
    /// interpret (function keys, Escape, Tab, …) — callers handle those
    /// themselves (Escape to blur, Tab to move focus, etc.).
    ///
    /// `Ctrl+Left`/`Ctrl+Right` map to [`EditOp::MoveWordLeft`]/
    /// [`EditOp::MoveWordRight`] and `Ctrl+Backspace`/`Ctrl+Delete` to
    /// [`EditOp::DeleteWordBackward`]/[`EditOp::DeleteWordForward`];
    /// `Ctrl+Home`/`Ctrl+End` map to [`EditOp::MoveDocStart`]/
    /// [`EditOp::MoveDocEnd`]; plain `Home`/`End` map to
    /// [`EditOp::MoveLineStart`]/[`EditOp::MoveLineEnd`]. `modifiers.shift`
    /// becomes every movement op's `extend` flag.
    pub fn from_key(key: &Key, modifiers: Modifiers) -> Option<EditOp> {
        let extend = modifiers.shift;
        let word = modifiers.ctrl;
        match key {
            Key::Char(c) => Some(EditOp::InsertChar(*c)),
            Key::Named(NamedKey::Enter) => Some(EditOp::InsertChar('\n')),
            Key::Named(NamedKey::Backspace) if word => Some(EditOp::DeleteWordBackward),
            Key::Named(NamedKey::Backspace) => Some(EditOp::DeleteBackward),
            Key::Named(NamedKey::Delete) if word => Some(EditOp::DeleteWordForward),
            Key::Named(NamedKey::Delete) => Some(EditOp::DeleteForward),
            Key::Named(NamedKey::Left) if word => Some(EditOp::MoveWordLeft { extend }),
            Key::Named(NamedKey::Right) if word => Some(EditOp::MoveWordRight { extend }),
            Key::Named(NamedKey::Left) => Some(EditOp::MoveLeft { extend }),
            Key::Named(NamedKey::Right) => Some(EditOp::MoveRight { extend }),
            Key::Named(NamedKey::Up) => Some(EditOp::MoveUp { extend }),
            Key::Named(NamedKey::Down) => Some(EditOp::MoveDown { extend }),
            Key::Named(NamedKey::Home) if word => Some(EditOp::MoveDocStart { extend }),
            Key::Named(NamedKey::End) if word => Some(EditOp::MoveDocEnd { extend }),
            Key::Named(NamedKey::Home) => Some(EditOp::MoveLineStart { extend }),
            Key::Named(NamedKey::End) => Some(EditOp::MoveLineEnd { extend }),
            _ => None,
        }
    }

    /// Map a universal [`KeyBinding`] accelerator to the [`EditOp`] it
    /// represents — wires [`KeyBinding::Undo`]/[`KeyBinding::Redo`]/
    /// [`KeyBinding::SelectAll`] to real editing behaviour. Apps register
    /// an [`crate::accelerator::Accelerator`] with one of these bindings and,
    /// on the matching [`crate::event::UiEvent::Accelerator`], apply the
    /// result.
    ///
    /// Returns `None` for every other binding: `Copy`/`Cut`/`Paste` need
    /// clipboard content that arrives via
    /// [`crate::event::UiEvent::ClipboardPaste`]/`TextCopied`, not the
    /// accelerator alone — see the module doc.
    pub fn from_key_binding(binding: &KeyBinding) -> Option<EditOp> {
        match binding {
            KeyBinding::Undo => Some(EditOp::Undo),
            KeyBinding::Redo => Some(EditOp::Redo),
            KeyBinding::SelectAll => Some(EditOp::SelectAll),
            _ => None,
        }
    }
}

/// A keystroke's clipboard/select-all intent — the chord vocabulary
/// [`crate::TextInput`], [`crate::TextField`] and [`crate::CommandLine`]
/// all recognize the same way before falling back to [`EditOp::from_key`]
/// (which would otherwise read `Ctrl+A` as typing an `a`).
///
/// [`EditIntent::from_key`] is the one place that chord-to-intent mapping
/// lives; a widget's `handle_key` matches the result and keeps its own
/// clipboard I/O and side effects local:
/// * [`Self::SelectAll`] — apply [`EditOp::SelectAll`] through the
///   widget's own `commit`.
/// * [`Self::Copy`] — read the widget's own selected text and emit its
///   own `Copied` action; no buffer change.
/// * [`Self::Cut`] — same read, plus apply [`EditOp::DeleteSelection`].
/// * [`Self::Paste`] — always the host's: the clipboard text arrives as
///   [`crate::UiEvent::ClipboardPaste`], fed back in as a text-insert
///   event, so every widget answers this with `Handled::Ignored`.
/// * [`Self::Op`] — whatever [`EditOp::from_key`] resolved the key to,
///   for every key this vocabulary doesn't claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditIntent {
    /// Ctrl/Cmd+A.
    SelectAll,
    /// Ctrl/Cmd+C.
    Copy,
    /// Ctrl/Cmd+X.
    Cut,
    /// Ctrl/Cmd+V.
    Paste,
    /// Any other key [`EditOp::from_key`] interprets.
    Op(EditOp),
}

impl EditIntent {
    /// Map a keypress to the [`EditIntent`] it means: the Ctrl/Cmd-chorded
    /// clipboard and select-all keys first, else whatever
    /// [`EditOp::from_key`] resolves the key to (`None` for keys neither
    /// layer interprets — function keys, Escape, Tab, a bare Enter a
    /// single-line widget means to claim itself, …).
    ///
    /// Ctrl/Cmd+Z is [`EditOp::Undo`] and Ctrl/Cmd+Shift+Z
    /// [`EditOp::Redo`], served from the widget's own [`EditHistory`].
    /// Every other Ctrl/Cmd-chorded character is `None`: backends deliver
    /// a chorded letter as a plain [`Key::Char`] with the modifier set, so
    /// reading it as typing would put a literal `s` in the buffer on
    /// Ctrl+S and swallow the app's own accelerator. Ctrl+Alt without Cmd
    /// stays typing, since that is how AltGr arrives on Windows layouts.
    pub fn from_key(key: &Key, modifiers: Modifiers) -> Option<EditIntent> {
        let alt_gr = modifiers.ctrl && modifiers.alt && !modifiers.cmd;
        if (modifiers.ctrl || modifiers.cmd) && !alt_gr {
            match key {
                Key::Char('a') | Key::Char('A') => return Some(EditIntent::SelectAll),
                Key::Char('c') | Key::Char('C') => return Some(EditIntent::Copy),
                Key::Char('x') | Key::Char('X') => return Some(EditIntent::Cut),
                Key::Char('v') | Key::Char('V') => return Some(EditIntent::Paste),
                Key::Char('z') | Key::Char('Z') if modifiers.shift => {
                    return Some(EditIntent::Op(EditOp::Redo))
                }
                Key::Char('z') | Key::Char('Z') => return Some(EditIntent::Op(EditOp::Undo)),
                Key::Char(_) => return None,
                _ => {}
            }
        }
        EditOp::from_key(key, modifiers).map(EditIntent::Op)
    }
}

/// Whether `op` rewrites buffer content (as opposed to only moving the
/// caret or changing the selection).
///
/// A widget that keeps an undo history records a pre-mutation snapshot
/// exactly for these, so moving the caret between two edits doesn't split
/// them into separate undo steps.
pub fn mutates_content(op: &EditOp) -> bool {
    matches!(
        op,
        EditOp::InsertChar(_)
            | EditOp::InsertText(_)
            | EditOp::DeleteBackward
            | EditOp::DeleteForward
            | EditOp::DeleteSelection
            | EditOp::DeleteWordBackward
            | EditOp::DeleteWordForward
    )
}

/// Byte offset of the `col`-th char boundary in `line` (`col` in char
/// columns), clamped to `line.len()` if `col` exceeds the line's char
/// count.
pub fn col_to_byte(line: &str, col: usize) -> usize {
    line.char_indices()
        .nth(col)
        .map(|(b, _)| b)
        .unwrap_or(line.len())
}

/// Whether `ch` counts as part of a word for [`EditOp::MoveWordLeft`] /
/// [`EditOp::MoveWordRight`] and their delete twins.
///
/// Alphanumerics plus `_` — the identifier-shaped definition a code
/// editor's Ctrl-Left uses, so `foo_bar baz` is two words, not three.
fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// The char column the previous word boundary sits at, scanning left
/// from `col` within `line`.
///
/// Skips any run of non-word characters immediately left of the caret,
/// then the word run before it — so a caret just right of `"foo  "`
/// lands on `f`'s column, and one inside `"foo"` lands on its start.
/// Returns `0` when the scan runs off the start of the line.
pub fn word_boundary_left(line: &str, col: usize) -> usize {
    let chars: Vec<char> = line.chars().collect();
    let mut i = col.min(chars.len());
    while i > 0 && !is_word_char(chars[i - 1]) {
        i -= 1;
    }
    while i > 0 && is_word_char(chars[i - 1]) {
        i -= 1;
    }
    i
}

/// The char column the next word boundary sits at, scanning right from
/// `col` within `line` — the mirror of [`word_boundary_left`]. Returns
/// the line's char count when the scan runs off its end.
pub fn word_boundary_right(line: &str, col: usize) -> usize {
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = col.min(len);
    while i < len && !is_word_char(chars[i]) {
        i += 1;
    }
    while i < len && is_word_char(chars[i]) {
        i += 1;
    }
    i
}

/// The selection `caret`/`anchor` describe, as an ordered
/// `(start, end)` pair, or `None` when `anchor` is absent or equal to
/// `caret` — the caret is the selection's *moving* end, the anchor its
/// *fixed* end.
pub fn selection_range(caret: Caret, anchor: Option<Caret>) -> Option<(Caret, Caret)> {
    let anchor = anchor?;
    if anchor == caret {
        return None;
    }
    Some(if anchor <= caret {
        (anchor, caret)
    } else {
        (caret, anchor)
    })
}

/// Clamp a selection's line indices into `lines_len`'s bounds. A
/// (`sl`, `el`) pair can arrive pointing past the end of the current
/// buffer — the caller's position was captured against a longer
/// `lines`, and the buffer has since shrunk — so both indices are
/// pulled back to the last valid line (`0` for an empty buffer)
/// before any indexing happens. `sl <= el` on the way in is preserved
/// on the way out, since clamping is monotonic.
fn clamp_line_range(lines_len: usize, sl: usize, el: usize) -> (usize, usize) {
    let max = lines_len.saturating_sub(1);
    (sl.min(max), el.min(max))
}

/// The buffer text between two ordered positions, lines joined with
/// `'\n'` — what [`EditView::selected_text`] answers with.
pub fn text_in_range(lines: &[String], start: Caret, end: Caret) -> String {
    let (sl, sc) = start;
    let (el, ec) = end;
    let (sl, el) = clamp_line_range(lines.len(), sl, el);
    if sl == el {
        let line = lines.get(sl).map(String::as_str).unwrap_or("");
        let sb = col_to_byte(line, sc);
        let eb = col_to_byte(line, ec);
        return line[sb.min(eb)..sb.max(eb)].to_string();
    }
    let mut out = String::new();
    let first = lines.get(sl).map(String::as_str).unwrap_or("");
    out.push_str(&first[col_to_byte(first, sc)..]);
    for line in &lines[sl + 1..el] {
        out.push('\n');
        out.push_str(line);
    }
    out.push('\n');
    let last = lines.get(el).map(String::as_str).unwrap_or("");
    out.push_str(&last[..col_to_byte(last, ec)]);
    out
}

/// A borrowed view over one widget's editable state — the buffer lines,
/// the caret, and the selection anchor — carrying every [`EditOp`]
/// mutation.
///
/// Borrowed rather than owned so a widget that already stores those three
/// pieces (every editable text widget in this crate does) lends them for
/// the duration of one [`Self::apply`] call instead of cloning its buffer
/// per keystroke.
pub struct EditView<'a> {
    /// One entry per logical line; newlines are implicit between
    /// consecutive entries. An empty buffer is `vec![String::new()]`, and
    /// [`Self::apply`] restores that shape if handed a truly empty `Vec`.
    pub lines: &'a mut Vec<String>,
    /// The caret's line index.
    pub cursor_line: &'a mut usize,
    /// The caret's char column within its line.
    pub cursor_col: &'a mut usize,
    /// The selection's *fixed* end, if a selection is active. The caret
    /// is the moving end — see [`Self::selection_range`].
    pub anchor: &'a mut Option<Caret>,
}

impl EditView<'_> {
    /// The caret as one `(line, col)` pair.
    pub fn caret(&self) -> Caret {
        (*self.cursor_line, *self.cursor_col)
    }

    /// Char count of line `line`, or `0` if it doesn't exist.
    pub fn line_len(&self, line: usize) -> usize {
        self.lines.get(line).map_or(0, |l| l.chars().count())
    }

    /// `(line, col)` clamped into the buffer: `line` to the last line,
    /// `col` to that line's char count.
    pub fn clamp_pos(&self, line: usize, col: usize) -> Caret {
        let line = line.min(self.lines.len().saturating_sub(1));
        let col = col.min(self.line_len(line));
        (line, col)
    }

    /// The selection as an ordered `(start, end)` pair, or `None` if no
    /// selection is active — see [`selection_range`], which this borrows
    /// its own caret/anchor into.
    pub fn selection_range(&self) -> Option<(Caret, Caret)> {
        selection_range(self.caret(), *self.anchor)
    }

    /// The text currently selected, or `None` if no selection is active.
    /// Multi-line selections join lines with `'\n'`, mirroring how
    /// [`EditOp::InsertText`] would re-insert the same text verbatim.
    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection_range()?;
        Some(text_in_range(self.lines, start, end))
    }

    /// Apply one [`EditOp`], mutating the borrowed buffer/caret/anchor in
    /// place. Returns whether anything actually changed (useful for
    /// deciding whether a redraw is needed).
    ///
    /// [`EditOp::Undo`]/[`EditOp::Redo`] are inert here and report
    /// `false`: a history outlives one op, so it belongs to the widget
    /// that owns the buffer — see [`EditHistory::apply`].
    pub fn apply(&mut self, op: &EditOp) -> bool {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        match op {
            EditOp::InsertChar(ch) => {
                self.delete_selection();
                self.insert_char_raw(*ch);
                true
            }
            EditOp::InsertText(text) => {
                self.delete_selection();
                for ch in text.chars() {
                    self.insert_char_raw(ch);
                }
                true
            }
            EditOp::DeleteBackward => self.delete_selection() || self.delete_backward_raw(),
            EditOp::DeleteForward => self.delete_selection() || self.delete_forward_raw(),
            EditOp::DeleteSelection => self.delete_selection(),
            EditOp::DeleteWordBackward => {
                if self.delete_selection() {
                    return true;
                }
                let (line, col) = self.caret();
                if col == 0 {
                    return self.delete_backward_raw();
                }
                let text = self.lines.get(line).map(String::as_str).unwrap_or("");
                let target = word_boundary_left(text, col);
                self.delete_range((line, target), (line, col));
                target != col
            }
            EditOp::DeleteWordForward => {
                if self.delete_selection() {
                    return true;
                }
                let (line, col) = self.caret();
                let len = self.line_len(line);
                if col >= len {
                    return self.delete_forward_raw();
                }
                let text = self.lines.get(line).map(String::as_str).unwrap_or("");
                let target = word_boundary_right(text, col);
                self.delete_range((line, col), (line, target));
                target != col
            }
            EditOp::MoveLeft { extend } => {
                // An unmodified arrow press while a selection is active
                // collapses to the selection's near edge instead of also
                // taking a step from the caret's (moving-end) position —
                // otherwise a single Left press after a rightward
                // shift-selection would visually jump two characters.
                if !extend {
                    if let Some((start, _end)) = self.selection_range() {
                        return self.move_cursor_to(start, false);
                    }
                }
                let (line, col) = self.caret();
                let target = if col > 0 {
                    (line, col - 1)
                } else if line > 0 {
                    (line - 1, self.line_len(line - 1))
                } else {
                    (line, col)
                };
                self.move_cursor_to(target, *extend)
            }
            EditOp::MoveRight { extend } => {
                // See `MoveLeft`: collapses to the selection's far edge.
                if !extend {
                    if let Some((_start, end)) = self.selection_range() {
                        return self.move_cursor_to(end, false);
                    }
                }
                let (line, col) = self.caret();
                let len = self.line_len(line);
                let target = if col < len {
                    (line, col + 1)
                } else if line + 1 < self.lines.len() {
                    (line + 1, 0)
                } else {
                    (line, col)
                };
                self.move_cursor_to(target, *extend)
            }
            EditOp::MoveWordLeft { extend } => {
                let (line, col) = self.caret();
                let target = if col == 0 {
                    if line > 0 {
                        (line - 1, self.line_len(line - 1))
                    } else {
                        (line, col)
                    }
                } else {
                    let text = self.lines.get(line).map(String::as_str).unwrap_or("");
                    (line, word_boundary_left(text, col))
                };
                self.move_cursor_to(target, *extend)
            }
            EditOp::MoveWordRight { extend } => {
                let (line, col) = self.caret();
                let len = self.line_len(line);
                let target = if col >= len {
                    if line + 1 < self.lines.len() {
                        (line + 1, 0)
                    } else {
                        (line, col)
                    }
                } else {
                    let text = self.lines.get(line).map(String::as_str).unwrap_or("");
                    (line, word_boundary_right(text, col))
                };
                self.move_cursor_to(target, *extend)
            }
            EditOp::MoveUp { extend } => {
                let (line, col) = self.caret();
                let target = if line > 0 {
                    self.clamp_pos(line - 1, col)
                } else {
                    (line, col)
                };
                self.move_cursor_to(target, *extend)
            }
            EditOp::MoveDown { extend } => {
                let (line, col) = self.caret();
                let target = if line + 1 < self.lines.len() {
                    self.clamp_pos(line + 1, col)
                } else {
                    (line, col)
                };
                self.move_cursor_to(target, *extend)
            }
            EditOp::MoveLineStart { extend } => {
                self.move_cursor_to((*self.cursor_line, 0), *extend)
            }
            EditOp::MoveLineEnd { extend } => {
                let line = *self.cursor_line;
                self.move_cursor_to((line, self.line_len(line)), *extend)
            }
            EditOp::MoveDocStart { extend } => self.move_cursor_to((0, 0), *extend),
            EditOp::MoveDocEnd { extend } => {
                let last = self.lines.len().saturating_sub(1);
                let len = self.line_len(last);
                self.move_cursor_to((last, len), *extend)
            }
            EditOp::SetCursor { line, col, extend } => {
                let target = self.clamp_pos(*line, *col);
                self.move_cursor_to(target, *extend)
            }
            EditOp::SelectAll => {
                let last = self.lines.len().saturating_sub(1);
                let end = (last, self.line_len(last));
                if end == (0, 0) {
                    false
                } else {
                    *self.anchor = Some((0, 0));
                    *self.cursor_line = end.0;
                    *self.cursor_col = end.1;
                    true
                }
            }
            // Served by the widget's own history — see `EditHistory`.
            EditOp::Undo | EditOp::Redo => false,
        }
    }

    /// The buffer, caret and anchor as one restorable value.
    pub fn snapshot(&self) -> EditSnapshot {
        EditSnapshot {
            lines: self.lines.clone(),
            caret: self.caret(),
            anchor: *self.anchor,
        }
    }

    /// Replace the buffer, caret and anchor with `snapshot`'s.
    pub fn restore(&mut self, snapshot: EditSnapshot) {
        *self.lines = snapshot.lines;
        *self.cursor_line = snapshot.caret.0;
        *self.cursor_col = snapshot.caret.1;
        *self.anchor = snapshot.anchor;
    }

    /// Move the caret to `pos`, updating (or clearing) the selection
    /// anchor per `extend`. Returns whether anything actually changed.
    pub fn move_cursor_to(&mut self, pos: Caret, extend: bool) -> bool {
        let before = self.caret();
        let anchor_before = *self.anchor;
        if extend {
            if self.anchor.is_none() {
                *self.anchor = Some(before);
            }
        } else {
            *self.anchor = None;
        }
        *self.cursor_line = pos.0;
        *self.cursor_col = pos.1;
        before != pos || *self.anchor != anchor_before
    }

    /// Delete the active selection, if any, leaving the caret at its
    /// start. Returns whether anything was deleted. Records no history —
    /// that is the owning widget's job, so a delete-then-insert op pair
    /// stays one undo step.
    pub fn delete_selection(&mut self) -> bool {
        match self.selection_range() {
            Some((start, end)) => {
                self.delete_range(start, end);
                *self.anchor = None;
                true
            }
            None => false,
        }
    }

    /// Delete `start..end` and leave the caret at `start`. Clearing the
    /// selection anchor is the caller's job.
    fn delete_range(&mut self, start: Caret, end: Caret) {
        let (sl, sc) = start;
        let (el, ec) = end;
        let (sl, el) = clamp_line_range(self.lines.len(), sl, el);
        if sl == el {
            if let Some(line) = self.lines.get_mut(sl) {
                let sb = col_to_byte(line, sc);
                let eb = col_to_byte(line, ec);
                line.replace_range(sb..eb, "");
            }
        } else {
            let head = {
                let line = self.lines.get(sl).map(String::as_str).unwrap_or("");
                line[..col_to_byte(line, sc)].to_string()
            };
            let tail = {
                let line = self.lines.get(el).map(String::as_str).unwrap_or("");
                line[col_to_byte(line, ec)..].to_string()
            };
            self.lines.splice(sl..=el, [format!("{head}{tail}")]);
        }
        *self.cursor_line = sl;
        *self.cursor_col = sc;
    }

    fn insert_char_raw(&mut self, ch: char) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        let cl = (*self.cursor_line).min(self.lines.len() - 1);
        *self.cursor_line = cl;
        if ch == '\n' {
            let byte = col_to_byte(self.lines[cl].as_str(), *self.cursor_col);
            let rest = self.lines[cl][byte..].to_string();
            self.lines[cl].truncate(byte);
            self.lines.insert(cl + 1, rest);
            *self.cursor_line = cl + 1;
            *self.cursor_col = 0;
        } else {
            let col = *self.cursor_col;
            let line = &mut self.lines[cl];
            let byte = col_to_byte(line, col);
            line.insert(byte, ch);
            *self.cursor_col = col + 1;
        }
    }

    fn delete_backward_raw(&mut self) -> bool {
        let (cl, col) = self.caret();
        if col > 0 {
            let line = &mut self.lines[cl];
            let end = col_to_byte(line, col);
            let start = col_to_byte(line, col - 1);
            line.replace_range(start..end, "");
            *self.cursor_col = col - 1;
            true
        } else if cl > 0 {
            let current = self.lines.remove(cl);
            let prev = cl - 1;
            let prev_len = self.line_len(prev);
            self.lines[prev].push_str(&current);
            *self.cursor_line = prev;
            *self.cursor_col = prev_len;
            true
        } else {
            false
        }
    }

    fn delete_forward_raw(&mut self) -> bool {
        let (cl, col) = self.caret();
        let len = self.line_len(cl);
        if col < len {
            let line = &mut self.lines[cl];
            let start = col_to_byte(line, col);
            let end = col_to_byte(line, col + 1);
            line.replace_range(start..end, "");
            true
        } else if cl + 1 < self.lines.len() {
            let next = self.lines.remove(cl + 1);
            self.lines[cl].push_str(&next);
            true
        } else {
            false
        }
    }
}

// ── Flat-value bridge ──────────────────────────────────────────────────
//
// `EditView` works in `(line, char_column)` coordinates, but every flat
// `value: String` + byte-offset caret/anchor field
// (`FieldKind::TextInput`/`TextArea`/`PasswordInput`, `TextField`,
// `CommandLine`) needs the same intent applied against that shape
// instead. `apply_flat_edit_op` is the one bridge every such caller
// shares, so none of them re-derive this module's caret math or keep a
// second editor.

/// One undo step: the buffer, caret and selection anchor an
/// [`EditView`] held before an edit. Viewport fields (scroll, focus) are
/// not part of it, so undoing an edit does not snap the view back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditSnapshot {
    lines: Vec<String>,
    caret: Caret,
    anchor: Option<Caret>,
}

/// How many undo steps an [`EditHistory`] keeps. Each step clones the
/// whole buffer, so the bound keeps a long-lived input's memory a small
/// constant times its buffer size.
pub const UNDO_HISTORY_LIMIT: usize = 200;

/// A widget's undo/redo history over its own [`EditView`] — the one
/// implementation of [`EditOp::Undo`]/[`EditOp::Redo`] every editable
/// text widget shares.
///
/// ```
/// use quadraui::behaviour::text_edit::{EditHistory, EditOp, EditView};
///
/// let (mut lines, mut line, mut col, mut anchor) = (vec![String::new()], 0, 0, None);
/// let mut history = EditHistory::default();
/// let mut view = EditView {
///     lines: &mut lines,
///     cursor_line: &mut line,
///     cursor_col: &mut col,
///     anchor: &mut anchor,
/// };
/// history.apply(&mut view, &EditOp::InsertText("hi".into()));
/// history.apply(&mut view, &EditOp::Undo);
/// assert_eq!(view.lines, &vec![String::new()]);
/// history.apply(&mut view, &EditOp::Redo);
/// assert_eq!(view.lines, &vec!["hi".to_string()]);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditHistory {
    stack: UndoStack<EditSnapshot>,
}

impl Default for EditHistory {
    fn default() -> Self {
        Self {
            stack: UndoStack::with_limit(UNDO_HISTORY_LIMIT),
        }
    }
}

impl EditHistory {
    /// Whether an [`EditOp::Undo`] would change anything.
    pub fn can_undo(&self) -> bool {
        self.stack.can_undo()
    }

    /// Whether an [`EditOp::Redo`] would change anything.
    pub fn can_redo(&self) -> bool {
        self.stack.can_redo()
    }

    /// Apply `op` to `view`: [`EditOp::Undo`]/[`EditOp::Redo`] restore
    /// from this history, and every other op goes to
    /// [`EditView::apply`], with the pre-edit state recorded when the op
    /// changed the buffer. Pure caret or selection moves record nothing,
    /// so moving between two edits does not split them into separate
    /// undo steps. Returns whether anything changed.
    pub fn apply(&mut self, view: &mut EditView<'_>, op: &EditOp) -> bool {
        match op {
            EditOp::Undo => match self.stack.undo(view.snapshot()) {
                Some(prev) => {
                    view.restore(prev);
                    true
                }
                None => false,
            },
            EditOp::Redo => match self.stack.redo(view.snapshot()) {
                Some(next) => {
                    view.restore(next);
                    true
                }
                None => false,
            },
            op if mutates_content(op) => {
                let before = view.snapshot();
                let changed = view.apply(op);
                if changed && *view.lines != before.lines {
                    self.stack.record(before);
                }
                changed
            }
            op => view.apply(op),
        }
    }
}

/// Byte offset `byte_offset` into `value` (which may contain `'\n'`,
/// e.g. a `TextArea`) → `(line, char_column)`, the coordinate space
/// [`EditView`] operates in.
pub fn byte_offset_to_line_col(value: &str, byte_offset: usize) -> Caret {
    let byte_offset = crate::text_util::snap_to_char_boundary(value, byte_offset.min(value.len()));
    let prefix = &value[..byte_offset];
    let line = prefix.matches('\n').count();
    let line_start = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col = prefix[line_start..].chars().count();
    (line, col)
}

/// Inverse of [`byte_offset_to_line_col`]: `(line, char_column)` →
/// byte offset into `value`.
pub fn line_col_to_byte_offset(value: &str, line: usize, col: usize) -> usize {
    let mut offset = 0usize;
    for (i, l) in value.split('\n').enumerate() {
        if i == line {
            let take: usize = l.chars().take(col).map(char::len_utf8).sum();
            return offset + take.min(l.len());
        }
        offset += l.len() + 1; // +1 for the '\n' separator consumed between lines
    }
    value.len()
}

/// Apply one [`EditOp`] against a flat `value` plus byte-offset
/// `cursor`/`selection_anchor`, and hand back the resulting
/// `(value, cursor, selection_anchor)`.
///
/// [`EditView`] itself works in `(line, char_column)` coordinates, so
/// this splits `value` on `'\n'` for the one `apply` call and converts
/// back via [`byte_offset_to_line_col`]/[`line_col_to_byte_offset`] — the
/// conversion this function owns so every caller keeps working in the
/// byte-offset space its own field already documents.
///
/// `cursor` and `selection_anchor` are app-owned public data with no
/// guarantee of landing on a char boundary; both are snapped and clamped,
/// so a bogus byte offset (e.g. an app computing it in char counts)
/// clamps instead of panicking on a mid-char slice.
pub fn apply_flat_edit_op(
    value: &str,
    cursor: usize,
    selection_anchor: Option<usize>,
    op: EditOp,
) -> (String, usize, Option<usize>) {
    apply_flat_edit_op_in(value, cursor, selection_anchor, &op, None)
}

/// [`apply_flat_edit_op`], served through `history` when there is one so
/// [`EditOp::Undo`]/[`EditOp::Redo`] act and content edits are recorded.
fn apply_flat_edit_op_in(
    value: &str,
    cursor: usize,
    selection_anchor: Option<usize>,
    op: &EditOp,
    history: Option<&mut EditHistory>,
) -> (String, usize, Option<usize>) {
    let mut lines: Vec<String> = if value.is_empty() {
        vec![String::new()]
    } else {
        value.split('\n').map(str::to_string).collect()
    };
    let (mut cursor_line, mut cursor_col) = byte_offset_to_line_col(value, cursor);
    let mut anchor = selection_anchor.map(|a| byte_offset_to_line_col(value, a));
    let mut view = EditView {
        lines: &mut lines,
        cursor_line: &mut cursor_line,
        cursor_col: &mut cursor_col,
        anchor: &mut anchor,
    };
    match history {
        Some(history) => history.apply(&mut view, op),
        None => view.apply(op),
    };

    let new_value = lines.join("\n");
    let new_cursor = line_col_to_byte_offset(&new_value, cursor_line, cursor_col);
    let new_anchor = anchor.map(|(l, c)| line_col_to_byte_offset(&new_value, l, c));
    (new_value, new_cursor, new_anchor)
}

/// The caret a flat-value widget's `handle` call starts from — shared by
/// [`crate::TextField`] and [`crate::CommandLine`], the two widgets that
/// store their value as a flat `String` plus a byte-offset caret: whatever
/// the widget's own per-focus `state_cursor` has walked to, else the
/// descriptor's own `descriptor_cursor`, else the end of `value` —
/// snapped to a char boundary either way.
pub fn effective_flat_cursor(
    value: &str,
    state_cursor: Option<usize>,
    descriptor_cursor: Option<usize>,
) -> usize {
    let raw = state_cursor
        .or(descriptor_cursor)
        .unwrap_or(value.len())
        .min(value.len());
    crate::text_util::snap_to_char_boundary(value, raw)
}

/// The text currently selected in a flat-value widget, as
/// [`flat_selection_range`] resolves it against `value` — what a Ctrl-C
/// in [`crate::TextField`] or [`crate::CommandLine`] reads before
/// emitting its own `Copied` action.
pub fn flat_selected_text(value: &str, cursor: usize, anchor: Option<usize>) -> Option<String> {
    let (lo, hi) = flat_selection_range(value, cursor, anchor)?;
    Some(value[lo..hi].to_string())
}

/// The selected span of a flat `value`, as an ordered byte-offset
/// `(start, end)` pair, or `None` when `anchor` is absent or equal to
/// `cursor`.
///
/// Both offsets are snapped to char boundaries and clamped to `value`, so
/// the result is always a safe slice range — what a widget's Ctrl-C needs
/// before emitting [`crate::UiEvent::TextCopied`].
pub fn flat_selection_range(
    value: &str,
    cursor: usize,
    anchor: Option<usize>,
) -> Option<(usize, usize)> {
    let anchor = crate::text_util::snap_to_char_boundary(value, anchor?.min(value.len()));
    let cursor = crate::text_util::snap_to_char_boundary(value, cursor.min(value.len()));
    if anchor == cursor {
        return None;
    }
    Some((anchor.min(cursor), anchor.max(cursor)))
}

// ── Routed-event handling ──────────────────────────────────────────────
//
// Every editable text widget answers the same routed events the same way:
// keys through `EditIntent`, committed text as an insert, a press parks
// the caret and starts a drag-select, a captured move extends it, focus
// in/out reseeds and drops the caret. `handle_flat_edit_event` (a flat
// `String` with byte offsets) and `handle_lines_edit_event` (a
// `Vec<String>` with `(line, col)` carets) are two views over that one
// event flow, `handle_edit_event`, so a widget only claims its own keys
// (Enter, Escape, list navigation) and its own sub-regions first.

/// Per-focus editing state for a widget whose value is one flat `String`
/// with byte-offset positions: the caret and anchor its own editing has
/// walked to, and whether a press is dragging a selection.
///
/// `cursor` is `None` while the widget still follows its descriptor's own
/// caret; [`painted_flat_caret`] gives a painter the caret and anchor to
/// show either way. The undo history survives focus changes, the way a
/// native text field's does.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FlatEditState {
    /// The caret byte offset, once editing has moved it.
    pub cursor: Option<usize>,
    /// The selection's fixed end, as a byte offset.
    pub anchor: Option<usize>,
    /// Whether a left press on the text is still held, so pointer moves
    /// extend the selection.
    pub dragging: bool,
    /// This widget's undo/redo history.
    pub history: EditHistory,
}

/// Per-focus editing state for a multi-line widget whose value is a
/// `Vec<String>` with `(line, char_column)` carets — the
/// [`handle_lines_edit_event`] counterpart of [`FlatEditState`].
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinesEditState {
    /// The caret, once editing has moved it.
    pub caret: Option<Caret>,
    /// The selection's fixed end.
    pub anchor: Option<Caret>,
    /// Whether a left press on the text is still held, so pointer moves
    /// extend the selection.
    pub dragging: bool,
    /// This widget's undo/redo history.
    pub history: EditHistory,
}

/// What [`handle_flat_edit_event`] / [`handle_lines_edit_event`] resolved
/// one routed event to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlatEditOutcome {
    /// The value changed; carries the whole new value (lines joined with
    /// `'\n'` for the multi-line view).
    Changed(String),
    /// The event was this widget's — a caret or selection move, a copy,
    /// a focus change — with no new value to report. A redraw has been
    /// requested where anything visible moved.
    Consumed,
    /// Not an editing event: the caller's to handle or bubble.
    Ignored,
}

/// The caret and selection anchor a flat-value painter shows: `state`'s
/// own once editing has moved the caret, else the descriptor's
/// `cursor`/`anchor`.
pub fn painted_flat_caret(
    state: Option<&FlatEditState>,
    cursor: Option<usize>,
    anchor: Option<usize>,
) -> (Option<usize>, Option<usize>) {
    match state {
        Some(FlatEditState {
            cursor: Some(c),
            anchor: a,
            ..
        }) => (Some(*c), *a),
        _ => (cursor, anchor),
    }
}

/// The two value shapes [`handle_edit_event`] edits through.
trait EditTarget {
    /// Apply `op`; the new value when content changed.
    fn apply(&mut self, op: &EditOp) -> Option<String>;
    /// The selected text, if a selection is active.
    fn selected_text(&self) -> Option<String>;
    /// Reseed the caret and anchor from the descriptor.
    fn focus_in(&mut self);
    /// Drop the caret, anchor and any drag.
    fn focus_out(&mut self);
    /// Whether a press is dragging a selection.
    fn is_dragging(&self) -> bool;
    /// Start or end a drag-select.
    fn set_dragging(&mut self, dragging: bool);
}

struct FlatTarget<'a> {
    value: &'a str,
    cursor: Option<usize>,
    anchor: Option<usize>,
    state: &'a mut FlatEditState,
}

impl FlatTarget<'_> {
    fn caret_and_anchor(&self) -> (usize, Option<usize>) {
        let cursor = effective_flat_cursor(self.value, self.state.cursor, self.cursor);
        let anchor = if self.state.cursor.is_some() {
            self.state.anchor
        } else {
            self.anchor
        };
        (cursor, anchor)
    }
}

impl EditTarget for FlatTarget<'_> {
    fn apply(&mut self, op: &EditOp) -> Option<String> {
        let (cursor, anchor) = self.caret_and_anchor();
        let (value, cursor, anchor) = apply_flat_edit_op_in(
            self.value,
            cursor,
            anchor,
            op,
            Some(&mut self.state.history),
        );
        self.state.cursor = Some(cursor);
        // A no-op extend (Shift+Left at offset 0, a drag that has not
        // left its press point) leaves `anchor == cursor`; keeping it
        // would read as a zero-width selection the next keystroke eats.
        self.state.anchor = anchor.filter(|a| *a != cursor);
        (value != self.value).then_some(value)
    }

    fn selected_text(&self) -> Option<String> {
        let (cursor, anchor) = self.caret_and_anchor();
        flat_selected_text(self.value, cursor, anchor)
    }

    fn focus_in(&mut self) {
        self.state.cursor = Some(
            self.cursor
                .unwrap_or(self.value.len())
                .min(self.value.len()),
        );
        self.state.anchor = self.anchor;
    }

    fn focus_out(&mut self) {
        self.state.cursor = None;
        self.state.anchor = None;
        self.state.dragging = false;
    }

    fn is_dragging(&self) -> bool {
        self.state.dragging
    }

    fn set_dragging(&mut self, dragging: bool) {
        self.state.dragging = dragging;
    }
}

struct LinesTarget<'a> {
    lines: &'a [String],
    seed: Caret,
    state: &'a mut LinesEditState,
}

impl LinesTarget<'_> {
    fn caret(&self) -> Caret {
        let (line, col) = self.state.caret.unwrap_or(self.seed);
        let line = line.min(self.lines.len().saturating_sub(1));
        let col = col.min(self.lines.get(line).map_or(0, |l| l.chars().count()));
        (line, col)
    }
}

impl EditTarget for LinesTarget<'_> {
    fn apply(&mut self, op: &EditOp) -> Option<String> {
        let mut lines = if self.lines.is_empty() {
            vec![String::new()]
        } else {
            self.lines.to_vec()
        };
        let (mut line, mut col) = self.caret();
        let mut anchor = self.state.anchor;
        let changed = self.state.history.apply(
            &mut EditView {
                lines: &mut lines,
                cursor_line: &mut line,
                cursor_col: &mut col,
                anchor: &mut anchor,
            },
            op,
        );
        self.state.caret = Some((line, col));
        self.state.anchor = anchor.filter(|a| *a != (line, col));
        (changed && lines != self.lines).then(|| lines.join("\n"))
    }

    fn selected_text(&self) -> Option<String> {
        let (start, end) = selection_range(self.caret(), self.state.anchor)?;
        Some(text_in_range(self.lines, start, end))
    }

    fn focus_in(&mut self) {
        self.state.caret = Some(self.seed);
        self.state.anchor = None;
    }

    fn focus_out(&mut self) {
        self.state.caret = None;
        self.state.anchor = None;
        self.state.dragging = false;
    }

    fn is_dragging(&self) -> bool {
        self.state.dragging
    }

    fn set_dragging(&mut self, dragging: bool) {
        self.state.dragging = dragging;
    }
}

/// Resolve one routed event against a flat `value`.
///
/// `cursor`/`anchor` are the descriptor's own caret and selection anchor
/// (byte offsets), which seed `state` on focus-in and stand in for it
/// until editing moves the caret. `offset_at` maps a widget-local point
/// to the byte offset under it, or `None` when the point is not on the
/// text (a press there only takes focus).
///
/// Ctrl/Cmd+C and the copy half of Ctrl/Cmd+X emit
/// [`crate::UiEvent::TextCopied`] through `cx`; paste is the host's (its
/// text arrives back as [`crate::interactive::RoutedEvent::Text`]), so
/// Ctrl/Cmd+V is [`FlatEditOutcome::Ignored`]. Ctrl/Cmd+Z and
/// Ctrl/Cmd+Shift+Z undo and redo through `state`'s own [`EditHistory`],
/// and are [`FlatEditOutcome::Ignored`] when it has nothing to give
/// back, so the chord still reaches an app accelerator. Callers claim
/// Enter before calling when it means commit rather than a newline.
pub fn handle_flat_edit_event(
    value: &str,
    cursor: Option<usize>,
    anchor: Option<usize>,
    state: &mut FlatEditState,
    event: &crate::interactive::RoutedEvent,
    offset_at: &dyn Fn(crate::event::Point) -> Option<usize>,
    cx: &mut crate::interactive::EventCx,
) -> FlatEditOutcome {
    let caret_at = |p| offset_at(p).map(|off| byte_offset_to_line_col(value, off));
    let mut target = FlatTarget {
        value,
        cursor,
        anchor,
        state,
    };
    handle_edit_event(&mut target, event, &caret_at, cx)
}

/// [`handle_flat_edit_event`] for a multi-line `lines` buffer, with
/// `seed` the descriptor's own caret and `caret_at` mapping a
/// widget-local point to the `(line, char_column)` under it.
pub fn handle_lines_edit_event(
    lines: &[String],
    seed: Caret,
    state: &mut LinesEditState,
    event: &crate::interactive::RoutedEvent,
    caret_at: &dyn Fn(crate::event::Point) -> Option<Caret>,
    cx: &mut crate::interactive::EventCx,
) -> FlatEditOutcome {
    let mut target = LinesTarget { lines, seed, state };
    handle_edit_event(&mut target, event, caret_at, cx)
}

fn handle_edit_event(
    target: &mut dyn EditTarget,
    event: &crate::interactive::RoutedEvent,
    caret_at: &dyn Fn(crate::event::Point) -> Option<Caret>,
    cx: &mut crate::interactive::EventCx,
) -> FlatEditOutcome {
    use crate::event::MouseButton;
    use crate::interactive::{PointerKind, RoutedEvent};

    match event {
        RoutedEvent::Key(key) => match EditIntent::from_key(&key.key, key.modifiers) {
            Some(EditIntent::SelectAll) => commit(target, &EditOp::SelectAll, cx),
            Some(EditIntent::Copy) => {
                if let Some(text) = target.selected_text() {
                    cx.emit(crate::UiEvent::TextCopied(text));
                }
                FlatEditOutcome::Consumed
            }
            Some(EditIntent::Cut) => match target.selected_text() {
                Some(text) => {
                    cx.emit(crate::UiEvent::TextCopied(text));
                    commit(target, &EditOp::DeleteSelection, cx)
                }
                None => FlatEditOutcome::Consumed,
            },
            Some(EditIntent::Paste) | None => FlatEditOutcome::Ignored,
            // An empty history leaves the chord to the app's own
            // accelerator.
            Some(EditIntent::Op(op @ (EditOp::Undo | EditOp::Redo))) => match target.apply(&op) {
                Some(value) => FlatEditOutcome::Changed(value),
                None => FlatEditOutcome::Ignored,
            },
            Some(EditIntent::Op(op)) => commit(target, &op, cx),
        },
        RoutedEvent::Text(text) => commit(target, &EditOp::InsertText(text.clone()), cx),
        RoutedEvent::Pointer {
            kind: PointerKind::Down(MouseButton::Left),
            local,
            mods,
            ..
        } => {
            cx.request_focus(None);
            match caret_at(*local) {
                Some((line, col)) => {
                    cx.capture_pointer();
                    target.set_dragging(true);
                    commit(
                        target,
                        &EditOp::SetCursor {
                            line,
                            col,
                            extend: mods.shift,
                        },
                        cx,
                    )
                }
                None => FlatEditOutcome::Consumed,
            }
        }
        RoutedEvent::Pointer {
            kind: PointerKind::Move,
            local,
            ..
        } if target.is_dragging() => match caret_at(*local) {
            Some((line, col)) => commit(
                target,
                &EditOp::SetCursor {
                    line,
                    col,
                    extend: true,
                },
                cx,
            ),
            None => FlatEditOutcome::Consumed,
        },
        RoutedEvent::Pointer {
            kind: PointerKind::Up(MouseButton::Left),
            ..
        } if target.is_dragging() => {
            target.set_dragging(false);
            cx.release_pointer();
            FlatEditOutcome::Consumed
        }
        RoutedEvent::Action(crate::a11y::A11yAction::Focus) => {
            cx.request_focus(None);
            FlatEditOutcome::Consumed
        }
        RoutedEvent::FocusIn(_) => {
            target.focus_in();
            FlatEditOutcome::Consumed
        }
        RoutedEvent::FocusOut => {
            target.focus_out();
            FlatEditOutcome::Consumed
        }
        _ => FlatEditOutcome::Ignored,
    }
}

/// Apply `op` and report it: a content change as
/// [`FlatEditOutcome::Changed`], anything else as a redraw plus
/// [`FlatEditOutcome::Consumed`].
fn commit(
    target: &mut dyn EditTarget,
    op: &EditOp,
    cx: &mut crate::interactive::EventCx,
) -> FlatEditOutcome {
    match target.apply(op) {
        Some(value) => FlatEditOutcome::Changed(value),
        None => {
            cx.request_redraw();
            FlatEditOutcome::Consumed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view<'a>(
        lines: &'a mut Vec<String>,
        caret: &'a mut Caret,
        anchor: &'a mut Option<Caret>,
    ) -> EditView<'a> {
        let (line, col) = caret;
        EditView {
            lines,
            cursor_line: line,
            cursor_col: col,
            anchor,
        }
    }

    #[test]
    fn word_boundaries_treat_underscored_identifiers_as_one_word() {
        assert_eq!(word_boundary_left("foo_bar baz", 11), 8);
        assert_eq!(word_boundary_left("foo_bar baz", 8), 0);
        assert_eq!(word_boundary_right("foo_bar baz", 0), 7);
        assert_eq!(word_boundary_right("foo_bar baz", 7), 11);
    }

    #[test]
    fn word_boundaries_clamp_at_both_ends() {
        assert_eq!(word_boundary_left("abc", 0), 0);
        assert_eq!(word_boundary_right("abc", 3), 3);
        assert_eq!(word_boundary_left("abc", 99), 0);
        assert_eq!(word_boundary_right("abc", 99), 3);
    }

    #[test]
    fn ctrl_left_and_right_map_to_word_motion() {
        let ctrl = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        assert_eq!(
            EditOp::from_key(&Key::Named(NamedKey::Left), ctrl),
            Some(EditOp::MoveWordLeft { extend: false })
        );
        assert_eq!(
            EditOp::from_key(&Key::Named(NamedKey::Backspace), ctrl),
            Some(EditOp::DeleteWordBackward)
        );
        assert_eq!(
            EditOp::from_key(&Key::Named(NamedKey::Left), Modifiers::default()),
            Some(EditOp::MoveLeft { extend: false })
        );
    }

    #[test]
    fn delete_word_backward_stops_at_the_word_boundary() {
        let mut lines = vec!["let foo_bar = 1".to_string()];
        let mut caret = (0usize, 11usize);
        let mut anchor = None;
        assert!(view(&mut lines, &mut caret, &mut anchor).apply(&EditOp::DeleteWordBackward));
        assert_eq!(lines, vec!["let  = 1".to_string()]);
        assert_eq!(caret, (0, 4));
    }

    #[test]
    fn delete_word_backward_at_column_zero_joins_the_previous_line() {
        let mut lines = vec!["ab".to_string(), "cd".to_string()];
        let mut caret = (1usize, 0usize);
        let mut anchor = None;
        assert!(view(&mut lines, &mut caret, &mut anchor).apply(&EditOp::DeleteWordBackward));
        assert_eq!(lines, vec!["abcd".to_string()]);
        assert_eq!(caret, (0, 2));
    }

    #[test]
    fn word_motion_crosses_line_edges() {
        let mut lines = vec!["ab".to_string(), "cd".to_string()];
        let mut caret = (1usize, 0usize);
        let mut anchor = None;
        view(&mut lines, &mut caret, &mut anchor).apply(&EditOp::MoveWordLeft { extend: false });
        assert_eq!(caret, (0, 2));
        view(&mut lines, &mut caret, &mut anchor).apply(&EditOp::MoveWordRight { extend: false });
        assert_eq!(caret, (1, 0));
    }

    #[test]
    fn multibyte_insert_and_delete_stay_on_char_boundaries() {
        let mut lines = vec!["éé".to_string()];
        let mut caret = (0usize, 1usize);
        let mut anchor = None;
        view(&mut lines, &mut caret, &mut anchor).apply(&EditOp::InsertChar('x'));
        assert_eq!(lines, vec!["éxé".to_string()]);
        view(&mut lines, &mut caret, &mut anchor).apply(&EditOp::DeleteBackward);
        assert_eq!(lines, vec!["éé".to_string()]);
    }

    #[test]
    fn edit_intent_claims_the_clipboard_chords_ahead_of_edit_op() {
        let ctrl = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        assert_eq!(
            EditIntent::from_key(&Key::Char('a'), ctrl),
            Some(EditIntent::SelectAll)
        );
        assert_eq!(
            EditIntent::from_key(&Key::Char('C'), ctrl),
            Some(EditIntent::Copy)
        );
        assert_eq!(
            EditIntent::from_key(&Key::Char('x'), ctrl),
            Some(EditIntent::Cut)
        );
        assert_eq!(
            EditIntent::from_key(&Key::Char('V'), ctrl),
            Some(EditIntent::Paste)
        );
        // Unmodified, the same letter is ordinary typing, not a chord.
        assert_eq!(
            EditIntent::from_key(&Key::Char('a'), Modifiers::default()),
            Some(EditIntent::Op(EditOp::InsertChar('a')))
        );
        // A key the chord vocabulary doesn't claim falls back to
        // `EditOp::from_key`.
        assert_eq!(
            EditIntent::from_key(&Key::Named(NamedKey::Left), Modifiers::default()),
            Some(EditIntent::Op(EditOp::MoveLeft { extend: false }))
        );
    }

    #[test]
    fn effective_flat_cursor_prefers_state_then_descriptor_then_end() {
        assert_eq!(effective_flat_cursor("hello", Some(2), Some(4)), 2);
        assert_eq!(effective_flat_cursor("hello", None, Some(4)), 4);
        assert_eq!(effective_flat_cursor("hello", None, None), 5);
        // A stale offset past the current value clamps instead of
        // panicking on the slice it feeds.
        assert_eq!(effective_flat_cursor("hi", Some(99), None), 2);
    }

    #[test]
    fn flat_selected_text_reads_the_ordered_span() {
        assert_eq!(
            flat_selected_text("hello", 1, Some(4)),
            Some("ell".to_string())
        );
        assert_eq!(
            flat_selected_text("hello", 4, Some(1)),
            Some("ell".to_string())
        );
        assert_eq!(flat_selected_text("hello", 2, Some(2)), None);
        assert_eq!(flat_selected_text("hello", 2, None), None);
    }

    #[test]
    fn flat_bridge_round_trips_byte_offsets() {
        let (value, cursor, anchor) =
            apply_flat_edit_op("héllo", 1, None, EditOp::MoveWordRight { extend: true });
        assert_eq!(value, "héllo");
        assert_eq!(cursor, value.len());
        assert_eq!(anchor, Some(1));
    }

    #[test]
    fn flat_bridge_snaps_a_mid_char_cursor_instead_of_panicking() {
        // Byte 2 sits inside the 2-byte 'é' that starts at byte 1, so the
        // caret snaps back to byte 1 and Backspace deletes the 'h'.
        let (value, cursor, _) = apply_flat_edit_op("héllo", 2, None, EditOp::DeleteBackward);
        assert_eq!(value, "éllo");
        assert_eq!(cursor, 0);
    }

    #[test]
    fn flat_selection_range_orders_and_snaps_its_ends() {
        assert_eq!(flat_selection_range("héllo", 5, Some(1)), Some((1, 5)));
        assert_eq!(flat_selection_range("héllo", 1, Some(5)), Some((1, 5)));
        assert_eq!(flat_selection_range("héllo", 2, Some(2)), None);
        assert_eq!(flat_selection_range("héllo", 1, None), None);
    }

    #[test]
    fn undo_and_redo_are_inert_in_the_view() {
        let mut lines = vec!["a".to_string()];
        let mut caret = (0usize, 1usize);
        let mut anchor = None;
        assert!(!view(&mut lines, &mut caret, &mut anchor).apply(&EditOp::Undo));
        assert!(!view(&mut lines, &mut caret, &mut anchor).apply(&EditOp::Redo));
        assert_eq!(lines, vec!["a".to_string()]);
    }

    #[test]
    fn mutates_content_names_only_the_buffer_rewriting_ops() {
        assert!(mutates_content(&EditOp::InsertChar('a')));
        assert!(mutates_content(&EditOp::DeleteWordForward));
        assert!(!mutates_content(&EditOp::MoveWordLeft { extend: false }));
        assert!(!mutates_content(&EditOp::SelectAll));
    }

    #[test]
    fn text_in_range_clamps_an_end_line_past_the_buffer() {
        // `lines` has shrunk to 2 entries, but `end` still names line 4 —
        // the shape a stale anchor takes once content changes out from
        // under it. The end line clamps to the last real line (1) instead
        // of indexing past it.
        let lines = vec!["ab".to_string(), "cd".to_string()];
        assert_eq!(text_in_range(&lines, (0, 1), (4, 2)), "b\ncd");
    }

    #[test]
    fn delete_range_clamps_an_end_line_past_the_buffer() {
        // Same shape, driven through the selection-delete path a keystroke
        // takes: anchor at line 4 against a buffer that now has only 2
        // lines. Deleting the (clamped) selection must not panic, and
        // leaves a sane, fully-clamped result.
        let mut lines = vec!["ab".to_string(), "cd".to_string()];
        let mut caret = (0usize, 1usize);
        let mut anchor = Some((4usize, 2usize));
        assert!(view(&mut lines, &mut caret, &mut anchor).delete_selection());
        assert_eq!(lines, vec!["a".to_string()]);
        assert_eq!(caret, (0, 1));
        assert_eq!(anchor, None);
    }

    #[test]
    fn edit_intent_refuses_a_chorded_letter_that_is_not_an_editing_chord() {
        let ctrl = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        let cmd = Modifiers {
            cmd: true,
            ..Default::default()
        };
        assert_eq!(EditIntent::from_key(&Key::Char('s'), ctrl), None);
        assert_eq!(EditIntent::from_key(&Key::Char('s'), cmd), None);
        assert_eq!(
            EditIntent::from_key(&Key::Char('c'), ctrl),
            Some(EditIntent::Copy)
        );
        assert_eq!(
            EditIntent::from_key(&Key::Named(NamedKey::Left), ctrl),
            Some(EditIntent::Op(EditOp::MoveWordLeft { extend: false }))
        );
    }

    #[test]
    fn edit_intent_types_an_alt_gr_character() {
        let alt_gr = Modifiers {
            ctrl: true,
            alt: true,
            ..Default::default()
        };
        assert_eq!(
            EditIntent::from_key(&Key::Char('@'), alt_gr),
            Some(EditIntent::Op(EditOp::InsertChar('@')))
        );
    }

    #[test]
    fn painted_flat_caret_prefers_the_state_once_editing_moved_it() {
        let moved = FlatEditState {
            cursor: Some(4),
            anchor: Some(1),
            ..Default::default()
        };
        assert_eq!(
            painted_flat_caret(Some(&moved), Some(0), None),
            (Some(4), Some(1))
        );
        assert_eq!(
            painted_flat_caret(Some(&FlatEditState::default()), Some(2), Some(0)),
            (Some(2), Some(0))
        );
        assert_eq!(painted_flat_caret(None, None, None), (None, None));
    }

    fn routed_key(key: Key, modifiers: Modifiers) -> crate::interactive::RoutedEvent {
        crate::interactive::RoutedEvent::Key(crate::interactive::KeyEvent {
            key,
            modifiers,
            repeat: false,
        })
    }

    fn ctrl_mods(shift: bool) -> Modifiers {
        Modifiers {
            ctrl: true,
            shift,
            ..Default::default()
        }
    }

    /// Drives one routed event through the flat handler the way a widget
    /// does, against a value the "app" echoes back after every change.
    fn flat_step(
        value: &mut String,
        state: &mut FlatEditState,
        event: crate::interactive::RoutedEvent,
    ) -> FlatEditOutcome {
        let (outcome, _) = crate::compose::widget_host::with_cx(
            crate::types::WidgetId::new("f"),
            crate::primitive::Units::Cells,
            |cx| handle_flat_edit_event(value, None, None, state, &event, &|_| None, cx),
        );
        if let FlatEditOutcome::Changed(new) = &outcome {
            *value = new.clone();
        }
        outcome
    }

    fn lines_step(
        lines: &mut Vec<String>,
        state: &mut LinesEditState,
        event: crate::interactive::RoutedEvent,
    ) -> FlatEditOutcome {
        let (outcome, _) = crate::compose::widget_host::with_cx(
            crate::types::WidgetId::new("l"),
            crate::primitive::Units::Cells,
            |cx| handle_lines_edit_event(lines, (0, 0), state, &event, &|_| None, cx),
        );
        if let FlatEditOutcome::Changed(new) = &outcome {
            *lines = new.split('\n').map(str::to_string).collect();
        }
        outcome
    }

    #[test]
    fn ctrl_z_and_ctrl_shift_z_map_to_undo_and_redo() {
        assert_eq!(
            EditIntent::from_key(&Key::Char('z'), ctrl_mods(false)),
            Some(EditIntent::Op(EditOp::Undo))
        );
        assert_eq!(
            EditIntent::from_key(&Key::Char('Z'), ctrl_mods(true)),
            Some(EditIntent::Op(EditOp::Redo))
        );
        let cmd = Modifiers {
            cmd: true,
            ..Default::default()
        };
        assert_eq!(
            EditIntent::from_key(&Key::Char('z'), cmd),
            Some(EditIntent::Op(EditOp::Undo))
        );
    }

    #[test]
    fn edit_history_records_content_edits_only() {
        let (mut lines, mut line, mut col, mut anchor) = (vec!["ab".to_string()], 0, 2, None);
        let mut history = EditHistory::default();
        let mut view = EditView {
            lines: &mut lines,
            cursor_line: &mut line,
            cursor_col: &mut col,
            anchor: &mut anchor,
        };
        assert!(history.apply(&mut view, &EditOp::InsertChar('c')));
        assert!(history.apply(&mut view, &EditOp::MoveLeft { extend: false }));
        assert!(history.apply(&mut view, &EditOp::Undo));
        assert_eq!(view.lines, &vec!["ab".to_string()]);
        assert_eq!(view.caret(), (0, 2));
        assert!(!history.can_undo());
        assert!(history.apply(&mut view, &EditOp::Redo));
        assert_eq!(view.lines, &vec!["abc".to_string()]);
        assert!(!history.apply(&mut view, &EditOp::Redo));
    }

    #[test]
    fn a_flat_widget_undoes_and_redoes_through_its_own_state() {
        let mut value = "hi".to_string();
        let mut state = FlatEditState::default();
        flat_step(
            &mut value,
            &mut state,
            crate::interactive::RoutedEvent::Text("!".into()),
        );
        assert_eq!(value, "hi!");
        assert_eq!(
            flat_step(
                &mut value,
                &mut state,
                routed_key(Key::Char('z'), ctrl_mods(false))
            ),
            FlatEditOutcome::Changed("hi".into())
        );
        assert_eq!(
            flat_step(
                &mut value,
                &mut state,
                routed_key(Key::Char('z'), ctrl_mods(true))
            ),
            FlatEditOutcome::Changed("hi!".into())
        );
    }

    #[test]
    fn undo_with_nothing_to_give_back_leaves_the_chord_to_the_app() {
        let mut value = "hi".to_string();
        let mut state = FlatEditState::default();
        assert_eq!(
            flat_step(
                &mut value,
                &mut state,
                routed_key(Key::Char('z'), ctrl_mods(false))
            ),
            FlatEditOutcome::Ignored
        );
    }

    #[test]
    fn undo_history_survives_a_focus_change() {
        let mut lines = vec![String::new()];
        let mut state = LinesEditState::default();
        lines_step(
            &mut lines,
            &mut state,
            crate::interactive::RoutedEvent::Text("ab".into()),
        );
        lines_step(
            &mut lines,
            &mut state,
            crate::interactive::RoutedEvent::FocusOut,
        );
        assert_eq!(state.caret, None);
        assert_eq!(
            lines_step(
                &mut lines,
                &mut state,
                routed_key(Key::Char('z'), ctrl_mods(false))
            ),
            FlatEditOutcome::Changed(String::new())
        );
    }

    #[test]
    fn ctrl_word_and_doc_keys_edit_through_the_lines_handler() {
        let mut lines = vec!["foo bar".to_string()];
        let mut state = LinesEditState {
            caret: Some((0, 7)),
            ..Default::default()
        };
        assert_eq!(
            lines_step(
                &mut lines,
                &mut state,
                routed_key(Key::Named(NamedKey::Backspace), ctrl_mods(false))
            ),
            FlatEditOutcome::Changed("foo ".into())
        );
        lines_step(
            &mut lines,
            &mut state,
            routed_key(Key::Named(NamedKey::Home), ctrl_mods(false)),
        );
        assert_eq!(state.caret, Some((0, 0)));
        lines_step(
            &mut lines,
            &mut state,
            routed_key(Key::Named(NamedKey::Right), ctrl_mods(false)),
        );
        assert_eq!(state.caret, Some((0, 3)));
        assert_eq!(
            lines_step(
                &mut lines,
                &mut state,
                routed_key(Key::Named(NamedKey::Delete), ctrl_mods(false))
            ),
            FlatEditOutcome::Changed("foo".into())
        );
    }
}
