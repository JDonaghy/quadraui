//! TUI rasteriser for a plain solid-color chrome fill.
//!
//! An opaque `color` (`a == 255`) fills every cell in `rect` with a blank
//! glyph and `color` as both foreground and background — for chrome
//! elements that are a solid color block with no text and no per-row
//! seams (e.g. a resize divider) with no existing content to preserve.
//! Unlike [`super::draw_focus_ring`], this clears the interior.
//!
//! A translucent `color` (`a < 255`) instead blends `color` over each
//! cell's *existing* background, leaving the glyph and foreground
//! untouched — mirroring the alpha-compositing the pixel backends'
//! `surface_fill_rect` do for the same call, so a modal scrim dims the
//! content behind it instead of blanking it. See [`draw_solid_fill`]'s
//! doc for the one edge case this can't blend exactly.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect as RRect;
use ratatui::style::Color as RatatuiColor;

use super::{ratatui_color, set_cell};
use crate::types::Color;

/// Fill every cell in `rect` with `color`. No-op if `rect` is
/// degenerate (zero width or height).
///
/// `color.a == 255` (the common case — dividers, chrome blocks) blanks
/// every cell: glyph cleared, `fg == bg == color`.
///
/// `color.a < 255` instead alpha-blends `color` over each cell's current
/// background at `color.a / 255` and leaves that cell's glyph and
/// foreground alone, so text already painted there stays legible under
/// the dim. ratatui buffer cells can hold a background that isn't a
/// literal RGB triple (`Color::Reset`, a named colour, `Color::Indexed`)
/// — every rasteriser in this crate paints through [`ratatui_color`],
/// which only ever produces `Color::Rgb`, so this only happens for a
/// cell this crate never painted (e.g. the backend's initial blank
/// buffer). There is no literal colour to blend against in that case, so
/// the fallback is to leave that cell's background unblended rather than
/// guess at one.
pub fn draw_solid_fill(buf: &mut Buffer, rect: RRect, color: Color) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    if color.a == 255 {
        let c = ratatui_color(color);
        for row in rect.y..rect.y + rect.height {
            for col in rect.x..rect.x + rect.width {
                set_cell(buf, col, row, ' ', c, c);
            }
        }
        return;
    }
    let alpha = color.a as f64 / 255.0;
    let area = buf.area;
    for row in rect.y..rect.y + rect.height {
        for col in rect.x..rect.x + rect.width {
            if col < area.x
                || row < area.y
                || col >= area.x + area.width
                || row >= area.y + area.height
            {
                continue;
            }
            let cell = &mut buf[(col, row)];
            if let Some(base) = color_from_ratatui(cell.bg) {
                cell.set_bg(ratatui_color(base.blend(color, alpha)));
            }
        }
    }
}

/// Recover a [`Color`] from a ratatui [`RatatuiColor`] that this crate's
/// own rasterisers produced (always [`RatatuiColor::Rgb`] — see
/// [`ratatui_color`]). Returns `None` for any other variant (`Reset`, a
/// named colour, `Indexed`), which carries no literal RGB to blend
/// against.
fn color_from_ratatui(c: RatatuiColor) -> Option<Color> {
    match c {
        RatatuiColor::Rgb(r, g, b) => Some(Color::rgb(r, g, b)),
        _ => None,
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

    fn filled_buffer_with_fg(w: u16, h: u16, fg: RatatuiColor, bg: RatatuiColor) -> Buffer {
        let mut buf = Buffer::empty(RRect::new(0, 0, w, h));
        for y in 0..h {
            for x in 0..w {
                let mut cell = Cell::default();
                cell.set_char('x').set_fg(fg).set_bg(bg);
                buf[(x, y)] = cell;
            }
        }
        buf
    }

    #[test]
    fn translucent_color_blends_over_existing_rgb_background() {
        let base_bg = Color::rgb(10, 20, 30);
        let existing_fg = ratatui_color(Color::rgb(200, 200, 200));
        let mut buf = filled_buffer_with_fg(5, 5, existing_fg, ratatui_color(base_bg));
        let scrim = Color::rgba(0, 0, 0, 102); // ~0.4 alpha, like a modal scrim

        draw_solid_fill(&mut buf, RRect::new(1, 1, 2, 2), scrim);

        let expected_bg = ratatui_color(base_bg.blend(scrim, 102.0 / 255.0));
        for row in 1..3u16 {
            for col in 1..3u16 {
                // Glyph and fg are untouched — the dimmed content stays
                // legible, only the background darkens.
                assert_eq!(buf[(col, row)].symbol(), "x");
                assert_eq!(buf[(col, row)].fg, existing_fg);
                assert_eq!(buf[(col, row)].bg, expected_bg);
            }
        }
    }

    #[test]
    fn opaque_color_still_blanks_glyph_and_sets_fg_bg() {
        let mut buf = filled_buffer_with_fg(
            4,
            4,
            ratatui_color(Color::rgb(200, 200, 200)),
            ratatui_color(Color::rgb(10, 20, 30)),
        );
        let opaque = Color::rgb(5, 5, 5);

        draw_solid_fill(&mut buf, RRect::new(0, 0, 4, 4), opaque);

        let c = ratatui_color(opaque);
        for row in 0..4u16 {
            for col in 0..4u16 {
                assert_eq!(buf[(col, row)].symbol(), " ");
                assert_eq!(buf[(col, row)].fg, c);
                assert_eq!(buf[(col, row)].bg, c);
            }
        }
    }

    #[test]
    fn translucent_color_leaves_non_rgb_background_unblended() {
        // A cell this crate never painted (e.g. the backend's initial
        // blank buffer) can carry a non-`Rgb` ratatui colour with no
        // literal RGB to blend against — the documented fallback is to
        // leave that cell's background alone rather than guess.
        let mut buf = filled_buffer(3, 3, RatatuiColor::Black);
        draw_solid_fill(&mut buf, RRect::new(0, 0, 3, 3), Color::rgba(0, 0, 0, 102));

        for row in 0..3u16 {
            for col in 0..3u16 {
                assert_eq!(buf[(col, row)].symbol(), "x");
                assert_eq!(buf[(col, row)].bg, RatatuiColor::Black);
            }
        }
    }
}
