//! TUI rasteriser for [`crate::Minimap`]: braille density view (#382).
//!
//! Each terminal cell packs a `2`-dot-wide x `4`-dot-tall braille glyph
//! (`4` buffer lines per row, `2` columns per cell — [`LINES_PER_ROW`] /
//! [`COLS_PER_CELL`]), lifting the bit-packing itself from
//! [`super::braille`] rather than a second copy (see that module's docs
//! for why one copy matters). The dot rule: a dot's *coverage fraction* —
//! how much of its source-column bucket is non-whitespace — is
//! thresholded through [`super::braille::dither_threshold_met`]'s ordered
//! dither, not a plain boolean OR (issue #1007). A boolean OR over a
//! bucket wider than one column saturates: as soon as *any* column in the
//! bucket is non-whitespace the dot sets, so practically every dot from
//! the end of a line's indent onward lights up and every row runs to the
//! strip's right edge, destroying the one signal — line length — that
//! makes a minimap read as a thumbnail of the code rather than a solid
//! bar. Dithering spreads that decision across many dots' worth of
//! threshold instead, so a sparsely-covered bucket only lights a few dot
//! positions and a densely-covered one lights (almost) all of them —
//! producing the ragged, length-tracking right edge VS Code's minimap has.
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

use super::braille::{dither_threshold_met, pack_braille_cell};
use super::{ratatui_color, set_cell};
use crate::primitives::minimap::{Minimap, MinimapLayout, MinimapSizing};
use crate::theme::Theme;

/// Buffer lines packed into one terminal row's braille dots.
pub const LINES_PER_ROW: usize = 4;
/// **Default** buffer-columns-per-cell scale — one braille cell (2 dot
/// columns) folds this many source columns into its colour + dot content,
/// so an `N`-cell strip represents `N * COLS_PER_CELL` source columns.
///
/// This is only the *default*: [`draw_minimap`] bakes it in for source
/// compatibility, but [`draw_minimap_with_scale`] accepts any
/// `cols_per_cell` explicitly (issue #1000). Before #1000 this scale was
/// baked directly into [`braille_char_for_cell`] and [`cell_color`] with
/// no way for a host to widen it — a VS-Code-proportioned strip (~11
/// cells) could only ever represent ~22 source columns, so code indented
/// past column ~22 painted no dots and no colour at all. A host that
/// wants to cover more source columns (VS Code's own minimap reaches
/// ~120) calls [`draw_minimap_with_scale`] with a larger value and
/// aggregates its [`crate::MinimapSpan`]s with a matching
/// [`crate::MinimapGrid::cols_per_cell`] — the two must agree, since
/// [`crate::aggregate_spans`] and this rasteriser are only ever combined
/// by convention, not by a shared type.
///
/// An **odd** `cols_per_cell` is rounded up to the nearest even effective
/// width internally (see [`cols_per_dot`]) so that the dot rasteriser and
/// the colour lookup always agree on exactly which source columns a cell
/// covers — passing an odd value still works, it just covers one extra
/// source column per cell rather than silently desyncing dots from
/// colour.
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

/// Draw a [`Minimap`] into `area` on `buf`, at the default
/// [`COLS_PER_CELL`] horizontal scale. Returns the layout for host click
/// dispatch (`layout.hit_test(x, y)` -> [`crate::MinimapHit`]).
///
/// A thin wrapper over [`draw_minimap_with_scale`] — kept as the
/// zero-argument-change entry point so existing callers (in-crate and
/// downstream) are unaffected by issue #1000's new scale parameter.
pub fn draw_minimap(
    buf: &mut Buffer,
    area: Rect,
    minimap: &Minimap,
    theme: &Theme,
) -> MinimapLayout {
    draw_minimap_with_scale(buf, area, minimap, theme, COLS_PER_CELL)
}

/// [`draw_minimap`], but with the horizontal scale — how many source
/// columns fold into one terminal cell — as an explicit parameter instead
/// of the hardcoded [`COLS_PER_CELL`] (issue #1000).
///
/// `cols_per_cell` is clamped to at least `1` (a `0` value would collapse
/// [`cell_color`]'s lookup range to empty, matching nothing). The same
/// value must be used to build the [`crate::MinimapGrid`] passed to
/// [`crate::aggregate_spans`] — this rasteriser and that aggregation step
/// are only related by the caller's own consistent choice of scale, not by
/// a shared type, exactly as [`LINES_PER_ROW`] already works for the
/// vertical axis.
pub fn draw_minimap_with_scale(
    buf: &mut Buffer,
    area: Rect,
    minimap: &Minimap,
    theme: &Theme,
    cols_per_cell: usize,
) -> MinimapLayout {
    let cols_per_cell = cols_per_cell.max(1);
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
            let ch = braille_char_for_cell(&row_lines, col, cols_per_cell);
            let fg = cell_color(
                minimap,
                vline.start_line_idx,
                col,
                default_fg,
                theme,
                cols_per_cell,
            );
            set_cell(buf, area.x + col as u16, row_y, ch, fg, row_bg);
        }
    }

    layout
}

/// Source columns folded into a single **dot** column, derived from a
/// requested `cols_per_cell` so that [`braille_char_for_cell`]'s dot
/// ranges and [`cell_color`]'s colour range can never disagree.
///
/// Both functions used to derive their column width from `cols_per_cell`
/// independently — `braille_char_for_cell` via `(cols_per_cell /
/// 2).max(1)` (truncating division) and `cell_color` via `cols_per_cell`
/// directly — which coincide only when `cols_per_cell` is even. For an
/// odd value the two ranges drift apart, growing linearly with `col`:
/// at `cols_per_cell = 11`, `col = 4`, dots used to cover source columns
/// `[40, 50)` while `cell_color` looked up `[44, 55)`. A span landing in
/// only one of the two mismatched ranges either tinted a cell with no
/// real dot behind it (reintroducing the #993 symptom this rasteriser
/// exists to prevent) or painted a dot with no colour.
///
/// Routing both call sites through this one function — rounding *up*
/// (`div_ceil`) rather than down, so the degenerate `cols_per_cell <= 1`
/// case still gets a non-empty width — makes the effective per-cell
/// width exactly `2 * cols_per_dot` everywhere, for any input.
fn cols_per_dot(cols_per_cell: usize) -> usize {
    cols_per_cell.max(1).div_ceil(2).max(1)
}

/// Pack one terminal cell's braille glyph from up to [`LINES_PER_ROW`]
/// pre-split lines. `col` is the terminal-cell column (0-based within
/// the minimap); each cell is 2 dots wide, so dot columns
/// `col*2..col*2+2` map to buffer columns at a **fixed** scale of
/// [`cols_per_dot`] buffer columns per dot — the same scale for every
/// line, matching [`cell_color`]'s fixed-grid lookup below (issue #993),
/// and the same scale for every dot column of every cell, so widening it
/// (issue #1000) widens the represented range without disturbing the
/// fixed-per-line property #993 established.
///
/// Column position is otherwise the only signal indentation has: if the
/// scale were normalised per-line (each line stretched to fill the full
/// strip width, as this used to do), a short line's 4-space indent would
/// land at a different dot column than the same 4-space indent on a long
/// line, destroying indentation entirely. Fixed scale means a line
/// shorter than the strip simply leaves the remaining dots clear, and a
/// line longer than `width_cells * 2 * cols_per_dot(cols_per_cell)` is
/// clipped rather than compressed — exactly what VS Code's minimap does.
///
/// The dot rule itself is a **coverage fraction**, not a boolean OR (issue
/// #1007): `covered` counts the non-whitespace characters inside the
/// dot's own `[c0, c1)` bucket — a plain per-dot scan, the same order of
/// work the old `.any()` scan already paid, no new allocation — and
/// [`dither_threshold_met`] thresholds that count against an ordered
/// dither matrix keyed on the dot's own absolute position. At the
/// (default) `cols_per_dot == 1` scale this is a no-op — a bucket of width
/// one has no fractional coverage to dither, so the result is identical to
/// the old `covered > 0` check — the behaviour change only appears once a
/// host widens `cols_per_cell` (issue #1000) far enough that a bucket can
/// be partially covered.
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

/// Resolve this cell's foreground: the aggregated [`crate::MinimapSpan`]
/// covering `(start_line_idx, col)` if one exists, else the theme
/// default. `minimap.syntax_spans` is expected to already be aggregated
/// at TUI's `4`-line x (`2 * `[`cols_per_dot`]`(cols_per_cell)`)-column
/// cell granularity (via [`crate::aggregate_spans`], with a matching
/// [`crate::MinimapGrid::cols_per_cell`]) — this does a plain containment
/// scan, no re-aggregation.
///
/// The column range is derived from [`cols_per_dot`] — the same helper
/// [`braille_char_for_cell`] uses — rather than from `cols_per_cell`
/// directly, so the two can never disagree (issue #1000 review fix; see
/// [`cols_per_dot`]'s doc for the drift this closes).
fn cell_color(
    minimap: &Minimap,
    start_line_idx: usize,
    col: usize,
    default_fg: ratatui::style::Color,
    theme: &Theme,
    cols_per_cell: usize,
) -> ratatui::style::Color {
    let cell_width = 2 * cols_per_dot(cols_per_cell);
    let col_lo = col * cell_width;
    let col_hi = col_lo + cell_width;
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
    /// cell carrying `chars`' leading non-whitespace character, at the
    /// default [`COLS_PER_CELL`] scale.
    fn first_set_cell(chars: &[char], width_cells: usize) -> Option<usize> {
        first_set_cell_at_scale(chars, width_cells, COLS_PER_CELL)
    }

    /// [`first_set_cell`], but at an explicit `cols_per_cell` scale
    /// (issue #1000).
    fn first_set_cell_at_scale(
        chars: &[char],
        width_cells: usize,
        cols_per_cell: usize,
    ) -> Option<usize> {
        let row_lines = [Some(chars.to_vec()), None, None, None];
        (0..width_cells)
            .find(|&col| braille_char_for_cell(&row_lines, col, cols_per_cell) != '\u{2800}')
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

    // ── issue #1000: cols_per_cell as a parameter, not a constant ──────

    #[test]
    fn draw_minimap_matches_draw_minimap_with_scale_at_the_default_cols_per_cell() {
        // `draw_minimap` must remain a pure delegation to
        // `draw_minimap_with_scale(.., COLS_PER_CELL)` — existing callers
        // (in-crate and downstream) see byte-identical output.
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

    /// Core #1000 deliverable: at the default [`COLS_PER_CELL`] scale, an
    /// 11-cell (VS-Code-proportioned) strip can only represent `11 * 2 =
    /// 22` source columns, so content at column 40 is unrepresentable —
    /// clipped, exactly like `a_line_past_the_visible_range_is_clipped_not_compressed`
    /// above. Widening `cols_per_cell` to `10` (5 source columns per dot)
    /// makes the same 11-cell strip cover `11 * 10 = 110` columns, so
    /// column 40 now lands inside cell `4` — this is the scale becoming a
    /// *parameter* a host can widen, not a constant baked into the
    /// rasteriser.
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

    /// Reproduces vimcode#1030 deliverable 2 ("colour must survive at
    /// indent 40 and 80") from the TUI side: with `cols_per_cell` widened
    /// to `10`, two syntax spans at columns 40 and 80 each resolve to
    /// their own colour in the painted cell rather than falling back to
    /// the theme default — the same colour-survival property #993 already
    /// guaranteed near column 0, now reachable at depths the default
    /// scale could never paint at all.
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

        // At the default scale, both spans are out of the 22-column
        // visible range: both cells fall back to the theme default —
        // this is the #1000 bug, reproduced as a byte-for-byte guard.
        let mut buf_default = Buffer::empty(area);
        draw_minimap(&mut buf_default, area, &mm, &Theme::default());
        assert_eq!(
            buf_default[(4u16, 0u16)].fg,
            ratatui_color(Theme::default().foreground),
            "at the default scale, column 40's span must be unreachable"
        );
        assert_eq!(
            buf_default[(8u16, 0u16)].fg,
            ratatui_color(Theme::default().foreground),
            "at the default scale, column 80's span must be unreachable"
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

    /// Regression test for the review fix on issue #1000: before this
    /// fix, `braille_char_for_cell` derived its dot range as
    /// `(cols_per_cell / 2).max(1)` columns per dot (truncating division)
    /// while `cell_color` derived its colour range as `cols_per_cell`
    /// columns directly — these only coincide when `cols_per_cell` is
    /// **even**. At the odd value used here (`11`), the two used to
    /// diverge: dots for terminal cell 4 covered source columns `[40,
    /// 50)` while `cell_color` looked up `[44, 55)`. A span landing in
    /// only one of the two mismatched ranges either painted colour with
    /// no real dot behind it, or a dot with no colour.
    ///
    /// Both functions now derive their width from the same
    /// [`cols_per_dot`] helper, rounding `11` up to `6` and giving both a
    /// shared effective cell width of `2 * 6 = 12`: cell 4 covers exactly
    /// `[48, 60)`. This asserts that shared boundary directly — a source
    /// range fully inside it (`[48, 60)` itself) must set both a dot and
    /// its span's colour in cell 4, and a source column just outside it
    /// (`47`) must set neither, proving the two lookups can't drift apart
    /// for an odd `cols_per_cell`.
    ///
    /// The "inside" case fills the *entire* `[48, 60)` bucket rather than
    /// a single character (issue #1007: a lone non-whitespace character in
    /// a multi-column bucket is no longer guaranteed to set its dot — that
    /// coverage is now dithered — but full coverage of a bucket always
    /// does, since [`super::braille::dither_threshold_met`]'s threshold
    /// tops out below full scale). This keeps the test's actual target —
    /// dot and colour ranges must agree — deterministic regardless of
    /// which of #1007's two dot positions the dither happens to land on.
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

    // ── issue #1007: density dithering fixes right-edge saturation ─────

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
        }
    }

    /// Core #1007 deliverable, acceptance criterion 1: on a real (>= 500
    /// line) source file, at a VS-Code-proportioned 12-cell x 33-row TUI
    /// strip, at most 20% of painted rows may run all the way to the
    /// strip's right edge. This is the exact reproduction the issue's
    /// measurement script produced "33/33 (100%)" for under the old
    /// boolean-OR dot rule (`draw_minimap`'s default `COLS_PER_CELL`
    /// alone can't fix this — the strip only covers 24 source columns
    /// then, which is `#1000`'s half); this test drives the same
    /// widened-scale + dithered-dots combination the issue's "variant C"
    /// measured directly: `cols_per_cell: 10` (5 source columns per dot,
    /// `ceil(COLUMN_CAPACITY / (12 cells * 2 dots))` — see
    /// [`crate::primitives::minimap::COLUMN_CAPACITY`]) is exactly the
    /// scale a host widens to via `draw_minimap_with_scale` (issue #1000)
    /// once it also wants #1007's dithered dot rule to stop saturating at
    /// that scale.
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
        let cols_per_cell = 10; // 5 source columns per dot (issue #1007 "variant C")

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

    /// Acceptance criterion 2: a file of uniform-length lines and a file
    /// of mixed-length lines must produce *visibly different* right-edge
    /// profiles -- the whole reason #1007 exists is that line length
    /// should be a visible signal in the minimap's silhouette, not
    /// something every row erases by saturating to the same column.
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
                std::iter::repeat(line).take(LINES_PER_ROW)
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
}
