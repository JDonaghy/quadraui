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
//!
//! # Backend contract
//!
//! **Mostly declarative.** Render fields top-to-bottom from
//! `fields[scroll_offset..]`. Per-field rendering depends on `FieldKind`:
//!
//! - `Toggle` — checkbox / switch UI; click flips value, emit
//!   `FormEvent::ToggleChanged`.
//! - `TextInput` — render text + cursor + selection; route printable
//!   keys to text mutation, emit `FormEvent::TextInputChanged` per
//!   keystroke and `TextInputCommitted` on Enter.
//! - `Button` — render label, click emits `FormEvent::ButtonClicked`.
//! - `Label` — non-interactive header / divider.
//!
//! Tab / Shift-Tab move `focused_field` forward/backward through
//! interactive fields (skip `Label`); emit `FormEvent::FocusChanged
//! { id }`. The *app* updates `focused_field` on the next frame.
//!
//! No measurement-dependent state — fields are uniform-height per
//! backend.

use crate::event::Rect;
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
    /// and `FormEvent::TextInputCommitted` on Enter.
    ///
    /// `cursor` is a byte offset into `value`. When `Some(n)`, backends
    /// render a cursor at that position; when `None`, the field is
    /// displayed read-only (no cursor). The app is responsible for
    /// updating `cursor` as the user types / moves — the primitive does
    /// not do its own input handling.
    ///
    /// `selection_anchor` is a byte offset into `value`. When `Some(n)`
    /// and `n != cursor`, backends render the range between `anchor` and
    /// `cursor` with a selection highlight. `None` means no selection.
    ///
    /// Scroll offset for long text is a later primitive extension.
    TextInput {
        value: String,
        #[serde(default)]
        placeholder: String,
        #[serde(default)]
        cursor: Option<usize>,
        #[serde(default)]
        selection_anchor: Option<usize>,
    },
    /// A clickable button. `label` on the containing field is also used
    /// as the button caption. Emits `FormEvent::ButtonClicked`.
    Button,
    /// Read-only display of a value computed elsewhere. Text-only; no
    /// events. Used for "current version: 0.10.0" style rows.
    ReadOnly { value: StyledText },
}

// ── D6 Layout API ───────────────────────────────────────────────────────────
//
// Per Decision D6: primitives return fully-resolved `Layout` structs.
// Seventh primitive on the new shape. Form is structurally a vertical
// stack of fields — same geometry as TreeView. The FieldKind distinction
// (Toggle / TextInput / Button / Label / ReadOnly) is about rendering,
// not layout, so it stays backend-owned: the layout just places rows.

/// Per-field measurement supplied by the backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FormFieldMeasure {
    pub height: f32,
}

impl FormFieldMeasure {
    pub fn new(height: f32) -> Self {
        Self { height }
    }
}

/// Resolved position of one visible form field after layout.
#[derive(Debug, Clone, PartialEq)]
pub struct VisibleFormField {
    /// Index into `Form.fields`.
    pub field_idx: usize,
    /// Clone of the field's `WidgetId` for click routing without a
    /// re-index into the primitive.
    pub id: WidgetId,
    pub bounds: Rect,
}

/// Classification of a hit-test result. Carries the field's `WidgetId`
/// so apps can dispatch via the same ID plumbing their event handlers
/// use — no secondary indirection through `fields[i].id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormHit {
    /// Click landed on a field row.
    Field(WidgetId),
    /// Click landed outside any field.
    Empty,
}

/// Fully-resolved form layout.
#[derive(Debug, Clone, PartialEq)]
pub struct FormLayout {
    pub viewport_width: f32,
    pub viewport_height: f32,
    pub visible_fields: Vec<VisibleFormField>,
    pub hit_regions: Vec<(Rect, FormHit)>,
    /// Scroll offset actually used, clamped to `[0, fields.len())`.
    pub resolved_scroll_offset: usize,
}

impl FormLayout {
    pub fn hit_test(&self, x: f32, y: f32) -> FormHit {
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        FormHit::Empty
    }
}

impl Form {
    /// Compute the full rendering + hit-test layout for this form.
    ///
    /// # Arguments
    ///
    /// - `viewport_width`, `viewport_height` — form area dimensions.
    /// - `measure_field(i)` — height for field `i`. Backends vary
    ///   height by `FieldKind` (e.g. `Label` may be shorter than
    ///   `TextInput`; fields with non-empty `hint` may be taller).
    ///
    /// # Row clipping
    ///
    /// The last visible field's `bounds.height` is clipped to what fits
    /// in the viewport (same semantics as `TreeView::layout`).
    pub fn layout<F>(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        measure_field: F,
    ) -> FormLayout
    where
        F: Fn(usize) -> FormFieldMeasure,
    {
        let mut visible_fields: Vec<VisibleFormField> = Vec::new();
        let mut hit_regions: Vec<(Rect, FormHit)> = Vec::new();

        let resolved_scroll_offset = if self.fields.is_empty() {
            0
        } else {
            self.scroll_offset.min(self.fields.len() - 1)
        };

        let mut y = 0.0_f32;
        for i in resolved_scroll_offset..self.fields.len() {
            if y >= viewport_height {
                break;
            }
            let m = measure_field(i);
            let remaining = viewport_height - y;
            let height = m.height.min(remaining).max(0.0);
            if height <= 0.0 {
                break;
            }
            let bounds = Rect::new(0.0, y, viewport_width, height);
            let id = self.fields[i].id.clone();
            visible_fields.push(VisibleFormField {
                field_idx: i,
                id: id.clone(),
                bounds,
            });
            hit_regions.push((bounds, FormHit::Field(id)));
            y += m.height;
        }

        FormLayout {
            viewport_width,
            viewport_height,
            visible_fields,
            hit_regions,
            resolved_scroll_offset,
        }
    }
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
