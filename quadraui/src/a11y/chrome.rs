//! Accessibility mappings for controls and application chrome.
//!
//! One [`A11yPrimitive`] impl per chrome primitive: tab bars, activity
//! bars, menus, status bars, dialogs, forms, toasts, tooltips, progress
//! indicators, scrollbars, splits and panels. The impls live here rather
//! than next to each primitive so the descriptors stay pure data and the
//! whole role/state vocabulary can be read in one place.
//!
//! Three conventions hold throughout:
//!
//! * **Nothing vanishes.** A purely visual part — a scrollbar thumb, a
//!   split divider, a spinner's animated glyph — is emitted as an
//!   [`A11yRole::Presentation`] child rather than silently dropped, so a
//!   tree snapshot still shows the whole structure.
//! * **Live regions are declared, not inferred.** Toast overlays and the
//!   individual toasts inside them are [`A11yLive::Polite`]: an assistive
//!   technology announces them when they appear without stealing focus.
//! * **Modality is derived, not declared.** A [`Dialog`] does not assert
//!   `modal` itself; the tree builder marks whichever surface the
//!   backend's modal stack names and hides the rest. See
//!   [`crate::a11y::frame_to_tree_with_modal`].
//!
//! Child keys are stable and local to their parent, so node ids survive
//! across frames: a widget-owned child is keyed by its `WidgetId`
//! (`button.save`), an anonymous one by its index (`tab.2`).

use super::tree::{in_set, non_empty, plain_text, A11yChild, A11yPrimitive};
use crate::a11y::{A11yChecked, A11yInfo, A11yLive, A11yRole, A11yValueRange};
use crate::primitives::activity_bar::{ActivityBar, ActivityItem};
use crate::primitives::combo_box::ComboBox;
use crate::primitives::command_center::CommandCenter;
use crate::primitives::command_line::CommandLine;
use crate::primitives::context_menu::{ContextMenu, ContextMenuItem};
use crate::primitives::date_picker::DatePicker;
use crate::primitives::dialog::{Dialog, DialogInput, DialogTable};
use crate::primitives::form::{FieldKind, Form, FormField, ValidationState};
use crate::primitives::menu_bar::MenuBar;
use crate::primitives::panel::Panel;
use crate::primitives::progress::ProgressBar;
use crate::primitives::rich_text_popup::RichTextPopup;
use crate::primitives::scrollbar::{ScrollAxis, Scrollbar};
use crate::primitives::sidebar_panel::SidebarPanel;
use crate::primitives::spinner::Spinner;
use crate::primitives::split::{Split, SplitDirection};
use crate::primitives::split_tree::SplitTree;
use crate::primitives::status_bar::{StatusBar, StatusBarSegment};
use crate::primitives::tab_bar::TabBar;
use crate::primitives::toast::{Toast, ToastOverlay, ToastSeverity};
use crate::primitives::tooltip::Tooltip;

/// A node with no semantics of its own, kept so the tree still shows the
/// structure a sighted user sees.
fn presentational(key: impl Into<String>, label: &str) -> A11yChild {
    A11yChild::new(
        key,
        A11yInfo::new()
            .with_role(A11yRole::Presentation)
            .with_label(label),
    )
}

/// A menu label with its Alt-activation marker removed: `"&File"`
/// announces as `File`, and a literal ampersand is written `"&&"`.
/// The marker is a visual underline cue, not part of the name.
fn strip_mnemonic(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut chars = label.chars();
    while let Some(ch) = chars.next() {
        if ch != '&' {
            out.push(ch);
        } else if chars.as_str().starts_with('&') {
            chars.next();
            out.push('&');
        }
    }
    out
}

/// The axis name a scrollbar or split reports as its value.
fn direction_name(direction: SplitDirection) -> &'static str {
    match direction {
        SplitDirection::Horizontal => "horizontal",
        SplitDirection::Vertical => "vertical",
    }
}

// ---------------------------------------------------------------- tab bar

impl A11yPrimitive for TabBar {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new()
            .with_role(A11yRole::TabList)
            .with_label("Tab bar")
            .with_set_size(self.tabs.len() as u32)
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        let total = self.tabs.len();
        let mut children = Vec::with_capacity(total + self.right_segments.len());
        for (idx, tab) in self.tabs.iter().enumerate() {
            let mut info = in_set(
                A11yInfo::new()
                    .with_role(A11yRole::Tab)
                    .with_label(&tab.label)
                    .with_selected(tab.is_active),
                idx,
                total,
            );
            if tab.is_dirty {
                info = info.with_value("modified");
            } else if tab.is_preview {
                info = info.with_value("preview");
            }
            let mut tab_child = A11yChild::new(format!("tab.{idx}"), info);
            if self.show_tab_close && tab.is_closable {
                tab_child = tab_child.with_children(vec![A11yChild::new(
                    "close",
                    A11yInfo::new()
                        .with_role(A11yRole::Button)
                        .with_label(format!("Close {}", tab.label)),
                )]);
            }
            children.push(tab_child);
        }
        for (idx, segment) in self.right_segments.iter().enumerate() {
            let role = match segment.id {
                Some(_) => A11yRole::Button,
                None => A11yRole::Label,
            };
            children.push(A11yChild::new(
                format!("segment.{idx}"),
                A11yInfo::new()
                    .with_role(role)
                    .with_label(&segment.text)
                    .with_selected(segment.is_active),
            ));
        }
        children
    }

    fn a11y_focus_key(&self) -> Option<String> {
        let idx = self.tabs.iter().position(|tab| tab.is_active)?;
        Some(format!("tab.{idx}"))
    }
}

// ----------------------------------------------------------- activity bar

impl A11yPrimitive for ActivityBar {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new()
            .with_role(A11yRole::Toolbar)
            .with_label("Activity bar")
            .with_set_size(self.top_items.len() as u32 + self.bottom_items.len() as u32)
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        let total = self.top_items.len() + self.bottom_items.len();
        let mut children = Vec::with_capacity(total);
        let sides = [("top", &self.top_items), ("bottom", &self.bottom_items)];
        let mut idx = 0;
        for (side, items) in sides {
            for item in items.iter() {
                children.push(A11yChild::new(
                    activity_child_key(side, item),
                    in_set(
                        A11yInfo::new()
                            .with_role(A11yRole::Tab)
                            .with_label(&item.tooltip)
                            .with_selected(item.is_active),
                        idx,
                        total,
                    ),
                ));
                idx += 1;
            }
        }
        children
    }

    fn a11y_focus_key(&self) -> Option<String> {
        if !self.is_keyboard_focused {
            return None;
        }
        let find = |side: &'static str, items: &[ActivityItem]| {
            items
                .iter()
                .find(|item| item.is_keyboard_selected)
                .map(|item| activity_child_key(side, item))
        };
        find("top", &self.top_items).or_else(|| find("bottom", &self.bottom_items))
    }
}

fn activity_child_key(side: &str, item: &ActivityItem) -> String {
    format!("{side}.{}", item.id.as_str())
}

// --------------------------------------------------------------- menu bar

impl A11yPrimitive for MenuBar {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new()
            .with_role(A11yRole::MenuBar)
            .with_label("Menu bar")
            .with_set_size(self.items.len() as u32)
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        let total = self.items.len();
        self.items
            .iter()
            .enumerate()
            .map(|(idx, item)| {
                let name = strip_mnemonic(&item.label);
                let info = in_set(
                    A11yInfo::new()
                        .with_role(A11yRole::MenuItem)
                        .with_label(&name)
                        .with_disabled(item.disabled)
                        .with_expanded(self.open_item == Some(idx)),
                    idx,
                    total,
                );
                let child = A11yChild::new(format!("item.{}", item.id.as_str()), info);
                match &item.submenu {
                    Some(items) => child.with_children(vec![A11yChild::new(
                        "menu",
                        A11yInfo::new()
                            .with_role(A11yRole::Menu)
                            .with_label(&name)
                            .with_set_size(items.len() as u32),
                    )
                    .with_children(context_menu_children(items))]),
                    None => child,
                }
            })
            .collect()
    }

    fn a11y_focus_key(&self) -> Option<String> {
        let idx = self.focused_item.or(self.open_item)?;
        let item = self.items.get(idx)?;
        Some(format!("item.{}", item.id.as_str()))
    }
}

// ----------------------------------------------------------- context menu

impl A11yPrimitive for ContextMenu {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new()
            .with_role(A11yRole::Menu)
            .with_label("Context menu")
            .with_set_size(self.items.len() as u32)
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        context_menu_children(&self.items)
    }

    fn a11y_focus_key(&self) -> Option<String> {
        let item = self.items.get(self.selected_idx)?;
        (!item.is_separator()).then(|| context_menu_key(self.selected_idx, item))
    }
}

fn context_menu_key(idx: usize, item: &ContextMenuItem) -> String {
    match &item.id {
        Some(id) => format!("item.{}", id.as_str()),
        None => format!("item.{idx}"),
    }
}

fn context_menu_children(items: &[ContextMenuItem]) -> Vec<A11yChild> {
    let total = items.len();
    items
        .iter()
        .enumerate()
        .map(|(idx, item)| {
            if item.is_separator() {
                return presentational(format!("separator.{idx}"), "Separator");
            }
            let mut info = in_set(
                A11yInfo::new()
                    .with_role(A11yRole::MenuItem)
                    .with_label(plain_text(&item.label))
                    .with_disabled(item.disabled),
                idx,
                total,
            );
            if let Some(checked) = item.checked {
                info = info.with_checked(if checked {
                    A11yChecked::True
                } else {
                    A11yChecked::False
                });
            }
            if let Some(detail) = &item.detail {
                if let Some(value) = non_empty(plain_text(detail)) {
                    info = info.with_value(value);
                }
            }
            if item.submenu.is_some() {
                info = info.with_expanded(false);
            }
            let child = A11yChild::new(context_menu_key(idx, item), info);
            match &item.submenu {
                Some(sub) => child.with_children(vec![A11yChild::new(
                    "menu",
                    A11yInfo::new()
                        .with_role(A11yRole::Menu)
                        .with_label(plain_text(&item.label))
                        .with_set_size(sub.len() as u32),
                )
                .with_children(context_menu_children(sub))]),
                None => child,
            }
        })
        .collect()
}

// ------------------------------------------------------------- status bar

impl A11yPrimitive for StatusBar {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new()
            .with_role(A11yRole::Status)
            .with_label("Status bar")
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        let mut children = Vec::new();
        for (side, segments) in [
            ("left", &self.left_segments),
            ("right", &self.right_segments),
        ] {
            for (idx, segment) in segments.iter().enumerate() {
                if let Some(child) = status_segment(side, idx, segment) {
                    children.push(child);
                }
            }
        }
        children
    }
}

fn status_segment(side: &str, idx: usize, segment: &StatusBarSegment) -> Option<A11yChild> {
    // Segments carry their own visual padding (`" main "`). An announced
    // name must not, so the label is the trimmed text.
    let label = non_empty(segment.text.trim().to_string())?;
    let role = match segment.action_id {
        Some(_) => A11yRole::Button,
        None => A11yRole::Label,
    };
    Some(A11yChild::new(
        format!("{side}.{idx}"),
        A11yInfo::new().with_role(role).with_label(label),
    ))
}

// --------------------------------------------------------- command center

impl A11yPrimitive for CommandCenter {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new()
            .with_role(A11yRole::Toolbar)
            .with_label("Command center")
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        vec![
            A11yChild::new(
                "back",
                A11yInfo::new()
                    .with_role(A11yRole::Button)
                    .with_label("Back")
                    .with_disabled(!self.back_enabled),
            ),
            A11yChild::new(
                "forward",
                A11yInfo::new()
                    .with_role(A11yRole::Button)
                    .with_label("Forward")
                    .with_disabled(!self.forward_enabled),
            ),
            A11yChild::new(
                "search",
                A11yInfo::new()
                    .with_role(A11yRole::TextInput)
                    .with_label(&self.search_label),
            ),
        ]
    }

    fn a11y_focus_key(&self) -> Option<String> {
        Some("search".to_string())
    }
}

// ------------------------------------------------------------ command line

impl A11yPrimitive for CommandLine {
    fn a11y_info(&self) -> A11yInfo {
        let info = A11yInfo::new()
            .with_role(A11yRole::TextInput)
            .with_label("Command line")
            .with_read_only(self.cursor_offset.is_none());
        match non_empty(self.text.clone()) {
            Some(text) => info.with_value(text),
            None => info,
        }
    }
}

// ----------------------------------------------------------------- dialog

impl A11yPrimitive for Dialog {
    /// Deliberately does not assert `modal`: the backend's modal stack is
    /// the authority on which dialog is on top, and the tree builder
    /// applies it (see [`crate::a11y::frame_to_tree_with_modal`]).
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new()
            .with_role(A11yRole::Dialog)
            .with_label(plain_text(&self.title))
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        let mut children = Vec::new();
        for (idx, line) in self.body.iter().enumerate() {
            if let Some(text) = non_empty(plain_text(line)) {
                children.push(A11yChild::new(
                    format!("body.{idx}"),
                    A11yInfo::new().with_role(A11yRole::Label).with_label(text),
                ));
            }
        }
        if let Some(table) = &self.table {
            children.push(dialog_table(table));
        }
        match &self.input {
            Some(DialogInput::TextInput(input)) => {
                let info = A11yInfo::new()
                    .with_role(A11yRole::TextInput)
                    .with_label(
                        non_empty(input.placeholder.clone()).unwrap_or_else(|| "Input".into()),
                    )
                    .with_read_only(input.cursor.is_none());
                let info = match non_empty(input.value.clone()) {
                    Some(value) => info.with_value(value),
                    None => info,
                };
                children.push(A11yChild::new("input", info));
            }
            Some(DialogInput::Toolbar(bar)) => {
                children.push(
                    A11yChild::new("input", bar.a11y_info()).with_children(bar.a11y_children()),
                );
            }
            None => {}
        }
        let total = self.buttons.len();
        for (idx, button) in self.buttons.iter().enumerate() {
            let mut info = in_set(
                A11yInfo::new()
                    .with_role(A11yRole::Button)
                    .with_label(&button.label),
                idx,
                total,
            );
            if button.is_default {
                info = info.with_value("default");
            } else if button.is_cancel {
                info = info.with_value("cancel");
            }
            children.push(A11yChild::new(
                format!("button.{}", button.id.as_str()),
                info,
            ));
        }
        children
    }

    fn a11y_focus_key(&self) -> Option<String> {
        let button = self
            .buttons
            .iter()
            .find(|button| button.is_default)
            .or_else(|| self.buttons.first())?;
        Some(format!("button.{}", button.id.as_str()))
    }
}

fn dialog_table(table: &DialogTable) -> A11yChild {
    let mut rows = Vec::with_capacity(table.rows.len() + 1);
    if let Some(headers) = &table.headers {
        rows.push(
            A11yChild::new(
                "header",
                A11yInfo::new()
                    .with_role(A11yRole::Row)
                    .with_label("Header"),
            )
            .with_children(table_cells(headers)),
        );
    }
    for (idx, row) in table.rows.iter().enumerate() {
        rows.push(
            A11yChild::new(
                format!("row.{idx}"),
                in_set(
                    A11yInfo::new().with_role(A11yRole::Row),
                    idx,
                    table.rows.len(),
                ),
            )
            .with_children(table_cells(row)),
        );
    }
    A11yChild::new(
        "table",
        A11yInfo::new()
            .with_role(A11yRole::Table)
            .with_set_size(table.rows.len() as u32),
    )
    .with_children(rows)
}

fn table_cells(cells: &[String]) -> Vec<A11yChild> {
    cells
        .iter()
        .enumerate()
        .map(|(idx, cell)| {
            A11yChild::new(
                format!("cell.{idx}"),
                in_set(
                    A11yInfo::new().with_role(A11yRole::Cell).with_label(cell),
                    idx,
                    cells.len(),
                ),
            )
        })
        .collect()
}

// ------------------------------------------------------------------- form

impl A11yPrimitive for Form {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new()
            .with_role(A11yRole::Form)
            .with_label("Form")
            .with_set_size(self.fields.len() as u32)
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        let total = self.fields.len();
        self.fields
            .iter()
            .enumerate()
            .map(|(idx, field)| form_field(idx, total, field))
            .collect()
    }

    fn a11y_focus_key(&self) -> Option<String> {
        let focused = self.focused_field.as_ref()?;
        self.fields
            .iter()
            .find(|field| &field.id == focused)
            .map(|field| format!("field.{}", field.id.as_str()))
    }
}

fn form_field(idx: usize, total: usize, field: &FormField) -> A11yChild {
    let label = plain_text(&field.label);
    let (role, info, mut children) = field_kind(&field.kind);
    // A kind that counts its own options (a dropdown, a radio group, a
    // combo box) already carries a `set_size` describing *that* set, not
    // the form's fields. Pairing the field's position among the fields
    // with the kind's own total would announce a position and size drawn
    // from two different sets, so position/size in the form's set is
    // reported only when the kind does not already own one.
    let kind_owns_set_size = info.set_size.is_some();
    let mut info = info
        .with_role(role)
        .with_label(label)
        .with_disabled(field.disabled);
    if !kind_owns_set_size {
        info = info
            .with_position_in_set(idx as u32 + 1)
            .with_set_size(total as u32);
    }
    if info.value.is_none() {
        if let Some(hint) = non_empty(plain_text(&field.hint)) {
            info = info.with_value(hint);
        }
    }
    if let Some(validation) = &field.validation {
        let (kind, message) = match validation {
            ValidationState::Error(message) => ("Error", message),
            ValidationState::Warning(message) => ("Warning", message),
        };
        children.push(A11yChild::new(
            "validation",
            A11yInfo::new()
                .with_role(A11yRole::Alert)
                .with_label(format!("{kind}: {message}"))
                .with_live(A11yLive::Polite),
        ));
    }
    A11yChild::new(format!("field.{}", field.id.as_str()), info).with_children(children)
}

/// Role, state and children for one field's kind. The caller supplies the
/// role-independent parts (label, disabled, position in set).
fn field_kind(kind: &FieldKind) -> (A11yRole, A11yInfo, Vec<A11yChild>) {
    let plain = A11yInfo::new();
    match kind {
        FieldKind::Label => (A11yRole::Label, plain, Vec::new()),
        FieldKind::Toggle { value } => (
            A11yRole::CheckBox,
            plain.with_checked(if *value {
                A11yChecked::True
            } else {
                A11yChecked::False
            }),
            Vec::new(),
        ),
        FieldKind::TextInput {
            value,
            placeholder,
            cursor,
            ..
        } => (
            A11yRole::TextInput,
            text_entry(value, placeholder, *cursor),
            Vec::new(),
        ),
        FieldKind::TextArea {
            value,
            placeholder,
            cursor,
            ..
        } => (
            A11yRole::TextInput,
            text_entry(value, placeholder, *cursor),
            Vec::new(),
        ),
        FieldKind::PasswordInput {
            value,
            placeholder,
            cursor,
            mask_char,
        } => {
            let masked: String = std::iter::repeat_n(*mask_char, value.chars().count()).collect();
            (
                A11yRole::TextInput,
                text_entry(&masked, placeholder, *cursor),
                Vec::new(),
            )
        }
        FieldKind::Button => (A11yRole::Button, plain, Vec::new()),
        FieldKind::ReadOnly { value } => (
            A11yRole::Label,
            match non_empty(plain_text(value)) {
                Some(text) => plain.with_value(text),
                None => plain,
            }
            .with_read_only(true),
            Vec::new(),
        ),
        FieldKind::Slider {
            value,
            min,
            max,
            step: _,
        } => (
            A11yRole::Slider,
            plain.with_value_range(A11yValueRange::new(*value as f64, *min as f64, *max as f64)),
            Vec::new(),
        ),
        FieldKind::ColorPicker { .. } => (A11yRole::Button, plain, Vec::new()),
        FieldKind::Dropdown {
            options,
            selected_idx,
        } => {
            let info = match options.get(*selected_idx) {
                Some(option) => plain.with_value(plain_text(option)),
                None => plain,
            };
            (
                A11yRole::ComboBox,
                info.with_set_size(options.len() as u32),
                Vec::new(),
            )
        }
        FieldKind::SegmentedControl {
            options,
            selected_idx,
        } => {
            let children = options
                .iter()
                .enumerate()
                .map(|(idx, option)| {
                    A11yChild::new(
                        format!("option.{idx}"),
                        in_set(
                            A11yInfo::new()
                                .with_role(A11yRole::RadioButton)
                                .with_label(option)
                                .with_selected(idx == *selected_idx),
                            idx,
                            options.len(),
                        ),
                    )
                })
                .collect();
            (
                A11yRole::RadioGroup,
                plain.with_set_size(options.len() as u32),
                children,
            )
        }
        FieldKind::ToggleGroup { toggles } => {
            let children = toggles
                .iter()
                .enumerate()
                .map(|(idx, toggle)| {
                    A11yChild::new(
                        format!("toggle.{}", toggle.id.as_str()),
                        in_set(
                            A11yInfo::new()
                                .with_role(A11yRole::CheckBox)
                                .with_label(&toggle.label)
                                .with_checked(if toggle.value {
                                    A11yChecked::True
                                } else {
                                    A11yChecked::False
                                }),
                            idx,
                            toggles.len(),
                        ),
                    )
                })
                .collect();
            (A11yRole::Group, plain, children)
        }
        FieldKind::ButtonRow { buttons } => {
            let children = buttons
                .iter()
                .enumerate()
                .map(|(idx, button)| {
                    A11yChild::new(
                        format!("button.{}", button.id.as_str()),
                        in_set(
                            A11yInfo::new()
                                .with_role(A11yRole::Button)
                                .with_label(&button.label)
                                .with_disabled(button.disabled),
                            idx,
                            buttons.len(),
                        ),
                    )
                })
                .collect();
            (A11yRole::Group, plain, children)
        }
        FieldKind::Toolbar(bar) => (A11yRole::Toolbar, plain, bar.a11y_children()),
        FieldKind::DatePicker(picker) => (A11yRole::Group, picker.a11y_info(), Vec::new()),
        FieldKind::ComboBox(combo) => (A11yRole::ComboBox, combo.a11y_info(), Vec::new()),
    }
}

/// Shared state for the three text-entry field kinds: the current text as
/// the value, and read-only whenever no cursor is placed.
fn text_entry(value: &str, placeholder: &str, cursor: Option<usize>) -> A11yInfo {
    let info = A11yInfo::new().with_read_only(cursor.is_none());
    match non_empty(value.to_string()).or_else(|| non_empty(placeholder.to_string())) {
        Some(text) => info.with_value(text),
        None => info,
    }
}

// --------------------------------------------------------------- combo box

impl A11yPrimitive for ComboBox {
    fn a11y_info(&self) -> A11yInfo {
        let info = A11yInfo::new()
            .with_role(A11yRole::ComboBox)
            .with_expanded(self.open)
            .with_disabled(self.disabled)
            .with_set_size(self.options.len() as u32);
        match non_empty(self.text.clone()) {
            Some(text) => info.with_value(text),
            None => info,
        }
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        if !self.open {
            return Vec::new();
        }
        let total = self.options.len();
        self.options
            .iter()
            .enumerate()
            .map(|(idx, option)| {
                A11yChild::new(
                    format!("option.{idx}"),
                    in_set(
                        A11yInfo::new()
                            .with_role(A11yRole::ListItem)
                            .with_label(option)
                            .with_selected(self.highlighted == Some(idx)),
                        idx,
                        total,
                    ),
                )
            })
            .collect()
    }
}

// ------------------------------------------------------------- date picker

impl A11yPrimitive for DatePicker {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new()
            .with_role(A11yRole::Group)
            .with_label("Date picker")
    }
}

// ------------------------------------------------------------------ toasts

impl A11yPrimitive for ToastOverlay {
    /// A polite live region: an assistive technology announces newly
    /// appearing toasts without moving focus away from the user's work.
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new()
            .with_role(A11yRole::Status)
            .with_label("Notifications")
            .with_live(A11yLive::Polite)
            .with_set_size(self.toasts.len() as u32)
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        let total = self.toasts.len();
        self.toasts
            .iter()
            .enumerate()
            .map(|(idx, toast)| toast_child(idx, total, toast))
            .collect()
    }

    fn a11y_focus_key(&self) -> Option<String> {
        let focus = self.focus.as_ref()?;
        Some(format!("toast.{}", focus.toast_id.as_str()))
    }
}

fn toast_child(idx: usize, total: usize, toast: &Toast) -> A11yChild {
    let mut info = in_set(
        A11yInfo::new()
            .with_role(A11yRole::Alert)
            .with_label(&toast.title)
            .with_live(A11yLive::Polite),
        idx,
        total,
    );
    let body = non_empty(toast.body.clone());
    let severity = severity_name(toast.severity);
    info = info.with_value(match &body {
        Some(body) => format!("{severity}: {body}"),
        None => severity.to_string(),
    });
    let mut children: Vec<A11yChild> = toast
        .actions
        .iter()
        .map(|action| {
            A11yChild::new(
                format!("action.{}", action.id.as_str()),
                A11yInfo::new()
                    .with_role(A11yRole::Button)
                    .with_label(&action.label),
            )
        })
        .collect();
    children.push(A11yChild::new(
        "dismiss",
        A11yInfo::new()
            .with_role(A11yRole::Button)
            .with_label("Dismiss"),
    ));
    A11yChild::new(format!("toast.{}", toast.id.as_str()), info).with_children(children)
}

fn severity_name(severity: ToastSeverity) -> &'static str {
    match severity {
        ToastSeverity::Info => "info",
        ToastSeverity::Success => "success",
        ToastSeverity::Warning => "warning",
        ToastSeverity::Error => "error",
    }
}

// ----------------------------------------------------------------- tooltip

impl A11yPrimitive for Tooltip {
    fn a11y_info(&self) -> A11yInfo {
        // Tip text is written for a bordered box, so it may carry padding
        // spaces at either end; an announced name must not.
        let label = match &self.styled_lines {
            Some(lines) if !lines.is_empty() => lines
                .iter()
                .map(plain_text)
                .collect::<Vec<_>>()
                .join(" ")
                .trim()
                .to_string(),
            _ => self.text.trim().to_string(),
        };
        A11yInfo::new()
            .with_role(A11yRole::Tooltip)
            .with_label(label)
    }
}

// --------------------------------------------------------- rich text popup

impl A11yPrimitive for RichTextPopup {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new()
            .with_role(A11yRole::Document)
            .with_label("Rich text popup")
            .with_set_size(self.lines.len() as u32)
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        let total = self.lines.len();
        let mut children: Vec<A11yChild> = self
            .lines
            .iter()
            .enumerate()
            .map(|(idx, line)| {
                A11yChild::new(
                    format!("line.{idx}"),
                    in_set(
                        A11yInfo::new()
                            .with_role(A11yRole::Label)
                            .with_label(plain_text(line)),
                        idx,
                        total,
                    ),
                )
            })
            .collect();
        for (idx, link) in self.links.iter().enumerate() {
            let text = self
                .line_text
                .get(link.line)
                .and_then(|line| line.get(link.start_byte..link.end_byte))
                .unwrap_or(&link.url);
            children.push(A11yChild::new(
                format!("link.{idx}"),
                A11yInfo::new()
                    .with_role(A11yRole::Button)
                    .with_label(text)
                    .with_value(&link.url)
                    .with_selected(self.focused_link == Some(idx)),
            ));
        }
        children
    }

    fn a11y_focus_key(&self) -> Option<String> {
        self.focused_link.map(|idx| format!("link.{idx}"))
    }
}

// ---------------------------------------------------------------- progress

impl A11yPrimitive for ProgressBar {
    fn a11y_info(&self) -> A11yInfo {
        let info = A11yInfo::new()
            .with_role(A11yRole::Progress)
            .with_label(&self.label);
        // No range means indeterminate, which is exactly what an absent
        // `value` encodes on the descriptor.
        match self.value {
            Some(value) => info.with_value_range(A11yValueRange::new(value as f64, 0.0, 1.0)),
            None => info,
        }
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        let mut children = vec![presentational("track", "Progress track")];
        if self.cancellable {
            children.push(A11yChild::new(
                "cancel",
                A11yInfo::new()
                    .with_role(A11yRole::Button)
                    .with_label("Cancel"),
            ));
        }
        children
    }
}

// ----------------------------------------------------------------- spinner

impl A11yPrimitive for Spinner {
    /// A spinner is an indeterminate progress indicator, so it takes the
    /// progress role and omits a range; the animated glyph itself is
    /// presentational.
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new()
            .with_role(A11yRole::Progress)
            .with_label(&self.label)
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        vec![presentational("glyph", "Spinner")]
    }
}

// --------------------------------------------------------------- scrollbar

impl A11yPrimitive for Scrollbar {
    fn a11y_info(&self) -> A11yInfo {
        let (label, track_len) = match self.axis {
            ScrollAxis::Vertical => ("Vertical scrollbar", self.track.height),
            ScrollAxis::Horizontal => ("Horizontal scrollbar", self.track.width),
        };
        // The descriptor carries thumb geometry in pixels, not the
        // content extents behind it, so the reported value is how far
        // along the scrollable range the thumb sits: `0.0` at the top
        // or left edge, `1.0` at the end.
        let travel = (track_len - self.thumb_len).max(0.0);
        let fraction = if travel > 0.0 {
            (self.thumb_start / travel).clamp(0.0, 1.0)
        } else {
            0.0
        };
        A11yInfo::new()
            .with_role(A11yRole::ScrollBar)
            .with_label(label)
            .with_value_range(A11yValueRange::new(fraction as f64, 0.0, 1.0))
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        vec![presentational("thumb", "Scrollbar thumb")]
    }
}

// ------------------------------------------------------------------- split

impl A11yPrimitive for Split {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new()
            .with_role(A11yRole::Group)
            .with_label("Split")
            .with_value(direction_name(self.direction))
            .with_value_range(A11yValueRange::new(self.ratio as f64, 0.0, 1.0))
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        vec![presentational("divider", "Split divider")]
    }
}

impl A11yPrimitive for SplitTree {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new()
            .with_role(A11yRole::Group)
            .with_label("Split tree")
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        split_tree_children(self)
    }
}

fn split_tree_children(tree: &SplitTree) -> Vec<A11yChild> {
    vec![split_tree_node(tree, "split")]
}

/// Builds the single [`A11yChild`] for one node of a [`SplitTree`], keyed
/// by `key`. A split's two halves are recursed into with positional keys
/// (`"split.first"` / `"split.second"`) rather than the constant `"split"`
/// used here, so a nested split-of-splits — the ordinary four-pane grid —
/// never gives two sibling nodes the same key (and therefore the same
/// [`crate::a11y::tree::A11yNodeId`]).
fn split_tree_node(tree: &SplitTree, key: impl Into<String>) -> A11yChild {
    match tree {
        SplitTree::Leaf(id) => A11yChild::new(
            format!("leaf.{}", id.as_str()),
            A11yInfo::new()
                .with_role(A11yRole::Group)
                .with_label(id.as_str()),
        ),
        SplitTree::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            let mut children = vec![split_tree_node(first, "split.first")];
            children.push(presentational("divider", "Split divider"));
            children.push(split_tree_node(second, "split.second"));
            A11yChild::new(
                key,
                A11yInfo::new()
                    .with_role(A11yRole::Group)
                    .with_label("Split")
                    .with_value(direction_name(*direction))
                    .with_value_range(A11yValueRange::new(*ratio as f64, 0.0, 1.0)),
            )
            .with_children(children)
        }
    }
}

// ------------------------------------------------------------------- panel

impl A11yPrimitive for Panel {
    fn a11y_info(&self) -> A11yInfo {
        let label = self
            .title
            .as_ref()
            .and_then(|title| non_empty(plain_text(title)))
            .unwrap_or_else(|| "Panel".to_string());
        A11yInfo::new()
            .with_role(A11yRole::Group)
            .with_label(label)
            .with_expanded(!self.collapsed)
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        let total = self.actions.len();
        self.actions
            .iter()
            .enumerate()
            .map(|(idx, action)| {
                let mut info = in_set(
                    A11yInfo::new()
                        .with_role(A11yRole::Button)
                        .with_label(&action.tooltip),
                    idx,
                    total,
                );
                if action.is_active {
                    info = info.with_checked(A11yChecked::True);
                }
                A11yChild::new(format!("action.{}", action.id.as_str()), info)
            })
            .collect()
    }
}

impl A11yPrimitive for SidebarPanel {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new()
            .with_role(A11yRole::Group)
            .with_label("Sidebar panel")
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        match &self.toolbar {
            Some(bar) => {
                vec![A11yChild::new("toolbar", bar.a11y_info()).with_children(bar.a11y_children())]
            }
            None => Vec::new(),
        }
    }
}
