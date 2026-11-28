//! Semantic event synthesis: turns a raw pointer hit ([`semantic_event_for_hit`])
//! or a focus-scoped key press ([`focus_scoped_key_event`]) into the
//! leaf-specific `UiEvent` variant a binding's handler would otherwise have
//! to re-derive from the raw `MouseDown`/`KeyPressed` plus a `"hit"`
//! payload itself.

use quadraui::{
    CanvasEvent, DataTableEvent, Key, ListViewEvent, Modifiers, MouseButton, NamedKey, TabBar,
    TabBarEvent, TextDisplayEvent, TreeEvent, UiEvent, WidgetId,
};

use crate::drag::{tab_bar_for_right_segment, tree_row_path};
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
/// `WidgetState` entry rather than through an event. A binding's handler
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

/// Left/Right navigation and Enter/Space activation for a focused `TabBar`
/// — the keyboard half of the seam [`quadraui::TabChrome::focused_index`]
/// composes with on the paint side. Delegates to
/// [`TabBar::handle_focus_key`] (the primitive's own pure helper) using the
/// `focused_index`/`tab_count` [`crate::hit::PaintedHit::TabBar`] recorded
/// at paint time, the same way [`data_table_key_event`] reads its hit's
/// `selected_idx`/`total_rows`. Returns `None` for any key
/// `handle_focus_key` doesn't recognize (or when `tab_count == 0`),
/// leaving [`focus_scoped_key_event`] to fall through to the raw
/// `TabBarEvent::KeyPressed` the same way `list_key_event`'s callers do.
fn tab_bar_key_event(
    id: WidgetId,
    key: &str,
    focused_index: Option<usize>,
    tab_count: usize,
) -> Option<UiEvent> {
    TabBar::handle_focus_key(focused_index, tab_count, key).map(|ev| UiEvent::TabBar(id, ev))
}

/// The semantic [`UiEvent`] a click on `detail` (within primitive `id`)
/// stands for, if any — the set the binding's own event handler synthesizes so a tab
/// click, tab close-button click, tree-row/chevron click, list-item click,
/// data-table-row/header click, or tab-bar button click reaches the
/// binding's own event handler as the same shape it would get by pushing that event directly
/// (`UiEvent::TabBar`/`UiEvent::Tree`/`UiEvent::List`/`UiEvent::DataTable`),
/// rather than only as a raw `MouseDown`/`DoubleClick` plus a `"hit"` dict
/// the handler would have to re-interpret itself. `double_click` selects
/// between a variant's single- and double-click shapes where both exist
/// (e.g. `TreeEvent::RowClicked` vs `RowDoubleClicked`,
/// `ListViewEvent::SelectionChanged` vs `ItemDoubleClicked`,
/// `DataTableEvent::RowSelected` vs `RowDoubleClicked`). `None` for every
/// other `HitDetail` — a data-table header divider or a split divider
/// (handled separately, via [`crate::drag::ActiveDrag`], since either spans
/// a drag rather than a single click), a tab-bar scroll-left/right arrow (no semantic
/// event exists for it), or a modified list-item click (see the `ListItem`
/// arm below, which loses the gesture if folded into
/// `ListViewEvent::SelectionChanged`) — for which the raw
/// `MouseDown`/`DoubleClick` and its `"hit"` dict reach the binding's own
/// event handler unchanged.
/// [`HitDetail::ExtraBody`] resolves against the matched extra's own click
/// semantics (currently [`Checkbox`]/[`Slider`]/[`Select`]); an extra with no
/// recognized click behavior still falls through to the raw `MouseDown`. `point` is
/// the absolute click position the caller already has (every caller is a
/// `MouseDown`/`DoubleClick` branch, so it's always available) — only
/// [`HitDetail::ExtraBody`]'s [`Slider`] arm and [`HitDetail::CanvasRegion`]'s
/// own arm use it, the latter to translate it into the canvas's own LOCAL
/// frame for [`quadraui::CanvasEvent`]'s `x`/`y` fields. `button` is the
/// pressed mouse button the caller's raw `MouseDown` carried — a
/// `DoubleClick` has no `button` field of its own, so every caller passes
/// [`MouseButton::Left`] for one (matching `modifiers`' own
/// `Modifiers::default()` fallback there); only
/// [`HitDetail::CanvasRegion`]'s non-double-click arm reads it, for
/// [`quadraui::CanvasEvent::RegionClicked`]'s own `button` field.
pub fn semantic_event_for_hit(
    id: WidgetId,
    detail: &HitDetail,
    button: MouseButton,
    modifiers: Modifiers,
    double_click: bool,
    hit_map: &[PaintedHit],
    point: quadraui::Point,
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
        HitDetail::ExtraBody => hit_map.iter().find_map(|hit| match hit {
            PaintedHit::Extra {
                id: hit_id,
                extra,
                area,
                ..
            } if *hit_id == id => None,
            _ => None,
        }),
        // Translates the absolute `point` into the canvas's own LOCAL frame
        // (looked up from the matched `PaintedHit::Extra`'s own `area`,
        // the same "every `DrawOp`/`CanvasRegion::rect` coordinate" frame
        // `quadraui::Canvas`'s module doc describes) for
        // `CanvasEvent::RegionClicked`/`RegionDoubleClicked`'s own `x`/`y`
        // fields — a region's own rect is authored in that frame, so this
        // lets an app compare the two without re-deriving the canvas's
        // absolute paint position itself.
        HitDetail::CanvasRegion { region } => hit_map.iter().find_map(|hit| match hit {
            PaintedHit::Extra {
                id: hit_id, area, ..
            } if *hit_id == id => {
                let x = point.x - area.x;
                let y = point.y - area.y;
                Some(UiEvent::Canvas(
                    id,
                    if double_click {
                        CanvasEvent::RegionDoubleClicked {
                            region: *region,
                            x,
                            y,
                        }
                    } else {
                        CanvasEvent::RegionClicked {
                            region: *region,
                            button,
                            modifiers,
                            x,
                            y,
                        }
                    },
                ))
            }
            _ => None,
        }),
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
/// [`crate::extra_paint::WIDGET_KINDS`] leaf (e.g. `Button`, `Switch`) never
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
            PaintedHit::TabBar {
                id,
                focused_index,
                tab_count,
                ..
            } if *id == focused => tab_bar_key_event(*id, &key, *focused_index, *tab_count)
                .or_else(|| {
                    Some(UiEvent::TabBar(
                        *id,
                        TabBarEvent::KeyPressed {
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

/// Forwards a resolved [`HitDetail::CanvasRegion`] click to the matched
/// leaf's own [`crate::scripted::ScriptedExtra`] facet, if it has one —
/// the counterpart, for a script-defined widget, of
/// [`semantic_event_for_hit`]'s own `CanvasRegion` arm (which only builds
/// the `UiEvent::Canvas` an app's own handler sees; a `Canvas` descriptor
/// has no `on_event` of its own for anything to call). `crate::engine`
/// calls both at the same site, on every click [`HitDetail::CanvasRegion`]
/// resolves — see that call site's own comment. `point` is ABSOLUTE
/// (the same frame `semantic_event_for_hit`'s own `point` parameter takes),
/// translated into the leaf's own LOCAL frame here via the matched
/// [`PaintedHit::Extra`]'s `area`, the same translation that other
/// function's `CanvasRegion` arm already does for
/// [`CanvasEvent::RegionClicked`]'s own `x`/`y` fields.
///
/// Returns `false` (nothing to forward, and so nothing for the caller to
/// force a redraw over) whenever no leaf named `id` is an `Extra` with a
/// [`crate::scripted::ScriptedExtra`] facet — true for every hit this
/// isn't called for as much as for a genuine miss, so a caller is free to
/// call this unconditionally on a `CanvasRegion` hit without checking the
/// matched descriptor's own kind first.
pub fn scripted_region_click(
    hit_map: &[PaintedHit],
    id: WidgetId,
    region: WidgetId,
    button: MouseButton,
    modifiers: Modifiers,
    double_click: bool,
    point: quadraui::Point,
) -> bool {
    hit_map
        .iter()
        .find_map(|hit| match hit {
            PaintedHit::Extra {
                id: hit_id,
                extra,
                area,
                ..
            } if *hit_id == id => {
                let scripted = extra.as_scripted()?;
                let local = quadraui::Point::new(point.x - area.x, point.y - area.y);
                Some(scripted.scripted_click_region(region, button, modifiers, local, double_click))
            }
            _ => None,
        })
        .unwrap_or(false)
}

/// Forwards a focused key press to whichever leaf [`FocusTree`][quadraui::FocusTree]
/// names as focused, if it is an `Extra` with a
/// [`crate::scripted::ScriptedExtra`] facet — the key-press counterpart of
/// [`scripted_region_click`] above, called alongside
/// [`focus_scoped_key_event`] rather than from inside it, for the same
/// reason: that function's return type has no room for the `bool` a
/// script's own `on_event` answers with, only a `UiEvent` for the app's
/// own handler to see.
pub fn scripted_key_press(hit_map: &[PaintedHit], focused: WidgetId, key: &Key) -> bool {
    hit_map
        .iter()
        .find_map(|hit| match hit {
            PaintedHit::Extra { id, extra, .. } if *id == focused => {
                let scripted = extra.as_scripted()?;
                Some(scripted.scripted_key_pressed(key.clone()))
            }
            _ => None,
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

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
    use quadraui::{Rect, TabBarLayout};

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

    /// A minimal [`PaintedHit::TabBar`] with `tab_count` tabs and no
    /// close/scroll/right-segment chrome — the `hit_test`-reachable fields
    /// `focus_scoped_key_event` never touches are left empty, same as
    /// [`crate::drag::tab_bar_for_right_segment`]'s own test fixtures.
    fn tab_bar_hit(id: WidgetId, focused_index: Option<usize>, tab_count: usize) -> PaintedHit {
        PaintedHit::TabBar {
            id,
            area: Rect::new(0.0, 0.0, 40.0, 1.0),
            clip: Rect::new(0.0, 0.0, 40.0, 1.0),
            layout: TabBarLayout {
                bar_width: 40.0,
                bar_height: 1.0,
                visible_tabs: Vec::new(),
                visible_segments: Vec::new(),
                scroll_left: None,
                scroll_right: None,
                hit_regions: Vec::new(),
                resolved_scroll_offset: 0,
            },
            focused_index,
            tab_count,
        }
    }

    /// `ArrowRight` on a focused `"tabs"` node with a `focused_index` of
    /// `0` moves the focus to tab `1` — [`TabBarEvent::FocusMoved`], not
    /// the raw `TabBarEvent::KeyPressed` fallback. This is the keyboard
    /// half of the seam [`quadraui::TabChrome::focused_index`] composes on
    /// the paint side: an app only sees `FocusMoved` here because
    /// [`focus_scoped_key_event`]'s `PaintedHit::TabBar` arm now tries
    /// [`tab_bar_key_event`] before falling back to `KeyPressed`.
    #[test]
    fn arrow_right_on_a_focused_tab_bar_moves_focus() {
        let id = WidgetId::new("tabs");
        let hit_map = vec![tab_bar_hit(id, Some(0), 3)];
        let event = focus_scoped_key_event(
            id,
            &Key::Named(NamedKey::Right),
            Modifiers::default(),
            &hit_map,
        );
        assert_eq!(
            event,
            UiEvent::TabBar(id, TabBarEvent::FocusMoved { index: 1 })
        );
    }

    /// `Enter` on a focused `"tabs"` node activates the currently focused
    /// tab — [`TabBarEvent::TabActivated`], the keyboard equivalent of a
    /// pointer click on the tab, mirroring `data_table_key_event`'s own
    /// Enter-activates-the-selection handling.
    #[test]
    fn enter_on_a_focused_tab_bar_activates_it() {
        let id = WidgetId::new("tabs");
        let hit_map = vec![tab_bar_hit(id, Some(2), 3)];
        let event = focus_scoped_key_event(
            id,
            &Key::Named(NamedKey::Enter),
            Modifiers::default(),
            &hit_map,
        );
        assert_eq!(
            event,
            UiEvent::TabBar(id, TabBarEvent::TabActivated { index: 2 })
        );
    }

    /// A key `TabBar::handle_focus_key` doesn't recognize (`Escape`) falls
    /// through to the raw `TabBarEvent::KeyPressed`, same as it always
    /// did before the `FocusMoved`/`TabActivated` synthesis existed.
    #[test]
    fn unrecognized_key_on_a_focused_tab_bar_falls_back_to_key_pressed() {
        let id = WidgetId::new("tabs");
        let hit_map = vec![tab_bar_hit(id, Some(0), 3)];
        let event = focus_scoped_key_event(
            id,
            &Key::Named(NamedKey::Escape),
            Modifiers::default(),
            &hit_map,
        );
        assert_eq!(
            event,
            UiEvent::TabBar(
                id,
                TabBarEvent::KeyPressed {
                    key: "Escape".to_string(),
                    modifiers: Modifiers::default(),
                }
            )
        );
    }

    /// A `ScriptWidget` built directly in Rust test code, recording every
    /// `on_event` call it receives into `log` (shared externally, since
    /// [`crate::scripted::ScriptedPrimitive`] owns the widget by value) —
    /// enough to prove [`scripted_region_click`]/[`scripted_key_press`]
    /// forward the right data, with no real binding callback object in
    /// scope.
    struct RecordingWidget {
        log: Rc<RefCell<Vec<String>>>,
    }

    impl crate::scripted::ScriptWidget for RecordingWidget {
        fn measure(
            &self,
            _ctx: &quadraui::primitive::LayoutCtx,
            _constraints: crate::scripted::ScriptedConstraints,
        ) -> crate::scripted::ScriptedSize {
            crate::scripted::ScriptedSize::default()
        }

        fn paint(
            &self,
            _ctx: &quadraui::primitive::LayoutCtx,
            _props: &crate::scripted::ScriptedProps,
            _rect: Rect,
        ) -> crate::scripted::ScriptedPaint {
            crate::scripted::ScriptedPaint::default()
        }

        fn on_event(
            &self,
            event: &crate::scripted::ScriptedInputEvent,
            _props: &crate::scripted::ScriptedProps,
            _state: &crate::scripted::ScriptedState,
        ) -> bool {
            match event {
                crate::scripted::ScriptedInputEvent::Canvas(CanvasEvent::RegionClicked {
                    region,
                    x,
                    y,
                    ..
                }) => {
                    self.log
                        .borrow_mut()
                        .push(format!("click:{}:{x},{y}", region.as_str()));
                    true
                }
                crate::scripted::ScriptedInputEvent::Key(key) => {
                    self.log
                        .borrow_mut()
                        .push(format!("key:{}", key_to_focus_string(key)));
                    true
                }
                _ => false,
            }
        }
    }

    /// Wraps a [`RecordingWidget`] in a [`PaintedHit::Extra`] at `area` —
    /// the shape [`scripted_region_click`]/[`scripted_key_press`] both
    /// scan `hit_map` for.
    fn recording_scripted_extra_hit(
        id: WidgetId,
        area: Rect,
        log: Rc<RefCell<Vec<String>>>,
    ) -> PaintedHit {
        let primitive = crate::scripted::ScriptedPrimitive::new(id, RecordingWidget { log }, true);
        PaintedHit::Extra {
            id,
            area,
            extra: Rc::new(primitive) as Rc<dyn crate::extra_paint::ExtraPaint>,
        }
    }

    /// [`scripted_region_click`] translates `point` into the leaf's own
    /// LOCAL frame (via the matched hit's `area`) before forwarding to
    /// `on_event`, the same translation [`semantic_event_for_hit`]'s own
    /// `CanvasRegion` arm does for `CanvasEvent::RegionClicked`'s `x`/`y`
    /// — and reports the script's own `true` (consumed) back to the
    /// caller.
    #[test]
    fn scripted_region_click_translates_to_local_and_reports_consumed() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let hit_map = vec![recording_scripted_extra_hit(
            WidgetId::new("w"),
            Rect::new(10.0, 20.0, 10.0, 4.0),
            log.clone(),
        )];

        let handled = scripted_region_click(
            &hit_map,
            WidgetId::new("w"),
            WidgetId::new("w:region"),
            MouseButton::Left,
            Modifiers::default(),
            false,
            quadraui::Point::new(18.0, 22.0),
        );

        assert!(handled, "the script's own on_event reported true");
        assert_eq!(log.borrow().as_slice(), ["click:w:region:8,2"]);
    }

    /// With no leaf named `id` at all — or one with no
    /// [`crate::scripted::ScriptedExtra`] facet — [`scripted_region_click`]
    /// is a safe no-op reporting `false`, never a panic.
    #[test]
    fn scripted_region_click_is_false_when_nothing_matches() {
        let hit_map: Vec<PaintedHit> = Vec::new();
        assert!(!scripted_region_click(
            &hit_map,
            WidgetId::new("missing"),
            WidgetId::new("missing:region"),
            MouseButton::Left,
            Modifiers::default(),
            false,
            quadraui::Point::new(0.0, 0.0),
        ));
    }

    /// [`scripted_key_press`] forwards a focused key press to the
    /// matching leaf's own `on_event`, reporting its `true` (consumed)
    /// back to the caller.
    #[test]
    fn scripted_key_press_forwards_the_key_and_reports_consumed() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let hit_map = vec![recording_scripted_extra_hit(
            WidgetId::new("w"),
            Rect::new(0.0, 0.0, 10.0, 4.0),
            log.clone(),
        )];

        let handled =
            scripted_key_press(&hit_map, WidgetId::new("w"), &Key::Named(NamedKey::Enter));

        assert!(handled, "the script's own on_event reported true");
        assert_eq!(log.borrow().as_slice(), ["key:Enter"]);
    }
}
