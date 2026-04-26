//! Logical actions that mutate state, plus the [`apply_action`]
//! reducer.
//!
//! Backends translate raw input (crossterm `KeyCode`, GTK key
//! signals, mouse coordinates) into [`Action`] variants and pass
//! them here. This is the single place all semantic behaviour lives:
//! adding a new feature means adding a variant and an `apply_action`
//! arm — both backends pick it up the moment they bind a key/click
//! to the new variant.
//!
//! Coordinate-bearing variants (`StatusBarClick`, `PickerClick`)
//! carry their geometry already resolved into the relevant logical
//! coordinates (cells for TUI, pixels for GTK). The reducer doesn't
//! know which one — it just consults the same `quadraui` primitives
//! the view-builder produced.

use quadraui::WidgetId;

use crate::k8s;
use crate::state::{AppState, Focus, Picker, PickerPurpose, ResourceKind};
use crate::view;

/// Every logical mutation the app supports. New features get a new
/// variant + an arm in [`apply_action`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Set `state.should_quit = true`.
    Quit,
    /// Re-fetch the current resource kind in the current namespace.
    Refresh,
    /// Open the namespace picker.
    OpenNamespacePicker,
    /// Open the resource-kind picker.
    OpenKindPicker,
    /// Cycle focus between resource list and YAML pane.
    ToggleFocus,

    /// Move selection in whatever pane currently has focus.
    MoveDown,
    MoveUp,

    /// Always scroll the YAML pane regardless of focus.
    YamlPageDown,
    YamlPageUp,

    // ── Picker (modal) actions ────────────────────────────────────
    /// Apply the picker's current selection.
    PickerCommit,
    /// Close the picker without applying.
    PickerCancel,
    PickerMoveDown,
    PickerMoveUp,
    /// Append a character to the picker's filter query.
    PickerInput(char),
    /// Drop the last character from the filter query.
    PickerBackspace,
    /// Move the picker's highlight to a specific filtered-row index
    /// (used by mouse handlers that resolve a click to a row).
    PickerSelectVisible(usize),

    // ── Click resolution helpers ──────────────────────────────────
    /// A click landed on a status-bar segment — backend has already
    /// resolved the segment id via `StatusBar::resolve_click`.
    StatusBarSegmentClicked(WidgetId),
}

/// Apply one [`Action`] to `state`. Async k8s calls are run on `rt`.
///
/// Pure with respect to anything other than `state` — backends call
/// this freely, then re-render from `state` on the next frame.
pub fn apply_action(state: &mut AppState, action: Action, rt: &tokio::runtime::Runtime) {
    // Picker, when open, has highest precedence — its actions are
    // routed first regardless of focus.
    if state.picker.is_some() && apply_picker_action(state, &action, rt) {
        return;
    }

    match action {
        Action::Quit => state.should_quit = true,
        Action::Refresh => refresh_resources(state, rt),
        Action::OpenNamespacePicker => open_namespace_picker(state),
        Action::OpenKindPicker => open_kind_picker(state),
        Action::ToggleFocus => {
            state.focus = match state.focus {
                Focus::Resources => Focus::Yaml,
                Focus::Yaml => Focus::Resources,
            };
        }
        Action::MoveDown => match state.focus {
            Focus::Resources => {
                if !state.resources.is_empty() {
                    state.selected = (state.selected + 1).min(state.resources.len() - 1);
                    state.yaml_scroll = 0;
                }
            }
            Focus::Yaml => {
                state.yaml_scroll = state.yaml_scroll.saturating_add(1);
            }
        },
        Action::MoveUp => match state.focus {
            Focus::Resources => {
                state.selected = state.selected.saturating_sub(1);
                state.yaml_scroll = 0;
            }
            Focus::Yaml => {
                state.yaml_scroll = state.yaml_scroll.saturating_sub(1);
            }
        },
        Action::YamlPageDown => {
            state.yaml_scroll = state.yaml_scroll.saturating_add(10);
        }
        Action::YamlPageUp => {
            state.yaml_scroll = state.yaml_scroll.saturating_sub(10);
        }
        Action::StatusBarSegmentClicked(id) => {
            if id == WidgetId::new("status:namespace") {
                open_namespace_picker(state);
            } else if id == WidgetId::new("status:kind") {
                open_kind_picker(state);
            }
        }
        // Picker variants when no picker is open: ignored. Keeps
        // backends from having to special-case them.
        Action::PickerCommit
        | Action::PickerCancel
        | Action::PickerMoveDown
        | Action::PickerMoveUp
        | Action::PickerInput(_)
        | Action::PickerBackspace
        | Action::PickerSelectVisible(_) => {}
    }
}

/// Picker-mode dispatch. Returns true when the action was consumed
/// here — caller skips the base-layer match in that case.
fn apply_picker_action(
    state: &mut AppState,
    action: &Action,
    rt: &tokio::runtime::Runtime,
) -> bool {
    match action {
        Action::PickerCancel => {
            state.picker = None;
            true
        }
        Action::PickerCommit => {
            commit_picker(state, rt);
            true
        }
        Action::PickerMoveDown => {
            if let Some(p) = state.picker.as_mut() {
                p.move_down();
            }
            true
        }
        Action::PickerMoveUp => {
            if let Some(p) = state.picker.as_mut() {
                p.move_up();
            }
            true
        }
        Action::PickerInput(ch) => {
            if let Some(p) = state.picker.as_mut() {
                p.type_char(*ch);
            }
            true
        }
        Action::PickerBackspace => {
            if let Some(p) = state.picker.as_mut() {
                p.backspace();
            }
            true
        }
        Action::PickerSelectVisible(idx) => {
            if let Some(p) = state.picker.as_mut() {
                if *idx < p.visible_indices().len() {
                    p.selected = *idx;
                }
            }
            commit_picker(state, rt);
            true
        }
        // Some non-picker actions stay live during a picker (e.g.
        // Quit) — let them fall through.
        _ => false,
    }
}

fn open_namespace_picker(state: &mut AppState) {
    state.picker = Some(Picker {
        title: " Namespace ".to_string(),
        purpose: PickerPurpose::Namespace,
        items: state.namespaces.clone(),
        query: String::new(),
        selected: state.current_ns,
    });
}

fn open_kind_picker(state: &mut AppState) {
    let items: Vec<String> = ResourceKind::ALL
        .iter()
        .map(|k| k.label().to_string())
        .collect();
    let selected = view::picker_current_index(state, PickerPurpose::ResourceKind).unwrap_or(0);
    state.picker = Some(Picker {
        title: " Resource kind ".to_string(),
        purpose: PickerPurpose::ResourceKind,
        items,
        query: String::new(),
        selected,
    });
}

/// Apply the picker's current selection — switch namespace or kind
/// and trigger a refresh.
fn commit_picker(state: &mut AppState, rt: &tokio::runtime::Runtime) {
    let Some(p) = state.picker.take() else {
        return;
    };
    let Some(orig) = p.selected_orig_index() else {
        return;
    };
    match p.purpose {
        PickerPurpose::Namespace => {
            if orig < state.namespaces.len() {
                state.current_ns = orig;
                refresh_resources(state, rt);
            }
        }
        PickerPurpose::ResourceKind => {
            if let Some(kind) = ResourceKind::ALL.get(orig).copied() {
                state.kind = kind;
                refresh_resources(state, rt);
            }
        }
    }
}

/// Re-fetch the current resource kind in the current namespace,
/// updating `state.resources` and `state.status`.
pub fn refresh_resources(state: &mut AppState, rt: &tokio::runtime::Runtime) {
    let ns = state.current_namespace().to_string();
    let kind = state.kind;
    state.status = format!("Listing {} in {ns}…", kind.label());
    match rt.block_on(k8s::list_resources(kind, &ns)) {
        Ok(items) => {
            state.resources = items;
            state.selected = state.selected.min(state.resources.len().saturating_sub(1));
            state.status = format!(
                "Loaded {} {}.",
                state.resources.len(),
                kind.label().to_ascii_lowercase()
            );
        }
        Err(e) => {
            state.resources.clear();
            state.status = format!("Error: {e}");
        }
    }
    state.yaml_scroll = 0;
}
