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
/// TUI uses [`MinimapSizing::FixedPitch`] at exactly `1.0` cell row per
/// minimap row (#992). `draw_minimap` below paints exactly one cell row
/// per [`VisibleMinimapLine`], so the layout's row pitch and the
/// rasteriser's paint granularity must agree — under the previous
/// [`MinimapSizing::Fill`], a short file's pitch could stretch up to
/// [`crate::primitives::minimap::MAX_ROW_PITCH`] cell rows, but
/// `draw_minimap` still only painted the first cell row of each band,
/// leaving `pitch - 1` blank cell rows between every painted row — a gap
/// that grew as the file got shorter. `FixedPitch(1.0)` pins the pitch to
/// the rasteriser's actual per-row paint cost, so rows tile contiguously
/// regardless of strip height, and long files fall back to the same
/// sliding-window behaviour GTK already relies on (see the module docs
/// on [`crate::primitives::minimap::Minimap::layout_with_sizing`]).
pub fn tui_minimap_layout(minimap: &Minimap, area: Rect) -> MinimapLayout {
    minimap.layout_with_sizing(
        crate::event::Rect::new(
            area.x as f32,
            area.y as f32,
            area.width as f32,
            area.height as f32,
        ),
        LINES_PER_ROW,
        MinimapSizing::FixedPitch(1.0),
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
            let ch = braille_char_for_cell(&row_lines, col);
            let fg = cell_color(minimap, vline.start_line_idx, col, default_fg, theme);
            set_cell(buf, area.x + col as u16, row_y, ch, fg, row_bg);
        }
    }

    layout
}

/// Pack one terminal cell's braille glyph from up to [`LINES_PER_ROW`]
/// pre-split lines. `col` is the terminal-cell column (0-based within
/// the minimap); each cell is 2 dots wide, so dot columns
/// `col*2..col*2+2` map to buffer columns at a **fixed** scale of
/// [`COLS_PER_CELL`] buffer columns per cell — the same scale for every
/// line, matching [`cell_color`]'s fixed-grid lookup below (issue #993).
///
/// Column position is otherwise the only signal indentation has: if the
/// scale were normalised per-line (each line stretched to fill the full
/// strip width, as this used to do), a short line's 4-space indent would
/// land at a different dot column than the same 4-space indent on a long
/// line, destroying indentation entirely. Fixed scale means a line
/// shorter than the strip simply leaves the remaining dots clear, and a
/// line longer than `width_cells * COLS_PER_CELL` is clipped rather than
/// compressed — exactly what VS Code's minimap does.
fn braille_char_for_cell(row_lines: &[Option<Vec<char>>], col: usize) -> char {
    pack_braille_cell(|dr, dc| {
        let chars = match row_lines.get(dr).and_then(|o| o.as_ref()) {
            Some(c) if !c.is_empty() => c,
            _ => return false,
        };
        let dot_col = col * 2 + dc;
        let cols_per_dot = (COLS_PER_CELL / 2).max(1);
        let c0 = dot_col * cols_per_dot;
        let c1 = c0 + cols_per_dot;
        chars
            .get(c0..c1.min(chars.len()))
            .is_some_and(|s| s.iter().any(|c| !c.is_whitespace()))
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

    /// Shared body for `paint_and_click_round_trip_returns_seek_for_the_clicked_fraction`
    /// — see `docs/PRIMITIVE_RULES.md`'s "Coordinate frames for
    /// `*_layout` methods" (issue #505): `minimap_layout` is documented
    /// **ABSOLUTE**, so `hit_test` must be called with coordinates
    /// shifted by the same `origin_x`/`origin_y` the minimap was
    /// painted at, not with area-local coordinates.
    fn paint_and_click_round_trip_at(origin_x: u16, origin_y: u16) {
        let mm = minimap_from(vec!["x"; 8], 8);
        let area = Rect::new(origin_x, origin_y, 4, 8); // 2 rows of 4 lines each -> track height 8
        let mut buf = Buffer::empty(area);
        let layout = draw_minimap(&mut buf, area, &mm, &Theme::default());
        assert_eq!(
            layout.hit_test(origin_x as f32 + 2.0, origin_y as f32 + 4.0),
            MinimapHit::Seek { fraction: 0.5 }
        );
        assert_eq!(
            layout.hit_test(origin_x as f32 + 2.0, origin_y as f32),
            MinimapHit::Seek { fraction: 0.0 }
        );
    }

    #[test]
    fn paint_and_click_round_trip_returns_seek_for_the_clicked_fraction() {
        paint_and_click_round_trip_at(0, 0);
    }

    /// Non-zero-origin regression guard (issue #505 / LESSONS.md
    /// "Layout helpers must return coords in the same frame across
    /// backends"): `area = (0, 0)` is exactly the case where a
    /// LOCAL/ABSOLUTE mixup in `tui_minimap_layout` would be invisible.
    #[test]
    fn paint_and_click_round_trip_returns_seek_for_the_clicked_fraction_at_nonzero_origin() {
        paint_and_click_round_trip_at(7, 13);
    }

    /// Regression test for #992: under the previous [`MinimapSizing::Fill`]
    /// sizing, a short file in a tall strip resolved to a row pitch of up
    /// to `MAX_ROW_PITCH` cell rows, but `draw_minimap` only ever painted
    /// the first cell row of each band — leaving `pitch - 1` blank cell
    /// rows between every painted row, growing as the file got shorter.
    /// TUI now uses `FixedPitch(1.0)`: exactly one cell row per minimap
    /// row, tiled top-down with no gaps, regardless of strip height.
    #[test]
    fn short_file_in_a_tall_strip_paints_contiguous_rows_no_gaps() {
        let mm = eight_by_four(); // 8 lines, 2 row groups of 4 lines each
        let area = Rect::new(0, 0, 2, 20); // strip far taller than 2 rows
        let mut buf = Buffer::empty(area);
        let layout = draw_minimap(&mut buf, area, &mm, &Theme::default());

        assert_eq!(layout.visible_lines.len(), 2);
        assert_eq!(
            layout.visible_lines[0].bounds.height, 1.0,
            "row pitch must be exactly one cell row, not stretched toward MAX_ROW_PITCH"
        );
        assert_eq!(layout.visible_lines[1].bounds.height, 1.0);
        assert_eq!(layout.visible_lines[0].bounds.y, 0.0);
        assert_eq!(
            layout.visible_lines[1].bounds.y, 1.0,
            "row 1 must sit directly below row 0 with no gap"
        );

        // Both painted rows must actually carry braille glyphs (row 1 is
        // all-whitespace, so it paints the blank-braille U+2800 glyph, not
        // an untouched space) -- contiguous, with no blank cell row
        // painted between them.
        assert_ne!(cell_char(&buf, 0, 0), ' ');
        assert_ne!(cell_char(&buf, 0, 1), ' ');
        // The rest of the tall strip stays untouched: a short file
        // top-aligns instead of stretching to fill it (unchanged from
        // `Fill`'s own top-align behaviour).
        assert_eq!(cell_char(&buf, 0, 2), ' ');
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let mm = eight_by_four();
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 4));
        let _layout = draw_minimap(&mut buf, Rect::new(0, 0, 0, 4), &mm, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    /// Returns the first terminal-cell column (out of `0..width_cells`)
    /// whose braille glyph differs from the blank glyph U+2800, i.e. the
    /// cell carrying `chars`' leading non-whitespace character.
    fn first_set_cell(chars: &[char], width_cells: usize) -> Option<usize> {
        let row_lines = [Some(chars.to_vec()), None, None, None];
        (0..width_cells).find(|&col| braille_char_for_cell(&row_lines, col) != '\u{2800}')
    }

    /// Regression test for #993: `braille_char_for_cell` used to
    /// normalise each line's dot mapping by **that line's own**
    /// character count (`chars.len() / dot_w`), so a short line's
    /// content was stretched across the *entire* strip width regardless
    /// of how little of that width the line actually occupies. Column
    /// position is the only signal indentation has, so three lines with
    /// increasing indent ("x", "    x", "        x") must paint their
    /// set dots at three distinct columns *proportional to the indent*
    /// — not bunched together near the strip's far edge, which is what
    /// the old per-line normalisation produced (a 1-char line placed its
    /// dot near column 0 "by accident", but 5- and 9-char lines both got
    /// stretched to place their trailing 'x' near the right edge of the
    /// full strip, regardless of the actual indent).
    #[test]
    fn indentation_lands_at_distinct_columns_proportional_to_indent() {
        let width_cells = 8;
        let flush: Vec<char> = "x".chars().collect();
        let indent_4: Vec<char> = "    x".chars().collect();
        let indent_8: Vec<char> = "        x".chars().collect();

        let c_flush = first_set_cell(&flush, width_cells).expect("flush line must set a dot");
        let c_indent_4 =
            first_set_cell(&indent_4, width_cells).expect("indent-4 line must set a dot");
        let c_indent_8 =
            first_set_cell(&indent_8, width_cells).expect("indent-8 line must set a dot");

        assert!(
            c_flush < c_indent_4 && c_indent_4 < c_indent_8,
            "expected strictly increasing columns, got {c_flush} < {c_indent_4} < {c_indent_8}"
        );
        // Fixed scale: COLS_PER_CELL buffer columns per terminal cell,
        // so doubling the indent must double the column offset exactly
        // — this is what "proportional" pins down, not just "increasing".
        assert_eq!(c_indent_4, c_flush + 4 / COLS_PER_CELL);
        assert_eq!(c_indent_8, c_flush + 8 / COLS_PER_CELL);
    }

    /// A short, indented line's dot column must depend only on its own
    /// indent — never on the length of some other line sharing the same
    /// braille row group (issue #993's fixed-scale requirement, applied
    /// defensively at the row-group level: `dr=0`'s short line and
    /// `dr=1`'s 200-column line are packed into the very same terminal
    /// cell row, so a future regression toward row-level normalisation
    /// — e.g. scaling by the longest line in the group instead of a
    /// fixed constant — would move `dr=0`'s dot even though nothing
    /// about that line itself changed).
    #[test]
    fn a_long_neighbour_line_does_not_move_a_short_lines_dot() {
        let width_cells = 8;
        let indented: Vec<char> = "    x".chars().collect();
        let very_long: Vec<char> = format!("{}y", " ".repeat(200)).chars().collect();

        let alone = {
            let row_lines = [Some(indented.clone()), None, None, None];
            (0..width_cells).find(|&col| braille_char_for_cell(&row_lines, col) != '\u{2800}')
        };
        let with_long_neighbor = {
            let row_lines = [Some(indented.clone()), Some(very_long), None, None];
            (0..width_cells).find(|&col| braille_char_for_cell(&row_lines, col) != '\u{2800}')
        };

        assert_eq!(
            alone, with_long_neighbor,
            "a long neighbouring line must not shift where the short line's dot lands"
        );
    }

    /// A line longer than the strip's visible column range must be
    /// **clipped**, not compressed to fit — content past
    /// `width_cells * COLS_PER_CELL` never sets a dot, no matter how far
    /// past it extends (issue #993: this is what distinguishes a fixed
    /// scale from the old per-line stretch, which had no notion of
    /// "off the edge" since it always rescaled to fit exactly).
    #[test]
    fn a_line_past_the_visible_range_is_clipped_not_compressed() {
        let width_cells = 4;
        let visible_cols = width_cells * COLS_PER_CELL;

        // Non-whitespace only at the last visible column: must be seen.
        let mut at_edge = vec![' '; visible_cols];
        at_edge[visible_cols - 1] = 'x';
        assert!(
            first_set_cell(&at_edge, width_cells).is_some(),
            "a dot within the visible range must be painted"
        );

        // Non-whitespace only one column past the visible range: clipped.
        let mut past_edge = vec![' '; visible_cols + 1];
        past_edge[visible_cols] = 'x';
        assert_eq!(
            first_set_cell(&past_edge, width_cells),
            None,
            "content past width_cells * COLS_PER_CELL must be clipped, not compressed into view"
        );
    }
}
