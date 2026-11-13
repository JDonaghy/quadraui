//! The standalone `TextInput` extra's cursor/selection/undo controller:
//! caches each widget's own [`TextEditor`] across keystrokes, keyed by
//! `WidgetId` in the binding's own map, so a multi-character edit lands at
//! the right position instead of restarting from the descriptor's own
//! `cursor_line`/`cursor_col` snapshot every keystroke, and so
//! `EditOp::Undo`/`Redo` have real history to act on instead of starting
//! from an empty stack on every call.

use std::cell::RefCell;
use std::collections::HashMap;

use quadraui::accelerator::{key_to_binding_name, parse_binding};
use quadraui::{
    EditOp, FormEvent, Key, KeyBinding, Modifiers, NamedKey, Point, TextEditor, TextInput,
    TextInputHit, TextInputMeasure, UiEvent, WidgetId,
};

use crate::extra_paint::{extra_as, extra_as_text_input, PasswordInput};
use crate::hit::PaintedHit;

/// Per-widget cache of the live [`TextEditor`] session behind a standalone
/// [`TextInput`] extra — [`apply_text_input_edit`] reads and writes the
/// entry for `id` around every [`TextEditor::apply`] call. Caching the
/// whole editor (not just cursor/anchor) is what lets its own
/// `undo_stack` survive across keystrokes — see [`apply_text_input_edit`]'s
/// doc for why that matters.
pub type TextEditorCache = RefCell<HashMap<WidgetId, TextEditor>>;

/// Resolves a key focused on a standalone [`TextInput`] extra into the
/// [`FormEvent`] it should produce: `Enter` always commits and drops the
/// cached editor for `id` (this primitive has no multiline mode to
/// special-case — an app that wants a literal newline needs a different
/// key, since `Enter` here always means "done"), `Ctrl`/`Cmd`+`A`/`Z`
/// resolve to this primitive's own `SelectAll`/`Undo`/`Redo` built-ins via
/// [`edit_op_from_binding`] (the exact chords
/// [`quadraui::accelerator::KeyBinding::SelectAll`]/`Undo`/`Redo` parse to,
/// so a standalone `TextInput` gets them without an app having to register
/// accelerators itself, and without drifting from those chords the way a
/// hand-rolled modifier check could), any other Ctrl/Cmd+letter chord
/// passes through unclaimed so a host's own accelerator still fires, and
/// every other key goes through [`EditOp::from_key`]/[`TextEditor::apply`].
/// Returns `None` when the key isn't one any of the above recognizes, or
/// when applying it doesn't actually change the text — the caller then
/// falls through to the raw `KeyPressed` fallback.
pub fn text_input_key_event(
    id: WidgetId,
    text_input: &TextInput,
    key: &Key,
    modifiers: Modifiers,
    text_editors: &TextEditorCache,
) -> Option<UiEvent> {
    if matches!(key, Key::Named(NamedKey::Enter)) {
        text_editors.borrow_mut().remove(&id);
        return Some(UiEvent::Form(
            id,
            FormEvent::TextInputCommitted {
                id,
                value: text_input.lines.join("\n"),
            },
        ));
    }
    if modifiers.ctrl || modifiers.cmd {
        let op = edit_op_from_binding(key, modifiers)?;
        return apply_text_input_edit(id, text_input, op, text_editors);
    }
    let op = EditOp::from_key(key, modifiers)?;
    apply_text_input_edit(id, text_input, op, text_editors)
}

/// Matches `(key, modifiers)` against [`KeyBinding::Undo`]/`Redo`/
/// `SelectAll`'s own parsed chord (`<C-z>`/`<C-S-z>`/`<C-a>` —
/// [`quadraui::accelerator::parse_binding`]) and maps a hit to the
/// [`EditOp`] it stands for via [`EditOp::from_key_binding`]. Unlike a
/// hand-rolled `Key::Char('z') | Key::Char('Z')` match, this rejects a
/// wider chord the canonical binding doesn't cover itself — `Ctrl+Shift+A`
/// (not `SelectAll`'s `<C-a>`) and `Ctrl+Alt+Z` (not `Undo`'s `<C-z>`) both
/// fall through to `None` here, the same as `preprocess_event`'s own
/// `Ctrl+A` gate (`!shift && !alt && !cmd`) and `accelerator.rs`'s own
/// definitions.
fn edit_op_from_binding(key: &Key, modifiers: Modifiers) -> Option<EditOp> {
    let key_name = key_to_binding_name(key);
    [KeyBinding::Undo, KeyBinding::Redo, KeyBinding::SelectAll]
        .into_iter()
        .find_map(|binding| {
            let parsed = parse_binding(&binding)?;
            // `parse_binding` hard-codes the primary modifier as Ctrl for
            // these universal bindings (`<C-z>`/`<C-S-z>`/`<C-a>`, i.e.
            // `cmd: false`), but the same chord on macOS arrives as a real
            // keypress with `cmd: true, ctrl: false`. Treat Ctrl and Cmd as
            // one interchangeable "primary" modifier when comparing — the
            // accelerator-registration path does the equivalent swap via
            // `macos_universal_binding_modifiers` — while still requiring
            // Shift and Alt to match exactly, so a wider chord like
            // `Ctrl+Shift+A` or `Ctrl+Alt+Z` keeps falling through unclaimed.
            let primary = modifiers.ctrl || modifiers.cmd;
            let parsed_primary = parsed.modifiers.ctrl || parsed.modifiers.cmd;
            (primary == parsed_primary
                && modifiers.shift == parsed.modifiers.shift
                && modifiers.alt == parsed.modifiers.alt
                && parsed.key == key_name)
                .then(|| EditOp::from_key_binding(&binding))?
        })
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

/// Applies `op` against the cached [`TextEditor`] session for `id` in
/// `text_editors` — creating one (seeded from `text_input`) the first time
/// `id` is seen. The cached editor's buffer/cursor/selection are
/// resynchronized to `text_input` on every call (its content is the single
/// source of truth, same as before), but the editor's own `undo_stack`
/// is left untouched, since it's a private field of `TextEditor` with no
/// equivalent on `TextInput` itself — this is what lets `EditOp::Undo`/
/// `Redo` actually have history to act on: recreating a fresh
/// `TextEditor` on every call (the previous approach) started every
/// call's `undo_stack` empty, so neither could ever do anything.
///
/// The resynchronized cursor/selection come from the *cached* editor, not
/// `text_input`'s own `cursor_line`/`cursor_col` — those are only ever the
/// position the binding's own event handler last echoed back (typically
/// 0/0, since `FormEvent::TextInputChanged`/`Committed` carry no cursor for
/// a host to echo). Without the cache, every keystroke would restart from
/// the same snapshot position and (for anything but a single appended
/// character) corrupt the text.
///
/// The resulting editor is written back to `text_editors` unconditionally
/// — even when the text itself didn't change, e.g. a pure cursor move or a
/// `SelectAll` — so the *next* keystroke continues from it; this mirrors
/// `compose::FormController::handle_text_op`'s `self.text_edit = Some(TextEditState
/// { .. })`, including its degenerate-anchor cleanup for a no-op selection
/// extend. Returns `None` when the resulting text is unchanged (a pure
/// cursor move, a selection, or a key `EditOp::from_key` mapped to a
/// no-op) — the caller then falls through to the raw `KeyPressed`
/// fallback.
pub fn apply_text_input_edit(
    id: WidgetId,
    text_input: &TextInput,
    op: EditOp,
    text_editors: &TextEditorCache,
) -> Option<UiEvent> {
    let before = text_input.lines.join("\n");
    let mut editors = text_editors.borrow_mut();
    let mut editor = editors
        .remove(&id)
        .unwrap_or_else(|| TextEditor::new(text_input.clone()));

    // Preserve the cached cursor/selection across the resync below — see
    // this function's own doc for why they (not `text_input`'s own
    // `cursor_line`/`cursor_col`) are the ones that matter.
    let prev_cursor = (editor.input.cursor_line, editor.input.cursor_col);
    let prev_anchor = editor.selection_anchor();

    editor.input = text_input.clone();
    let (seed_line, seed_col) = clamp_cursor(prev_cursor.0, prev_cursor.1, &editor.input.lines);
    editor.input.cursor_line = seed_line;
    editor.input.cursor_col = seed_col;
    let seed_anchor = prev_anchor.map(|(al, ac)| clamp_cursor(al, ac, &editor.input.lines));
    editor.set_selection_anchor(seed_anchor);

    editor.apply(op);
    let after = editor.input.lines.join("\n");

    let new_line = editor.input.cursor_line;
    let new_col = editor.input.cursor_col;
    // `TextEditor::apply`'s `MoveLeft`/`MoveRight`/etc with `extend: true`
    // can leave a degenerate `anchor == cursor` selection in place for a
    // no-op extend; left uncleared, the next keystroke would see a phantom
    // one-character-wide selection. See
    // `compose::FormController::handle_text_op`'s identical cleanup.
    if editor.selection_anchor() == Some((new_line, new_col)) {
        editor.set_selection_anchor(None);
    }
    editors.insert(id, editor);
    drop(editors);

    if after == before {
        return None;
    }
    Some(UiEvent::Form(
        id,
        FormEvent::TextInputChanged { id, value: after },
    ))
}

/// Resolves a `CharTyped`/`ClipboardPaste` landing on the currently focused
/// `TextInput`/`PasswordInput`/`TextArea` extra (via
/// [`extra_as_text_input`]) into the [`FormEvent::TextInputChanged`] the
/// insert produced. Returns `None` when nothing is focused, the focused
/// widget is none of those three, or it has scrolled out of this frame's
/// hit map. A [`PasswordInput`] strips any `'\n'` out of the inserted text
/// first — [`PasswordInput::as_form`]'s doc keeps `inner` to exactly one
/// line, and unlike `Enter` (caught earlier by
/// [`text_input_key_event`]'s own commit arm before `EditOp::from_key` is
/// even reached), nothing upstream of this call stops an IME commit or a
/// multi-line clipboard paste from inserting one.
pub fn text_input_insert_event(
    focused: WidgetId,
    hit_map: &[PaintedHit],
    op: EditOp,
    text_editors: &TextEditorCache,
) -> Option<UiEvent> {
    let (text_input, is_password) = hit_map.iter().find_map(|hit| match hit {
        PaintedHit::Extra { id, extra, .. } if *id == focused => {
            extra_as_text_input(extra).map(|ti| (ti, extra_as::<PasswordInput>(extra).is_some()))
        }
        _ => None,
    })?;
    let op = match (is_password, op) {
        (true, EditOp::InsertChar('\n')) => return None,
        (true, EditOp::InsertText(text)) => EditOp::InsertText(text.replace(['\n', '\r'], "")),
        (_, op) => op,
    };
    apply_text_input_edit(focused, text_input, op, text_editors)
}

/// Resolves a `MouseDown` landing on a standalone `TextInput`/
/// `PasswordInput`/`TextArea` extra's (via [`extra_as_text_input`])
/// [`crate::hit::HitDetail::ExtraBody`] into an [`EditOp::SetCursor`]
/// placing the caret under the click, via [`quadraui::TextInput::layout`]'s
/// own metrics-aware hit-test — the same per-click resolution
/// [`EditOp::SetCursor`]'s own doc names as its intended source. `measure`
/// must come from the live backend (`Backend::measure`), not a cached
/// value, since GTK/win/macos char width can change with the font/DPI.
///
/// A `SetCursor` never itself changes the buffer text, so the returned
/// `Option<UiEvent>` is always `None` in practice — the point of calling
/// this is its side effect on `text_editors`: it seeds the cache so the
/// *next* keystroke on this widget starts from the clicked position
/// instead of wherever the cursor was left after the previous edit (or
/// `(0, 0)` if this is the first interaction). Returns `None` outright
/// (no cache write) when `id` isn't painted as one of those three extras
/// this frame.
pub fn text_input_click_event(
    hit_map: &[PaintedHit],
    id: WidgetId,
    point: Point,
    measure: TextInputMeasure,
    text_editors: &TextEditorCache,
) -> Option<UiEvent> {
    let (area, text_input) = hit_map.iter().find_map(|hit| match hit {
        PaintedHit::Extra {
            id: hit_id,
            area,
            extra,
            ..
        } if *hit_id == id => extra_as_text_input(extra).map(|ti| (*area, ti)),
        _ => None,
    })?;
    let layout = text_input.layout(area, measure);
    let line_idx = match layout.hit_test(point.x, point.y) {
        TextInputHit::Line { line_idx } => line_idx,
        // `EmptyArea` covers both a click below the last visible line
        // (inside `content_bounds`, just past where lines run out) and
        // one on the border/padding outside `content_bounds` entirely —
        // nearest visible line by vertical distance handles both: past
        // the last line that's the last one (as before), but a click
        // above the first line (e.g. the top border on a multi-line box)
        // now lands on the first line instead of jumping to the last.
        TextInputHit::EmptyArea => layout
            .visible_lines
            .iter()
            .min_by(|a, b| {
                let da = (a.bounds.y - point.y).abs();
                let db = (b.bounds.y - point.y).abs();
                da.total_cmp(&db)
            })
            .map(|vl| vl.line_idx)
            .unwrap_or_else(|| text_input.lines.len().saturating_sub(1)),
    };
    let line_len = text_input
        .lines
        .get(line_idx)
        .map_or(0, |l| l.chars().count());
    let char_w = measure.char_width.max(1.0);
    let raw_col = ((point.x - layout.content_bounds.x) / char_w)
        .round()
        .max(0.0) as usize;
    let col = (raw_col + layout.resolved_scroll_col).min(line_len);
    apply_text_input_edit(
        id,
        text_input,
        EditOp::SetCursor {
            line: line_idx,
            col,
            extend: false,
        },
        text_editors,
    )
}
