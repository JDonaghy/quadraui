//! The cell painter for [`Minimap`]: a braille density view.
//!
//! Each cell packs a 2-dot-wide by 4-dot-tall braille glyph:
//! [`super::BRAILLE_LINES_PER_ROW`] buffer lines per row, and
//! [`MinimapLayout::cols_per_cell`] buffer columns per cell (see
//! [`super::resolve_cols_per_cell`]). A dot's *coverage fraction* — how
//! much of its source-column bucket is non-whitespace — is thresholded
//! through an ordered dither ([`crate::primitives::minimap::dither_threshold_met`])
//! rather than a boolean OR, so a sparsely covered bucket lights only a
//! few dots and each row's right edge tracks the line's length.
//!
//! Colour is one foreground per cell, read from
//! [`Minimap::syntax_spans`], already aggregated to this cell size by
//! [`crate::aggregate_spans`]. The viewport highlight is a background band
//! across the highlighted rows, pre-blended
//! (`blend(background, accent_bg, 0.25)`) since a cell has no alpha.

use super::{Minimap, MinimapLayout, BRAILLE_LINES_PER_ROW};
use crate::cell_surface::{solid_style, CellSurface};
use crate::theme::Theme;
use crate::tui::braille::{dither_threshold_met, pack_braille_cell};
use crate::Color;

/// Paint `minimap` at `layout` onto `cells`, at `layout.cols_per_cell`
/// buffer columns per cell. Each row repeats its content across every
/// whole cell row its pitch spans, so a [`super::MinimapSizing::RepeatToFill`]
/// layout stretches a short file with no blank rows.
pub(crate) fn paint(
    minimap: &Minimap,
    layout: &MinimapLayout,
    cells: &mut dyn CellSurface,
    theme: &Theme,
) {
    let cols_per_cell = layout.cols_per_cell.max(1);
    let b = layout.bounds;
    let (area_x, area_y) = (b.x.max(0.0) as u16, b.y.max(0.0) as u16);
    let (area_w, area_h) = (b.width.max(0.0) as u16, b.height.max(0.0) as u16);
    if area_w == 0 || area_h == 0 {
        return;
    }

    let highlight_bg = theme.background.blend(theme.accent_bg, 0.25);
    let hl = &layout.viewport_highlight;
    let width_cells = area_w as usize;

    for vline in &layout.visible_lines {
        let row_y0 = vline.bounds.y.round();
        let repeat_rows = vline.bounds.height.round().max(1.0) as u16;

        let row_mid = vline.bounds.y + vline.bounds.height * 0.5;
        let in_highlight = hl.height > 0.0 && row_mid >= hl.y && row_mid < hl.y + hl.height;
        let row_bg = if in_highlight {
            highlight_bg
        } else {
            theme.background
        };

        let row_lines: Vec<Option<Vec<char>>> = (0..BRAILLE_LINES_PER_ROW)
            .map(|dr| {
                minimap
                    .lines
                    .get(vline.start_line_idx + dr)
                    .map(|l| l.text.chars().collect())
            })
            .collect();

        for repeat in 0..repeat_rows {
            let row_y = row_y0 + repeat as f32;
            if row_y < 0.0 {
                continue;
            }
            let row_y = row_y as u16;
            if row_y < area_y || row_y >= area_y + area_h {
                continue;
            }

            for col in 0..width_cells {
                let ch = braille_char_for_cell(&row_lines, col, cols_per_cell);
                let fg = cell_color(
                    minimap,
                    vline.start_line_idx,
                    col,
                    theme.foreground,
                    cols_per_cell,
                );
                cells.put_char(area_x + col as u16, row_y, ch, solid_style(fg, row_bg));
            }
        }
    }
}

/// Source columns folded into one dot column. Both
/// [`braille_char_for_cell`]'s dot ranges and [`cell_color`]'s colour range
/// derive from this one value, so the effective cell width is exactly
/// `2 * cols_per_dot` for any `cols_per_cell` — an odd scale rounds up
/// rather than letting dots and colour cover different columns.
fn cols_per_dot(cols_per_cell: usize) -> usize {
    cols_per_cell.max(1).div_ceil(2).max(1)
}

/// Pack one cell's braille glyph from up to [`BRAILLE_LINES_PER_ROW`]
/// pre-split lines. `col` is the cell column within the strip; its dot
/// columns `col*2..col*2+2` map to buffer columns at a fixed
/// [`cols_per_dot`] per dot, the same for every line, so indentation keeps
/// its position and a line wider than the strip is clipped rather than
/// compressed.
///
/// A dot sets when [`dither_threshold_met`] accepts the count of
/// non-whitespace characters in its bucket; at one column per dot that is
/// a plain "is this column non-blank" test.
fn braille_char_for_cell(
    row_lines: &[Option<Vec<char>>],
    col: usize,
    cols_per_cell: usize,
) -> char {
    let cols_per_dot = cols_per_dot(cols_per_cell);
    pack_braille_cell(|dr, dc| {
        let chars = match row_lines.get(dr).and_then(|o| o.as_ref()) {
            Some(c) if !c.is_empty() => c,
            _ => return false,
        };
        let dot_col = col * 2 + dc;
        let c0 = dot_col * cols_per_dot;
        if c0 >= chars.len() {
            return false;
        }
        let c1 = (c0 + cols_per_dot).min(chars.len());
        let covered = chars[c0..c1].iter().filter(|c| !c.is_whitespace()).count();
        dither_threshold_met(covered, cols_per_dot, dr, dot_col)
    })
}

/// The cell's foreground: the first aggregated span on `start_line_idx`
/// overlapping the cell's column range, else `default_fg`.
fn cell_color(
    minimap: &Minimap,
    start_line_idx: usize,
    col: usize,
    default_fg: Color,
    cols_per_cell: usize,
) -> Color {
    let cell_width = 2 * cols_per_dot(cols_per_cell);
    let col_lo = col * cell_width;
    let col_hi = col_lo + cell_width;
    minimap
        .syntax_spans
        .iter()
        .find(|s| s.line_idx == start_line_idx && s.start_col < col_hi && s.end_col > col_lo)
        .map(|s| s.color)
        .unwrap_or(default_fg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::minimap::{
        resolve_cols_per_cell, MinimapHit, MinimapLine, MinimapSizing, MinimapSpan,
    };
    use crate::tui::{
        draw_minimap, draw_minimap_with_scale, draw_minimap_with_sizing, ratatui_color,
        tui_minimap_layout, COLS_PER_CELL, LINES_PER_ROW,
    };
    use crate::types::WidgetId;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

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
            scale: Default::default(),
            sizing: None,
        }
    }

    /// 8 lines x 4 columns, one "on" dot per line at a different column,
    /// laid out over a 2-cell-wide x 2-row area (dot grid 4 wide x 8
    /// tall matches the buffer exactly — no proportional scaling to
    /// reason about). This is the transposition guard: each of the 4 dot
    /// *rows* in the first cell-row carries exactly one set dot, in a
    /// different dot *column* each time, so swapping row/col in the bit
    /// table would produce a different (wrong) codepoint.
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
        // Hand-derived from the BRAILLE_OFFSETS table (see `crate::tui::braille`):
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
        let expected_tint = ratatui_color(theme.background.blend(theme.accent_bg, 0.25));
        assert_eq!(buf[(0u16, 0u16)].bg, expected_tint);
        assert_ne!(buf[(0u16, 1u16)].bg, expected_tint);
    }

    /// Regression test: the viewport highlight must be the same pre-blended
    /// tint converged on across backends (`blend(background, accent_bg, 0.25)`),
    /// not opaque `theme.accent_bg` at full strength.
    #[test]
    fn viewport_highlight_is_a_blended_tint_not_opaque_accent_bg() {
        let mut mm = eight_by_four();
        mm.visible_row_start = 0;
        mm.visible_row_count = 4; // first row group only
        let theme = Theme {
            background: Color::rgb(20, 20, 20),
            accent_bg: Color::rgb(0, 96, 191),
            ..Theme::default()
        };
        let area = Rect::new(0, 0, 2, 2);
        let mut buf = Buffer::empty(area);
        let _layout = draw_minimap(&mut buf, area, &mm, &theme);

        let expected = theme.background.blend(theme.accent_bg, 0.25);
        assert_eq!(buf[(0u16, 0u16)].bg, ratatui_color(expected));
        assert_ne!(
            buf[(0u16, 0u16)].bg,
            ratatui_color(theme.accent_bg),
            "the viewport slider must not paint opaque accent_bg at full strength"
        );
    }

    #[test]
    fn fully_visible_file_paints_no_viewport_band_at_all() {
        // An empty or short buffer whose viewport already spans every row
        // has nothing to scroll to, so no row may carry the highlight
        // tint -- otherwise the whole strip reads as one solid band
        // instead of a position indicator.
        let theme = Theme {
            background: Color::rgb(20, 20, 20),
            accent_bg: Color::rgb(0, 96, 191),
            ..Theme::default()
        };
        let tint = ratatui_color(theme.background.blend(theme.accent_bg, 0.25));

        for (lines, label) in [
            (vec![""], "single empty line"),
            (vec!["fn main() {}", "", "// short"], "three-line file"),
        ] {
            let total = lines.len();
            let mut mm = minimap_from(lines, total);
            mm.visible_row_start = 0;
            // A host reporting its whole window's row budget, well past
            // the end of the buffer.
            mm.visible_row_count = 40;
            let area = Rect::new(0, 0, 3, 6);
            let mut buf = Buffer::empty(area);
            let layout = draw_minimap(&mut buf, area, &mm, &theme);
            assert_eq!(
                layout.viewport_highlight.height, 0.0,
                "{label}: layout must not report a viewport band"
            );
            assert!(
                !layout.visible_lines.is_empty(),
                "{label}: nothing was painted, so the tint check below is vacuous"
            );
            for y in area.y..area.y + area.height {
                for x in area.x..area.x + area.width {
                    assert_ne!(buf[(x, y)].bg, tint, "{label}: cell ({x},{y}) is tinted");
                }
            }
        }
    }

    /// Shared body for `paint_and_click_round_trip_returns_seek_for_the_clicked_fraction`
    /// — see `docs/PRIMITIVE_RULES.md`'s "Coordinate frames for
    /// `*_layout` methods": `minimap_layout` is documented
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

    /// Non-zero-origin regression guard: `area = (0, 0)` is exactly the case
    /// where a LOCAL/ABSOLUTE mixup in `tui_minimap_layout` would be invisible.
    #[test]
    fn paint_and_click_round_trip_returns_seek_for_the_clicked_fraction_at_nonzero_origin() {
        paint_and_click_round_trip_at(7, 13);
    }

    /// Regression test: under the previous [`MinimapSizing::Fill`] sizing, a
    /// short file in a tall strip resolved to a row pitch of up to
    /// `MAX_ROW_PITCH` cell rows, but `draw_minimap` only ever painted the
    /// first cell row of each band — leaving `pitch - 1` blank cell rows
    /// between every painted row, growing as the file got shorter. TUI now
    /// uses `FixedPitch(1.0)`: exactly one cell row per minimap row, tiled
    /// top-down with no gaps, regardless of strip height.
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
    /// cell carrying `chars`' leading non-whitespace character, at the
    /// default [`COLS_PER_CELL`] scale.
    fn first_set_cell(chars: &[char], width_cells: usize) -> Option<usize> {
        first_set_cell_at_scale(chars, width_cells, COLS_PER_CELL)
    }

    /// [`first_set_cell`], but at an explicit `cols_per_cell` scale
    fn first_set_cell_at_scale(
        chars: &[char],
        width_cells: usize,
        cols_per_cell: usize,
    ) -> Option<usize> {
        let row_lines = [Some(chars.to_vec()), None, None, None];
        (0..width_cells)
            .find(|&col| braille_char_for_cell(&row_lines, col, cols_per_cell) != '\u{2800}')
    }

    /// The dot mapping uses one fixed scale for every line, so three lines
    /// with increasing indent ("x", "    x", "        x") paint their set
    /// dots at three distinct columns *proportional to the indent*, rather
    /// than each line being stretched across the strip by its own length.
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
    /// braille row group. This is the fixed-scale requirement applied
    /// defensively at the row-group level: `dr=0`'s short line and
    /// `dr=1`'s 200-column line are packed into the very same terminal
    /// cell row, so a regression toward row-level normalisation would move
    /// `dr=0`'s dot even though nothing about that line itself changed.
    #[test]
    fn a_long_neighbour_line_does_not_move_a_short_lines_dot() {
        let width_cells = 8;
        let indented: Vec<char> = "    x".chars().collect();
        let very_long: Vec<char> = format!("{}y", " ".repeat(200)).chars().collect();

        let alone = {
            let row_lines = [Some(indented.clone()), None, None, None];
            (0..width_cells)
                .find(|&col| braille_char_for_cell(&row_lines, col, COLS_PER_CELL) != '\u{2800}')
        };
        let with_long_neighbor = {
            let row_lines = [Some(indented.clone()), Some(very_long), None, None];
            (0..width_cells)
                .find(|&col| braille_char_for_cell(&row_lines, col, COLS_PER_CELL) != '\u{2800}')
        };

        assert_eq!(
            alone, with_long_neighbor,
            "a long neighbouring line must not shift where the short line's dot lands"
        );
    }

    /// A line longer than the strip's visible column range must be
    /// **clipped**, not compressed to fit — content past
    /// `width_cells * COLS_PER_CELL` never sets a dot, no matter how far
    /// past it extends. This is what distinguishes a fixed scale from a
    /// per-line stretch, which had no notion of "off the edge" since it
    /// always rescaled to fit exactly.
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

    // ── cols_per_cell as a parameter, not a constant ───────────

    #[test]
    fn draw_minimap_matches_draw_minimap_with_scale_at_the_default_cols_per_cell() {
        // `draw_minimap` delegates to `draw_minimap_with_scale`, resolving
        // its own scale via `resolve_cols_per_cell` rather than always passing
        // a hardcoded constant. `eight_by_four`'s widest line is 4 columns on a
        // 2-cell-wide strip (dot budget 4), which resolves to exactly
        // `COLS_PER_CELL` — this fixture is deliberately narrow so the two entry
        // points still agree here, proving `draw_minimap` is still a pure
        // delegation (just to a resolved, not hardcoded, scale) rather than
        // diverging paint logic of its own.
        let mm = eight_by_four();
        let area = Rect::new(0, 0, 2, 2);

        let mut via_plain = Buffer::empty(area);
        let plain_layout = draw_minimap(&mut via_plain, area, &mm, &Theme::default());

        let mut via_scale = Buffer::empty(area);
        let scale_layout =
            draw_minimap_with_scale(&mut via_scale, area, &mm, &Theme::default(), COLS_PER_CELL);

        assert_eq!(via_plain, via_scale);
        assert_eq!(plain_layout, scale_layout);
    }

    /// At the default [`COLS_PER_CELL`] scale, an 11-cell (VS-Code-proportioned)
    /// strip can only represent `11 * 2 = 22` source columns, so content at
    /// column 40 is unrepresentable — clipped. Widening `cols_per_cell` to
    /// `10` (5 source columns per dot) makes the same 11-cell strip cover
    /// `11 * 10 = 110` columns, so column 40 now lands inside cell `4` — this
    /// is the scale becoming a *parameter* a host can widen, not a constant
    /// baked into the rasteriser.
    #[test]
    fn draw_minimap_with_scale_widens_the_representable_source_range() {
        let width_cells = 11;
        let mut indent_40 = vec![' '; 41];
        indent_40[40] = 'x';

        assert_eq!(
            first_set_cell_at_scale(&indent_40, width_cells, COLS_PER_CELL),
            None,
            "column 40 must be unrepresentable at the default 2-cols-per-cell scale \
             on an 11-cell strip (visible range is only 22 columns)"
        );

        let widened_cols_per_cell = 10;
        assert_eq!(
            first_set_cell_at_scale(&indent_40, width_cells, widened_cols_per_cell),
            Some(4),
            "column 40 must land in cell 4 once cols_per_cell is widened to 10 \
             (5 source columns per dot, 11 cells -> 110 columns of coverage)"
        );
    }

    /// With `cols_per_cell` widened *explicitly* to `10`, two syntax spans at
    /// columns 40 and 80 each resolve to their own colour in the painted cell
    /// rather than falling back to the theme default — the same colour-survival
    /// property guaranteed near column 0, now reachable at depths the fixed
    /// [`COLS_PER_CELL`] scale could never paint at all.
    ///
    /// The "before widening" baseline here uses [`draw_minimap_with_scale`]
    /// pinned to the fixed [`COLS_PER_CELL`], not [`draw_minimap`]'s own
    /// default — since `draw_minimap` resolves its scale adaptively from the
    /// buffer's widest line, so for this fixture (a 90-column line) it would
    /// already reach both spans without any explicit widening. This test's own
    /// job is to prove the *explicit* `cols_per_cell` parameter works as a
    /// widening mechanism independent of the adaptive default.
    #[test]
    fn cell_color_survives_at_indent_40_and_80_with_a_widened_scale() {
        let mut line = vec![' '; 90];
        line[40] = 'a';
        line[80] = 'b';
        let text: String = line.into_iter().collect();

        let mut mm = minimap_from(vec![&text], 8);
        let red = Color::rgb(255, 0, 0);
        let blue = Color::rgb(0, 0, 255);
        mm.syntax_spans.push(MinimapSpan {
            line_idx: 0,
            start_col: 40,
            end_col: 50,
            color: red,
        });
        mm.syntax_spans.push(MinimapSpan {
            line_idx: 0,
            start_col: 80,
            end_col: 90,
            color: blue,
        });

        let area = Rect::new(0, 0, 11, 1);

        // Both spans are beyond the 22-column visible range at the fixed
        // COLS_PER_CELL scale, so both cells fall back to the theme default.
        let mut buf_narrow = Buffer::empty(area);
        draw_minimap_with_scale(&mut buf_narrow, area, &mm, &Theme::default(), COLS_PER_CELL);
        assert_eq!(
            buf_narrow[(4u16, 0u16)].fg,
            ratatui_color(Theme::default().foreground),
            "at the fixed COLS_PER_CELL scale, column 40's span must be unreachable"
        );
        assert_eq!(
            buf_narrow[(8u16, 0u16)].fg,
            ratatui_color(Theme::default().foreground),
            "at the fixed COLS_PER_CELL scale, column 80's span must be unreachable"
        );

        // Widened to 10 cols per cell (5 per dot), both spans land inside
        // the strip and paint their own distinct colour.
        let mut buf_wide = Buffer::empty(area);
        draw_minimap_with_scale(&mut buf_wide, area, &mm, &Theme::default(), 10);
        assert_eq!(
            buf_wide[(4u16, 0u16)].fg,
            ratatui_color(red),
            "colour must survive at indent 40"
        );
        assert_eq!(
            buf_wide[(8u16, 0u16)].fg,
            ratatui_color(blue),
            "colour must survive at indent 80"
        );
    }

    #[test]
    fn draw_minimap_with_scale_clamps_zero_to_one_instead_of_matching_everything() {
        // `cols_per_cell: 0` would otherwise collapse `cell_color`'s
        // `col_lo..col_hi` lookup range to empty (matching nothing) while
        // `braille_char_for_cell`'s `cols_per_dot` already guards its own
        // `.max(1)` — the clamp in `draw_minimap_with_scale` must cover
        // both call sites uniformly rather than relying on each helper's
        // own partial guard.
        let mm = eight_by_four();
        let area = Rect::new(0, 0, 2, 2);

        let mut via_zero = Buffer::empty(area);
        draw_minimap_with_scale(&mut via_zero, area, &mm, &Theme::default(), 0);

        let mut via_one = Buffer::empty(area);
        draw_minimap_with_scale(&mut via_one, area, &mm, &Theme::default(), 1);

        assert_eq!(
            via_zero, via_one,
            "cols_per_cell: 0 must behave like 1, not empty-match"
        );
    }

    /// `braille_char_for_cell`'s dot range and `cell_color`'s colour range
    /// must cover the same source columns even at an odd `cols_per_cell`.
    ///
    /// Both functions derive their width from the same [`cols_per_dot`]
    /// helper, rounding `11` up to `6` and giving both a shared effective cell
    /// width of `2 * 6 = 12`: cell 4 covers exactly `[48, 60)`. This asserts
    /// that shared boundary directly — a source range fully inside it covers
    /// both a dot and its span's colour in cell 4, and a source column just
    /// outside it must set neither, proving the two lookups can't drift apart.
    ///
    /// The "inside" case fills the *entire* `[48, 60)` bucket rather than
    /// a single character: a lone non-whitespace character in a multi-column
    /// bucket is not guaranteed to set its dot — that coverage is
    /// dithered — but full coverage of a bucket always does, since
    /// [`dither_threshold_met`]'s threshold tops out below
    /// full scale. This keeps the test's target — dot and colour ranges must
    /// agree — deterministic regardless of which positions the dither lands on.
    #[test]
    fn odd_cols_per_cell_keeps_dot_and_colour_ranges_aligned() {
        let width_cells = 8;
        let area = Rect::new(0, 0, width_cells as u16, 1);
        let red = Color::rgb(255, 0, 0);

        // Inside the shared [48, 60) range for cell 4: dot and colour
        // must both be present.
        {
            let mut chars = vec![' '; 60];
            chars[48..60].fill('x');
            let text: String = chars.into_iter().collect();
            let mut mm = minimap_from(vec![&text], 8);
            mm.syntax_spans.push(MinimapSpan {
                line_idx: 0,
                start_col: 55,
                end_col: 56,
                color: red,
            });

            let mut buf = Buffer::empty(area);
            draw_minimap_with_scale(&mut buf, area, &mm, &Theme::default(), 11);
            assert_ne!(
                cell_char(&buf, 4, 0),
                '\u{2800}',
                "a fully-covered [48, 60) bucket must set a dot in cell 4"
            );
            assert_eq!(
                buf[(4u16, 0u16)].fg,
                ratatui_color(red),
                "column 55's span is inside cell 4's [48, 60) colour range and must be found"
            );
        }

        // Just outside the shared range (column 47, one before 48): dot
        // and colour must both be absent -- if the two ranges had drifted
        // apart (as they did pre-fix), one of these would fire while the
        // other didn't.
        {
            let mut chars = vec![' '; 60];
            chars[47] = 'x';
            let text: String = chars.into_iter().collect();
            let mut mm = minimap_from(vec![&text], 8);
            mm.syntax_spans.push(MinimapSpan {
                line_idx: 0,
                start_col: 47,
                end_col: 48,
                color: red,
            });

            let mut buf = Buffer::empty(area);
            draw_minimap_with_scale(&mut buf, area, &mm, &Theme::default(), 11);
            assert_eq!(
                cell_char(&buf, 4, 0),
                '\u{2800}',
                "column 47 is outside cell 4's [48, 60) range and must not set a dot"
            );
            assert_eq!(
                buf[(4u16, 0u16)].fg,
                ratatui_color(Theme::default().foreground),
                "column 47's span is outside cell 4's [48, 60) colour range and must not be found"
            );
        }
    }

    // ── density dithering fixes right-edge saturation ────────────

    /// Draws `lines` into a `width_cells`-wide, `rows`-tall strip at
    /// `cols_per_cell` scale (down-sampling first if there are more than
    /// `rows * LINES_PER_ROW` lines, exactly as a real host would via
    /// [`crate::primitives::minimap::sample_blocks`]) and returns, for each
    /// painted row, the index of the last terminal-cell column whose
    /// braille glyph is not blank (`U+2800`) — `None` for a row with no
    /// content at all.
    fn last_set_cell_per_row(
        lines: &[&str],
        width_cells: usize,
        rows: usize,
        cols_per_cell: usize,
    ) -> Vec<Option<usize>> {
        use crate::primitives::minimap::sample_blocks;

        let sampled = sample_blocks(lines.len(), rows * LINES_PER_ROW, |i| lines[i].to_string());
        let total = lines.len();
        let mm = minimap_from_sampled(sampled, total);

        let area = Rect::new(0, 0, width_cells as u16, rows as u16);
        let mut buf = Buffer::empty(area);
        draw_minimap_with_scale(&mut buf, area, &mm, &Theme::default(), cols_per_cell);

        (0..rows as u16)
            .map(|y| {
                (0..width_cells as u16)
                    .rev()
                    .find(|&x| cell_char(&buf, x, y) != '\u{2800}')
                    .map(|x| x as usize)
            })
            .collect()
    }

    fn minimap_from_sampled(
        sampled: Vec<crate::primitives::minimap::MinimapLine>,
        total_buffer_lines: usize,
    ) -> Minimap {
        Minimap {
            id: WidgetId::new("mm"),
            lines: sampled,
            syntax_spans: Vec::new(),
            visible_row_start: 0,
            visible_row_count: 0,
            total_buffer_lines,
            scale: Default::default(),
            sizing: None,
        }
    }

    /// Core deliverable, acceptance criterion 1: on a real (>= 500 line)
    /// source file, at a VS-Code-proportioned 12-cell x 33-row TUI strip, at
    /// most 20% of painted rows may run all the way to the strip's right edge.
    /// A boolean-OR dot rule saturates every row here; this test drives the
    /// widened scale with dithered dots: `cols_per_cell: 10` (5 source
    /// columns per dot), the scale a host widens to via
    /// `draw_minimap_with_scale`.
    #[test]
    fn real_source_file_minimap_right_edge_tracks_line_length_not_saturated() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/compose/tab_group.rs"
        ));
        let lines: Vec<&str> = source.lines().collect();
        assert!(
            lines.len() >= 500,
            "fixture must be a real, checked-in source file of at least 500 lines, got {}",
            lines.len()
        );

        let width_cells = 12;
        let rows = 33;
        let cols_per_cell = 10; // 5 source columns per dot

        let last_cols = last_set_cell_per_row(&lines, width_cells, rows, cols_per_cell);

        let painted: Vec<usize> = last_cols.into_iter().flatten().collect();
        assert!(
            !painted.is_empty(),
            "expected at least some painted rows from a real source file"
        );
        let right_edge_rows = painted.iter().filter(|&&c| c == width_cells - 1).count();
        let fraction = right_edge_rows as f64 / painted.len() as f64;
        assert!(
            fraction <= 0.20,
            "{right_edge_rows}/{} painted rows ({:.1}%) run to the strip's right edge, \
             expected at most 20% -- the whole point of #1007 is that most rows should \
             NOT saturate to the full strip width",
            painted.len(),
            fraction * 100.0
        );
    }

    /// Acceptance criterion 2: a file of uniform-length lines and a file of
    /// mixed-length lines must produce *visibly different* right-edge profiles
    /// — the whole reason this fix exists is that line length should be a
    /// visible signal in the minimap's silhouette, not something every row
    /// erases by saturating to the same column.
    #[test]
    fn last_set_cell_varies_across_rows_for_mixed_lengths_but_not_for_uniform_ones() {
        let width_cells = 12;
        let rows = 16;
        let cols_per_cell = 10;

        // Every line identical in shape and length: the right edge must
        // land on the very same column for every painted row.
        let uniform_lines: Vec<String> = (0..rows * LINES_PER_ROW)
            .map(|_| "    let value = compute_something(a, b, c);".to_string())
            .collect();
        let uniform_refs: Vec<&str> = uniform_lines.iter().map(String::as_str).collect();
        let uniform_last_cols =
            last_set_cell_per_row(&uniform_refs, width_cells, rows, cols_per_cell);
        let uniform_painted: Vec<usize> = uniform_last_cols.into_iter().flatten().collect();
        assert!(
            uniform_painted.len() > 1,
            "expected multiple painted rows from the uniform fixture"
        );
        assert!(
            uniform_painted.iter().all(|&c| c == uniform_painted[0]),
            "uniform-length lines must produce a uniform right edge, got {uniform_painted:?}"
        );

        // Alternating short/long lines, one strip *row's* worth (4 lines)
        // at a time -- since a row's braille cell ORs across all
        // LINES_PER_ROW lines it packs, alternating line-by-line would
        // give every row group the exact same short+long mix and thus the
        // exact same (non-varying) right edge; alternating a whole row
        // group at a time is what actually exercises "does length vary
        // *between rows*".
        let mixed_lines: Vec<String> = (0..rows)
            .flat_map(|r| {
                let line = if r % 2 == 0 {
                    "x;".to_string()
                } else {
                    "                    let very_long_line_of_code_here = 12345;".to_string()
                };
                std::iter::repeat_n(line, LINES_PER_ROW)
            })
            .collect();
        let mixed_refs: Vec<&str> = mixed_lines.iter().map(String::as_str).collect();
        let mixed_last_cols = last_set_cell_per_row(&mixed_refs, width_cells, rows, cols_per_cell);
        let mixed_painted: Vec<usize> = mixed_last_cols.into_iter().flatten().collect();
        assert!(
            mixed_painted.len() > 1,
            "expected multiple painted rows from the mixed fixture"
        );
        assert!(
            mixed_painted.iter().any(|&c| c != mixed_painted[0]),
            "mixed-length lines must produce a varying right edge, got {mixed_painted:?}"
        );
    }

    // ── default scale adapts to the buffer's own width ──────────

    #[test]
    fn resolve_cols_per_cell_stays_at_the_narrow_default_when_the_file_already_fits() {
        // A 20-column line on a 12-cell strip (dot budget 24) already
        // fits inside the crispest possible scale: widening would only
        // throw away resolution for no reason.
        let text = "x".repeat(20);
        let mm = minimap_from(vec![text.as_str()], 1);
        assert_eq!(resolve_cols_per_cell(&mm, 12), COLS_PER_CELL);
    }

    #[test]
    fn resolve_cols_per_cell_widens_to_cover_a_wider_files_longest_line() {
        // Same 90-column fixture as
        // `cell_color_survives_at_indent_40_and_80_with_a_widened_scale`
        // above, but asserting the resolution function directly: an
        // 11-cell strip (dot budget 22) covering a 90-column file must
        // widen to 10 cols/cell (5 cols/dot, 110 columns of coverage) —
        // the exact value that test widens to *explicitly*, now produced
        // *automatically*.
        let text = format!("{}x", " ".repeat(89));
        let mm = minimap_from(vec![text.as_str()], 1);
        assert_eq!(resolve_cols_per_cell(&mm, 11), 10);
    }

    #[test]
    fn resolve_cols_per_cell_caps_at_column_capacity_for_an_extremely_long_line() {
        use crate::primitives::minimap::COLUMN_CAPACITY;
        let far_past_cap = "x".repeat(COLUMN_CAPACITY * 4);
        let exactly_at_cap = "x".repeat(COLUMN_CAPACITY);
        let mm_far = minimap_from(vec![far_past_cap.as_str()], 1);
        let mm_cap = minimap_from(vec![exactly_at_cap.as_str()], 1);
        assert_eq!(
            resolve_cols_per_cell(&mm_far, 12),
            resolve_cols_per_cell(&mm_cap, 12),
            "a line far past COLUMN_CAPACITY must resolve the same scale as a line \
             exactly at the cap -- VS Code parity, not unbounded widening"
        );
    }

    #[test]
    fn resolve_cols_per_cell_on_an_empty_buffer_does_not_divide_by_zero() {
        let mm = minimap_from(vec![], 0);
        assert_eq!(resolve_cols_per_cell(&mm, 12), COLS_PER_CELL);
    }

    /// Distinguishes the adaptive default scale from the fixed scale: a
    /// 24-column line and a 300-column line — both at or past the old fixed
    /// default's 24-column window — painted identically saturated runs under
    /// the fixed scale. The fixed scale only passed because the 1-char line
    /// sits *inside* the window; this fixture straddles the old window's right
    /// edge instead, so it only passes once the scale itself adapts. Both lines
    /// live in the same `Minimap` (different row groups so their dots never
    /// interact) — the resolved scale is a per-*file* property, so both rows
    /// share the one scale the file's widest line demands.
    #[test]
    fn default_scale_distinguishes_a_24_column_line_from_a_300_column_line_in_the_same_file() {
        let width_cells = 12;
        let short_line = "x".repeat(24); // exactly the old fixed-default window width
        let long_line = "x".repeat(300); // far past it, and past COLUMN_CAPACITY too

        let mm = minimap_from(
            vec![
                short_line.as_str(),
                "",
                "",
                "",
                long_line.as_str(),
                "",
                "",
                "",
            ],
            8,
        );

        let area = Rect::new(0, 0, width_cells as u16, 2);
        let mut buf = Buffer::empty(area);
        draw_minimap(&mut buf, area, &mm, &Theme::default());

        let last_set = |y: u16| -> Option<u16> {
            (0..width_cells as u16)
                .rev()
                .find(|&x| cell_char(&buf, x, y) != '\u{2800}')
        };
        let short_last = last_set(0);
        let long_last = last_set(1);

        assert_ne!(
            short_last, long_last,
            "a 24-column line and a 300-column line in the same file must not paint \
             identical-extent runs under the default (adaptive) scale -- got \
             short={short_last:?} long={long_last:?}"
        );
        assert_eq!(
            long_last,
            Some(width_cells as u16 - 1),
            "the 300-column line, clipped at COLUMN_CAPACITY like VS Code's own minimap, \
             should still saturate the strip's right edge"
        );
    }

    /// Direct regression for the exact scenario: `draw_minimap`'s *default*
    /// entry point (no explicit `cols_per_cell`) reaching indent 40 and 80 on
    /// a 90-column buffer, with no host-side widening required. Companion to
    /// `cell_color_survives_at_indent_40_and_80_with_a_widened_scale`, which
    /// pins the same buffer's colour behaviour under the *explicit* parameter
    /// mechanism instead.
    #[test]
    fn draw_minimap_default_scale_reaches_deep_indents_on_a_wide_buffer() {
        let mut line = vec![' '; 90];
        line[40] = 'a';
        line[80] = 'b';
        let text: String = line.into_iter().collect();

        let mut mm = minimap_from(vec![&text], 8);
        let red = Color::rgb(255, 0, 0);
        let blue = Color::rgb(0, 0, 255);
        mm.syntax_spans.push(MinimapSpan {
            line_idx: 0,
            start_col: 40,
            end_col: 50,
            color: red,
        });
        mm.syntax_spans.push(MinimapSpan {
            line_idx: 0,
            start_col: 80,
            end_col: 90,
            color: blue,
        });

        let area = Rect::new(0, 0, 11, 1);
        let mut buf = Buffer::empty(area);
        let layout = draw_minimap(&mut buf, area, &mm, &Theme::default());

        assert_eq!(
            layout.cols_per_cell, 10,
            "layout must report back the scale draw_minimap actually resolved and used"
        );
        assert_eq!(
            buf[(4u16, 0u16)].fg,
            ratatui_color(red),
            "the default scale must reach indent 40 without any explicit widening"
        );
        assert_eq!(
            buf[(8u16, 0u16)].fg,
            ratatui_color(blue),
            "the default scale must reach indent 80 without any explicit widening"
        );
    }

    /// [`tui_minimap_layout`] (the no-paint layout query `Backend::minimap_layout`
    /// uses) must resolve the exact same scale `draw_minimap` will paint
    /// with — a host reading `cols_per_cell` back before a paint has run
    /// (e.g. to hit-test a click, or to build a colour-aggregation grid
    /// ahead of painting, as `examples/common/minimap_app.rs` does) must
    /// not see a different answer than after painting.
    #[test]
    fn tui_minimap_layout_resolves_the_same_scale_draw_minimap_paints_with() {
        let mut line = vec![' '; 90];
        line[40] = 'a';
        let text: String = line.into_iter().collect();
        let mm = minimap_from(vec![&text], 8);
        let area = Rect::new(0, 0, 11, 1);

        let no_paint_layout = tui_minimap_layout(&mm, area);

        let mut buf = Buffer::empty(area);
        let painted_layout = draw_minimap(&mut buf, area, &mm, &Theme::default());

        assert_eq!(no_paint_layout.cols_per_cell, painted_layout.cols_per_cell);
        assert_eq!(painted_layout.cols_per_cell, 10);
    }

    /// [`draw_minimap_with_sizing`] under [`MinimapSizing::RepeatToFill`]
    /// must repeat-paint a short file's one sampled row across every cell
    /// row [`tui_minimap_layout_with_sizing`] assigns it — a 4-line file in
    /// a 20-row-tall strip resolves `k = 20` (one braille dot-group repeated
    /// 20 times), so every one of the 20 cell rows must carry the identical
    /// braille glyph, with no blank gap anywhere in the strip.
    #[test]
    fn draw_minimap_with_sizing_repeat_to_fill_paints_identical_content_across_every_repeated_row()
    {
        let mm = minimap_from(vec!["X   ", "  X ", " X  ", "   X"], 4);
        let area = Rect::new(0, 0, 2, 20);
        let mut buf = Buffer::empty(area);

        let layout = draw_minimap_with_sizing(
            &mut buf,
            area,
            &mm,
            &Theme::default(),
            2,
            MinimapSizing::RepeatToFill(1.0),
        );

        assert_eq!(
            layout.visible_lines.len(),
            1,
            "a 4-line file samples to exactly one braille row group"
        );
        assert_eq!(
            layout.visible_lines[0].bounds.height, 20.0,
            "the repeat factor must stretch the single row across the whole 20-row strip"
        );

        let expected_left = cell_char(&buf, 0, 0);
        let expected_right = cell_char(&buf, 1, 0);
        assert_ne!(
            expected_left, '\u{2800}',
            "the sampled row must actually paint content, not a blank braille glyph"
        );
        for y in 0..20u16 {
            assert_eq!(
                cell_char(&buf, 0, y),
                expected_left,
                "row {y} must repeat the same left-cell glyph as every other repeated row"
            );
            assert_eq!(
                cell_char(&buf, 1, y),
                expected_right,
                "row {y} must repeat the same right-cell glyph as every other repeated row"
            );
        }
    }
}
