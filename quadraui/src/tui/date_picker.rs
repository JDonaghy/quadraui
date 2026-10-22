//! TUI rasteriser for [`crate::primitives::date_picker::DatePicker`].
//!
//! Paints a one-row month/year header with prev/next controls, a 7-column
//! × 6-row day grid, and an optional one-row time field below. One TUI
//! cell per day row/column — no sub-cell fractional painting, matching
//! `board.rs`/`data_table.rs`'s cell-granular style.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color as RatatuiColor;

use super::{ratatui_color, set_cell};
use crate::primitives::date_picker::{DatePicker, DatePickerLayout};
use crate::theme::Theme;

/// Row height in TUI cells for the header, each day row, and the time row.
const TUI_DATE_PICKER_ROW_H: f32 = 1.0;

/// Resolve the layout `draw_date_picker` paints `picker` at, without
/// touching a buffer.
///
/// This is the **single** definition of TUI DatePicker geometry: both
/// [`draw_date_picker`] and `Backend::date_picker_layout` go through it,
/// so the layout a cache-free, layout-on-demand hit-test runs against is
/// bit-for-bit the layout that was painted.
pub fn tui_date_picker_layout(area: Rect, picker: &DatePicker) -> DatePickerLayout {
    let cell_width = area.width as f32 / 7.0;
    let time_height = if picker.time.is_some() {
        TUI_DATE_PICKER_ROW_H
    } else {
        0.0
    };
    picker.layout(
        area.x as f32,
        area.y as f32,
        cell_width,
        TUI_DATE_PICKER_ROW_H,
        TUI_DATE_PICKER_ROW_H,
        time_height,
    )
}

/// Fill one row of `rect` (clipped to `area`) with `bg`, so a selected or
/// focused day cell's whole cell-width is tinted, not just the glyph.
fn fill_row(buf: &mut Buffer, area: Rect, rect: crate::event::Rect, bg: RatatuiColor) {
    let y = rect.y.round() as i32;
    if y < area.y as i32 || y >= (area.y + area.height) as i32 {
        return;
    }
    let x0 = rect.x.round() as i32;
    let w = rect.width.round() as i32;
    for dx in 0..w {
        let cx = x0 + dx;
        if cx >= area.x as i32 && cx < (area.x + area.width) as i32 {
            set_cell(buf, cx as u16, y as u16, ' ', bg, bg);
        }
    }
}

/// Draw `text` left-aligned within one row of `rect`, clipped to `area`.
fn draw_row_text(
    buf: &mut Buffer,
    area: Rect,
    rect: crate::event::Rect,
    text: &str,
    fg: RatatuiColor,
    bg: RatatuiColor,
) {
    let y = rect.y.round() as i32;
    if y < area.y as i32 || y >= (area.y + area.height) as i32 {
        return;
    }
    let x0 = rect.x.round() as i32;
    let w = rect.width.round() as i32;
    for (i, ch) in text.chars().enumerate() {
        let cx = x0 + i as i32;
        if cx >= x0 + w {
            break;
        }
        if cx >= area.x as i32 && cx < (area.x + area.width) as i32 {
            set_cell(buf, cx as u16, y as u16, ch, fg, bg);
        }
    }
}

/// Draw a [`DatePicker`] into `area`: month/year header with prev/next
/// controls, the day grid, and the time row (when `picker.time` is
/// `Some`). Returns the layout used for painting so callers can hit-test
/// at the same coordinates.
pub fn draw_date_picker(
    buf: &mut Buffer,
    area: Rect,
    picker: &DatePicker,
    theme: &Theme,
) -> DatePickerLayout {
    let layout = tui_date_picker_layout(area, picker);

    if area.width == 0 || area.height == 0 {
        return layout;
    }

    let fg = ratatui_color(theme.foreground);
    let muted_fg = ratatui_color(theme.muted_fg);
    let accent_fg = ratatui_color(theme.accent_fg);
    let accent_bg = ratatui_color(theme.accent_bg);
    let bg = ratatui_color(theme.background);
    let sel_bg = ratatui_color(theme.selection_bg);

    if let Some(header) = layout.header_bounds {
        let label = format!(
            "{:04}-{:02}",
            picker.visible_month.year, picker.visible_month.month
        );
        draw_row_text(buf, area, header, &label, fg, bg);
    }
    if let Some(prev) = layout.prev_bounds {
        draw_row_text(buf, area, prev, "<", muted_fg, bg);
    }
    if let Some(next) = layout.next_bounds {
        draw_row_text(buf, area, next, ">", muted_fg, bg);
    }

    for cell in &layout.day_cells {
        if cell.bounds.width <= 0.0 || cell.bounds.height <= 0.0 {
            continue;
        }
        let cell_bg = if cell.is_selected {
            accent_bg
        } else if cell.is_focused && picker.has_focus {
            sel_bg
        } else {
            bg
        };
        let cell_fg = if cell.is_selected {
            accent_fg
        } else if !cell.in_visible_month {
            muted_fg
        } else {
            fg
        };
        fill_row(buf, area, cell.bounds, cell_bg);
        let label = format!("{:>2}", cell.date.day);
        draw_row_text(buf, area, cell.bounds, &label, cell_fg, cell_bg);
    }

    if let Some(time_bounds) = layout.time_bounds {
        if let Some(time) = picker.time {
            draw_row_text(buf, area, time_bounds, &time.format(), fg, bg);
        }
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::date_picker::CalendarDate;
    use crate::types::WidgetId;

    fn theme() -> Theme {
        Theme::default()
    }

    #[test]
    fn layout_and_paint_agree_on_day_cell_count() {
        let picker = DatePicker::new(WidgetId::new("date_picker"), CalendarDate::new(2024, 3, 15));
        let area = Rect::new(0, 0, 21, 10);
        let layout = tui_date_picker_layout(area, &picker);
        assert_eq!(layout.day_cells.len(), 42);

        let mut buf = Buffer::empty(area);
        let painted = draw_date_picker(&mut buf, area, &picker, &theme());
        assert_eq!(painted.day_cells.len(), layout.day_cells.len());
    }

    #[test]
    fn zero_area_returns_layout_without_panicking() {
        let picker = DatePicker::new(WidgetId::new("date_picker"), CalendarDate::new(2024, 3, 15));
        let area = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        let layout = draw_date_picker(&mut buf, area, &picker, &theme());
        assert_eq!(layout.day_cells.len(), 42);
    }
}
