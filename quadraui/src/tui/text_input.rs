//! TUI rasteriser for [`crate::TextInput`].
//!
//! Paints a 1-cell border, the visible lines, and a block cursor
//! (inverted fg/bg on the cell at `cursor_bounds`). Placeholder
//! renders in `theme.muted_fg` when no content is present.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{qc, set_cell};
use crate::primitives::text_input::{TextInput, TextInputLayout, TextInputMeasure};
use crate::theme::Theme;

pub fn tui_text_input_layout(ti: &TextInput, rect: Rect) -> TextInputLayout {
    let r = crate::event::Rect::new(
        rect.x as f32,
        rect.y as f32,
        rect.width as f32,
        rect.height as f32,
    );
    ti.layout(r, TextInputMeasure::new(1.0, 1.0))
}

pub fn draw_text_input(
    buf: &mut Buffer,
    rect: Rect,
    ti: &TextInput,
    theme: &Theme,
) -> TextInputLayout {
    let layout = tui_text_input_layout(ti, rect);
    if rect.width == 0 || rect.height == 0 {
        return layout;
    }

    let bg = qc(theme.background);
    let fg = qc(theme.foreground);
    let muted = qc(theme.muted_fg);
    let border_fg = if ti.has_focus {
        qc(theme.accent_fg)
    } else {
        qc(theme.border_fg)
    };

    // Fill background + border.
    let x0 = rect.x;
    let y0 = rect.y;
    let x1 = rect.x + rect.width.saturating_sub(1);
    let y1 = rect.y + rect.height.saturating_sub(1);
    for y in y0..=y1 {
        for x in x0..=x1 {
            set_cell(buf, x, y, ' ', fg, bg);
        }
    }
    // Top + bottom border.
    for x in x0..=x1 {
        set_cell(buf, x, y0, '─', border_fg, bg);
        if y1 != y0 {
            set_cell(buf, x, y1, '─', border_fg, bg);
        }
    }
    // Left + right border.
    for y in y0..=y1 {
        set_cell(buf, x0, y, '│', border_fg, bg);
        if x1 != x0 {
            set_cell(buf, x1, y, '│', border_fg, bg);
        }
    }
    // Corners.
    set_cell(buf, x0, y0, '┌', border_fg, bg);
    set_cell(buf, x1, y0, '┐', border_fg, bg);
    set_cell(buf, x0, y1, '└', border_fg, bg);
    set_cell(buf, x1, y1, '┘', border_fg, bg);

    // Content.
    if layout.placeholder_active {
        if let Some(text) = ti.placeholder.as_ref() {
            if let Some(first) = layout.visible_lines.first() {
                let row_x = first.bounds.x as u16;
                let row_y = first.bounds.y as u16;
                let max_w = first.bounds.width as u16;
                for (x, ch) in (row_x..).zip(text.chars().take(max_w as usize)) {
                    set_cell(buf, x, row_y, ch, muted, bg);
                }
            }
        }
    } else {
        let h_scroll = layout.resolved_scroll_col;
        for vis in &layout.visible_lines {
            let line = ti.lines.get(vis.line_idx).map(String::as_str).unwrap_or("");
            let row_x = vis.bounds.x as u16;
            let row_y = vis.bounds.y as u16;
            let max_w = vis.bounds.width as u16;
            for (x, ch) in (row_x..).zip(line.chars().skip(h_scroll).take(max_w as usize)) {
                set_cell(buf, x, row_y, ch, fg, bg);
            }
        }
    }

    // Cursor — block cursor by inverting fg/bg on the cell.
    if ti.has_focus {
        if let Some(cb) = layout.cursor_bounds {
            let cx = cb.x as u16;
            let cy = cb.y as u16;
            let area = buf.area;
            if cx < area.x + area.width && cy < area.y + area.height {
                let cell = &mut buf[(cx, cy)];
                let old_fg = cell.fg;
                let old_bg = cell.bg;
                cell.set_fg(old_bg).set_bg(old_fg);
            }
        }
    }

    layout
}
