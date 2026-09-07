//! TUI rasteriser for the focus-ring convention (issue #830).
//!
//! Draws a border-only box (rounded corners, matching `dialog::draw_dialog`'s
//! glyph set) around whichever widget [`crate::focus::FocusManager`]
//! currently reports as focused. Unlike `draw_dialog`'s bordered box, this
//! never clears its interior: it only overwrites the four edge cells,
//! reading each one's *existing* background before repainting its
//! glyph/foreground so the focused widget's own content keeps showing
//! through underneath the ring.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect as RRect;
use ratatui::style::Color as RatatuiColor;

use super::{ratatui_color, set_cell};
use crate::theme::Theme;

/// Paint the focus ring around `rect`. No-op if `rect` is degenerate
/// (zero width or height).
pub fn draw_focus_ring(buf: &mut Buffer, rect: RRect, theme: &Theme) {
    let fg = ratatui_color(theme.accent_fg);
    let (x, y, w, h) = (rect.x, rect.y, rect.width, rect.height);
    if w == 0 || h == 0 {
        return;
    }

    let bg_at = |buf: &Buffer, cx: u16, cy: u16| -> RatatuiColor {
        let area = buf.area;
        if cx >= area.x && cy >= area.y && cx < area.x + area.width && cy < area.y + area.height {
            buf[(cx, cy)].bg
        } else {
            RatatuiColor::Reset
        }
    };

    // Top border.
    for col in 0..w {
        let ch = if col == 0 {
            '╭'
        } else if col == w - 1 {
            '╮'
        } else {
            '─'
        };
        set_cell(buf, x + col, y, ch, fg, bg_at(buf, x + col, y));
    }
    // Bottom border (distinct row only when h > 1 — otherwise this would
    // repaint the same row the top border just wrote).
    if h > 1 {
        for col in 0..w {
            let ch = if col == 0 {
                '╰'
            } else if col == w - 1 {
                '╯'
            } else {
                '─'
            };
            set_cell(buf, x + col, y + h - 1, ch, fg, bg_at(buf, x + col, y + h - 1));
        }
    }
    // Left/right borders for interior rows.
    for row in (y + 1)..(y + h.saturating_sub(1)) {
        set_cell(buf, x, row, '│', fg, bg_at(buf, x, row));
        set_cell(buf, x + w - 1, row, '│', fg, bg_at(buf, x + w - 1, row));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Cell;

    fn filled_buffer(w: u16, h: u16, bg: RatatuiColor) -> Buffer {
        let mut buf = Buffer::empty(RRect::new(0, 0, w, h));
        for y in 0..h {
            for x in 0..w {
                let mut cell = Cell::default();
                cell.set_char('x').set_bg(bg);
                buf[(x, y)] = cell;
            }
        }
        buf
    }

    #[test]
    fn draws_corners_and_edges() {
        let theme = Theme::default();
        let mut buf = filled_buffer(10, 5, RatatuiColor::Black);
        draw_focus_ring(&mut buf, RRect::new(1, 1, 5, 3), &theme);

        assert_eq!(buf[(1, 1)].symbol(), "╭");
        assert_eq!(buf[(5, 1)].symbol(), "╮");
        assert_eq!(buf[(1, 3)].symbol(), "╰");
        assert_eq!(buf[(5, 3)].symbol(), "╯");
        assert_eq!(buf[(2, 1)].symbol(), "─");
        assert_eq!(buf[(1, 2)].symbol(), "│");
    }

    #[test]
    fn preserves_existing_background() {
        let theme = Theme::default();
        let bg = RatatuiColor::Rgb(10, 20, 30);
        let mut buf = filled_buffer(10, 5, bg);
        draw_focus_ring(&mut buf, RRect::new(1, 1, 5, 3), &theme);

        assert_eq!(buf[(1, 1)].bg, bg, "ring cells must keep the existing bg");
        // Interior (non-border) cells are untouched entirely.
        assert_eq!(buf[(3, 2)].symbol(), "x");
        assert_eq!(buf[(3, 2)].bg, bg);
    }

    #[test]
    fn zero_size_rect_is_a_noop() {
        let theme = Theme::default();
        let mut buf = filled_buffer(10, 5, RatatuiColor::Black);
        draw_focus_ring(&mut buf, RRect::new(1, 1, 0, 3), &theme);
        // Every cell keeps its pre-existing content — nothing was painted.
        for y in 0..5u16 {
            for x in 0..10u16 {
                assert_eq!(buf[(x, y)].symbol(), "x");
            }
        }
    }
}
