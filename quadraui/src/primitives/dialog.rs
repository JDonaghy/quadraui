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

/// A 2-D data table embedded inside a [`Dialog`] body.
///
/// Rendered between the body text lines and the [`DialogInput`] slot.
/// Columns are separated by ` │ `. When `headers` is `Some`, a header row
/// is drawn above a `─┼─` separator row.
///
/// # Column widths
///
/// `column_widths` is an optional explicit hint (in *character cells* for TUI,
/// in *pixels* for pixel backends). When `None` every backend auto-sizes each
/// column to fit its widest cell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DialogTable {
    /// Optional column header labels.
    pub headers: Option<Vec<String>>,
    /// Data rows; each row is a `Vec` of cell strings, one per column.
    pub rows: Vec<Vec<String>>,
    /// Explicit column widths (TUI: char cells; pixel backends: pixels).
    /// `None` → auto-size.
    #[serde(default)]
    pub column_widths: Option<Vec<u16>>,
}

impl DialogTable {
    /// Number of columns derived from the widest row / header.
    pub fn num_cols(&self) -> usize {
        let from_rows = self.rows.iter().map(|r| r.len()).max().unwrap_or(0);
        let from_headers = self.headers.as_ref().map(|h| h.len()).unwrap_or(0);
        from_rows.max(from_headers)
    }

    /// Auto-compute column char-widths from content (TUI helper).
    ///
    /// Returns a `Vec` of length [`Self::num_cols`]; each entry is the
    /// maximum character count seen in that column across headers + rows.
    /// If `column_widths` is `Some`, those values are used as a *minimum*
    /// (content may still be wider).
    pub fn auto_col_widths(&self) -> Vec<usize> {
        let ncols = self.num_cols();
        let mut widths = vec![0usize; ncols];

        if let Some(explicit) = &self.column_widths {
            for (j, &w) in explicit.iter().enumerate() {
                if j < ncols {
                    widths[j] = w as usize;
                }
            }
        }

        if let Some(headers) = &self.headers {
            for (j, h) in headers.iter().enumerate() {
                if j < ncols {
                    widths[j] = widths[j].max(h.chars().count());
                }
            }
        }
        for row in &self.rows {
            for (j, cell) in row.iter().enumerate() {
                if j < ncols {
                    widths[j] = widths[j].max(cell.chars().count());
                }
            }
        }
        widths
    }

    /// Total rendered width in character cells (TUI).
    ///
    /// Columns are separated by ` │ ` (3 chars). Returns 0 when there are no
    /// columns.
    pub fn tui_total_width(&self) -> usize {
        let col_widths = self.auto_col_widths();
        if col_widths.is_empty() {
            return 0;
        }
        let sum: usize = col_widths.iter().sum();
        sum + 3 * col_widths.len().saturating_sub(1)
    }

    /// Total height in TUI rows: data rows + header row + separator row (when
    /// headers present).
    pub fn tui_total_height(&self) -> usize {
        let header_rows = if self.headers.is_some() { 2 } else { 0 };
        header_rows + self.rows.len()
    }
}

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
    /// Optional structured table rendered between the body text and the
    /// [`DialogInput`] slot. Used for keybindings grids, comparison
    /// tables, etc.
    ///
    /// Layout order: title → body → **table** → input → buttons.
    #[serde(default)]
    pub table: Option<DialogTable>,
    /// Optional content rendered between the table and the button row.
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
    /// Height reserved for the table slot (0 when `dialog.table` is `None`).
    /// Set by the backend to `row_count * row_height` (including header +
    /// separator rows when headers are present).
    pub table_height: f32,
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
            + self.table_height
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
    /// Bounds of the table slot when `dialog.table` is `Some` and
    /// `measure.table_height > 0`. Rasterisers render the table inside
    /// this rectangle.
    pub table_bounds: Option<Rect>,
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
        // A vertical button stack reserves one row per button; total_height()
        // budgets only a single button row, so swap in the full block height.
        let button_block_h = if self.vertical_buttons {
            measure.button_row_height * self.buttons.len().max(1) as f32
        } else {
            measure.button_row_height
        };
        let total_h = measure.total_height() - measure.button_row_height + button_block_h;
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

        let table_bounds = if self.table.is_some() && measure.table_height > 0.0 {
            let b = Rect::new(content_x, cursor_y, content_w, measure.table_height);
            cursor_y += measure.table_height;
            Some(b)
        } else {
            None
        };

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

        let button_row_bounds = Rect::new(content_x, cursor_y, content_w, button_block_h);

        let mut visible_buttons: Vec<VisibleDialogButton> = Vec::new();
        let mut hit_regions: Vec<(Rect, DialogHit)> = Vec::new();

        if self.vertical_buttons {
            // Stack vertically, one full row per button, full content width,
            // starting at the button-row origin and growing downward.
            let btn_h = measure.button_row_height;
            for (i, btn) in self.buttons.iter().enumerate() {
                let y = cursor_y + (i as f32) * btn_h;
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
            table_bounds,
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
            table_height: 0.0,
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
            table: None,
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
            focused_index: None,
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
            focused_index: None,
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
            focused_index: None,
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

    // ── DialogTable ──────────────────────────────────────────────────────────

    fn table_dialog() -> Dialog {
        Dialog {
            id: WidgetId::new("d"),
            title: crate::types::StyledText::plain("Keybindings"),
            body: vec![],
            buttons: vec![btn("close")],
            severity: None,
            vertical_buttons: false,
            table: Some(DialogTable {
                headers: Some(vec!["Key".into(), "Action".into()]),
                rows: vec![
                    vec!["Ctrl+S".into(), "Save".into()],
                    vec!["Ctrl+Z".into(), "Undo".into()],
                ],
                column_widths: None,
            }),
            input: None,
        }
    }

    #[test]
    fn dialog_table_num_cols() {
        let t = DialogTable {
            headers: Some(vec!["A".into(), "B".into(), "C".into()]),
            rows: vec![vec!["x".into(), "y".into()]],
            column_widths: None,
        };
        assert_eq!(t.num_cols(), 3, "headers win when wider than rows");
    }

    #[test]
    fn dialog_table_auto_col_widths() {
        let t = DialogTable {
            headers: Some(vec!["Key".into(), "Action".into()]),
            rows: vec![
                vec!["Ctrl+S".into(), "Save".into()],
                vec!["Ctrl+Shift+Z".into(), "Redo".into()],
            ],
            column_widths: None,
        };
        let widths = t.auto_col_widths();
        assert_eq!(widths.len(), 2);
        assert_eq!(widths[0], "Ctrl+Shift+Z".chars().count()); // 12
        assert_eq!(widths[1], "Action".chars().count()); // 6
    }

    #[test]
    fn dialog_table_tui_total_width() {
        let t = DialogTable {
            headers: Some(vec!["Key".into(), "Action".into()]),
            rows: vec![vec!["Ctrl+S".into(), "Save".into()]],
            column_widths: None,
        };
        // col0=6 ("Ctrl+S"), col1=6 ("Action"), sep=" │ "=3 → total=6+3+6=15
        assert_eq!(t.tui_total_width(), 15);
    }

    #[test]
    fn dialog_table_tui_total_height_with_headers() {
        let t = DialogTable {
            headers: Some(vec!["Key".into()]),
            rows: vec![vec!["a".into()], vec!["b".into()]],
            column_widths: None,
        };
        // 2 header rows (header + separator) + 2 data rows = 4
        assert_eq!(t.tui_total_height(), 4);
    }

    #[test]
    fn dialog_table_tui_total_height_no_headers() {
        let t = DialogTable {
            headers: None,
            rows: vec![vec!["a".into()], vec!["b".into()], vec!["c".into()]],
            column_widths: None,
        };
        assert_eq!(t.tui_total_height(), 3);
    }

    #[test]
    fn layout_with_table_creates_table_bounds() {
        let d = table_dialog();
        // table: 2 header rows + 2 data rows = 4 rows → table_height=4*20=80
        let measure = DialogMeasure {
            width: 200.0,
            title_height: 20.0,
            body_height: 0.0,
            table_height: 80.0,
            input_height: 0.0,
            button_row_height: 20.0,
            button_width: 60.0,
            button_gap: 8.0,
            padding: 10.0,
        };
        let layout = d.layout(viewport(), measure, no_measure);
        assert!(
            layout.table_bounds.is_some(),
            "table_bounds should be Some when table is set"
        );
        let tb = layout.table_bounds.unwrap();
        assert_eq!(tb.height, 80.0);
    }

    #[test]
    fn layout_without_table_has_no_table_bounds() {
        let d = base_dialog(None);
        let layout = d.layout(viewport(), measure_no_input(), no_measure);
        assert!(
            layout.table_bounds.is_none(),
            "table_bounds should be None when table is None"
        );
    }

    #[test]
    fn layout_table_placed_below_body() {
        let d = table_dialog();
        let measure = DialogMeasure {
            width: 200.0,
            title_height: 10.0,
            body_height: 0.0,
            table_height: 40.0,
            input_height: 0.0,
            button_row_height: 10.0,
            button_width: 60.0,
            button_gap: 8.0,
            padding: 5.0,
        };
        let layout = d.layout(viewport(), measure, no_measure);
        let body_bottom = layout.body_bounds.y + layout.body_bounds.height;
        let table_top = layout.table_bounds.unwrap().y;
        assert_eq!(
            table_top, body_bottom,
            "table starts immediately after body"
        );
    }

    #[test]
    fn serde_dialog_table_round_trip() {
        let t = DialogTable {
            headers: Some(vec!["Key".into(), "Action".into()]),
            rows: vec![vec!["Ctrl+S".into(), "Save".into()]],
            column_widths: Some(vec![10, 20]),
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: DialogTable = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn serde_dialog_with_table_round_trip() {
        let d = table_dialog();
        let json = serde_json::to_string(&d).unwrap();
        let back: Dialog = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn serde_dialog_without_table_defaults_to_none() {
        // A JSON blob that doesn't contain the "table" key should deserialise
        // with table = None (backward-compat — old serialised dialogs keep working).
        let json = r#"{"id":"d","title":{"spans":[{"text":"T","fg":null,"bg":null,"bold":false,"italic":false,"underline":false,"strike":false}]},"body":[],"buttons":[]}"#;
        let d: Dialog = serde_json::from_str(json).unwrap();
        assert!(d.table.is_none());
    }
}
