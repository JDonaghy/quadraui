//! TUI rasteriser for [`crate::Form`].
//!
//! Per D6: this function asks the primitive for a [`crate::FormLayout`]
//! using a uniform 1-cell-per-field measurer (TUI rows are always 1
//! cell tall) and paints the resolved positions verbatim.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{draw_styled_text, ratatui_color, set_cell};
use crate::primitives::form::{
    FieldKind, Form, FormFieldMeasure, FormItemMeasure, ValidationState,
};
use crate::primitives::toolbar::ToolbarButton;
use crate::theme::Theme;
use crate::types::{Decoration, WidgetId};

/// Compute the form layout using TUI cell metrics (1 cell per row,
/// char-count item widths).
pub fn tui_form_layout(form: &Form, area: Rect) -> crate::primitives::form::FormLayout {
    form.layout(area.width as f32, area.height as f32, |i| {
        let field = &form.fields[i];
        match &field.kind {
            FieldKind::ToggleGroup { toggles } => {
                let label_w = field.label.visible_width();
                let start_x = if label_w > 0 { label_w + 2 } else { 1 };
                let items = toggles
                    .iter()
                    .map(|t| FormItemMeasure {
                        id: t.id.clone(),
                        width: t.label.chars().count() as f32,
                    })
                    .collect();
                FormFieldMeasure::with_items(1.0, start_x as f32, 1.0, items)
            }
            FieldKind::ButtonRow { buttons } => {
                let label_w = field.label.visible_width();
                let start_x = if label_w > 0 { label_w + 2 } else { 1 };
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
                            id: b.id.clone(),
                            width: (b.label.chars().count() + icon_w + 2) as f32,
                        }
                    })
                    .collect();
                FormFieldMeasure::with_items(1.0, start_x as f32, 1.0, items)
            }
            FieldKind::TextArea { visible_rows, .. } => FormFieldMeasure::new(*visible_rows as f32),
            FieldKind::Toolbar(toolbar) => {
                use super::toolbar::tui_item_width;
                let label_w = field.label.visible_width();
                let start_x = if label_w > 0 { label_w + 2 } else { 0 };
                // Include ALL toolbar items (actions, separators, labels) in
                // item_measures so the sequential layout matches `draw_toolbar`'s
                // left-to-right packing. Separators and labels get the field's
                // id so clicks on them resolve to `FormHit::Field(field_id)`.
                let items = toolbar
                    .buttons
                    .iter()
                    .map(|btn| {
                        let id = match btn {
                            ToolbarButton::Action { id, .. } => id.clone(),
                            _ => field.id.clone(),
                        };
                        FormItemMeasure {
                            id,
                            width: tui_item_width(btn),
                        }
                    })
                    .collect();
                // item_gap = 0: toolbar buttons pack edge-to-edge, same as
                // how `Toolbar::layout` places items.
                FormFieldMeasure::with_items(1.0, start_x as f32, 0.0, items)
            }
            FieldKind::SegmentedControl { options, .. } => {
                let label_w = field.label.visible_width();
                let start_x = if label_w > 0 { label_w + 2 } else { 1 };
                let items = options
                    .iter()
                    .enumerate()
                    .map(|(idx, opt)| FormItemMeasure {
                        id: WidgetId::new(format!("{}__seg_{}", field.id.as_str(), idx)),
                        width: opt.chars().count() as f32,
                    })
                    .collect();
                FormFieldMeasure::with_items(1.0, start_x as f32, 1.0, items)
            }
            _ => FormFieldMeasure::new(1.0),
        }
    })
}

/// Draw a [`Form`] into `area` on `buf`.
pub fn draw_form(buf: &mut Buffer, area: Rect, form: &Form, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let bg = ratatui_color(theme.tab_bar_bg);
    let fg = ratatui_color(theme.foreground);
    let hdr_fg = ratatui_color(theme.header_fg);
    let hdr_bg = ratatui_color(theme.header_bg);
    let sel_bg = ratatui_color(theme.selected_bg);
    let dim_fg = ratatui_color(theme.muted_fg);
    let accent_fg = ratatui_color(theme.accent_fg);
    let error_fg = ratatui_color(theme.error_fg);
    let warning_fg = ratatui_color(theme.warning_fg);

    let layout = tui_form_layout(form, area);

    for visible_field in &layout.visible_fields {
        let field = &form.fields[visible_field.field_idx];
        let y = area.y + visible_field.bounds.y.round() as u16;

        let is_focused = form.has_focus
            && form
                .focused_field
                .as_ref()
                .is_some_and(|id| id == &field.id);
        let is_header = matches!(field.kind, FieldKind::Label);

        let (default_fg, row_bg) = match (is_header, is_focused) {
            (_, true) => (fg, sel_bg),
            (true, false) => (hdr_fg, hdr_bg),
            (false, false) => (fg, bg),
        };

        for x in area.x..area.x + area.width {
            set_cell(buf, x, y, ' ', default_fg, row_bg);
        }

        let field_fg = if field.disabled { dim_fg } else { default_fg };

        let label_col = 1usize;
        let label_end = draw_styled_text(
            buf,
            area,
            y,
            label_col,
            &field.label,
            field_fg,
            row_bg,
            Decoration::Normal,
            dim_fg,
        );
        let no_label = field.label.visible_width() == 0;

        match &field.kind {
            FieldKind::Label => {
                // No separate input — label spans the row.
            }
            FieldKind::Toggle { value } => {
                let glyph = if *value { "[x]" } else { "[ ]" };
                let w = glyph.chars().count();
                let start_col = if no_label {
                    1
                } else {
                    (area.width as usize).saturating_sub(w + 2)
                };
                if start_col > label_end + 1 || no_label {
                    let input_fg = if *value { accent_fg } else { field_fg };
                    for (col, ch) in (start_col..).zip(glyph.chars()) {
                        if col >= area.width as usize {
                            break;
                        }
                        set_cell(buf, area.x + col as u16, y, ch, input_fg, row_bg);
                    }
                }
            }
            FieldKind::TextInput {
                value,
                placeholder,
                cursor,
                selection_anchor,
            } => {
                let shown = if value.is_empty() {
                    placeholder.as_str()
                } else {
                    value.as_str()
                };
                let input_fg = if value.is_empty() { dim_fg } else { field_fg };

                let (start_col, desired) = if no_label {
                    let sc = 1usize;
                    let avail = (area.width as usize).saturating_sub(sc + 2);
                    (sc, shown.chars().count().min(avail))
                } else {
                    let max_input = (area.width as usize * 2 / 3).max(10);
                    let d = shown.chars().count().min(max_input);
                    let sc = (area.width as usize).saturating_sub(d + 2);
                    (sc, d)
                };

                let (sel_lo, sel_hi) = if value.is_empty() {
                    (0, 0)
                } else {
                    match (cursor, selection_anchor) {
                        (Some(c), Some(a)) if c != a => (*c.min(a), *c.max(a)),
                        _ => (0, 0),
                    }
                };

                if start_col > label_end + 1 || no_label {
                    if start_col > 0 && start_col - 1 < area.width as usize {
                        set_cell(buf, area.x + (start_col - 1) as u16, y, '[', dim_fg, row_bg);
                    }
                    let mut col = start_col;
                    let mut byte = 0usize;
                    for ch in shown.chars().take(desired) {
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
                        col += 1;
                        byte += ch.len_utf8();
                    }
                    let bracket_col = if no_label {
                        (area.width as usize).saturating_sub(1)
                    } else {
                        col
                    };
                    if bracket_col < area.width as usize {
                        set_cell(buf, area.x + bracket_col as u16, y, ']', dim_fg, row_bg);
                    }

                    if let Some(cur) = cursor {
                        if !value.is_empty() {
                            let mut byte = 0usize;
                            let mut char_idx = 0usize;
                            for ch in shown.chars().take(desired) {
                                if byte >= *cur {
                                    break;
                                }
                                byte += ch.len_utf8();
                                char_idx += 1;
                            }
                            let cursor_col = start_col + char_idx;
                            if cursor_col < area.width as usize {
                                let ch = shown.chars().nth(char_idx).unwrap_or(' ');
                                set_cell(buf, area.x + cursor_col as u16, y, ch, row_bg, field_fg);
                            }
                        }
                    }
                }
            }
            FieldKind::Button => {
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
            FieldKind::ReadOnly { value } => {
                let w = value.visible_width();
                let start_col = if no_label {
                    1
                } else {
                    (area.width as usize).saturating_sub(w + 2)
                };
                if start_col > label_end + 1 || no_label {
                    draw_styled_text(
                        buf,
                        area,
                        y,
                        start_col,
                        value,
                        dim_fg,
                        row_bg,
                        Decoration::Muted,
                        dim_fg,
                    );
                }
            }
            FieldKind::Slider {
                value,
                min,
                max,
                step: _,
            } => {
                let range = (*max - *min).max(f32::EPSILON);
                let frac = ((*value - *min) / range).clamp(0.0, 1.0);
                let track_cells: usize = 12;
                let filled = (frac * track_cells as f32).round() as usize;
                let value_str = format!("{value:.2}");
                let total = track_cells + 2 + value_str.chars().count() + 2;
                let start_col = (area.width as usize).saturating_sub(total + 2);
                if start_col > label_end + 1 {
                    let mut col = start_col;
                    set_cell(buf, area.x + col as u16, y, '[', dim_fg, row_bg);
                    col += 1;
                    for i in 0..track_cells {
                        let ch = if i < filled { '=' } else { '-' };
                        let fg = if i < filled { accent_fg } else { dim_fg };
                        set_cell(buf, area.x + col as u16, y, ch, fg, row_bg);
                        col += 1;
                    }
                    set_cell(buf, area.x + col as u16, y, ']', dim_fg, row_bg);
                    col += 2;
                    for ch in value_str.chars() {
                        if col >= area.width as usize {
                            break;
                        }
                        set_cell(buf, area.x + col as u16, y, ch, field_fg, row_bg);
                        col += 1;
                    }
                }
            }
            FieldKind::ColorPicker { value } => {
                let hex = format!("#{:02x}{:02x}{:02x}", value.r, value.g, value.b);
                let total = 2 + hex.chars().count();
                let start_col = (area.width as usize).saturating_sub(total + 2);
                if start_col > label_end + 1 {
                    let swatch_fg = ratatui::style::Color::Rgb(value.r, value.g, value.b);
                    set_cell(
                        buf,
                        area.x + start_col as u16,
                        y,
                        '\u{25A0}',
                        swatch_fg,
                        row_bg,
                    );
                    for (col, ch) in (start_col + 2..).zip(hex.chars()) {
                        if col >= area.width as usize {
                            break;
                        }
                        set_cell(buf, area.x + col as u16, y, ch, field_fg, row_bg);
                    }
                }
            }
            FieldKind::Dropdown {
                options,
                selected_idx,
            } => {
                let chosen = options.get(*selected_idx).cloned().unwrap_or_default();
                let label_w = chosen.visible_width();
                let total = label_w + 4;
                let start_col = (area.width as usize).saturating_sub(total + 1);
                if start_col > label_end + 1 {
                    draw_styled_text(
                        buf,
                        area,
                        y,
                        start_col + 1,
                        &chosen,
                        field_fg,
                        row_bg,
                        Decoration::Normal,
                        dim_fg,
                    );
                    let chev_col = start_col + 1 + label_w + 1;
                    if chev_col < area.width as usize {
                        set_cell(buf, area.x + chev_col as u16, y, '\u{25BE}', dim_fg, row_bg);
                    }
                }
            }
            FieldKind::ToggleGroup { toggles } => {
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
            FieldKind::ButtonRow { buttons } => {
                for (item_id, item_rect) in &visible_field.item_bounds {
                    let button = buttons.iter().find(|b| &b.id == item_id);
                    if let Some(button) = button {
                        let col = area.x as f32 + item_rect.x;
                        let btn_fg = if button.disabled || field.disabled {
                            dim_fg
                        } else {
                            field_fg
                        };
                        let brk_fg = if button.disabled || field.disabled {
                            dim_fg
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
            FieldKind::Toolbar(toolbar) => {
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
            FieldKind::TextArea {
                value,
                placeholder,
                cursor,
                visible_rows,
            } => {
                let shown = if value.is_empty() {
                    placeholder.as_str()
                } else {
                    value.as_str()
                };
                let input_fg = if value.is_empty() { dim_fg } else { field_fg };
                let rows = *visible_rows;

                // Paint each row of the text area.
                let avail_w = (area.width as usize).saturating_sub(2); // inside brackets
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

                    // Left bracket on first column.
                    set_cell(buf, area.x, row_y, '[', dim_fg, row_bg);

                    // Content characters for this row.
                    let mut col = 1usize;
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

                    // Right bracket at last column.
                    let bracket_col = (area.width as usize).saturating_sub(1);
                    if bracket_col < area.width as usize {
                        set_cell(buf, area.x + bracket_col as u16, row_y, ']', dim_fg, row_bg);
                    }
                }

                // Cursor rendering on the first row (same logic as TextInput).
                if let Some(cur) = cursor {
                    if !value.is_empty() {
                        let mut byte = 0usize;
                        let mut char_idx = 0usize;
                        for ch in value.chars().take(avail_w) {
                            if byte >= *cur {
                                break;
                            }
                            byte += ch.len_utf8();
                            char_idx += 1;
                        }
                        let cursor_col = 1 + char_idx;
                        if cursor_col < area.width as usize {
                            let ch = value.chars().nth(char_idx).unwrap_or(' ');
                            set_cell(buf, area.x + cursor_col as u16, y, ch, row_bg, field_fg);
                        }
                    }
                }
            }
            FieldKind::PasswordInput {
                value,
                placeholder,
                cursor,
                mask_char,
            } => {
                // Mask the value: replace each character with mask_char.
                let masked: String = value.chars().map(|_| *mask_char).collect();
                let shown = if value.is_empty() {
                    placeholder.as_str()
                } else {
                    masked.as_str()
                };
                let input_fg = if value.is_empty() { dim_fg } else { field_fg };

                let (start_col, desired) = if no_label {
                    let sc = 1usize;
                    let avail = (area.width as usize).saturating_sub(sc + 2);
                    (sc, shown.chars().count().min(avail))
                } else {
                    let max_input = (area.width as usize * 2 / 3).max(10);
                    let d = shown.chars().count().min(max_input);
                    let sc = (area.width as usize).saturating_sub(d + 2);
                    (sc, d)
                };

                if start_col > label_end + 1 || no_label {
                    if start_col > 0 && start_col - 1 < area.width as usize {
                        set_cell(buf, area.x + (start_col - 1) as u16, y, '[', dim_fg, row_bg);
                    }
                    let mut col = start_col;
                    for ch in shown.chars().take(desired) {
                        if col >= area.width as usize {
                            break;
                        }
                        set_cell(buf, area.x + col as u16, y, ch, input_fg, row_bg);
                        col += 1;
                    }
                    let bracket_col = if no_label {
                        (area.width as usize).saturating_sub(1)
                    } else {
                        col
                    };
                    if bracket_col < area.width as usize {
                        set_cell(buf, area.x + bracket_col as u16, y, ']', dim_fg, row_bg);
                    }

                    // Cursor (byte offset into original value, rendered at
                    // the masked character position).
                    if let Some(cur) = cursor {
                        if !value.is_empty() {
                            let mut byte = 0usize;
                            let mut char_idx = 0usize;
                            for ch in value.chars().take(desired) {
                                if byte >= *cur {
                                    break;
                                }
                                byte += ch.len_utf8();
                                char_idx += 1;
                            }
                            let cursor_col = start_col + char_idx;
                            if cursor_col < area.width as usize {
                                let ch = masked.chars().nth(char_idx).unwrap_or(' ');
                                set_cell(buf, area.x + cursor_col as u16, y, ch, row_bg, field_fg);
                            }
                        }
                    }
                }
            }
            FieldKind::SegmentedControl {
                options,
                selected_idx,
            } => {
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
                        let opt_fg = if i == *selected_idx {
                            accent_fg
                        } else {
                            dim_fg
                        };
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
        }

        // ── Validation indicator ─────────────────────────────────────
        // Render a colored prefix character at column 0 on the field's
        // first row. This avoids adding extra rows that would break TUI
        // layout (all non-TextArea fields are 1 cell tall).
        if let Some(ref vs) = field.validation {
            let (indicator, v_fg) = match vs {
                ValidationState::Error(_) => ('!', error_fg),
                ValidationState::Warning(_) => ('\u{26A0}', warning_fg),
            };
            set_cell(buf, area.x, y, indicator, v_fg, row_bg);
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
                },
                FormField {
                    id: WidgetId::new("wrap"),
                    label: label("wrap"),
                    kind: FieldKind::Toggle { value: true },
                    disabled: false,
                    validation: None,
                    hint: label(""),
                },
            ],
            focused_field: Some(WidgetId::new("wrap")),
            scroll_offset: 0,
            has_focus: true,
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

        // Toggle row: "wrap" label + "[x]" right-aligned.
        let row1: String = (0..5).map(|x| cell_char(&buf, x, 1)).collect();
        assert!(row1.contains("wrap"));
        // "[x]" near the right edge.
        let mut found_x = false;
        for x in 20..30 {
            if cell_char(&buf, x, 1) == 'x' {
                found_x = true;
            }
        }
        assert!(found_x, "expected '[x]' near right edge");
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
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
        };
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 3));
        draw_form(&mut buf, Rect::new(0, 0, 30, 3), &f, &Theme::default());

        // Row fills full width: '[' at col 0, text at col 1, ']' at col 29.
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
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
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

    #[test]
    fn toggle_group_click_hits_correct_item() {
        let area = Rect::new(0, 0, 40, 3);
        let mut buf = Buffer::empty(area);
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

        // Find where "Aa" is painted (first toggle).
        let mut aa_col = None;
        for x in 0..40u16 {
            if cell_char(&buf, x, 0) == 'A' {
                if x + 1 < 40 && cell_char(&buf, x + 1, 0) == 'a' {
                    aa_col = Some(x);
                    break;
                }
            }
        }
        let aa_col = aa_col.expect("'Aa' must be painted");
        let hit = layout.hit_test(aa_col as f32, 0.0);
        assert_eq!(hit, FormHit::Field(WidgetId::new("case")));

        // Find where "Ab|" is painted (second toggle).
        let mut ab_col = None;
        for x in (aa_col + 2)..40u16 {
            if cell_char(&buf, x, 0) == 'A' {
                if x + 1 < 40 && cell_char(&buf, x + 1, 0) == 'b' {
                    ab_col = Some(x);
                    break;
                }
            }
        }
        let ab_col = ab_col.expect("'Ab|' must be painted");
        let hit = layout.hit_test(ab_col as f32, 0.0);
        assert_eq!(hit, FormHit::Field(WidgetId::new("word")));

        // Find ".*" (third toggle).
        let mut dot_col = None;
        for x in (ab_col + 3)..40u16 {
            if cell_char(&buf, x, 0) == '.' && x + 1 < 40 && cell_char(&buf, x + 1, 0) == '*' {
                dot_col = Some(x);
                break;
            }
        }
        let dot_col = dot_col.expect("'.*' must be painted");
        let hit = layout.hit_test(dot_col as f32, 0.0);
        assert_eq!(hit, FormHit::Field(WidgetId::new("regex")));
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
                            id: WidgetId::new("next"),
                            label: "Next".into(),
                            disabled: false,
                            icon: None,
                        },
                        ButtonRowItem {
                            id: WidgetId::new("replace"),
                            label: "Repl".into(),
                            disabled: false,
                            icon: None,
                        },
                        ButtonRowItem {
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
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
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

    #[test]
    fn button_row_click_hits_correct_item() {
        let area = Rect::new(0, 0, 40, 3);
        let mut buf = Buffer::empty(area);
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

        // Find "[Next]" — the 'N' inside brackets.
        let mut next_col = None;
        for x in 0..40u16 {
            if cell_char(&buf, x, 0) == '[' && x + 1 < 40 && cell_char(&buf, x + 1, 0) == 'N' {
                next_col = Some(x);
                break;
            }
        }
        let next_col = next_col.expect("'[Next]' must be painted");
        let hit = layout.hit_test(next_col as f32, 0.0);
        assert_eq!(hit, FormHit::Field(WidgetId::new("next")));

        // Click inside "Next" text (col + 2).
        let hit = layout.hit_test((next_col + 2) as f32, 0.0);
        assert_eq!(hit, FormHit::Field(WidgetId::new("next")));

        // Find "[Repl]".
        let mut repl_col = None;
        for x in (next_col + 5)..40u16 {
            if cell_char(&buf, x, 0) == '[' && x + 1 < 40 && cell_char(&buf, x + 1, 0) == 'R' {
                repl_col = Some(x);
                break;
            }
        }
        let repl_col = repl_col.expect("'[Repl]' must be painted");
        let hit = layout.hit_test(repl_col as f32, 0.0);
        assert_eq!(hit, FormHit::Field(WidgetId::new("replace")));

        // Find "[All]".
        let mut all_col = None;
        for x in (repl_col + 5)..40u16 {
            if cell_char(&buf, x, 0) == '[' && x + 1 < 40 && cell_char(&buf, x + 1, 0) == 'A' {
                all_col = Some(x);
                break;
            }
        }
        let all_col = all_col.expect("'[All]' must be painted");
        let hit = layout.hit_test(all_col as f32, 0.0);
        assert_eq!(hit, FormHit::Field(WidgetId::new("all")));
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
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
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
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
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
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
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
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
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
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
        };
        draw_form(&mut buf, Rect::new(0, 0, 40, 6), &form, &Theme::default());
        let layout = tui_form_layout(&form, Rect::new(0, 0, 40, 6));
        assert_eq!(
            layout.visible_fields[0].bounds.height, 3.0,
            "TextArea with visible_rows=3 should be 3 cells tall"
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
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
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
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
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
                }),
                hint: label(""),
                disabled: false,
                validation: None,
            }],
            focused_field: None,
            scroll_offset: 0,
            has_focus: false,
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
}
