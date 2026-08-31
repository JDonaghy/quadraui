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
}
