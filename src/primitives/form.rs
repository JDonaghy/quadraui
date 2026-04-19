//! `Form` primitive: a vertical stack of labeled field rows for settings
//! pages, dialogs, connection-config screens, and any other
//! "fill-in-the-fields" UI.
//!
//! A `Form` describes a sequence of `FormField`s. Each field has a
//! stable `WidgetId` (so events carry the field identity back to the
//! app), an optional `label` (rendered to the left of the input), and
//! a `FieldKind` that determines how it's drawn and what events it emits.
//!
//! Field state (toggle values, text content, focus) is owned by the
//! description — the app rebuilds the `Form` each frame from its own
//! canonical state. The primitive does not retain field values between
//! frames. (Scroll offset, if added later, follows the same
//! primitive-owned-via-WidgetId pattern as `TreeView`.)
//!
//! Keyboard navigation between fields is backend-driven: Tab / Shift-Tab
//! moves focus forward/backward; arrow keys within a field are handled
//! by that field's kind-specific logic.

use crate::types::{Modifiers, StyledText, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a `Form` widget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Form {
    pub id: WidgetId,
    pub fields: Vec<FormField>,
    /// `WidgetId` of the field that currently has keyboard focus, or
    /// `None` if the form as a whole has focus but no field is active.
    pub focused_field: Option<WidgetId>,
    /// How many rows have been scrolled past. App-owned for now; a
    /// later primitive stage may lift this into `ScrollState` keyed by
    /// `WidgetId` the same way `TreeView`'s scroll will.
    #[serde(default)]
    pub scroll_offset: usize,
    #[serde(default)]
    pub has_focus: bool,
}

/// One row in a `Form`: a label + an input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormField {
    pub id: WidgetId,
    /// Rendered to the left of the field. Omit (empty spans) for rows
    /// that are just buttons or labels standing alone.
    pub label: StyledText,
    pub kind: FieldKind,
    /// Tooltip / hint text rendered below the field (or to the right,
    /// depending on backend). Empty = no hint.
    #[serde(default)]
    pub hint: StyledText,
    /// When true, the field is rendered dimmed and will not emit events.
    #[serde(default)]
    pub disabled: bool,
}

/// The input variant carried by a `FormField`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldKind {
    /// A bold/sectioned header row. Not interactive; used to group
    /// related fields. The field's `label` carries the header text;
    /// `FormEvent`s are never emitted from this kind.
    Label,
    /// Boolean toggle. `value` is the current state. Space / Enter
    /// toggles; emits `FormEvent::ToggleChanged`.
    Toggle { value: bool },
    /// Single-line text input. `value` is the current text. Typing
    /// modifies it; emits `FormEvent::TextInputChanged` on every keystroke
    /// and `FormEvent::TextInputCommitted` on Enter. Cursor position is
    /// a primitive-owned detail; `value` is the authoritative content.
    ///
    /// A later primitive extension will carry cursor position,
    /// selection anchor, and scroll offset for long text.
    TextInput {
        value: String,
        #[serde(default)]
        placeholder: String,
    },
    /// A clickable button. `label` on the containing field is also used
    /// as the button caption. Emits `FormEvent::ButtonClicked`.
    Button,
    /// Read-only display of a value computed elsewhere. Text-only; no
    /// events. Used for "current version: 0.10.0" style rows.
    ReadOnly { value: StyledText },
}

/// Events a `Form` emits back to the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FormEvent {
    /// A `Toggle` field's value changed.
    ToggleChanged { id: WidgetId, value: bool },
    /// A `TextInput` field's text changed. Fires on every keystroke that
    /// modifies the value.
    TextInputChanged { id: WidgetId, value: String },
    /// A `TextInput` received Enter while focused.
    TextInputCommitted { id: WidgetId, value: String },
    /// Keyboard focus moved to a different field.
    FocusChanged { id: WidgetId },
    /// A `Button` was clicked or activated with Enter / Space.
    ButtonClicked { id: WidgetId },
    /// A key was pressed while the form had focus and the primitive did
    /// not consume it. The app may interpret it (e.g. `?` opens help).
    KeyPressed { key: String, modifiers: Modifiers },
}
