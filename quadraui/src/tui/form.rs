//! TUI rasteriser for [`crate::Form`].
//!
//! Per D6: this function asks the primitive for a [`crate::FormLayout`]
//! using a uniform 1-cell-per-field measurer (TUI rows are always 1
//! cell tall) and paints the resolved positions verbatim.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{draw_styled_text, ratatui_color, set_cell};
use crate::primitives::form::{FieldKind, Form, FormFieldMeasure};
use crate::theme::Theme;
use crate::types::Decoration;

/// Draw a [`Form`] into `area` on `buf`.
///
/// # Visual contract
///
/// - **Header field** (`FieldKind::Label`): `header_bg` / `header_fg`.
/// - **Focused field**: row gets a `selected_bg` tint with the row's
///   default foreground.
/// - **Disabled field**: foreground replaced with `muted_fg`.
/// - **Toggle**: `[x]` (in `accent_fg` when on) / `[ ]` right-aligned.
/// - **TextInput**: `[value]` right-aligned in brackets; cursor block
///   inverts fg/bg at the cursor position; selection range highlights
///   in `selected_bg`. Empty value renders the placeholder in
///   `muted_fg`.
/// - **Button**: redraws the label as `< text >` right-aligned;
///   brackets in `accent_fg` when focused, `muted_fg` otherwise.
/// - **ReadOnly**: right-aligned dimmed value in `muted_fg`.
/// - **Slider**: `[====----]  N.NN` track with filled cells in
///   `accent_fg`.
/// - **ColorPicker**: `■ #rrggbb` with the swatch glyph painted in
///   the value's RGB.
/// - **Dropdown**: " selected ▾ " right-aligned with the chevron
///   dimmed.
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

    let layout = form.layout(area.width as f32, area.height as f32, |_| {
        FormFieldMeasure::new(1.0)
    });

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

        match &field.kind {
            FieldKind::Label => {
                // No separate input — label spans the row.
            }
            FieldKind::Toggle { value } => {
                let glyph = if *value { "[x]" } else { "[ ]" };
                let w = glyph.chars().count();
                let start_col = (area.width as usize).saturating_sub(w + 2);
                if start_col > label_end + 1 {
                    let input_fg = if *value { accent_fg } else { field_fg };
                    let mut col = start_col;
                    for ch in glyph.chars() {
                        if col >= area.width as usize {
                            break;
                        }
                        set_cell(buf, area.x + col as u16, y, ch, input_fg, row_bg);
                        col += 1;
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
                let max_input = (area.width as usize * 2 / 3).max(10);
                let desired = shown.chars().count().min(max_input);
                let start_col = (area.width as usize).saturating_sub(desired + 2);

                let (sel_lo, sel_hi) = if value.is_empty() {
                    (0, 0)
                } else {
                    match (cursor, selection_anchor) {
                        (Some(c), Some(a)) if c != a => (*c.min(a), *c.max(a)),
                        _ => (0, 0),
                    }
                };

                if start_col > label_end + 1 {
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
                        let cell_bg = if in_selection { sel_bg } else { row_bg };
                        set_cell(buf, area.x + col as u16, y, ch, input_fg, cell_bg);
                        col += 1;
                        byte += ch.len_utf8();
                    }
                    if col < area.width as usize {
                        set_cell(buf, area.x + col as u16, y, ']', dim_fg, row_bg);
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
                // The field's label IS the button caption. Redraw it
                // wrapped in `< text >` on the right side, overwriting the
                // normal label rendering.
                for x in area.x..area.x + (label_end as u16).min(area.width) {
                    set_cell(buf, x, y, ' ', default_fg, row_bg);
                }
                let width = field.label.visible_width() + 4;
                let start_col = (area.width as usize).saturating_sub(width + 1);
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
                let start_col = (area.width as usize).saturating_sub(w + 2);
                if start_col > label_end + 1 {
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
                    let mut col = start_col + 2;
                    for ch in hex.chars() {
                        if col >= area.width as usize {
                            break;
                        }
                        set_cell(buf, area.x + col as u16, y, ch, field_fg, row_bg);
                        col += 1;
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
    let mut x = area.x;
    for ch in header_text.chars() {
        if x >= area.x + area.width {
            break;
        }
        set_cell(buf, x, header_y, ch, header_fg, header_bg);
        x += 1;
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
                    hint: label(""),
                },
                FormField {
                    id: WidgetId::new("wrap"),
                    label: label("wrap"),
                    kind: FieldKind::Toggle { value: true },
                    disabled: false,
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
}
