//! Cell-grid layout resolver for [`Form`] — the measuring half of this
//! widget's TUI story, feeding [`super::cell_paint`] and
//! [`crate::primitive::Primitive::layout`]'s
//! [`crate::primitive::Units::Cells`] branch.
//!
//! TUI rows are a uniform one cell tall, so this measures each field
//! with a 1-cell-per-row closure (plus the extra rows a
//! [`LabelPosition::Top`] label, an open `Dropdown` popup, a
//! `TextArea`, a boxed `NumberInput` or a multi-option `RadioGroup`
//! need) and hands the resolved positions to [`Form::layout`], which
//! owns the scroll clamp and the hit regions.
//!
//! Lives next to the descriptor rather than in `crate::tui` so the
//! contract's cell layout and the legacy `Backend::form_layout` path
//! resolve through one body of code by construction;
//! [`crate::tui::form`] only re-exports these names.

use ratatui::layout::Rect;

use super::{
    FieldKind, Form, FormField, FormFieldMeasure, FormItemMeasure, LabelPosition,
};
use crate::primitives::toolbar::ToolbarButton;
use crate::types::WidgetId;


/// Shared label-column width (in cells) for every `Left`-positioned
/// field in `form`. An explicit [`Form::resolved_label_width`] on any
/// such field counts toward the max, so one wide override still pulls
/// the whole column wider; fields that leave it unset fall back to
/// their own label's width. Fields resolved to `Top`/`Inline`/`Hidden`
/// don't share this column and are excluded.
pub(crate) fn left_label_column_width(form: &Form) -> usize {
    form.fields
        .iter()
        .filter(|f| {
            !matches!(f.kind, FieldKind::Label)
                && form.resolved_label_position(f) == LabelPosition::Left
                && f.label.visible_width() > 0
        })
        .map(|f| {
            form.resolved_label_width(f)
                .map(|w| w.round().max(0.0) as usize)
                .unwrap_or_else(|| f.label.visible_width())
        })
        .max()
        .unwrap_or(0)
}

/// Whether `field` renders its label on its own row above the input
/// (only meaningful for fields with an actual input row — a bare
/// `FieldKind::Label` header has no separate input to split from).
pub(crate) fn is_top_label(form: &Form, field: &FormField) -> bool {
    !matches!(field.kind, FieldKind::Label)
        && form.resolved_label_position(field) == LabelPosition::Top
        && field.label.visible_width() > 0
}

/// Whether `field` is the form's current keyboard focus. Shared by the
/// measure closure (to grow a focused `Dropdown`'s row count) and
/// [`build_row_ctx`] (to pick focused-row colours), so both agree on
/// exactly when a `Dropdown`'s popup list is visible.
pub(crate) fn field_is_focused(form: &Form, field: &FormField) -> bool {
    form.has_focus
        && form
            .focused_field
            .as_ref()
            .is_some_and(|id| id == &field.id)
}

/// `field`'s control column at its own natural, unstretched size (in
/// cells) — what [`ControlWidth::Content`] resolves to, and the floor
/// [`ControlWidth::Fill`]/[`ControlWidth::Fixed`] still have to beat via
/// [`Form::resolved_min_control_width`]. Only field kinds whose control
/// paints a left-anchored box/value (every kind `draw_form` dispatches
/// outside the item-based `ToggleGroup`/`ButtonRow`/`Toolbar`/
/// `SegmentedControl` quartet, which already pack their items flush
/// left of the label and don't participate in this column) need an
/// entry; everything else returns `0` and is never consulted.
pub(crate) fn natural_content_width(field: &FormField) -> usize {
    match &field.kind {
        FieldKind::Toggle { .. } => 3,
        FieldKind::TextInput {
            value, placeholder, ..
        }
        | FieldKind::PasswordInput {
            value, placeholder, ..
        }
        | FieldKind::TextArea {
            value, placeholder, ..
        } => {
            let shown = if value.is_empty() {
                placeholder.as_str()
            } else {
                value.as_str()
            };
            shown.chars().count() + 2
        }
        FieldKind::Button => field.label.visible_width() + 4,
        FieldKind::ReadOnly { value } => value.visible_width(),
        FieldKind::Slider { value, .. } => 12 + 2 + format!("{value:.2}").chars().count() + 2,
        FieldKind::ColorPicker { .. } => 2 + 7,
        FieldKind::Dropdown {
            options,
            selected_idx,
        } => {
            options
                .get(*selected_idx)
                .map(|o| o.visible_width())
                .unwrap_or(0)
                + 4
        }
        FieldKind::ComboBox(combo) => combo.text.chars().count() + 2,
        FieldKind::DatePicker(picker) => {
            picker.focused.format(picker.locale.format).chars().count()
        }
        _ => 0,
    }
}

/// Resolve `field`'s control-column width in cells: the form's
/// [`Form::resolved_control_width`] mode applied against `available`
/// (remaining cells after the label column, for `Fill`) and `field`'s
/// own [`natural_content_width`] (for `Content`), floored by
/// [`Form::resolved_min_control_width`]. `Fixed` cells are already
/// whole numbers in the descriptor (the same dual-unit convention as
/// `label_width`), rounded up just in case a binding hands in a
/// fractional value.
pub(crate) fn resolved_control_width_cells(form: &Form, field: &FormField, available: usize) -> usize {
    let content_width = natural_content_width(field) as f32;
    let min_width = form.resolved_min_control_width(field);
    let width =
        form.resolved_control_width(field)
            .resolve(content_width, available as f32, min_width);
    (width.ceil().max(0.0) as usize).max(1)
}

/// X offset where a field's input/items start, after its resolved
/// label column (or 0/1 when there's no reserved column to clear).
/// `no_label_fallback` is the start column when the field has no
/// label to reserve space for (`0` for `Toolbar`, which packs flush
/// left; `1` for everything else, to leave room for the row's left
/// border decoration).
pub(crate) fn field_items_start_x(
    form: &Form,
    field: &FormField,
    left_label_width: usize,
    no_label_fallback: usize,
) -> f32 {
    let label_w = field.label.visible_width();
    if label_w == 0 {
        return no_label_fallback as f32;
    }
    match form.resolved_label_position(field) {
        LabelPosition::Hidden | LabelPosition::Top => no_label_fallback as f32,
        LabelPosition::Left => (left_label_width.max(label_w) + 2) as f32,
        LabelPosition::Inline => (label_w + 2) as f32,
    }
}

/// Track cells a `FieldKind::Slider` paints between `[` and `]`: its
/// control width minus both brackets, the 2-cell gap and the value text,
/// floored at 3.
pub(crate) fn track_cells_within(control_width: usize, value: f32) -> usize {
    let value_len = format!("{value:.2}").chars().count();
    control_width.saturating_sub(2 + 2 + value_len).max(3)
}

/// Compute the form layout using TUI cell metrics (1 cell per row,
/// char-count item widths). A field whose label resolves to
/// [`LabelPosition::Top`] measures one extra row for the label line.
pub fn tui_form_layout(form: &Form, area: Rect) -> crate::primitives::form::FormLayout {
    tui_form_layout_with_label_width(form, area, left_label_column_width(form))
}

/// Same as [`tui_form_layout`], but takes an already-computed
/// `left_label_width` so a caller that also needs that value (like
/// [`draw_form`]) only runs [`left_label_column_width`] once per frame.
pub(crate) fn tui_form_layout_with_label_width(
    form: &Form,
    area: Rect,
    left_label_width: usize,
) -> crate::primitives::form::FormLayout {
    form.layout(area.width as f32, area.height as f32, |i| {
        let field = &form.fields[i];
        let extra_row = if is_top_label(form, field) { 1.0 } else { 0.0 };
        match &field.kind {
            FieldKind::ToggleGroup { toggles } => {
                let start_x = field_items_start_x(form, field, left_label_width, 1);
                let items = toggles
                    .iter()
                    .map(|t| FormItemMeasure {
                        id: t.id,
                        width: t.label.chars().count() as f32,
                    })
                    .collect();
                FormFieldMeasure::with_items(1.0 + extra_row, start_x, 1.0, items)
            }
            FieldKind::ButtonRow { buttons } => {
                let start_x = field_items_start_x(form, field, left_label_width, 1);
                let items = buttons
                    .iter()
                    .map(|b| {
                        let icon_w = b
                            .icon
                            .as_ref()
                            .map(|i| {
                                let gw = i.fallback.chars().count();
                                if b.label.is_empty() {
                                    gw
                                } else {
                                    gw + 1
                                }
                            })
                            .unwrap_or(0);
                        FormItemMeasure {
                            id: b.id,
                            width: (b.label.chars().count() + icon_w + 2) as f32,
                        }
                    })
                    .collect();
                FormFieldMeasure::with_items(1.0 + extra_row, start_x, 1.0, items)
            }
            FieldKind::TextArea { visible_rows, .. } => {
                FormFieldMeasure::new(*visible_rows as f32 + extra_row)
            }
            // A focused `Dropdown` grows to fit its open option list (one
            // row per option) below the compact row; unfocused, it's the
            // same single row as every other field.
            FieldKind::Dropdown { options, .. }
                if field_is_focused(form, field) && !options.is_empty() =>
            {
                FormFieldMeasure::new(options.len() as f32 + 1.0 + extra_row)
            }
            FieldKind::Toolbar(toolbar) => {
                use crate::primitives::toolbar::cell_layout::tui_item_width;
                let start_x = field_items_start_x(form, field, left_label_width, 0);
                // Include ALL toolbar items (actions, separators, labels) in
                // item_measures so the sequential layout matches `draw_toolbar`'s
                // left-to-right packing. Separators and labels get the field's
                // id so clicks on them resolve to `FormHit::Field(field_id)`.
                let items = toolbar
                    .buttons
                    .iter()
                    .map(|btn| {
                        let id = match btn {
                            ToolbarButton::Action { id, .. } => *id,
                            _ => field.id,
                        };
                        FormItemMeasure {
                            id,
                            width: tui_item_width(btn),
                        }
                    })
                    .collect();
                // item_gap = 0: toolbar buttons pack edge-to-edge, same as
                // how `Toolbar::layout` places items.
                FormFieldMeasure::with_items(1.0 + extra_row, start_x, 0.0, items)
            }
            FieldKind::SegmentedControl { options, .. } => {
                let start_x = field_items_start_x(form, field, left_label_width, 1);
                let items = options
                    .iter()
                    .enumerate()
                    .map(|(idx, opt)| FormItemMeasure {
                        id: WidgetId::new(format!("{}__seg_{}", field.id.as_str(), idx)),
                        width: opt.chars().count() as f32,
                    })
                    .collect();
                FormFieldMeasure::with_items(1.0 + extra_row, start_x, 1.0, items)
            }
            // Unlike every compact `FieldKind` above, `NumberInput`'s row
            // reuses the real standalone `tui::draw_number_input` box
            // painter (see `draw_field_number_input`), which needs room
            // for a top border, a content line, and a bottom border — 3
            // cells, not 1. The whole control is a single item (carrying
            // the *field's* own id, not a synthetic one) purely so its
            // on-screen rect is available via `VisibleFormField::item_bounds`
            // for click routing, mirroring how `FormController` resolves
            // every other item-bearing kind's clicks.
            FieldKind::NumberInput(number) => {
                let start_x = field_items_start_x(form, field, left_label_width, 1);
                let width = (number_input_display_text(number).chars().count() + 2 + 3) as f32;
                let items = vec![FormItemMeasure {
                    id: field.id,
                    width,
                }];
                FormFieldMeasure::with_items(3.0 + extra_row, start_x, 0.0, items)
                    .with_items_start_y(extra_row)
            }
            // One item, the track alone (no brackets/value text): both
            // `draw_field_slider` and `FormController` click routing use it.
            FieldKind::Slider { value, .. } => {
                let start_x = field_items_start_x(form, field, left_label_width, 1);
                let content_w = (area.width as f32 - form.spacing.padding.max(0.0) * 2.0).max(0.0);
                let avail = (content_w.round() as usize).saturating_sub(start_x as usize);
                let cw = resolved_control_width_cells(form, field, avail);
                let width = track_cells_within(cw, *value) as f32;
                let items = vec![FormItemMeasure {
                    id: field.id,
                    width,
                }];
                FormFieldMeasure::with_items(1.0 + extra_row, start_x + 1.0, 0.0, items)
                    .with_items_start_y(extra_row)
            }
            // `RadioGroup` is the one `FieldKind` whose row spans one
            // sub-row per option (see `FormFieldMeasure::vertical`'s doc)
            // instead of staying a single compact row — there is no popup
            // to keep collapsed, so the real `RadioOption`s are always on
            // screen. Each item carries its *option's* own id, resolving
            // a click straight to `form_click_event`'s existing
            // `FieldKind::RadioGroup` arm the same way a `ToggleGroup`
            // item's click already does.
            FieldKind::RadioGroup(group) => {
                let start_x = field_items_start_x(form, field, left_label_width, 1);
                let items: Vec<FormItemMeasure> = group
                    .options
                    .iter()
                    .map(|o| FormItemMeasure {
                        id: o.id,
                        width: 1.0,
                    })
                    .collect();
                let rows = group.options.len().max(1) as f32;
                FormFieldMeasure::with_vertical_items(rows + extra_row, start_x, items)
                    .with_items_start_y(extra_row)
            }
            _ => FormFieldMeasure::new(1.0 + extra_row),
        }
    })
}

/// The text currently shown in `number`: the in-progress
/// [`crate::primitives::number_input::NumberInput::draft`] verbatim when
/// one is pending, otherwise `value` formatted to `precision` decimal
/// places — mirrors [`crate::tui::number_input`]'s own private copy (that
/// module isn't reachable from here without duplicating this one-liner,
/// since both are plain private helpers in sibling modules).
pub(crate) fn number_input_display_text(number: &crate::primitives::number_input::NumberInput) -> String {
    match &number.draft {
        Some(draft) => draft.clone(),
        None => format!("{:.*}", number.precision as usize, number.value),
    }
}
