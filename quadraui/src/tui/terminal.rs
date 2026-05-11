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

    let sb_cols: u16 = match &term.scrollbar {
        Some(sb) => sb.width.unwrap_or(1),
        None => 0,
    };
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
            sb_state.effective_scroll_offset() as f32,
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

/// Helper: find the top-most and bottom-most rows containing the thumb
/// glyph (`█`) in the scrollbar column.
#[cfg(test)]
fn find_thumb_extent(buf: &Buffer, sb_col: u16, y0: u16, height: u16) -> Option<(u16, u16)> {
    let mut first = None;
    let mut last = None;
    for dy in 0..height {
        let y = y0 + dy;
        if buf[(sb_col, y)].symbol() == "█" {
            if first.is_none() {
                first = Some(dy);
            }
            last = Some(dy);
        }
    }
    first.zip(last)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::terminal::{TerminalCell, TerminalScrollbar};
    use crate::types::Color;

    fn blank_cell() -> TerminalCell {
        TerminalCell {
            ch: ' ',
            fg: Color::rgb(200, 200, 200),
            bg: Color::rgb(0, 0, 0),
            bold: false,
            italic: false,
            underline: false,
            selected: false,
            is_cursor: false,
            is_find_match: false,
            is_find_active: false,
        }
    }

    fn make_terminal(rows: usize, cols: usize, sb: Option<TerminalScrollbar>) -> Terminal {
        let row = vec![blank_cell(); cols];
        Terminal {
            id: "test-term".into(),
            cells: vec![row; rows],
            scrollbar: sb,
        }
    }

    #[test]
    fn inverted_offset_zero_thumb_at_bottom() {
        let sb = TerminalScrollbar {
            total_lines: 500,
            visible_lines: 20,
            scroll_offset: 0,
            inverted: true,
            width: None,
        };
        let term = make_terminal(20, 39, Some(sb));
        let area = Rect::new(0, 0, 40, 20);
        let theme = Theme::default();
        let mut buf = Buffer::empty(area);
        draw_terminal(&mut buf, area, &term, &theme);

        let (top, bot) = find_thumb_extent(&buf, 39, 0, 20).expect("thumb should be painted");
        // effective_scroll_offset = 480 → thumb near track bottom
        assert!(
            bot >= 18,
            "inverted offset=0: thumb bottom ({bot}) should be at/near row 19"
        );
        assert!(
            top > 0,
            "inverted offset=0: thumb top ({top}) should NOT be at row 0"
        );
    }

    #[test]
    fn inverted_offset_max_thumb_at_top() {
        let sb = TerminalScrollbar {
            total_lines: 500,
            visible_lines: 20,
            scroll_offset: 480,
            inverted: true,
            width: None,
        };
        let term = make_terminal(20, 39, Some(sb));
        let area = Rect::new(0, 0, 40, 20);
        let theme = Theme::default();
        let mut buf = Buffer::empty(area);
        draw_terminal(&mut buf, area, &term, &theme);

        let (top, _bot) = find_thumb_extent(&buf, 39, 0, 20).expect("thumb should be painted");
        // effective_scroll_offset = 0 → thumb at track top
        assert_eq!(top, 0, "inverted offset=max: thumb should start at row 0");
    }

    #[test]
    fn non_inverted_offset_zero_thumb_at_top() {
        let sb = TerminalScrollbar {
            total_lines: 500,
            visible_lines: 20,
            scroll_offset: 0,
            inverted: false,
            width: None,
        };
        let term = make_terminal(20, 39, Some(sb));
        let area = Rect::new(0, 0, 40, 20);
        let theme = Theme::default();
        let mut buf = Buffer::empty(area);
        draw_terminal(&mut buf, area, &term, &theme);

        let (top, _bot) = find_thumb_extent(&buf, 39, 0, 20).expect("thumb should be painted");
        assert_eq!(top, 0, "non-inverted offset=0: thumb should start at row 0");
    }
}
