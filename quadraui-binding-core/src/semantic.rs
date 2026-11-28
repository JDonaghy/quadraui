//! Semantic event synthesis: turns a raw pointer hit ([`semantic_event_for_hit`])
//! or a focus-scoped key press ([`focus_scoped_key_event`]) into the
//! leaf-specific `UiEvent` variant a binding's handler would otherwise have
//! to re-derive from the raw `MouseDown`/`KeyPressed` plus a `"hit"`
//! payload itself.

use quadraui::{
    DataTableEvent, Key, ListViewEvent, Modifiers, MouseButton, NamedKey, Point, SelectionCore,
    SelectionMode, TextDisplayEvent, TreeEvent, UiEvent, WidgetId,
};

use crate::drag::tree_row_path;
use crate::hit::{HitDetail, PaintedHit};

/// Each `DataTable`/`List`/`Tree` leaf's [`SelectionCore`], by widget id —
/// what a widget keeps in its own `Interactive::State`.
pub type Selections = std::collections::HashMap<WidgetId, SelectionCore>;

/// A focused `DataTable`'s keys: Up/Down move the selected row (clamped,
/// from row 0 when none), Enter activates it, and with `multi` (the painted
/// `selected_indices`, `Some` only in [`SelectionMode::Multi`]) Shift+Up/Down
/// and [`SelectionCore::key`]'s chords report `SelectionSet`. `None` for any
/// other key: the raw `UiEvent::KeyPressed` passes through.
fn data_table_key_event(
    id: WidgetId,
    (key, modifiers): (&Key, Modifiers),
    (selected_idx, total_rows): (Option<usize>, usize),
    multi: Option<&[usize]>,
    selection: &mut SelectionCore,
) -> Option<UiEvent> {
    if total_rows == 0 {
        return None;
    }
    let set = |indices| {
        Some(UiEvent::DataTable(
            id,
            DataTableEvent::SelectionSet { indices },
        ))
    };
    if let Some(painted) = multi {
        selection.sync_painted(painted);
        let chord = selection.key(
            SelectionMode::Multi,
            key,
            modifiers,
            selected_idx,
            total_rows,
        );
        if chord.is_some() {
            return set(chord?);
        }
    }
    if matches!(key, Key::Named(NamedKey::Enter)) {
        let idx = selected_idx?;
        return Some(UiEvent::DataTable(id, DataTableEvent::RowActivated { idx }));
    }
    let delta = match key {
        Key::Named(NamedKey::Down) => 1,
        Key::Named(NamedKey::Up) => -1,
        _ => return None,
    };
    if modifiers.shift && multi.is_some() {
        return set(selection.extend_by(selected_idx, delta, total_rows));
    }
    let idx = selected_idx.map_or(0, |i| i.saturating_add_signed(delta).min(total_rows - 1));
    Some(UiEvent::DataTable(id, DataTableEvent::RowSelected { idx }))
}

/// Enter on a focused, non-empty `List` activates its selection (the
/// keyboard twin of `ItemDoubleClicked`), and [`SelectionCore::key`]'s
/// multi-select chords report `SelectionSet` — a `List` always takes them,
/// as its own `Interactive` impl does. Every other key returns `None` for
/// [`focus_scoped_key_event`]'s raw `ListViewEvent::KeyPressed` fallthrough;
/// Up/Down/`j`/`k` move the cursor in [`crate::host::apply_list_nav_key`]'s
/// store, so an app tracking its own `selected_idx` sees them as keys.
fn list_key_event(
    id: WidgetId,
    (key, modifiers): (&Key, Modifiers),
    selected_idx: usize,
    total_items: usize,
    selection: &mut SelectionCore,
) -> Option<UiEvent> {
    if total_items == 0 {
        return None;
    }
    if let Some(indices) = selection.key(
        SelectionMode::Multi,
        key,
        modifiers,
        Some(selected_idx),
        total_items,
    ) {
        return Some(UiEvent::List(id, ListViewEvent::SelectionSet { indices }));
    }
    if matches!(key, Key::Named(NamedKey::Enter)) {
        return Some(UiEvent::List(
            id,
            ListViewEvent::ItemActivated { idx: selected_idx },
        ));
    }
    None
}

/// One pointer press, as [`semantic_event_for_hit`] resolves it.
#[derive(Debug, Clone, Copy)]
pub struct Press {
    pub position: Point,
    pub button: MouseButton,
    pub modifiers: Modifiers,
    /// The press is the second of a quick pair (`UiEvent::DoubleClick`).
    pub double_click: bool,
}

/// The selection mode, painted multi-selection and cursor row of the
/// row leaf `id` in `hit_map` — what [`SelectionCore::press`] needs.
fn row_selection(hit_map: &[PaintedHit], id: WidgetId) -> (SelectionMode, &[usize], Option<usize>) {
    use PaintedHit as H;
    let none: &[usize] = &[];
    let found = hit_map.iter().find_map(|hit| match hit {
        H::DataTable {
            id: h,
            multi,
            selected_idx: at,
            ..
        } if *h == id => match multi {
            Some(set) => Some((SelectionMode::Multi, &set[..], *at)),
            None => Some((SelectionMode::Single, none, *at)),
        },
        H::List {
            id: h,
            selected_idx: at,
            ..
        } if *h == id => Some((SelectionMode::Multi, none, Some(*at))),
        H::Tree {
            id: h,
            selection_mode: mode,
            ..
        } if *h == id => Some((*mode, none, None)),
        _ => None,
    });
    found.unwrap_or((SelectionMode::Single, none, None))
}

/// The semantic [`UiEvent`] a press on `detail` (within primitive `id`)
/// stands for, if any, so a tree-row/chevron, list-item or data-table
/// row/header press reaches the binding's handler in the shape it would
/// get by pushing that event directly, not as a raw `MouseDown` plus a
/// `"hit"` dict. A left double-click picks the double-click shape
/// (`RowDoubleClicked`, `ItemDoubleClicked`); every other row press
/// resolves through [`SelectionCore::press`] on `id`'s entry in
/// `selections` — select, grow a multi-selection (`SelectionSet`), ask
/// for the context menu (right), or report the button (`RowClicked`,
/// `ItemClicked`). Only a left press toggles a chevron or clicks a header.
/// `None` for every other `HitDetail` (dividers arm drags through
/// [`crate::drag::ActiveDrag`]), leaving the raw event and its `"hit"`.
pub fn semantic_event_for_hit(
    id: WidgetId,
    detail: &HitDetail,
    press: Press,
    hit_map: &[PaintedHit],
    selections: &mut Selections,
) -> Option<UiEvent> {
    let left = press.button == MouseButton::Left;
    let double = press.double_click && left;
    let (mode, painted, current) = row_selection(hit_map, id);
    let mut resolve = |idx: usize| {
        let selection = selections.entry(id).or_default();
        selection.sync_painted(painted);
        selection.press(mode, current, idx, press.button, press.modifiers)
    };
    let (mods, at) = (press.modifiers, press.position);
    match detail {
        HitDetail::TreeChevron { row } if left => {
            let path = tree_row_path(hit_map, id, *row)?;
            Some(UiEvent::Tree(id, TreeEvent::RowToggleExpand { path }))
        }
        HitDetail::TreeRow { row } | HitDetail::TreeChevron { row } => {
            let path = tree_row_path(hit_map, id, *row)?;
            if double {
                return Some(UiEvent::Tree(id, TreeEvent::RowDoubleClicked { path }));
            }
            let paths = |rows: &[usize]| {
                rows.iter()
                    .filter_map(|&r| tree_row_path(hit_map, id, r))
                    .collect()
            };
            let event = TreeEvent::from_row_press(resolve(*row), path, mods, at, paths);
            Some(UiEvent::Tree(id, event))
        }
        HitDetail::ListItem { index } if double => Some(UiEvent::List(
            id,
            ListViewEvent::ItemDoubleClicked { idx: *index },
        )),
        HitDetail::ListItem { index } => {
            let event = ListViewEvent::from_row_press(resolve(*index), *index, mods, at);
            Some(UiEvent::List(id, event))
        }
        HitDetail::DataTableHeader { col } if left => Some(UiEvent::DataTable(
            id,
            DataTableEvent::HeaderClicked { col: *col },
        )),
        HitDetail::DataTableRow { row } if double => Some(UiEvent::DataTable(
            id,
            DataTableEvent::RowDoubleClicked { idx: *row },
        )),
        HitDetail::DataTableRow { row } => {
            let event = DataTableEvent::from_row_press(resolve(*row), *row, mods, at);
            Some(UiEvent::DataTable(id, event))
        }
        _ => None,
    }
}

/// Wire-shaped drag-gesture event [`crate::engine::InteractionEngine::handle`]
/// dispatches through [`crate::engine::EventSink::dispatch_drag_gesture`]
/// (which by default ignores it, returning `Continue`, until a binding
/// overrides it). `Serialize` only: [`HitDetail`], which every `*_detail`
/// field reuses, has no reverse (host -> engine) decode path either.
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

/// Routes a raw `KeyPressed` to the focused leaf as that leaf's semantic
/// event: its own `KeyPressed` variant, or [`data_table_key_event`]'s /
/// [`list_key_event`]'s selection and activation events, the multi-select
/// ones growing `id`'s entry in `selections`. A focused
/// [`crate::widget_kinds::WIDGET_KINDS`] leaf never reaches here
/// (`InteractionEngine::handle` routes it through `DynWidget::handle`).
/// An unrecognised key, or a `focused` id absent from this frame's hit
/// map, passes through as the original `UiEvent::KeyPressed`.
pub fn focus_scoped_key_event(
    focused: WidgetId,
    raw_key: &Key,
    modifiers: Modifiers,
    hit_map: &[PaintedHit],
    selections: &mut Selections,
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
            } if *id == focused => list_key_event(
                *id,
                (raw_key, modifiers),
                *selected_idx,
                *total_items,
                selections.entry(*id).or_default(),
            )
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
                multi,
                ..
            } if *id == focused => data_table_key_event(
                *id,
                (raw_key, modifiers),
                (*selected_idx, *total_rows),
                multi.as_deref(),
                selections.entry(*id).or_default(),
            ),
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
