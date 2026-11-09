//! Logical actions that mutate state, plus the [`apply_action`]
//! reducer.
//!
//! `quadraui::AppLogic::handle` on [`crate::KubeApp`] translates raw
//! `quadraui::UiEvent`s into [`Action`] variants and passes them
//! here. This is the single place all semantic behaviour lives:
//! adding a new feature means adding a variant and an `apply_action`
//! arm.
//!
//! Picker (namespace/resource-kind) key handling does **not** go
//! through `Action` — while a `PickerOverlay` is open,
//! [`crate::state::PickerOverlay::handle_key`]/`handle_mouse` consume
//! the raw event directly (they already know how to turn keystrokes
//! into query edits / selection moves / confirm / cancel), and
//! [`crate::KubeApp`] applies the resulting
//! [`crate::state::PickerOutcome`] via [`open_namespace_picker`],
//! [`open_kind_picker`] and [`commit_picker_selection`] below.

use quadraui::WidgetId;

use crate::k8s;
use crate::state::{AppState, Focus, PickerOverlay, PickerPurpose, ResourceKind};

/// Every logical mutation the app supports that isn't routed through
/// the open picker. New features get a new variant + an
/// `apply_action` arm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
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

    // ── Click resolution helpers ──────────────────────────────────
    /// A click landed on a status-bar segment — the caller has
    /// already resolved the segment id via `StatusBar::layout` +
    /// `StatusBarLayout::hit_test`.
    StatusBarSegmentClicked(WidgetId),
}

/// Apply one [`Action`] to `state`. Async k8s calls are run on `rt`.
///
/// Pure with respect to anything other than `state` — callers call
/// this freely, then re-render from `state` on the next frame.
pub fn apply_action(state: &mut AppState, action: Action, rt: &tokio::runtime::Runtime) {
    match action {
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
    }
}

/// Current resource-kind index into `ResourceKind::ALL`, for
/// pre-selecting the kind picker on the row the app is already on.
fn current_kind_index(state: &AppState) -> Option<usize> {
    ResourceKind::ALL.iter().position(|k| *k == state.kind)
}

pub fn open_namespace_picker(state: &mut AppState) {
    state.picker = Some(PickerOverlay::new(
        PickerPurpose::Namespace,
        "Namespace",
        state.namespaces.clone(),
        Some(state.current_ns),
    ));
}

pub fn open_kind_picker(state: &mut AppState) {
    let items: Vec<String> = ResourceKind::ALL
        .iter()
        .map(|k| k.label().to_string())
        .collect();
    state.picker = Some(PickerOverlay::new(
        PickerPurpose::ResourceKind,
        "Resource kind",
        items,
        current_kind_index(state),
    ));
}

/// Apply a picker's confirmed selection — switch namespace or kind
/// and trigger a refresh. `orig` is the `source`-index the picker
/// confirmed (`PickerOverlay::selected_orig_index`). Called by
/// [`crate::KubeApp::handle`] when the open `PickerOverlay` reports
/// `crate::state::PickerOutcome::Confirmed`.
pub fn commit_picker_selection(
    state: &mut AppState,
    purpose: PickerPurpose,
    orig: usize,
    rt: &tokio::runtime::Runtime,
) {
    match purpose {
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
