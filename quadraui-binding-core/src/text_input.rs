//! The standalone `TextInput` extra's cursor/selection controller: caches
//! each widget's cursor/selection across keystrokes ([`TextCursorState`],
//! keyed by `WidgetId` in the binding's own map) so a multi-character edit
//! lands at the right position instead of restarting from the
//! descriptor's own `cursor_line`/`cursor_col` snapshot every keystroke.

use std::cell::RefCell;
use std::collections::HashMap;

use quadraui::{
    EditOp, FormEvent, Key, Modifiers, NamedKey, TextEditor, TextInput, UiEvent, WidgetId,
};

use crate::extra_paint::extra_as;
use crate::hit::PaintedHit;

/// One [`TextInput`]'s cached cursor/selection, read and written by
/// [`apply_text_input_edit`] around each `TextEditor::apply` call so the
/// position survives across the keystrokes that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextCursorState {
    line: usize,
    col: usize,
    anchor: Option<(usize, usize)>,
}

/// Resolves a key focused on a standalone [`TextInput`] extra into the
/// [`FormEvent`] it should produce: `Enter` always commits and drops the
/// cached cursor for `id` (this primitive has no multiline mode to
/// special-case — an app that wants a literal newline needs a different
/// key, since `Enter` here always means "done"), a Ctrl/Cmd+letter
/// chord passes through unclaimed so a host's accelerator still fires, and
/// every other key goes through [`EditOp::from_key`]/[`TextEditor::apply`].
/// Returns `None` when the key isn't one [`EditOp::from_key`] recognizes, or
/// when applying it doesn't actually change the text — the caller then
/// falls through to the raw `KeyPressed` fallback.
pub fn text_input_key_event(
    id: WidgetId,
    text_input: &TextInput,
    key: &Key,
    modifiers: Modifiers,
    text_cursors: &RefCell<HashMap<WidgetId, TextCursorState>>,
) -> Option<UiEvent> {
    if matches!(key, Key::Named(NamedKey::Enter)) {
        text_cursors.borrow_mut().remove(&id);
        return Some(UiEvent::Form(
            id,
            FormEvent::TextInputCommitted {
                id,
                value: text_input.lines.join("\n"),
            },
        ));
    }
    if matches!(key, Key::Char(_)) && (modifiers.ctrl || modifiers.cmd) {
        return None;
    }
    let op = EditOp::from_key(key, modifiers)?;
    apply_text_input_edit(id, text_input, op, text_cursors)
}

/// Clamps a cached (or descriptor-seeded) cursor position to `lines`' actual
/// bounds, so a stale cache entry left over from a longer previous value (or
/// a `cursor_col`/`cursor_line` the host set past end-of-line) can never
/// index out of range once handed to [`TextEditor`].
pub fn clamp_cursor(line: usize, col: usize, lines: &[String]) -> (usize, usize) {
    let line = line.min(lines.len().saturating_sub(1));
    let col = col.min(lines.get(line).map_or(0, |l| l.chars().count()));
    (line, col)
}

/// Applies `op` to a [`TextEditor`] seeded from `text_input`'s current text
/// but from the *cached* cursor/selection for `id` in `text_cursors` — not
/// the descriptor's own `cursor_line`/`cursor_col`, which is only ever the
/// position the binding's own event handler last echoed back (typically
/// 0/0, since `FormEvent::TextInputChanged`/`Committed` carry no cursor for
/// a host to echo). This is what lets a multi-character edit land at the right
/// position: without it, every keystroke would restart from the same
/// snapshot position and (for anything but a single appended character)
/// corrupt the text. The resulting cursor/selection is written back to
/// `text_cursors` unconditionally — even when the text itself didn't
/// change, e.g. a pure cursor move — so the *next* keystroke continues from
/// it; this mirrors `compose::FormController::handle_text_op`'s
/// `self.text_edit = Some(TextEditState { .. })`, including its
/// degenerate-anchor cleanup for a no-op selection extend. Selection
/// (`Shift`+arrows, `SelectAll`) and undo/redo are tracked here the same
/// way, but a selecting/undoing op is otherwise unreachable through any
/// binding today: nothing in this module currently calls [`EditOp::from_key`] with
/// `extend: true` or produces `EditOp::Undo`/`Redo`, since `Shift`+arrow
/// falls through [`EditOp::from_key`] unclaimed. Returns `None` when the
/// resulting text is unchanged (a pure cursor move, or a key
/// `EditOp::from_key` mapped to a no-op) — the caller then falls through to
/// the raw `KeyPressed` fallback.
pub fn apply_text_input_edit(
    id: WidgetId,
    text_input: &TextInput,
    op: EditOp,
    text_cursors: &RefCell<HashMap<WidgetId, TextCursorState>>,
) -> Option<UiEvent> {
    let before = text_input.lines.join("\n");
    let cached = text_cursors.borrow().get(&id).copied();
    let (seed_line, seed_col) = cached
        .map(|c| (c.line, c.col))
        .unwrap_or((text_input.cursor_line, text_input.cursor_col));
    let (seed_line, seed_col) = clamp_cursor(seed_line, seed_col, &text_input.lines);
    let seed_anchor = cached
        .and_then(|c| c.anchor)
        .map(|(al, ac)| clamp_cursor(al, ac, &text_input.lines));

    let mut input = text_input.clone();
    input.cursor_line = seed_line;
    input.cursor_col = seed_col;
    let mut editor = TextEditor::new(input);
    editor.set_selection_anchor(seed_anchor);
    editor.apply(op);
    let after = editor.lines.join("\n");

    let new_line = editor.cursor_line;
    let new_col = editor.cursor_col;
    // `TextEditor::apply`'s `MoveLeft`/`MoveRight`/etc with `extend: true`
    // can leave a degenerate `anchor == cursor` selection in place for a
    // no-op extend; left uncleared, the next keystroke would see a phantom
    // one-character-wide selection. See
    // `compose::FormController::handle_text_op`'s identical cleanup.
    let new_anchor = editor
        .selection_anchor()
        .filter(|a| *a != (new_line, new_col));
    text_cursors.borrow_mut().insert(
        id,
        TextCursorState {
            line: new_line,
            col: new_col,
            anchor: new_anchor,
        },
    );

    if after == before {
        return None;
    }
    Some(UiEvent::Form(
        id,
        FormEvent::TextInputChanged { id, value: after },
    ))
}

/// Resolves a `CharTyped`/`ClipboardPaste` landing on the currently focused
/// [`TextInput`] extra into the [`FormEvent::TextInputChanged`] the insert
/// produced. Returns `None` when nothing is focused, the focused widget
/// isn't a `TextInput`, or it has scrolled out of this frame's hit map.
pub fn text_input_insert_event(
    focused: WidgetId,
    hit_map: &[PaintedHit],
    op: EditOp,
    text_cursors: &RefCell<HashMap<WidgetId, TextCursorState>>,
) -> Option<UiEvent> {
    let text_input = hit_map.iter().find_map(|hit| match hit {
        PaintedHit::Extra { id, extra, .. } if *id == focused => extra_as::<TextInput>(extra),
        _ => None,
    })?;
    apply_text_input_edit(focused, text_input, op, text_cursors)
}
