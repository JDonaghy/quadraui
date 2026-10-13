//! TUI rasteriser for [`crate::ComboBox`].
//!
//! Paints the field (border + current text) exactly like
//! [`crate::tui::text_input`], then — when `layout.dropdown` is
//! `Some` — the filtered option list beneath it, in the same row
//! chrome as [`crate::tui::draw_completions`]. Row labels resolve
//! through [`ComboBox::filtered_indices`] back into `combo_box.options`,
//! mirroring [`crate::primitives::combo_box::native_surface_paint::paint`].

use ratatui::buffer::Buffer;

use super::{qc, set_cell};
use crate::primitives::combo_box::{ComboBox, ComboBoxLayout};
use crate::theme::Theme;

fn stroke_rect(
    buf: &mut Buffer,
    rect: crate::event::Rect,
    fg: ratatui::style::Color,
    bg: ratatui::style::Color,
) {
    let x0 = rect.x.round() as u16;
    let y0 = rect.y.round() as u16;
    let w = rect.width.round() as u16;
    let h = rect.height.round() as u16;
    if w == 0 || h == 0 {
        return;
    }
    let x1 = x0 + w.saturating_sub(1);
    let y1 = y0 + h.saturating_sub(1);
    for x in x0..=x1 {
        set_cell(buf, x, y0, '─', fg, bg);
        if y1 != y0 {
            set_cell(buf, x, y1, '─', fg, bg);
        }
    }
    for y in y0..=y1 {
        set_cell(buf, x0, y, '│', fg, bg);
        if x1 != x0 {
            set_cell(buf, x1, y, '│', fg, bg);
        }
    }
    set_cell(buf, x0, y0, '┌', fg, bg);
    if x1 != x0 {
        set_cell(buf, x1, y0, '┐', fg, bg);
    }
    if y1 != y0 {
        set_cell(buf, x0, y1, '└', fg, bg);
        if x1 != x0 {
            set_cell(buf, x1, y1, '┘', fg, bg);
        }
    }
}

fn fill_rect(
    buf: &mut Buffer,
    rect: crate::event::Rect,
    ch: char,
    fg: ratatui::style::Color,
    bg: ratatui::style::Color,
) {
    let x0 = rect.x.round() as u16;
    let y0 = rect.y.round() as u16;
    let w = rect.width.round() as u16;
    let h = rect.height.round() as u16;
    for y in y0..y0.saturating_add(h) {
        for x in x0..x0.saturating_add(w) {
            set_cell(buf, x, y, ch, fg, bg);
        }
    }
}

fn draw_text(
    buf: &mut Buffer,
    rect: crate::event::Rect,
    text: &str,
    fg: ratatui::style::Color,
    bg: ratatui::style::Color,
) {
    let x0 = rect.x.round() as u16;
    let y0 = rect.y.round() as u16;
    let w = rect.width.round() as u16;
    for (i, ch) in text.chars().take(w as usize).enumerate() {
        set_cell(buf, x0 + i as u16, y0, ch, fg, bg);
    }
}

/// Draw a [`ComboBox`] at its resolved layout.
pub fn draw_combo_box(
    buf: &mut Buffer,
    combo_box: &ComboBox,
    layout: &ComboBoxLayout,
    theme: &Theme,
) {
    let bg = qc(theme.background);
    let input_bg = qc(theme.input_bg);
    let field = layout.field_bounds;
    if field.width > 0.0 && field.height > 0.0 {
        let border = if combo_box.disabled {
            qc(theme.border_fg)
        } else if combo_box.focused {
            qc(theme.accent_fg)
        } else {
            qc(theme.border_fg)
        };
        let fg = if combo_box.disabled {
            qc(theme.muted_fg)
        } else {
            qc(theme.surface_fg)
        };
        fill_rect(buf, field, ' ', fg, input_bg);
        stroke_rect(buf, field, border, bg);
        if !combo_box.text.is_empty() {
            draw_text(buf, field, &combo_box.text, fg, input_bg);
        }
    }

    let Some(dropdown) = &layout.dropdown else {
        return;
    };
    if dropdown.bounds.width <= 0.0 || dropdown.bounds.height <= 0.0 {
        return;
    }

    let pop_bg = qc(theme.completion_bg);
    let sel_bg = qc(theme.completion_selected_bg);
    let pop_fg = qc(theme.completion_fg);
    let border = qc(theme.completion_border);

    let x = dropdown.bounds.x.round() as u16;
    let y = dropdown.bounds.y.round() as u16;
    let w = dropdown.bounds.width.round() as u16;
    if w < 3 {
        return;
    }

    let filtered = combo_box.filtered_indices();
    for visible in &dropdown.visible_items {
        let row_y = y + (visible.bounds.y - dropdown.bounds.y).round() as u16;
        let is_selected = combo_box.highlighted == Some(visible.item_idx);
        let row_bg = if is_selected { sel_bg } else { pop_bg };

        for col in 0..w {
            set_cell(buf, x + col, row_y, ' ', pop_fg, row_bg);
        }
        set_cell(buf, x, row_y, '│', border, pop_bg);
        set_cell(buf, x + w - 1, row_y, '│', border, pop_bg);

        if let Some(&option_idx) = filtered.get(visible.item_idx) {
            let label = &combo_box.options[option_idx];
            let display = format!(" {label}");
            for (j, ch) in display.chars().enumerate() {
                let col = x + 1 + j as u16;
                if col + 1 >= x + w {
                    break;
                }
                set_cell(buf, col, row_y, ch, pop_fg, row_bg);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Point, Rect};
    use crate::primitives::combo_box::ComboBoxHit;
    use crate::primitives::completions::CompletionItemMeasure;
    use crate::types::WidgetId;
    use ratatui::layout::Rect as RRect;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn mk_combo_box() -> ComboBox {
        ComboBox {
            id: WidgetId::new("combo"),
            options: vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()],
            text: String::new(),
            committed: String::new(),
            open: true,
            highlighted: Some(0),
            disabled: false,
            focused: true,
        }
    }

    fn measure(_idx: usize) -> CompletionItemMeasure {
        CompletionItemMeasure { height: 1.0 }
    }

    fn paint_and_click_round_trip_at(origin_x: u16, origin_y: u16) {
        let field_bounds = Rect::new(origin_x as f32, origin_y as f32, 20.0, 1.0);
        let viewport = Rect::new(0.0, 0.0, 80.0, 24.0);
        let combo_box = mk_combo_box();
        let layout = combo_box.layout(field_bounds, viewport, 20.0, 10.0, measure);

        let rect = RRect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(rect);
        draw_combo_box(&mut buf, &combo_box, &layout, &Theme::default());

        // Field border top-left corner paints at the absolute origin.
        assert_eq!(cell_char(&buf, origin_x, origin_y), '┌');

        let dropdown = layout.dropdown.as_ref().expect("dropdown should be open");
        let first_item = dropdown
            .visible_items
            .first()
            .expect("at least one filtered option");
        let row_y = first_item.bounds.y as u16;
        let row_x = dropdown.bounds.x as u16;

        // First option label ("alpha") renders inside the dropdown row,
        // after the left border + 1-cell pad.
        assert_eq!(cell_char(&buf, row_x + 2, row_y), 'a');

        // hit_test: a click on the first dropdown row resolves to the
        // DropdownItem variant for that filtered index.
        let p = Point::new(
            first_item.bounds.x + 1.0,
            first_item.bounds.y + first_item.bounds.height / 2.0,
        );
        assert_eq!(layout.hit_test(p), ComboBoxHit::DropdownItem(0));
    }

    #[test]
    fn paint_and_click_round_trip() {
        paint_and_click_round_trip_at(0, 0);
    }

    /// Paint and hit-test must work correctly when the control is not at
    /// the screen origin.
    #[test]
    fn paint_and_click_round_trip_at_nonzero_origin() {
        paint_and_click_round_trip_at(7, 3);
    }
}
