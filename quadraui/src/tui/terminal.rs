//! TUI rasteriser for [`crate::Terminal`] cell grids.
//!
//! Iterates `cells[row][col]`, writing styled characters into the
//! ratatui buffer with overlay colour logic matching the GTK rasteriser:
//! cursor inverts fg/bg, selection uses `theme.selection_bg`, find-match
//! and find-active use fixed highlight colours.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color as RatatuiColor, Modifier};

use crate::primitives::scrollbar::Scrollbar;
use crate::primitives::terminal::Terminal;
use crate::theme::Theme;

use super::{draw_scrollbar, ratatui_color};

/// Draw a terminal cell grid into `area`, with an optional themed
/// scrollbar on the right edge when `term.scrollbar` is `Some`.
pub fn draw_terminal(buf: &mut Buffer, area: Rect, term: &Terminal, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let sb_cols: u16 = if term.scrollbar.is_some() { 1 } else { 0 };
    let cell_area_w = area.width.saturating_sub(sb_cols);

    for (row_idx, row) in term.cells.iter().enumerate() {
        if row_idx as u16 >= area.height {
            break;
        }
        let y = area.y + row_idx as u16;
        for (col_idx, cell) in row.iter().enumerate() {
            if col_idx as u16 >= cell_area_w {
                break;
            }
            let x = area.x + col_idx as u16;

            let (draw_bg, draw_fg) = resolve_cell_colors(cell, theme);

            let buf_cell = &mut buf[(x, y)];
            buf_cell.set_char(cell.ch).set_fg(draw_fg).set_bg(draw_bg);

            let mut modifier = Modifier::empty();
            if cell.bold {
                modifier |= Modifier::BOLD;
            }
            if cell.italic {
                modifier |= Modifier::ITALIC;
            }
            if cell.underline {
                modifier |= Modifier::UNDERLINED;
            }
            buf_cell.modifier = modifier;
            buf_cell.underline_color = RatatuiColor::Reset;
        }
    }

    if let Some(ref sb_state) = term.scrollbar {
        let track = crate::event::Rect::new(
            (area.x + cell_area_w) as f32,
            area.y as f32,
            1.0,
            area.height as f32,
        );
        let sb = Scrollbar::vertical(
            term.id.clone(),
            track,
            sb_state.scroll_offset as f32,
            sb_state.total_lines as f32,
            sb_state.visible_lines as f32,
            1.0,
        );
        draw_scrollbar(buf, &sb, theme, theme.background);
    }
}

fn resolve_cell_colors(
    cell: &crate::primitives::terminal::TerminalCell,
    theme: &Theme,
) -> (RatatuiColor, RatatuiColor) {
    let bg = ratatui_color(cell.bg);
    let fg = ratatui_color(cell.fg);

    if cell.is_cursor {
        (fg, bg)
    } else if cell.is_find_active {
        (RatatuiColor::Rgb(255, 165, 0), RatatuiColor::Rgb(0, 0, 0))
    } else if cell.is_find_match {
        (RatatuiColor::Rgb(100, 80, 20), fg)
    } else if cell.selected {
        (ratatui_color(theme.selection_bg), fg)
    } else {
        (bg, fg)
    }
}

/// Draw a vertical divider for a terminal split pane.
/// Places `│` characters down the column at `x` using
/// `theme.separator` colour.
pub fn draw_terminal_divider(buf: &mut Buffer, x: u16, y: u16, height: u16, theme: &Theme) {
    let sep_fg = ratatui_color(theme.separator);
    let sep_bg = ratatui_color(theme.background);
    for row in 0..height {
        let cy = y + row;
        if cy < buf.area.y + buf.area.height && x < buf.area.x + buf.area.width {
            let cell = &mut buf[(x, cy)];
            cell.set_char('│').set_fg(sep_fg).set_bg(sep_bg);
            cell.modifier = Modifier::empty();
        }
    }
}
