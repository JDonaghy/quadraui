//! TUI rasteriser for [`crate::DatePicker`].
//!
//! Paints the month/year header with prev/next controls, the 42-cell day
//! grid (focus ring on `is_focused`, fill on `is_selected`, dimmed text for
//! days outside `in_visible_month`), and — when `layout.time_bounds` is
//! `Some` — the time field, mirroring
//! [`crate::primitives::date_picker::native_surface_paint::paint`]'s pixel
//! rasteriser but through direct cell writes, the same split
//! [`crate::tui::combo_box`] uses against
//! [`crate::primitives::combo_box::native_surface_paint::paint`].

use ratatui::buffer::Buffer;

use super::set_cell;
use crate::primitives::date_picker::{DatePicker, DatePickerLayout};
use crate::theme::Theme;

fn fill_text(
    buf: &mut Buffer,
    rect: crate::event::Rect,
    text: &str,
    fg: ratatui::style::Color,
    bg: ratatui::style::Color,
) {
    fill_text_aligned(buf, rect, text, fg, bg, false);
}

/// Fill `rect` with `bg` and draw `text`, either left-aligned (`centered =
/// false`) or centered (`centered = true`). The header label is centered so
/// it does not collide with the prev/next glyphs drawn into the header's
/// edge columns.
fn fill_text_aligned(
    buf: &mut Buffer,
    rect: crate::event::Rect,
    text: &str,
    fg: ratatui::style::Color,
    bg: ratatui::style::Color,
    centered: bool,
) {
    let x0 = rect.x.round() as u16;
    let y0 = rect.y.round() as u16;
    let w = rect.width.round() as u16;
    let h = rect.height.round() as u16;
    for row in 0..h {
        for col in 0..w {
            set_cell(buf, x0 + col, y0 + row, ' ', fg, bg);
        }
    }
    let len = text.chars().count() as u16;
    let start = if centered && len < w {
        (w - len) / 2
    } else {
        0
    };
    for (i, ch) in text
        .chars()
        .take(w.saturating_sub(start) as usize)
        .enumerate()
    {
        set_cell(buf, x0 + start + i as u16, y0, ch, fg, bg);
    }
}

fn stroke_corners(
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

/// Draw a [`DatePicker`] at its resolved `layout`.
pub fn draw_date_picker(
    buf: &mut Buffer,
    date_picker: &DatePicker,
    layout: &DatePickerLayout,
    theme: &Theme,
) {
    let bg = ratatui::style::Color::Reset;
    let header_fg = super::qc(theme.header_fg);
    let header_bg = super::qc(theme.header_bg);

    if let Some(header) = layout.header_bounds {
        if header.width > 0.0 && header.height > 0.0 {
            let label = format!(
                "{:04}-{:02}",
                date_picker.visible_month.year, date_picker.visible_month.month
            );
            fill_text_aligned(buf, header, &label, header_fg, header_bg, true);
        }
    }
    if let Some(prev) = layout.prev_bounds {
        if prev.width > 0.0 && prev.height > 0.0 {
            fill_text(buf, prev, "<", header_fg, header_bg);
        }
    }
    if let Some(next) = layout.next_bounds {
        if next.width > 0.0 && next.height > 0.0 {
            fill_text(buf, next, ">", header_fg, header_bg);
        }
    }

    let selected_bg = super::qc(theme.selected_bg);
    let accent_fg = super::qc(theme.accent_fg);
    let surface_fg = super::qc(theme.surface_fg);
    let muted_fg = super::qc(theme.muted_fg);

    for cell in &layout.day_cells {
        if cell.bounds.width <= 0.0 || cell.bounds.height <= 0.0 {
            continue;
        }
        let cell_bg = if cell.is_selected { selected_bg } else { bg };
        let fg = if cell.in_visible_month {
            surface_fg
        } else {
            muted_fg
        };
        fill_text(buf, cell.bounds, &cell.date.day.to_string(), fg, cell_bg);
        if cell.is_focused {
            stroke_corners(buf, cell.bounds, accent_fg, cell_bg);
        }
    }

    if let Some(time) = layout.time_bounds {
        if time.width > 0.0 && time.height > 0.0 {
            let input_bg = super::qc(theme.input_bg);
            let border_fg = super::qc(theme.border_fg);
            let text = date_picker.time.map(|t| t.format()).unwrap_or_default();
            fill_text(buf, time, &text, surface_fg, input_bg);
            stroke_corners(buf, time, border_fg, input_bg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Rect;
    use crate::primitives::date_picker::{CalendarDate, DatePickerHit};
    use crate::types::WidgetId;
    use ratatui::layout::Rect as RRect;

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn mk_date_picker() -> DatePicker {
        let mut picker = DatePicker::new(WidgetId::new("date"), CalendarDate::new(2024, 3, 15));
        picker.has_focus = true;
        picker
    }

    fn paint_and_click_round_trip_at(origin_x: u16, origin_y: u16) {
        let date_picker = mk_date_picker();
        let layout = date_picker.layout(origin_x as f32, origin_y as f32, 4.0, 1.0, 1.0, 0.0);

        let rect = RRect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(rect);
        draw_date_picker(&mut buf, &date_picker, &layout, &Theme::default());

        // Header label is centered in the header row, clear of the prev/next
        // glyphs painted into the header's edge columns.
        let header = layout.header_bounds.expect("header is reserved");
        let label_len = 7u16; // "2024-03"
        let label_x = header.x as u16 + (header.width as u16 - label_len) / 2;
        assert_eq!(cell_char(&buf, label_x, origin_y), '2');
        // Prev/next glyphs still own the header's edge columns.
        assert_eq!(cell_char(&buf, origin_x, origin_y), '<');

        let focused_cell = layout
            .day_cells
            .iter()
            .find(|c| c.is_focused)
            .expect("focused day is in the grid");
        // Focused day's top-left corner gets the focus-ring glyph.
        assert_eq!(
            cell_char(
                &buf,
                focused_cell.bounds.x as u16,
                focused_cell.bounds.y as u16
            ),
            '┌'
        );

        let p = Rect::new(focused_cell.bounds.x + 1.0, focused_cell.bounds.y, 0.0, 0.0);
        assert_eq!(
            layout.hit_test(p.x, p.y),
            DatePickerHit::Day(focused_cell.date)
        );
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
