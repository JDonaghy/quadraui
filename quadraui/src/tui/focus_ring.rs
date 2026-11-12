//! TUI rasteriser for the focus-ring convention.
//!
//! Draws a border-only box (rounded corners, matching `dialog::draw_dialog`'s
//! glyph set) around whichever widget [`crate::focus::FocusManager`]
//! currently reports as focused. Unlike `draw_dialog`'s bordered box, this
//! never clears its interior: it only overwrites the four edge cells,
//! reading each one's *existing* background before repainting its
//! glyph/foreground so the focused widget's own content keeps showing
//! through underneath the ring.
//!
//! A one-row-tall `rect` (e.g. a `Form` `ButtonRow`/`ToggleGroup` item —
//! see `compose::form_controller`'s "Item-level keyboard focus" doc
//! section) has no interior left for side walls once a top border is
//! drawn, so the box would consume the entire row and erase its label.
//! [`draw_focus_ring`] special-cases that height with an underline
//! instead — decorating the row's existing content rather than
//! overwriting it with border glyphs.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect as RRect;
use ratatui::style::{Color as RatatuiColor, Modifier};

use super::{ratatui_color, set_cell};
use crate::theme::Theme;

/// Paint the focus ring around `rect`. No-op if `rect` is degenerate
/// (zero width or height). A one-row-tall `rect` gets an underline
/// instead of a box — see this module's doc.
pub fn draw_focus_ring(buf: &mut Buffer, rect: RRect, theme: &Theme) {
    let fg = ratatui_color(theme.accent_fg);
    let (x, y, w, h) = (rect.x, rect.y, rect.width, rect.height);
    if w == 0 || h == 0 {
        return;
    }

    if h == 1 {
        let area = buf.area;
        for col in 0..w {
            let cx = x + col;
            if cx >= area.x && y >= area.y && cx < area.x + area.width && y < area.y + area.height {
                let cell = &mut buf[(cx, y)];
                cell.modifier |= Modifier::UNDERLINED;
                cell.underline_color = fg;
            }
        }
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
            set_cell(
                buf,
                x + col,
                y + h - 1,
                ch,
                fg,
                bg_at(buf, x + col, y + h - 1),
            );
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
    fn one_row_tall_rect_underlines_instead_of_drawing_a_box() {
        let theme = Theme::default();
        let bg = RatatuiColor::Rgb(10, 20, 30);
        let mut buf = filled_buffer(10, 5, bg);
        draw_focus_ring(&mut buf, RRect::new(1, 1, 5, 1), &theme);

        // The row's existing glyph/background survive — only the
        // modifier/underline colour change, unlike the boxed case where
        // border glyphs overwrite the cell's symbol outright.
        for x in 1..6u16 {
            assert_eq!(buf[(x, 1)].symbol(), "x");
            assert_eq!(buf[(x, 1)].bg, bg);
            assert!(buf[(x, 1)].modifier.contains(Modifier::UNDERLINED));
            assert_eq!(buf[(x, 1)].underline_color, ratatui_color(theme.accent_fg));
        }
        // Rows above/below the one-row rect are untouched.
        assert!(!buf[(2, 0)].modifier.contains(Modifier::UNDERLINED));
        assert!(!buf[(2, 2)].modifier.contains(Modifier::UNDERLINED));
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

    /// End-to-end regression for a mouse-clicked layout-tree `Button`
    /// repainting as an empty bordered box with its label gone: a `Button`
    /// painted into a cell taller than its one row of chrome (an
    /// `EqualShare`-sized `Column` slot, say — see
    /// `crate::tui::button`'s own doc), then this same cell's rect fed to
    /// [`draw_focus_ring`] once the click gives it focus (the host's
    /// generic focus-ring convention — the only one a layout-tree `Button`
    /// has, since its own descriptor carries no `focused` state). Before
    /// `crate::tui::button::draw_button` centred the label, it painted on
    /// the cell's own top row, which is exactly the ring's own top border
    /// — the ring's `╭──╮` silently replaced the button's `[ OK ]`.
    #[test]
    fn focus_ring_over_a_tall_button_cell_does_not_erase_the_centred_label() {
        use crate::primitives::button::Button;
        use crate::tui::button::draw_button;
        use crate::types::WidgetId;

        let theme = Theme::default();
        let rect = RRect::new(0, 0, 8, 3);
        let mut buf = Buffer::empty(rect);
        let button = Button::new(WidgetId::new("b"), "OK");
        draw_button(&mut buf, rect, &button, &theme, false);

        draw_focus_ring(&mut buf, rect, &theme);

        let row: String = (0..rect.width)
            .map(|x| buf[(x, 1)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            row.contains("OK"),
            "the label must survive the ring on the middle row, got {row:?}"
        );
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
