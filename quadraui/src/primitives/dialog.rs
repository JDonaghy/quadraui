//! `Dialog` primitive: a modal message box with a title, body, and
//! action buttons. Used for confirmations, error reports, and anything
//! else that needs the user to acknowledge or choose before continuing.
//!
//! A `Dialog` is a modal overlay with a fixed layout: title row + body
//! text + bottom-right-aligned button row. Backends render it as a
//! centered overlay box; modal-vs-base hit precedence is arbitrated by
//! [`crate::ModalStack`], not by this primitive.
//!
//! # Backend contract
//!
//! **Modal overlay — intercept all clicks.** Clicks outside the dialog
//! either dismiss (emit `Cancelled`) or are swallowed — app policy.
//! Click on a button emits `ButtonClicked { id }`. Enter activates the
//! default button (the first whose `is_default = true`); Escape emits
//! `Cancelled` unconditionally.
//!
//! # Two-way reference with `backend.rs`
//!
//! [`native_dialog_options`] imports [`MessageDialogButton`] and
//! [`MessageDialogOptions`] from `crate::backend` — the first import
//! from `crate::backend` in `src/primitives/`. `backend.rs` imports
//! [`Dialog`] and [`DialogSeverity`] from here, creating a two-way
//! reference between the files. This inverts the one-directional
//! primitives→backend layering but stays within `ARCHITECTURE.md`'s
//! rules: [`MessageDialogOptions`] is a plain data type, not a backend
//! crate type, and serves as the mapping from `Dialog` descriptor to
//! native options — a pure function, unit-testable with no live surface.

use crate::backend::{MessageDialogButton, MessageDialogOptions};
use crate::event::{Key, Rect};
use crate::primitives::form::Form;
use crate::primitives::layout_metrics::TextMeasure;
use crate::primitives::toolbar::{Toolbar, ToolbarHit, ToolbarItemMeasure, ToolbarLayout};
use crate::style::Style;
use crate::types::{Color, StyledText, WidgetId};
use crate::Handled;
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
///
/// # Examples
///
/// ```
/// use quadraui::{
///     Dialog, DialogButton, DialogHit, DialogMeasure, Rect, StyledText, ToolbarItemMeasure,
///     WidgetId,
/// };
///
/// let dialog = Dialog {
///     id: WidgetId::new("dialog:confirm_close"),
///     title: StyledText::plain("Unsaved changes"),
///     body: vec![StyledText::plain("Close without saving?")],
///     buttons: vec![DialogButton {
///         id: WidgetId::new("dialog:ok"),
///         label: "OK".to_string(),
///         is_default: true,
///         is_cancel: false,
///         tint: None,
///         spec: None,
///     }],
///     severity: None,
///     vertical_buttons: false,
///     table: None,
///     input: None,
/// };
///
/// let measure = DialogMeasure {
///     width: 40.0,
///     title_height: 1.0,
///     body_height: 1.0,
///     table_height: 0.0,
///     input_height: 0.0,
///     button_row_height: 1.0,
///     button_width: 10.0,
///     button_gap: 1.0,
///     padding: 1.0,
/// };
///
/// let viewport = Rect::new(0.0, 0.0, 80.0, 24.0);
/// // `input` is `None` above, so `layout` never calls this closure — it's
/// // only reached by the `DialogInput::Toolbar` body variant.
/// let layout = dialog.layout(viewport, measure, |_| ToolbarItemMeasure::new(10.0));
/// let ok = layout.visible_buttons[0].bounds;
/// assert_eq!(layout.hit_test(ok.x, ok.y), DialogHit::Button(dialog.buttons[0].id.clone()));
/// ```
///
/// Not `Eq`: [`DialogButton`] carries an `f32`-bearing [`crate::ButtonSpec`]
/// override, which has no total order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// - [`DialogInput::Form`] — a multi-field [`Form`] (e.g. Name +
    ///   Description); used when a single [`DialogTextInput`] isn't
    ///   enough. Rendered and driven through
    ///   [`crate::compose::form_dialog::FormDialogController`], which
    ///   composes this `Dialog` with a [`crate::compose::FormController`]
    ///   so Tab/Shift-Tab traverse the form's fields and then the
    ///   button row.
    ///
    /// Apps own the value; interactions resolve via [`DialogLayout::hit_test`]
    /// / [`DialogHit`].
    #[serde(default)]
    pub input: Option<DialogInput>,
}

/// Content of the slot rendered between the body text and the button row.
///
/// Not `Eq`: [`Toolbar`]/[`DialogButton`] each carry an `f32`-bearing
/// [`crate::ButtonSpec`] override, which has no total order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DialogInput {
    /// Single-line text field (rename prompts, input-required confirms).
    TextInput(DialogTextInput),
    /// Inline horizontal action strip. Backends render this by calling their
    /// `draw_toolbar` equivalent inside the body slot. Clicks resolve as
    /// [`DialogHit::BodyToolbarButton`].
    Toolbar(Toolbar),
    /// A multi-field form (e.g. Name + Description), for dialogs that need
    /// more than one input. Unlike [`Self::Toolbar`] (which every
    /// backend's `draw_dialog` paints inline, inside the input slot),
    /// **no** backend's `draw_dialog` paints this variant at all — a
    /// bare `Dialog::layout` + `draw_dialog` caller gets a blank
    /// reserved hole where the form would go.
    /// [`crate::compose::form_dialog::FormDialogController`] is the
    /// intended (and today, only) way to use this variant: it paints
    /// the form separately through its widget contract — the same
    /// cross-backend path every other `Form` host uses — after calling
    /// `draw_dialog`, and owns the Tab/Shift-Tab focus hand-off between
    /// the form's fields and this dialog's buttons.
    Form(Form),
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
///
/// Not `Eq`: [`Self::spec`] carries a [`crate::ButtonSpec`], which has an
/// `f32` (`icon_gap`) with no total order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Override colour for destructive actions ("Delete", "Discard") on
    /// the legacy plain highlighted-row paint path. Ignored once
    /// [`Self::spec`] is `Some`: a spec expresses "destructive" through
    /// [`crate::ButtonVariant::Danger`]'s own paint colour instead of a
    /// standalone tint, so set `.variant` on the spec rather than
    /// relying on this field once a button carries one. `None` = theme
    /// default.
    #[serde(default)]
    pub tint: Option<Color>,
    /// Optional [`crate::ButtonSpec`] override, carrying the B3a
    /// vocabulary (variant intent — e.g. [`crate::ButtonVariant::Danger`]
    /// for "Move to Trash" — icon, icon placement/gap, a tooltip) this
    /// button's own legacy fields above can't express. `None` (the
    /// default) renders the plain highlighted-row look; see
    /// [`Self::resolved_button_spec`]. A `Some` spec is painted as-is —
    /// it, not [`Self::tint`], is the source of truth for a spec-backed
    /// button's colour.
    #[serde(default)]
    pub spec: Option<crate::primitives::button::ButtonSpec>,
}

impl DialogButton {
    /// This button's explicit [`Self::spec`] override when set, otherwise
    /// one derived from its legacy `is_default` field (default → primary,
    /// otherwise secondary) — see that field's own "deprecate-first" doc.
    pub fn resolved_button_spec(&self) -> crate::primitives::button::ButtonSpec {
        if let Some(spec) = &self.spec {
            return spec.clone();
        }
        let mut derived = crate::primitives::button::ButtonSpec::new(self.label.clone());
        derived.variant = if self.is_default {
            crate::primitives::button::ButtonVariant::Primary
        } else {
            crate::primitives::button::ButtonVariant::Secondary
        };
        derived
    }
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
                // `Dialog`'s body toolbar never embeds a `Field`/
                // `MenuButton` or opts into `ToolbarOverflow::Menu`
                // today; falls through to the generic `hit_regions` scan
                // below, same as `Empty`.
                ToolbarHit::Empty
                | ToolbarHit::Overflow
                | ToolbarHit::Field(_)
                | ToolbarHit::MenuButton(_) => {}
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
                    id: btn.id,
                    bounds: b,
                });
                hit_regions.push((b, DialogHit::Button(btn.id)));
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
                    id: btn.id,
                    bounds: b,
                });
                hit_regions.push((b, DialogHit::Button(btn.id)));
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

    /// Derive a [`DialogMeasure`] from `char_width` and `line_height`
    /// without real text measurement. This is the char-cell approximation
    /// used where `char_width == line_height == 1.0` on TUI (one character
    /// cell) and the backend's real glyph metrics on pixel backends.
    ///
    /// `border_chrome_inset` optionally shrinks the resulting `width` to
    /// leave room for a border the rasteriser draws *outside*
    /// [`DialogLayout::bounds`] rather than inside it — e.g. a TUI backend
    /// whose box-drawing glyphs live one cell past the content box, the
    /// same convention the `ContextMenu` cell painter already uses. Pass `0.0` when the
    /// border is painted *inside* the returned bounds — this crate's own
    /// `tui::dialog::draw_dialog` and every pixel backend do this.
    ///
    /// Does not measure [`Self::table`] or [`Self::buttons`]' rendered pixel
    /// width beyond the char-cell approximation above; callers with real
    /// text measurement available should prefer [`Self::measure_generic_measured`].
    ///
    /// Sizes [`DialogMeasure::button_row_height`] off the raw `line_height`
    /// on every backend, pixel or TUI — callers with a [`Style`] available
    /// should prefer [`Self::measure_generic_with_style`], which sizes the
    /// button row at [`Style::control_height`] on a pixel backend instead,
    /// matching [`crate::compose::MessageDialogController`]'s own button row.
    pub fn measure_generic(
        &self,
        char_width: f32,
        line_height: f32,
        viewport: Rect,
        border_chrome_inset: f32,
    ) -> DialogMeasure {
        self.measure_generic_inner(
            char_width,
            line_height,
            viewport,
            border_chrome_inset,
            None,
            None,
        )
    }

    /// Like [`Self::measure_generic`], but sizes [`DialogMeasure::button_row_height`]
    /// from [`Style::control_height`] on a pixel backend (`line_height > 1.0`)
    /// instead of the raw chrome line height — the same fixed-pixel button
    /// row [`crate::compose::MessageDialogController::layout`] sizes its
    /// OK/Cancel row at. On TUI (`line_height == 1.0`, the char-cell
    /// approximation), the button row stays one row tall — unchanged from
    /// [`Self::measure_generic`]. [`DialogButton::resolved_button_spec`]'s
    /// own corner radius ([`Style::button_radius`]) is applied separately,
    /// at paint time, by `native_surface_paint::paint`.
    pub fn measure_generic_with_style(
        &self,
        char_width: f32,
        line_height: f32,
        viewport: Rect,
        border_chrome_inset: f32,
        style: &Style,
    ) -> DialogMeasure {
        self.measure_generic_inner(
            char_width,
            line_height,
            viewport,
            border_chrome_inset,
            None,
            Some(style),
        )
    }

    /// Like [`Self::measure_generic`], but sizes [`DialogMeasure::button_width`]
    /// from each [`DialogButton::label`]'s real rendered width through
    /// `measure` instead of the fixed `char_width * 8.0` estimate — the
    /// widest button sets a uniform width for the row (`button_width` is
    /// shared by every button; see [`DialogMeasure::button_width`]'s doc),
    /// plus 2 char-widths of horizontal padding, floored at the original
    /// `char_width * 8.0` so a row of short labels ("OK" / "No") doesn't
    /// shrink below what every existing caller of [`Self::measure_generic`]
    /// already renders.
    ///
    /// Every other field is identical to [`Self::measure_generic`] — only
    /// the button row's width changes.
    pub fn measure_generic_measured(
        &self,
        char_width: f32,
        line_height: f32,
        viewport: Rect,
        border_chrome_inset: f32,
        measure: &dyn TextMeasure,
    ) -> DialogMeasure {
        self.measure_generic_inner(
            char_width,
            line_height,
            viewport,
            border_chrome_inset,
            Some(measure),
            None,
        )
    }

    /// Combines [`Self::measure_generic_measured`]'s real label-width
    /// button sizing with [`Self::measure_generic_with_style`]'s
    /// `Style::control_height` button row height — the full pixel-backend
    /// measurement a caller with both a real [`TextMeasure`] and a
    /// [`Style`] on hand should prefer over either single-purpose variant.
    pub fn measure_generic_measured_with_style(
        &self,
        char_width: f32,
        line_height: f32,
        viewport: Rect,
        border_chrome_inset: f32,
        measure: &dyn TextMeasure,
        style: &Style,
    ) -> DialogMeasure {
        self.measure_generic_inner(
            char_width,
            line_height,
            viewport,
            border_chrome_inset,
            Some(measure),
            Some(style),
        )
    }

    fn measure_generic_inner(
        &self,
        char_width: f32,
        line_height: f32,
        viewport: Rect,
        border_chrome_inset: f32,
        measure: Option<&dyn TextMeasure>,
        style: Option<&Style>,
    ) -> DialogMeasure {
        let table = self.table.as_ref();
        let table_total_h = table
            .map(|t| t.tui_total_height() as f32 * line_height)
            .unwrap_or(0.0);
        // Preferred table width: char cells × char_width + 2 char-widths
        // of padding.
        let table_preferred_w = table
            .map(|t| t.tui_total_width() as f32 * char_width + char_width * 2.0)
            .unwrap_or(0.0);

        let title_h = if self.title.spans.iter().any(|s| !s.text.is_empty()) {
            line_height
        } else {
            0.0
        };
        let body_h = self.body.len() as f32 * line_height;
        // A `Form` input needs one row per field, not the single row
        // every other `DialogInput` variant occupies.
        let input_h = match &self.input {
            Some(DialogInput::Form(form)) => line_height * form.fields.len().max(1) as f32,
            Some(_) => line_height,
            None => 0.0,
        };

        let min_w = char_width * 30.0; // ≈ 30 char-widths
        let max_w = char_width * 60.0; // ≈ 60 char-widths
        let default_w = (viewport.width * 0.5).clamp(min_w, max_w);
        let dialog_w = default_w
            .max(table_preferred_w)
            .min(viewport.width - char_width * 4.0 - border_chrome_inset * 2.0);

        // Floored at today's 8-char-width estimate so a row of short
        // legacy labels doesn't shrink below what every existing caller
        // of `measure_generic`/`measure_generic_measured` already
        // renders; widened per-button via `resolved_button_spec()` so an
        // explicit `spec`'s icon + longer label never clips its closing
        // bracket — see `ButtonSpec::min_cell_width`'s own doc.
        let estimated_button_w = char_width * 8.0;
        let button_width = self
            .buttons
            .iter()
            .map(|b| {
                let spec = b.resolved_button_spec();
                let spec_w = spec.min_cell_width() * char_width;
                let measured_w = match measure {
                    Some(m) => m.width_of(&spec.label) + char_width * 2.0,
                    None => estimated_button_w,
                };
                spec_w.max(measured_w)
            })
            .fold(estimated_button_w, f32::max);

        // On a pixel backend (`line_height > 1.0`), a `style` sizes the
        // button row at `ButtonMetrics::resolve`'s `height` — the same
        // single source of truth for button geometry
        // `native_surface_paint::paint` resolves its own `corner_radius`
        // through just below, and the same fixed-pixel row
        // `MessageDialogController::layout`'s own `is_pixel` split
        // renders at. TUI's char-grid geometry (`line_height == 1.0`)
        // is unaffected either way.
        let is_pixel = line_height > 1.0;
        let button_row_height = match style {
            Some(s) if is_pixel => crate::primitives::button::ButtonMetrics::resolve(s).height,
            _ => line_height,
        };

        DialogMeasure {
            width: dialog_w,
            title_height: title_h,
            body_height: body_h,
            table_height: table_total_h,
            input_height: input_h,
            button_row_height,
            button_width,
            button_gap: char_width * 2.0,
            padding: line_height,
        }
    }
}

/// Flatten a [`StyledText`] to plain text. Native alert facilities carry
/// no per-span styling, so this discards it the same way `gtk::dialog`
/// and `tui::dialog`'s private `flatten` helpers do for the in-canvas
/// rasterisers.
fn flatten(text: &StyledText) -> String {
    text.spans.iter().map(|s| s.text.as_str()).collect()
}

/// Map a [`Dialog`] descriptor to the options
/// [`crate::backend::PlatformServices::show_message_dialog`] needs, or
/// `None` when `d` is not natively expressible.
///
/// Not natively expressible today: `d.table.is_some() || d.input.is_some()`
/// — no native alert facility hosts a table or a text field, so callers
/// must fall back to `draw_dialog` (the in-canvas primitive) for those.
/// This decision lives here, as a pure function of the descriptor, so
/// callers never grow their own hand-written list of "which dialogs can
/// go native" — the same reasoning behind keeping `gtk::minimap::render_mode`
/// pure and unit-testable with no live surface.
///
/// `DialogButton::tint` has no equivalent in [`MessageDialogButton`] (no
/// native alert facility exposes per-button colour) and is silently
/// dropped — every other field maps straight across.
///
/// `d.buttons` is passed through as-is, including when empty — this
/// function does not enforce "a dialog must declare at least one
/// button". Every native alert facility (GTK's `AlertDialog`, macOS's
/// `NSAlert`, Win32's `MessageBoxEx`) expects at least one, so a caller
/// building a `Dialog` for native display should always populate
/// `buttons`; `Dialog`'s own doc doesn't currently state that invariant
/// either.
pub fn native_dialog_options(d: &Dialog) -> Option<MessageDialogOptions> {
    if d.table.is_some() || d.input.is_some() {
        return None;
    }
    Some(MessageDialogOptions {
        title: flatten(&d.title),
        body: d.body.iter().map(flatten).collect::<Vec<_>>().join("\n"),
        buttons: d
            .buttons
            .iter()
            .map(|b| MessageDialogButton {
                id: b.id,
                label: b.label.clone(),
                is_default: b.is_default,
                is_cancel: b.is_cancel,
                spec: b.spec.clone(),
            })
            .collect(),
        severity: d.severity,
    })
}

// PaintSurface Phase 4: shared paint implementation written against
// [`crate::paint_surface::PaintSurface`] instead of any one backend's
// API. Pre-migration per-backend implementations had substantive drift,
// resolved by picking one implementation and documenting divergences:
//
// - **Font role.** The whole primitive is classified as chrome — one font
//   for the entire box. Pre-migration, only macOS did this; GTK swapped
//   fonts mid-paint (title/buttons in chrome, body/table/input in editor
//   monospace), and Windows used only the editor font. `paint` takes a
//   single `surface` already constructed against the chrome font by its
//   caller, so the whole dialog now renders in chrome everywhere.
//   `DialogInput::Toolbar` is the exception: `paint` does not render it.
// - **Per-button border stroke.** Windows alone stroked `theme.border_fg`
//   around every button; GTK/macOS filled the default button's
//   selected-bg but never stroked a border. `paint` strokes every button.
// - **`DialogButton::tint`.** Override colour for destructive actions.
//   Windows was the only backend that read it; GTK/macOS always painted
//   labels in `theme.surface_fg`. `paint` reads tint on every backend.
// - **Button label vertical centring.** GTK/macOS centre each label;
//   Windows left labels at the top. `paint` centres on every backend.
// - **`DialogTextInput` vertical centring.** Same drift: GTK/macOS
//   centred display text; Windows passed the box's full height uncentred.
// - **`DialogTable` explicit `column_widths`.** GTK/macOS honour widths
//   as per-column minimum; Windows never read the field.
// - **`DialogTable` header-separator dash count.** GTK/macOS divide by
//   approximate char width; Windows divided by plain `line_height`,
//   causing the separator row to undershoot actual content width.
//
// `DialogInput::Toolbar` is not painted by `paint` — the wrapper paints
// it after `paint` returns, through
// `primitives::toolbar::native_surface_paint::paint_embedded` (GTK/macOS)
// or inline Direct2D code (Windows). This skips nothing a caller
// observes: every backend's `dialog_layout.input_bounds` position is
// unaffected.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{Dialog, DialogInput, DialogLayout, DialogTable};
    use crate::paint_surface::PaintSurface;
    use crate::style::Style;
    use crate::theme::Theme;
    use crate::types::Color;
    use crate::{Point, Rect};

    /// Approximate monospace character width ratio: converts
    /// [`DialogTable::column_widths`] char-cell hints to pixels.
    const APPROX_CHAR_WIDTH_RATIO: f32 = 0.6;

    /// Horizontal gap reserved between two columns for the 1px separator
    /// line drawn through its middle — replaces the ` │ ` text-run
    /// separator with a drawn rule on GUI backends; the TUI painter is
    /// untouched and keeps drawing the glyph.
    const COLUMN_SEP_GAP: f32 = 16.0;

    /// Paint a [`DialogTable`] at `(bounds.x, bounds.y)`, top-left
    /// anchored. Column widths are auto-computed from content via
    /// [`PaintSurface::surface_measure_text`], honouring
    /// `table.column_widths` as a per-column minimum. A header row
    /// (when present) is followed by a drawn 1px rule — no `┼`
    /// junction, matching every backend's pre-migration simplification.
    /// Column separators and the header rule are drawn as 1px lines
    /// rather than the ` │ ` / `─` glyphs the TUI painter still emits —
    /// those glyphs read as terminal affordances on a pixel canvas.
    fn draw_table(
        surface: &mut dyn PaintSurface,
        table: &DialogTable,
        bounds: Rect,
        line_height: f32,
        fg: crate::Color,
        border: crate::Color,
    ) {
        let ncols = table.num_cols();
        if ncols == 0 {
            return;
        }

        let measure_w = |surface: &dyn PaintSurface, s: &str| surface.surface_measure_text(s).0;

        let mut col_w = vec![0.0f32; ncols];
        if let Some(headers) = &table.headers {
            for (j, h) in headers.iter().enumerate().take(ncols) {
                col_w[j] = col_w[j].max(measure_w(surface, h));
            }
        }
        for row in &table.rows {
            for (j, cell) in row.iter().enumerate().take(ncols) {
                col_w[j] = col_w[j].max(measure_w(surface, cell));
            }
        }

        if let Some(explicit) = &table.column_widths {
            for (j, &w) in explicit.iter().enumerate() {
                if j < ncols {
                    let px = w as f32 * (line_height * APPROX_CHAR_WIDTH_RATIO);
                    col_w[j] = col_w[j].max(px);
                }
            }
        }

        let mut col_x = vec![0.0f32; ncols];
        let mut cursor_x = bounds.x;
        for j in 0..ncols {
            col_x[j] = cursor_x;
            cursor_x += col_w[j];
            if j + 1 < ncols {
                cursor_x += COLUMN_SEP_GAP;
            }
        }

        let mut row_y = bounds.y;
        let row_rect = |x: f32, y: f32, w: f32| Rect::new(x, y, w.max(1.0), line_height);
        let draw_col_separators = |surface: &mut dyn PaintSurface, row_y: f32| {
            for j in 0..ncols.saturating_sub(1) {
                let sep_x = col_x[j] + col_w[j] + COLUMN_SEP_GAP / 2.0;
                surface.surface_draw_line(
                    Point::new(sep_x, row_y + 1.0),
                    Point::new(sep_x, row_y + line_height - 1.0),
                    border,
                    1.0,
                );
            }
        };

        if let Some(headers) = &table.headers {
            for (j, h) in headers.iter().enumerate().take(ncols) {
                surface.surface_draw_text_run(row_rect(col_x[j], row_y, col_w[j]), h, fg);
            }
            draw_col_separators(surface, row_y);
            row_y += line_height;

            let total_w = col_x[ncols - 1] + col_w[ncols - 1] - bounds.x;
            let rule_y = row_y + line_height / 2.0;
            surface.surface_draw_line(
                Point::new(bounds.x, rule_y),
                Point::new(bounds.x + total_w, rule_y),
                border,
                1.0,
            );
            row_y += line_height;
        }

        for row in &table.rows {
            for (j, cell) in row.iter().enumerate().take(ncols) {
                surface.surface_draw_text_run(row_rect(col_x[j], row_y, col_w[j]), cell, fg);
            }
            draw_col_separators(surface, row_y);
            row_y += line_height;
        }
    }

    /// Paint a [`Dialog`] at its resolved `dialog_layout` onto `surface`
    /// — background, border, title, body, optional table, optional
    /// [`DialogInput::TextInput`] slot, and buttons (selected-bg fill +
    /// border stroke + tint-aware label). Returns the per-button hit
    /// rectangles in `dialog_layout.visible_buttons` order, matching
    /// [`crate::Backend::draw_dialog`]'s contract.
    ///
    /// `surface` must already be constructed against the **chrome**
    /// font — see this module's doc for why the whole dialog paints in
    /// one font now. `DialogInput::Toolbar` is deliberately not painted
    /// here; see the module doc. `style` supplies the
    /// corner radius/border width/shadow elevation the box paints with
    /// — callers pass `&self.style()`.
    pub(crate) fn paint(
        dialog: &Dialog,
        dialog_layout: &DialogLayout,
        line_height: f32,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        style: &Style,
    ) -> Vec<Rect> {
        let bounds = dialog_layout.bounds;
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return Vec::new();
        }

        // Modal dialogs get the heaviest shadow elevation of the
        // overlays this issue wires up — matching VS Code's own modal
        // tier.
        surface.surface_draw_shadow(
            bounds,
            style.corner_radius,
            style.shadow_elevation.max(3),
            Color::rgba(0, 0, 0, 120),
        );
        surface.surface_fill_rounded_rect(bounds, style.corner_radius, theme.surface_bg);
        surface.surface_stroke_rounded_rect(
            bounds,
            style.corner_radius,
            theme.border_fg,
            style.border_width,
        );

        if let Some(title_rect) = dialog_layout.title_bounds {
            surface.surface_draw_text_run(
                title_rect,
                &super::flatten(&dialog.title),
                theme.title_fg,
            );
        }

        let body_b = dialog_layout.body_bounds;
        for (i, line) in dialog.body.iter().enumerate() {
            let row_y = body_b.y + i as f32 * line_height;
            if row_y + line_height > body_b.y + body_b.height {
                break;
            }
            let row_rect = Rect::new(body_b.x, row_y, body_b.width, line_height);
            surface.surface_draw_text_run(row_rect, &super::flatten(line), theme.surface_fg);
        }

        if let (Some(table_b), Some(table)) = (dialog_layout.table_bounds, dialog.table.as_ref()) {
            draw_table(
                surface,
                table,
                table_b,
                line_height,
                theme.surface_fg,
                theme.border_fg,
            );
        }

        if let (Some(input_b), Some(DialogInput::TextInput(input))) =
            (dialog_layout.input_bounds, dialog.input.as_ref())
        {
            surface.surface_fill_rect(input_b, theme.input_bg);
            surface.surface_stroke_rect(input_b, theme.border_fg, 1.0);
            let display = if input.value.is_empty() {
                format!(" {}", input.placeholder)
            } else {
                format!(" {}", input.value)
            };
            let (_, ih) = surface.surface_measure_text(&display);
            let text_rect = Rect::new(
                input_b.x + 2.0,
                input_b.y + (input_b.height - ih) / 2.0,
                (input_b.width - 2.0).max(0.0),
                ih.max(1.0),
            );
            surface.surface_draw_text_run(text_rect, &display, theme.surface_fg);
        }
        // DialogInput::Toolbar is intentionally not painted here — see
        // this module's doc.

        let mut rects = Vec::with_capacity(dialog_layout.visible_buttons.len());
        for vis in &dialog_layout.visible_buttons {
            let btn = &dialog.buttons[vis.button_idx];
            rects.push(vis.bounds);

            // An explicit `spec` override paints through the shared
            // `Button` fill/border/icon/intent-colour path instead of
            // this bare highlighted-row look, so e.g. a `Danger` spec
            // reads in `theme.button_danger_bg()` the same way
            // `crate::tui::dialog`'s own spec-backed path already does —
            // see that module's doc. `btn.is_default` maps to `focused`
            // on the materialised `Button`, so the "this is the default"
            // cue (an accent-tinted border/focus ring) survives on a
            // spec-backed button the same way the legacy path's
            // `selected_bg` fill does below. A `spec: None` button keeps
            // painting through the plain highlighted-row/label path
            // below, but still fills/strokes at `Style::button_radius`
            // (via `ButtonMetrics::resolve`) so both paths share the
            // same corner shape.
            if let Some(spec) = &btn.spec {
                let button = spec.to_button(btn.id, btn.is_default);
                crate::primitives::button::native_surface_paint::paint(
                    &button, surface, theme, vis.bounds, style, false, false,
                );
                continue;
            }

            let button_radius =
                crate::primitives::button::ButtonMetrics::resolve(style).corner_radius;
            if btn.is_default {
                surface.surface_fill_rounded_rect(vis.bounds, button_radius, theme.selected_bg);
            }
            surface.surface_stroke_rounded_rect(vis.bounds, button_radius, theme.border_fg, 1.0);

            // The default button's style (selected_bg fill + border) is
            // the marker on GUI — no leading glyph needed (the TUI
            // painter still prefixes "▸ " since it has no fill
            // affordance to lean on).
            let label = btn.label.clone();
            let label_fg = btn.tint.unwrap_or(theme.surface_fg);
            let (lw, lh) = surface.surface_measure_text(&label);
            let label_x = if dialog.vertical_buttons {
                vis.bounds.x + 8.0
            } else {
                vis.bounds.x + (vis.bounds.width - lw) / 2.0
            };
            let label_y = vis.bounds.y + (vis.bounds.height - lh) / 2.0;
            surface.surface_draw_text_run(
                Rect::new(label_x, label_y, lw.max(1.0), lh.max(1.0)),
                &label,
                label_fg,
            );
        }

        rects
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::primitives::dialog::{DialogButton, VisibleDialogButton};
        use crate::types::{Color, StyledText, WidgetId};
        use crate::Image;

        /// Records every drawing verb issued by `draw_table` / `paint` —
        /// mirrors every other primitive's `RecordingSurface` test
        /// double (e.g. `primitives::form`, `primitives::pipeline_view`).
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            /// Every fill's rounded-rect radius, in the same order as
            /// `fills` — a plain `surface_fill_rect` call records `0.0`
            /// here so a test asserting "this fill used the button
            /// radius" can zip the two vecs instead of needing a
            /// separate recording per call shape.
            fill_radii: Vec<f32>,
            strokes: Vec<(Rect, Color, f32)>,
            /// Parallel to `strokes` — see `fill_radii`'s doc.
            stroke_radii: Vec<f32>,
            text_runs: Vec<(Rect, String, Color)>,
            lines: Vec<(Point, Point, Color, f32)>,
        }

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(300.0, 200.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                14.0
            }
            fn surface_char_width(&self) -> f32 {
                7.0
            }
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 7.0, 12.0)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
                self.fill_radii.push(0.0);
            }
            fn surface_fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
                self.fills.push((rect, color));
                self.fill_radii.push(radius);
            }
            fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
                self.strokes.push((rect, color, stroke_width));
                self.stroke_radii.push(0.0);
            }
            fn surface_stroke_rounded_rect(
                &mut self,
                rect: Rect,
                radius: f32,
                color: Color,
                stroke_width: f32,
            ) {
                self.strokes.push((rect, color, stroke_width));
                self.stroke_radii.push(radius);
            }
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.text_runs.push((rect, text.to_string(), color));
            }
            fn surface_draw_line(
                &mut self,
                from: crate::Point,
                to: crate::Point,
                color: Color,
                stroke_width: f32,
            ) {
                self.lines.push((from, to, color, stroke_width));
            }
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        /// Column separators and the header rule paint as 1px lines,
        /// never the ` │ ` / `─` glyphs the TUI painter still emits.
        #[test]
        fn draw_table_paints_lines_not_separator_glyphs() {
            let table = DialogTable {
                headers: Some(vec!["Key".to_string(), "Action".to_string()]),
                rows: vec![vec!["Ctrl+S".to_string(), "Save".to_string()]],
                column_widths: None,
            };
            let mut surface = RecordingSurface::default();
            draw_table(
                &mut surface,
                &table,
                Rect::new(0.0, 0.0, 200.0, 42.0),
                14.0,
                Color::rgb(255, 255, 255),
                Color::rgb(128, 128, 128),
            );

            assert!(
                !surface
                    .text_runs
                    .iter()
                    .any(|(_, t, _)| t.contains('│') || t.contains('─')),
                "no ` │ ` / `─` glyph should be painted; text runs were {:?}",
                surface.text_runs,
            );
            // One column separator per row (header + data row) plus the
            // header rule below it = 3 lines.
            assert_eq!(
                surface.lines.len(),
                3,
                "expected 2 column separators + 1 header rule; lines were {:?}",
                surface.lines,
            );
        }

        /// The default button's marker is the selected-bg fill +
        /// border, not a leading `▸ ` glyph.
        #[test]
        fn default_button_has_no_leading_glyph() {
            let dialog = Dialog {
                id: WidgetId::new("dialog:confirm"),
                title: StyledText::plain("Confirm"),
                body: vec![],
                buttons: vec![DialogButton {
                    spec: None,
                    id: WidgetId::new("dialog:ok"),
                    label: "OK".to_string(),
                    is_default: true,
                    is_cancel: false,
                    tint: None,
                }],
                severity: None,
                vertical_buttons: true,
                table: None,
                input: None,
            };
            let dlayout = DialogLayout {
                bounds: Rect::new(0.0, 0.0, 100.0, 60.0),
                title_bounds: None,
                body_bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
                table_bounds: None,
                input_bounds: None,
                body_toolbar_layout: None,
                button_row_bounds: Rect::new(0.0, 40.0, 100.0, 20.0),
                visible_buttons: vec![VisibleDialogButton {
                    button_idx: 0,
                    id: dialog.buttons[0].id.clone(),
                    bounds: Rect::new(10.0, 40.0, 40.0, 16.0),
                }],
                hit_regions: vec![],
            };
            let mut surface = RecordingSurface::default();
            paint(
                &dialog,
                &dlayout,
                14.0,
                &mut surface,
                &Theme::default(),
                &crate::style::Style::default(),
            );

            assert!(
                surface.text_runs.iter().any(|(_, t, _)| t == "OK"),
                "the button label itself must still be painted; text runs \
                 were {:?}",
                surface.text_runs,
            );
            assert!(
                !surface.text_runs.iter().any(|(_, t, _)| t.contains('▸')),
                "no leading glyph should be painted on the default button; \
                 text runs were {:?}",
                surface.text_runs,
            );
            let theme = Theme::default();
            assert!(
                surface.fills.iter().any(|(_, c)| *c == theme.selected_bg),
                "the default button must still fill selected_bg as its \
                 marker; fills were {:?}",
                surface.fills,
            );
        }

        /// A `spec: None` button — the default OK/Cancel path
        /// `MessageDialogController` builds — still fills and strokes at
        /// `Style::button_radius`, not square corners: it shares
        /// `ButtonMetrics::resolve` with the spec-backed path above
        /// rather than falling back to the bare `surface_fill_rect`/
        /// `surface_stroke_rect` verbs.
        #[test]
        fn spec_none_button_paints_at_the_shared_button_radius() {
            let dialog = Dialog {
                id: WidgetId::new("dialog:confirm"),
                title: StyledText::plain("Confirm"),
                body: vec![],
                buttons: vec![DialogButton {
                    spec: None,
                    id: WidgetId::new("dialog:ok"),
                    label: "OK".to_string(),
                    is_default: true,
                    is_cancel: false,
                    tint: None,
                }],
                severity: None,
                vertical_buttons: true,
                table: None,
                input: None,
            };
            let dlayout = DialogLayout {
                bounds: Rect::new(0.0, 0.0, 100.0, 60.0),
                title_bounds: None,
                body_bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
                table_bounds: None,
                input_bounds: None,
                body_toolbar_layout: None,
                button_row_bounds: Rect::new(0.0, 40.0, 100.0, 20.0),
                visible_buttons: vec![VisibleDialogButton {
                    button_idx: 0,
                    id: dialog.buttons[0].id.clone(),
                    bounds: Rect::new(10.0, 40.0, 40.0, 16.0),
                }],
                hit_regions: vec![],
            };
            let style = crate::style::Style::default();
            let mut surface = RecordingSurface::default();
            paint(
                &dialog,
                &dlayout,
                14.0,
                &mut surface,
                &Theme::default(),
                &style,
            );

            let expected_radius =
                crate::primitives::button::ButtonMetrics::resolve(&style).corner_radius;
            assert_ne!(
                expected_radius, 0.0,
                "the default Style::button_radius must be non-zero for \
                 this test to be meaningful",
            );
            assert!(
                surface.fill_radii.contains(&expected_radius),
                "the default button's fill must use Style::button_radius \
                 ({expected_radius}); fill radii were {:?}",
                surface.fill_radii,
            );
            assert!(
                surface.stroke_radii.contains(&expected_radius),
                "the button border must stroke at Style::button_radius \
                 ({expected_radius}); stroke radii were {:?}",
                surface.stroke_radii,
            );
        }

        /// A `Danger` spec on a `DialogButton` paints through
        /// `button::native_surface_paint::paint` — `theme.button_danger_bg()`,
        /// not the plain `selected_bg`/`theme.border_fg` highlighted-row
        /// look `spec: None` buttons still get.
        #[test]
        fn danger_spec_button_fills_the_shared_button_danger_colour() {
            let mut spec = crate::primitives::button::ButtonSpec::new("Move to Trash");
            spec.variant = crate::primitives::button::ButtonVariant::Danger;
            let dialog = Dialog {
                id: WidgetId::new("dialog:confirm"),
                title: StyledText::plain("Confirm"),
                body: vec![],
                buttons: vec![DialogButton {
                    spec: Some(spec),
                    id: WidgetId::new("dialog:delete"),
                    label: "Move to Trash".to_string(),
                    is_default: true,
                    is_cancel: false,
                    tint: None,
                }],
                severity: None,
                vertical_buttons: true,
                table: None,
                input: None,
            };
            let dlayout = DialogLayout {
                bounds: Rect::new(0.0, 0.0, 100.0, 60.0),
                title_bounds: None,
                body_bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
                table_bounds: None,
                input_bounds: None,
                body_toolbar_layout: None,
                button_row_bounds: Rect::new(0.0, 40.0, 100.0, 20.0),
                visible_buttons: vec![VisibleDialogButton {
                    button_idx: 0,
                    id: dialog.buttons[0].id.clone(),
                    bounds: Rect::new(10.0, 40.0, 40.0, 16.0),
                }],
                hit_regions: vec![],
            };
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(
                &dialog,
                &dlayout,
                14.0,
                &mut surface,
                &theme,
                &crate::style::Style::default(),
            );

            assert!(
                surface
                    .fills
                    .iter()
                    .any(|(_, c)| *c == theme.button_danger_bg()),
                "a Danger spec must fill the shared Button's danger colour; \
                 fills were {:?}",
                surface.fills,
            );
            assert!(
                surface
                    .text_runs
                    .iter()
                    .any(|(_, t, _)| t == "Move to Trash"),
                "the button label must still be painted through the shared \
                 Button path; text runs were {:?}",
                surface.text_runs,
            );
        }
    }
}

// ── Cell-grid measurement ────────────────────────────────────────────────────

/// The cell-grid [`DialogLayout`] — [`cell_measure`] for the box, and
/// [`crate::primitives::toolbar::cell_item_width`] for a
/// [`DialogInput::Toolbar`] body slot's item widths.
///
/// The single cell-units entry point: [`Primitive::layout`] on a
/// [`crate::primitive::Units::Cells`] context and `tui::tui_dialog_layout`
/// both resolve through here, so a feature set with no terminal backend
/// compiled in lays a dialog out exactly as a terminal does — including
/// the body toolbar's hit regions.
pub(crate) fn cell_layout(dialog: &Dialog, viewport: Rect) -> DialogLayout {
    Dialog::layout(dialog, viewport, cell_measure(dialog, viewport), |btn| {
        ToolbarItemMeasure::new(crate::primitives::toolbar::cell_item_width(btn))
    })
}

/// The cell-grid [`DialogMeasure`] a terminal paints a [`Dialog`] at:
/// one row per body line (capped at [`CELL_MAX_BODY_ROWS`] so a long
/// body can't fill the whole viewport), a one-row title/button row, and
/// a box width that fits the widest table column, clamped to
/// 30…60 columns and never wider than the viewport minus a two-column
/// margin either side.
///
/// Every other field matches what [`Dialog::measure_generic`] derives
/// for `char_width == line_height == 1.0`; only the body-row cap is
/// specific to the cell grid, where a dialog shares a 24-row terminal
/// with the app behind it.
pub(crate) fn cell_measure(dialog: &Dialog, viewport: Rect) -> DialogMeasure {
    let body_h = (dialog.body.len() as f32).clamp(0.0, CELL_MAX_BODY_ROWS);

    let (table_h, table_preferred_w) = match &dialog.table {
        // +2 for the 1-cell padding on each side.
        Some(t) => (
            t.tui_total_height() as f32,
            t.tui_total_width() as f32 + 2.0,
        ),
        None => (0.0, 0.0),
    };

    // A `Form` input needs one row per field, not the single row every
    // other `DialogInput` variant occupies.
    let input_h = match &dialog.input {
        Some(DialogInput::Form(form)) => form.fields.len().max(1) as f32,
        Some(_) => 1.0,
        None => 0.0,
    };

    let default_w = (viewport.width * 0.5).clamp(30.0, 60.0);
    let dialog_w = default_w.max(table_preferred_w).min(viewport.width - 4.0);

    // Floored at an 8-cell estimate so a row of short labels ("OK" /
    // "No") doesn't shrink below the width a plain label row renders at;
    // widened per-button via `resolved_button_spec()` so an explicit
    // `spec`'s icon + longer label (e.g. a `Danger` "Move to Trash")
    // never clips its closing bracket — see
    // `ButtonSpec::min_cell_width`'s own doc.
    let button_width = dialog
        .buttons
        .iter()
        .map(|b| b.resolved_button_spec().min_cell_width())
        .fold(8.0_f32, f32::max);

    DialogMeasure {
        width: dialog_w,
        title_height: if dialog.title.spans.iter().any(|s| !s.text.is_empty()) {
            1.0
        } else {
            0.0
        },
        body_height: body_h,
        table_height: table_h,
        input_height: input_h,
        button_row_height: 1.0,
        button_width,
        button_gap: 2.0,
        padding: 1.0,
    }
}

/// Most body rows [`cell_measure`] reserves — see its own doc.
const CELL_MAX_BODY_ROWS: f32 = 10.0;

// ── Cell-grid painter ───────────────────────────────────────────────────────

/// The cell-grid painter for [`Dialog`], written against
/// [`crate::CellSurface`].
///
/// A bordered modal box with rounded `╭─╮ ╰─╯` corners, the title
/// overlaid into the top rule, multi-line body text, an optional table,
/// an optional input slot, and a row (or column) of buttons.
///
/// One sub-region reaches for [`CellSurface::ratatui_buffer`] rather than
/// a cell verb, because the widget it paints is a whole primitive of its
/// own with its own cell rasteriser: a [`DialogInput::Toolbar`] body slot
/// paints through the toolbar rasteriser. A [`DialogButton`] carrying an
/// explicit [`crate::ButtonSpec`] paints through
/// [`crate::primitives::button::cell_paint::paint`] (bracket chrome,
/// icon, intent colour) directly against this module's own `surface`,
/// needing no raw buffer. Everything else here is
/// `fill`/`put_char`/`put_str_clipped`.
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::{flatten, Dialog, DialogInput, DialogLayout, DialogTable};
    use crate::cell_surface::{CellStyle, CellSurface};
    use crate::theme::Theme;
    use crate::types::Color;
    use crate::Rect;

    /// A foreground/background pair with no modifier bits — the only
    /// [`CellStyle`] shape this painter builds.
    fn ink(fg: Color, bg: Color) -> CellStyle {
        CellStyle {
            fg: Some(fg),
            bg: Some(bg),
            ..CellStyle::default()
        }
    }

    /// Render a [`DialogTable`] inside `bounds`.
    ///
    /// Columns are separated by ` │ `. When headers are present the
    /// first two rows are the header row and a `─┼─` separator.
    fn draw_table(
        surface: &mut dyn CellSurface,
        table: &DialogTable,
        bounds: &Rect,
        fg: Color,
        bg: Color,
        border_fg: Color,
    ) {
        let col_widths = table.auto_col_widths();
        if col_widths.is_empty() {
            return;
        }
        let ncols = col_widths.len();
        let start_x = bounds.x.round() as u16;
        let start_y = bounds.y.round() as u16;
        let max_w = bounds.width.round() as u16;
        let max_h = bounds.height.round() as u16;

        // Column x-offsets relative to `start_x`.
        let mut col_offsets = Vec::with_capacity(ncols);
        let mut cursor = 0u16;
        for (j, &w) in col_widths.iter().enumerate() {
            col_offsets.push(cursor);
            cursor += w as u16;
            if j + 1 < ncols {
                cursor += 3; // " │ "
            }
        }

        let mut row_idx = 0u16;

        // One text run inside one column's slot, clipped to both the
        // column's own width and the table's remaining width.
        let paint_cell = |surface: &mut dyn CellSurface,
                          row: u16,
                          col_start: u16,
                          col_max_w: u16,
                          text: &str,
                          text_fg: Color| {
            let y = start_y + row;
            if y >= start_y + max_h {
                return;
            }
            let budget = col_max_w.min(max_w.saturating_sub(col_start));
            surface.put_str_clipped(start_x + col_start, y, text, budget, ink(text_fg, bg));
        };

        // The vertical ` │ ` column separators on one row.
        let paint_separators = |surface: &mut dyn CellSurface, row: u16, sep_ch: char| {
            let y = start_y + row;
            if y >= start_y + max_h {
                return;
            }
            for j in 0..ncols.saturating_sub(1) {
                let sep_x = start_x + col_offsets[j] + col_widths[j] as u16 + 1;
                if sep_x < start_x + max_w {
                    surface.put_char(sep_x, y, sep_ch, ink(border_fg, bg));
                }
            }
        };

        if let Some(headers) = &table.headers {
            for (j, h) in headers.iter().enumerate() {
                if j < ncols {
                    paint_cell(
                        surface,
                        row_idx,
                        col_offsets[j],
                        col_widths[j] as u16,
                        h,
                        fg,
                    );
                }
            }
            paint_separators(surface, row_idx, '│');
            row_idx += 1;

            // Separator row: ────┼────
            let y = start_y + row_idx;
            if y < start_y + max_h {
                let rule = ink(border_fg, bg);
                let mut x = start_x;
                for (j, &col_w) in col_widths.iter().enumerate().take(ncols) {
                    for _ in 0..col_w as u16 {
                        if x < start_x + max_w {
                            surface.put_char(x, y, '─', rule);
                        }
                        x += 1;
                    }
                    if j + 1 < ncols {
                        for ch in ['─', '┼', '─'] {
                            if x < start_x + max_w {
                                surface.put_char(x, y, ch, rule);
                            }
                            x += 1;
                        }
                    }
                }
            }
            row_idx += 1;
        }

        for row in &table.rows {
            for (j, cell) in row.iter().enumerate() {
                if j < ncols {
                    paint_cell(
                        surface,
                        row_idx,
                        col_offsets[j],
                        col_widths[j] as u16,
                        cell,
                        fg,
                    );
                }
            }
            paint_separators(surface, row_idx, '│');
            row_idx += 1;
        }
    }

    /// Paint `dialog` at its resolved `layout`.
    ///
    /// `nerd_fonts_enabled` resolves the Nerd-Font half of a
    /// [`crate::ButtonSpec::icon`] on a button that carries one.
    pub(crate) fn paint(
        dialog: &Dialog,
        layout: &DialogLayout,
        surface: &mut dyn CellSurface,
        theme: &Theme,
        nerd_fonts_enabled: bool,
    ) {
        let bg = theme.surface_bg;
        let fg = theme.surface_fg;
        let border_fg = theme.border_fg;

        let x = layout.bounds.x.round() as u16;
        let y = layout.bounds.y.round() as u16;
        let w = layout.bounds.width.round() as u16;
        let h = layout.bounds.height.round() as u16;
        if w == 0 || h == 0 {
            return;
        }

        let body_ink = ink(fg, bg);
        let chrome = ink(border_fg, bg);

        // Clear the box area.
        surface.fill(Rect::new(x as f32, y as f32, w as f32, h as f32), body_ink);

        // Top border with the title overlaid into the rule. Painted cell
        // by cell rather than through `CellSurface::draw_border`: the
        // title overlay needs the top row's glyphs chosen individually,
        // and a degenerate one-row box still paints its bottom rule over
        // the top one, which a whole-rect border verb cannot express.
        for col in 0..w {
            let ch = if col == 0 {
                '╭'
            } else if col == w - 1 {
                '╮'
            } else {
                '─'
            };
            surface.put_char(x + col, y, ch, chrome);
        }
        let title_text = format!(" {} ", flatten(&dialog.title));
        if w > 3 {
            surface.put_str_clipped(x + 2, y, &title_text, w - 3, ink(theme.title_fg, bg));
        }

        // Left/right borders.
        for row in (y + 1)..(y + h - 1) {
            surface.put_char(x, row, '│', chrome);
            surface.put_char(x + w - 1, row, '│', chrome);
        }
        // Bottom border.
        for col in 0..w {
            let ch = if col == 0 {
                '╰'
            } else if col == w - 1 {
                '╯'
            } else {
                '─'
            };
            surface.put_char(x + col, y + h - 1, ch, chrome);
        }

        // Body text — one StyledText per line.
        let body_x = layout.body_bounds.x.round() as u16;
        let body_y = layout.body_bounds.y.round() as u16;
        let body_w = layout.body_bounds.width.round() as u16;
        let body_h = layout.body_bounds.height.round() as u16;
        for (i, line) in dialog.body.iter().enumerate() {
            let row = body_y + i as u16;
            if row >= body_y + body_h {
                break;
            }
            surface.put_str_clipped(body_x, row, &flatten(line), body_w, body_ink);
        }

        // Optional table slot.
        if let (Some(table_bounds), Some(table)) = (layout.table_bounds, &dialog.table) {
            draw_table(surface, table, &table_bounds, fg, bg, border_fg);
        }

        // Optional input slot — text input or embedded toolbar.
        if let (Some(input_bounds), Some(input_kind)) = (layout.input_bounds, &dialog.input) {
            let ix = input_bounds.x.round() as u16;
            let iy = input_bounds.y.round() as u16;
            let iw = input_bounds.width.round() as u16;
            match input_kind {
                DialogInput::TextInput(input) => {
                    let field = ink(fg, theme.input_bg);
                    surface.fill(Rect::new(ix as f32, iy as f32, iw as f32, 1.0), field);
                    surface.put_str_clipped(ix, iy, &format!(" {}", input.value), iw, field);
                }
                DialogInput::Toolbar(toolbar) => {
                    // The toolbar is its own primitive with its own cell
                    // rasteriser — see this module's doc for why this
                    // reaches for the raw buffer. `body_toolbar_layout`
                    // already resolved the positions; the rasteriser
                    // repaints from `input_bounds`, with no hover/pressed
                    // state on the dialog paint path.
                    if let Some(buf) = surface.ratatui_buffer() {
                        let area = ratatui::layout::Rect::new(ix, iy, iw, 1);
                        crate::primitives::toolbar::cell_paint::draw_toolbar(
                            buf, area, toolbar, theme, None, None,
                        );
                    }
                }
                // `FormDialogController` (`compose::form_dialog`) paints
                // the embedded form separately, through
                // `Backend::draw_form` inside `input_bounds`, the same
                // cross-backend path every other `Form` host uses —
                // nothing to draw here.
                DialogInput::Form(_) => {}
            }
        }

        // Buttons — the default button gets a `selected_bg` highlight.
        for vis in &layout.visible_buttons {
            let btn = &dialog.buttons[vis.button_idx];
            let bx = vis.bounds.x.round() as u16;
            let by = vis.bounds.y.round() as u16;
            let bw = vis.bounds.width.round() as u16;

            // An explicit `spec` override paints through the shared
            // `Button` bracket-chrome/icon/intent-colour path instead of
            // the bare highlighted row below, so e.g. a `Danger` spec
            // reads as `[ ⚠ Move to Trash ]` in `theme.error_fg`.
            // `btn.is_default` maps to `focused` on the materialised
            // `Button` so the "this is the default" cue (`accent_fg`
            // bracket chrome) survives on a spec-backed button the same
            // way the legacy path's `selected_bg` highlight does.
            if let Some(spec) = &btn.spec {
                let button = spec.to_button(btn.id, btn.is_default);
                let rect = Rect::new(bx as f32, by as f32, bw as f32, 1.0);
                let ctx = crate::primitive::LayoutCtx::new(1.0, 1.0, nerd_fonts_enabled);
                let btn_layout = crate::primitives::button::cell_button_layout(&button, &ctx, rect);
                crate::primitives::button::cell_paint::paint(
                    &button,
                    &btn_layout,
                    surface,
                    theme,
                    false,
                    false,
                );
                continue;
            }

            let btn_bg = if btn.is_default {
                theme.selected_bg
            } else {
                bg
            };
            let face = ink(fg, btn_bg);
            surface.fill(Rect::new(bx as f32, by as f32, bw as f32, 1.0), face);
            let label_w = btn.label.chars().count() as u16;
            // Vertical button lists (e.g. a repo picker) read as a
            // left-justified menu; horizontal action rows keep their
            // centred labels.
            let start = if dialog.vertical_buttons {
                bx + 1
            } else {
                bx + (bw.saturating_sub(label_w)) / 2
            };
            surface.put_str_clipped(start, by, &btn.label, (bx + bw).saturating_sub(start), face);
        }
    }
}

// ── Widget contract ─────────────────────────────────────────────────────────

/// [`Dialog`]'s fully-resolved widget-contract layout: the absolute area
/// the modal was laid out into, plus the [`DialogLayout`] resolved inside
/// it.
///
/// `layout`'s rects are **absolute** — [`DialogLayout`] bakes `area`'s
/// origin into every bound, the same frame [`DialogLayout::hit_test`] has
/// always taken — so [`Self::hit_test`] takes absolute coordinates too.
/// [`crate::interactive::Interactive::sub_regions`], which must answer in
/// widget-local space, subtracts `area`'s origin itself.
///
/// `#[non_exhaustive]`: produced by
/// [`Primitive::layout`](crate::primitive::Primitive::layout), never built
/// by a consumer via struct literal.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct DialogPrimitiveLayout {
    /// The rect [`Primitive::layout`](crate::primitive::Primitive::layout)
    /// received — the viewport the modal is centred in.
    pub area: Rect,
    /// The modal's own resolved layout, in absolute coordinates.
    pub layout: DialogLayout,
    /// Whether the laying-out backend had a Nerd Font icon face
    /// registered — see [`crate::primitive::LayoutCtx::nerd_fonts_enabled`].
    ///
    /// Baked into the layout because [`crate::primitive::Primitive::paint_cells`]
    /// takes no [`crate::primitive::LayoutCtx`] of its own, and a
    /// [`DialogButton::spec`] carrying a [`crate::ButtonSpec::icon`]
    /// resolves a different glyph with and without one.
    pub nerd_fonts: bool,
}

impl DialogPrimitiveLayout {
    /// Classify an absolute point against this modal — see
    /// [`DialogLayout::hit_test`].
    pub fn hit_test(&self, x: f32, y: f32) -> DialogHit {
        self.layout.hit_test(x, y)
    }
}

/// [`crate::interactive::Interactive::State`] for [`Dialog`]: which
/// button keyboard focus has reached.
///
/// `#[non_exhaustive]`: a future field add here is additive, the same
/// convention [`DialogPrimitiveLayout`] follows.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DialogState {
    /// Index into [`Dialog::buttons`] that Tab/Shift-Tab has moved
    /// keyboard focus to. `None` until the first Tab or click, at which
    /// point Enter falls back to [`Dialog::default_button_id`].
    pub focused_button: Option<usize>,
}

/// What a [`Dialog`]'s own [`crate::interactive::Interactive::handle`]
/// resolves one gesture to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogAction {
    /// A button in the modal's button row was activated — by click, by
    /// Enter on the keyboard-focused button, or by an assistive-technology
    /// `Click`. Carries that [`DialogButton::id`].
    ButtonActivated(WidgetId),
    /// Escape asked to cancel the modal. `button` is
    /// [`Dialog::cancel_button_id`] when the modal declares one, so a
    /// host can report the same choice a click on that button would have;
    /// `None` when it declares no cancel button, in which case the modal
    /// is simply dismissed.
    Cancelled {
        /// The declared cancel button, if any.
        button: Option<WidgetId>,
    },
    /// A button inside a [`DialogInput::Toolbar`] body slot was clicked.
    /// `toolbar` is that [`Toolbar`]'s own id and `event` is what
    /// [`Toolbar::click`] resolved — the body toolbar is an inline action
    /// strip, not part of the modal's own OK/Cancel row, so it stays a
    /// distinct outcome rather than reading as "the modal was answered".
    BodyToolbar {
        /// The embedded toolbar's own id.
        toolbar: WidgetId,
        /// What the click resolved to on that toolbar.
        event: crate::primitives::toolbar::ToolbarEvent,
    },
}

impl crate::interactive::WidgetAction for DialogAction {
    /// Both modal outcomes ride
    /// [`crate::UiEvent::MessageDialogResolved`] — the crate's one
    /// "an in-canvas dialog was answered" channel, which already names
    /// click, Enter and Escape-to-cancel as its three sources. A cancel
    /// with no declared cancel button reports the modal's own id, since
    /// there is no button id to name.
    ///
    /// A body-toolbar click rides [`crate::UiEvent::Toolbar`] under the
    /// embedded toolbar's own id instead, the same variant every other
    /// [`Toolbar`] host's clicks arrive as, so a host routes it with the
    /// handler it already has.
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        match self {
            DialogAction::ButtonActivated(button) => crate::UiEvent::MessageDialogResolved(button),
            DialogAction::Cancelled { button } => {
                crate::UiEvent::MessageDialogResolved(button.unwrap_or(id))
            }
            DialogAction::BodyToolbar { toolbar, event } => crate::UiEvent::Toolbar(toolbar, event),
        }
    }
}

impl crate::primitive::Primitive for Dialog {
    type Layout = DialogPrimitiveLayout;
    type Event = DialogHit;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::Dialog(self.clone())
    }

    /// The modal box's own size inside `avail` — what [`Self::layout`]
    /// would centre in a viewport that big.
    fn measure(
        &self,
        ctx: &crate::primitive::LayoutCtx,
        avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        let bounds = crate::primitive::Primitive::layout(
            self,
            ctx,
            Rect::new(0.0, 0.0, avail.width, avail.height),
        )
        .layout
        .bounds;
        crate::NaturalSize::new(Some(bounds.width), Some(bounds.height))
    }

    /// `rect` is the viewport the modal centres itself in — the same
    /// argument [`Dialog::layout`] takes.
    ///
    /// On a cell grid the box is measured by [`cell_measure`]; on a pixel
    /// surface by [`Dialog::measure_generic_measured_with_style`], so
    /// button widths come from the calling backend's real
    /// [`crate::Backend::measure_text`] and the button row is
    /// [`Style::control_height`] tall.
    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> DialogPrimitiveLayout {
        let layout = match ctx.units() {
            crate::primitive::Units::Cells => cell_layout(self, rect),
            _ => {
                let measure = crate::primitives::layout_metrics::LayoutCtxMeasure {
                    ctx,
                    role: crate::FontRole::Chrome,
                };
                let dialog_measure = self.measure_generic_measured_with_style(
                    ctx.char_width,
                    ctx.line_height,
                    rect,
                    0.0,
                    &measure,
                    ctx.style(),
                );
                Dialog::layout(self, rect, dialog_measure, |btn| {
                    ToolbarItemMeasure::new(crate::primitives::toolbar::measure_button(
                        &measure, btn,
                    ))
                })
            }
        };
        DialogPrimitiveLayout {
            area: rect,
            layout,
            nerd_fonts: ctx.nerd_fonts_enabled,
        }
    }

    /// The shared pixel painter, with the surface's own line height as
    /// the row pitch — the same metric every pixel backend's
    /// `draw_dialog` passes it. A [`DialogInput::Toolbar`] body slot is
    /// not painted here; see `native_surface_paint`'s own module doc.
    fn paint(
        &self,
        layout: &DialogPrimitiveLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        theme: &crate::theme::Theme,
        style: &Style,
    ) {
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            let line_height = surface.surface_line_height();
            native_surface_paint::paint(self, &layout.layout, line_height, surface, theme, style);
        }
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        {
            let _ = (layout, surface, theme, style);
        }
    }

    /// [`cell_paint::paint`] over `buf`, reached through
    /// [`crate::tui::draw_dialog_with_nerd_fonts`] because the `Buffer` →
    /// [`crate::CellSurface`] adapter is private to `crate::tui`, and
    /// carrying [`DialogPrimitiveLayout::nerd_fonts`] so a
    /// [`crate::ButtonSpec::icon`] resolves the same glyph the legacy
    /// path picks.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &DialogPrimitiveLayout,
        buf: &mut ratatui::buffer::Buffer,
        theme: &crate::theme::Theme,
    ) {
        crate::tui::draw_dialog_with_nerd_fonts(
            buf,
            self,
            &layout.layout,
            theme,
            layout.nerd_fonts,
        );
    }
}

impl Dialog {
    /// Move keyboard focus `delta` buttons along the row, wrapping at
    /// either end, and tell the host which stop now holds it.
    fn cycle_focus(
        &self,
        state: &mut DialogState,
        delta: isize,
        cx: &mut crate::interactive::EventCx,
    ) -> Handled<DialogAction> {
        let key = if delta < 0 {
            crate::behaviour::nav::NavKey::Up
        } else {
            crate::behaviour::nav::NavKey::Down
        };
        match crate::behaviour::nav::FlatNav::mv(
            self.buttons.len(),
            state.focused_button,
            key,
            true,
            |_| true,
        ) {
            Some(idx) => {
                self.focus_button(state, idx, cx);
                Handled::Consumed
            }
            None => Handled::Ignored,
        }
    }

    /// The button Enter activates: the keyboard-focused one, else
    /// [`Self::default_button_id`].
    fn activation_target(&self, state: &DialogState) -> Option<WidgetId> {
        state
            .focused_button
            .and_then(|i| self.buttons.get(i))
            .map(|b| b.id)
            .or_else(|| self.default_button_id().copied())
    }

    /// Index of the button [`Self::default_button_id`] names.
    fn default_button_index(&self) -> Option<usize> {
        self.buttons
            .iter()
            .position(|b| b.is_default)
            .or_else(|| self.buttons.len().checked_sub(1))
    }

    /// Park keyboard focus on `idx`, both in this widget's own state and
    /// in the host's focus tree.
    ///
    /// The host addresses a [`crate::interactive::FocusSpec::Children`]
    /// widget by `(id, sub)`, so a focus request must name the stop —
    /// the engine's own click-to-focus step can only express `(id,
    /// None)`, which is not one of this modal's stops and would be
    /// dropped on the next focus-tree resync.
    fn focus_button(
        &self,
        state: &mut DialogState,
        idx: usize,
        cx: &mut crate::interactive::EventCx,
    ) {
        state.focused_button = Some(idx);
        cx.request_focus(Some(crate::interactive::SubTarget(idx as u64)));
    }
}

impl crate::interactive::Interactive for Dialog {
    type State = DialogState;
    type Action = DialogAction;

    /// One focus stop per button, so Tab/Shift-Tab walk the modal's own
    /// button row — the host's tab-order walk sees the real stop count
    /// without knowing anything about the modal's internals.
    /// [`crate::interactive::FocusSpec::None`] for a button-less dialog:
    /// there is nothing for keyboard focus to land on.
    fn focus(&self) -> crate::interactive::FocusSpec {
        if self.buttons.is_empty() {
            crate::interactive::FocusSpec::None
        } else {
            crate::interactive::FocusSpec::Children(
                (0..self.buttons.len())
                    .map(|i| crate::interactive::FocusStop {
                        sub: crate::interactive::SubTarget(i as u64),
                    })
                    .collect(),
            )
        }
    }

    /// Every button's own rect, most-specific first, then the modal box
    /// itself — translated out of [`DialogLayout`]'s absolute frame into
    /// the widget-local space this method is contracted to answer in.
    fn sub_regions(
        &self,
        layout: &DialogPrimitiveLayout,
    ) -> Vec<(Rect, crate::interactive::SubTarget)> {
        let (ox, oy) = (layout.area.x, layout.area.y);
        let local = |r: Rect| Rect::new(r.x - ox, r.y - oy, r.width, r.height);
        layout
            .layout
            .visible_buttons
            .iter()
            .map(|vis| {
                (
                    local(vis.bounds),
                    crate::interactive::SubTarget(vis.button_idx as u64),
                )
            })
            .collect()
    }

    /// `Click` activates whichever button Enter would — see
    /// [`Dialog::activation_target`].
    fn a11y_actions(&self, _state: &DialogState) -> Vec<crate::a11y::A11yAction> {
        if self.buttons.is_empty() {
            Vec::new()
        } else {
            vec![crate::a11y::A11yAction::Click]
        }
    }

    /// Tab/Shift-Tab cycle the button row, Enter activates the focused
    /// button, Escape cancels, and a left click activates whatever it
    /// lands on. A click on the modal box itself is
    /// [`Handled::Consumed`] rather than ignored — a modal intercepts
    /// clicks that reach it instead of letting them fall through to the
    /// app behind it; a click outside the box is left for the host's own
    /// dismiss-or-swallow policy.
    fn handle(
        &self,
        state: &mut DialogState,
        event: &crate::interactive::RoutedEvent,
        layout: &DialogPrimitiveLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> Handled<DialogAction> {
        use crate::interactive::{KeyEvent, PointerKind, RoutedEvent};

        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::event::MouseButton::Left),
                local,
                ..
            } => {
                let (x, y) = (layout.area.x + local.x, layout.area.y + local.y);
                match layout.hit_test(x, y) {
                    DialogHit::Button(id) => {
                        if let Some(idx) = self.buttons.iter().position(|b| b.id == id) {
                            self.focus_button(state, idx, cx);
                        }
                        Handled::Event(DialogAction::ButtonActivated(id))
                    }
                    DialogHit::BodyToolbarButton(button_id) => {
                        match &self.input {
                            Some(DialogInput::Toolbar(toolbar)) => match toolbar.click(button_id) {
                                Some(event) => Handled::Event(DialogAction::BodyToolbar {
                                    toolbar: toolbar.id,
                                    event,
                                }),
                                // A disabled action resolves to no
                                // toolbar event at all, but the click
                                // still landed inside the modal.
                                None => Handled::Consumed,
                            },
                            _ => Handled::Consumed,
                        }
                    }
                    DialogHit::Body => {
                        // Clicking the box gives the modal's button row
                        // keyboard focus, seeded on the default button,
                        // so Tab and Enter work from that click onward
                        // without the host wiring focus itself.
                        if let Some(idx) =
                            state.focused_button.or_else(|| self.default_button_index())
                        {
                            self.focus_button(state, idx, cx);
                        }
                        Handled::Consumed
                    }
                    DialogHit::Outside => Handled::Ignored,
                }
            }
            RoutedEvent::Key(KeyEvent { key, modifiers, .. }) => match key {
                Key::Named(crate::event::NamedKey::Tab) if modifiers.shift => {
                    self.cycle_focus(state, -1, cx)
                }
                Key::Named(crate::event::NamedKey::BackTab) => self.cycle_focus(state, -1, cx),
                Key::Named(crate::event::NamedKey::Tab) => self.cycle_focus(state, 1, cx),
                Key::Named(crate::event::NamedKey::Enter) => match self.activation_target(state) {
                    Some(id) => Handled::Event(DialogAction::ButtonActivated(id)),
                    None => Handled::Ignored,
                },
                Key::Named(crate::event::NamedKey::Escape) => {
                    Handled::Event(DialogAction::Cancelled {
                        button: self.cancel_button_id().copied(),
                    })
                }
                _ => Handled::Ignored,
            },
            RoutedEvent::Action(crate::a11y::A11yAction::Click) => {
                match self.activation_target(state) {
                    Some(id) => Handled::Event(DialogAction::ButtonActivated(id)),
                    None => Handled::Ignored,
                }
            }
            RoutedEvent::FocusIn(Some(sub)) => {
                let idx = sub.0 as usize;
                if idx < self.buttons.len() {
                    state.focused_button = Some(idx);
                    Handled::Consumed
                } else {
                    Handled::Ignored
                }
            }
            _ => Handled::Ignored,
        }
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
            spec: None,
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
                    spec: None,
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
                    spec: None,
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
            ..Default::default()
        });
        let json = serde_json::to_string(&input).unwrap();
        let back: DialogInput = serde_json::from_str(&json).unwrap();
        assert_eq!(input, back);
    }

    // ── Toolbar variant creates body_toolbar_layout ───────────────────────

    #[test]
    fn layout_toolbar_variant_sets_body_toolbar_layout() {
        let d = base_dialog(Some(DialogInput::Toolbar(Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![ToolbarButton::Action {
                spec: None,
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
            ..Default::default()
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
                spec: None,
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
            ..Default::default()
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

    // ── native_dialog_options ─────────────────────────────────────────────

    fn dialog_with_buttons() -> Dialog {
        Dialog {
            id: WidgetId::new("d"),
            title: crate::types::StyledText::plain("Unsaved Changes"),
            body: vec![
                crate::types::StyledText::plain("Do you want to save?"),
                crate::types::StyledText::plain("Changes will be lost."),
            ],
            buttons: vec![
                DialogButton {
                    spec: None,
                    id: WidgetId::new("save"),
                    label: "Save".into(),
                    is_default: true,
                    is_cancel: false,
                    tint: None,
                },
                DialogButton {
                    spec: None,
                    id: WidgetId::new("cancel"),
                    label: "Cancel".into(),
                    is_default: false,
                    is_cancel: true,
                    tint: None,
                },
            ],
            severity: Some(DialogSeverity::Warning),
            vertical_buttons: false,
            table: None,
            input: None,
        }
    }

    #[test]
    fn native_dialog_options_maps_title_body_buttons_default_cancel_severity() {
        let d = dialog_with_buttons();
        let opts =
            native_dialog_options(&d).expect("no table/input — must be natively expressible");
        assert_eq!(opts.title, "Unsaved Changes");
        assert_eq!(opts.body, "Do you want to save?\nChanges will be lost.");
        assert_eq!(opts.buttons.len(), 2);
        assert_eq!(opts.buttons[0].id.as_str(), "save");
        assert_eq!(opts.buttons[0].label, "Save");
        assert!(opts.buttons[0].is_default);
        assert!(!opts.buttons[0].is_cancel);
        assert_eq!(opts.buttons[1].id.as_str(), "cancel");
        assert_eq!(opts.buttons[1].label, "Cancel");
        assert!(opts.buttons[1].is_cancel);
        assert!(!opts.buttons[1].is_default);
        assert_eq!(opts.severity, Some(DialogSeverity::Warning));
    }

    #[test]
    fn native_dialog_options_passes_through_none_severity() {
        let mut d = dialog_with_buttons();
        d.severity = None;
        let opts =
            native_dialog_options(&d).expect("no table/input — must be natively expressible");
        assert_eq!(opts.severity, None);
    }

    /// `DialogButton::spec` carries through to [`MessageDialogButton::spec`]
    /// unchanged — unlike `tint`, which has no native-alert equivalent and
    /// is dropped (see this function's own doc).
    #[test]
    fn native_dialog_options_carries_button_spec_through() {
        let mut d = dialog_with_buttons();
        let mut spec = crate::primitives::button::ButtonSpec::new("Save");
        spec.variant = crate::primitives::button::ButtonVariant::Danger;
        d.buttons[0].spec = Some(spec.clone());
        let opts =
            native_dialog_options(&d).expect("no table/input — must be natively expressible");
        assert_eq!(opts.buttons[0].spec, Some(spec));
        assert_eq!(opts.buttons[1].spec, None);
    }

    #[test]
    fn native_dialog_options_none_for_table() {
        let d = table_dialog();
        assert!(
            native_dialog_options(&d).is_none(),
            "no native alert facility hosts a DialogTable"
        );
    }

    #[test]
    fn native_dialog_options_none_for_input() {
        let d = base_dialog(Some(DialogInput::TextInput(DialogTextInput {
            value: "x".into(),
            placeholder: String::new(),
            cursor: None,
        })));
        assert!(
            native_dialog_options(&d).is_none(),
            "no native alert facility hosts a DialogInput text field"
        );
    }

    // ── Dialog primitive tests ────────────────────────────────────────

    fn mk_dialog_button(id: &str, label: &str) -> DialogButton {
        DialogButton {
            spec: None,
            id: WidgetId::new(id),
            label: label.to_string(),
            is_default: false,
            is_cancel: false,
            tint: None,
        }
    }

    #[test]
    fn dialog_layout_centered_horizontal_buttons() {
        let mut ok = mk_dialog_button("ok", "OK");
        ok.is_default = true;
        let mut cancel = mk_dialog_button("cancel", "Cancel");
        cancel.is_cancel = true;
        let d = Dialog {
            id: WidgetId::new("confirm"),
            title: StyledText::plain("Close unsaved file?"),
            body: vec![StyledText::plain("You have unsaved changes. Close anyway?")],
            buttons: vec![cancel, ok],
            severity: Some(DialogSeverity::Question),
            vertical_buttons: false,
            table: None,
            input: None,
        };
        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
        let measure = DialogMeasure {
            width: 400.0,
            title_height: 24.0,
            body_height: 40.0,
            table_height: 0.0,
            input_height: 0.0,
            button_row_height: 32.0,
            button_width: 80.0,
            button_gap: 8.0,
            padding: 16.0,
        };
        let layout = d.layout(viewport, measure, |_| {
            crate::primitives::toolbar::ToolbarItemMeasure::new(0.0)
        });

        // Centered: x = (800 - 400)/2 = 200
        assert_eq!(layout.bounds.x, 200.0);
        // total_h = 24 + 40 + 32 + 32 = 128; y = (600 - 128)/2 = 236
        assert_eq!(layout.bounds.y, 236.0);

        assert!(layout.title_bounds.is_some());
        assert_eq!(layout.visible_buttons.len(), 2);
        // Right-aligned: last button's right edge = content right edge.
        let last = &layout.visible_buttons[1];
        assert_eq!(
            last.bounds.x + last.bounds.width,
            layout.bounds.x + measure.width - measure.padding
        );
    }

    #[test]
    fn dialog_layout_vertical_buttons() {
        let d = Dialog {
            id: WidgetId::new("code-actions"),
            title: StyledText::plain("Code actions"),
            body: vec![StyledText::default()],
            buttons: vec![
                mk_dialog_button("a1", "Add import"),
                mk_dialog_button("a2", "Use fully qualified path"),
                mk_dialog_button("a3", "Ignore"),
            ],
            severity: None,
            vertical_buttons: true,
            table: None,
            input: None,
        };
        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
        let measure = DialogMeasure {
            width: 300.0,
            title_height: 20.0,
            body_height: 0.0,
            table_height: 0.0,
            input_height: 0.0,
            button_row_height: 90.0,
            button_width: 280.0,
            button_gap: 0.0,
            padding: 10.0,
        };
        let layout = d.layout(viewport, measure, |_| {
            crate::primitives::toolbar::ToolbarItemMeasure::new(0.0)
        });
        assert_eq!(layout.visible_buttons.len(), 3);
        // Stacked: each one full button row tall (button_row_height = 90);
        // widths equal content_w = 300-20 = 280.
        for b in &layout.visible_buttons {
            assert_eq!(b.bounds.width, 280.0);
            assert_eq!(b.bounds.height, 90.0);
        }
        // Button 0 y < button 1 y < button 2 y.
        assert!(layout.visible_buttons[0].bounds.y < layout.visible_buttons[1].bounds.y);
    }

    #[test]
    fn dialog_hit_test_on_button_returns_id() {
        let d = Dialog {
            id: WidgetId::new("confirm"),
            title: StyledText::plain("T"),
            body: vec![StyledText::plain("B")],
            buttons: vec![
                mk_dialog_button("cancel", "Cancel"),
                mk_dialog_button("ok", "OK"),
            ],
            severity: None,
            vertical_buttons: false,
            table: None,
            input: None,
        };
        let viewport = Rect::new(0.0, 0.0, 400.0, 300.0);
        let measure = DialogMeasure {
            width: 200.0,
            title_height: 20.0,
            body_height: 20.0,
            table_height: 0.0,
            input_height: 0.0,
            button_row_height: 20.0,
            button_width: 60.0,
            button_gap: 10.0,
            padding: 10.0,
        };
        let layout = d.layout(viewport, measure, |_| {
            crate::primitives::toolbar::ToolbarItemMeasure::new(0.0)
        });
        // Click on the second (right) button — OK.
        let ok_btn = &layout.visible_buttons[1];
        let cx = ok_btn.bounds.x + ok_btn.bounds.width / 2.0;
        let cy = ok_btn.bounds.y + ok_btn.bounds.height / 2.0;
        match layout.hit_test(cx, cy) {
            DialogHit::Button(id) => assert_eq!(id.as_str(), "ok"),
            _ => panic!(),
        }
        // Click on body (not on button) → Body.
        assert_eq!(
            layout.hit_test(layout.body_bounds.x + 5.0, layout.body_bounds.y + 5.0),
            DialogHit::Body
        );
        // Click outside dialog → Outside.
        assert_eq!(layout.hit_test(0.0, 0.0), DialogHit::Outside);
    }

    #[test]
    fn dialog_with_input_field_layout() {
        // Rename-style dialog: title + short body + input + buttons.
        let ok = mk_dialog_button("ok", "OK");
        let cancel = mk_dialog_button("cancel", "Cancel");
        let d = Dialog {
            id: WidgetId::new("rename"),
            title: StyledText::plain("Rename"),
            body: vec![StyledText::plain("New name:")],
            buttons: vec![cancel, ok],
            severity: None,
            vertical_buttons: false,
            table: None,
            input: Some(DialogInput::TextInput(DialogTextInput {
                value: "old_name.rs".to_string(),
                placeholder: String::new(),
                cursor: Some(11),
            })),
        };
        let viewport = Rect::new(0.0, 0.0, 400.0, 300.0);
        let measure = DialogMeasure {
            width: 200.0,
            title_height: 20.0,
            body_height: 20.0,
            table_height: 0.0,
            input_height: 24.0,
            button_row_height: 20.0,
            button_width: 60.0,
            button_gap: 10.0,
            padding: 10.0,
        };
        let layout = d.layout(viewport, measure, |_| {
            crate::primitives::toolbar::ToolbarItemMeasure::new(0.0)
        });
        assert!(layout.input_bounds.is_some());
        let ib = layout.input_bounds.unwrap();
        // Input sits between body and buttons.
        assert_eq!(ib.y, layout.body_bounds.y + layout.body_bounds.height);
        assert_eq!(ib.height, 24.0);
        // Buttons pushed down by input_height.
        assert_eq!(layout.button_row_bounds.y, ib.y + ib.height);
    }

    #[test]
    fn dialog_default_and_cancel_resolution() {
        let mut ok = mk_dialog_button("ok", "OK");
        ok.is_default = true;
        let mut cancel = mk_dialog_button("cancel", "Cancel");
        cancel.is_cancel = true;
        let d = Dialog {
            id: WidgetId::new("d"),
            title: StyledText::default(),
            body: vec![StyledText::default()],
            buttons: vec![cancel, ok],
            severity: None,
            vertical_buttons: false,
            table: None,
            input: None,
        };
        assert_eq!(d.default_button_id().unwrap().as_str(), "ok");
        assert_eq!(d.cancel_button_id().unwrap().as_str(), "cancel");
    }

    // ── Dialog::measure_generic ───────────────────────────────────────────

    #[test]
    fn measure_generic_tui_shaped_matches_hand_rolled_formula() {
        // char_width == line_height == 1.0, mirroring TUI metrics —
        // reproduces the formula every caller currently hand-rolls.
        let d = table_dialog();
        let viewport = Rect::new(0.0, 0.0, 100.0, 30.0);
        let m = d.measure_generic(1.0, 1.0, viewport, 0.0);

        assert_eq!(m.title_height, 1.0, "non-empty title reserves one line");
        assert_eq!(m.body_height, 0.0, "table_dialog() has no body lines");
        let table = d.table.as_ref().unwrap();
        assert_eq!(m.table_height, table.tui_total_height() as f32);
        assert_eq!(m.button_row_height, 1.0);
        // `btn("close")`'s label is 5 chars; `ButtonSpec::min_cell_width`'s
        // `4 cells of chrome + label width` (9 cells) exceeds the flat
        // 8-char-width floor, so it — not the floor — wins here.
        assert_eq!(m.button_width, 9.0);
        assert_eq!(m.button_gap, 2.0);
        assert_eq!(m.padding, 1.0);
        assert_eq!(m.input_height, 0.0, "no DialogInput on table_dialog()");
    }

    #[test]
    fn measure_generic_empty_title_reserves_no_height() {
        let mut d = base_dialog(None);
        d.title = StyledText::default();
        let viewport = Rect::new(0.0, 0.0, 100.0, 30.0);
        let m = d.measure_generic(1.0, 1.0, viewport, 0.0);
        assert_eq!(m.title_height, 0.0);
    }

    #[test]
    fn measure_generic_input_present_reserves_one_line() {
        let d = base_dialog(Some(DialogInput::TextInput(DialogTextInput {
            value: "x".into(),
            placeholder: String::new(),
            cursor: None,
        })));
        let viewport = Rect::new(0.0, 0.0, 100.0, 30.0);
        let m = d.measure_generic(1.0, 1.0, viewport, 0.0);
        assert_eq!(m.input_height, 1.0);
    }

    #[test]
    fn measure_generic_widens_for_wide_table() {
        // A table wide enough to force the dialog beyond its default
        // preferred width.
        let wide_table_dialog = Dialog {
            id: WidgetId::new("wide"),
            title: StyledText::plain("Wide"),
            body: vec![],
            buttons: vec![btn("close")],
            severity: None,
            vertical_buttons: false,
            table: Some(DialogTable {
                headers: None,
                rows: vec![vec!["x".repeat(80)]],
                column_widths: None,
            }),
            input: None,
        };
        let viewport = Rect::new(0.0, 0.0, 200.0, 50.0);
        let m = wide_table_dialog.measure_generic(1.0, 1.0, viewport, 0.0);
        // Table content alone needs 80 + 2 padding chars = 82, well past
        // the ≈30-60 char default preferred width.
        assert!(
            m.width >= 82.0,
            "width {} should fit the wide table",
            m.width
        );
    }

    #[test]
    fn measure_generic_border_chrome_inset_shrinks_width_when_viewport_bound() {
        // Narrow viewport so the `viewport.width - 4*char_width -
        // 2*inset` clamp is the binding constraint, not the table or
        // the 30-60 char-width default band.
        let d = base_dialog(None);
        let viewport = Rect::new(0.0, 0.0, 20.0, 30.0);
        let no_inset = d.measure_generic(1.0, 1.0, viewport, 0.0);
        let with_inset = d.measure_generic(1.0, 1.0, viewport, 2.0);
        assert_eq!(
            with_inset.width,
            no_inset.width - 4.0,
            "border_chrome_inset shrinks width by 2x the inset"
        );
    }

    // ── Dialog::measure_generic_with_style ─────────────────────────────────

    /// On a pixel backend (`line_height > 1.0`), `measure_generic_with_style`
    /// sizes the button row at `Style::control_height` — matching
    /// `MessageDialogController::layout`'s own pixel-path button row — not
    /// the raw chrome line height `measure_generic` uses.
    #[test]
    fn measure_generic_with_style_sizes_button_row_at_control_height() {
        let d = base_dialog(None);
        let viewport = Rect::new(0.0, 0.0, 400.0, 300.0);
        let style = Style::default().with_control_height(26.0);
        let line_height = 16.0; // a pixel backend's real chrome row height

        let unstyled = d.measure_generic(8.0, line_height, viewport, 0.0);
        let styled = d.measure_generic_with_style(8.0, line_height, viewport, 0.0, &style);

        assert_eq!(
            unstyled.button_row_height, line_height,
            "measure_generic is unaffected by this test's own Style"
        );
        assert_eq!(
            styled.button_row_height, 26.0,
            "measure_generic_with_style sizes the button row at \
             Style::control_height on a pixel backend"
        );
    }

    /// On TUI (`line_height == 1.0`, the char-cell approximation), a
    /// `Style` must not inflate the button row past one cell — the
    /// row stays one char cell tall, same as `measure_generic`.
    #[test]
    fn measure_generic_with_style_leaves_tui_char_grid_unchanged() {
        let d = base_dialog(None);
        let viewport = Rect::new(0.0, 0.0, 80.0, 30.0);
        let style = Style::default().with_control_height(26.0);

        let styled = d.measure_generic_with_style(1.0, 1.0, viewport, 0.0, &style);
        assert_eq!(
            styled.button_row_height, 1.0,
            "TUI's one-row-per-button char-grid geometry is unchanged by Style"
        );
    }

    // ── Dialog::measure_generic_measured ──────────────────────────────────

    /// Fixed-width stand-in for a real font: `6.0` px per char. Mirrors
    /// `layout_metrics`'s `FakeMeasure`, giving this a platform-independent
    /// regression test instead of depending on a real backend's glyph
    /// metrics.
    struct FakeMeasure;
    impl crate::primitives::layout_metrics::TextMeasure for FakeMeasure {
        fn width_of(&self, text: &str) -> f32 {
            text.chars().count() as f32 * 6.0
        }
    }

    #[test]
    fn measure_generic_measured_uses_real_label_width_not_the_estimate() {
        let mut d = base_dialog(None);
        d.buttons = vec![btn("a-very-long-button-label")];
        let viewport = Rect::new(0.0, 0.0, 200.0, 30.0);
        let char_width = 1.0;
        let line_height = 1.0;

        let estimated = d.measure_generic(char_width, line_height, viewport, 0.0);
        let measured =
            d.measure_generic_measured(char_width, line_height, viewport, 0.0, &FakeMeasure);

        // With no real text measurer, `measure_generic` falls back to
        // `ButtonSpec::min_cell_width`'s cell-count estimate, which for
        // this long label already exceeds the flat 8-char-width floor.
        let spec_w = d.buttons[0].resolved_button_spec().min_cell_width() * char_width;
        assert_eq!(
            estimated.button_width, spec_w,
            "measure_generic falls back to the spec-derived estimate"
        );
        let label_w = FakeMeasure.width_of("a-very-long-button-label");
        assert_eq!(
            measured.button_width,
            label_w + char_width * 2.0,
            "measured button_width tracks the real label width + padding, \
             not the flat estimate"
        );
        assert!(measured.button_width > estimated.button_width);
    }

    #[test]
    fn measure_generic_measured_floors_at_the_char_width_estimate() {
        // Short labels must not shrink the button row below what
        // measure_generic already renders for every existing caller.
        struct NarrowMeasure;
        impl crate::primitives::layout_metrics::TextMeasure for NarrowMeasure {
            fn width_of(&self, text: &str) -> f32 {
                text.chars().count() as f32 * 1.0
            }
        }

        let d = base_dialog(None); // buttons: [btn("ok")]
        let viewport = Rect::new(0.0, 0.0, 200.0, 30.0);
        let char_width = 1.0;
        let line_height = 1.0;

        let measured =
            d.measure_generic_measured(char_width, line_height, viewport, 0.0, &NarrowMeasure);
        assert_eq!(measured.button_width, char_width * 8.0);
    }

    #[test]
    fn measure_generic_measured_matches_estimate_for_other_fields() {
        let d = table_dialog();
        let viewport = Rect::new(0.0, 0.0, 100.0, 30.0);
        let estimated = d.measure_generic(1.0, 1.0, viewport, 0.0);
        let measured = d.measure_generic_measured(1.0, 1.0, viewport, 0.0, &FakeMeasure);

        assert_eq!(measured.width, estimated.width);
        assert_eq!(measured.title_height, estimated.title_height);
        assert_eq!(measured.body_height, estimated.body_height);
        assert_eq!(measured.table_height, estimated.table_height);
        assert_eq!(measured.input_height, estimated.input_height);
        assert_eq!(measured.button_row_height, estimated.button_row_height);
        assert_eq!(measured.button_gap, estimated.button_gap);
        assert_eq!(measured.padding, estimated.padding);
    }

    /// `measure_generic_measured_with_style` combines both seams: the real
    /// label width drives `button_width` (same as `measure_generic_measured`)
    /// and `Style::control_height` drives `button_row_height` on a pixel
    /// backend (same as `measure_generic_with_style`).
    #[test]
    fn measure_generic_measured_with_style_combines_both_seams() {
        let mut d = base_dialog(None);
        d.buttons = vec![btn("a-very-long-button-label")];
        let viewport = Rect::new(0.0, 0.0, 400.0, 300.0);
        let style = Style::default().with_control_height(26.0);
        let line_height = 16.0;

        let combined = d.measure_generic_measured_with_style(
            8.0,
            line_height,
            viewport,
            0.0,
            &FakeMeasure,
            &style,
        );
        let measured_only =
            d.measure_generic_measured(8.0, line_height, viewport, 0.0, &FakeMeasure);

        assert_eq!(
            combined.button_row_height, 26.0,
            "button row height comes from Style::control_height"
        );
        assert_eq!(
            combined.button_width, measured_only.button_width,
            "button width still tracks the real label width, unaffected by style"
        );
    }

    // ── B3b: ButtonSpec seam ─────────────────────────────────────────────

    #[test]
    fn default_button_without_spec_derives_a_primary_spec() {
        let btn = DialogButton {
            id: WidgetId::new("dialog:ok"),
            label: "OK".to_string(),
            is_default: true,
            is_cancel: false,
            tint: None,
            spec: None,
        };
        let spec = btn.resolved_button_spec();
        assert_eq!(spec.label, "OK");
        assert_eq!(
            spec.variant,
            crate::primitives::button::ButtonVariant::Primary
        );
    }

    #[test]
    fn non_default_button_without_spec_derives_a_secondary_spec() {
        let btn = DialogButton {
            id: WidgetId::new("dialog:cancel"),
            label: "Cancel".to_string(),
            is_default: false,
            is_cancel: true,
            tint: None,
            spec: None,
        };
        let spec = btn.resolved_button_spec();
        assert_eq!(
            spec.variant,
            crate::primitives::button::ButtonVariant::Secondary
        );
    }

    /// A destructive dialog button carries an explicit `Danger` spec,
    /// which wins outright over the legacy `is_default`-derived variant.
    #[test]
    fn button_with_explicit_danger_spec_overrides_the_legacy_fields() {
        let mut explicit = crate::primitives::button::ButtonSpec::new("Move to Trash");
        explicit.variant = crate::primitives::button::ButtonVariant::Danger;
        explicit.icon = Some(crate::types::Icon::new("\u{f071}", "!"));
        let btn = DialogButton {
            id: WidgetId::new("dialog:delete"),
            label: "Delete".to_string(),
            is_default: true,
            is_cancel: false,
            tint: None,
            spec: Some(explicit.clone()),
        };

        assert_eq!(btn.resolved_button_spec(), explicit);
    }

    // ── DialogInput::Form ──────────────────────────────────────────────

    fn two_field_form() -> crate::primitives::form::Form {
        use crate::primitives::form::{FieldKind, FormField};
        crate::primitives::form::Form {
            id: WidgetId::new("form:create_milestone"),
            fields: vec![
                FormField {
                    id: WidgetId::new("field:name"),
                    label: StyledText::plain("Name"),
                    kind: FieldKind::TextInput {
                        value: String::new(),
                        placeholder: String::new(),
                        cursor: None,
                        selection_anchor: None,
                    },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                    label_position: None,
                    label_align: None,
                    label_width: None,
                    tab_index: None,
                    control_width: None,
                    min_control_width: None,
                    row_align: None,
                },
                FormField {
                    id: WidgetId::new("field:description"),
                    label: StyledText::plain("Description"),
                    kind: FieldKind::TextArea {
                        value: String::new(),
                        placeholder: String::new(),
                        cursor: None,
                        visible_rows: 3,
                    },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                    label_position: None,
                    label_align: None,
                    label_width: None,
                    tab_index: None,
                    control_width: None,
                    min_control_width: None,
                    row_align: None,
                },
            ],
            focused_field: Some(WidgetId::new("field:name")),
            scroll_offset: 0,
            has_focus: true,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        }
    }

    #[test]
    fn serde_dialog_form_round_trip() {
        let input = DialogInput::Form(two_field_form());
        let json = serde_json::to_string(&input).unwrap();
        let back: DialogInput = serde_json::from_str(&json).unwrap();
        assert_eq!(input, back);
    }

    /// A `Form` input reserves one row per field, not a single row like
    /// every other `DialogInput` variant.
    #[test]
    fn measure_generic_sizes_input_height_per_form_field() {
        let d = base_dialog(Some(DialogInput::Form(two_field_form())));
        let measure = d.measure_generic(1.0, 1.0, viewport(), 0.0);
        assert_eq!(measure.input_height, 2.0);
    }

    /// `layout` reserves `input_bounds` tall enough for every field when
    /// fed a `Form`-sized `DialogMeasure::input_height`, and does not set
    /// `body_toolbar_layout` (that's only for `DialogInput::Toolbar`).
    #[test]
    fn layout_form_variant_reserves_input_bounds_for_every_field() {
        let d = base_dialog(Some(DialogInput::Form(two_field_form())));
        let measure = DialogMeasure {
            input_height: 2.0,
            ..measure_no_input()
        };
        let layout = d.layout(viewport(), measure, no_measure);
        let input_bounds = layout.input_bounds.expect("input bounds present");
        assert_eq!(input_bounds.height, 2.0);
        assert!(
            layout.body_toolbar_layout.is_none(),
            "body_toolbar_layout should be None for Form variant"
        );
    }
}

/// The cell painter's own paint/click round-trip coverage, driven
/// through `crate::tui::draw_dialog` — the delegate that wraps a ratatui
/// `Buffer` in the `CellSurface` [`cell_paint::paint`] takes.
#[cfg(all(test, feature = "tui"))]
mod cell_paint_tests {
    use super::{
        Dialog, DialogButton, DialogHit, DialogInput, DialogLayout, DialogMeasure, DialogTextInput,
    };
    use crate::primitives::toolbar::{Toolbar, ToolbarButton, ToolbarItemMeasure};
    use crate::theme::Theme;
    use crate::tui::{draw_dialog, ratatui_color, tui_dialog_layout};
    use crate::types::{StyledSpan, StyledText, WidgetId};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    fn make_dialog() -> Dialog {
        Dialog {
            id: WidgetId::new("d"),
            title: StyledText {
                spans: vec![StyledSpan::plain("Confirm")],
            },
            body: vec![StyledText {
                spans: vec![StyledSpan::plain("Save before quitting?")],
            }],
            buttons: vec![
                DialogButton {
                    spec: None,
                    id: WidgetId::new("save"),
                    label: "Save".into(),
                    is_default: true,
                    is_cancel: false,
                    tint: None,
                },
                DialogButton {
                    spec: None,
                    id: WidgetId::new("cancel"),
                    label: "Cancel".into(),
                    is_default: false,
                    is_cancel: true,
                    tint: None,
                },
            ],
            severity: None,
            vertical_buttons: false,
            table: None,
            input: None,
        }
    }

    fn make_layout(dialog: &Dialog) -> DialogLayout {
        let measure = DialogMeasure {
            width: 40.0,
            title_height: 1.0,
            body_height: 2.0,
            table_height: 0.0,
            input_height: 0.0,
            button_row_height: 1.0,
            button_width: 8.0,
            button_gap: 2.0,
            padding: 1.0,
        };
        let viewport = crate::event::Rect::new(0.0, 0.0, 80.0, 30.0);
        dialog.layout(viewport, measure, |_| ToolbarItemMeasure::new(0.0))
    }

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    #[test]
    fn paints_corner_glyphs_and_title() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 30));
        let d = make_dialog();
        let layout = make_layout(&d);
        draw_dialog(&mut buf, &d, &layout, &Theme::default());

        let bx = layout.bounds.x.round() as u16;
        let by = layout.bounds.y.round() as u16;
        let bw = layout.bounds.width.round() as u16;
        let bh = layout.bounds.height.round() as u16;

        assert_eq!(cell_char(&buf, bx, by), '╭');
        assert_eq!(cell_char(&buf, bx + bw - 1, by), '╮');
        assert_eq!(cell_char(&buf, bx, by + bh - 1), '╰');
        assert_eq!(cell_char(&buf, bx + bw - 1, by + bh - 1), '╯');
    }

    #[test]
    fn default_button_has_selected_bg() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 30));
        let d = make_dialog();
        let layout = make_layout(&d);
        let theme = Theme {
            selected_bg: crate::types::Color::rgb(99, 0, 0),
            ..Theme::default()
        };
        draw_dialog(&mut buf, &d, &layout, &theme);

        // The first visible button is "Save" (is_default).
        let vis = &layout.visible_buttons[0];
        let bx = vis.bounds.x.round() as u16;
        let by = vis.bounds.y.round() as u16;
        let bg = buf[(bx, by)].bg;
        assert_eq!(bg, ratatui::style::Color::Rgb(99, 0, 0));
    }

    fn cell_fg(buf: &Buffer, x: u16, y: u16) -> ratatui::style::Color {
        buf[(x, y)]
            .style()
            .fg
            .unwrap_or(ratatui::style::Color::Reset)
    }

    /// A `DialogButton` with an explicit `Danger` spec reads as
    /// `[ ⚠ Move to Trash ]` in `theme.error_fg`, through the same
    /// bracket-chrome path `crate::tui::button` paints a standalone
    /// `Button` with — not the dialog's own bare highlighted-row look.
    ///
    /// Goes through the real `tui_dialog_layout` (not a hand-built
    /// `DialogMeasure`), so the button box it hands `draw_dialog` is
    /// exactly what every production TUI caller would compute — this
    /// is the regression guard for `tui_dialog_layout` sizing its
    /// button row from `resolved_button_spec()` rather than a fixed
    /// legacy width.
    #[test]
    fn danger_spec_button_renders_with_the_intent_colour_and_bracket_chrome() {
        let mut d = make_dialog();
        let mut spec = crate::primitives::button::ButtonSpec::new("Move to Trash");
        spec.variant = crate::primitives::button::ButtonVariant::Danger;
        spec.icon = Some(crate::types::Icon::new("\u{f071}", "!"));
        d.buttons[0].spec = Some(spec);

        let viewport = crate::event::Rect::new(0.0, 0.0, 80.0, 30.0);
        let layout = tui_dialog_layout(&d, viewport);
        let theme = Theme::default();

        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 30));
        draw_dialog(&mut buf, &d, &layout, &theme);

        let vis = &layout.visible_buttons[0];
        let bx = vis.bounds.x.round() as u16;
        let by = vis.bounds.y.round() as u16;
        let bw = vis.bounds.width.round() as u16;
        let row: String = (bx..bx + bw).map(|x| cell_char(&buf, x, by)).collect();

        assert!(
            row.starts_with('['),
            "a spec-backed button paints bracket chrome, got {row:?}"
        );
        assert!(
            row.trim_end().ends_with(']'),
            "a spec-backed button's closing bracket must not be clipped, got {row:?}"
        );
        assert!(row.contains("Move to Trash"));
        // The fallback half, since this test never turns Nerd Fonts on.
        assert!(row.contains('!'));
        assert_eq!(cell_fg(&buf, bx + 2, by), ratatui_color(theme.error_fg));
    }

    /// The flip side of the test above: a `DialogButton` with no `spec`
    /// paints through the plain legacy bare-highlighted-row path — no
    /// bracket chrome, just the label text.
    #[test]
    fn legacy_only_button_renders_exactly_as_before() {
        let d = make_dialog();
        assert!(d.buttons[0].spec.is_none());
        let layout = make_layout(&d);
        let theme = Theme::default();

        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 30));
        draw_dialog(&mut buf, &d, &layout, &theme);

        let vis = &layout.visible_buttons[0];
        let bx = vis.bounds.x.round() as u16;
        let by = vis.bounds.y.round() as u16;
        let bw = vis.bounds.width.round() as u16;
        let row: String = (bx..bx + bw).map(|x| cell_char(&buf, x, by)).collect();

        assert!(
            !row.contains('['),
            "a legacy-only button must not gain bracket chrome, got {row:?}"
        );
        assert!(row.contains("Save"));
        assert_eq!(cell_fg(&buf, bx + 1, by), ratatui_color(theme.surface_fg));
    }

    #[test]
    fn renders_input_field_when_present() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 30));
        let mut d = make_dialog();
        d.input = Some(crate::primitives::dialog::DialogInput::TextInput(
            crate::primitives::dialog::DialogTextInput {
                value: "hello".into(),
                placeholder: String::new(),
                cursor: Some(5),
            },
        ));
        let measure = DialogMeasure {
            width: 40.0,
            title_height: 1.0,
            body_height: 2.0,
            table_height: 0.0,
            input_height: 1.0,
            button_row_height: 1.0,
            button_width: 8.0,
            button_gap: 2.0,
            padding: 1.0,
        };
        let viewport = crate::event::Rect::new(0.0, 0.0, 80.0, 30.0);
        let layout = d.layout(viewport, measure, |_| ToolbarItemMeasure::new(0.0));
        let theme = Theme {
            input_bg: crate::types::Color::rgb(7, 7, 7),
            ..Theme::default()
        };
        draw_dialog(&mut buf, &d, &layout, &theme);

        // Input bounds carry input_bg as the row's bg.
        let ib = layout.input_bounds.expect("input bounds present");
        let bg = buf[(ib.x.round() as u16, ib.y.round() as u16)].bg;
        assert_eq!(bg, ratatui::style::Color::Rgb(7, 7, 7));
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 30));
        let d = make_dialog();
        // Use a zero-size viewport so the layout collapses.
        let measure = DialogMeasure {
            width: 0.0,
            title_height: 0.0,
            body_height: 0.0,
            table_height: 0.0,
            input_height: 0.0,
            button_row_height: 0.0,
            button_width: 0.0,
            button_gap: 0.0,
            padding: 0.0,
        };
        let viewport = crate::event::Rect::new(0.0, 0.0, 0.0, 0.0);
        let layout = d.layout(viewport, measure, |_| ToolbarItemMeasure::new(0.0));
        draw_dialog(&mut buf, &d, &layout, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    // ── Gap A: DialogInput::Toolbar paint↔click round-trip ───────────────

    fn make_toolbar_dialog() -> Dialog {
        Dialog {
            id: WidgetId::new("migrate"),
            title: StyledText {
                spans: vec![StyledSpan::plain("Confirm migration")],
            },
            body: vec![StyledText {
                spans: vec![StyledSpan::plain("Choose an action:")],
            }],
            buttons: vec![DialogButton {
                spec: None,
                id: WidgetId::new("ok"),
                label: "OK".into(),
                is_default: true,
                is_cancel: false,
                tint: None,
            }],
            severity: None,
            vertical_buttons: false,
            table: None,
            input: Some(DialogInput::Toolbar(Toolbar {
                id: WidgetId::new("body-toolbar"),
                buttons: vec![
                    ToolbarButton::Action {
                        spec: None,
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
                        spec: None,
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
                ..Default::default()
            })),
        }
    }

    #[test]
    fn body_toolbar_paints_button_brackets() {
        let d = make_toolbar_dialog();
        let viewport = crate::event::Rect::new(0.0, 0.0, 80.0, 30.0);
        let layout = tui_dialog_layout(&d, viewport);
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 30));
        draw_dialog(&mut buf, &d, &layout, &Theme::default());

        // The input_bounds slot should be set.
        assert!(
            layout.input_bounds.is_some(),
            "input_bounds should be Some for Toolbar variant"
        );
        assert!(
            layout.body_toolbar_layout.is_some(),
            "body_toolbar_layout should be Some"
        );

        // The toolbar should have painted `[` somewhere inside the dialog.
        let ib = layout.input_bounds.unwrap();
        let start_y = ib.y.round() as u16;
        let start_x = ib.x.round() as u16;
        let end_x = (ib.x + ib.width).round() as u16;

        let mut found_bracket = false;
        for x in start_x..end_x {
            if cell_char(&buf, x, start_y) == '[' {
                found_bracket = true;
                break;
            }
        }
        assert!(
            found_bracket,
            "toolbar `[` bracket should be painted in body slot"
        );
    }

    /// Shared body for `body_toolbar_click_routes_to_body_toolbar_button[_at_nonzero_origin]`:
    /// `tui_dialog_layout` bakes `viewport.x`/`viewport.y` into `Dialog::layout`'s
    /// `box_x`/`box_y` (absolute frame), so paint + click must agree at a
    /// non-zero viewport origin too, per LESSONS.md's "layout helpers must
    /// return coords in the same frame across backends". The
    /// dialog is centered in an 80×30 viewport and never exceeds ~60×30, so
    /// a buffer sized to the viewport's far corner (`origin + 80/30`) always
    /// has room for it.
    fn body_toolbar_click_routes_to_body_toolbar_button_at(origin_x: f32, origin_y: f32) {
        let d = make_toolbar_dialog();
        let viewport = crate::event::Rect::new(origin_x, origin_y, 80.0, 30.0);
        let layout = tui_dialog_layout(&d, viewport);
        let buf_w = (origin_x + 80.0).ceil() as u16;
        let buf_h = (origin_y + 30.0).ceil() as u16;
        let mut buf = Buffer::empty(Rect::new(0, 0, buf_w, buf_h));
        draw_dialog(&mut buf, &d, &layout, &Theme::default());

        let tl = layout
            .body_toolbar_layout
            .as_ref()
            .expect("toolbar layout present");
        // Find the "Preview" button in the toolbar layout.
        let preview_vis = tl
            .visible_items
            .iter()
            .find(|v| v.action_id.as_ref().map(|id| id.as_str()) == Some("preview"))
            .expect("preview button visible");
        assert!(preview_vis.clickable, "preview should be clickable");

        // Click inside the "Preview" button bounds.
        let cx = preview_vis.bounds.x + 1.0;
        let cy = preview_vis.bounds.y;
        match layout.hit_test(cx, cy) {
            DialogHit::BodyToolbarButton(id) => {
                assert_eq!(id.as_str(), "preview");
            }
            other => panic!("expected BodyToolbarButton(preview), got {:?}", other),
        }
    }

    #[test]
    fn body_toolbar_click_routes_to_body_toolbar_button() {
        body_toolbar_click_routes_to_body_toolbar_button_at(0.0, 0.0);
    }

    #[test]
    fn body_toolbar_click_routes_to_body_toolbar_button_at_nonzero_origin() {
        body_toolbar_click_routes_to_body_toolbar_button_at(7.0, 13.0);
    }

    /// Shared body for `body_toolbar_apply_button_also_routes[_at_nonzero_origin]`
    /// — see `body_toolbar_click_routes_to_body_toolbar_button_at` for the
    /// non-zero-origin rationale.
    fn body_toolbar_apply_button_also_routes_at(origin_x: f32, origin_y: f32) {
        let d = make_toolbar_dialog();
        let viewport = crate::event::Rect::new(origin_x, origin_y, 80.0, 30.0);
        let layout = tui_dialog_layout(&d, viewport);

        let tl = layout
            .body_toolbar_layout
            .as_ref()
            .expect("toolbar layout present");
        let apply_vis = tl
            .visible_items
            .iter()
            .find(|v| v.action_id.as_ref().map(|id| id.as_str()) == Some("apply"))
            .expect("apply button visible");

        let cx = apply_vis.bounds.x + 1.0;
        let cy = apply_vis.bounds.y;
        match layout.hit_test(cx, cy) {
            DialogHit::BodyToolbarButton(id) => {
                assert_eq!(id.as_str(), "apply");
            }
            other => panic!("expected BodyToolbarButton(apply), got {:?}", other),
        }
    }

    #[test]
    fn body_toolbar_apply_button_also_routes() {
        body_toolbar_apply_button_also_routes_at(0.0, 0.0);
    }

    #[test]
    fn body_toolbar_apply_button_also_routes_at_nonzero_origin() {
        body_toolbar_apply_button_also_routes_at(7.0, 13.0);
    }

    // ── Serde round-trip for DialogInput::Toolbar ─────────────────────────

    #[test]
    fn serde_roundtrip_dialog_input_toolbar() {
        use crate::types::WidgetId;
        let input = DialogInput::Toolbar(Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![
                ToolbarButton::Action {
                    spec: None,
                    id: WidgetId::new("preview"),
                    label: "Preview".into(),
                    icon: None,
                    key_hint: None,
                    enabled: true,
                    is_active: false,
                    tooltip: String::new(),
                },
                ToolbarButton::Separator,
            ],
            bg: None,
            focused_index: None,
            ..Default::default()
        });
        let json = serde_json::to_string(&input).unwrap();
        let back: DialogInput = serde_json::from_str(&json).unwrap();
        assert_eq!(input, back);
    }

    #[test]
    fn serde_roundtrip_dialog_input_text_input() {
        let input = DialogInput::TextInput(DialogTextInput {
            value: "hello".into(),
            placeholder: "placeholder".into(),
            cursor: Some(5),
        });
        let json = serde_json::to_string(&input).unwrap();
        let back: DialogInput = serde_json::from_str(&json).unwrap();
        assert_eq!(input, back);
    }
}

/// The `Interactive` half's own unit coverage, for the paths a headless
/// `TuiDriver` test cannot reach: the assistive-technology action route
/// and a host-driven `FocusIn`.
#[cfg(test)]
mod interactive_tests {
    use super::*;
    use crate::a11y::A11yAction;
    use crate::event::{MouseButton, NamedKey, Point};
    use crate::interactive::{
        EventCx, EventHost, FocusSpec, Interactive, KeyEvent, PointerKind, RoutedEvent, SubTarget,
    };
    use crate::primitive::{LayoutCtx, Primitive, Units};
    use crate::types::{StyledSpan, StyledText};
    use crate::PointerShape;

    /// Minimal [`EventHost`] mock — the same shape
    /// `crate::primitives::tab_bar`'s own module tests use.
    #[derive(Debug, Default)]
    struct MockHost {
        focus_requests: Vec<Option<SubTarget>>,
    }

    impl EventHost for MockHost {
        fn is_focused(&self, _id: &WidgetId, _sub: Option<SubTarget>) -> bool {
            false
        }
        fn request_focus(&mut self, _id: &WidgetId, sub: Option<SubTarget>) {
            self.focus_requests.push(sub);
        }
        fn capture_pointer(&mut self, _id: &WidgetId) {}
        fn release_pointer(&mut self) {}
        fn set_cursor(&mut self, _cursor: PointerShape) {}
        fn request_redraw(&mut self) {}
        fn emit(&mut self, _event: crate::UiEvent) {}
        fn units(&self) -> Units {
            Units::Cells
        }
    }

    fn button(id: &str, label: &str, is_default: bool, is_cancel: bool) -> DialogButton {
        DialogButton {
            id: WidgetId::new(id),
            label: label.into(),
            is_default,
            is_cancel,
            tint: None,
            spec: None,
        }
    }

    fn dialog() -> Dialog {
        Dialog {
            id: WidgetId::new("dlg"),
            title: StyledText {
                spans: vec![StyledSpan::plain("Confirm")],
            },
            body: vec![StyledText {
                spans: vec![StyledSpan::plain("Unsaved edits remain.")],
            }],
            buttons: vec![
                button("ok", "Save", true, false),
                button("no", "Cancel", false, true),
            ],
            severity: None,
            vertical_buttons: false,
            table: None,
            input: None,
        }
    }

    fn viewport() -> Rect {
        Rect::new(0.0, 0.0, 60.0, 20.0)
    }

    fn layout_of(d: &Dialog) -> DialogPrimitiveLayout {
        Primitive::layout(d, &LayoutCtx::new(1.0, 1.0, false), viewport())
    }

    fn run(
        d: &Dialog,
        state: &mut DialogState,
        event: RoutedEvent,
    ) -> (Handled<DialogAction>, MockHost) {
        let layout = layout_of(d);
        let mut host = MockHost::default();
        let outcome = {
            let mut cx = EventCx::new(d.id, &mut host);
            d.handle(state, &event, &layout, &mut cx)
        };
        (outcome, host)
    }

    #[test]
    fn focus_declares_one_stop_per_button() {
        let d = dialog();
        match d.focus() {
            FocusSpec::Children(stops) => {
                assert_eq!(stops.len(), 2);
                assert_eq!(stops[0].sub, SubTarget(0));
                assert_eq!(stops[1].sub, SubTarget(1));
            }
            other => panic!("expected per-button stops, got {other:?}"),
        }
    }

    #[test]
    fn a_button_less_dialog_takes_no_focus() {
        let mut d = dialog();
        d.buttons.clear();
        assert_eq!(d.focus(), FocusSpec::None);
    }

    #[test]
    fn a11y_click_activates_the_default_button_when_nothing_holds_focus() {
        let d = dialog();
        let mut state = DialogState::default();
        let (outcome, _) = run(&d, &mut state, RoutedEvent::Action(A11yAction::Click));
        assert_eq!(
            outcome,
            Handled::Event(DialogAction::ButtonActivated(WidgetId::new("ok")))
        );
    }

    #[test]
    fn a11y_click_activates_whichever_button_holds_focus() {
        let d = dialog();
        let mut state = DialogState {
            focused_button: Some(1),
        };
        let (outcome, _) = run(&d, &mut state, RoutedEvent::Action(A11yAction::Click));
        assert_eq!(
            outcome,
            Handled::Event(DialogAction::ButtonActivated(WidgetId::new("no")))
        );
    }

    #[test]
    fn focus_in_names_the_stop_the_host_moved_to() {
        let d = dialog();
        let mut state = DialogState::default();
        let (outcome, _) = run(&d, &mut state, RoutedEvent::FocusIn(Some(SubTarget(1))));
        assert_eq!(outcome, Handled::Consumed);
        assert_eq!(state.focused_button, Some(1));
    }

    #[test]
    fn a_focus_in_naming_no_button_is_ignored() {
        let d = dialog();
        let mut state = DialogState::default();
        let (outcome, _) = run(&d, &mut state, RoutedEvent::FocusIn(Some(SubTarget(7))));
        assert_eq!(outcome, Handled::Ignored);
        assert_eq!(state.focused_button, None);
    }

    #[test]
    fn escape_reports_the_declared_cancel_button() {
        let d = dialog();
        let mut state = DialogState::default();
        let (outcome, _) = run(
            &d,
            &mut state,
            RoutedEvent::Key(KeyEvent {
                key: Key::Named(NamedKey::Escape),
                modifiers: crate::types::Modifiers::default(),
                repeat: false,
            }),
        );
        assert_eq!(
            outcome,
            Handled::Event(DialogAction::Cancelled {
                button: Some(WidgetId::new("no")),
            })
        );
    }

    #[test]
    fn escape_on_a_dialog_with_no_cancel_button_reports_none() {
        let mut d = dialog();
        d.buttons = vec![button("ok", "OK", true, false)];
        let mut state = DialogState::default();
        let (outcome, _) = run(
            &d,
            &mut state,
            RoutedEvent::Key(KeyEvent {
                key: Key::Named(NamedKey::Escape),
                modifiers: crate::types::Modifiers::default(),
                repeat: false,
            }),
        );
        assert_eq!(
            outcome,
            Handled::Event(DialogAction::Cancelled { button: None })
        );
    }

    #[test]
    fn sub_regions_are_reported_in_widget_local_space() {
        let d = dialog();
        let layout = layout_of(&d);
        let regions = d.sub_regions(&layout);
        assert_eq!(regions.len(), 2);
        for ((rect, sub), vis) in regions.iter().zip(layout.layout.visible_buttons.iter()) {
            assert_eq!(*sub, SubTarget(vis.button_idx as u64));
            assert_eq!(rect.x, vis.bounds.x - layout.area.x);
            assert_eq!(rect.y, vis.bounds.y - layout.area.y);
        }
    }

    /// [`dialog`] with a single-action [`DialogInput::Toolbar`] body
    /// slot: toolbar id `body-toolbar`, action id `preview`.
    fn dialog_with_body_toolbar() -> Dialog {
        use crate::primitives::toolbar::{Toolbar, ToolbarButton};

        let mut d = dialog();
        d.input = Some(DialogInput::Toolbar(Toolbar {
            id: WidgetId::new("body-toolbar"),
            buttons: vec![ToolbarButton::Action {
                spec: None,
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
            ..Default::default()
        }));
        d
    }

    /// A cell-units [`Primitive::layout`] resolves through
    /// [`cell_layout`] whatever backend features are compiled in, so a
    /// body toolbar's slots are measured by `cell_item_width` rather
    /// than collapsing to zero width.
    #[test]
    fn a_cell_units_layout_is_the_shared_cell_layout() {
        let d = dialog_with_body_toolbar();
        assert_eq!(layout_of(&d).layout, cell_layout(&d, viewport()));
    }

    #[test]
    fn a_cell_units_body_toolbar_slot_is_as_wide_as_its_bracketed_label() {
        let d = dialog_with_body_toolbar();
        let layout = layout_of(&d);
        let toolbar_layout = layout
            .layout
            .body_toolbar_layout
            .as_ref()
            .expect("body toolbar laid out");
        let item = toolbar_layout
            .visible_items
            .iter()
            .find(|item| item.action_id == Some(WidgetId::new("preview")))
            .expect("a laid-out slot for the Preview button");
        // `[ Preview ]` — "Preview" plus `cell_item_width`'s 4 cells of
        // bracket chrome.
        assert_eq!(item.bounds.width, "Preview".len() as f32 + 4.0);
    }

    #[test]
    fn a_body_toolbar_click_reports_the_toolbars_own_event() {
        use crate::primitives::toolbar::ToolbarEvent;

        let d = dialog_with_body_toolbar();
        let toolbar_id = WidgetId::new("body-toolbar");
        let preview = WidgetId::new("preview");

        let layout = layout_of(&d);
        let toolbar_layout = layout
            .layout
            .body_toolbar_layout
            .as_ref()
            .expect("body toolbar laid out");
        let hit = toolbar_layout
            .visible_items
            .iter()
            .find(|item| item.action_id == Some(preview))
            .map(|item| item.bounds)
            .expect("a laid-out slot for the Preview button");

        let mut state = DialogState::default();
        let (outcome, _) = run(
            &d,
            &mut state,
            RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                sub: None,
                local: Point::new(
                    hit.x + hit.width * 0.5 - layout.area.x,
                    hit.y + hit.height * 0.5 - layout.area.y,
                ),
                mods: crate::types::Modifiers::default(),
            },
        );
        assert_eq!(
            outcome,
            Handled::Event(DialogAction::BodyToolbar {
                toolbar: toolbar_id,
                event: ToolbarEvent::ButtonClicked {
                    id: preview,
                    is_active: true,
                },
            })
        );
    }

    #[test]
    fn a_click_on_the_box_parks_focus_on_the_default_button() {
        let d = dialog();
        let layout = layout_of(&d);
        let body = layout.layout.body_bounds;
        let mut state = DialogState::default();
        let (outcome, host) = run(
            &d,
            &mut state,
            RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                sub: None,
                local: Point::new(body.x - layout.area.x, body.y - layout.area.y),
                mods: crate::types::Modifiers::default(),
            },
        );
        assert_eq!(outcome, Handled::Consumed);
        assert_eq!(state.focused_button, Some(0));
        assert_eq!(host.focus_requests, vec![Some(SubTarget(0))]);
    }

    #[test]
    fn a_click_outside_the_box_bubbles() {
        let d = dialog();
        let mut state = DialogState::default();
        let (outcome, _) = run(
            &d,
            &mut state,
            RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                sub: None,
                local: Point::new(0.0, 0.0),
                mods: crate::types::Modifiers::default(),
            },
        );
        assert_eq!(outcome, Handled::Ignored);
    }

    #[test]
    fn layout_carries_the_backends_nerd_font_state() {
        let d = dialog();
        let with = Primitive::layout(&d, &LayoutCtx::new(1.0, 1.0, true), viewport());
        let without = Primitive::layout(&d, &LayoutCtx::new(1.0, 1.0, false), viewport());
        assert!(with.nerd_fonts);
        assert!(!without.nerd_fonts);
    }
}
