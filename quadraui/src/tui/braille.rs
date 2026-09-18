//! Shared `U+2800`-block braille dot packing, used by every TUI
//! rasteriser that needs sub-cell resolution: [`super::chart`]'s line
//! charts and [`super::minimap`]'s density view (#382).
//!
//! Lifted out of `tui/chart.rs` rather than duplicated: getting the bit
//! order wrong transposes the whole image in a way that still looks
//! plausible, so there must be exactly one copy of it (#382 review note).

/// Braille dot offsets: `(row_in_cell, col_in_cell) -> bit index`.
/// Standard Unicode braille ordering — a terminal cell is 2 dots wide
/// (`col_in_cell` in `0..2`) by 4 dots tall (`row_in_cell` in `0..4`).
pub(crate) const BRAILLE_OFFSETS: [(usize, usize); 8] = [
    (0, 0), // bit 0
    (1, 0), // bit 1
    (2, 0), // bit 2
    (0, 1), // bit 3
    (1, 1), // bit 4
    (2, 1), // bit 5
    (3, 0), // bit 6
    (3, 1), // bit 7
];

/// Pack one 2x4 dot cell into its `U+2800`-block braille codepoint.
/// `dot_at(row, col)` is queried for every one of the 8 dots
/// (`row` in `0..4`, `col` in `0..2`) and should return whether that dot
/// is set. An all-`false` cell packs to `U+2800` itself (blank braille),
/// not a space — callers that want to skip painting an empty cell decide
/// that themselves by comparing the result to `'\u{2800}'`.
pub(crate) fn pack_braille_cell(mut dot_at: impl FnMut(usize, usize) -> bool) -> char {
    let mut code: u32 = 0x2800;
    for (bit, &(row, col)) in BRAILLE_OFFSETS.iter().enumerate() {
        if dot_at(row, col) {
            code |= 1 << bit;
        }
    }
    char::from_u32(code).unwrap_or(' ')
}

/// Threshold a dot's coverage (`covered` non-whitespace source columns out
/// of `bucket_width` total) against a 4x4 ordered-dither (Bayer) matrix,
/// indexed by the dot's own `(row, col)` position within the whole
/// rendered grid (not just within its cell) so that the dither pattern
/// tiles consistently across the entire minimap strip rather than
/// repeating identically inside every cell (issue #1007).
///
/// [`super::minimap`]'s density view used to decide "is this dot set?" with
/// a boolean OR over the dot's source-column bucket (`any(|c|
/// !c.is_whitespace())`), which saturates every dot from the end of a
/// line's indent onward as soon as the bucket widens past one column
/// (#1000 widened it from one column to several, which made the
/// saturation worse, not better — every row of real code ran to the
/// strip's right edge with no line-length signal at all). An ordered
/// dither spreads that "is there *any* code here" boolean across many
/// dots' worth of threshold instead: a sparsely-covered bucket only lights
/// up the small subset of dot positions whose threshold value happens to
/// be low, while a fully-covered bucket lights up every position — which
/// is what produces a raggedy, VS-Code-like right edge instead of a solid
/// wall.
///
/// Pure integer arithmetic — one multiply and one compare, no floating
/// point, no lookahead, no per-call allocation (issue #1007 acceptance
/// criterion 4). A fully-covered bucket (`covered == bucket_width`)
/// always returns `true` and an all-whitespace bucket (`covered == 0`)
/// always returns `false` — dithering only has any effect strictly
/// *between* those two extremes.
///
/// Re-exported from [`crate::primitives::minimap`] rather than defined
/// here (issue #1012 pt. 2): that module's `sample_blocks` uses the exact
/// same matrix and threshold formula to make an analogous decision one
/// granularity coarser — whether a *block*'s per-column coverage (several
/// real buffer lines folded into one output row) reads back non-blank.
/// Before #1012 those were two independently-tuned copies — this one
/// here, another grown separately in vimcode's own tree (vimcode#1085) —
/// that nobody had reasoned about composing; a single shared definition
/// makes both compositions provably the same dither policy applied
/// twice, not two that happen to agree today.
pub(crate) use crate::primitives::minimap::dither_threshold_met;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_dots_off_packs_to_blank_braille() {
        assert_eq!(pack_braille_cell(|_, _| false), '\u{2800}');
    }

    #[test]
    fn all_dots_on_packs_to_full_braille_block() {
        assert_eq!(pack_braille_cell(|_, _| true), '\u{28FF}');
    }

    #[test]
    fn single_dot_sets_the_documented_bit() {
        // bit 0 -> (row 0, col 0)
        assert_eq!(pack_braille_cell(|r, c| (r, c) == (0, 0)), '\u{2801}');
        // bit 3 -> (row 0, col 1)
        assert_eq!(pack_braille_cell(|r, c| (r, c) == (0, 1)), '\u{2808}');
        // bit 6 -> (row 3, col 0)
        assert_eq!(pack_braille_cell(|r, c| (r, c) == (3, 0)), '\u{2840}');
        // bit 7 -> (row 3, col 1)
        assert_eq!(pack_braille_cell(|r, c| (r, c) == (3, 1)), '\u{2880}');
    }

    // ── dither_threshold_met (issue #1007) ─────────────────────────────

    #[test]
    fn zero_coverage_never_meets_the_threshold() {
        for row in 0..4 {
            for col in 0..4 {
                assert!(!dither_threshold_met(0, 6, row, col));
            }
        }
    }

    #[test]
    fn zero_width_bucket_never_meets_the_threshold() {
        assert!(!dither_threshold_met(0, 0, 0, 0));
        assert!(!dither_threshold_met(5, 0, 0, 0));
    }

    #[test]
    fn full_coverage_always_meets_the_threshold() {
        // A fully-covered bucket must light up at every dot position,
        // including the matrix's own largest threshold (15) -- otherwise
        // a solid run of non-whitespace would still leave holes.
        for row in 0..4 {
            for col in 0..4 {
                assert!(
                    dither_threshold_met(6, 6, row, col),
                    "full coverage must always set the dot at ({row}, {col})"
                );
            }
        }
    }

    #[test]
    fn partial_coverage_sets_only_some_dot_positions() {
        // 1-of-6 covered: only the positions whose Bayer threshold is low
        // enough should light up, and it must not be all-or-nothing across
        // the matrix -- that's the whole point of dithering a partially
        // covered bucket instead of booleans-OR-ing it.
        let set_count = (0..4)
            .flat_map(|row| (0..4).map(move |col| (row, col)))
            .filter(|&(row, col)| dither_threshold_met(1, 6, row, col))
            .count();
        assert!(
            set_count > 0 && set_count < 16,
            "expected a strict subset of the 16 dot positions to light up, got {set_count}/16"
        );
    }

    #[test]
    fn threshold_is_indexed_by_absolute_position_not_just_local_position() {
        // Position (0, 0) and (4, 4) share the same `& 3` fold, so they
        // must resolve identically -- confirms the matrix tiles rather
        // than being looked up some other way.
        assert_eq!(
            dither_threshold_met(3, 6, 0, 0),
            dither_threshold_met(3, 6, 4, 4)
        );
    }
}
