//! TUI rasteriser for the focus-ring convention.
//!
//! Draws a border-only box (rounded corners, matching `dialog::draw_dialog`'s
//! glyph set) around whichever widget [`crate::focus::FocusTree`]
//! currently reports as focused.
//!
//! The ring never covers the focused widget's content. It is drawn only
//! in the padding: an edge cell whose glyph is blank takes the ring glyph
//! (keeping the background already under it), and an edge cell holding a
//! glyph keeps it, so a label that runs into the ring's path stays
//! readable — a list row reads `╭─Write the README───╮` rather than losing
//! its first row to the border.
//!
//! A one-row-tall `rect` (a button's `[ Label ]`, or a `Form`
//! `ButtonRow`/`ToggleGroup` item — see `primitives::form::contract`'s
//! "Item-level keyboard focus" doc section) has no padding row above or
//! below it, so [`draw_focus_ring`] underlines that row instead,
//! decorating its existing glyphs rather than replacing them.

use crate::cell_surface::{CellModifier, CellStyle, CellSurface};
use crate::theme::Theme;
use crate::Rect;

/// Paint the focus ring around `rect`. No-op if `rect` is degenerate
/// (zero width or height). A one-row-tall `rect` gets an underline
/// instead of a box — see this module's doc.
pub fn draw_focus_ring(surface: &mut dyn CellSurface, rect: Rect, theme: &Theme) {
    let fg = theme.accent_fg;
    let area = super::backend::q_rect_to_ratatui(rect);
    let (x, y, w, h) = (area.x, area.y, area.width, area.height);
    if w == 0 || h == 0 {
        return;
    }

    if h == 1 {
        for col in 0..w {
            surface.add_modifier(x + col, y, CellModifier::Underline { color: Some(fg) });
        }
        return;
    }

    let is_blank = |surface: &dyn CellSurface, cx: u16, cy: u16| {
        matches!(surface.get(cx, cy), Some((' ' | '\0', _)))
    };
    // A ring cell takes its glyph only where the cell is blank, keeping
    // the background already under it.
    let ring = |surface: &mut dyn CellSurface, cx: u16, cy: u16, ch: char| {
        if let Some((' ' | '\0', under)) = surface.get(cx, cy) {
            let style = CellStyle {
                fg: Some(fg),
                bg: under.bg,
                ..CellStyle::default()
            };
            surface.put_char(cx, cy, ch, style);
        }
    };
    // A horizontal edge steps around the whole run of text on its row, so
    // the blanks between words stay blank: `╭─Write the README───╮`.
    let edge = |surface: &mut dyn CellSurface, row: u16, ends: (char, char)| {
        let text: Vec<u16> = (0..w).filter(|&c| !is_blank(surface, x + c, row)).collect();
        let run = text.first().zip(text.last());
        for col in (0..w).filter(|c| !run.is_some_and(|(a, b)| (a..=b).contains(&c))) {
            let ch = match col {
                0 => ends.0,
                c if c == w - 1 => ends.1,
                _ => '─',
            };
            ring(surface, x + col, row, ch);
        }
    };
    edge(surface, y, ('╭', '╮'));
    edge(surface, y + h - 1, ('╰', '╯'));
    for row in (y + 1)..(y + h - 1) {
        ring(surface, x, row, '│');
        ring(surface, x + w - 1, row, '│');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::{ratatui_color, BufferCellSurface};
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::Rect as RRect;
    use ratatui::style::{Color as RatatuiColor, Modifier};

    fn ring(buf: &mut Buffer, rect: RRect, theme: &Theme) {
        let rect = Rect::new(
            rect.x as f32,
            rect.y as f32,
            rect.width as f32,
            rect.height as f32,
        );
        draw_focus_ring(&mut BufferCellSurface::new(buf), rect, theme);
    }

    fn filled_buffer(w: u16, h: u16, bg: RatatuiColor) -> Buffer {
        filled_with(w, h, bg, 'x')
    }

    fn filled_with(w: u16, h: u16, bg: RatatuiColor, ch: char) -> Buffer {
        let mut buf = Buffer::empty(RRect::new(0, 0, w, h));
        for y in 0..h {
            for x in 0..w {
                let mut cell = Cell::default();
                cell.set_char(ch).set_bg(bg);
                buf[(x, y)] = cell;
            }
        }
        buf
    }

    #[test]
    fn draws_corners_and_edges() {
        let theme = Theme::default();
        let mut buf = filled_with(10, 5, RatatuiColor::Black, ' ');
        ring(&mut buf, RRect::new(1, 1, 5, 3), &theme);

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
        ring(&mut buf, RRect::new(1, 1, 5, 1), &theme);

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
        let mut buf = filled_with(10, 5, bg, ' ');
        buf[(3, 2)].set_char('x');
        ring(&mut buf, RRect::new(1, 1, 5, 3), &theme);

        assert_eq!(buf[(1, 1)].symbol(), "╭");
        assert_eq!(buf[(1, 1)].bg, bg, "ring cells must keep the existing bg");
        // Interior (non-border) cells are untouched entirely.
        assert_eq!(buf[(3, 2)].symbol(), "x");
        assert_eq!(buf[(3, 2)].bg, bg);
    }

    /// The ring is drawn in the padding only: an edge cell already
    /// holding a glyph keeps it, glyph and colours alike, so a focused
    /// control's label is never replaced by a border glyph.
    #[test]
    fn never_covers_a_glyph_on_its_path() {
        let theme = Theme::default();
        let bg = RatatuiColor::Rgb(10, 20, 30);
        let mut buf = filled_with(10, 5, bg, ' ');
        // A label on the ring's left wall and one on its top edge.
        buf[(1, 2)].set_char('[').set_fg(RatatuiColor::Red);
        buf[(3, 1)].set_char('W');
        ring(&mut buf, RRect::new(1, 1, 5, 3), &theme);

        assert_eq!(buf[(1, 2)].symbol(), "[");
        assert_eq!(buf[(1, 2)].fg, RatatuiColor::Red);
        assert_eq!(buf[(3, 1)].symbol(), "W");
        // The blank cells either side still carry the ring.
        assert_eq!(buf[(2, 1)].symbol(), "─");
        assert_eq!(buf[(5, 2)].symbol(), "│");
    }

    /// A horizontal edge running into a row of text leaves the whole run
    /// alone, the blanks between its words included.
    #[test]
    fn an_edge_steps_around_a_run_of_words() {
        let theme = Theme::default();
        let mut buf = filled_with(12, 3, RatatuiColor::Black, ' ');
        for (i, ch) in "ab cd".chars().enumerate() {
            buf[(2 + i as u16, 0)].set_char(ch);
        }
        ring(&mut buf, RRect::new(0, 0, 12, 3), &theme);

        let row: String = (0..12u16)
            .map(|x| buf[(x, 0)].symbol().to_string())
            .collect();
        assert_eq!(row, "╭─ab cd────╮");
    }

    #[test]
    fn zero_size_rect_is_a_noop() {
        let theme = Theme::default();
        let mut buf = filled_buffer(10, 5, RatatuiColor::Black);
        ring(&mut buf, RRect::new(1, 1, 0, 3), &theme);
        // Every cell keeps its pre-existing content — nothing was painted.
        for y in 0..5u16 {
            for x in 0..10u16 {
                assert_eq!(buf[(x, y)].symbol(), "x");
            }
        }
    }

    /// The ring over a styled buffer — a box, a one-row underline and a
    /// box running off the buffer's edge — cell for cell against the
    /// golden.
    #[test]
    fn cell_paint_matches_the_golden() {
        use crate::tui::cell_surface::oracle;

        let theme = Theme::default();
        // Blank padding with a glyph every third column, so the golden
        // shows the ring both drawn in the padding and stepping around
        // content on its path.
        let prefill = |s: &mut dyn CellSurface| {
            let (w, h) = s.size();
            for y in 0..h {
                for x in 0..w {
                    let style = crate::cell_surface::solid_style(
                        crate::Color::rgb(200, 200, 200),
                        crate::Color::rgb(10, 20 + x as u8, 30 + y as u8),
                    );
                    let ch = if x % 3 == 0 { 'x' } else { ' ' };
                    s.put_char(x, y, ch, style);
                }
            }
        };
        let boxed = oracle::paint(10, 6, |s| {
            prefill(s);
            draw_focus_ring(s, Rect::new(1.0, 1.0, 5.0, 4.0), &theme);
        });
        let row = oracle::paint(10, 3, |s| {
            prefill(s);
            draw_focus_ring(s, Rect::new(2.0, 1.0, 6.0, 1.0), &theme);
        });
        let clipped = oracle::paint(6, 4, |s| {
            prefill(s);
            draw_focus_ring(s, Rect::new(3.0, 2.0, 6.0, 4.0), &theme);
        });
        oracle::assert_golden(
            "focus_ring",
            &[("boxed", &boxed), ("row", &row), ("clipped", &clipped)],
        );
    }
}
