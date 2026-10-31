//! Routes an assistive-technology action onto the [`crate::event::UiEvent`]
//! a mouse or keyboard user would have produced, so the application never
//! sees a separate "accessibility" code path.
//!
//! [`route_action`] looks the target node up in the tree's owner
//! side-table ([`A11yTree::owner`]) to find which [`WidgetId`] produced it
//! and, for a child node, which key identifies it within that widget. It
//! then builds the matching `UiEvent` the same way a backend's input
//! handling would. The caller (a backend's `accesskit::ActionHandler`
//! glue) is responsible for feeding the returned events through the same
//! dispatch path a real input event takes — `route_action` only decides
//! *what* happened, not how it reaches the application.
//!
//! An action the target does not support, or a value of the wrong shape,
//! is logged through [`crate::diagnostics::emit`] and produces no events.
//! This module never panics on untrusted assistive-technology input.

use super::tree::{decode_tree_path, A11yOwnerKind};
use crate::a11y::{A11yChecked, A11yNodeId, A11yRole, A11yTree};
use crate::event::{Point, ScrollDelta, UiEvent};
use crate::primitives::form::FormEvent;
use crate::primitives::list::ListViewEvent;
use crate::primitives::tree::TreeEvent;
use crate::types::WidgetId;

/// An assistive-technology action, independent of any one platform's
/// accessibility API.
///
/// Mirrors the subset of `accesskit::Action` [`route_action`] knows how to
/// translate into a `UiEvent`. `#[non_exhaustive]` so a future variant
/// (e.g. `ScrollLeft`/`ScrollRight`) does not break a consumer's `match`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum A11yAction {
    /// A single click or tap — activates a button, list item, tree row,
    /// toggle or checkbox.
    Click,
    /// Moves keyboard/screen-reader focus to the target.
    Focus,
    /// Opens a collapsed tree row or disclosure.
    Expand,
    /// Closes an expanded tree row or disclosure.
    Collapse,
    /// Scrolls the target's container just enough to bring it into view.
    ScrollIntoView,
    /// Scrolls the target's container up by one step.
    ScrollUp,
    /// Scrolls the target's container down by one step.
    ScrollDown,
    /// Replaces the target's text value, e.g. from a screen reader's
    /// braille input or voice dictation.
    SetValue,
    /// Increments a slider-like value by one step.
    Increment,
    /// Decrements a slider-like value by one step.
    Decrement,
}

/// The payload a [`A11yAction::SetValue`] action carries.
#[derive(Debug, Clone, PartialEq)]
pub enum A11yActionValue {
    /// A new text value, for a text input or text area.
    Text(String),
    /// A new numeric value, for a slider or other ranged control.
    Number(f64),
}

/// Turns an assistive-technology action against `target` into the
/// `UiEvent`(s) the owning widget's normal input handling understands.
///
/// Returns an empty `Vec` — after logging why, through
/// [`crate::diagnostics::emit`] — when:
/// - `target` is not in `tree`, or its owning widget did not opt into
///   action routing (see [`A11yTree::owner`]);
/// - `action` is not one the target's kind supports;
/// - a `SetValue`/`Increment`/`Decrement` arrives for a kind this module
///   does not yet translate.
///
/// The last case is a deliberate scope boundary: [`super::A11yTreeNode::supported_actions`]
/// advertises actions by role and state alone, so a role this module has
/// not been taught to route yet (e.g. a dropdown, color picker or
/// segmented control field) still advertises `SetValue`-shaped actions an
/// assistive-technology client could request. Routing those is future
/// work; until then the request is a safe no-op rather than a panic.
pub fn route_action(
    tree: &A11yTree,
    target: A11yNodeId,
    action: A11yAction,
    value: Option<A11yActionValue>,
) -> Vec<UiEvent> {
    let Some((widget, key, kind)) = tree.owner(target) else {
        crate::diagnostics::emit(format!(
            "a11y: action {action:?} has no routable owner for node {target:?}"
        ));
        return Vec::new();
    };
    let widget = *widget;

    // Focus and scroll-into-view only need a `WidgetId`, so every
    // top-level widget supports them regardless of content kind.
    match action {
        A11yAction::Focus => return vec![UiEvent::FocusChanged(Some(widget))],
        A11yAction::ScrollIntoView => return vec![UiEvent::FocusChanged(Some(widget))],
        A11yAction::ScrollUp => return vec![scroll_event(widget, -1.0)],
        A11yAction::ScrollDown => return vec![scroll_event(widget, 1.0)],
        _ => {}
    }

    match kind {
        A11yOwnerKind::List => route_list_action(widget, key, action),
        A11yOwnerKind::Tree => route_tree_action(widget, key, action),
        A11yOwnerKind::Form => route_form_action(tree, target, widget, key, action, value),
        A11yOwnerKind::Unsupported => {
            crate::diagnostics::emit(format!(
                "a11y: widget {widget:?} does not support routed actions"
            ));
            Vec::new()
        }
    }
}

/// A synthetic scroll-wheel event routed at `widget` rather than at
/// whatever happens to be under the pointer. The position is a
/// placeholder — `widget` is what decides the target, not `position` —
/// since an assistive-technology scroll request carries no cursor
/// location.
fn scroll_event(widget: WidgetId, dy: f32) -> UiEvent {
    UiEvent::Scroll {
        widget: Some(widget),
        delta: ScrollDelta::new(0.0, dy),
        position: Point::new(0.0, 0.0),
    }
}

fn route_list_action(widget: WidgetId, key: &str, action: A11yAction) -> Vec<UiEvent> {
    let Some(idx) = key
        .strip_prefix("item.")
        .and_then(|s| s.parse::<usize>().ok())
    else {
        crate::diagnostics::emit(format!(
            "a11y: list action {action:?} targets malformed key {key:?} on widget {widget:?}"
        ));
        return Vec::new();
    };
    match action {
        A11yAction::Click => vec![UiEvent::List(widget, ListViewEvent::ItemActivated { idx })],
        _ => {
            crate::diagnostics::emit(format!(
                "a11y: list widget {widget:?} does not support action {action:?}"
            ));
            Vec::new()
        }
    }
}

fn route_tree_action(widget: WidgetId, key: &str, action: A11yAction) -> Vec<UiEvent> {
    let Some(path) = key.strip_prefix("row.").and_then(decode_tree_path) else {
        crate::diagnostics::emit(format!(
            "a11y: tree action {action:?} targets malformed key {key:?} on widget {widget:?}"
        ));
        return Vec::new();
    };
    match action {
        A11yAction::Click => vec![UiEvent::Tree(widget, TreeEvent::RowDoubleClicked { path })],
        // The toggle event itself carries no direction — the primitive
        // flips whatever state the row is currently in, same as a mouse
        // click on its disclosure triangle.
        A11yAction::Expand | A11yAction::Collapse => {
            vec![UiEvent::Tree(widget, TreeEvent::RowToggleExpand { path })]
        }
        _ => {
            crate::diagnostics::emit(format!(
                "a11y: tree widget {widget:?} does not support action {action:?}"
            ));
            Vec::new()
        }
    }
}

fn route_form_action(
    tree: &A11yTree,
    target: A11yNodeId,
    widget: WidgetId,
    key: &str,
    action: A11yAction,
    value: Option<A11yActionValue>,
) -> Vec<UiEvent> {
    let Some(field_key) = key.strip_prefix("field.") else {
        crate::diagnostics::emit(format!(
            "a11y: form action {action:?} targets malformed key {key:?} on widget {widget:?}"
        ));
        return Vec::new();
    };
    let field_id = WidgetId::new(field_key);
    let role = tree.node(target).map(A11yTreeNodeRoleExt::role_of);

    let event = match (action, role) {
        (A11yAction::Click, Some(A11yRole::Button)) => {
            Some(FormEvent::ButtonClicked { id: field_id })
        }
        (A11yAction::Click, Some(A11yRole::CheckBox)) => {
            let current = tree
                .node(target)
                .and_then(|node| node.info.checked)
                .map(|checked| checked == A11yChecked::True)
                .unwrap_or(false);
            Some(FormEvent::ToggleChanged {
                id: field_id,
                value: !current,
            })
        }
        (A11yAction::SetValue, Some(A11yRole::TextInput)) => match value {
            Some(A11yActionValue::Text(text)) => Some(FormEvent::TextInputCommitted {
                id: field_id,
                value: text,
            }),
            _ => {
                crate::diagnostics::emit(format!(
                    "a11y: SetValue on text field {field_id:?} needs a text value"
                ));
                None
            }
        },
        (A11yAction::Increment | A11yAction::Decrement, Some(A11yRole::Slider)) => tree
            .node(target)
            .and_then(|node| node.info.value_range)
            .map(|range| {
                let step = slider_step(range.min, range.max);
                let delta = if action == A11yAction::Increment {
                    step
                } else {
                    -step
                };
                let next = (range.current + delta).clamp(range.min, range.max);
                FormEvent::SliderChanged {
                    id: field_id,
                    value: next as f32,
                }
            }),
        _ => {
            crate::diagnostics::emit(format!(
                "a11y: form field {field_id:?} does not support action {action:?}"
            ));
            None
        }
    };

    match event {
        Some(event) => vec![UiEvent::Form(widget, event)],
        None => Vec::new(),
    }
}

/// A single step for a slider's `Increment`/`Decrement`, derived as 1% of
/// its range. [`crate::a11y::A11yValueRange`] carries no step size of its
/// own — the field's real configured step lives on
/// [`crate::primitives::form::FieldKind::Slider`], which is not visible
/// from the accessibility tree — so this is an approximation, not the
/// field's actual increment.
fn slider_step(min: f64, max: f64) -> f64 {
    let range = max - min;
    if range > 0.0 {
        range / 100.0
    } else {
        1.0
    }
}

/// Resolves a node's role without borrowing it past the match above.
trait A11yTreeNodeRoleExt {
    fn role_of(self) -> A11yRole;
}

impl A11yTreeNodeRoleExt for &super::A11yTreeNode {
    fn role_of(self) -> A11yRole {
        self.role()
    }
}

#[cfg(feature = "a11y")]
#[cfg_attr(docsrs, doc(cfg(feature = "a11y")))]
impl From<A11yAction> for accesskit::Action {
    fn from(action: A11yAction) -> Self {
        match action {
            A11yAction::Click => accesskit::Action::Click,
            A11yAction::Focus => accesskit::Action::Focus,
            A11yAction::Expand => accesskit::Action::Expand,
            A11yAction::Collapse => accesskit::Action::Collapse,
            A11yAction::ScrollIntoView => accesskit::Action::ScrollIntoView,
            A11yAction::ScrollUp => accesskit::Action::ScrollUp,
            A11yAction::ScrollDown => accesskit::Action::ScrollDown,
            A11yAction::SetValue => accesskit::Action::SetValue,
            A11yAction::Increment => accesskit::Action::Increment,
            A11yAction::Decrement => accesskit::Action::Decrement,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Surface;
    use crate::primitives::form::{FieldKind, Form, FormField};
    use crate::primitives::list::{ListItem, ListView};
    use crate::primitives::tree::{TreeRow, TreeView};
    use crate::testing::a11y::A11yScenario;
    use crate::types::{Decoration, SelectionMode, StyledText, TreeStyle};
    use crate::Rect;

    fn list_surface(id: &str, items: &[&str], selected: usize) -> Surface {
        Surface::List {
            rect: Rect::new(0.0, 0.0, 20.0, 10.0),
            list: ListView {
                id: WidgetId::new(id),
                title: Some(StyledText::plain("Files")),
                items: items
                    .iter()
                    .map(|text| ListItem {
                        text: StyledText::plain(*text),
                        icon: None,
                        detail: None,
                        decoration: Decoration::default(),
                    })
                    .collect(),
                selected_idx: selected,
                scroll_offset: 0,
                has_focus: false,
                bordered: false,
                h_scroll: 0,
                max_content_width: None,
                show_v_scrollbar: false,
            },
        }
    }

    fn tree_surface(id: &str, rows: Vec<TreeRow>) -> Surface {
        Surface::Tree {
            rect: Rect::new(0.0, 0.0, 20.0, 10.0),
            tree: TreeView {
                id: WidgetId::new(id),
                rows,
                selection_mode: SelectionMode::Single,
                selected_path: None,
                scroll_offset: 0,
                style: TreeStyle::default(),
                has_focus: false,
            },
        }
    }

    fn row(label: &str, path: &[u16]) -> TreeRow {
        TreeRow {
            path: path.to_vec(),
            indent: path.len().saturating_sub(1) as u16,
            icon: None,
            text: StyledText::plain(label),
            badge: None,
            is_expanded: None,
            decoration: Decoration::default(),
            edit: None,
        }
    }

    fn form_surface(id: &str, fields: Vec<FormField>) -> Surface {
        Surface::Form {
            rect: Rect::new(0.0, 0.0, 20.0, 10.0),
            form: Form {
                id: WidgetId::new(id),
                fields,
                focused_field: None,
                scroll_offset: 0,
                has_focus: false,
                label_position: Default::default(),
                label_align: Default::default(),
                label_width: None,
            },
        }
    }

    fn field(id: &str, label: &str, kind: FieldKind) -> FormField {
        FormField {
            id: WidgetId::new(id),
            label: StyledText::plain(label),
            kind,
            hint: StyledText::plain(""),
            disabled: false,
            validation: None,
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
        }
    }

    #[test]
    fn focus_resolves_to_the_owning_widget() {
        let snapshot = A11yScenario::new()
            .with_surface(list_surface("files", &["a.rs"], 0))
            .snapshot();
        let node = snapshot.find("Files").expect("list node").id;

        let events = route_action(snapshot.tree(), node, A11yAction::Focus, None);
        assert_eq!(
            events,
            vec![UiEvent::FocusChanged(Some(WidgetId::new("files")))]
        );
    }

    #[test]
    fn click_on_list_item_activates_it() {
        let snapshot = A11yScenario::new()
            .with_surface(list_surface("files", &["a.rs", "b.rs"], 0))
            .snapshot();
        let node = snapshot.find("b.rs").expect("row").id;

        let events = route_action(snapshot.tree(), node, A11yAction::Click, None);
        assert_eq!(
            events,
            vec![UiEvent::List(
                WidgetId::new("files"),
                ListViewEvent::ItemActivated { idx: 1 }
            )]
        );
    }

    #[test]
    fn click_and_expand_on_tree_row_round_trip_the_path() {
        let snapshot = A11yScenario::new()
            .with_surface(tree_surface("outline", vec![row("root", &[0])]))
            .snapshot();
        let node = snapshot.find("root").expect("row").id;

        let clicked = route_action(snapshot.tree(), node, A11yAction::Click, None);
        assert_eq!(
            clicked,
            vec![UiEvent::Tree(
                WidgetId::new("outline"),
                TreeEvent::RowDoubleClicked { path: vec![0] }
            )]
        );

        let expanded = route_action(snapshot.tree(), node, A11yAction::Expand, None);
        assert_eq!(
            expanded,
            vec![UiEvent::Tree(
                WidgetId::new("outline"),
                TreeEvent::RowToggleExpand { path: vec![0] }
            )]
        );
    }

    #[test]
    fn click_on_form_button_emits_button_clicked() {
        let snapshot = A11yScenario::new()
            .with_surface(form_surface(
                "prefs",
                vec![field("save", "Save", FieldKind::Button)],
            ))
            .snapshot();
        let node = snapshot.find("Save").expect("field").id;

        let events = route_action(snapshot.tree(), node, A11yAction::Click, None);
        assert_eq!(
            events,
            vec![UiEvent::Form(
                WidgetId::new("prefs"),
                FormEvent::ButtonClicked {
                    id: WidgetId::new("save")
                }
            )]
        );
    }

    #[test]
    fn click_on_form_checkbox_flips_it() {
        let snapshot = A11yScenario::new()
            .with_surface(form_surface(
                "prefs",
                vec![field(
                    "wrap",
                    "Word wrap",
                    FieldKind::Toggle { value: false },
                )],
            ))
            .snapshot();
        let node = snapshot.find("Word wrap").expect("field").id;

        let events = route_action(snapshot.tree(), node, A11yAction::Click, None);
        assert_eq!(
            events,
            vec![UiEvent::Form(
                WidgetId::new("prefs"),
                FormEvent::ToggleChanged {
                    id: WidgetId::new("wrap"),
                    value: true
                }
            )]
        );
    }

    #[test]
    fn set_value_on_text_field_commits_text() {
        let snapshot = A11yScenario::new()
            .with_surface(form_surface(
                "prefs",
                vec![field(
                    "name",
                    "Name",
                    FieldKind::TextInput {
                        value: String::new(),
                        placeholder: String::new(),
                        cursor: None,
                        selection_anchor: None,
                    },
                )],
            ))
            .snapshot();
        let node = snapshot.find("Name").expect("field").id;

        let events = route_action(
            snapshot.tree(),
            node,
            A11yAction::SetValue,
            Some(A11yActionValue::Text("Ada".into())),
        );
        assert_eq!(
            events,
            vec![UiEvent::Form(
                WidgetId::new("prefs"),
                FormEvent::TextInputCommitted {
                    id: WidgetId::new("name"),
                    value: "Ada".into()
                }
            )]
        );
    }

    #[test]
    fn increment_on_slider_clamps_to_max() {
        let snapshot = A11yScenario::new()
            .with_surface(form_surface(
                "prefs",
                vec![field(
                    "volume",
                    "Volume",
                    FieldKind::Slider {
                        value: 99.6,
                        min: 0.0,
                        max: 100.0,
                        step: 1.0,
                    },
                )],
            ))
            .snapshot();
        let node = snapshot.find("Volume").expect("field").id;

        let events = route_action(snapshot.tree(), node, A11yAction::Increment, None);
        assert_eq!(
            events,
            vec![UiEvent::Form(
                WidgetId::new("prefs"),
                FormEvent::SliderChanged {
                    id: WidgetId::new("volume"),
                    value: 100.0
                }
            )]
        );
    }

    #[test]
    fn scroll_actions_target_the_widget_not_the_pointer() {
        let snapshot = A11yScenario::new()
            .with_surface(list_surface("files", &["a.rs"], 0))
            .snapshot();
        let node = snapshot.find("Files").expect("list node").id;

        let up = route_action(snapshot.tree(), node, A11yAction::ScrollUp, None);
        assert_eq!(
            up,
            vec![UiEvent::Scroll {
                widget: Some(WidgetId::new("files")),
                delta: ScrollDelta::new(0.0, -1.0),
                position: Point::new(0.0, 0.0),
            }]
        );
    }

    #[test]
    fn unknown_node_id_is_logged_not_panicked() {
        let snapshot = A11yScenario::new()
            .with_surface(list_surface("files", &["a.rs"], 0))
            .snapshot();
        let bogus = A11yNodeId::for_key("nonexistent");

        let events = route_action(snapshot.tree(), bogus, A11yAction::Click, None);
        assert!(events.is_empty());
    }

    #[test]
    fn scope_excluded_field_kind_is_a_safe_no_op() {
        let snapshot = A11yScenario::new()
            .with_surface(form_surface(
                "prefs",
                vec![field(
                    "theme",
                    "Theme",
                    FieldKind::Dropdown {
                        options: vec![StyledText::plain("Dark")],
                        selected_idx: 0,
                    },
                )],
            ))
            .snapshot();
        let node = snapshot.find("Theme").expect("field").id;

        let events = route_action(snapshot.tree(), node, A11yAction::Click, None);
        assert!(events.is_empty());
    }
}
