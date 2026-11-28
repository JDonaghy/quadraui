//! Semantic event synthesis: turns a raw pointer hit ([`semantic_event_for_hit`])
//! or a focus-scoped key press ([`focus_scoped_key_event`]) into the
//! leaf-specific `UiEvent` variant a binding's handler would otherwise have
//! to re-derive from the raw `MouseDown`/`KeyPressed` plus a `"hit"`
//! payload itself.

use quadraui::{
    DataTableEvent, Key, ListViewEvent, Modifiers, NamedKey, TextDisplayEvent, TreeEvent, UiEvent,
    WidgetId,
};

use crate::drag::tree_row_path;
use crate::hit::{HitDetail, PaintedHit};

/// Up/Down navigation for a focused `DataTable`: moves the current row
/// selection by one, clamped to `[0, total_rows - 1]` and starting at row 0
/// when nothing is selected yet. Enter activates the currently selected row
/// — the keyboard equivalent of a pointer double-click — and is a no-op
/// (`None`) when nothing is selected, since there is no row to activate.
/// Every other key returns `None`, leaving [`focus_scoped_key_event`] to
/// fall through to the raw `UiEvent::KeyPressed` — `DataTable` has no
/// focus-scoped `KeyPressed` variant of its own (unlike `Tree`/`List`/
/// `TabBar`) because `DataTableEvent::RowSelected`/`RowActivated` already
/// are the semantic shapes arrow-key navigation and Enter need; a
/// widget-scoped `KeyPressed` would just be an extra hop a binding's own
/// handler would have to unwrap before reaching the same `key` string.
fn data_table_key_event(
    id: WidgetId,
    key: &Key,
    selected_idx: Option<usize>,
    total_rows: usize,
) -> Option<UiEvent> {
    if total_rows == 0 {
        return None;
    }
    if matches!(key, Key::Named(NamedKey::Enter)) {
        let idx = selected_idx?;
        return Some(UiEvent::DataTable(id, DataTableEvent::RowActivated { idx }));
    }
    let idx = match key {
        Key::Named(NamedKey::Down) => selected_idx.map_or(0, |i| (i + 1).min(total_rows - 1)),
        Key::Named(NamedKey::Up) => selected_idx.map_or(0, |i| i.saturating_sub(1)),
        _ => return None,
    };
    Some(UiEvent::DataTable(id, DataTableEvent::RowSelected { idx }))
}

/// Enter on a focused `List` with at least one row activates the current
/// selection — the keyboard equivalent of [`data_table_key_event`]'s own
/// Enter handling, and of a `List` row's pointer double-click
/// (`ListViewEvent::ItemDoubleClicked`). `selected_idx` is always a valid
/// index into a non-empty `ListView::items` (the field has no `None`
/// state), so the only case with nothing to activate is an empty list
/// (`total_items == 0`), which returns `None` and leaves
/// [`focus_scoped_key_event`] to fall through to the raw
/// `ListViewEvent::KeyPressed`. Every other key also returns `None` for the
/// same fallthrough — in particular Up/Down/vim `j`/`k`, which move the
/// `List`'s row cursor inside [`crate::host::apply_list_nav_key`]'s own
/// `WidgetStateStore` entry rather than through an event. A binding's handler
/// therefore keeps seeing navigation keys as `ListViewEvent::KeyPressed`,
/// which is what an app that tracks its own `ListView::selected_idx`
/// (the controlled shape, since `selected_idx` is a required descriptor
/// field) needs in order to move that selection itself.
fn list_key_event(
    id: WidgetId,
    key: &Key,
    selected_idx: usize,
    total_items: usize,
) -> Option<UiEvent> {
    if total_items == 0 {
        return None;
    }
    if matches!(key, Key::Named(NamedKey::Enter)) {
        return Some(UiEvent::List(
            id,
            ListViewEvent::ItemActivated { idx: selected_idx },
        ));
    }
    None
}

/// The semantic [`UiEvent`] a click on `detail` (within primitive `id`)
/// stands for, if any — the set the binding's own event handler synthesizes so a
/// tree-row/chevron click, list-item click or data-table-row/header click
/// reaches the binding's own event handler as the same shape it would get by
/// pushing that event directly (`UiEvent::Tree`/`UiEvent::List`/`UiEvent::DataTable`),
/// rather than only as a raw `MouseDown`/`DoubleClick` plus a `"hit"` dict
/// the handler would have to re-interpret itself. `double_click` selects
/// between a variant's single- and double-click shapes where both exist
/// (e.g. `TreeEvent::RowClicked` vs `RowDoubleClicked`,
/// `ListViewEvent::SelectionChanged` vs `ItemDoubleClicked`,
/// `DataTableEvent::RowSelected` vs `RowDoubleClicked`). `None` for every
/// other `HitDetail` — a data-table header divider or a split divider
/// (handled separately, via [`crate::drag::ActiveDrag`], since either spans
/// a drag rather than a single click), or a modified list-item click (see the `ListItem`
/// arm below, which loses the gesture if folded into
/// `ListViewEvent::SelectionChanged`) — for which the raw
/// `MouseDown`/`DoubleClick` and its `"hit"` dict reach the binding's own
/// event handler unchanged.
pub fn semantic_event_for_hit(
    id: WidgetId,
    detail: &HitDetail,
    modifiers: Modifiers,
    double_click: bool,
    hit_map: &[PaintedHit],
) -> Option<UiEvent> {
    match detail {
        HitDetail::TreeRow { row } => {
            let path = tree_row_path(hit_map, id, *row)?;
            if double_click {
                Some(UiEvent::Tree(id, TreeEvent::RowDoubleClicked { path }))
            } else {
                Some(UiEvent::Tree(id, TreeEvent::RowClicked { path, modifiers }))
            }
        }
        HitDetail::TreeChevron { row } => {
            let path = tree_row_path(hit_map, id, *row)?;
            Some(UiEvent::Tree(id, TreeEvent::RowToggleExpand { path }))
        }
        HitDetail::ListItem { index } => {
            if double_click {
                Some(UiEvent::List(
                    id,
                    ListViewEvent::ItemDoubleClicked { idx: *index },
                ))
            } else if modifiers == Modifiers::default() {
                Some(UiEvent::List(
                    id,
                    ListViewEvent::SelectionChanged { idx: *index },
                ))
            } else {
                // `ListViewEvent::SelectionChanged` carries no modifiers, so a
                // Ctrl/Shift/Cmd click (a multi-select gesture) can't be
                // represented by it without losing which gesture it was.
                // Leaving the raw `MouseDown` (with its `"hit"` dict still
                // attached, since this returns `None`) lets the binding's
                // own event handler read `event["modifiers"]` and
                // `event["hit"]["index"]` and compute the selection itself,
                // the same way it would for any other modified click.
                None
            }
        }
        HitDetail::DataTableHeader { col } => Some(UiEvent::DataTable(
            id,
            DataTableEvent::HeaderClicked { col: *col },
        )),
        HitDetail::DataTableRow { row } => {
            if double_click {
                Some(UiEvent::DataTable(
                    id,
                    DataTableEvent::RowDoubleClicked { idx: *row },
                ))
            } else {
                Some(UiEvent::DataTable(
                    id,
                    DataTableEvent::RowSelected { idx: *row },
                ))
            }
        }
        _ => None,
    }
}

/// Wire-shaped drag-gesture event [`crate::engine::InteractionEngine::handle`]
/// dispatches through [`crate::engine::EventSink::dispatch_drag_gesture`],
/// a default method on that trait: it defaults to ignoring the event and
/// reporting [`quadraui::Reaction::Continue`], so every existing
/// [`crate::engine::EventSink`] impl keeps compiling unchanged until a
/// binding deliberately overrides it to thread this to its own callback.
/// Derives `Serialize` only, not `Deserialize`: [`HitDetail`] — the shape
/// every `*_detail` field below reuses rather than inventing a second
/// drag-specific hit description — has no `Deserialize` of its own either,
/// since neither type has a reverse (host -> engine) decode path.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DragGestureWireEvent {
    /// A `MouseDown` landed on a leaf
    /// [`crate::engine::InteractionEngine::drag_roles`] marks
    /// [`crate::wire::DragRole::draggable`] — the gesture's own arming
    /// moment. Dispatched before the leaf's own ordinary click/selection
    /// semantics (if any) also fire through [`crate::engine::EventSink::dispatch`]
    /// for the same `MouseDown` — the two are independent effects of one
    /// press, not alternatives.
    DragStarted {
        source: WidgetId,
        source_detail: HitDetail,
    },
    /// The pointer moved onto a NEW leaf `drag_roles` marks
    /// [`crate::wire::DragRole::drop_target`] while the gesture above is
    /// still armed — dispatched once per target change (including back to
    /// the same leaf under a different [`HitDetail`], e.g. a different
    /// `List` row), never on every `MouseMoved` pixel that lands on the
    /// unchanged target.
    DragOver {
        source: WidgetId,
        source_detail: HitDetail,
        target: WidgetId,
        target_detail: HitDetail,
    },
    /// The gesture ended (`MouseUp`) while hovering a valid drop target —
    /// the host applies whatever mutation `source`/`source_detail` dropped
    /// onto `target`/`target_detail` stands for (e.g. reordering a
    /// `List`'s own backing collection from `source_detail`'s row index to
    /// `target_detail`'s). A `MouseUp` with no drop target under the
    /// pointer ends the gesture silently instead — no `Dropped` is
    /// dispatched at all.
    Dropped {
        source: WidgetId,
        source_detail: HitDetail,
        target: WidgetId,
        target_detail: HitDetail,
    },
}

/// Resolves a key focused on a `TreeView` row that is mid-rename through
/// core's single-line rename editor ([`quadraui::primitives::tree::tree_rename_key`]):
/// Enter commits, Escape cancels, and an edit or caret move reports the
/// new session as `RowRenameChanged`. `None` for a key the editor leaves
/// alone, so the caller falls through to the tree's raw `KeyPressed`.
pub(crate) fn tree_rename_event(
    id: WidgetId,
    path: &quadraui::TreePath,
    edit: &quadraui::TreeRowEditState,
    key: &Key,
    modifiers: Modifiers,
) -> Option<UiEvent> {
    use quadraui::primitives::tree::{tree_rename_key, TreeRenameKey};
    let event = match tree_rename_key(edit, key, modifiers) {
        TreeRenameKey::Commit => TreeEvent::RowRenameCommitted {
            path: path.clone(),
            text: edit.text.clone(),
        },
        TreeRenameKey::Cancel => TreeEvent::RowRenameCancelled { path: path.clone() },
        TreeRenameKey::Edited(next) | TreeRenameKey::Moved(next) => TreeEvent::RowRenameChanged {
            path: path.clone(),
            text: next.text,
            cursor: next.cursor,
            selection_anchor: next.selection_anchor,
        },
        TreeRenameKey::Unhandled => return None,
    };
    Some(UiEvent::Tree(id, event))
}

/// Stringifies a raw key the same way every quadraui backend does, so a
/// `*Event::KeyPressed` variant built here carries the identical `key`
/// string an app would see from the TUI, GTK or macOS backend directly.
pub fn key_to_focus_string(key: &Key) -> String {
    match key {
        Key::Char(c) => c.to_string(),
        Key::Named(named) => named.name().into_owned(),
        // `Key` is `#[non_exhaustive]`; no variant beyond `Char`/`Named`
        // exists today, but a future one needs a string here too rather
        // than silently losing the keystroke.
        _ => String::new(),
    }
}

/// Routes a raw `KeyPressed` to whichever leaf the `FocusTree` names as
/// focused, wrapping it into that leaf's own semantic `KeyPressed` variant
/// (or, for a focused `DataTable`'s Up/Down/Enter, straight into
/// [`data_table_key_event`]'s `RowSelected`/`RowActivated`; or, for a
/// focused `List`'s Enter with at least one row, straight into
/// [`list_key_event`]'s `ItemActivated`). A focused
/// [`crate::widget_kinds::WIDGET_KINDS`] leaf (e.g. `Button`, `Switch`) never
/// reaches this function at all — `InteractionEngine::handle` claims its
/// `KeyPressed` earlier, through `DynWidget::handle` directly, the same
/// generic seam its `MouseDown`/`DoubleClick` routing already goes through.
/// Every other key a focused leaf doesn't recognize passes through as the
/// original `UiEvent::KeyPressed` unchanged; the same happens for any other
/// key a focused `DataTable`/`List` doesn't recognize, and when `focused`
/// names a leaf absent from this frame's hit map (for example a tree that
/// scrolled out of the render).
pub fn focus_scoped_key_event(
    focused: WidgetId,
    raw_key: &Key,
    modifiers: Modifiers,
    hit_map: &[PaintedHit],
) -> UiEvent {
    let fallback = UiEvent::KeyPressed {
        key: raw_key.clone(),
        modifiers,
        repeat: false,
    };
    let key = key_to_focus_string(raw_key);
    hit_map
        .iter()
        .find_map(|hit| match hit {
            PaintedHit::Tree { id, editing, .. } if *id == focused => {
                if let Some((path, edit)) = editing {
                    if let Some(ev) = tree_rename_event(*id, path, edit, raw_key, modifiers) {
                        return Some(ev);
                    }
                }
                Some(UiEvent::Tree(
                    *id,
                    TreeEvent::KeyPressed {
                        key: key.clone(),
                        modifiers,
                    },
                ))
            }
            PaintedHit::List {
                id,
                selected_idx,
                total_items,
                ..
            } if *id == focused => list_key_event(*id, raw_key, *selected_idx, *total_items)
                .or_else(|| {
                    Some(UiEvent::List(
                        *id,
                        ListViewEvent::KeyPressed {
                            key: key.clone(),
                            modifiers,
                        },
                    ))
                }),
            PaintedHit::TextDisplay { id, .. } if *id == focused => Some(UiEvent::TextDisplay(
                *id,
                TextDisplayEvent::KeyPressed {
                    key: key.clone(),
                    modifiers,
                },
            )),
            PaintedHit::DataTable {
                id,
                selected_idx,
                total_rows,
                ..
            } if *id == focused => data_table_key_event(*id, raw_key, *selected_idx, *total_rows),
            _ => None,
        })
        .unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rename_session(text: &str, cursor: usize) -> quadraui::TreeRowEditState {
        quadraui::TreeRowEditState {
            text: text.to_string(),
            cursor,
            selection_anchor: None,
            placeholder: None,
        }
    }

    #[test]
    fn rename_typing_inserts_at_the_caret() {
        let path = vec![0u16];
        let event = tree_rename_event(
            WidgetId::new("t"),
            &path,
            &rename_session("ac", 1),
            &Key::Char('b'),
            Modifiers::default(),
        );
        assert_eq!(
            event,
            Some(UiEvent::Tree(
                WidgetId::new("t"),
                TreeEvent::RowRenameChanged {
                    path,
                    text: "abc".to_string(),
                    cursor: 2,
                    selection_anchor: None,
                }
            ))
        );
    }

    /// Ctrl+A selects the whole name, the same chord every other text
    /// field honours.
    #[test]
    fn rename_ctrl_a_selects_the_whole_name() {
        let path = vec![0u16];
        let event = tree_rename_event(
            WidgetId::new("t"),
            &path,
            &rename_session("abc", 1),
            &Key::Char('a'),
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
        );
        assert_eq!(
            event,
            Some(UiEvent::Tree(
                WidgetId::new("t"),
                TreeEvent::RowRenameChanged {
                    path,
                    text: "abc".to_string(),
                    cursor: 3,
                    selection_anchor: Some(0),
                }
            ))
        );
    }

    #[test]
    fn rename_enter_commits_and_escape_cancels() {
        let path = vec![0u16];
        let session = rename_session("abc", 1);
        let id = WidgetId::new("t");
        assert_eq!(
            tree_rename_event(
                id,
                &path,
                &session,
                &Key::Named(NamedKey::Enter),
                Modifiers::default()
            ),
            Some(UiEvent::Tree(
                id,
                TreeEvent::RowRenameCommitted {
                    path: path.clone(),
                    text: "abc".to_string()
                }
            ))
        );
        assert_eq!(
            tree_rename_event(
                id,
                &path,
                &session,
                &Key::Named(NamedKey::Escape),
                Modifiers::default()
            ),
            Some(UiEvent::Tree(id, TreeEvent::RowRenameCancelled { path }))
        );
    }

    /// `key_to_focus_string` defers to the shared `NamedKey::name` table,
    /// so an F13+ press stringifies to `"F13"` rather than any
    /// binding-core-local spelling drifting from the other three call
    /// sites that stringify the same `NamedKey`.
    #[test]
    fn key_to_focus_string_matches_the_shared_named_key_table() {
        assert_eq!(key_to_focus_string(&Key::Named(NamedKey::Escape)), "Escape");
        assert_eq!(key_to_focus_string(&Key::Named(NamedKey::F(13))), "F13");
        assert_eq!(key_to_focus_string(&Key::Char('a')), "a");
    }
}
