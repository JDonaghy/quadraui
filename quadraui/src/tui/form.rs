//! TUI rasteriser for [`crate::Form`].
//!
//! Per D6: this function asks the primitive for a [`crate::FormLayout`]
//! using a uniform 1-cell-per-field measurer (TUI rows are always 1
//! cell tall) and paints the resolved positions verbatim.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{draw_styled_text, ratatui_color, set_cell};
use crate::primitives::form::{
    FieldKind, Form, FormField, FormFieldMeasure, FormItemMeasure, LabelAlign, LabelPosition,
    ValidationState,
};
use crate::primitives::toolbar::ToolbarButton;
use crate::theme::Theme;
use crate::types::{Decoration, WidgetId};

/// Shared label-column width (in cells) for every `Left`-positioned
/// field in `form`. An explicit [`Form::resolved_label_width`] on any
/// such field counts toward the max, so one wide override still pulls
/// the whole column wider; fields that leave it unset fall back to
/// their own label's width. Fields resolved to `Top`/`Inline`/`Hidden`
/// don't share this column and are excluded.
fn left_label_column_width(form: &Form) -> usize {
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
fn is_top_label(form: &Form, field: &FormField) -> bool {
    !matches!(field.kind, FieldKind::Label)
        && form.resolved_label_position(field) == LabelPosition::Top
        && field.label.visible_width() > 0
}

/// Whether `field` is the form's current keyboard focus. Shared by the
/// measure closure (to grow a focused `Dropdown`'s row count) and
/// [`build_row_ctx`] (to pick focused-row colours), so both agree on
/// exactly when a `Dropdown`'s popup list is visible.
fn field_is_focused(form: &Form, field: &FormField) -> bool {
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
fn natural_content_width(field: &FormField) -> usize {
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
fn resolved_control_width_cells(form: &Form, field: &FormField, available: usize) -> usize {
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
fn field_items_start_x(
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

/// Compute the form layout using TUI cell metrics (1 cell per row,
/// char-count item widths). A field whose label resolves to
/// [`LabelPosition::Top`] measures one extra row for the label line.
pub fn tui_form_layout(form: &Form, area: Rect) -> crate::primitives::form::FormLayout {
    tui_form_layout_with_label_width(form, area, left_label_column_width(form))
}

/// Same as [`tui_form_layout`], but takes an already-computed
/// `left_label_width` so a caller that also needs that value (like
/// [`draw_form`]) only runs [`left_label_column_width`] once per frame.
fn tui_form_layout_with_label_width(
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
                use super::toolbar::tui_item_width;
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
fn number_input_display_text(number: &crate::primitives::number_input::NumberInput) -> String {
    match &number.draft {
        Some(draft) => draft.clone(),
        None => format!("{:.*}", number.precision as usize, number.value),
    }
}

/// A row's horizontal content span within `area`, in cells — `offset`
/// from `area.x` (`FormSpacing::padding`, the same for every row) and
/// the row's own `width` (`VisibleFormField::bounds.width`, already
/// shrunk by the same padding on the right edge). [`build_row_ctx`]
/// adds `offset` on top of every label/control column it computes —
/// the item-bearing kinds (`ToggleGroup`/`ButtonRow`/`SegmentedControl`/
/// `Toolbar`) need no such adjustment here: their own
/// `VisibleFormField::item_bounds` already carry the same offset from
/// `Form::layout`, added to `ctx.area.x` directly in `draw_field_button_row`
/// and friends — adding it a second time by shifting `ctx.area` itself
/// would double-count it.
#[derive(Clone, Copy)]
struct RowX {
    offset: usize,
    width: usize,
}

/// Shared paint context for one form-field row, threaded through the
/// per-[`FieldKind`] `draw_field_*` helpers below so each stays a
/// small function with a handful of args instead of a giant match arm.
struct RowCtx<'a> {
    area: Rect,
    y: u16,
    default_fg: ratatui::style::Color,
    row_bg: ratatui::style::Color,
    field_fg: ratatui::style::Color,
    dim_fg: ratatui::style::Color,
    accent_fg: ratatui::style::Color,
    is_focused: bool,
    theme: &'a Theme,
    /// Whether this field has no reserved label column — an empty
    /// label, `Hidden`, or a `Top`-positioned field's own input row.
    /// Consulted only by [`draw_field_text_input`]/
    /// [`draw_field_password_input`], which paint their opening `[` one
    /// cell to the *left* of `control_start` in this case so the
    /// value's text content — not the bracket — lines up with
    /// `control_start`, matching a `Top` label's own column (see
    /// `form_label_placement_top_label_sits_above_its_value`).
    no_label: bool,
    /// Column where this field's control (checkbox / input / slider /
    /// dropdown / …) starts — right after the shared label column, the
    /// same for every field so every control lines up in one column
    /// regardless of `FieldKind` (D1's whole point). The label column's
    /// end plus one cell for a labeled field, `1` for one without.
    control_start: usize,
    /// This field's resolved control-column width in cells, from
    /// [`resolved_control_width_cells`] — `Fill` stretches to the row's
    /// right edge, `Fixed`/`Content` are exact/natural sizes, both
    /// floored by `Form::resolved_min_control_width`. Only consulted by
    /// `draw_field_*` helpers whose control can stretch (a checkbox's
    /// glyph, for instance, ignores it and just repositions to
    /// `control_start`).
    control_width: usize,
}

/// Draw a [`Form`] into `area` on `buf`.
pub fn draw_form(buf: &mut Buffer, area: Rect, form: &Form, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let colors = FormColors::from_theme(theme);
    let left_label_width = left_label_column_width(form);
    let layout = tui_form_layout_with_label_width(form, area, left_label_width);

    let area_bottom = area.y + area.height;
    for visible_field in &layout.visible_fields {
        let field = &form.fields[visible_field.field_idx];
        let base_y = area.y + visible_field.bounds.y.round() as u16;
        if base_y >= area_bottom {
            // Not even the label row fits in `area` — nothing to paint.
            continue;
        }
        let (label_y, input_y) = if is_top_label(form, field) {
            (base_y, base_y + 1)
        } else {
            (base_y, base_y)
        };
        // A `Top`-labeled field's input row can land one row past
        // `area`'s bottom edge even though its label row fit. Clamp the
        // row `build_row_ctx` clears/draws into to `label_y` in that
        // case so it never writes past `area`, and skip the per-kind
        // input paint below entirely.
        let input_fits = input_y < area_bottom;
        let row_x = RowX {
            offset: visible_field.bounds.x.round() as usize,
            width: visible_field.bounds.width.round() as usize,
        };
        let ctx = build_row_ctx(
            buf,
            area,
            label_y,
            if input_fits { input_y } else { label_y },
            form,
            field,
            theme,
            &colors,
            left_label_width,
            row_x,
        );

        if !input_fits {
            continue;
        }

        match &field.kind {
            FieldKind::Label => {
                // No separate input — label spans the row.
            }
            FieldKind::Toggle { value } => draw_field_toggle(buf, &ctx, *value),
            FieldKind::TextInput {
                value,
                placeholder,
                cursor,
                selection_anchor,
            } => draw_field_text_input(buf, &ctx, value, placeholder, *cursor, *selection_anchor),
            FieldKind::Button => draw_field_button(buf, &ctx, field),
            FieldKind::ReadOnly { value } => draw_field_read_only(buf, &ctx, value),
            FieldKind::Slider {
                value, min, max, ..
            } => draw_field_slider(buf, &ctx, *value, *min, *max),
            FieldKind::ColorPicker { value } => draw_field_color_picker(buf, &ctx, *value),
            FieldKind::Dropdown {
                options,
                selected_idx,
            } => draw_field_dropdown(buf, &ctx, options, *selected_idx),
            FieldKind::ToggleGroup { toggles } => {
                draw_field_toggle_group(buf, &ctx, visible_field, field, toggles)
            }
            FieldKind::ButtonRow { buttons } => {
                draw_field_button_row(buf, &ctx, visible_field, field, buttons)
            }
            FieldKind::Toolbar(toolbar) => draw_field_toolbar(buf, &ctx, visible_field, toolbar),
            FieldKind::DatePicker(picker) => draw_field_date_picker(buf, &ctx, picker),
            FieldKind::ComboBox(combo) => draw_field_combo_box(buf, &ctx, combo),
            FieldKind::TextArea {
                value,
                placeholder,
                cursor,
                visible_rows,
            } => draw_field_text_area(buf, &ctx, value, placeholder, *cursor, *visible_rows),
            FieldKind::PasswordInput {
                value,
                placeholder,
                cursor,
                mask_char,
            } => draw_field_password_input(buf, &ctx, value, placeholder, *cursor, *mask_char),
            FieldKind::SegmentedControl {
                options,
                selected_idx,
            } => draw_field_segmented_control(buf, &ctx, visible_field, options, *selected_idx),
            FieldKind::NumberInput(number) => {
                draw_field_number_input(buf, &ctx, visible_field, number)
            }
            FieldKind::RadioGroup(group) => draw_field_radio_group(buf, &ctx, visible_field, group),
        }

        paint_validation_indicator(buf, area, row_x.offset, input_y, ctx.row_bg, field, theme);
    }
}

/// Theme colours `draw_form` resolves once per call and threads into
/// every row via [`build_row_ctx`] — kept as a small struct rather than
/// nine positional args.
struct FormColors {
    bg: ratatui::style::Color,
    fg: ratatui::style::Color,
    hdr_fg: ratatui::style::Color,
    hdr_bg: ratatui::style::Color,
    sel_bg: ratatui::style::Color,
    dim_fg: ratatui::style::Color,
    accent_fg: ratatui::style::Color,
}

impl FormColors {
    fn from_theme(theme: &Theme) -> Self {
        Self {
            bg: ratatui_color(theme.tab_bar_bg),
            fg: ratatui_color(theme.foreground),
            hdr_fg: ratatui_color(theme.header_fg),
            hdr_bg: ratatui_color(theme.header_bg),
            sel_bg: ratatui_color(theme.selected_bg),
            dim_fg: ratatui_color(theme.muted_fg),
            accent_fg: ratatui_color(theme.accent_fg),
        }
    }
}

/// Paints one field row's background + label, and builds the [`RowCtx`]
/// the `draw_field_*` dispatch table below consumes. This is the
/// per-row "prelude" every `FieldKind` arm shares before its own paint.
///
/// `label_y`/`input_y` are the same row unless the field resolves to
/// [`LabelPosition::Top`], in which case the label is painted on
/// `label_y` and every `draw_field_*` helper paints the input itself
/// on `input_y` (one row below) via [`RowCtx::y`].
#[allow(clippy::too_many_arguments)]
fn build_row_ctx<'a>(
    buf: &mut Buffer,
    area: Rect,
    label_y: u16,
    input_y: u16,
    form: &Form,
    field: &crate::primitives::form::FormField,
    theme: &'a Theme,
    colors: &FormColors,
    left_label_width: usize,
    row_x: RowX,
) -> RowCtx<'a> {
    let is_focused = field_is_focused(form, field);
    let is_header = matches!(field.kind, FieldKind::Label);
    let RowX {
        offset: row_x_offset,
        width: row_width,
    } = row_x;

    let (default_fg, row_bg) = match (is_header, is_focused) {
        // `fg` (`theme.foreground`, the editor-body text
        // colour) has no guaranteed contrast against `selected_bg` — on
        // `vscode-light` both are near-black on mid-blue and the focused
        // row goes unreadable. `hdr_fg` (`theme.header_fg`) is `Theme`'s
        // documented on-selection foreground; `Tree` and
        // `native_surface_paint::paint` already pair it with
        // `selected_bg` the same way for their focused row.
        (_, true) => (colors.hdr_fg, colors.sel_bg),
        (true, false) => (colors.hdr_fg, colors.hdr_bg),
        (false, false) => (colors.fg, colors.bg),
    };

    let row_left = area.x + row_x_offset as u16;
    let row_right = row_left + row_width as u16;
    for x in row_left..row_right {
        set_cell(buf, x, input_y, ' ', default_fg, row_bg);
    }
    if label_y != input_y {
        for x in row_left..row_right {
            set_cell(buf, x, label_y, ' ', default_fg, row_bg);
        }
    }

    let field_fg = if field.disabled {
        colors.dim_fg
    } else {
        default_fg
    };

    // A bare header row (`FieldKind::Label`, no separate input) always
    // paints like `Inline`: there's no input to split the label from.
    let position = if is_header {
        LabelPosition::Inline
    } else {
        form.resolved_label_position(field)
    };
    let label_w = field.label.visible_width();

    let (label_end, no_label) = if label_w == 0 {
        (0usize, true)
    } else {
        match position {
            LabelPosition::Hidden => (0usize, true),
            LabelPosition::Top => {
                draw_styled_text(
                    buf,
                    area,
                    label_y,
                    row_x_offset + 1,
                    &field.label,
                    field_fg,
                    row_bg,
                    Decoration::Normal,
                    colors.dim_fg,
                );
                // The input row has no reserved label column of its own.
                (0usize, true)
            }
            LabelPosition::Left => {
                let col_width = left_label_width.max(label_w);
                let align_offset = match form.resolved_label_align(field) {
                    LabelAlign::Start => 0,
                    LabelAlign::Center => col_width.saturating_sub(label_w) / 2,
                    LabelAlign::End => col_width.saturating_sub(label_w),
                };
                draw_styled_text(
                    buf,
                    area,
                    input_y,
                    row_x_offset + 1 + align_offset,
                    &field.label,
                    field_fg,
                    row_bg,
                    Decoration::Normal,
                    colors.dim_fg,
                );
                // The reserved column's end, not the painted text's end,
                // so every row in the column leaves the same input start.
                (row_x_offset + 1 + col_width, false)
            }
            LabelPosition::Inline => {
                let end = draw_styled_text(
                    buf,
                    area,
                    input_y,
                    row_x_offset + 1,
                    &field.label,
                    field_fg,
                    row_bg,
                    Decoration::Normal,
                    colors.dim_fg,
                );
                (end, false)
            }
        }
    };

    // The control column starts right after the shared label column —
    // the same column for every field regardless of `FieldKind`, which
    // is what gives the row a single aligned control edge instead of
    // each kind separately hugging the row's right edge. Fields with no
    // reserved label column (`no_label`) start their control at
    // `row_x_offset + 1`, mirroring `field_items_start_x`'s
    // `no_label_fallback` for the item-based kinds (offset by the same
    // `FormSpacing::padding` every other column in this row honours).
    let control_start = if no_label {
        row_x_offset + 1
    } else {
        label_end + 1
    };
    let available = (row_x_offset + row_width).saturating_sub(control_start);
    let control_width = resolved_control_width_cells(form, field, available);

    RowCtx {
        area,
        y: input_y,
        default_fg,
        row_bg,
        field_fg,
        dim_fg: colors.dim_fg,
        accent_fg: colors.accent_fg,
        is_focused,
        theme,
        no_label,
        control_start,
        control_width,
    }
}

/// Render a colored prefix character at column 0 on the field's first
/// row when validation is set. This avoids adding extra rows that
/// would break TUI layout (all non-`TextArea` fields are 1 cell tall).
fn paint_validation_indicator(
    buf: &mut Buffer,
    area: Rect,
    row_x_offset: usize,
    y: u16,
    row_bg: ratatui::style::Color,
    field: &crate::primitives::form::FormField,
    theme: &Theme,
) {
    let Some(ref vs) = field.validation else {
        return;
    };
    let (indicator, v_fg) = match vs {
        ValidationState::Error(_) => ('!', ratatui_color(theme.error_fg)),
        ValidationState::Warning(_) => ('\u{26A0}', ratatui_color(theme.warning_fg)),
    };
    set_cell(
        buf,
        area.x + row_x_offset as u16,
        y,
        indicator,
        v_fg,
        row_bg,
    );
}

fn draw_field_toggle(buf: &mut Buffer, ctx: &RowCtx, value: bool) {
    let RowCtx {
        area,
        y,
        row_bg,
        field_fg,
        accent_fg,
        control_start,
        ..
    } = *ctx;

    let glyph = if value { "[x]" } else { "[ ]" };
    if control_start < area.width as usize {
        let input_fg = if value { accent_fg } else { field_fg };
        for (col, ch) in (control_start..).zip(glyph.chars()) {
            if col >= area.width as usize {
                break;
            }
            set_cell(buf, area.x + col as u16, y, ch, input_fg, row_bg);
        }
    }
}

fn draw_field_text_input(
    buf: &mut Buffer,
    ctx: &RowCtx,
    value: &str,
    placeholder: &str,
    cursor: Option<usize>,
    selection_anchor: Option<usize>,
) {
    let RowCtx {
        area,
        y,
        row_bg,
        field_fg,
        dim_fg,
        no_label,
        control_start,
        control_width,
        ..
    } = *ctx;

    let shown = if value.is_empty() { placeholder } else { value };
    let input_fg = if value.is_empty() { dim_fg } else { field_fg };
    let desired = control_width.saturating_sub(2).min(shown.chars().count());

    let (sel_lo, sel_hi) = if value.is_empty() {
        (0, 0)
    } else {
        match (cursor, selection_anchor) {
            (Some(c), Some(a)) if c != a => (c.min(a), c.max(a)),
            _ => (0, 0),
        }
    };

    // A no-label field's content starts exactly at `control_start` —
    // the bracket sits one cell to its *left* instead of taking
    // `control_start`'s own cell, so the value's text lines up with a
    // `Top` label's own column one row up. A labeled field has no such
    // column to match, so its bracket takes `control_start` itself.
    let (bracket_col, text_col) = if no_label {
        (control_start.saturating_sub(1), control_start)
    } else {
        (control_start, control_start + 1)
    };

    if control_start < area.width as usize {
        if bracket_col < area.width as usize {
            set_cell(buf, area.x + bracket_col as u16, y, '[', dim_fg, row_bg);
        }
        let mut byte = 0usize;
        for (col, ch) in (text_col..).zip(shown.chars().take(desired)) {
            if col >= area.width as usize {
                break;
            }
            let in_selection = sel_hi > sel_lo && byte >= sel_lo && byte < sel_hi;
            let (ch_fg, ch_bg) = if in_selection {
                (row_bg, input_fg)
            } else {
                (input_fg, row_bg)
            };
            set_cell(buf, area.x + col as u16, y, ch, ch_fg, ch_bg);
            byte += ch.len_utf8();
        }
        let closing_col =
            (control_start + control_width.max(2) - 1).min((area.width as usize).saturating_sub(1));
        set_cell(buf, area.x + closing_col as u16, y, ']', dim_fg, row_bg);

        if let Some(cur) = cursor {
            if !value.is_empty() {
                let mut byte = 0usize;
                let mut char_idx = 0usize;
                for ch in shown.chars().take(desired) {
                    if byte >= cur {
                        break;
                    }
                    byte += ch.len_utf8();
                    char_idx += 1;
                }
                let cursor_col = text_col + char_idx;
                if cursor_col < area.width as usize {
                    let ch = shown.chars().nth(char_idx).unwrap_or(' ');
                    set_cell(buf, area.x + cursor_col as u16, y, ch, row_bg, field_fg);
                }
            }
        }
    }
}

fn draw_field_button(buf: &mut Buffer, ctx: &RowCtx, field: &crate::primitives::form::FormField) {
    let RowCtx {
        area,
        y,
        default_fg,
        row_bg,
        field_fg,
        dim_fg,
        accent_fg,
        no_label,
        control_start,
        is_focused,
        ..
    } = *ctx;

    // A standalone `Button`'s caption *is* `field.label` — unlike every
    // other kind, there's no separate column-label/value split, so it
    // doesn't participate in the shared control column: it keeps its
    // own content-sized `<label>` box, right-anchored against the
    // row's edge.
    let label_end = if no_label { 0 } else { control_start - 1 };
    for x in area.x..area.x + (label_end as u16).min(area.width) {
        set_cell(buf, x, y, ' ', default_fg, row_bg);
    }
    let width = field.label.visible_width() + 4;
    let start_col = if no_label {
        1
    } else {
        (area.width as usize).saturating_sub(width + 1)
    };
    if start_col < area.width as usize {
        let brk_fg = if is_focused { accent_fg } else { dim_fg };
        let text_fg = if field.disabled { dim_fg } else { field_fg };
        set_cell(buf, area.x + start_col as u16, y, '<', brk_fg, row_bg);
        let after_lt = draw_styled_text(
            buf,
            area,
            y,
            start_col + 2,
            &field.label,
            text_fg,
            row_bg,
            Decoration::Normal,
            dim_fg,
        );
        if after_lt < area.width as usize {
            set_cell(buf, area.x + after_lt as u16, y, ' ', brk_fg, row_bg);
        }
        if after_lt + 1 < area.width as usize {
            set_cell(buf, area.x + (after_lt + 1) as u16, y, '>', brk_fg, row_bg);
        }
    }
}

fn draw_field_read_only(buf: &mut Buffer, ctx: &RowCtx, value: &crate::types::StyledText) {
    let RowCtx {
        area,
        y,
        row_bg,
        field_fg,
        dim_fg,
        control_start,
        ..
    } = *ctx;

    if control_start < area.width as usize {
        // Painted in `field_fg` (focus/disabled-aware), not
        // unconditionally in `dim_fg` (`theme.muted_fg`) — a focused
        // (`selected_bg`) row's value stays readable the same way
        // every other field kind here does. Mirrors the
        // `native_surface_paint::paint` fix.
        draw_styled_text(
            buf,
            area,
            y,
            control_start,
            value,
            field_fg,
            row_bg,
            Decoration::Normal,
            dim_fg,
        );
    }
}

fn draw_field_slider(buf: &mut Buffer, ctx: &RowCtx, value: f32, min: f32, max: f32) {
    let RowCtx {
        area,
        y,
        row_bg,
        field_fg,
        dim_fg,
        accent_fg,
        control_start,
        control_width,
        ..
    } = *ctx;

    let range = (max - min).max(f32::EPSILON);
    let frac = ((value - min) / range).clamp(0.0, 1.0);
    let value_str = format!("{value:.2}");
    // The track stretches to fill whatever the resolved `control_width`
    // leaves after its own brackets and value text — `Content` resolves
    // `control_width` from the same `12 + 2 + len + 2` budget
    // `natural_content_width` assumes, so this recovers exactly 12
    // there; `Fill`/`Fixed` grow or shrink the track instead.
    let track_cells = control_width
        .saturating_sub(2 + 2 + value_str.chars().count())
        .max(3);
    let filled = (frac * track_cells as f32).round() as usize;

    if control_start < area.width as usize {
        let mut col = control_start;
        set_cell(buf, area.x + col as u16, y, '[', dim_fg, row_bg);
        col += 1;
        for i in 0..track_cells {
            if col >= area.width as usize {
                break;
            }
            let ch = if i < filled { '=' } else { '-' };
            let fg = if i < filled { accent_fg } else { dim_fg };
            set_cell(buf, area.x + col as u16, y, ch, fg, row_bg);
            col += 1;
        }
        if col < area.width as usize {
            set_cell(buf, area.x + col as u16, y, ']', dim_fg, row_bg);
        }
        col += 3;
        for ch in value_str.chars() {
            if col >= area.width as usize {
                break;
            }
            set_cell(buf, area.x + col as u16, y, ch, field_fg, row_bg);
            col += 1;
        }
    }
}

fn draw_field_color_picker(buf: &mut Buffer, ctx: &RowCtx, value: crate::types::Color) {
    let RowCtx {
        area,
        y,
        row_bg,
        field_fg,
        control_start,
        ..
    } = *ctx;

    let hex = format!("#{:02x}{:02x}{:02x}", value.r, value.g, value.b);
    if control_start < area.width as usize {
        let swatch_fg = ratatui::style::Color::Rgb(value.r, value.g, value.b);
        set_cell(
            buf,
            area.x + control_start as u16,
            y,
            '\u{25A0}',
            swatch_fg,
            row_bg,
        );
        for (col, ch) in (control_start + 2..).zip(hex.chars()) {
            if col >= area.width as usize {
                break;
            }
            set_cell(buf, area.x + col as u16, y, ch, field_fg, row_bg);
        }
    }
}

fn draw_field_dropdown(
    buf: &mut Buffer,
    ctx: &RowCtx,
    options: &[crate::types::StyledText],
    selected_idx: usize,
) {
    let RowCtx {
        area,
        y,
        row_bg,
        field_fg,
        dim_fg,
        control_start,
        is_focused,
        theme,
        ..
    } = *ctx;

    let chosen = options.get(selected_idx).cloned().unwrap_or_default();
    if control_start < area.width as usize {
        draw_styled_text(
            buf,
            area,
            y,
            control_start,
            &chosen,
            field_fg,
            row_bg,
            Decoration::Normal,
            dim_fg,
        );
        let chev_col = control_start + chosen.visible_width() + 1;
        if chev_col < area.width as usize {
            set_cell(buf, area.x + chev_col as u16, y, '\u{25BE}', dim_fg, row_bg);
        }
    }

    if is_focused && !options.is_empty() {
        draw_dropdown_popup(buf, area, y, options, selected_idx, theme);
    }
}

/// Paints a focused [`FieldKind::Dropdown`]'s open option list, one row
/// per option, directly below its compact row — this is the in-canvas
/// popup the TUI backend shows instead of the GUI backends' app-drawn
/// one (`FieldKind::Dropdown`'s doc comment). Uses the same
/// `completion_*` theme tokens as
/// [`crate::tui::combo_box::draw_combo_box`]'s popup, since both are a
/// list of rows with one highlighted. There's no separate "open" or
/// "highlighted-but-uncommitted" state: `selected_idx` itself is both
/// what's shown and what Up/Down move (see
/// `compose::form_controller::FormController::handle_key`'s Dropdown
/// block), so the highlighted row is always exactly `selected_idx`.
fn draw_dropdown_popup(
    buf: &mut Buffer,
    area: Rect,
    y: u16,
    options: &[crate::types::StyledText],
    selected_idx: usize,
    theme: &Theme,
) {
    if area.width < 3 {
        return;
    }
    let pop_bg = ratatui_color(theme.completion_bg);
    let sel_bg = ratatui_color(theme.completion_selected_bg);
    let pop_fg = ratatui_color(theme.completion_fg);
    let border = ratatui_color(theme.completion_border);

    for (idx, option) in options.iter().enumerate() {
        let row_y = y + 1 + idx as u16;
        if row_y >= area.y + area.height {
            break;
        }
        let row_bg = if idx == selected_idx { sel_bg } else { pop_bg };
        for col in area.x..area.x + area.width {
            set_cell(buf, col, row_y, ' ', pop_fg, row_bg);
        }
        set_cell(buf, area.x, row_y, '\u{2502}', border, row_bg);
        set_cell(
            buf,
            area.x + area.width - 1,
            row_y,
            '\u{2502}',
            border,
            row_bg,
        );
        draw_styled_text(
            buf,
            area,
            row_y,
            2,
            option,
            pop_fg,
            row_bg,
            Decoration::Normal,
            pop_fg,
        );
    }
}

fn draw_field_toggle_group(
    buf: &mut Buffer,
    ctx: &RowCtx,
    visible_field: &crate::primitives::form::VisibleFormField,
    field: &crate::primitives::form::FormField,
    toggles: &[crate::primitives::form::ToggleGroupItem],
) {
    let RowCtx {
        area,
        y,
        row_bg,
        dim_fg,
        accent_fg,
        ..
    } = *ctx;

    for (item_id, item_rect) in &visible_field.item_bounds {
        let toggle = toggles.iter().find(|t| &t.id == item_id);
        if let Some(toggle) = toggle {
            let col = area.x as f32 + item_rect.x;
            let toggle_fg = if toggle.value && !field.disabled {
                accent_fg
            } else {
                dim_fg
            };
            for (i, ch) in toggle.label.chars().enumerate() {
                let cx = col as u16 + i as u16;
                if cx < area.x + area.width {
                    set_cell(buf, cx, y, ch, toggle_fg, row_bg);
                }
            }
        }
    }
}

fn draw_field_button_row(
    buf: &mut Buffer,
    ctx: &RowCtx,
    visible_field: &crate::primitives::form::VisibleFormField,
    field: &crate::primitives::form::FormField,
    buttons: &[crate::primitives::form::ButtonRowItem],
) {
    let RowCtx {
        area,
        y,
        row_bg,
        field_fg,
        dim_fg,
        accent_fg,
        theme,
        ..
    } = *ctx;

    for (item_id, item_rect) in &visible_field.item_bounds {
        let button = buttons.iter().find(|b| &b.id == item_id);
        if let Some(button) = button {
            let col = area.x as f32 + item_rect.x;
            // An explicit `spec` override's `variant` tints both the
            // label and the bracket chrome in the intent colour, same
            // convention `crate::tui::button` uses for a standalone
            // `Danger`/`Warning` `Button` — a `spec: None` item keeps
            // painting `field_fg`/`accent_fg`. Only `variant` is
            // honoured here today — `icon`/`icon_position`/`icon_gap`/
            // `tooltip` on the spec have no TUI form-row effect yet, and
            // `spec.disabled` does not override the item's own
            // `disabled` field below.
            let intent_fg = button.spec.as_ref().and_then(|s| match s.variant {
                crate::primitives::button::ButtonVariant::Danger => Some(theme.error_fg),
                crate::primitives::button::ButtonVariant::Warning => Some(theme.warning_fg),
                _ => None,
            });
            let btn_fg = if button.disabled || field.disabled {
                dim_fg
            } else if let Some(intent) = intent_fg {
                ratatui_color(intent)
            } else {
                field_fg
            };
            let brk_fg = if button.disabled || field.disabled {
                dim_fg
            } else if let Some(intent) = intent_fg {
                ratatui_color(intent)
            } else {
                accent_fg
            };
            let mut cx = col as u16;
            if cx < area.x + area.width {
                set_cell(buf, cx, y, '[', brk_fg, row_bg);
            }
            cx += 1;
            if let Some(ref icon) = button.icon {
                let glyph = icon.fallback.as_str();
                for ch in glyph.chars() {
                    if cx < area.x + area.width {
                        set_cell(buf, cx, y, ch, btn_fg, row_bg);
                    }
                    cx += 1;
                }
                if cx < area.x + area.width && !button.label.is_empty() {
                    set_cell(buf, cx, y, ' ', btn_fg, row_bg);
                    cx += 1;
                }
            }
            for ch in button.label.chars() {
                if cx < area.x + area.width {
                    set_cell(buf, cx, y, ch, btn_fg, row_bg);
                }
                cx += 1;
            }
            if cx < area.x + area.width {
                set_cell(buf, cx, y, ']', brk_fg, row_bg);
            }
        }
    }
}

/// Renders the embedded [`crate::primitives::date_picker::DatePicker`]'s
/// focused/selected date as a single right-aligned line of text — the row
/// stays one cell tall like every other field, so the full month grid never
/// appears here. Day-by-day keyboard navigation and selection still work;
/// they're just not visualized inline (see `FormController`, which drives
/// the picker's own `navigate` and emits `FormEvent::DateChanged`).
fn draw_field_date_picker(
    buf: &mut Buffer,
    ctx: &RowCtx,
    picker: &crate::primitives::date_picker::DatePicker,
) {
    let RowCtx {
        area,
        y,
        row_bg,
        field_fg,
        control_start,
        is_focused,
        ..
    } = *ctx;

    // While this row is focused, show the live scrub position
    // `FormController` overlays per keystroke rather than the
    // app-confirmed `selected` — see `compose::form_controller`'s
    // "Date picker fields" doc section. Otherwise fall back to
    // `selected`, or `focused` before any date has been confirmed.
    let date = if is_focused {
        picker.focused
    } else {
        picker.selected.unwrap_or(picker.focused)
    };
    let text = date.format(picker.locale.format);
    if control_start < area.width as usize {
        for (col, ch) in (control_start..).zip(text.chars()) {
            if col >= area.width as usize {
                break;
            }
            set_cell(buf, area.x + col as u16, y, ch, field_fg, row_bg);
        }
    }
}

/// Renders the embedded [`crate::primitives::combo_box::ComboBox`]'s live
/// `text` buffer plus a chevron as a single right-aligned line — the row
/// stays one cell tall like every other field, so the open dropdown list
/// never appears here. Type-to-filter keystrokes still work; they're just
/// not visualized inline (see `FormController`, which drives the combo
/// box's own `handle_key` and emits `FormEvent::ComboBoxTextChanged` /
/// `ComboBoxCommitted`). Painting the open dropdown itself is the app's
/// job, mirroring `FieldKind::ColorPicker`'s popup contract.
fn draw_field_combo_box(
    buf: &mut Buffer,
    ctx: &RowCtx,
    combo: &crate::primitives::combo_box::ComboBox,
) {
    let RowCtx {
        area,
        y,
        row_bg,
        field_fg,
        dim_fg,
        control_start,
        ..
    } = *ctx;

    if control_start < area.width as usize {
        let mut col = control_start;
        for ch in combo.text.chars() {
            if col >= area.width as usize {
                break;
            }
            set_cell(buf, area.x + col as u16, y, ch, field_fg, row_bg);
            col += 1;
        }
        col += 1;
        if col < area.width as usize {
            set_cell(buf, area.x + col as u16, y, '\u{25BE}', dim_fg, row_bg);
        }
    }
}

/// Renders the embedded [`crate::primitives::number_input::NumberInput`]
/// through the real [`crate::tui::draw_number_input`] rasteriser — the
/// bordered box plus stepper buttons, not a compact summary, since
/// (unlike `DatePicker`/`ComboBox`) there is no popup to keep collapsed.
/// `visible_field.item_bounds`'s single entry (populated by
/// `tui_form_layout_with_label_width`'s `FieldKind::NumberInput` arm) is
/// the control's on-screen rect — the same lookup `FormController`'s click
/// routing uses, so paint and click always agree.
fn draw_field_number_input(
    buf: &mut Buffer,
    ctx: &RowCtx,
    visible_field: &crate::primitives::form::VisibleFormField,
    number: &crate::primitives::number_input::NumberInput,
) {
    let RowCtx {
        area,
        y,
        default_fg,
        row_bg,
        theme,
        ..
    } = *ctx;

    let Some((_, item_rect)) = visible_field.item_bounds.first() else {
        return;
    };
    let rows = item_rect.height.round().max(1.0) as u16;
    // Row 0 was already cleared by `build_row_ctx`'s single-row fill;
    // clear the rest of the control's rows here, the same pattern
    // `draw_field_text_area` uses for its own multi-row field.
    for r in 1..rows {
        let row_y = y + r;
        if row_y >= area.y + area.height {
            break;
        }
        for x in area.x..area.x + area.width {
            set_cell(buf, x, row_y, ' ', default_fg, row_bg);
        }
    }

    let abs_x = area.x as f32 + item_rect.x;
    let rect = crate::event::Rect::new(abs_x, y as f32, item_rect.width, rows as f32);
    let layout = number.layout(rect);
    super::draw_number_input(buf, number, &layout, theme);
}

/// Renders the embedded [`crate::primitives::radio_group::RadioGroup`]
/// through the real [`crate::tui::draw_radio_group`] rasteriser, one row
/// per option — `visible_field.item_bounds` (populated by
/// `tui_form_layout_with_label_width`'s `FieldKind::RadioGroup` arm, one
/// entry per [`crate::primitives::radio_group::RadioOption`]) gives the
/// control's on-screen rect via its first entry; `FormController`'s click
/// routing resolves a click straight to the clicked option's own entry.
fn draw_field_radio_group(
    buf: &mut Buffer,
    ctx: &RowCtx,
    visible_field: &crate::primitives::form::VisibleFormField,
    group: &crate::primitives::radio_group::RadioGroup,
) {
    let RowCtx {
        area,
        y,
        default_fg,
        row_bg,
        theme,
        is_focused,
        ..
    } = *ctx;

    if visible_field.item_bounds.is_empty() {
        return;
    }
    let rows = visible_field.item_bounds.len() as u16;
    // Row 0 was already cleared by `build_row_ctx`'s single-row fill;
    // clear the remaining option rows here.
    for r in 1..rows {
        let row_y = y + r;
        if row_y >= area.y + area.height {
            break;
        }
        for x in area.x..area.x + area.width {
            set_cell(buf, x, row_y, ' ', default_fg, row_bg);
        }
    }

    // While this row is focused, the embedded group's own `focused` flag
    // drives the selected option's marker colour (see
    // `tui::radio_group::draw_radio_group`) — the app isn't expected to
    // mirror `Form::focused_field` onto the embedded primitive itself,
    // the same contract `FormController`'s `ComboBox`/`DatePicker`
    // handling already documents.
    let mut group = group.clone();
    group.focused = is_focused;

    let (_, first_rect) = &visible_field.item_bounds[0];
    let abs_x = (area.x as f32 + first_rect.x).round() as u16;
    let width = first_rect.width.round() as u16;
    let radio_area = Rect::new(abs_x, y, width, rows);
    super::draw_radio_group(buf, radio_area, &group, theme);
}

fn draw_field_toolbar(
    buf: &mut Buffer,
    ctx: &RowCtx,
    visible_field: &crate::primitives::form::VisibleFormField,
    toolbar: &crate::primitives::toolbar::Toolbar,
) {
    let RowCtx { area, y, theme, .. } = *ctx;

    // Delegate painting entirely to the toolbar rasteriser.
    // The toolbar occupies the portion of the row after the
    // label (items_start_x). We derive the start column from
    // the first item's form-local x coordinate.
    let start_col = visible_field
        .item_bounds
        .first()
        .map(|(_, r)| r.x.round() as u16)
        .unwrap_or(0);
    let toolbar_area = Rect::new(
        area.x + start_col,
        y,
        area.width.saturating_sub(start_col),
        1,
    );
    super::toolbar::draw_toolbar(buf, toolbar_area, toolbar, theme, None, None);
}

fn draw_field_text_area(
    buf: &mut Buffer,
    ctx: &RowCtx,
    value: &str,
    placeholder: &str,
    cursor: Option<usize>,
    visible_rows: usize,
) {
    let RowCtx {
        area,
        y,
        default_fg,
        row_bg,
        field_fg,
        dim_fg,
        no_label,
        control_start,
        control_width,
        ..
    } = *ctx;

    let shown = if value.is_empty() { placeholder } else { value };
    let input_fg = if value.is_empty() { dim_fg } else { field_fg };
    let rows = visible_rows;

    // Shares `draw_field_text_input`'s bracket/text column split so a
    // `TextArea` joins the same control column as every other field
    // kind instead of spanning the full row width over the label.
    let (bracket_col, text_col) = if no_label {
        (control_start.saturating_sub(1), control_start)
    } else {
        (control_start, control_start + 1)
    };
    let closing_col =
        (control_start + control_width.max(2) - 1).min((area.width as usize).saturating_sub(1));
    let avail_w = closing_col.saturating_sub(text_col);

    let mut chars_iter = shown.chars();
    for row in 0..rows {
        let row_y = y + row as u16;
        if row_y >= area.y + area.height {
            break;
        }
        // Clear the row background (rows > 0 weren't cleared by
        // the initial row-clear above).
        if row > 0 {
            for x in area.x..area.x + area.width {
                set_cell(buf, x, row_y, ' ', default_fg, row_bg);
            }
        }

        if control_start >= area.width as usize {
            continue;
        }

        if bracket_col < area.width as usize {
            set_cell(buf, area.x + bracket_col as u16, row_y, '[', dim_fg, row_bg);
        }

        // Content characters for this row.
        let mut col = text_col;
        for _ in 0..avail_w {
            if let Some(ch) = chars_iter.next() {
                if col < area.width as usize {
                    set_cell(buf, area.x + col as u16, row_y, ch, input_fg, row_bg);
                }
                col += 1;
            } else {
                break;
            }
        }

        if closing_col < area.width as usize {
            set_cell(buf, area.x + closing_col as u16, row_y, ']', dim_fg, row_bg);
        }
    }

    // Cursor rendering on the first row (same logic as TextInput).
    if let Some(cur) = cursor {
        if !value.is_empty() {
            let mut byte = 0usize;
            let mut char_idx = 0usize;
            for ch in value.chars().take(avail_w) {
                if byte >= cur {
                    break;
                }
                byte += ch.len_utf8();
                char_idx += 1;
            }
            let cursor_col = text_col + char_idx;
            if cursor_col < area.width as usize {
                let ch = value.chars().nth(char_idx).unwrap_or(' ');
                set_cell(buf, area.x + cursor_col as u16, y, ch, row_bg, field_fg);
            }
        }
    }
}

fn draw_field_password_input(
    buf: &mut Buffer,
    ctx: &RowCtx,
    value: &str,
    placeholder: &str,
    cursor: Option<usize>,
    mask_char: char,
) {
    let RowCtx {
        area,
        y,
        row_bg,
        field_fg,
        dim_fg,
        no_label,
        control_start,
        control_width,
        ..
    } = *ctx;

    // Mask the value: replace each character with mask_char.
    let masked: String = value.chars().map(|_| mask_char).collect();
    let shown = if value.is_empty() {
        placeholder
    } else {
        masked.as_str()
    };
    let input_fg = if value.is_empty() { dim_fg } else { field_fg };
    let desired = control_width.saturating_sub(2).min(shown.chars().count());

    // See `draw_field_text_input`'s matching comment: a no-label
    // field's value text lines up with `control_start` itself, with
    // the bracket one cell to its left.
    let (bracket_col, text_col) = if no_label {
        (control_start.saturating_sub(1), control_start)
    } else {
        (control_start, control_start + 1)
    };

    if control_start < area.width as usize {
        if bracket_col < area.width as usize {
            set_cell(buf, area.x + bracket_col as u16, y, '[', dim_fg, row_bg);
        }
        for (col, ch) in (text_col..).zip(shown.chars().take(desired)) {
            if col >= area.width as usize {
                break;
            }
            set_cell(buf, area.x + col as u16, y, ch, input_fg, row_bg);
        }
        let closing_col =
            (control_start + control_width.max(2) - 1).min((area.width as usize).saturating_sub(1));
        set_cell(buf, area.x + closing_col as u16, y, ']', dim_fg, row_bg);

        // Cursor (byte offset into original value, rendered at
        // the masked character position).
        if let Some(cur) = cursor {
            if !value.is_empty() {
                let mut byte = 0usize;
                let mut char_idx = 0usize;
                for ch in value.chars().take(desired) {
                    if byte >= cur {
                        break;
                    }
                    byte += ch.len_utf8();
                    char_idx += 1;
                }
                let cursor_col = text_col + char_idx;
                if cursor_col < area.width as usize {
                    let ch = masked.chars().nth(char_idx).unwrap_or(' ');
                    set_cell(buf, area.x + cursor_col as u16, y, ch, row_bg, field_fg);
                }
            }
        }
    }
}

fn draw_field_segmented_control(
    buf: &mut Buffer,
    ctx: &RowCtx,
    visible_field: &crate::primitives::form::VisibleFormField,
    options: &[String],
    selected_idx: usize,
) {
    let RowCtx {
        area,
        y,
        row_bg,
        dim_fg,
        accent_fg,
        ..
    } = *ctx;

    // Render as [opt1|opt2|opt3] using item_bounds for positioning.
    // If item_bounds are available, use them; otherwise fall back to
    // sequential rendering.
    if !visible_field.item_bounds.is_empty() {
        // Opening bracket before first item.
        let first_x = visible_field.item_bounds[0].1.x;
        let bracket_x = (area.x as f32 + first_x - 1.0).max(area.x as f32);
        if (bracket_x as u16) < area.x + area.width {
            set_cell(buf, bracket_x as u16, y, '[', dim_fg, row_bg);
        }

        for (i, (_item_id, item_rect)) in visible_field.item_bounds.iter().enumerate() {
            let opt = options.get(i).map(|s| s.as_str()).unwrap_or("");
            let opt_fg = if i == selected_idx { accent_fg } else { dim_fg };
            let col = area.x as f32 + item_rect.x;
            for (j, ch) in opt.chars().enumerate() {
                let cx = col as u16 + j as u16;
                if cx < area.x + area.width {
                    set_cell(buf, cx, y, ch, opt_fg, row_bg);
                }
            }

            // Separator '|' after each option except the last.
            if i + 1 < options.len() {
                let sep_x = col as u16 + opt.chars().count() as u16;
                if sep_x < area.x + area.width {
                    set_cell(buf, sep_x, y, '|', dim_fg, row_bg);
                }
            }
        }

        // Closing bracket after last item.
        if let Some((_last_id, last_rect)) = visible_field.item_bounds.last() {
            let last_opt_len = options.last().map(|s| s.chars().count()).unwrap_or(0);
            let close_x = area.x as f32 + last_rect.x + last_opt_len as f32;
            if (close_x as u16) < area.x + area.width {
                set_cell(buf, close_x as u16, y, ']', dim_fg, row_bg);
            }
        }
    }
}

/// Settings panel chrome: a 2-row strip with a header row and a search
/// input row, designed to sit immediately above a [`Form`] body.
///
/// `area` must be at least 2 rows tall — the first row is the header
/// (`header_bg` / `header_fg`), the second row is the search input
/// (full-width tinted `selected_bg` when `active`, otherwise the panel
/// `tab_bar_bg`). Layout from left to right inside the search row:
/// ` `, `/`, ` `, then either `query` (in `foreground`) or `placeholder`
/// (in `muted_fg`) when the query is empty. A 1-cell `█` cursor in
/// `accent_fg` follows the query when `active`.
///
/// Chrome only — the form body and any scrollbar layered below are
/// painted separately by the caller.
pub fn draw_settings_chrome(
    buf: &mut Buffer,
    area: Rect,
    header_text: &str,
    query: &str,
    placeholder: &str,
    active: bool,
    theme: &Theme,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let panel_bg = ratatui_color(theme.tab_bar_bg);
    let header_bg = ratatui_color(theme.header_bg);
    let header_fg = ratatui_color(theme.header_fg);
    let foreground = ratatui_color(theme.foreground);
    let muted_fg = ratatui_color(theme.muted_fg);
    let selected_bg = ratatui_color(theme.selected_bg);
    let accent_fg = ratatui_color(theme.accent_fg);

    // Row 0: header.
    let header_y = area.y;
    for x in area.x..area.x + area.width {
        set_cell(buf, x, header_y, ' ', header_fg, header_bg);
    }
    for (x, ch) in (area.x..).zip(header_text.chars()) {
        if x >= area.x + area.width {
            break;
        }
        set_cell(buf, x, header_y, ch, header_fg, header_bg);
    }

    if area.height < 2 {
        return;
    }

    // Row 1: search input.
    let search_y = area.y + 1;
    let row_bg = if active { selected_bg } else { panel_bg };
    for x in area.x..area.x + area.width {
        set_cell(buf, x, search_y, ' ', foreground, row_bg);
    }

    let mut x = area.x;
    set_cell(buf, x, search_y, ' ', muted_fg, row_bg);
    x += 1;
    if x < area.x + area.width {
        set_cell(buf, x, search_y, '/', muted_fg, row_bg);
        x += 1;
    }
    if x < area.x + area.width {
        set_cell(buf, x, search_y, ' ', muted_fg, row_bg);
        x += 1;
    }

    let show_placeholder = query.is_empty() && !placeholder.is_empty() && !active;
    let (text, fg) = if show_placeholder {
        (placeholder, muted_fg)
    } else {
        (query, foreground)
    };
    for ch in text.chars() {
        if x >= area.x + area.width {
            break;
        }
        set_cell(buf, x, search_y, ch, fg, row_bg);
        x += 1;
    }

    if active && x < area.x + area.width {
        set_cell(buf, x, search_y, '█', accent_fg, row_bg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::form::{FieldKind, Form, FormField};
    use crate::types::{StyledSpan, StyledText, WidgetId};

    fn label(text: &str) -> StyledText {
        StyledText {
            spans: vec![StyledSpan::plain(text)],
        }
    }

    fn make_form() -> Form {
        Form {
            id: WidgetId::new("settings"),
            fields: vec![
                FormField {
                    id: WidgetId::new("hdr"),
                    label: label("Editor"),
                    kind: FieldKind::Label,
                    disabled: false,
                    validation: None,
                    hint: label(""),
                    label_position: None,
                    label_align: None,
                    label_width: None,
                    tab_index: None,
                    control_width: None,
                    min_control_width: None,
                    row_align: None,
                },
                FormField {
                    id: WidgetId::new("wrap"),
                    label: label("wrap"),
                    kind: FieldKind::Toggle { value: true },
                    disabled: false,
                    validation: None,
                    hint: label(""),
                    label_position: None,
                    label_align: None,
                    label_width: None,
                    tab_index: None,
                    control_width: None,
                    min_control_width: None,
                    row_align: None,
                },
            ],
            focused_field: Some(WidgetId::new("wrap")),
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

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    #[test]
    fn paints_label_and_toggle_glyph() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
        let f = make_form();
        draw_form(&mut buf, Rect::new(0, 0, 30, 5), &f, &Theme::default());

        // Header row: "Editor" starts at col 1 (label_col).
        let row0: String = (1..7).map(|x| cell_char(&buf, x, 0)).collect();
        assert_eq!(row0, "Editor");

        // Toggle row: "wrap" label, then "[x]" right after the shared
        // label column — not hugging the row's right edge.
        let row1: String = (0..5).map(|x| cell_char(&buf, x, 1)).collect();
        assert!(row1.contains("wrap"));
        let mut found_x = false;
        for x in 5..10 {
            if cell_char(&buf, x, 1) == 'x' {
                found_x = true;
            }
        }
        assert!(found_x, "expected '[x]' right after the label column");
    }

    #[test]
    fn focused_row_uses_selected_bg() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
        let f = make_form();
        let theme = Theme {
            selected_bg: crate::types::Color::rgb(99, 0, 0),
            ..Theme::default()
        };
        draw_form(&mut buf, Rect::new(0, 0, 30, 5), &f, &theme);
        // Row 1 ("wrap", focused) bg should be (99, 0, 0).
        let bg = buf[(0u16, 1u16)].bg;
        assert_eq!(bg, ratatui::style::Color::Rgb(99, 0, 0));
    }

    fn make_dropdown_form(focused: bool) -> Form {
        Form {
            id: WidgetId::new("settings"),
            fields: vec![
                FormField {
                    id: WidgetId::new("hdr"),
                    label: label("Editor"),
                    kind: FieldKind::Label,
                    disabled: false,
                    validation: None,
                    hint: label(""),
                    label_position: None,
                    label_align: None,
                    label_width: None,
                    tab_index: None,
                    control_width: None,
                    min_control_width: None,
                    row_align: None,
                },
                FormField {
                    id: WidgetId::new("dd"),
                    label: label(""),
                    kind: FieldKind::Dropdown {
                        options: vec![label("alpha"), label("beta"), label("gamma")],
                        selected_idx: 1,
                    },
                    disabled: false,
                    validation: None,
                    hint: label(""),
                    label_position: None,
                    label_align: None,
                    label_width: None,
                    tab_index: None,
                    control_width: None,
                    min_control_width: None,
                    row_align: None,
                },
            ],
            focused_field: if focused {
                Some(WidgetId::new("dd"))
            } else {
                None
            },
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
    fn focused_dropdown_paints_its_open_option_list() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 6));
        let f = make_dropdown_form(true);
        let theme = Theme::default();
        draw_form(&mut buf, Rect::new(0, 0, 30, 6), &f, &theme);

        // Compact row (y=1) still shows the selected option ("beta"),
        // same as an unfocused Dropdown — this field's label is empty
        // so its control starts at column 1.
        let row1: String = (1..10).map(|x| cell_char(&buf, x, 1)).collect();
        assert!(row1.contains("beta"), "row1={row1:?}");

        // Popup rows start directly below the compact row: one row per
        // option, each spanning the field's full width with a left/right
        // border.
        assert_eq!(cell_char(&buf, 0, 2), '\u{2502}');
        assert_eq!(cell_char(&buf, 2, 2), 'a'); // "alpha" row
        assert_eq!(cell_char(&buf, 2, 3), 'b'); // "beta" row (selected)
        assert_eq!(cell_char(&buf, 2, 4), 'g'); // "gamma" row

        // The selected row (idx 1, "beta") uses completion_selected_bg;
        // the others use plain completion_bg.
        let selected_bg = buf[(0u16, 3u16)].bg;
        let plain_bg = buf[(0u16, 2u16)].bg;
        assert_eq!(
            selected_bg,
            crate::tui::ratatui_color(theme.completion_selected_bg)
        );
        assert_eq!(plain_bg, crate::tui::ratatui_color(theme.completion_bg));
        assert_ne!(selected_bg, plain_bg);
    }

    #[test]
    fn unfocused_dropdown_paints_no_popup() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 6));
        let f = make_dropdown_form(false);
        draw_form(&mut buf, Rect::new(0, 0, 30, 6), &f, &Theme::default());

        // Compact row still shows the selected option, starting at
        // column 1 (this field's label is empty).
        let row1: String = (1..10).map(|x| cell_char(&buf, x, 1)).collect();
        assert!(row1.contains("beta"), "row1={row1:?}");

        // No popup rows are painted below the compact row — they stay
        // whatever `Buffer::empty` default-fills them with (blank).
        assert_eq!(cell_char(&buf, 2, 2), ' ');
        assert_eq!(cell_char(&buf, 2, 3), ' ');
    }

    /// A focused row painted its label in `theme.foreground`
    /// — no guaranteed contrast against `selected_bg` (both near-black on
    /// mid-blue under `vscode-light`) — while `Tree`'s equivalent focused
    /// row already used `theme.header_fg`. Also covers the `ReadOnly` field
    /// kind, whose value was unconditionally painted in `theme.muted_fg`
    /// regardless of focus — the second half of the issue's "value is
    /// unreadable" report. Mirrors
    /// `primitives::form::native_surface_paint::tests::
    /// focused_row_paints_label_and_value_in_header_fg_not_foreground_or_muted_fg`,
    /// but against the TUI rasteriser (`draw_form`), which is a separate
    /// implementation from `native_surface_paint::paint` and was not fixed
    /// by that change.
    #[test]
    fn focused_row_paints_label_and_value_in_header_fg_not_foreground_or_muted_fg() {
        let f = Form {
            id: WidgetId::new("f"),
            fields: vec![FormField {
                id: WidgetId::new("note"),
                label: label("Note"),
                kind: FieldKind::ReadOnly {
                    value: label("hello"),
                },
                disabled: false,
                validation: None,
                hint: label(""),
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }],
            focused_field: Some(WidgetId::new("note")),
            scroll_offset: 0,
            has_focus: true,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };
        // `Theme::default()` sets `header_fg == foreground` (both the
        // neutral dark `fg` literal), which would make this test pass
        // whether `draw_form` used `header_fg` or `foreground` — exactly
        // the ambiguity the buggy code exploited. Use a theme (mirroring
        // `vscode-light`'s shape) where `header_fg`, `foreground` and
        // `muted_fg` are three distinct colours so the assertions below
        // actually discriminate between them.
        let theme = Theme {
            foreground: crate::types::Color::rgb(10, 10, 10),
            header_fg: crate::types::Color::rgb(250, 250, 250),
            muted_fg: crate::types::Color::rgb(0, 128, 128),
            ..Theme::default()
        };
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 3));
        draw_form(&mut buf, Rect::new(0, 0, 30, 3), &f, &theme);

        let hdr_fg = ratatui_color(theme.header_fg);

        // "Note" label starts at col 1 (label_col).
        let label_fg = buf[(1u16, 0u16)].fg;
        assert_eq!(
            label_fg, hdr_fg,
            "focused row's label must use theme.header_fg (Tree's \
             on-selection convention), not theme.foreground"
        );

        // "hello" value is right-aligned; find its start column.
        let mut hello_col = None;
        for x in 0..30u16 {
            if cell_char(&buf, x, 0) == 'h'
                && x + 4 < 30
                && cell_char(&buf, x + 1, 0) == 'e'
                && cell_char(&buf, x + 2, 0) == 'l'
            {
                hello_col = Some(x);
                break;
            }
        }
        let hello_col = hello_col.expect("'hello' value must be painted");
        let value_fg = buf[(hello_col, 0u16)].fg;
        assert_eq!(
            value_fg, hdr_fg,
            "a focused ReadOnly row's value must track the row's \
             on-selection foreground, not an unconditional theme.muted_fg"
        );
    }

    #[test]
    fn disabled_field_uses_muted_fg() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
        let mut f = make_form();
        f.fields[1].disabled = true;
        f.has_focus = false;
        let theme = Theme {
            muted_fg: crate::types::Color::rgb(50, 50, 50),
            ..Theme::default()
        };
        draw_form(&mut buf, Rect::new(0, 0, 30, 5), &f, &theme);
        // 'w' of "wrap" should be in muted_fg.
        let fg = buf[(1u16, 1u16)].fg;
        assert_eq!(fg, ratatui::style::Color::Rgb(50, 50, 50));
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
        let f = make_form();
        draw_form(&mut buf, Rect::new(0, 0, 0, 5), &f, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    #[test]
    fn text_input_no_label_spans_full_width() {
        let f = Form {
            id: WidgetId::new("search"),
            fields: vec![FormField {
                id: WidgetId::new("query"),
                label: label(""),
                kind: FieldKind::TextInput {
                    value: "test".into(),
                    placeholder: "Search…".into(),
                    cursor: Some(4),
                    selection_anchor: None,
                },
                disabled: false,
                validation: None,
                hint: label(""),
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 3));
        draw_form(&mut buf, Rect::new(0, 0, 30, 3), &f, &Theme::default());

        // No label to reserve a column for: '[' at col 0, text at col
        // 1 (so the value lines up with a `Top` label's own column one
        // row up), and — `Fill` being the default `control_width` —
        // ']' stretches to the row's right edge.
        let row: String = (0..30).map(|x| cell_char(&buf, x, 0)).collect();
        assert!(
            row.starts_with("[test"),
            "expected full-width '[test...' but got: {row:?}"
        );
        assert_eq!(cell_char(&buf, 29, 0), ']', "expected ']' at right edge");
    }

    // ── ToggleGroup paint↔click round-trip ──────────────────────────────

    use crate::primitives::form::{FormHit, ToggleGroupItem};

    fn make_toggle_group_form() -> Form {
        Form {
            id: WidgetId::new("search"),
            fields: vec![FormField {
                id: WidgetId::new("opts"),
                label: label(""),
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
                        ToggleGroupItem {
                            id: WidgetId::new("regex"),
                            label: ".*".into(),
                            value: false,
                        },
                    ],
                },
                disabled: false,
                validation: None,
                hint: label(""),
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        }
    }

    #[test]
    fn toggle_group_paints_labels() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 3));
        let f = make_toggle_group_form();
        draw_form(&mut buf, Rect::new(0, 0, 40, 3), &f, &Theme::default());

        let row: String = (0..40).map(|x| cell_char(&buf, x, 0)).collect();
        assert!(row.contains("Aa"), "expected 'Aa' in row: {row:?}");
        assert!(row.contains("Ab|"), "expected 'Ab|' in row: {row:?}");
        assert!(row.contains(".*"), "expected '.*' in row: {row:?}");
    }

    /// Shared body for the ToggleGroup paint↔click round trip, run at both
    /// the origin and a non-zero `area` origin. `tui_form_layout`/`Form::layout`
    /// never read `area.x`/`area.y` — the returned layout is field-local by
    /// construction, even though `draw_form` separately adds `area.x`/`area.y`
    /// per cell when painting. So `hit_test` always takes LOCAL coordinates
    /// (painted column minus `area.x`), regardless of where the form is painted.
    fn toggle_group_click_hits_correct_item_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 40, 3);
        let mut buf = Buffer::empty(Rect::new(0, 0, origin_x + 40, origin_y + 3));
        let f = make_toggle_group_form();
        draw_form(&mut buf, area, &f, &Theme::default());

        let layout = f.layout(area.width as f32, area.height as f32, |i| {
            let field = &f.fields[i];
            match &field.kind {
                FieldKind::ToggleGroup { toggles } => {
                    let label_w = field.label.visible_width();
                    let start_x = if label_w > 0 { label_w + 2 } else { 1 };
                    let items = toggles
                        .iter()
                        .map(|t| crate::primitives::form::FormItemMeasure {
                            id: t.id.clone(),
                            width: t.label.chars().count() as f32,
                        })
                        .collect();
                    FormFieldMeasure::with_items(1.0, start_x as f32, 1.0, items)
                }
                _ => FormFieldMeasure::new(1.0),
            }
        });

        // Find where "Aa" is painted (first toggle) on the form's absolute row.
        let mut aa_col = None;
        for x in area.x..area.x + area.width {
            if cell_char(&buf, x, area.y) == 'A' {
                if x + 1 < area.x + area.width && cell_char(&buf, x + 1, area.y) == 'a' {
                    aa_col = Some(x);
                    break;
                }
            }
        }
        let aa_col = aa_col.expect("'Aa' must be painted");
        let hit = layout.hit_test((aa_col - area.x) as f32, 0.0);
        assert_eq!(hit, FormHit::Field(WidgetId::new("case")));

        // Find where "Ab|" is painted (second toggle).
        let mut ab_col = None;
        for x in (aa_col + 2)..area.x + area.width {
            if cell_char(&buf, x, area.y) == 'A' {
                if x + 1 < area.x + area.width && cell_char(&buf, x + 1, area.y) == 'b' {
                    ab_col = Some(x);
                    break;
                }
            }
        }
        let ab_col = ab_col.expect("'Ab|' must be painted");
        let hit = layout.hit_test((ab_col - area.x) as f32, 0.0);
        assert_eq!(hit, FormHit::Field(WidgetId::new("word")));

        // Find ".*" (third toggle).
        let mut dot_col = None;
        for x in (ab_col + 3)..area.x + area.width {
            if cell_char(&buf, x, area.y) == '.'
                && x + 1 < area.x + area.width
                && cell_char(&buf, x + 1, area.y) == '*'
            {
                dot_col = Some(x);
                break;
            }
        }
        let dot_col = dot_col.expect("'.*' must be painted");
        let hit = layout.hit_test((dot_col - area.x) as f32, 0.0);
        assert_eq!(hit, FormHit::Field(WidgetId::new("regex")));
    }

    #[test]
    fn toggle_group_click_hits_correct_item() {
        toggle_group_click_hits_correct_item_at(0, 0);
    }

    /// Non-zero-origin regression guard: same round trip,
    /// painted at a shifted `area` origin, proving the local-frame contract
    /// holds even when the form isn't painted at (0, 0). See
    /// `toggle_group_click_hits_correct_item_at` for why `hit_test` still
    /// takes local (not absolute) coordinates here.
    #[test]
    fn toggle_group_click_hits_correct_item_at_nonzero_origin() {
        toggle_group_click_hits_correct_item_at(7, 13);
    }

    // ── ButtonRow paint↔click round-trip ────────────────────────────────

    use crate::primitives::form::ButtonRowItem;

    fn make_button_row_form() -> Form {
        Form {
            id: WidgetId::new("search"),
            fields: vec![FormField {
                id: WidgetId::new("actions"),
                label: label(""),
                kind: FieldKind::ButtonRow {
                    buttons: vec![
                        ButtonRowItem {
                            spec: None,
                            id: WidgetId::new("next"),
                            label: "Next".into(),
                            disabled: false,
                            icon: None,
                        },
                        ButtonRowItem {
                            spec: None,
                            id: WidgetId::new("replace"),
                            label: "Repl".into(),
                            disabled: false,
                            icon: None,
                        },
                        ButtonRowItem {
                            spec: None,
                            id: WidgetId::new("all"),
                            label: "All".into(),
                            disabled: true,
                            icon: None,
                        },
                    ],
                },
                disabled: false,
                validation: None,
                hint: label(""),
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        }
    }

    #[test]
    fn button_row_paints_bracketed_labels() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 3));
        let f = make_button_row_form();
        draw_form(&mut buf, Rect::new(0, 0, 40, 3), &f, &Theme::default());

        let row: String = (0..40).map(|x| cell_char(&buf, x, 0)).collect();
        assert!(row.contains("[Next]"), "expected '[Next]' in row: {row:?}");
        assert!(row.contains("[Repl]"), "expected '[Repl]' in row: {row:?}");
        assert!(row.contains("[All]"), "expected '[All]' in row: {row:?}");
    }

    fn cell_fg(buf: &Buffer, x: u16, y: u16) -> ratatui::style::Color {
        buf[(x, y)]
            .style()
            .fg
            .unwrap_or(ratatui::style::Color::Reset)
    }

    /// A `ButtonRowItem` with an explicit `Danger` spec tints both its
    /// label and bracket chrome in `theme.error_fg` — a `spec: None` item
    /// (every pre-existing construction site) keeps its plain
    /// `field_fg`/`accent_fg` look, unaffected.
    #[test]
    fn button_row_item_with_danger_spec_tints_error_fg() {
        let mut f = make_button_row_form();
        let mut spec = crate::primitives::button::ButtonSpec::new("Delete all");
        spec.variant = crate::primitives::button::ButtonVariant::Danger;
        if let FieldKind::ButtonRow { buttons } = &mut f.fields[0].kind {
            buttons[0].spec = Some(spec);
        }
        let theme = Theme::default();

        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 3));
        draw_form(&mut buf, Rect::new(0, 0, 40, 3), &f, &theme);

        let row: String = (0..40).map(|x| cell_char(&buf, x, 0)).collect();
        let bracket_col = row.find('[').expect("'[Next]' must be painted") as u16;
        assert_eq!(cell_fg(&buf, bracket_col, 0), ratatui_color(theme.error_fg));
        assert_eq!(
            cell_fg(&buf, bracket_col + 1, 0),
            ratatui_color(theme.error_fg)
        );
    }

    /// Shared body for the ButtonRow paint↔click round trip — see
    /// `toggle_group_click_hits_correct_item_at` for why `hit_test` takes
    /// local (not absolute) coordinates even at a non-zero `area` origin.
    fn button_row_click_hits_correct_item_at(origin_x: u16, origin_y: u16) {
        let area = Rect::new(origin_x, origin_y, 40, 3);
        let mut buf = Buffer::empty(Rect::new(0, 0, origin_x + 40, origin_y + 3));
        let f = make_button_row_form();
        draw_form(&mut buf, area, &f, &Theme::default());

        let layout = f.layout(area.width as f32, area.height as f32, |i| {
            let field = &f.fields[i];
            match &field.kind {
                FieldKind::ButtonRow { buttons } => {
                    let label_w = field.label.visible_width();
                    let start_x = if label_w > 0 { label_w + 2 } else { 1 };
                    let items = buttons
                        .iter()
                        .map(|b| crate::primitives::form::FormItemMeasure {
                            id: b.id.clone(),
                            width: (b.label.chars().count() + 2) as f32,
                        })
                        .collect();
                    FormFieldMeasure::with_items(1.0, start_x as f32, 1.0, items)
                }
                _ => FormFieldMeasure::new(1.0),
            }
        });

        // Find "[Next]" — the 'N' inside brackets, on the form's absolute row.
        let mut next_col = None;
        for x in area.x..area.x + area.width {
            if cell_char(&buf, x, area.y) == '['
                && x + 1 < area.x + area.width
                && cell_char(&buf, x + 1, area.y) == 'N'
            {
                next_col = Some(x);
                break;
            }
        }
        let next_col = next_col.expect("'[Next]' must be painted");
        let hit = layout.hit_test((next_col - area.x) as f32, 0.0);
        assert_eq!(hit, FormHit::Field(WidgetId::new("next")));

        // Click inside "Next" text (col + 2).
        let hit = layout.hit_test((next_col + 2 - area.x) as f32, 0.0);
        assert_eq!(hit, FormHit::Field(WidgetId::new("next")));

        // Find "[Repl]".
        let mut repl_col = None;
        for x in (next_col + 5)..area.x + area.width {
            if cell_char(&buf, x, area.y) == '['
                && x + 1 < area.x + area.width
                && cell_char(&buf, x + 1, area.y) == 'R'
            {
                repl_col = Some(x);
                break;
            }
        }
        let repl_col = repl_col.expect("'[Repl]' must be painted");
        let hit = layout.hit_test((repl_col - area.x) as f32, 0.0);
        assert_eq!(hit, FormHit::Field(WidgetId::new("replace")));

        // Find "[All]".
        let mut all_col = None;
        for x in (repl_col + 5)..area.x + area.width {
            if cell_char(&buf, x, area.y) == '['
                && x + 1 < area.x + area.width
                && cell_char(&buf, x + 1, area.y) == 'A'
            {
                all_col = Some(x);
                break;
            }
        }
        let all_col = all_col.expect("'[All]' must be painted");
        let hit = layout.hit_test((all_col - area.x) as f32, 0.0);
        assert_eq!(hit, FormHit::Field(WidgetId::new("all")));
    }

    #[test]
    fn button_row_click_hits_correct_item() {
        button_row_click_hits_correct_item_at(0, 0);
    }

    /// Non-zero-origin regression guard: same round trip,
    /// painted at a shifted `area` origin.
    #[test]
    fn button_row_click_hits_correct_item_at_nonzero_origin() {
        button_row_click_hits_correct_item_at(7, 13);
    }

    #[test]
    fn toggle_group_active_uses_accent_fg() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 3));
        let f = make_toggle_group_form();
        let theme = Theme {
            accent_fg: crate::types::Color::rgb(200, 100, 50),
            ..Theme::default()
        };
        draw_form(&mut buf, Rect::new(0, 0, 40, 3), &f, &theme);

        // "Aa" (value=true) should be in accent_fg.
        let mut aa_col = None;
        for x in 0..40u16 {
            if cell_char(&buf, x, 0) == 'A' && x + 1 < 40 && cell_char(&buf, x + 1, 0) == 'a' {
                aa_col = Some(x);
                break;
            }
        }
        let aa_col = aa_col.expect("'Aa' painted");
        let fg = buf[(aa_col, 0u16)].fg;
        assert_eq!(fg, ratatui::style::Color::Rgb(200, 100, 50));
    }

    #[test]
    fn button_row_disabled_uses_muted_fg() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 3));
        let f = make_button_row_form();
        let theme = Theme {
            muted_fg: crate::types::Color::rgb(80, 80, 80),
            accent_fg: crate::types::Color::rgb(200, 100, 50),
            ..Theme::default()
        };
        draw_form(&mut buf, Rect::new(0, 0, 40, 3), &f, &theme);

        // "[All]" (disabled=true) bracket should be in muted_fg.
        let mut all_col = None;
        for x in 0..40u16 {
            if cell_char(&buf, x, 0) == '[' && x + 1 < 40 && cell_char(&buf, x + 1, 0) == 'A' {
                if x + 2 < 40 && cell_char(&buf, x + 2, 0) == 'l' {
                    all_col = Some(x);
                    break;
                }
            }
        }
        let all_col = all_col.expect("'[All]' painted");
        let bracket_fg = buf[(all_col, 0u16)].fg;
        assert_eq!(bracket_fg, ratatui::style::Color::Rgb(80, 80, 80));
    }

    // ── PasswordInput round-trip ──────────────────────────────────────

    #[test]
    fn password_input_paints_mask_chars() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 3));
        let form = Form {
            id: WidgetId::new("f"),
            fields: vec![FormField {
                id: WidgetId::new("pw"),
                label: label("Token"),
                kind: FieldKind::PasswordInput {
                    value: "secret".into(),
                    placeholder: String::new(),
                    cursor: None,
                    mask_char: '•',
                },
                hint: label(""),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };
        draw_form(&mut buf, Rect::new(0, 0, 40, 3), &form, &Theme::default());
        let row: String = (0..40).map(|x| cell_char(&buf, x, 0)).collect();
        assert!(
            row.contains("••••••"),
            "password should show mask chars, got: {row:?}"
        );
        assert!(
            !row.contains("secret"),
            "password should NOT show plaintext, got: {row:?}"
        );
    }

    #[test]
    fn password_input_shows_placeholder_when_empty() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 3));
        let form = Form {
            id: WidgetId::new("f"),
            fields: vec![FormField {
                id: WidgetId::new("pw"),
                label: label(""),
                kind: FieldKind::PasswordInput {
                    value: String::new(),
                    placeholder: "Enter key".into(),
                    cursor: None,
                    mask_char: '•',
                },
                hint: label(""),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };
        draw_form(&mut buf, Rect::new(0, 0, 40, 3), &form, &Theme::default());
        let row: String = (0..40).map(|x| cell_char(&buf, x, 0)).collect();
        assert!(
            row.contains("Enter key"),
            "empty password should show placeholder, got: {row:?}"
        );
    }

    // ── SegmentedControl round-trip ───────────────────────────────────

    #[test]
    fn segmented_control_paints_all_options() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 3));
        let form = Form {
            id: WidgetId::new("f"),
            fields: vec![FormField {
                id: WidgetId::new("scope"),
                label: label(""),
                kind: FieldKind::SegmentedControl {
                    options: vec!["Ws".into(), "File".into(), "Sel".into()],
                    selected_idx: 1,
                },
                hint: label(""),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };
        draw_form(&mut buf, Rect::new(0, 0, 40, 3), &form, &Theme::default());
        let row: String = (0..40).map(|x| cell_char(&buf, x, 0)).collect();
        assert!(row.contains("Ws"), "should paint 'Ws', got: {row:?}");
        assert!(row.contains("File"), "should paint 'File', got: {row:?}");
        assert!(row.contains("Sel"), "should paint 'Sel', got: {row:?}");
    }

    #[test]
    fn segmented_control_hit_test_returns_item_id() {
        use crate::primitives::form::FormHit;
        let form = Form {
            id: WidgetId::new("f"),
            fields: vec![FormField {
                id: WidgetId::new("scope"),
                label: label(""),
                kind: FieldKind::SegmentedControl {
                    options: vec!["Ws".into(), "File".into()],
                    selected_idx: 0,
                },
                hint: label(""),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };
        let layout = tui_form_layout(&form, Rect::new(0, 0, 40, 3));
        let vis = &layout.visible_fields[0];
        assert!(
            vis.item_bounds.len() >= 2,
            "SegmentedControl should have per-item bounds"
        );
        let (id0, rect0) = &vis.item_bounds[0];
        assert_eq!(id0, &WidgetId::new("scope__seg_0"));
        let hit = layout.hit_test(rect0.x + 1.0, rect0.y + 0.5);
        assert_eq!(hit, FormHit::Field(WidgetId::new("scope__seg_0")));
    }

    // ── TextArea round-trip ──────────────────────────────────────────

    #[test]
    fn textarea_paints_multi_row() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 6));
        let form = Form {
            id: WidgetId::new("f"),
            fields: vec![FormField {
                id: WidgetId::new("desc"),
                label: label("Desc"),
                kind: FieldKind::TextArea {
                    value: "line one content here".into(),
                    placeholder: String::new(),
                    cursor: None,
                    visible_rows: 3,
                },
                hint: label(""),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };
        draw_form(&mut buf, Rect::new(0, 0, 40, 6), &form, &Theme::default());
        let layout = tui_form_layout(&form, Rect::new(0, 0, 40, 6));
        assert_eq!(
            layout.visible_fields[0].bounds.height, 3.0,
            "TextArea with visible_rows=3 should be 3 cells tall"
        );
    }

    #[test]
    fn textarea_joins_the_shared_control_column_and_does_not_clobber_its_label() {
        let form = Form {
            id: WidgetId::new("f"),
            fields: vec![FormField {
                id: WidgetId::new("desc"),
                label: label("Description"),
                kind: FieldKind::TextArea {
                    value: "hi".into(),
                    placeholder: String::new(),
                    cursor: None,
                    visible_rows: 2,
                },
                hint: label(""),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 5));
        draw_form(&mut buf, Rect::new(0, 0, 40, 5), &form, &Theme::default());

        let row: String = (0..40).map(|x| cell_char(&buf, x, 0)).collect();
        assert!(
            row[1..].starts_with("Description"),
            "label must still be painted, not clobbered by the control: {row:?}"
        );
        // Labels start at column 1 (column 0 is the row's left border);
        // `control_start` is `label_end + 1` where `label_end = 1 +
        // col_width` (see `build_row_ctx`) — "Description".len() == 11,
        // so the control starts at column 13, right after the label,
        // not at column 0 like the bare `[hi...` a painter that ignores
        // `control_start` would draw over the label.
        assert_eq!(
            cell_char(&buf, 13, 0),
            '[',
            "TextArea's control box should start right after the label: {row:?}"
        );
    }

    // ── ValidationState round-trip ───────────────────────────────────

    #[test]
    fn validation_error_paints_indicator() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 3));
        let form = Form {
            id: WidgetId::new("f"),
            fields: vec![FormField {
                id: WidgetId::new("name"),
                label: label("Name"),
                kind: FieldKind::TextInput {
                    value: String::new(),
                    placeholder: String::new(),
                    cursor: None,
                    selection_anchor: None,
                },
                hint: label(""),
                disabled: false,
                validation: Some(ValidationState::Error("Required".into())),
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };
        draw_form(&mut buf, Rect::new(0, 0, 40, 3), &form, &Theme::default());
        let col0 = cell_char(&buf, 0, 0);
        assert_eq!(col0, '!', "error validation should paint '!' at col 0");
    }

    #[test]
    fn validation_warning_paints_indicator() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 3));
        let form = Form {
            id: WidgetId::new("f"),
            fields: vec![FormField {
                id: WidgetId::new("name"),
                label: label("Name"),
                kind: FieldKind::TextInput {
                    value: "ab".into(),
                    placeholder: String::new(),
                    cursor: None,
                    selection_anchor: None,
                },
                hint: label(""),
                disabled: false,
                validation: Some(ValidationState::Warning("Too short".into())),
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };
        draw_form(&mut buf, Rect::new(0, 0, 40, 3), &form, &Theme::default());
        let col0 = cell_char(&buf, 0, 0);
        assert_eq!(
            col0, '\u{26A0}',
            "warning validation should paint warning sign at col 0"
        );
    }

    // ── FieldKind::Toolbar paint↔click round-trip ────────────────────────

    use crate::primitives::toolbar::{Toolbar, ToolbarButton};

    fn make_toolbar_form() -> Form {
        Form {
            id: WidgetId::new("settings"),
            fields: vec![FormField {
                id: WidgetId::new("actions"),
                label: label(""),
                kind: FieldKind::Toolbar(Toolbar {
                    id: WidgetId::new("tb"),
                    buttons: vec![
                        ToolbarButton::Action {
                            spec: None,
                            id: WidgetId::new("reset"),
                            label: "Reset".into(),
                            icon: None,
                            key_hint: None,
                            enabled: true,
                            is_active: false,
                            tooltip: String::new(),
                        },
                        ToolbarButton::Separator,
                        ToolbarButton::Action {
                            spec: None,
                            id: WidgetId::new("export"),
                            label: "Export".into(),
                            icon: None,
                            key_hint: None,
                            enabled: true,
                            is_active: false,
                            tooltip: String::new(),
                        },
                    ],
                    bg: None,
                    focused_index: None,
                }),
                hint: label(""),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        }
    }

    #[test]
    fn toolbar_field_paints_bracket() {
        let area = Rect::new(0, 0, 40, 3);
        let mut buf = Buffer::empty(area);
        let f = make_toolbar_form();
        draw_form(&mut buf, area, &f, &Theme::default());

        let row: String = (0..40).map(|x| cell_char(&buf, x, 0)).collect();
        assert!(
            row.contains('['),
            "toolbar `[` bracket should be painted; row: {row:?}"
        );
        assert!(
            row.contains("Reset"),
            "toolbar 'Reset' label should be painted; row: {row:?}"
        );
        assert!(
            row.contains("Export"),
            "toolbar 'Export' label should be painted; row: {row:?}"
        );
    }

    #[test]
    fn toolbar_field_click_routes_to_action_id() {
        let area = Rect::new(0, 0, 40, 3);
        let mut buf = Buffer::empty(area);
        let f = make_toolbar_form();
        draw_form(&mut buf, area, &f, &Theme::default());

        let layout = tui_form_layout(&f, area);

        // Find where 'R' (first char of "Reset") is painted.
        let mut reset_col = None;
        for x in 0..40u16 {
            if cell_char(&buf, x, 0) == 'R'
                && x + 4 < 40
                && cell_char(&buf, x + 1, 0) == 'e'
                && cell_char(&buf, x + 2, 0) == 's'
            {
                reset_col = Some(x);
                break;
            }
        }
        let reset_col = reset_col.expect("'Reset' must be painted");
        let hit = layout.hit_test(reset_col as f32, 0.0);
        assert_eq!(
            hit,
            FormHit::Field(WidgetId::new("reset")),
            "clicking Reset label should hit the reset action id"
        );
    }

    #[test]
    fn toolbar_field_export_click_routes_to_export_id() {
        let area = Rect::new(0, 0, 40, 3);
        let mut buf = Buffer::empty(area);
        let f = make_toolbar_form();
        draw_form(&mut buf, area, &f, &Theme::default());

        let layout = tui_form_layout(&f, area);

        // Find where 'E' (first char of "Export") is painted.
        let mut export_col = None;
        for x in 0..40u16 {
            if cell_char(&buf, x, 0) == 'E'
                && x + 5 < 40
                && cell_char(&buf, x + 1, 0) == 'x'
                && cell_char(&buf, x + 2, 0) == 'p'
            {
                export_col = Some(x);
                break;
            }
        }
        let export_col = export_col.expect("'Export' must be painted");
        let hit = layout.hit_test(export_col as f32, 0.0);
        assert_eq!(
            hit,
            FormHit::Field(WidgetId::new("export")),
            "clicking Export label should hit the export action id"
        );
    }

    // ── Serde round-trip for FieldKind::Toolbar ───────────────────────────

    #[test]
    fn serde_roundtrip_field_kind_toolbar() {
        use crate::primitives::form::FormEvent;
        let kind = FieldKind::Toolbar(Toolbar {
            id: WidgetId::new("tb"),
            buttons: vec![
                ToolbarButton::Action {
                    spec: None,
                    id: WidgetId::new("reset"),
                    label: "Reset".into(),
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
        });
        let json = serde_json::to_string(&kind).unwrap();
        let back: FieldKind = serde_json::from_str(&json).unwrap();
        assert_eq!(kind, back);

        // FormEvent::ToolbarButtonClicked round-trip.
        let ev = FormEvent::ToolbarButtonClicked {
            field_id: WidgetId::new("actions"),
            button_id: WidgetId::new("reset"),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: FormEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    // ── FieldKind::ComboBox paint round-trip ──────────────────────────────

    use crate::primitives::combo_box::ComboBox;

    fn make_combo_box_form(text: &str) -> Form {
        let mut combo = ComboBox::new(
            WidgetId::new("lang-combo"),
            vec!["rust-analyzer".into(), "pyright".into(), "gopls".into()],
        );
        combo.text = text.to_string();
        Form {
            id: WidgetId::new("settings"),
            fields: vec![FormField {
                id: WidgetId::new("language"),
                label: label("Language"),
                kind: FieldKind::ComboBox(combo),
                hint: label(""),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        }
    }

    #[test]
    fn combo_box_field_paints_text_and_chevron() {
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        let f = make_combo_box_form("rust");
        draw_form(&mut buf, area, &f, &Theme::default());

        let row: String = (0..40).map(|x| cell_char(&buf, x, 0)).collect();
        assert!(
            row.contains("rust"),
            "combo box live text should be painted; row: {row:?}"
        );
        assert!(
            row.contains('\u{25BE}'),
            "combo box chevron should be painted; row: {row:?}"
        );
        // The open dropdown list itself is never painted by `Form` — only
        // the compact row — so none of the other options should appear.
        assert!(
            !row.contains("pyright"),
            "closed dropdown options must not appear in the compact row; row: {row:?}"
        );
    }

    // ── Serde round-trip for FieldKind::ComboBox ──────────────────────────

    #[test]
    fn serde_roundtrip_field_kind_combo_box() {
        use crate::primitives::form::FormEvent;
        let kind = FieldKind::ComboBox(ComboBox::new(
            WidgetId::new("lang-combo"),
            vec!["rust-analyzer".into(), "pyright".into()],
        ));
        let json = serde_json::to_string(&kind).unwrap();
        let back: FieldKind = serde_json::from_str(&json).unwrap();
        assert_eq!(kind, back);

        // FormEvent::ComboBoxCommitted round-trip.
        let ev = FormEvent::ComboBoxCommitted {
            id: WidgetId::new("language"),
            value: "rust-analyzer".to_string(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: FormEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    // ── FieldKind::NumberInput paint round-trip ───────────────────────────

    use crate::primitives::number_input::NumberInput;
    use crate::primitives::radio_group::{RadioGroup, RadioOption};

    fn make_number_input_form(number: NumberInput) -> Form {
        Form {
            id: WidgetId::new("settings"),
            fields: vec![FormField {
                id: WidgetId::new("volume"),
                label: label("Volume"),
                kind: FieldKind::NumberInput(number),
                hint: label(""),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        }
    }

    /// `FieldKind::NumberInput` paints through the real
    /// `tui::draw_number_input` box rasteriser — a bordered control with
    /// stepper glyphs, not a compact one-cell summary like `DatePicker`/
    /// `ComboBox` — and reserves the 3 cell rows that box needs.
    #[test]
    fn number_input_field_paints_bordered_box_with_steppers() {
        let mut number = NumberInput::new(WidgetId::new("volume"), 0.0, 100.0);
        number.value = 42.0;
        let area = Rect::new(0, 0, 40, 3);
        let mut buf = Buffer::empty(area);
        let f = make_number_input_form(number);
        draw_form(&mut buf, area, &f, &Theme::default());

        let rows: Vec<String> = (0..3)
            .map(|y| (0..40).map(|x| cell_char(&buf, x, y)).collect())
            .collect();
        assert!(
            rows.iter().any(|r| r.contains('4') && r.contains('2')),
            "committed value must be painted; rows: {rows:?}"
        );
        assert!(rows.iter().any(|r| r.contains('+')));
        assert!(rows.iter().any(|r| r.contains('-')));
    }

    // ── FieldKind::RadioGroup paint round-trip ────────────────────────────

    fn make_radio_group_form(group: RadioGroup) -> Form {
        Form {
            id: WidgetId::new("settings"),
            fields: vec![FormField {
                id: WidgetId::new("theme"),
                label: label("Theme"),
                kind: FieldKind::RadioGroup(group),
                hint: label(""),
                disabled: false,
                validation: None,
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        }
    }

    /// `FieldKind::RadioGroup` paints every option on its own row through
    /// the real `tui::draw_radio_group` rasteriser — unlike every other
    /// field kind, the row is allowed to span more than one cell tall
    /// (see `FormFieldMeasure::vertical`'s doc).
    #[test]
    fn radio_group_field_paints_one_row_per_option() {
        let mut group = RadioGroup::new(
            WidgetId::new("theme"),
            vec![
                RadioOption::new(WidgetId::new("theme:light"), "Light"),
                RadioOption::new(WidgetId::new("theme:dark"), "Dark"),
            ],
        );
        group.selected = Some(1);
        let area = Rect::new(0, 0, 40, 2);
        let mut buf = Buffer::empty(area);
        let f = make_radio_group_form(group);
        draw_form(&mut buf, area, &f, &Theme::default());

        let row0: String = (0..40).map(|x| cell_char(&buf, x, 0)).collect();
        let row1: String = (0..40).map(|x| cell_char(&buf, x, 1)).collect();
        assert!(row0.contains("Light"), "row 0: {row0:?}");
        assert!(row1.contains("Dark"), "row 1: {row1:?}");
        assert!(
            row1.contains('\u{2022}'),
            "selected option's marker should paint; row 1: {row1:?}"
        );
    }

    // ── CJK label paint↔measure agreement ───────────────────────────────
    //
    // `tui_form_layout` reserves `label.visible_width()` (display-width-aware)
    // columns for the label, and `draw_form` paints it via the char-count-
    // strided `draw_styled_text`. These tests pin that the *painted* label
    // span and the *measured* `visible_width()` agree, for both a right-aligned
    // value field (`ReadOnly`) and an items-after-label field (`ToggleGroup`).

    #[test]
    fn readonly_field_cjk_label_paints_full_display_width_no_gap() {
        // "日本語:" — display width 7 (2+2+2+1), char count 4. Before the
        // fix, `draw_styled_text` would stop painting after 4 columns while
        // `tui_form_layout`/the ReadOnly branch reserved 7, leaving 3
        // stray background columns between the label and the value.
        let f = Form {
            id: WidgetId::new("info"),
            fields: vec![FormField {
                id: WidgetId::new("lang"),
                label: label("日本語:"),
                kind: FieldKind::ReadOnly { value: label("OK") },
                disabled: false,
                validation: None,
                hint: label(""),
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };
        let area = Rect::new(0, 0, 30, 3);
        let mut buf = Buffer::empty(area);
        draw_form(&mut buf, area, &f, &Theme::default());

        let label_visible_width = f.fields[0].label.visible_width();
        assert_eq!(label_visible_width, 7);

        // Label starts at col 1 (label_col) and must span exactly its
        // `visible_width()` in painted columns — cols 1..=6 hold the three
        // wide CJK glyphs (each followed by an empty continuation cell),
        // col 7 holds ':', and col 8 (label_col + visible_width) is
        // unpainted background, not a stray copy of ':' or the value.
        assert_eq!(cell_char(&buf, 1, 0), '日');
        assert_eq!(buf[(2u16, 0u16)].symbol(), "");
        assert_eq!(cell_char(&buf, 3, 0), '本');
        assert_eq!(buf[(4u16, 0u16)].symbol(), "");
        assert_eq!(cell_char(&buf, 5, 0), '語');
        assert_eq!(buf[(6u16, 0u16)].symbol(), "");
        assert_eq!(cell_char(&buf, 7, 0), ':');
        assert_eq!(
            cell_char(&buf, 1 + label_visible_width as u16, 0),
            ' ',
            "no leftover glyph past the label's measured display width"
        );

        // The ReadOnly value ("OK") starts right after the label column
        // (col 1 + visible_width 7 + 1 margin = col 9), not hugging the
        // row's right edge — confirming the control column lines up
        // with the label's actual painted extent.
        assert_eq!(cell_char(&buf, 9, 0), 'O');
        assert_eq!(cell_char(&buf, 10, 0), 'K');
    }

    #[test]
    fn toggle_group_cjk_label_items_start_flush_with_painted_label() {
        // Same desync, but for the "items packed after the label" shape
        // (`ToggleGroup`/`ButtonRow`/`SegmentedControl`): `tui_form_layout`
        // computes `start_x = label.visible_width() + 2` for where the
        // toggles begin. Before the fix, the label painted narrower than
        // that (char count, not display width), so toggles started further
        // right than the label's actual painted end — a CJK-content-
        // dependent gap. Pin that the gap is now exactly the intended 1
        // blank column (the `+2` reserves 2 cells past the label's last
        // glyph column, and the layout's `start_x` is 0-based while
        // painting's `label_col` is 1-based, so 1 of those 2 reserved
        // cells is absorbed by that frame offset).
        let f = Form {
            id: WidgetId::new("opts"),
            fields: vec![FormField {
                id: WidgetId::new("toggles"),
                label: label("中文:"),
                kind: FieldKind::ToggleGroup {
                    toggles: vec![ToggleGroupItem {
                        id: WidgetId::new("a"),
                        label: "A".into(),
                        value: true,
                    }],
                },
                disabled: false,
                validation: None,
                hint: label(""),
                label_position: None,
                label_align: None,
                label_width: None,
                tab_index: None,
                control_width: None,
                min_control_width: None,
                row_align: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };
        let area = Rect::new(0, 0, 30, 3);
        let mut buf = Buffer::empty(area);
        draw_form(&mut buf, area, &f, &Theme::default());

        let label_visible_width = f.fields[0].label.visible_width();
        assert_eq!(label_visible_width, 5); // 2 + 2 + 1

        // Label painted at cols 1..=5 ("中", "文", ':').
        assert_eq!(cell_char(&buf, 1, 0), '中');
        assert_eq!(buf[(2u16, 0u16)].symbol(), "");
        assert_eq!(cell_char(&buf, 3, 0), '文');
        assert_eq!(buf[(4u16, 0u16)].symbol(), "");
        assert_eq!(cell_char(&buf, 5, 0), ':');

        // The toggle item ("A") starts exactly 1 column past the label's
        // last painted glyph — not drifted further right by the old
        // char-count/display-width mismatch (which for this label would
        // have painted only 3 columns wide, leaving a 3-column gap instead
        // of 1).
        let a_col = (1..30)
            .find(|&x| cell_char(&buf, x, 0) == 'A')
            .expect("toggle item 'A' must be painted");
        assert_eq!(a_col, 1 + label_visible_width as u16 + 1);
    }

    fn field(id: &str, label_text: &str, kind: FieldKind) -> FormField {
        FormField {
            id: WidgetId::new(id),
            label: label(label_text),
            kind,
            disabled: false,
            validation: None,
            hint: label(""),
            label_position: None,
            label_align: None,
            label_width: None,
            tab_index: None,
            control_width: None,
            min_control_width: None,
            row_align: None,
        }
    }

    #[test]
    fn top_label_position_paints_label_above_input_row() {
        let mut top_field = field("name", "Name", FieldKind::Toggle { value: true });
        top_field.label_position = Some(LabelPosition::Top);
        let after = field("after", "After", FieldKind::Toggle { value: false });
        let f = Form {
            id: WidgetId::new("f"),
            fields: vec![top_field, after],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };
        let area = Rect::new(0, 0, 30, 4);
        let mut buf = Buffer::empty(area);
        draw_form(&mut buf, area, &f, &Theme::default());

        // Row 0: the label on its own line.
        let row0: String = (1..5).map(|x| cell_char(&buf, x, 0)).collect();
        assert_eq!(row0, "Name");

        // Row 1: the input, starting flush left (no reserved label
        // column) rather than beside the label.
        let found_x = (1..30).any(|x| cell_char(&buf, x, 1) == 'x');
        assert!(
            found_x,
            "expected the toggle glyph on the row below the label"
        );

        // The second field starts two rows down, proving `Top` reserved
        // an extra row for the first field rather than overlapping it.
        let row2: String = (1..6).map(|x| cell_char(&buf, x, 2)).collect();
        assert_eq!(row2, "After");
    }

    #[test]
    fn left_label_width_and_align_aligns_label_column() {
        let mut form = Form {
            id: WidgetId::new("f"),
            fields: vec![
                field("short", "ID", FieldKind::ReadOnly { value: label("1") }),
                field(
                    "long",
                    "Description",
                    FieldKind::ReadOnly { value: label("x") },
                ),
            ],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: LabelPosition::Left,
            label_align: LabelAlign::End,
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };
        // Force an explicit column width wider than either label so
        // `End` alignment has room to shift "ID" rightward within it.
        form.label_width = Some(15.0);

        let area = Rect::new(0, 0, 40, 3);
        let mut buf = Buffer::empty(area);
        draw_form(&mut buf, area, &form, &Theme::default());

        // "Description" (11 cells) right-aligned in a 15-wide column
        // starts at column 1 + (15 - 11) = 5.
        let desc: String = (5..16).map(|x| cell_char(&buf, x, 1)).collect();
        assert_eq!(desc, "Description");

        // "ID" (2 cells) right-aligned in the same 15-wide column
        // starts at column 1 + (15 - 2) = 14, not column 1 — proving
        // both rows share one aligned column rather than each sizing
        // its own.
        assert_eq!(cell_char(&buf, 14, 0), 'I');
        assert_eq!(cell_char(&buf, 15, 0), 'D');
        assert_eq!(cell_char(&buf, 1, 0), ' ');
    }

    #[test]
    fn hidden_label_position_suppresses_label_text() {
        let mut f = field(
            "secret",
            "Secret",
            FieldKind::ReadOnly { value: label("v") },
        );
        f.label_position = Some(LabelPosition::Hidden);
        let form = Form {
            id: WidgetId::new("f"),
            fields: vec![f],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };
        let area = Rect::new(0, 0, 30, 2);
        let mut buf = Buffer::empty(area);
        draw_form(&mut buf, area, &form, &Theme::default());

        let row0: String = (0..30).map(|x| cell_char(&buf, x, 0)).collect();
        assert!(
            !row0.contains('S') && !row0.contains('e'),
            "hidden label text must not be painted: {row0:?}"
        );
    }

    #[test]
    fn inline_label_position_does_not_share_a_column() {
        // Unlike `Left`, `Inline` sizes each row's label column to its
        // own label text — two fields with very different label lengths
        // don't line up their items into one shared column.
        let toggles = || {
            vec![ToggleGroupItem {
                id: WidgetId::new("on"),
                label: "On".into(),
                value: true,
            }]
        };
        let mut short = field("a", "A", FieldKind::ToggleGroup { toggles: toggles() });
        short.label_position = Some(LabelPosition::Inline);
        let mut long = field(
            "b",
            "A much longer label",
            FieldKind::ToggleGroup { toggles: toggles() },
        );
        long.label_position = Some(LabelPosition::Inline);
        let form = Form {
            id: WidgetId::new("f"),
            fields: vec![short, long],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
            label_position: Default::default(),
            label_align: Default::default(),
            label_width: None,
            control_width: Default::default(),
            min_control_width: None,
            spacing: Default::default(),
        };
        let area = Rect::new(0, 0, 40, 3);
        let mut buf = Buffer::empty(area);
        draw_form(&mut buf, area, &form, &Theme::default());

        // Short label's item starts right after its own 1-cell label
        // ("A"), not pushed out to match the long label's width.
        let short_item_col = (1..40)
            .find(|&x| cell_char(&buf, x, 0) == 'O')
            .expect("'On' item must be painted on the short-label row");
        assert_eq!(short_item_col, 1 /* "A" */ + 2 /* gap */);

        // Long label's item starts well past where a shared `Left`
        // column would have placed it, proving each row sized its own
        // column rather than agreeing on one.
        let long_item_col = (1..40)
            .find(|&x| cell_char(&buf, x, 1) == 'O')
            .expect("'On' item must be painted on the long-label row");
        assert!(
            long_item_col > short_item_col,
            "expected the long-label row's item further right than the short-label row's"
        );
    }
}
