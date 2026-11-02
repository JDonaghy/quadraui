//! Semantic event synthesis: turns a raw pointer hit ([`semantic_event_for_hit`])
//! or a focus-scoped key press ([`focus_scoped_key_event`]) into the
//! leaf-specific `UiEvent` variant a binding's handler would otherwise have
//! to re-derive from the raw `MouseDown`/`KeyPressed` plus a `"hit"`
//! payload itself.

use std::cell::RefCell;
use std::collections::HashMap;

use quadraui::{
    DataTableEvent, FormEvent, Key, ListViewEvent, Modifiers, NamedKey, Switch, SwitchEvent,
    TabBarEvent, TextInput, TreeEvent, UiEvent, WidgetId,
};

use crate::drag::{tab_bar_for_right_segment, tree_row_path};
use crate::extra_paint::extra_as;
use crate::hit::{HitDetail, PaintedHit};
use crate::text_input::{text_input_key_event, TextCursorState};

/// The semantic [`UiEvent`] a click on `detail` (within primitive `id`)
/// stands for, if any — the set the binding's own event handler synthesizes so a tab
/// click, tab close-button click, tree-row/chevron click, list-item click,
/// data-table-row/header click, or tab-bar button click reaches a Python
/// handler as the same shape it would get by pushing that event directly
/// (`UiEvent::TabBar`/`UiEvent::Tree`/`UiEvent::List`/`UiEvent::DataTable`),
/// rather than only as a raw `MouseDown`/`DoubleClick` plus a `"hit"` dict
/// the handler would have to re-interpret itself. `double_click` selects
/// between a variant's single- and double-click shapes where both exist
/// (e.g. `TreeEvent::RowClicked` vs `RowDoubleClicked`,
/// `ListViewEvent::SelectionChanged` vs `ItemActivated`). `None` for every
/// other `HitDetail` — a data-table header divider or a split divider
/// (handled separately, via [`ActiveDrag`], since either spans a drag rather
/// than a single click), a tab-bar scroll-left/right arrow (no semantic
/// event exists for it), or a modified list-item click (see the `ListItem`
/// arm below, which loses the gesture if folded into
/// `ListViewEvent::SelectionChanged`) — for which the raw
/// `MouseDown`/`DoubleClick` and its `"hit"` dict reach Python unchanged.
/// [`HitDetail::ExtraBody`] resolves against the matched extra's own click
/// semantics (currently [`Switch::click`]); an extra with no recognized
/// click behavior still falls through to the raw `MouseDown`.
pub fn semantic_event_for_hit(
    id: WidgetId,
    detail: &HitDetail,
    modifiers: Modifiers,
    double_click: bool,
    hit_map: &[PaintedHit],
) -> Option<UiEvent> {
    match detail {
        HitDetail::TabBarTab { tab } => Some(UiEvent::TabBar(
            id,
            TabBarEvent::TabActivated { index: *tab },
        )),
        HitDetail::TabBarTabClose { tab } => {
            Some(UiEvent::TabBar(id, TabBarEvent::TabClosed { index: *tab }))
        }
        HitDetail::TabBarRightSegment => {
            let tab_bar_id = tab_bar_for_right_segment(hit_map, id)?;
            Some(UiEvent::TabBar(
                tab_bar_id,
                TabBarEvent::ButtonClicked { id },
            ))
        }
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
                    ListViewEvent::ItemActivated { idx: *index },
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
                // attached, since this returns `None`) lets a Python handler
                // read `event["modifiers"]` and `event["hit"]["index"]` and
                // compute the selection itself, the same way it would for any
                // other modified click.
                None
            }
        }
        HitDetail::DataTableHeader { col } => Some(UiEvent::DataTable(
            id,
            DataTableEvent::HeaderClicked { col: *col },
        )),
        HitDetail::DataTableRow { row } => Some(UiEvent::DataTable(
            id,
            DataTableEvent::RowActivated { idx: *row },
        )),
        HitDetail::ExtraBody => hit_map.iter().find_map(|hit| match hit {
            PaintedHit::Extra {
                id: hit_id, extra, ..
            } if *hit_id == id => {
                let switch = extra_as::<Switch>(extra)?;
                // Route the matched hit id, not the descriptor's embedded
                // `SwitchEvent::Toggled::id` — a render callback may leave
                // the descriptor's own `id` at its default and reuse it
                // across leaves, so only the tree-address hit id is
                // guaranteed unique. Renamed rather than destructured as
                // `id` so the two can't silently shadow each other again.
                let SwitchEvent::Toggled {
                    id: _switch_id,
                    value,
                } = switch.click()?;
                Some(UiEvent::Form(id, FormEvent::ToggleChanged { id, value }))
            }
            _ => None,
        }),
        _ => None,
    }
}

/// Stringifies a raw key the same way every quadraui backend does, so a
/// `*Event::KeyPressed` variant built here carries the identical `key`
/// string an app would see from the TUI, GTK or macOS backend directly.
pub fn key_to_focus_string(key: &Key) -> String {
    match key {
        Key::Char(c) => c.to_string(),
        Key::Named(named) => {
            let s = match named {
                NamedKey::Escape => "Escape".to_string(),
                NamedKey::Tab => "Tab".to_string(),
                NamedKey::BackTab => "BackTab".to_string(),
                NamedKey::Enter => "Enter".to_string(),
                NamedKey::Backspace => "Backspace".to_string(),
                NamedKey::Delete => "Delete".to_string(),
                NamedKey::Insert => "Insert".to_string(),
                NamedKey::Home => "Home".to_string(),
                NamedKey::End => "End".to_string(),
                NamedKey::PageUp => "PageUp".to_string(),
                NamedKey::PageDown => "PageDown".to_string(),
                NamedKey::Up => "Up".to_string(),
                NamedKey::Down => "Down".to_string(),
                NamedKey::Left => "Left".to_string(),
                NamedKey::Right => "Right".to_string(),
                NamedKey::F(n) => format!("F{n}"),
                NamedKey::CapsLock => "CapsLock".to_string(),
                NamedKey::NumLock => "NumLock".to_string(),
                NamedKey::ScrollLock => "ScrollLock".to_string(),
                NamedKey::Menu => "Menu".to_string(),
            };
            s
        }
    }
}

/// Routes a raw `KeyPressed` to whichever leaf the `FocusManager` names as
/// focused, wrapping it into that leaf's own semantic `KeyPressed` variant.
/// `DataTable` and `Button` have no such variant, so a key focused on one
/// of them passes through as the original `UiEvent::KeyPressed` unchanged;
/// the same happens when `focused` names a leaf no longer present in this
/// frame's hit map (for example a tree that scrolled out of the render).
pub fn focus_scoped_key_event(
    focused: WidgetId,
    raw_key: &Key,
    modifiers: Modifiers,
    hit_map: &[PaintedHit],
    text_cursors: &RefCell<HashMap<WidgetId, TextCursorState>>,
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
            PaintedHit::Tree { id, .. } if *id == focused => Some(UiEvent::Tree(
                *id,
                TreeEvent::KeyPressed {
                    key: key.clone(),
                    modifiers,
                },
            )),
            PaintedHit::List { id, .. } if *id == focused => Some(UiEvent::List(
                *id,
                ListViewEvent::KeyPressed {
                    key: key.clone(),
                    modifiers,
                },
            )),
            PaintedHit::TabBar { id, .. } if *id == focused => Some(UiEvent::TabBar(
                *id,
                TabBarEvent::KeyPressed {
                    key: key.clone(),
                    modifiers,
                },
            )),
            PaintedHit::Extra { id, extra, .. } if *id == focused => {
                if let Some(text_input) = extra_as::<TextInput>(extra) {
                    return text_input_key_event(*id, text_input, raw_key, modifiers, text_cursors);
                }
                if let Some(switch) = extra_as::<Switch>(extra) {
                    // Route the matched hit id (`*id`), not the
                    // descriptor's embedded `SwitchEvent::Toggled::id` —
                    // see the identical comment in `semantic_event_for_hit`.
                    let SwitchEvent::Toggled {
                        id: _switch_id,
                        value,
                    } = switch.handle_key(raw_key, &modifiers)?;
                    return Some(UiEvent::Form(
                        *id,
                        FormEvent::ToggleChanged { id: *id, value },
                    ));
                }
                None
            }
            _ => None,
        })
        .unwrap_or(fallback)
}
