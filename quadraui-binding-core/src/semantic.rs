//! Semantic event synthesis: turns a raw pointer hit ([`semantic_event_for_hit`])
//! or a focus-scoped key press ([`focus_scoped_key_event`]) into the
//! leaf-specific `UiEvent` variant a binding's handler would otherwise have
//! to re-derive from the raw `MouseDown`/`KeyPressed` plus a `"hit"`
//! payload itself.

use quadraui::{
    ActivityBar, ActivityBarEvent, ComboBox, ComboBoxEvent, DataTableEvent, DatePicker,
    DatePickerEvent, FormEvent, Key, ListViewEvent, Modifiers, NamedKey, NumberInput,
    NumberInputEvent, RadioGroup, RadioGroupEvent, StatusBar, StatusBarEvent, Switch, SwitchEvent,
    TabBar, TabBarEvent, TextDisplayEvent, TextInput, Toolbar, TreeEvent, UiEvent, WidgetId,
};

use crate::drag::{tab_bar_for_right_segment, tree_row_path};
use crate::extra_paint::extra_as;
use crate::hit::{HitDetail, PaintedHit};
use crate::text_input::{text_input_key_event, TextEditorCache};
use crate::tree_edit::tree_row_rename_key_event;

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
/// same fallthrough — `List`'s own Up/Down/`j`/`k` navigation stays the
/// app's to interpret from that `KeyPressed`, unlike `DataTable`'s, since a
/// `List`'s selection is tracked by the app already instead of inside this
/// semantic layer.
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

/// `NumberInput`'s own value as `FormEvent`'s payload would show it:
/// `NumberInput::precision` decimal places, matching
/// `native_surface_paint::display_text`'s own formatting so a standalone
/// `NumberInput`'s routed events render identically to what its own
/// rasteriser paints.
fn format_number_input_value(value: f64, precision: u32) -> String {
    format!("{value:.*}", precision as usize)
}

/// Translates one [`NumberInputEvent`] into the [`FormEvent`] it should
/// produce — shared by [`number_input_key_event`] (keyboard) and
/// [`crate::engine::InteractionEngine::handle`]'s own stepper-click
/// routing (pointer), so both paths describe the same mapping once.
/// `NumberInput` has no `FormEvent` slot of its own (unlike
/// `ComboBox`/`DatePicker` below) — its own typed events are translated
/// into the closest matching `TextInputChanged`/`TextInputCommitted`
/// shape, the same String-payload reuse `Switch`/`TextInput`'s own
/// standalone routing above already relies on (a `FormEvent` variant
/// whose name nominally describes a `Form`-embedded field, reused
/// verbatim for the matching standalone extra).
pub(crate) fn number_input_form_event(
    id: WidgetId,
    number_input: &NumberInput,
    event: NumberInputEvent,
) -> FormEvent {
    match event {
        NumberInputEvent::ValueChanged { value, .. } => FormEvent::TextInputChanged {
            id,
            value: format_number_input_value(value, number_input.precision),
        },
        NumberInputEvent::DraftChanged { draft, .. } => {
            FormEvent::TextInputChanged { id, value: draft }
        }
        NumberInputEvent::Committed { value, .. } => FormEvent::TextInputCommitted {
            id,
            value: format_number_input_value(value, number_input.precision),
        },
        NumberInputEvent::Cancelled { .. } => FormEvent::TextInputChanged {
            id,
            value: format_number_input_value(number_input.value, number_input.precision),
        },
    }
}

/// Resolves a key focused on a standalone [`NumberInput`] extra into the
/// [`FormEvent`] it should produce, via [`number_input_form_event`].
/// Returns `None` for every key [`NumberInput::handle_key`] doesn't
/// recognize — the caller then falls through to the raw `KeyPressed`
/// fallback.
fn number_input_key_event(
    id: WidgetId,
    number_input: &NumberInput,
    key: &Key,
    modifiers: &Modifiers,
) -> Option<UiEvent> {
    let event = number_input.handle_key(key, modifiers)?;
    Some(UiEvent::Form(
        id,
        number_input_form_event(id, number_input, event),
    ))
}

/// Resolves a `MouseDown`/`DoubleClick` landing on a standalone
/// [`NumberInput`] extra's [`crate::hit::HitDetail::ExtraBody`] into the
/// stepper click it stands for — unlike `RadioGroup`/`ComboBox`/`DatePicker`
/// below, [`NumberInput::layout`] needs no backend metrics (its stepper
/// column is a fixed width), so the increment/decrement regions are
/// precisely resolvable here from `point` alone, the same way
/// [`crate::text_input::text_input_click_event`] resolves a `TextInput`
/// click. Returns `None` when `point` landed on the text area or outside
/// the control (`NumberInput::click`'s own contract), or when `id` isn't
/// painted as a `NumberInput` extra this frame.
pub(crate) fn number_input_click_event(
    hit_map: &[PaintedHit],
    id: WidgetId,
    point: quadraui::Point,
) -> Option<UiEvent> {
    let (area, number_input) = hit_map.iter().find_map(|hit| match hit {
        PaintedHit::Extra {
            id: hit_id,
            area,
            extra,
            ..
        } if *hit_id == id => extra_as::<NumberInput>(extra).map(|ni| (*area, ni)),
        _ => None,
    })?;
    let hit = number_input.layout(area).hit_test(point);
    let event = number_input.click(hit)?;
    Some(UiEvent::Form(
        id,
        number_input_form_event(id, number_input, event),
    ))
}

/// Resolves a key focused on a standalone [`RadioGroup`] extra into the
/// [`FormEvent`] it should produce: `RadioGroup` has no `FormEvent` slot
/// of its own either, so [`RadioGroupEvent::SelectionChanged`] is
/// translated into `SegmentedControlChanged` — both carry the exact same
/// `{ id, selected_idx: usize }` shape (a single exclusive choice from a
/// list of options), and `FormEvent` has no other index-based variant free
/// of a `Dropdown`-specific connotation `RadioGroup` doesn't share.
/// Returns `None` for every key [`RadioGroup::handle_key`] doesn't
/// recognize (also the case when nothing is focused, since this is only
/// reached once `focused` already matched) — the caller then falls
/// through to the raw `KeyPressed` fallback.
fn radio_group_key_event(
    id: WidgetId,
    group: &RadioGroup,
    key: &Key,
    modifiers: &Modifiers,
) -> Option<UiEvent> {
    let RadioGroupEvent::SelectionChanged { index } = group.handle_key(key, modifiers)?;
    Some(UiEvent::Form(
        id,
        FormEvent::SegmentedControlChanged {
            id,
            selected_idx: index,
        },
    ))
}

/// Resolves a key focused on a standalone [`ComboBox`] extra into the
/// [`FormEvent`] it should produce. Unlike `NumberInput`/`RadioGroup`
/// above, `FormEvent` already has a dedicated variant for every
/// [`ComboBoxEvent`] shape (`Form`'s own `FieldKind::ComboBox` embeds the
/// same primitive), so each one carries over field-for-field. Returns
/// `None` for every key [`ComboBox::handle_key`] doesn't recognize — the
/// caller then falls through to the raw `KeyPressed` fallback.
fn combo_box_key_event(
    id: WidgetId,
    combo_box: &ComboBox,
    key: &Key,
    modifiers: &Modifiers,
) -> Option<UiEvent> {
    let form_event = match combo_box.handle_key(key, modifiers)? {
        ComboBoxEvent::TextChanged { text, .. } => {
            FormEvent::ComboBoxTextChanged { id, value: text }
        }
        ComboBoxEvent::HighlightChanged { index, .. } => {
            FormEvent::ComboBoxHighlightChanged { id, index }
        }
        ComboBoxEvent::Committed { value, .. } => FormEvent::ComboBoxCommitted { id, value },
        ComboBoxEvent::Cancelled { .. } => FormEvent::ComboBoxCancelled { id },
    };
    Some(UiEvent::Form(id, form_event))
}

/// Resolves a focus-scoped key (already stringified by
/// [`key_to_focus_string`]) against a standalone [`DatePicker`] extra via
/// its own [`DatePicker::navigate`] — only [`DatePickerEvent::DateSelected`]
/// (Enter confirming the focused day) has a matching `FormEvent` slot
/// (`DateChanged`, which already carries the same `CalendarDate` type);
/// [`DatePickerEvent::FocusMoved`] (arrow-key/page scrubbing before a day is
/// confirmed) has no `FormEvent` counterpart, so it resolves to `None` and
/// falls through to the raw `KeyPressed` fallback — a standalone
/// `DatePicker`'s scrub position does not yet survive across keystrokes
/// the way a `Form`-embedded one does via `FormController`'s own
/// `DateEditState` cache.
fn date_picker_key_event(id: WidgetId, picker: &DatePicker, key: &str) -> Option<UiEvent> {
    match picker.navigate(key)? {
        DatePickerEvent::DateSelected(value) => {
            Some(UiEvent::Form(id, FormEvent::DateChanged { id, value }))
        }
        DatePickerEvent::FocusMoved(_) => None,
    }
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
                id: hit_id, extra, ..
            } if *hit_id == id => {
                let switch = extra_as::<Switch>(extra)?;
                // Route the matched hit id, not the descriptor's embedded
                // `SwitchEvent::Toggled::id` — a render callback may leave
                // the descriptor's own `id` at its default and reuse it
                // across leaves, so only the tree-address hit id is
                // guaranteed unique. Renamed rather than destructured as
                // `id` so the two can't silently shadow each other.
                let SwitchEvent::Toggled {
                    id: _switch_id,
                    value,
                } = switch.click()?;
                Some(UiEvent::Form(id, FormEvent::ToggleChanged { id, value }))
            }
            _ => None,
        }),
        HitDetail::ExtraChrome { bar } => hit_map.iter().find_map(|hit| match hit {
            PaintedHit::Extra {
                id: hit_id, extra, ..
            } if *hit_id == *bar => {
                if extra_as::<StatusBar>(extra).is_some() {
                    Some(UiEvent::StatusBar(
                        *bar,
                        StatusBarEvent::SegmentClicked { id },
                    ))
                } else if extra_as::<ActivityBar>(extra).is_some() {
                    Some(UiEvent::ActivityBar(
                        *bar,
                        ActivityBarEvent::ItemClicked { id },
                    ))
                } else {
                    let toolbar = extra_as::<Toolbar>(extra)?;
                    Some(UiEvent::Toolbar(*bar, toolbar.click(id)?))
                }
            }
            _ => None,
        }),
        // `bar` is the embedded `Toolbar`'s own id (see
        // `HitDetail::SidebarPanelToolbarButton`'s doc) — look up that same
        // `PaintedHit::SidebarPanel` entry to get the descriptor back and
        // reuse `Toolbar::click`, exactly like the standalone/chrome case
        // above.
        HitDetail::SidebarPanelToolbarButton { bar } => hit_map.iter().find_map(|hit| match hit {
            PaintedHit::SidebarPanel {
                toolbar: Some(toolbar),
                ..
            } if toolbar.id == *bar => Some(UiEvent::Toolbar(*bar, toolbar.click(id)?)),
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
/// focused, wrapping it into that leaf's own semantic `KeyPressed` variant
/// (or, for a focused `DataTable`'s Up/Down/Enter, straight into
/// [`data_table_key_event`]'s `RowSelected`/`RowActivated`; or, for a
/// focused `List`'s Enter with at least one row, straight into
/// [`list_key_event`]'s `ItemActivated`). `Button` has
/// no semantic key variant at all, so a key focused on one passes through
/// as the original
/// `UiEvent::KeyPressed` unchanged; the same happens for any other key a
/// focused `DataTable`/`List` doesn't recognize, and when `focused` names a
/// leaf absent from this frame's hit map (for example a tree that scrolled
/// out of the render).
pub fn focus_scoped_key_event(
    focused: WidgetId,
    raw_key: &Key,
    modifiers: Modifiers,
    hit_map: &[PaintedHit],
    text_editors: &TextEditorCache,
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
                    if let Some(ev) =
                        tree_row_rename_key_event(*id, path, edit, raw_key, &modifiers)
                    {
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
            PaintedHit::Extra { id, extra, .. } if *id == focused => {
                if let Some(text_input) = extra_as::<TextInput>(extra) {
                    return text_input_key_event(*id, text_input, raw_key, modifiers, text_editors);
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
                if let Some(number_input) = extra_as::<NumberInput>(extra) {
                    return number_input_key_event(*id, number_input, raw_key, &modifiers);
                }
                if let Some(radio_group) = extra_as::<RadioGroup>(extra) {
                    return radio_group_key_event(*id, radio_group, raw_key, &modifiers);
                }
                if let Some(combo_box) = extra_as::<ComboBox>(extra) {
                    return combo_box_key_event(*id, combo_box, raw_key, &modifiers);
                }
                if let Some(date_picker) = extra_as::<DatePicker>(extra) {
                    return date_picker_key_event(*id, date_picker, &key);
                }
                if let Some(toolbar) = extra_as::<Toolbar>(extra) {
                    // Enter/Space activates whichever button
                    // `Toolbar::focused_index` names (set by the host, same
                    // convention `TabChrome::focused_index` documents);
                    // every other key is left to the raw fallback since
                    // `Toolbar` has no `KeyPressed` variant of its own to
                    // wrap it in.
                    if matches!(raw_key, Key::Named(NamedKey::Enter) | Key::Char(' ')) {
                        return Some(UiEvent::Toolbar(*id, toolbar.activate_focused()?));
                    }
                    return None;
                }
                None
            }
            _ => None,
        })
        .unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quadraui::{Rect, TabBarLayout};

    /// A minimal [`PaintedHit::TabBar`] with `tab_count` tabs and no
    /// close/scroll/right-segment chrome — the `hit_test`-reachable fields
    /// `focus_scoped_key_event` never touches are left empty, same as
    /// [`crate::drag::tab_bar_for_right_segment`]'s own test fixtures.
    fn tab_bar_hit(id: WidgetId, focused_index: Option<usize>, tab_count: usize) -> PaintedHit {
        PaintedHit::TabBar {
            id,
            area: Rect::new(0.0, 0.0, 40.0, 1.0),
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
        let text_editors = TextEditorCache::default();
        let event = focus_scoped_key_event(
            id,
            &Key::Named(NamedKey::Right),
            Modifiers::default(),
            &hit_map,
            &text_editors,
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
        let text_editors = TextEditorCache::default();
        let event = focus_scoped_key_event(
            id,
            &Key::Named(NamedKey::Enter),
            Modifiers::default(),
            &hit_map,
            &text_editors,
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
        let text_editors = TextEditorCache::default();
        let event = focus_scoped_key_event(
            id,
            &Key::Named(NamedKey::Escape),
            Modifiers::default(),
            &hit_map,
            &text_editors,
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
}
