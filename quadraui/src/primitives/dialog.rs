//! `Dialog` primitive: a modal message box with a title, body, and
//! action buttons. Used for confirmations ("Close unsaved file?"),
//! error reports, and anything else that needs the user to
//! acknowledge / choose before continuing.
//!
//! A `Dialog` is structurally a `Modal` with a fixed layout: title
//! row + body text + bottom-right-aligned button row. Backends render
//! it as a centered overlay box.
//!
//! # Backend contract
//!
//! **Modal overlay — intercept all clicks.** Clicks outside the dialog
//! either dismiss (emit `Cancelled`) or are swallowed — app policy.
//! Click on a button emits `ButtonClicked { id }`. Enter activates the
//! default button (the first whose `is_default = true`); Escape emits
//! `Cancelled` unconditionally.

use crate::event::Rect;
use crate::primitives::toolbar::{Toolbar, ToolbarHit, ToolbarItemMeasure, ToolbarLayout};
use crate::types::{Color, Modifiers, StyledText, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dialog {
    pub id: WidgetId,
    pub title: StyledText,
    /// Body content lines. Each entry is one line rendered top-to-bottom.
    /// Supports per-line styled spans for keybinding references, help
    /// text, and other multi-line content.
    pub body: Vec<StyledText>,
    pub buttons: Vec<DialogButton>,
    /// Optional severity tint — backends may add an icon or edge
    /// accent. `None` = neutral.
    #[serde(default)]
    pub severity: Option<DialogSeverity>,
    /// When true, buttons are stacked vertically (useful for narrow
    /// dialogs or many-choice dialogs like code-action pickers). When
    /// false, buttons are horizontal, right-aligned.
    #[serde(default)]
    pub vertical_buttons: bool,
    /// Optional content rendered between the body text and the button row.
    ///
    /// - [`DialogInput::TextInput`] — single-line text field; used for
    ///   rename prompts, input-required confirms.
    /// - [`DialogInput::Toolbar`] — horizontal action strip; used when
    ///   the dialog wants an inline action bar (e.g. "Preview / Skip /
    ///   Apply") in addition to the modal OK/Cancel buttons.
    ///
    /// Apps own the value; events come back through [`DialogEvent`].
    #[serde(default)]
    pub input: Option<DialogInput>,
}

/// Content of the slot rendered between the body text and the button row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DialogInput {
    /// Single-line text field (rename prompts, input-required confirms).
    TextInput(DialogTextInput),
    /// Inline horizontal action strip. Backends render this by calling their
    /// `draw_toolbar` equivalent inside the body slot. Click events are
    /// returned as [`DialogEvent::BodyToolbarClicked`].
    Toolbar(Toolbar),
}

/// Single-line text input embedded in a dialog body slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DialogTextInput {
    /// Current input value.
    pub value: String,
    /// Placeholder text shown when `value` is empty.
    #[serde(default)]
    pub placeholder: String,
    /// Cursor byte offset. `None` renders the input without a cursor.
    #[serde(default)]
    pub cursor: Option<usize>,
}

/// Severity of a `Dialog`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DialogSeverity {
    Info,
    Question,
    Warning,
    Error,
}

/// One button on a dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DialogButton {
    pub id: WidgetId,
    pub label: String,
    /// When true, Enter activates this button (and backends typically
    /// style it as the primary). Only one button should be default;
    /// if multiple, the first wins.
    #[serde(default)]
    pub is_default: bool,
    /// When true, Escape activates this button (cancel-button
    /// convention). Only one button should have this.
    #[serde(default)]
    pub is_cancel: bool,
    /// Override colour for destructive actions ("Delete", "Discard").
    /// `None` = theme default.
    #[serde(default)]
    pub tint: Option<Color>,
}

/// Events a `Dialog` emits back to the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DialogEvent {
    /// User clicked a button (or activated via Enter / Escape mapping).
    ButtonClicked { id: WidgetId },
    /// The input field's value changed. Fires per keystroke.
    InputChanged { value: String },
    /// User pressed Enter inside the input field — apps typically
    /// treat this like clicking the default button.
    InputCommitted { value: String },
    /// User clicked an enabled action button inside a
    /// [`DialogInput::Toolbar`] body slot.
    BodyToolbarClicked { id: WidgetId },
    /// Dialog dismissed without a specific button (click-outside
    /// where the app allows it). Prefer `ButtonClicked` with the
    /// cancel button when possible.
    Cancelled,
    /// Key pressed while the dialog had focus and the primitive didn't
    /// consume it.
    KeyPressed { key: String, modifiers: Modifiers },
}

// ── D6 Layout API ───────────────────────────────────────────────────────────

/// Measurements for dialog sub-regions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DialogMeasure {
    /// Full dialog box width.
    pub width: f32,
    /// Height reserved for the title row (may be 0 if title is empty).
    pub title_height: f32,
    /// Height of the body content.
    pub body_height: f32,
    /// Height reserved for the input row (0 when dialog has no input).
    pub input_height: f32,
    /// Height reserved for the button row.
    pub button_row_height: f32,
    /// Width of each button (uniform, for simplicity).
    pub button_width: f32,
    /// Horizontal gap between buttons.
    pub button_gap: f32,
    /// Padding inside the dialog (between content and box edges).
    pub padding: f32,
}

impl DialogMeasure {
    pub fn total_height(&self) -> f32 {
        self.title_height
            + self.body_height
            + self.input_height
            + self.button_row_height
            + self.padding * 2.0
    }
}

/// Resolved position of one button.
#[derive(Debug, Clone, PartialEq)]
pub struct VisibleDialogButton {
    pub button_idx: usize,
    pub id: WidgetId,
    pub bounds: Rect,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogHit {
    /// Click landed on a button.
    Button(WidgetId),
    /// Click landed on an enabled action button inside the body toolbar
    /// ([`DialogInput::Toolbar`]).
    BodyToolbarButton(WidgetId),
    /// Click landed on the dialog box (not a button) — apps typically
    /// swallow this so it doesn't dismiss.
    Body,
    /// Click landed outside the dialog box — apps may dismiss on this.
    Outside,
}

/// Fully-resolved dialog layout.
#[derive(Debug, Clone, PartialEq)]
pub struct DialogLayout {
    /// Full dialog box bounds.
    pub bounds: Rect,
    /// Title row bounds (if `measure.title_height > 0`).
    pub title_bounds: Option<Rect>,
    /// Body content bounds.
    pub body_bounds: Rect,
    /// Bounds of the input slot (text input or toolbar), when present
    /// and `measure.input_height > 0`. Rasterisers use this to position
    /// whichever kind of input the dialog carries.
    pub input_bounds: Option<Rect>,
    /// Pre-computed [`ToolbarLayout`] for the body-slot toolbar, when
    /// the dialog carries a [`DialogInput::Toolbar`]. `None` for all
    /// other input kinds. Rasterisers use this to paint the toolbar
    /// and route click events.
    pub body_toolbar_layout: Option<ToolbarLayout>,
    /// Button row bounds.
    pub button_row_bounds: Rect,
    pub visible_buttons: Vec<VisibleDialogButton>,
    pub hit_regions: Vec<(Rect, DialogHit)>,
}

impl DialogLayout {
    pub fn hit_test(&self, x: f32, y: f32) -> DialogHit {
        let inside = x >= self.bounds.x
            && x < self.bounds.x + self.bounds.width
            && y >= self.bounds.y
            && y < self.bounds.y + self.bounds.height;
        if !inside {
            return DialogHit::Outside;
        }
        // Check toolbar hit regions first (finer-grained than the body
        // slot bounding box in `hit_regions`).
        if let Some(ref tl) = self.body_toolbar_layout {
            match tl.hit_test(x, y) {
                ToolbarHit::Button(id) => return DialogHit::BodyToolbarButton(id),
                ToolbarHit::Empty => {}
            }
        }
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        DialogHit::Body
    }
}

impl Dialog {
    /// Compute dialog layout.
    ///
    /// # Arguments
    ///
    /// - `viewport` — parent surface bounds; the dialog is centered
    ///   within this.
    /// - `measure` — sub-region widths/heights. Backends measure the
    ///   body text (wrapping to `measure.width`) and set
    ///   `body_height` accordingly; ditto for title and buttons.
    ///   `input_height` is used for both the text-input and toolbar
    ///   variants — set it to the desired slot height regardless of
    ///   which [`DialogInput`] kind is present.
    /// - `measure_toolbar_item` — per-item width callback for the
    ///   toolbar variant. When `self.input` is not
    ///   [`DialogInput::Toolbar`], this callback is never called and
    ///   may be `|_| ToolbarItemMeasure::new(0.0)`.
    ///
    /// # Centering
    ///
    /// The dialog box is placed at the viewport's horizontal + vertical
    /// center. Button row is at the bottom of the box, right-aligned
    /// (horizontal) or stretched (vertical).
    pub fn layout<F>(
        &self,
        viewport: Rect,
        measure: DialogMeasure,
        measure_toolbar_item: F,
    ) -> DialogLayout
    where
        F: Fn(&crate::primitives::toolbar::ToolbarButton) -> ToolbarItemMeasure,
    {
        let total_h = measure.total_height();
        let box_x = viewport.x + (viewport.width - measure.width) * 0.5;
        let box_y = viewport.y + (viewport.height - total_h) * 0.5;
        let bounds = Rect::new(box_x, box_y, measure.width, total_h);

        let content_x = box_x + measure.padding;
        let content_w = (measure.width - measure.padding * 2.0).max(0.0);
        let mut cursor_y = box_y + measure.padding;

        let title_bounds = if measure.title_height > 0.0 {
            let b = Rect::new(content_x, cursor_y, content_w, measure.title_height);
            cursor_y += measure.title_height;
            Some(b)
        } else {
            None
        };

        let body_bounds = Rect::new(content_x, cursor_y, content_w, measure.body_height);
        cursor_y += measure.body_height;

        let (input_bounds, body_toolbar_layout) =
            if self.input.is_some() && measure.input_height > 0.0 {
                let b = Rect::new(content_x, cursor_y, content_w, measure.input_height);
                cursor_y += measure.input_height;

                let tl = match &self.input {
                    Some(DialogInput::Toolbar(toolbar)) => {
                        Some(toolbar.layout(b.x, b.y, b.width, b.height, &measure_toolbar_item))
                    }
                    _ => None,
                };
                (Some(b), tl)
            } else {
                (None, None)
            };

        let button_row_bounds =
            Rect::new(content_x, cursor_y, content_w, measure.button_row_height);

        let mut visible_buttons: Vec<VisibleDialogButton> = Vec::new();
        let mut hit_regions: Vec<(Rect, DialogHit)> = Vec::new();

        if self.vertical_buttons {
            // Stack vertically, each button full content width.
            let btn_h = if self.buttons.is_empty() {
                0.0
            } else {
                measure.button_row_height / self.buttons.len() as f32
            };
            for (i, btn) in self.buttons.iter().enumerate() {
                let y = cursor_y - measure.button_row_height + (i as f32) * btn_h;
                let b = Rect::new(content_x, y, content_w, btn_h);
                visible_buttons.push(VisibleDialogButton {
                    button_idx: i,
                    id: btn.id.clone(),
                    bounds: b,
                });
                hit_regions.push((b, DialogHit::Button(btn.id.clone())));
            }
        } else {
            // Right-aligned horizontal row.
            let total_btns_w = self.buttons.len() as f32 * measure.button_width
                + (self.buttons.len().saturating_sub(1)) as f32 * measure.button_gap;
            let start_x = content_x + content_w - total_btns_w;
            for (i, btn) in self.buttons.iter().enumerate() {
                let x = start_x + (i as f32) * (measure.button_width + measure.button_gap);
                let b = Rect::new(x, cursor_y, measure.button_width, measure.button_row_height);
                visible_buttons.push(VisibleDialogButton {
                    button_idx: i,
                    id: btn.id.clone(),
                    bounds: b,
                });
                hit_regions.push((b, DialogHit::Button(btn.id.clone())));
            }
        }

        DialogLayout {
            bounds,
            title_bounds,
            body_bounds,
            input_bounds,
            body_toolbar_layout,
            button_row_bounds,
            visible_buttons,
            hit_regions,
        }
    }

    /// Convenience: find the default button's id (first with
    /// `is_default = true`, or the last button as a fallback).
    pub fn default_button_id(&self) -> Option<&WidgetId> {
        self.buttons
            .iter()
            .find(|b| b.is_default)
            .map(|b| &b.id)
            .or_else(|| self.buttons.last().map(|b| &b.id))
    }

    /// Convenience: find the cancel button's id (first with
    /// `is_cancel = true`).
    pub fn cancel_button_id(&self) -> Option<&WidgetId> {
        self.buttons.iter().find(|b| b.is_cancel).map(|b| &b.id)
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::toolbar::{Toolbar, ToolbarButton};
    use crate::types::WidgetId;

    fn no_measure(_: &crate::primitives::toolbar::ToolbarButton) -> ToolbarItemMeasure {
        ToolbarItemMeasure::new(0.0)
    }

    fn viewport() -> Rect {
        Rect::new(0.0, 0.0, 400.0, 300.0)
    }

    fn measure_no_input() -> DialogMeasure {
        DialogMeasure {
            width: 200.0,
            title_height: 20.0,
            body_height: 20.0,
            input_height: 0.0,
            button_row_height: 20.0,
            button_width: 60.0,
            button_gap: 8.0,
            padding: 10.0,
        }
    }

    fn measure_with_input() -> DialogMeasure {
        DialogMeasure {
            input_height: 20.0,
            ..measure_no_input()
        }
    }

    fn btn(id: &str) -> DialogButton {
        DialogButton {
            id: WidgetId::new(id),
            label: id.to_string(),
            is_default: false,
            is_cancel: false,
            tint: None,
        }
    }

    fn base_dialog(input: Option<DialogInput>) -> Dialog {
        Dialog {
            id: WidgetId::new("d"),
            title: crate::types::StyledText::plain("Title"),
            body: vec![crate::types::StyledText::plain("Body")],
            buttons: vec![btn("ok")],
            severity: None,
            vertical_buttons: false,
            input,
        }
    }

    // ── DialogInput::TextInput serde round-trip ───────────────────────────

    #[test]
    fn serde_dialog_text_input_round_trip() {
        let input = DialogInput::TextInput(DialogTextInput {
            value: "hello".into(),
            placeholder: "type here".into(),
            cursor: Some(5),
        });
        let json = serde_json::to_string(&input).unwrap();
        let back: DialogInput = serde_json::from_str(&json).unwrap();
        assert_eq!(input, back);
    }

    // ── DialogInput::Toolbar serde round-trip ─────────────────────────────

    #[test]
    fn serde_dialog_toolbar_round_trip() {
        let input = DialogInput::Toolbar(Toolbar {
            id: WidgetId::new("body-tb"),
            buttons: vec![
                ToolbarButton::Action {
                    id: WidgetId::new("preview"),
                    label: "Preview".into(),
                    icon: None,
                    key_hint: None,
                    enabled: true,
                    is_active: false,
                    tooltip: String::new(),
                },
                ToolbarButton::Separator,
                ToolbarButton::Action {
                    id: WidgetId::new("apply"),
                    label: "Apply".into(),
                    icon: None,
                    key_hint: None,
                    enabled: true,
                    is_active: false,
                    tooltip: String::new(),
                },
            ],
            bg: None,
        });
        let json = serde_json::to_string(&input).unwrap();
        let back: DialogInput = serde_json::from_str(&json).unwrap();
        assert_eq!(input, back);
    }

    // ── DialogEvent::BodyToolbarClicked serde round-trip ──────────────────

    #[test]
    fn serde_dialog_event_body_toolbar_clicked() {
        let ev = DialogEvent::BodyToolbarClicked {
            id: WidgetId::new("preview"),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: DialogEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    // ── Toolbar variant creates body_toolbar_layout ───────────────────────

    #[test]
    fn layout_toolbar_variant_sets_body_toolbar_layout() {
        let d = base_dialog(Some(DialogInput::Toolbar(Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![ToolbarButton::Action {
                id: WidgetId::new("a"),
                label: "A".into(),
                icon: None,
                key_hint: None,
                enabled: true,
                is_active: false,
                tooltip: String::new(),
            }],
            bg: None,
        })));
        let layout = d.layout(viewport(), measure_with_input(), |_| {
            ToolbarItemMeasure::new(10.0)
        });
        assert!(
            layout.input_bounds.is_some(),
            "input_bounds should be Some for Toolbar variant"
        );
        assert!(
            layout.body_toolbar_layout.is_some(),
            "body_toolbar_layout should be Some for Toolbar variant"
        );
        assert!(
            layout
                .body_toolbar_layout
                .as_ref()
                .unwrap()
                .visible_items
                .len()
                == 1
        );
    }

    #[test]
    fn layout_text_input_variant_does_not_set_body_toolbar_layout() {
        let d = base_dialog(Some(DialogInput::TextInput(DialogTextInput {
            value: "x".into(),
            placeholder: String::new(),
            cursor: None,
        })));
        let layout = d.layout(viewport(), measure_with_input(), no_measure);
        assert!(layout.input_bounds.is_some());
        assert!(
            layout.body_toolbar_layout.is_none(),
            "body_toolbar_layout should be None for TextInput variant"
        );
    }

    // ── BodyToolbarButton hit routing ─────────────────────────────────────

    #[test]
    fn hit_test_body_toolbar_button_routes_correctly() {
        let d = base_dialog(Some(DialogInput::Toolbar(Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![ToolbarButton::Action {
                id: WidgetId::new("preview"),
                label: "Preview".into(),
                icon: None,
                key_hint: None,
                enabled: true,
                is_active: false,
                tooltip: String::new(),
            }],
            bg: None,
        })));
        let layout = d.layout(viewport(), measure_with_input(), |_| {
            ToolbarItemMeasure::new(60.0)
        });
        let tl = layout.body_toolbar_layout.as_ref().unwrap();
        let vis = &tl.visible_items[0];
        // Click inside the toolbar button bounds.
        let cx = vis.bounds.x + 1.0;
        let cy = vis.bounds.y;
        match layout.hit_test(cx, cy) {
            DialogHit::BodyToolbarButton(id) => {
                assert_eq!(id.as_str(), "preview");
            }
            other => panic!("expected BodyToolbarButton, got {:?}", other),
        }
    }

    #[test]
    fn hit_test_no_toolbar_no_body_toolbar_hit() {
        let d = base_dialog(None);
        let layout = d.layout(viewport(), measure_no_input(), no_measure);
        // Click inside dialog body — should be Body, not BodyToolbarButton.
        let cx = layout.body_bounds.x + 5.0;
        let cy = layout.body_bounds.y + 5.0;
        assert_eq!(layout.hit_test(cx, cy), DialogHit::Body);
    }
}
