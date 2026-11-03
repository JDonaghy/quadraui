//! The inline-rename controller for a `TreeView` row being edited
//! (`TreeRow::edit`): turns a raw key into the next
//! [`TreeRowEditState`] it should hold, so a binding doesn't have to
//! reimplement cursor/selection math itself for every keystroke. Unlike
//! [`crate::text_input::TextEditorCache`], no per-widget cache is needed
//! here — the rename's cursor/selection round-trips through
//! `TreeEvent::RowRenameChanged` and back onto the row's own `edit` field,
//! since that event (unlike `FormEvent::TextInputChanged`) carries them.

use quadraui::{
    next_char_boundary, prev_char_boundary, snap_to_char_boundary, Key, Modifiers, NamedKey,
    TreeEvent, TreePath, TreeRowEditState, UiEvent, WidgetId,
};

/// Removes the current `[lo, hi)` selection (if any and non-empty) from
/// `text`, moving `cursor` to its start and clearing `anchor`. Returns
/// whether a selection was actually removed — callers use this to decide
/// whether a following single-character delete/insert should still run
/// (an `InsertChar` always replaces a non-empty selection; a `Backspace`/
/// `Delete` with a non-empty selection removes just the selection and
/// skips its own char-removal step).
fn delete_selection(text: &mut String, cursor: &mut usize, anchor: &mut Option<usize>) -> bool {
    let Some(a) = *anchor else { return false };
    if a == *cursor {
        return false;
    }
    let lo = snap_to_char_boundary(text, a.min(*cursor));
    let hi = snap_to_char_boundary(text, a.max(*cursor));
    text.replace_range(lo..hi, "");
    *cursor = lo;
    *anchor = None;
    true
}

/// Applies one key to `edit`, returning the [`TreeRowEditState`] it leaves
/// behind — the new text, cursor, and selection anchor — or `None` when
/// the key is `Enter`/`Escape` (handled by [`tree_row_rename_key_event`]
/// itself, not here), a Ctrl/Cmd+letter chord (left unclaimed so a host's
/// accelerator still fires, mirroring `text_input::text_input_key_event`'s
/// identical carve-out), an unrecognized key, or a key that would leave
/// every field exactly as it already was (e.g. `Left` at column 0 with no
/// selection to drop). Byte offsets throughout, like
/// [`TreeRowEditState::cursor`]/`selection_anchor` themselves — no
/// char-column conversion is needed, unlike the multi-line, char-column
/// `TextInput` this mirrors.
fn apply_rename_key(
    edit: &TreeRowEditState,
    key: &Key,
    modifiers: &Modifiers,
) -> Option<TreeRowEditState> {
    let mut text = edit.text.clone();
    let mut cursor = snap_to_char_boundary(&text, edit.cursor.min(text.len()));
    let mut anchor = edit
        .selection_anchor
        .map(|a| snap_to_char_boundary(&text, a.min(text.len())));
    let extend = modifiers.shift;

    match key {
        Key::Named(NamedKey::Backspace) => {
            if !delete_selection(&mut text, &mut cursor, &mut anchor) {
                if cursor == 0 {
                    return None;
                }
                let prev = prev_char_boundary(&text, cursor);
                text.replace_range(prev..cursor, "");
                cursor = prev;
                anchor = None;
            }
        }
        Key::Named(NamedKey::Delete) => {
            if !delete_selection(&mut text, &mut cursor, &mut anchor) {
                if cursor >= text.len() {
                    return None;
                }
                let next = next_char_boundary(&text, cursor);
                text.replace_range(cursor..next, "");
            }
        }
        Key::Named(NamedKey::Left) => {
            if cursor == 0 {
                if anchor.is_none() || extend {
                    return None;
                }
                anchor = None;
            } else {
                if extend && anchor.is_none() {
                    anchor = Some(cursor);
                }
                cursor = prev_char_boundary(&text, cursor);
                if !extend {
                    anchor = None;
                }
            }
        }
        Key::Named(NamedKey::Right) => {
            if cursor >= text.len() {
                if anchor.is_none() || extend {
                    return None;
                }
                anchor = None;
            } else {
                if extend && anchor.is_none() {
                    anchor = Some(cursor);
                }
                cursor = next_char_boundary(&text, cursor);
                if !extend {
                    anchor = None;
                }
            }
        }
        Key::Named(NamedKey::Home) => {
            if cursor == 0 && (anchor.is_none() || extend) {
                return None;
            }
            if extend && anchor.is_none() {
                anchor = Some(cursor);
            }
            cursor = 0;
            if !extend {
                anchor = None;
            }
        }
        Key::Named(NamedKey::End) => {
            if cursor == text.len() && (anchor.is_none() || extend) {
                return None;
            }
            if extend && anchor.is_none() {
                anchor = Some(cursor);
            }
            cursor = text.len();
            if !extend {
                anchor = None;
            }
        }
        Key::Char(_) if modifiers.ctrl || modifiers.cmd => return None,
        Key::Char(c) => {
            delete_selection(&mut text, &mut cursor, &mut anchor);
            let at = snap_to_char_boundary(&text, cursor);
            text.insert(at, *c);
            cursor = at + c.len_utf8();
            anchor = None;
        }
        _ => return None,
    }

    if text == edit.text && cursor == edit.cursor && anchor == edit.selection_anchor {
        return None;
    }
    Some(TreeRowEditState {
        text,
        cursor,
        selection_anchor: anchor,
        placeholder: edit.placeholder.clone(),
    })
}

/// Resolves a key focused on a `TreeView` whose current row (named by
/// `path`) is mid-rename (`TreeRow::edit` is `Some(edit)`) into the
/// [`TreeEvent`] it should produce: `Enter` commits (`RowRenameCommitted`),
/// `Escape` cancels (`RowRenameCancelled`), and every other recognized key
/// applies via [`apply_rename_key`] into `RowRenameChanged`. Returns `None`
/// when [`apply_rename_key`] does (see its own doc) — the caller then falls
/// through to the tree's raw `TreeEvent::KeyPressed`, the same fallback a
/// non-editing key press already gets.
pub fn tree_row_rename_key_event(
    id: WidgetId,
    path: &TreePath,
    edit: &TreeRowEditState,
    key: &Key,
    modifiers: &Modifiers,
) -> Option<UiEvent> {
    if matches!(key, Key::Named(NamedKey::Enter)) {
        return Some(UiEvent::Tree(
            id,
            TreeEvent::RowRenameCommitted {
                path: path.clone(),
                text: edit.text.clone(),
            },
        ));
    }
    if matches!(key, Key::Named(NamedKey::Escape)) {
        return Some(UiEvent::Tree(
            id,
            TreeEvent::RowRenameCancelled { path: path.clone() },
        ));
    }
    let next = apply_rename_key(edit, key, modifiers)?;
    Some(UiEvent::Tree(
        id,
        TreeEvent::RowRenameChanged {
            path: path.clone(),
            text: next.text,
            cursor: next.cursor,
            selection_anchor: next.selection_anchor,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use quadraui::Modifiers;

    fn edit(text: &str, cursor: usize, anchor: Option<usize>) -> TreeRowEditState {
        TreeRowEditState {
            text: text.to_string(),
            cursor,
            selection_anchor: anchor,
            placeholder: None,
        }
    }

    #[test]
    fn typing_a_char_inserts_at_cursor_and_advances_it() {
        let e = edit("ac", 1, None);
        let next = apply_rename_key(&e, &Key::Char('b'), &Modifiers::default()).unwrap();
        assert_eq!(next.text, "abc");
        assert_eq!(next.cursor, 2);
        assert_eq!(next.selection_anchor, None);
    }

    #[test]
    fn typing_three_chars_in_sequence_does_not_reverse_them() {
        let mut e = edit("", 0, None);
        for c in ['a', 'b', 'c'] {
            e = apply_rename_key(&e, &Key::Char(c), &Modifiers::default()).unwrap();
        }
        assert_eq!(e.text, "abc");
        assert_eq!(e.cursor, 3);
    }

    #[test]
    fn backspace_at_start_is_a_no_op() {
        let e = edit("abc", 0, None);
        assert!(
            apply_rename_key(&e, &Key::Named(NamedKey::Backspace), &Modifiers::default()).is_none()
        );
    }

    #[test]
    fn backspace_removes_a_non_empty_selection_instead_of_one_char() {
        let e = edit("abc", 2, Some(0));
        let next =
            apply_rename_key(&e, &Key::Named(NamedKey::Backspace), &Modifiers::default()).unwrap();
        assert_eq!(next.text, "c");
        assert_eq!(next.cursor, 0);
        assert_eq!(next.selection_anchor, None);
    }

    #[test]
    fn shift_left_extends_a_selection_from_the_cursor() {
        let e = edit("abc", 2, None);
        let next = apply_rename_key(
            &e,
            &Key::Named(NamedKey::Left),
            &Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        )
        .unwrap();
        assert_eq!(next.cursor, 1);
        assert_eq!(next.selection_anchor, Some(2));
    }

    #[test]
    fn ctrl_letter_is_left_unclaimed_for_a_host_accelerator() {
        let e = edit("abc", 1, None);
        assert!(apply_rename_key(
            &e,
            &Key::Char('a'),
            &Modifiers {
                ctrl: true,
                ..Modifiers::default()
            }
        )
        .is_none());
    }

    #[test]
    fn enter_commits_with_the_current_text() {
        let id = WidgetId::new("tree:main");
        let path: TreePath = vec![0];
        let e = edit("renamed", 7, None);
        let ev = tree_row_rename_key_event(
            id,
            &path,
            &e,
            &Key::Named(NamedKey::Enter),
            &Modifiers::default(),
        )
        .unwrap();
        match ev {
            UiEvent::Tree(
                got_id,
                TreeEvent::RowRenameCommitted {
                    path: got_path,
                    text,
                },
            ) => {
                assert_eq!(got_id, id);
                assert_eq!(got_path, path);
                assert_eq!(text, "renamed");
            }
            other => panic!("expected RowRenameCommitted, got {other:?}"),
        }
    }

    #[test]
    fn escape_cancels_without_carrying_the_text() {
        let id = WidgetId::new("tree:main");
        let path: TreePath = vec![0];
        let e = edit("half typed", 4, None);
        let ev = tree_row_rename_key_event(
            id,
            &path,
            &e,
            &Key::Named(NamedKey::Escape),
            &Modifiers::default(),
        )
        .unwrap();
        match ev {
            UiEvent::Tree(got_id, TreeEvent::RowRenameCancelled { path: got_path }) => {
                assert_eq!(got_id, id);
                assert_eq!(got_path, path);
            }
            other => panic!("expected RowRenameCancelled, got {other:?}"),
        }
    }
}
