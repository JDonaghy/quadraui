//! TUI rasteriser for a plain solid-color chrome fill (issue #996).
//!
//! Fills every cell in `rect` with a blank glyph and `color` as both
//! foreground and background. For chrome elements that are a solid
//! color block with no text and no per-row seams — e.g. `AppShell`'s
//! sidebar/editor resize divider, which used to be faked as N stacked
//! one-row `StatusBar`s (exact on a cell grid, wrong on a pixel
//! backend — see `Backend::draw_solid_fill`'s doc for the full story).
//! Unlike [`super::draw_focus_ring`], this clears the interior; there
//! is no existing content to preserve.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect as RRect;

use super::{ratatui_color, set_cell};
use crate::types::Color;

/// Fill every cell in `rect` with `color`. No-op if `rect` is
/// degenerate (zero width or height).
pub fn draw_solid_fill(buf: &mut Buffer, rect: RRect, color: Color) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    let c = ratatui_color(color);
    for row in rect.y..rect.y + rect.height {
        for col in rect.x..rect.x + rect.width {
            set_cell(buf, col, row, ' ', c, c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Cell;
    use ratatui::style::Color as RatatuiColor;

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
    fn fills_every_cell_in_rect_no_gaps() {
        let mut buf = filled_buffer(10, 8, RatatuiColor::Black);
        let color = Color::rgb(100, 100, 110);
        draw_solid_fill(&mut buf, RRect::new(2, 1, 3, 5), color);

        let expected_bg = ratatui_color(color);
        // Every cell inside the rect — including every row boundary a
        // per-row-`StatusBar` loop would have seamed — must be filled.
        for row in 1..6u16 {
            for col in 2..5u16 {
                assert_eq!(
                    buf[(col, row)].symbol(),
                    " ",
                    "cell ({col},{row}) should be blank"
                );
                assert_eq!(
                    buf[(col, row)].bg,
                    expected_bg,
                    "cell ({col},{row}) should be filled with the divider color"
                );
            }
        }
        // Outside the rect is untouched.
        assert_eq!(buf[(1, 1)].symbol(), "x");
        assert_eq!(buf[(5, 1)].symbol(), "x");
    }

    #[test]
    fn zero_size_rect_is_a_noop() {
        let mut buf = filled_buffer(10, 5, RatatuiColor::Black);
        draw_solid_fill(&mut buf, RRect::new(1, 1, 0, 3), Color::rgb(1, 2, 3));
        for y in 0..5u16 {
            for x in 0..10u16 {
                assert_eq!(buf[(x, y)].symbol(), "x");
            }
        }
    }
}
