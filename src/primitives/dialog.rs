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
use crate::types::{Color, Modifiers, StyledText, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dialog {
    pub id: WidgetId,
    pub title: StyledText,
    pub body: StyledText,
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
        self.title_height + self.body_height + self.button_row_height + self.padding * 2.0
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
    ///
    /// # Centering
    ///
    /// The dialog box is placed at the viewport's horizontal + vertical
    /// center. Button row is at the bottom of the box, right-aligned
    /// (horizontal) or stretched (vertical).
    pub fn layout(&self, viewport: Rect, measure: DialogMeasure) -> DialogLayout {
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
