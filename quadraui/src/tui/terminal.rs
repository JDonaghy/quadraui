//! TUI rasteriser for [`crate::Terminal`] cell grids.
//!
//! Iterates `cells[row][col]`, writing styled characters into the
//! ratatui buffer using [`crate::terminal_style::resolve_cell_style`] —
//! the overlay ladder shared with the GTK and macOS rasterisers (#500):
//! cursor inverts fg/bg, selection uses `theme.selection_bg`, find-match
//! and find-active use the highlight colours on [`Theme`].
//!
//! # Cursor shape (quadraui#338, best-effort)
//!
//! [`crate::primitives::terminal::TerminalCursorShape::Block`] is painted entirely by
//! [`resolve_cell_style`]'s colour invert — no extra work here.
//! `Underline`/`Bar` don't invert (see that fn's doc), so this rasteriser
//! layers `Modifier::UNDERLINED` on top when
//! [`crate::terminal_style::cursor_accent_visible`] says the accent
//! should show. A ratatui cell has no sub-cell geometry, so **`Bar`
//! renders identically to `Underline`** here — pixel-based rasterisers
//! (GTK/macOS/win, via [`crate::primitives::terminal`]'s shared `paint`)
//! are the ones that draw a true thin vertical bar; this is the
//! documented embedded-TUI limitation, not a bug.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color as RatatuiColor, Modifier};

use crate::primitives::scrollbar::Scrollbar;
use crate::primitives::terminal::Terminal;
use crate::terminal_style::{cursor_accent_visible, resolve_cell_style};
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

            let (bg, fg) = resolve_cell_style(cell, theme);
            let (draw_bg, draw_fg) = (ratatui_color(bg), ratatui_color(fg));

            let buf_cell = &mut buf[(x, y)];
            // `cell.text` is the cell's full grapheme cluster (base
            // character plus any combining marks vt100 attached to it,
            // e.g. an accent) rather than a single `char` — `set_symbol`
            // (not `set_char`) is what lets ratatui paint the whole
            // cluster as one glyph instead of silently dropping every
            // codepoint after the first (quadraui#337). ratatui ≥ 0.30
            // debug_asserts on ASCII control chars in cell symbols, so
            // any control byte in the cluster falls back to a plain
            // space, matching the old single-`char` behaviour.
            if cell.text.chars().any(|c| c.is_ascii_control()) {
                buf_cell.set_char(' ');
            } else {
                buf_cell.set_symbol(&cell.text);
            }
            buf_cell.set_fg(draw_fg).set_bg(draw_bg);

            let mut modifier = Modifier::empty();
            if cell.bold {
                modifier |= Modifier::BOLD;
            }
            if cell.italic {
                modifier |= Modifier::ITALIC;
            }
            let cursor_underline = cursor_accent_visible(cell);
            if cell.underline || cursor_underline {
                modifier |= Modifier::UNDERLINED;
            }
            buf_cell.modifier = modifier;
            buf_cell.underline_color = if cursor_underline {
                draw_fg
            } else {
                RatatuiColor::Reset
            };
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
    use crate::primitives::terminal::{TerminalCell, TerminalCursorShape, TerminalScrollbar};
    use crate::types::Color;

    fn blank_cell() -> TerminalCell {
        TerminalCell {
            text: " ".to_string(),
            fg: Color::rgb(200, 200, 200),
            bg: Color::rgb(0, 0, 0),
            bold: false,
            italic: false,
            underline: false,
            dim: false,
            selected: false,
            is_cursor: false,
            is_find_match: false,
            is_find_active: false,
            cursor_shape: TerminalCursorShape::Block,
            cursor_blinking: false,
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

    // ── Cursor shape (quadraui#338, best-effort) ────────────────────────

    /// A `Block` cursor still paints as a full colour invert — this
    /// rasteriser's pre-#338 default behaviour is unchanged.
    #[test]
    fn block_cursor_paints_inverted_with_no_underline() {
        let fg = ratatui_color(Color::rgb(200, 200, 200));
        let bg = ratatui_color(Color::rgb(0, 0, 0));
        let mut c = blank_cell();
        c.is_cursor = true;
        let term = Terminal {
            id: "t".into(),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let area = Rect::new(0, 0, 1, 1);
        let theme = Theme::default();
        let mut buf = Buffer::empty(area);
        draw_terminal(&mut buf, area, &term, &theme);

        let cell = &buf[(0, 0)];
        assert_eq!(cell.fg, bg, "inverted: cell fg becomes its own bg");
        assert_eq!(cell.bg, fg, "inverted: cell bg becomes its own fg");
        assert!(!cell.modifier.contains(Modifier::UNDERLINED));
    }

    /// `Underline`/`Bar` cursors don't invert (colours stay the cell's
    /// own) but do force `Modifier::UNDERLINED`, with `underline_color`
    /// set to the cell's own foreground — the best-effort approximation
    /// this rasteriser uses since a ratatui cell has no sub-cell
    /// geometry for a real thin bar (see this module's doc).
    #[test]
    fn underline_and_bar_cursor_paint_uninverted_with_underline_modifier() {
        let fg = Color::rgb(200, 200, 200);
        let bg = Color::rgb(0, 0, 0);
        let theme = Theme::default();

        for shape in [TerminalCursorShape::Underline, TerminalCursorShape::Bar] {
            let mut c = blank_cell();
            c.fg = fg;
            c.bg = bg;
            c.is_cursor = true;
            c.cursor_shape = shape;
            let term = Terminal {
                id: "t".into(),
                cells: vec![vec![c]],
                scrollbar: None,
            };
            let area = Rect::new(0, 0, 1, 1);
            let mut buf = Buffer::empty(area);
            draw_terminal(&mut buf, area, &term, &theme);

            let cell = &buf[(0, 0)];
            assert_eq!(
                cell.fg,
                ratatui_color(fg),
                "{shape:?}: colours should not invert"
            );
            assert_eq!(cell.bg, ratatui_color(bg));
            assert!(
                cell.modifier.contains(Modifier::UNDERLINED),
                "{shape:?}: best-effort accent is an underline modifier"
            );
            assert_eq!(cell.underline_color, ratatui_color(fg));
        }
    }

    /// A blinking cursor's accent is a wall-clock toggle
    /// (`cursor_blink_visible`) — this only pins that a *steady*
    /// (`cursor_blinking: false`) `Underline` cursor is unconditionally
    /// visible, since a real blink assertion would be flaky against the
    /// system clock.
    #[test]
    fn steady_underline_cursor_is_always_visible() {
        let mut c = blank_cell();
        c.is_cursor = true;
        c.cursor_shape = TerminalCursorShape::Underline;
        c.cursor_blinking = false;
        let term = Terminal {
            id: "t".into(),
            cells: vec![vec![c]],
            scrollbar: None,
        };
        let area = Rect::new(0, 0, 1, 1);
        let theme = Theme::default();
        let mut buf = Buffer::empty(area);
        draw_terminal(&mut buf, area, &term, &theme);
        assert!(buf[(0, 0)].modifier.contains(Modifier::UNDERLINED));
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
