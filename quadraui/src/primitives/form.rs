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
///
/// Note: `Eq` is not derivable because `FieldKind::Slider` carries
/// `f32` values. Apps should not put `Form` into hash maps or use
/// struct equality for state diffing — compare field IDs and
/// individual values instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

/// Validation state rendered as a colored indicator + message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ValidationState {
    Error(String),
    Warning(String),
}

/// One row in a `Form`: a label + an input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// When set, backends render a red/yellow indicator + message.
    /// Overrides `hint` visually when present.
    #[serde(default)]
    pub validation: Option<ValidationState>,
}

/// The input variant carried by a `FormField`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Numeric slider bounded by `min`..=`max` with increment `step`.
    /// `value` is the current setting. Click-and-drag on the track
    /// updates value; arrow keys step by `step`; Home/End jump to
    /// bounds. Emits `FormEvent::SliderChanged` per adjustment.
    ///
    /// Use for tab width, font size multiplier, cursor-blink rate,
    /// anything with a natural numeric domain and a reasonable range.
    Slider {
        value: f32,
        min: f32,
        max: f32,
        #[serde(default = "slider_default_step")]
        step: f32,
    },
    /// Colour picker. `value` is the current selection. Click opens an
    /// app-provided palette / hex-input popup (the Form primitive
    /// doesn't render the popup itself — apps handle that). Emits
    /// `FormEvent::ColorChanged` when the user picks a new value.
    ///
    /// Use for theme customisation, highlight-group overrides, chart
    /// colour assignments.
    ColorPicker { value: crate::Color },
    /// Single-choice selection from a fixed list. `options` is the
    /// display list; `selected_idx` is the current choice.
    /// Click / Enter opens the dropdown; arrows select; Enter confirms.
    /// Emits `FormEvent::DropdownChanged` on commit.
    ///
    /// Use for enum-style settings (theme selection, cursor style,
    /// line-ending preference, language server choice).
    Dropdown {
        options: Vec<StyledText>,
        selected_idx: usize,
    },
    /// Multi-line text editing area. `visible_rows` controls the height
    /// hint (number of rows rendered). Emits `FormEvent::TextInputChanged`
    /// and `TextInputCommitted` like `TextInput`.
    TextArea {
        value: String,
        #[serde(default)]
        placeholder: String,
        #[serde(default)]
        cursor: Option<usize>,
        #[serde(default = "textarea_default_rows")]
        visible_rows: usize,
    },
    /// Masked single-line text input. Characters display as `mask_char`
    /// (default `'•'`). Value is stored in plaintext; only rendering is
    /// masked. Emits the same events as `TextInput`.
    PasswordInput {
        value: String,
        #[serde(default)]
        placeholder: String,
        #[serde(default)]
        cursor: Option<usize>,
        #[serde(default = "password_default_mask")]
        mask_char: char,
    },
    /// Horizontal row of N options where exactly one is selected.
    /// All options are visible (unlike Dropdown). Click selects; emits
    /// `FormEvent::SegmentedControlChanged`.
    SegmentedControl {
        options: Vec<String>,
        selected_idx: usize,
    },
    /// Horizontal row of named boolean toggles. Each toggle has its own
    /// `WidgetId`, label, and value. Click toggles the value; emits
    /// `FormEvent::ToggleChanged { id, value }` with the individual
    /// toggle's ID.
    ///
    /// Use for filter bars, toolbar toggle options, search-panel flags
    /// (case-sensitive / whole-word / regex).
    ToggleGroup { toggles: Vec<ToggleGroupItem> },
    /// Horizontal row of action buttons. Each button has its own
    /// `WidgetId` and label. Click emits `FormEvent::ButtonClicked { id }`
    /// with the individual button's ID.
    ///
    /// Use for dialog footers, form submit rows, search-panel actions
    /// (Find Next / Replace / Replace All).
    ButtonRow { buttons: Vec<ButtonRowItem> },
    /// A full [`crate::Toolbar`] strip embedded inside the form as a
    /// single field row. Renders all toolbar features (icons, key hints,
    /// separators, active/disabled states, tooltips) inside the field's
    /// value column. Click emits
    /// `FormEvent::ToolbarButtonClicked { field_id, button_id }`.
    ///
    /// Use for Settings → Reset / Import / Export rows, or any place
    /// where the toolbar idiom (hover states, separators, key hints) is
    /// more appropriate than `ButtonRow`'s simple bracketed labels.
    Toolbar(crate::primitives::toolbar::Toolbar),
}

/// One toggle in a [`FieldKind::ToggleGroup`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToggleGroupItem {
    pub id: WidgetId,
    pub label: String,
    pub value: bool,
}

/// One button in a [`FieldKind::ButtonRow`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ButtonRowItem {
    pub id: WidgetId,
    pub label: String,
    #[serde(default)]
    pub disabled: bool,
    /// Optional icon rendered before the label. Enables icon-button
    /// rows (commit, push, pull, fetch) in source-control panels.
    #[serde(default)]
    pub icon: Option<crate::types::Icon>,
}

fn slider_default_step() -> f32 {
    1.0
}

fn textarea_default_rows() -> usize {
    3
}

fn password_default_mask() -> char {
    '•'
}

// ── D6 Layout API ───────────────────────────────────────────────────────────
//
// Per Decision D6: primitives return fully-resolved `Layout` structs.
// Seventh primitive on the new shape. Form is structurally a vertical
// stack of fields — same geometry as TreeView. The FieldKind distinction
// (Toggle / TextInput / Button / Label / ReadOnly) is about rendering,
// not layout, so it stays backend-owned: the layout just places rows.

/// Per-field measurement supplied by the backend.
#[derive(Debug, Clone, PartialEq)]
pub struct FormFieldMeasure {
    pub height: f32,
    /// Per-item widths for `ToggleGroup` / `ButtonRow` fields. The
    /// layout uses these to compute per-item hit regions. Empty for
    /// other field kinds.
    pub item_measures: Vec<FormItemMeasure>,
    /// X offset where items start (after label + padding). Only used
    /// when `item_measures` is non-empty.
    pub items_start_x: f32,
    /// Gap between items (cells for TUI, pixels for GTK). Only used
    /// when `item_measures` is non-empty.
    pub item_gap: f32,
}

impl FormFieldMeasure {
    pub fn new(height: f32) -> Self {
        Self {
            height,
            item_measures: Vec::new(),
            items_start_x: 0.0,
            item_gap: 0.0,
        }
    }

    pub fn with_items(
        height: f32,
        items_start_x: f32,
        item_gap: f32,
        items: Vec<FormItemMeasure>,
    ) -> Self {
        Self {
            height,
            item_measures: items,
            items_start_x,
            item_gap,
        }
    }
}

/// Width of one item inside a `ToggleGroup` or `ButtonRow`.
#[derive(Debug, Clone, PartialEq)]
pub struct FormItemMeasure {
    pub id: WidgetId,
    pub width: f32,
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
    /// Resolved per-item rects for `ToggleGroup` / `ButtonRow` fields.
    /// Empty for other field kinds. Rasterisers consume these to paint
    /// items at the layout-determined positions.
    pub item_bounds: Vec<(WidgetId, Rect)>,
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

        let resolved_scroll_offset = crate::primitives::scrollbar::clamp_scroll_offset(
            self.scroll_offset,
            self.fields.len(),
        );

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

            let mut item_bounds = Vec::new();
            if !m.item_measures.is_empty() {
                let mut item_x = m.items_start_x;
                for item in &m.item_measures {
                    let item_rect = Rect::new(item_x, y, item.width, height);
                    hit_regions.push((item_rect, FormHit::Field(item.id.clone())));
                    item_bounds.push((item.id.clone(), item_rect));
                    item_x += item.width + m.item_gap;
                }
            }
            hit_regions.push((bounds, FormHit::Field(id.clone())));

            visible_fields.push(VisibleFormField {
                field_idx: i,
                id,
                bounds,
                item_bounds,
            });
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

// ── NativeSurface paint (#808, Phase 2a of the NativeSurface milestone) ────
//
// Before this, `gtk::form::draw_form`, `macos::form::draw_form` and
// `win::form::draw_form` each independently matched every `FieldKind` and
// painted it with their own cairo / CoreGraphics / Direct2D calls — 1,813
// duplicated lines across the three copies (`docs/SMELL_AUDIT_2026-07.md`
// §5, quadraui#785 child #808). Worse, `macos::form::draw_form` matched
// only 10 of 14 variants and silently fell through (`_ => {}`) on
// `Slider`, `ColorPicker`, `Dropdown` and `TextArea` — a form using one of
// those rendered nothing on macOS, with no error anywhere.
//
// `paint` below is the one shared implementation, written against
// [`crate::native_surface::NativeSurface`] (#807, Phase 1) instead of any
// one backend's drawing API. It handles all 14 variants, so the silent
// macOS fall-through disappears by construction rather than by
// four more copy-pasted match arms.
//
// `#[allow(dead_code)]`: `paint` and its private helpers are only *called*
// when a real pixel backend is compiled in — `GtkBackend::draw_form`,
// `MacBackend::draw_form`/`draw_form_body` and `WinBackend::draw_form`/
// `draw_form_body` are the call sites, and the Win ones live behind a
// further `target_os = "windows"` gate (see `win::form`'s module doc).
// `--features win` alone, on a non-Windows host — exactly what `ci.yml`'s
// win leg (`cargo check -p quadraui --features win`) runs — compiles this
// module (the `cfg` above is satisfied) but reaches none of those call
// sites, so nothing here is "dead" in the sense the lint means; it's the
// same shape `native_surface.rs`'s own `#[allow(dead_code)]` documents,
// one level up the call chain.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
mod native_surface_paint {
    use super::{FieldKind, Form, FormLayout, ValidationState};
    use crate::event::Rect;
    use crate::native_surface::NativeSurface;
    use crate::text_util::snap_to_char_boundary;
    use crate::theme::Theme;
    use crate::types::{Color, StyledText};

    /// Translate a form-local `Rect` (`FormLayout`'s `bounds`/
    /// `item_bounds` are always local to the form, origin at `(0, 0)` —
    /// see `FormLayout`'s doc) into `surface`-absolute coordinates.
    ///
    /// The single place every field-kind arm below crosses into absolute
    /// space, rather than re-deriving the translation per call site the
    /// way the three deleted per-backend copies did — one of which,
    /// `win::form::shift`, added the row's *already-translated* `y` to
    /// the item's still-form-local `y` a second time, over-shifting every
    /// `ToggleGroup` / `ButtonRow` / `SegmentedControl` item on every row
    /// but the first. Not reproduced here: an item's rect and its row's
    /// rect are translated by the same `origin`, once each.
    fn translate(r: &Rect, origin: crate::Point) -> Rect {
        Rect::new(origin.x + r.x, origin.y + r.y, r.width, r.height)
    }

    /// Vertical inset applied to a selected `ToggleGroup` /
    /// `SegmentedControl` item's background pill, in points.
    const PILL_INSET_Y: f32 = 2.0;

    /// Shrink an item rect into the "pill" the *on* / *selected* state
    /// paints its background into: full item width, inset
    /// [`PILL_INSET_Y`] at top and bottom so consecutive items keep a
    /// visible gap between their highlights.
    ///
    /// Ports `macos::form::draw_form`'s pre-#808 `iy + 2.0` /
    /// `ih - 4.0` fill — the one selected-state affordance the three
    /// deleted per-backend copies disagreed about (macOS painted it,
    /// GTK painted nothing, Windows painted `hover_bg` at full row
    /// height). Unifying on macOS's shape gives every pixel backend the
    /// same at-a-glance "which toggle is on" cue; TUI keeps signalling
    /// it with `accent_fg` alone, since a cell grid has no sub-cell
    /// inset to give.
    ///
    /// Degenerate rows (height <= `2 * PILL_INSET_Y`) fall back to the
    /// un-inset rect rather than producing a negative height.
    fn selection_pill(r: Rect) -> Rect {
        if r.height <= PILL_INSET_Y * 2.0 {
            return r;
        }
        Rect::new(
            r.x,
            r.y + PILL_INSET_Y,
            r.width,
            r.height - PILL_INSET_Y * 2.0,
        )
    }

    fn plain_text(t: &StyledText) -> String {
        t.spans.iter().map(|s| s.text.as_str()).collect()
    }

    /// Paint a laid-out [`Form`] using `surface`'s [`NativeSurface`]
    /// verbs.
    ///
    /// `flayout` must be the *same* [`FormLayout`] the caller uses for
    /// hit-testing (typically `Backend::form_layout`'s return value for
    /// this `form`) — `paint` reads geometry only from `flayout`, never
    /// recomputes it, so paint and hit-test can never disagree (the #710
    /// failure class, closed by construction for every backend at once
    /// instead of per-backend vigilance).
    ///
    /// `origin` places the form's local `(0, 0)` at `origin` in
    /// `surface`'s coordinate space — mirrors every other `*_layout`
    /// primitive's "local layout + host-supplied origin" contract (see
    /// `mac_form_layout`'s doc for the same convention stated from the
    /// hit-test side).
    ///
    /// # `FieldKind::Toolbar` is not painted here
    ///
    /// `NativeSurface`'s drawing verbs have no rounded-rect or
    /// hover/pressed/focus-state support, so a `Toolbar` field's full
    /// chrome (see `crate::gtk::toolbar`'s module doc for the state
    /// table every pixel backend already implements) can't be
    /// reproduced through this trait without growing it well past this
    /// primitive's own scope. `paint` still paints a `Toolbar` field's
    /// row background / label / validation indicator like any other
    /// field, but leaves the value column blank — callers must paint the
    /// embedded `Toolbar` themselves *after* `paint` returns (so the two
    /// never fight over `surface`'s exclusive borrow), using their
    /// existing per-backend toolbar rasteriser at the field's
    /// `VisibleFormField::bounds` / `item_bounds`. See
    /// `GtkBackend::draw_form` for the reference pattern.
    pub(crate) fn paint(
        form: &Form,
        flayout: &FormLayout,
        surface: &mut dyn NativeSurface,
        theme: &Theme,
        origin: crate::Point,
    ) {
        let form_rect = Rect::new(
            origin.x,
            origin.y,
            flayout.viewport_width,
            flayout.viewport_height,
        );
        surface.surface_fill_rect(form_rect, theme.tab_bar_bg);

        for vf in &flayout.visible_fields {
            let Some(field) = form.fields.get(vf.field_idx) else {
                continue;
            };
            let row_rect = translate(&vf.bounds, origin);
            let row_h = row_rect.height;

            let is_focused = form.has_focus
                && form
                    .focused_field
                    .as_ref()
                    .is_some_and(|id| id == &field.id);
            let is_header = matches!(field.kind, FieldKind::Label);

            let (default_fg, row_bg) = if is_focused {
                (theme.foreground, theme.selected_bg)
            } else if is_header {
                (theme.header_fg, theme.header_bg)
            } else {
                (theme.foreground, theme.tab_bar_bg)
            };
            surface.surface_fill_rect(row_rect, row_bg);

            let field_fg = if field.disabled {
                theme.muted_fg
            } else {
                default_fg
            };

            let label_text = plain_text(&field.label);
            let (label_w, label_h) = surface.surface_measure_text(&label_text);
            let label_x = row_rect.x + 6.0;
            let label_y = row_rect.y + (row_h - label_h) / 2.0;
            surface.surface_draw_text_run(
                Rect::new(label_x, label_y, label_w, label_h),
                &label_text,
                field_fg,
            );
            let label_right = label_x + label_w;
            let no_label = label_text.is_empty();
            let input_right = row_rect.x + row_rect.width - 8.0;

            match &field.kind {
                FieldKind::Label => {}
                FieldKind::Toggle { value } => {
                    let glyph = if *value { "[x]" } else { "[ ]" };
                    let fg = if *value && !field.disabled {
                        theme.accent_fg
                    } else {
                        field_fg
                    };
                    let (w, h) = surface.surface_measure_text(glyph);
                    let ix = if no_label { label_x } else { input_right - w };
                    if no_label || ix > label_right + 8.0 {
                        let iy = row_rect.y + (row_h - h) / 2.0;
                        surface.surface_draw_text_run(Rect::new(ix, iy, w, h), glyph, fg);
                    }
                }
                FieldKind::TextInput {
                    value,
                    placeholder,
                    cursor,
                    selection_anchor,
                } => {
                    paint_bracketed_text(
                        surface,
                        row_rect,
                        label_right,
                        no_label,
                        input_right,
                        value,
                        placeholder,
                        *cursor,
                        *selection_anchor,
                        field_fg,
                        theme.muted_fg,
                        theme.selection_bg,
                        theme.accent_fg,
                        false,
                        '\0',
                    );
                }
                FieldKind::PasswordInput {
                    value,
                    placeholder,
                    cursor,
                    mask_char,
                } => {
                    paint_bracketed_text(
                        surface,
                        row_rect,
                        label_right,
                        no_label,
                        input_right,
                        value,
                        placeholder,
                        *cursor,
                        None,
                        field_fg,
                        theme.muted_fg,
                        theme.selection_bg,
                        theme.accent_fg,
                        true,
                        *mask_char,
                    );
                }
                FieldKind::TextArea {
                    value,
                    placeholder,
                    cursor,
                    ..
                } => {
                    let first_line = value.lines().next().unwrap_or("");
                    paint_bracketed_text(
                        surface,
                        row_rect,
                        label_right,
                        no_label,
                        input_right,
                        first_line,
                        placeholder,
                        *cursor,
                        None,
                        field_fg,
                        theme.muted_fg,
                        theme.selection_bg,
                        theme.accent_fg,
                        false,
                        '\0',
                    );
                }
                FieldKind::Button => {
                    let cap = label_text.clone();
                    let total_w = label_w + 24.0;
                    let ix = if no_label {
                        label_x
                    } else {
                        input_right - total_w
                    };
                    if no_label || ix > row_rect.x + 8.0 {
                        let brk = if is_focused {
                            theme.accent_fg
                        } else {
                            theme.muted_fg
                        };
                        let (_, h) = surface.surface_measure_text("<");
                        let y = row_rect.y + (row_h - h) / 2.0;
                        surface.surface_draw_text_run(Rect::new(ix, y, 12.0, h), "<", brk);
                        let text_fg = if field.disabled {
                            theme.muted_fg
                        } else {
                            field_fg
                        };
                        surface.surface_draw_text_run(
                            Rect::new(ix + 12.0, y, label_w, h),
                            &cap,
                            text_fg,
                        );
                        surface.surface_draw_text_run(
                            Rect::new(ix + 12.0 + label_w + 4.0, y, 12.0, h),
                            ">",
                            brk,
                        );
                    }
                }
                FieldKind::ReadOnly { value } => {
                    let text = plain_text(value);
                    let (w, h) = surface.surface_measure_text(&text);
                    let ix = if no_label { label_x } else { input_right - w };
                    if no_label || ix > label_right + 8.0 {
                        let iy = row_rect.y + (row_h - h) / 2.0;
                        surface.surface_draw_text_run(
                            Rect::new(ix, iy, w, h),
                            &text,
                            theme.muted_fg,
                        );
                    }
                }
                FieldKind::Slider {
                    value, min, max, ..
                } => {
                    let track_w = 80.0_f32;
                    let range = (max - min).max(f32::EPSILON);
                    let frac = ((value - min) / range).clamp(0.0, 1.0);
                    let value_str = format!("{value:.2}");
                    let (value_w, value_h) = surface.surface_measure_text(&value_str);
                    let total = track_w + 8.0 + value_w;
                    let ix = input_right - total;
                    if ix > label_right + 8.0 {
                        let track_y = row_rect.y + row_h / 2.0 - 2.0;
                        surface.surface_fill_rect(
                            Rect::new(ix, track_y, track_w, 4.0),
                            theme.muted_fg,
                        );
                        surface.surface_fill_rect(
                            Rect::new(ix, track_y, track_w * frac, 4.0),
                            theme.accent_fg,
                        );
                        let vy = row_rect.y + (row_h - value_h) / 2.0;
                        surface.surface_draw_text_run(
                            Rect::new(ix + track_w + 8.0, vy, value_w, value_h),
                            &value_str,
                            field_fg,
                        );
                    }
                }
                FieldKind::ColorPicker { value } => {
                    let hex = format!("#{:02x}{:02x}{:02x}", value.r, value.g, value.b);
                    let (hw, hh) = surface.surface_measure_text(&hex);
                    let swatch = 12.0_f32;
                    let total = swatch + 6.0 + hw;
                    let ix = input_right - total;
                    if ix > label_right + 8.0 {
                        let sy = row_rect.y + (row_h - swatch) / 2.0;
                        surface.surface_fill_rect(Rect::new(ix, sy, swatch, swatch), *value);
                        let ty = row_rect.y + (row_h - hh) / 2.0;
                        surface.surface_draw_text_run(
                            Rect::new(ix + swatch + 6.0, ty, hw, hh),
                            &hex,
                            field_fg,
                        );
                    }
                }
                FieldKind::Dropdown {
                    options,
                    selected_idx,
                } => {
                    let chosen = options
                        .get(*selected_idx)
                        .map(plain_text)
                        .unwrap_or_default();
                    let (cw, ch) = surface.surface_measure_text(&chosen);
                    let (chev_w, _) = surface.surface_measure_text("\u{25BE}");
                    let total = cw + 4.0 + chev_w;
                    let ix = input_right - total;
                    if ix > label_right + 8.0 {
                        let ty = row_rect.y + (row_h - ch) / 2.0;
                        surface.surface_draw_text_run(Rect::new(ix, ty, cw, ch), &chosen, field_fg);
                        surface.surface_draw_text_run(
                            Rect::new(ix + cw + 4.0, ty, chev_w, ch),
                            "\u{25BE}",
                            theme.muted_fg,
                        );
                    }
                }
                FieldKind::ToggleGroup { toggles } => {
                    for (item_id, item_rect) in &vf.item_bounds {
                        if let Some(t) = toggles.iter().find(|t| &t.id == item_id) {
                            let on = t.value && !field.disabled;
                            let fg = if on { theme.accent_fg } else { theme.muted_fg };
                            let r = translate(item_rect, origin);
                            if on {
                                surface.surface_fill_rect(selection_pill(r), theme.selected_bg);
                            }
                            surface.surface_draw_text_run(r, &t.label, fg);
                        }
                    }
                }
                FieldKind::ButtonRow { buttons } => {
                    for (item_id, item_rect) in &vf.item_bounds {
                        if let Some(b) = buttons.iter().find(|b| &b.id == item_id) {
                            let r = translate(item_rect, origin);
                            let disabled = b.disabled || field.disabled;
                            let bg = if disabled { row_bg } else { theme.hover_bg };
                            surface.surface_fill_rect(r, bg);
                            let fg = if disabled { theme.muted_fg } else { field_fg };
                            let mut tx = r.x + 4.0;
                            if let Some(ref icon) = b.icon {
                                let (iw, ih) = surface.surface_measure_text(&icon.fallback);
                                let iy = r.y + (r.height - ih) / 2.0;
                                surface.surface_draw_text_run(
                                    Rect::new(tx, iy, iw, ih),
                                    &icon.fallback,
                                    fg,
                                );
                                tx += iw + 4.0;
                            }
                            let (lw, lh) = surface.surface_measure_text(&b.label);
                            let ly = r.y + (r.height - lh) / 2.0;
                            surface.surface_draw_text_run(Rect::new(tx, ly, lw, lh), &b.label, fg);
                        }
                    }
                }
                FieldKind::SegmentedControl {
                    options,
                    selected_idx,
                } => {
                    for (i, (_item_id, item_rect)) in vf.item_bounds.iter().enumerate() {
                        let opt = options.get(i).map(|s| s.as_str()).unwrap_or("");
                        let fg = if i == *selected_idx {
                            theme.accent_fg
                        } else {
                            theme.muted_fg
                        };
                        let r = translate(item_rect, origin);
                        if i == *selected_idx {
                            surface.surface_fill_rect(selection_pill(r), theme.selected_bg);
                        }
                        let (tw, th) = surface.surface_measure_text(opt);
                        let ty = r.y + (r.height - th) / 2.0;
                        surface.surface_draw_text_run(Rect::new(r.x, ty, tw, th), opt, fg);
                    }
                }
                FieldKind::Toolbar(_) => {
                    // See this fn's doc: painted by the caller after
                    // `paint` returns, using the backend's own toolbar
                    // rasteriser.
                }
            }

            if let Some(ref vs) = field.validation {
                let (color, msg) = match vs {
                    ValidationState::Error(m) => (theme.error_fg, m.as_str()),
                    ValidationState::Warning(m) => (theme.warning_fg, m.as_str()),
                };
                surface.surface_fill_rect(
                    Rect::new(row_rect.x + 2.0, row_rect.y + (row_h - 3.0) / 2.0, 3.0, 3.0),
                    color,
                );
                if !msg.is_empty() {
                    let (mw, mh) = surface.surface_measure_text(msg);
                    let my = row_rect.y + (row_h - mh) / 2.0;
                    surface.surface_draw_text_run(
                        Rect::new(row_rect.x + 8.0, my, mw, mh),
                        msg,
                        color,
                    );
                }
            }
        }
    }

    /// Shared bracketed `[value]` painter for `TextInput` / `PasswordInput`
    /// / the single-line `TextArea` preview. `mask` replaces every
    /// character with `mask_char` when `masked` is true. Port of
    /// `win::form::draw_bracketed_text` (the most complete of the three
    /// deleted copies — it already carried the selection highlight GTK's
    /// copy had and macOS's never gained).
    #[allow(clippy::too_many_arguments)]
    fn paint_bracketed_text(
        surface: &mut dyn NativeSurface,
        row_rect: Rect,
        label_right: f32,
        no_label: bool,
        input_right: f32,
        value: &str,
        placeholder: &str,
        cursor: Option<usize>,
        selection_anchor: Option<usize>,
        field_fg: Color,
        dim_fg: Color,
        sel_bg: Color,
        accent_fg: Color,
        masked: bool,
        mask_char: char,
    ) {
        let masked_value: String;
        let shown: &str = if value.is_empty() {
            placeholder
        } else if masked {
            masked_value = value.chars().map(|_| mask_char).collect();
            &masked_value
        } else {
            value
        };
        let input_fg = if value.is_empty() { dim_fg } else { field_fg };
        let row_h = row_rect.height;
        let (shown_w, shown_h) = surface.surface_measure_text(shown);

        let (ix, _dw, bracket_right) = if no_label {
            let ix = row_rect.x + 6.0;
            let bracket_r = input_right - 4.0;
            let avail = (bracket_r - ix - 8.0).max(0.0);
            (ix, shown_w.min(avail), bracket_r)
        } else {
            let max_width = (row_rect.width * 0.6).max(80.0);
            let dw = shown_w.min(max_width);
            let ix = input_right - dw - 14.0;
            (ix, dw, ix + 8.0 + dw + 2.0)
        };
        if !(no_label || ix > label_right + 8.0) {
            return;
        }

        let y = row_rect.y + (row_h - shown_h) / 2.0;
        surface.surface_draw_text_run(Rect::new(ix, y, 8.0, shown_h), "[", dim_fg);

        let has_sel = !masked
            && matches!((cursor, selection_anchor), (Some(c), Some(a)) if c != a && !value.is_empty());
        if has_sel {
            let (c, a) = (cursor.unwrap(), selection_anchor.unwrap());
            let (lo, hi) = (c.min(a), c.max(a));
            let lo = snap_to_char_boundary(shown, lo);
            let hi = snap_to_char_boundary(shown, hi);
            let prefix = &shown[..lo];
            let sel_text = &shown[lo..hi];
            let suffix = &shown[hi..];
            let (pw, _) = surface.surface_measure_text(prefix);
            let (sw, _) = surface.surface_measure_text(sel_text);
            surface.surface_fill_rect(
                Rect::new(ix + 8.0 + pw, row_rect.y + 2.0, sw, row_h - 4.0),
                sel_bg,
            );
            surface.surface_draw_text_run(Rect::new(ix + 8.0, y, pw, shown_h), prefix, input_fg);
            surface.surface_draw_text_run(
                Rect::new(ix + 8.0 + pw, y, sw, shown_h),
                sel_text,
                field_fg,
            );
            surface.surface_draw_text_run(
                Rect::new(ix + 8.0 + pw + sw, y, shown_w - pw - sw, shown_h),
                suffix,
                input_fg,
            );
        } else {
            surface.surface_draw_text_run(
                Rect::new(ix + 8.0, y, shown_w, shown_h),
                shown,
                input_fg,
            );
        }

        surface.surface_draw_text_run(Rect::new(bracket_right, y, 8.0, shown_h), "]", dim_fg);

        if !masked {
            if let Some(cur) = cursor {
                if !value.is_empty() {
                    let prefix = &shown[..snap_to_char_boundary(shown, cur)];
                    let (pw, _) = surface.surface_measure_text(prefix);
                    let cx = ix + 8.0 + pw;
                    surface.surface_fill_rect(
                        Rect::new(cx, row_rect.y + 3.0, 1.5, row_h - 6.0),
                        accent_fg,
                    );
                }
            }
        } else if let Some(cur) = cursor {
            if !value.is_empty() {
                let char_pos = value[..snap_to_char_boundary(value, cur)].chars().count();
                let prefix: String = shown.chars().take(char_pos).collect();
                let (pw, _) = surface.surface_measure_text(&prefix);
                let cx = ix + 8.0 + pw;
                surface.surface_fill_rect(
                    Rect::new(cx, row_rect.y + 3.0, 1.5, row_h - 6.0),
                    accent_fg,
                );
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::primitives::form::{FormField, ToggleGroupItem};
        use crate::primitives::layout_metrics::{form_field_measure, form_row_height, TextMeasure};
        use crate::types::WidgetId;
        use crate::Image;

        /// Records every drawing verb `paint` issues, so a paint
        /// assertion can run on any host — no cairo, Core Graphics or
        /// Direct2D needed. The pixel backends' own probes
        /// (`macos::form`'s `BitmapSurface`, `gtk::form`'s
        /// `ImageSurface`) still cover "the verb reached real pixels";
        /// this covers "the shared painter emits the right verb at all",
        /// on every leg of the quality gate rather than only the macOS
        /// one.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
        }

        /// 6 units per character — same fixed-width stand-in the
        /// `layout_metrics` tests use, so item widths stay predictable.
        struct FixedMeasure;

        impl TextMeasure for FixedMeasure {
            fn width_of(&self, text: &str) -> f32 {
                text.chars().count() as f32 * 6.0
            }
        }

        impl NativeSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(320.0, 160.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                14.0
            }
            fn surface_char_width(&self) -> f32 {
                6.0
            }
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (FixedMeasure.width_of(text), 12.0)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_draw_text_run(&mut self, _rect: Rect, _text: &str, _color: Color) {}
            fn surface_draw_line(
                &mut self,
                _from: crate::Point,
                _to: crate::Point,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        /// Lay `form` out the way every pixel backend's `form_layout`
        /// does, paint it through the shared painter, and hand back the
        /// recorded fills alongside the layout.
        fn paint_recorded(form: &Form) -> (RecordingSurface, FormLayout) {
            let row_h = form_row_height(14.0);
            let flayout = form.layout(320.0, 160.0, |i| {
                form_field_measure(&form.fields[i], row_h, &FixedMeasure)
            });
            let mut surface = RecordingSurface::default();
            paint(
                form,
                &flayout,
                &mut surface,
                &Theme::default(),
                crate::Point::new(0.0, 0.0),
            );
            (surface, flayout)
        }

        fn one_field_form(id: &str, kind: FieldKind) -> Form {
            Form {
                id: WidgetId::new("form"),
                fields: vec![FormField {
                    id: WidgetId::new(id),
                    label: StyledText::default(),
                    kind,
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                }],
                focused_field: None,
                scroll_offset: 0,
                has_focus: false,
            }
        }

        /// The pill a selected item paints: full item width, inset 2 at
        /// top and bottom.
        fn expected_pill(item: &Rect) -> Rect {
            Rect::new(item.x, item.y + 2.0, item.width, item.height - 4.0)
        }

        /// Regression for the #808 unification: the shared painter was
        /// written from the Windows copy, which never painted an
        /// on-toggle background — so adopting it silently dropped the
        /// `selected_bg` pill macOS had behind every *on* toggle, and
        /// `macos::form`'s `toggle_group_on_item_paints_selected_bg`
        /// caught it only on the macOS CI leg.
        #[test]
        fn toggle_group_fills_selected_bg_behind_on_items_only() {
            let form = one_field_form(
                "flags",
                FieldKind::ToggleGroup {
                    toggles: vec![
                        ToggleGroupItem {
                            id: WidgetId::new("case"),
                            label: "Aa".into(),
                            value: false,
                        },
                        ToggleGroupItem {
                            id: WidgetId::new("regex"),
                            label: ".*".into(),
                            value: true,
                        },
                    ],
                },
            );
            let (surface, flayout) = paint_recorded(&form);
            let theme = Theme::default();
            let items = &flayout.visible_fields[0].item_bounds;
            let on = &items
                .iter()
                .find(|(id, _)| id == &WidgetId::new("regex"))
                .expect("on toggle laid out")
                .1;
            let off = &items
                .iter()
                .find(|(id, _)| id == &WidgetId::new("case"))
                .expect("off toggle laid out")
                .1;

            assert!(
                surface
                    .fills
                    .contains(&(expected_pill(on), theme.selected_bg)),
                "on toggle should fill selected_bg inset 2 vertically; fills were {:?}",
                surface.fills,
            );
            assert!(
                !surface
                    .fills
                    .iter()
                    .any(|(r, c)| r.x == off.x && *c == theme.selected_bg),
                "off toggle must not paint a selected_bg pill; fills were {:?}",
                surface.fills,
            );
        }

        /// Same regression on the sibling variant: the selected segment
        /// used to fill `hover_bg` at full row height (the Windows
        /// copy's shape), which reads as a hover cue rather than a
        /// selection one and mismatched macOS's `selected_bg` pill.
        #[test]
        fn segmented_control_fills_selected_bg_behind_the_selected_segment_only() {
            let form = one_field_form(
                "scope",
                FieldKind::SegmentedControl {
                    options: vec!["File".into(), "Folder".into(), "Project".into()],
                    selected_idx: 1,
                },
            );
            let (surface, flayout) = paint_recorded(&form);
            let theme = Theme::default();
            let items = &flayout.visible_fields[0].item_bounds;
            assert_eq!(items.len(), 3, "three segments should be laid out");

            assert!(
                surface
                    .fills
                    .contains(&(expected_pill(&items[1].1), theme.selected_bg)),
                "selected segment should fill selected_bg inset 2 vertically; \
                 fills were {:?}",
                surface.fills,
            );
            for idx in [0usize, 2] {
                let seg = &items[idx].1;
                assert!(
                    !surface
                        .fills
                        .iter()
                        .any(|(r, c)| r.x == seg.x && *c == theme.selected_bg),
                    "unselected segment {idx} must not paint a selected_bg pill; \
                     fills were {:?}",
                    surface.fills,
                );
            }
            assert!(
                !surface.fills.iter().any(|(_, c)| *c == theme.hover_bg),
                "no segment should paint hover_bg — nothing is hovered; \
                 fills were {:?}",
                surface.fills,
            );
        }

        /// A row too short to inset falls back to the un-inset rect
        /// rather than producing a negative height (which every backend
        /// would either clamp, drop, or paint as an inverted rect).
        #[test]
        fn selection_pill_does_not_invert_on_degenerate_rows() {
            let flat = Rect::new(10.0, 20.0, 30.0, 3.0);
            assert_eq!(selection_pill(flat), flat);
            let tall = Rect::new(10.0, 20.0, 30.0, 20.0);
            assert_eq!(selection_pill(tall), Rect::new(10.0, 22.0, 30.0, 16.0));
        }
    }
}

// `#[allow(unused_imports)]`: see `mod native_surface_paint`'s doc — a
// `--features win` build on a non-Windows host has no reachable call site
// for `paint` (the Win one is further gated to `target_os = "windows"`).
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(unused_imports)]
pub(crate) use native_surface_paint::paint;

/// Events a `Form` emits back to the app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FormEvent {
    /// A `Toggle` field's value changed.
    ToggleChanged { id: WidgetId, value: bool },
    /// A `TextInput` field's text changed. Fires on every keystroke that
    /// modifies the value.
    TextInputChanged { id: WidgetId, value: String },
    /// A `TextInput` received Enter while focused.
    TextInputCommitted { id: WidgetId, value: String },
    /// A `Slider` field's value changed (drag, arrow key, Home/End).
    SliderChanged { id: WidgetId, value: f32 },
    /// A `ColorPicker` field produced a new colour.
    ColorChanged { id: WidgetId, value: crate::Color },
    /// A `Dropdown` field committed a new selection.
    DropdownChanged { id: WidgetId, selected_idx: usize },
    /// A `SegmentedControl` field committed a new selection.
    SegmentedControlChanged { id: WidgetId, selected_idx: usize },
    /// Keyboard focus moved to a different field.
    FocusChanged { id: WidgetId },
    /// A `Button` was clicked or activated with Enter / Space.
    ButtonClicked { id: WidgetId },
    /// An action button inside a [`FieldKind::Toolbar`] field was
    /// clicked. `field_id` identifies the form field; `button_id`
    /// identifies the toolbar action within it.
    ToolbarButtonClicked {
        field_id: WidgetId,
        button_id: WidgetId,
    },
    /// A key was pressed while the form had focus and the primitive did
    /// not consume it. The app may interpret it (e.g. `?` opens help).
    KeyPressed { key: String, modifiers: Modifiers },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Color;

    #[test]
    fn form_roundtrip_serde() {
        let form = Form {
            id: WidgetId::new("settings"),
            fields: vec![
                FormField {
                    id: WidgetId::new("header"),
                    label: StyledText::plain("Editor"),
                    kind: FieldKind::Label,
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                },
                FormField {
                    id: WidgetId::new("line-numbers"),
                    label: StyledText::plain("Show line numbers"),
                    kind: FieldKind::Toggle { value: true },
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                },
                FormField {
                    id: WidgetId::new("tabstop"),
                    label: StyledText::plain("Tab width"),
                    kind: FieldKind::TextInput {
                        value: "4".to_string(),
                        placeholder: "2".to_string(),
                        cursor: Some(1),
                        selection_anchor: None,
                    },
                    hint: StyledText::plain("Number of spaces per tab"),
                    disabled: false,
                    validation: None,
                },
                FormField {
                    id: WidgetId::new("save"),
                    label: StyledText::plain("Save settings"),
                    kind: FieldKind::Button,
                    hint: StyledText::default(),
                    disabled: false,
                    validation: None,
                },
            ],
            focused_field: Some(WidgetId::new("line-numbers")),
            scroll_offset: 0,
            has_focus: true,
        };
        let json = serde_json::to_string(&form).unwrap();
        let back: Form = serde_json::from_str(&json).unwrap();
        assert_eq!(form, back);
    }

    #[test]
    fn form_event_roundtrip_serde() {
        let events = vec![
            FormEvent::ToggleChanged {
                id: WidgetId::new("line-numbers"),
                value: false,
            },
            FormEvent::TextInputChanged {
                id: WidgetId::new("tabstop"),
                value: "8".to_string(),
            },
            FormEvent::TextInputCommitted {
                id: WidgetId::new("tabstop"),
                value: "8".to_string(),
            },
            FormEvent::FocusChanged {
                id: WidgetId::new("save"),
            },
            FormEvent::ButtonClicked {
                id: WidgetId::new("save"),
            },
            FormEvent::KeyPressed {
                key: "Escape".to_string(),
                modifiers: Modifiers::default(),
            },
        ];
        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let back: FormEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(event, &back);
        }
    }

    // ── Form field primitive tests (#143 Slider/ColorPicker/Dropdown) ─

    #[test]
    fn form_slider_field_serde() {
        let field = FormField {
            id: WidgetId::new("font-size"),
            label: StyledText::plain("Font size"),
            kind: FieldKind::Slider {
                value: 14.0,
                min: 8.0,
                max: 32.0,
                step: 1.0,
            },
            hint: StyledText::plain("Editor font size in px"),
            disabled: false,
            validation: None,
        };
        let json = serde_json::to_string(&field).unwrap();
        let back: FormField = serde_json::from_str(&json).unwrap();
        assert_eq!(field, back);
    }

    #[test]
    fn form_color_picker_field_serde() {
        let field = FormField {
            id: WidgetId::new("accent"),
            label: StyledText::plain("Accent colour"),
            kind: FieldKind::ColorPicker {
                value: Color::rgb(0x78, 0xb4, 0xff),
            },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
        };
        let json = serde_json::to_string(&field).unwrap();
        let back: FormField = serde_json::from_str(&json).unwrap();
        assert_eq!(field, back);
    }

    #[test]
    fn form_dropdown_field_serde() {
        let field = FormField {
            id: WidgetId::new("theme"),
            label: StyledText::plain("Theme"),
            kind: FieldKind::Dropdown {
                options: vec![
                    StyledText::plain("One Dark"),
                    StyledText::plain("Solarized Light"),
                    StyledText::plain("Monokai"),
                ],
                selected_idx: 0,
            },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
        };
        let json = serde_json::to_string(&field).unwrap();
        let back: FormField = serde_json::from_str(&json).unwrap();
        assert_eq!(field, back);
    }

    #[test]
    fn form_slider_legacy_deserialize_with_default_step() {
        // A client might omit `step` — it defaults to 1.0 via serde.
        let json = r#"{
            "id": "x",
            "label": {"spans":[{"text":"X","fg":null,"bg":null}]},
            "kind": {"Slider": {"value": 5.0, "min": 0.0, "max": 10.0}},
            "hint": {"spans":[]}
        }"#;
        let field: FormField = serde_json::from_str(json).unwrap();
        match field.kind {
            FieldKind::Slider { step, .. } => assert_eq!(step, 1.0),
            _ => panic!("expected Slider"),
        }
    }

    #[test]
    fn form_toggle_group_serde() {
        let field = FormField {
            id: WidgetId::new("opts"),
            label: StyledText::plain("Options"),
            kind: FieldKind::ToggleGroup {
                toggles: vec![
                    ToggleGroupItem {
                        id: WidgetId::new("case"),
                        label: "Aa".into(),
                        value: true,
                    },
                    ToggleGroupItem {
                        id: WidgetId::new("word"),
                        label: "Ab|".into(),
                        value: false,
                    },
                ],
            },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
        };
        let json = serde_json::to_string(&field).unwrap();
        let back: FormField = serde_json::from_str(&json).unwrap();
        assert_eq!(field, back);
    }

    #[test]
    fn form_button_row_serde() {
        let field = FormField {
            id: WidgetId::new("actions"),
            label: StyledText::default(),
            kind: FieldKind::ButtonRow {
                buttons: vec![
                    ButtonRowItem {
                        id: WidgetId::new("next"),
                        label: "Find Next".into(),
                        disabled: false,
                        icon: None,
                    },
                    ButtonRowItem {
                        id: WidgetId::new("all"),
                        label: "Replace All".into(),
                        disabled: true,
                        icon: None,
                    },
                ],
            },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
        };
        let json = serde_json::to_string(&field).unwrap();
        let back: FormField = serde_json::from_str(&json).unwrap();
        assert_eq!(field, back);
    }

    // ── D6 Form layout API tests ──────────────────────────────────────

    fn make_form_field(id: &str, label: &str, kind: FieldKind) -> FormField {
        FormField {
            id: WidgetId::new(id),
            label: StyledText::plain(label),
            kind,
            hint: StyledText::default(),
            disabled: false,
            validation: None,
        }
    }

    fn make_form(fields: Vec<FormField>, scroll: usize) -> Form {
        Form {
            id: WidgetId::new("f"),
            fields,
            focused_field: None,
            scroll_offset: scroll,
            has_focus: true,
        }
    }

    #[test]
    fn form_layout_empty() {
        let f = make_form(vec![], 0);
        let layout = f.layout(40.0, 20.0, |_| FormFieldMeasure::new(1.0));
        assert_eq!(layout.visible_fields.len(), 0);
        assert_eq!(layout.hit_test(5.0, 5.0), FormHit::Empty);
    }

    #[test]
    fn form_layout_stacks_fields() {
        let f = make_form(
            vec![
                make_form_field("header", "Editor", FieldKind::Label),
                make_form_field("toggle1", "Line numbers", FieldKind::Toggle { value: true }),
                make_form_field("btn", "Save", FieldKind::Button),
            ],
            0,
        );
        let layout = f.layout(40.0, 10.0, |_| FormFieldMeasure::new(1.0));
        assert_eq!(layout.visible_fields.len(), 3);
        assert_eq!(layout.visible_fields[0].bounds.y, 0.0);
        assert_eq!(layout.visible_fields[1].bounds.y, 1.0);
        assert_eq!(layout.visible_fields[2].bounds.y, 2.0);
        match layout.hit_test(10.0, 1.5) {
            FormHit::Field(id) => assert_eq!(id.as_str(), "toggle1"),
            _ => panic!("expected Field(toggle1)"),
        }
    }

    #[test]
    fn form_layout_hit_carries_widget_id_not_index() {
        // Adding fields in arbitrary order — hit_test returns the id,
        // not the flat index, so apps don't care about ordering.
        let f = make_form(
            vec![
                make_form_field("zebra", "Zebra", FieldKind::Button),
                make_form_field("alpha", "Alpha", FieldKind::Button),
            ],
            0,
        );
        let layout = f.layout(40.0, 5.0, |_| FormFieldMeasure::new(1.0));
        match layout.hit_test(10.0, 0.5) {
            FormHit::Field(id) => assert_eq!(id.as_str(), "zebra"),
            _ => panic!(),
        }
        match layout.hit_test(10.0, 1.5) {
            FormHit::Field(id) => assert_eq!(id.as_str(), "alpha"),
            _ => panic!(),
        }
    }

    #[test]
    fn form_layout_scroll_offset_skips() {
        let f = make_form(
            (0..5)
                .map(|i| make_form_field(&format!("f{i}"), &format!("F{i}"), FieldKind::Button))
                .collect(),
            2,
        );
        let layout = f.layout(40.0, 10.0, |_| FormFieldMeasure::new(1.0));
        assert_eq!(layout.visible_fields[0].field_idx, 2);
        assert_eq!(layout.visible_fields[0].id.as_str(), "f2");
    }

    #[test]
    fn form_layout_varying_heights_by_kind() {
        // Fields with hints are taller; Label rows can be shorter.
        let fields = vec![
            make_form_field("hdr", "Header", FieldKind::Label),
            make_form_field(
                "txt",
                "Name",
                FieldKind::TextInput {
                    value: "John".to_string(),
                    placeholder: String::new(),
                    cursor: Some(4),
                    selection_anchor: None,
                },
            ),
        ];
        let f = make_form(fields.clone(), 0);
        let layout = f.layout(40.0, 10.0, |i| {
            // Pretend TextInput fields are 2 rows tall (room for hint), Label is 1.
            match fields[i].kind {
                FieldKind::TextInput { .. } => FormFieldMeasure::new(2.0),
                _ => FormFieldMeasure::new(1.0),
            }
        });
        assert_eq!(layout.visible_fields[0].bounds.height, 1.0);
        assert_eq!(layout.visible_fields[1].bounds.y, 1.0);
        assert_eq!(layout.visible_fields[1].bounds.height, 2.0);
    }

    #[test]
    fn text_input_cursor_and_selection_serde() {
        // Round-trip a TextInput variant with explicit cursor + selection state.
        let field = FormField {
            id: WidgetId::new("name"),
            label: StyledText::plain("Name"),
            kind: FieldKind::TextInput {
                value: "hello world".to_string(),
                placeholder: String::new(),
                cursor: Some(5),
                selection_anchor: Some(0),
            },
            hint: StyledText::default(),
            disabled: false,
            validation: None,
        };
        let json = serde_json::to_string(&field).unwrap();
        let back: FormField = serde_json::from_str(&json).unwrap();
        assert_eq!(field, back);

        // Legacy shape without cursor/selection_anchor also deserializes
        // (new fields default to None) — ensures the extension is
        // backward-compatible with pre-A.3d serialised forms.
        let legacy = r#"{
            "id": "legacy",
            "label": {"spans":[{"text":"Legacy","fg":null,"bg":null}]},
            "kind": {"TextInput": {"value": "x"}},
            "hint": {"spans":[]}
        }"#;
        let parsed: FormField = serde_json::from_str(legacy).unwrap();
        match parsed.kind {
            FieldKind::TextInput {
                cursor,
                selection_anchor,
                ..
            } => {
                assert_eq!(cursor, None);
                assert_eq!(selection_anchor, None);
            }
            other => panic!("unexpected kind: {:?}", other),
        }
    }
}
