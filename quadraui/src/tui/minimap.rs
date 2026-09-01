//! TUI rasteriser for [`crate::Minimap`]: braille density view (#382).
//!
//! Each terminal cell packs a `2`-dot-wide x `4`-dot-tall braille glyph
//! (`4` buffer lines per row, `2` columns per cell — [`LINES_PER_ROW`] /
//! [`COLS_PER_CELL`]), lifting the bit-packing itself from
//! [`super::braille`] rather than a second copy (see that module's docs
//! for why one copy matters). The dot rule: a dot is set when its
//! column-range contains a non-whitespace character — this is what
//! produces the recognisable "shape of the code" VS Code's minimap is
//! going for.
//!
//! Colour is one foreground per *cell*, read from
//! [`crate::Minimap::syntax_spans`] — already aggregated to this exact
//! cell granularity by [`crate::aggregate_spans`], so this rasteriser
//! never re-reduces colour data itself. The viewport highlight is a
//! **background** band across the highlighted rows — the previous
//! design's `█`/`▄`/`▌` overlay would have destroyed the dot content it
//! sat on, since braille has already spent its one foreground slot on
//! syntax colour.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::braille::pack_braille_cell;
use super::{ratatui_color, set_cell};
use crate::primitives::minimap::{Minimap, MinimapLayout, MinimapSizing};
use crate::theme::Theme;

/// Buffer lines packed into one terminal row's braille dots.
pub const LINES_PER_ROW: usize = 4;
/// Buffer columns folded into one terminal cell's colour.
pub const COLS_PER_CELL: usize = 2;

/// Compute the TUI cell-unit layout for a [`Minimap`] without painting.
///
/// TUI keeps [`MinimapSizing::Fill`] (#667): braille rows are cell-native
/// with no font to scale, so there's no file-length-dependent glyph size
/// to fix here the way GTK's fixed pitch fixes GTK's.
pub fn tui_minimap_layout(minimap: &Minimap, area: Rect) -> MinimapLayout {
    minimap.layout(
        crate::event::Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        ),
        LINES_PER_ROW,
        MinimapSizing::Fill,
    )
}

/// Draw a [`Minimap`] into `area` on `buf`. Returns the layout for host
/// click dispatch (`layout.hit_test(x, y)` -> [`crate::MinimapHit`]).
pub fn draw_minimap(
    buf: &mut Buffer,
    area: Rect,
    minimap: &Minimap,
    theme: &Theme,
) -> MinimapLayout {
    let layout = tui_minimap_layout(minimap, area);

    if area.width == 0 || area.height == 0 {
        return layout;
    }

    let bg = ratatui_color(theme.background);
    let default_fg = ratatui_color(theme.foreground);
    let highlight_bg = ratatui_color(theme.accent_bg);
    let hl = &layout.viewport_highlight;
    let width_cells = area.width as usize;

    for vline in &layout.visible_lines {
        let row_y = vline.bounds.y.round();
        if row_y < 0.0 {
            continue;
        }
        let row_y = row_y as u16;
        if row_y < area.y || row_y >= area.y + area.height {
            continue;
        }

        let row_mid = vline.bounds.y + vline.bounds.height * 0.5;
        let in_highlight = hl.height > 0.0 && row_mid >= hl.y && row_mid < hl.y + hl.height;
        let row_bg = if in_highlight { highlight_bg } else { bg };

        // The (up to) 4 buffer lines this row's braille dots come from,
        // pre-split into chars once per row rather than once per dot.
        let row_lines: Vec<Option<Vec<char>>> = (0..LINES_PER_ROW)
            .map(|dr| {
                minimap
                    .lines
                    .get(vline.start_line_idx + dr)
                    .map(|l| l.text.chars().collect())
            })
            .collect();

        for col in 0..width_cells {
            let ch = braille_char_for_cell(&row_lines, col, width_cells);
            let fg = cell_color(minimap, vline.start_line_idx, col, default_fg, theme);
            set_cell(buf, area.x + col as u16, row_y, ch, fg, row_bg);
        }
    }

    layout
}

/// Pack one terminal cell's braille glyph from up to [`LINES_PER_ROW`]
/// pre-split lines. `col` is the terminal-cell column (0-based within
/// the minimap); each cell is 2 dots wide, so dot columns
/// `col*2..col*2+2` map proportionally back into each line's characters.
fn braille_char_for_cell(row_lines: &[Option<Vec<char>>], col: usize, width_cells: usize) -> char {
    let dot_w = (width_cells * 2).max(1);
    pack_braille_cell(|dr, dc| {
        let chars = match row_lines.get(dr).and_then(|o| o.as_ref()) {
            Some(c) if !c.is_empty() => c,
            _ => return false,
        };
        let dot_col = col * 2 + dc;
        let char_start = (dot_col * chars.len() / dot_w).min(chars.len().saturating_sub(1));
        let char_end = (((dot_col + 1) * chars.len()).div_ceil(dot_w))
            .max(char_start + 1)
            .min(chars.len());
        chars[char_start..char_end]
            .iter()
            .any(|c| !c.is_whitespace())
    })
}

/// Resolve this cell's foreground: the aggregated [`crate::MinimapSpan`]
/// covering `(start_line_idx, col)` if one exists, else the theme
/// default. `minimap.syntax_spans` is expected to already be aggregated
/// at TUI's `4`-line x `2`-column cell granularity (via
/// [`crate::aggregate_spans`]) — this does a plain containment scan, no
/// re-aggregation.
fn cell_color(
    minimap: &Minimap,
    start_line_idx: usize,
    col: usize,
    default_fg: ratatui::style::Color,
    theme: &Theme,
) -> ratatui::style::Color {
    let col_lo = col * COLS_PER_CELL;
    let col_hi = col_lo + COLS_PER_CELL;
    minimap
        .syntax_spans
        .iter()
        .find(|s| s.line_idx == start_line_idx && s.start_col < col_hi && s.end_col > col_lo)
        .map(|s| ratatui_color(s.color))
        .unwrap_or({
            let _ = theme; // default_fg already derives from theme
            default_fg
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::minimap::{MinimapHit, MinimapLine, MinimapSpan};
    use crate::types::{Color, WidgetId};

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    fn minimap_from(lines: Vec<&str>, total_buffer_lines: usize) -> Minimap {
        Minimap {
            id: WidgetId::new("mm"),
            lines: lines
                .into_iter()
                .enumerate()
                .map(|(i, t)| MinimapLine {
                    text: t.into(),
                    line_idx: i,
                })
                .collect(),
            syntax_spans: Vec::new(),
            visible_row_start: 0,
            visible_row_count: 0,
            total_buffer_lines,
        }
    }

    /// 8 lines x 4 columns, one "on" dot per line at a different column,
    /// laid out over a 2-cell-wide x 2-row area (dot grid 4 wide x 8
    /// tall matches the buffer exactly — no proportional scaling to
    /// reason about). This is the transposition guard from #382: each
    /// of the 4 dot *rows* in the first cell-row carries exactly one set
    /// dot, in a different dot *column* each time, so swapping row/col
    /// in the bit table would produce a different (wrong) codepoint.
    fn eight_by_four() -> Minimap {
        minimap_from(
            vec![
                "X   ", "  X ", " X  ", "   X", "    ", "    ", "    ", "    ",
            ],
            8,
        )
    }

    #[test]
    fn braille_packing_exercises_all_dot_rows_and_both_dot_columns() {
        let mm = eight_by_four();
        let area = Rect::new(0, 0, 2, 2);
        let mut buf = Buffer::empty(area);
        let _layout = draw_minimap(&mut buf, area, &mm, &Theme::default());
        // Hand-derived from the BRAILLE_OFFSETS table (see `super::braille`):
        // left cell: bit0 (line0 col0) + bit5 (line2 col1) -> 0x21.
        // right cell: bit1 (line1 col0) + bit7 (line3 col1) -> 0x82.
        assert_eq!(cell_char(&buf, 0, 0), '\u{2821}', "left cell mispacked");
        assert_eq!(cell_char(&buf, 1, 0), '\u{2882}', "right cell mispacked");
    }

    #[test]
    fn all_whitespace_group_packs_to_blank_braille_not_a_space() {
        let mm = eight_by_four();
        let area = Rect::new(0, 0, 2, 2);
        let mut buf = Buffer::empty(area);
        let _layout = draw_minimap(&mut buf, area, &mm, &Theme::default());
        // Second row group (lines 4-7) is all-whitespace: must paint the
        // actual U+2800 blank-braille glyph, not a plain space, so a
        // minimap row visually reads as "no code here" rather than an
        // untouched cell.
        assert_eq!(cell_char(&buf, 0, 1), '\u{2800}');
        assert_eq!(cell_char(&buf, 1, 1), '\u{2800}');
    }

    #[test]
    fn cell_color_uses_the_aggregated_span_for_that_cell() {
        let mut mm = eight_by_four();
        let red = Color::rgb(255, 0, 0);
        mm.syntax_spans.push(MinimapSpan {
            line_idx: 0,
            start_col: 0,
            end_col: 2,
            color: red,
        });
        let area = Rect::new(0, 0, 2, 2);
        let mut buf = Buffer::empty(area);
        let _layout = draw_minimap(&mut buf, area, &mm, &Theme::default());
        assert_eq!(buf[(0u16, 0u16)].fg, ratatui_color(red));
        // The other cell has no matching span: falls back to theme fg.
        assert_eq!(
            buf[(1u16, 0u16)].fg,
            ratatui_color(Theme::default().foreground)
        );
    }

    #[test]
    fn viewport_highlight_paints_a_background_band_not_a_foreground_overlay() {
        let mut mm = eight_by_four();
        mm.visible_row_start = 0;
        mm.visible_row_count = 4; // first row group only
        let theme = Theme {
            accent_bg: Color::rgb(9, 9, 9),
            ..Theme::default()
        };
        let area = Rect::new(0, 0, 2, 2);
        let mut buf = Buffer::empty(area);
        let _layout = draw_minimap(&mut buf, area, &mm, &theme);
        assert_eq!(buf[(0u16, 0u16)].bg, ratatui_color(theme.accent_bg));
        assert_ne!(buf[(0u16, 1u16)].bg, ratatui_color(theme.accent_bg));
    }

    #[test]
    fn paint_and_click_round_trip_returns_seek_for_the_clicked_fraction() {
        let mm = minimap_from(vec!["x"; 8], 8);
        let area = Rect::new(0, 0, 4, 8); // 2 rows of 4 lines each -> track height 8
        let mut buf = Buffer::empty(area);
        let layout = draw_minimap(&mut buf, area, &mm, &Theme::default());
        assert_eq!(
            layout.hit_test(2.0, 4.0),
            MinimapHit::Seek { fraction: 0.5 }
        );
        assert_eq!(
            layout.hit_test(2.0, 0.0),
            MinimapHit::Seek { fraction: 0.0 }
        );
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let mm = eight_by_four();
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 4));
        let _layout = draw_minimap(&mut buf, Rect::new(0, 0, 0, 4), &mm, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }
}
