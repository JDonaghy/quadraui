//! Terminal cell-width and safe-truncation helpers.
//!
//! Every TUI consumer measuring or truncating text needs to reckon in
//! terminal display columns, not bytes or `char`s: CJK and wide-emoji
//! glyphs occupy two columns, and a byte-indexed slice (`&s[..n]`) can
//! land mid-codepoint and panic. Before this module existed, every
//! consumer re-rolled this by hand (coord-tui alone had ~24 inline
//! `s.chars().take(n).collect()` sites, none of which accounted for
//! display width). See issue #472.
//!
//! [`char_cell_width`] and [`display_width`] measure; [`truncate_to_width`]
//! and [`truncate_to_width_ellipsis`] clip a string to a column budget
//! without ever splitting a codepoint or a double-width glyph's two
//! columns.
//!
//! [`char_cell_width`] and [`display_width`] are re-exports of
//! [`crate::text_util::char_cell_width`] /
//! [`crate::text_util::display_width`], not separate implementations —
//! they moved to that core (feature-independent) module in #471 so
//! [`crate::types::StyledText::visible_width`] could use real cell-width
//! measurement too, without pulling in the `tui` feature. They stay
//! re-exported here for API stability (existing `quadraui::tui::*`
//! callers, and the `unicode-width` PUA doc details below).
//!
//! Uses the `unicode-width` crate's UAX#11 tables directly, with no
//! codepoint-range overrides. Private-Use-Area codepoints — including
//! both Nerd Font PUA blocks (BMP `U+E000`–`U+F8FF` and Supplementary-A
//! `U+F0000`–`U+FFFFD`) — measure as width 1, matching `unicode-width`
//! and `Nerd Font Mono` (the terminal-recommended, single-cell variant).
//! Non-Mono Nerd Font variants are genuinely double-width, but that is a
//! font/theme property, not something derivable from the codepoint
//! alone; if double-width PUA glyphs ever need supporting, it must come
//! in as an explicit input (theme/config/probe), not a range guess here.
//! See issue #545.

use std::borrow::Cow;

pub use crate::text_util::{char_cell_width, display_width};

/// Truncate `s` to at most `max_cols` display columns.
///
/// Always cuts on a `char` boundary (never panics, unlike `&s[..n]`) and
/// never splits a double-width glyph's two columns — if the next
/// character would overflow the budget it is dropped whole, even if one
/// column of budget remains. The returned slice's [`display_width`] is
/// always `<= max_cols`. Returns `s` unchanged (no allocation) if it
/// already fits.
pub fn truncate_to_width(s: &str, max_cols: usize) -> &str {
    let mut used = 0usize;
    for (idx, c) in s.char_indices() {
        let w = char_cell_width(c) as usize;
        if used + w > max_cols {
            return &s[..idx];
        }
        used += w;
    }
    s
}

/// Truncate `s` to at most `max_cols` display columns, appending a
/// single-column `…` when truncation actually happened. The result's
/// [`display_width`] is always `<= max_cols`.
///
/// Returns `s` unchanged (borrowed, no allocation) if it already fits;
/// otherwise returns an owned, ellipsized `String`. `max_cols == 0`
/// returns an empty string.
pub fn truncate_to_width_ellipsis(s: &str, max_cols: usize) -> Cow<'_, str> {
    if display_width(s) <= max_cols {
        return Cow::Borrowed(s);
    }
    if max_cols == 0 {
        return Cow::Borrowed("");
    }
    let body = truncate_to_width(s, max_cols - 1);
    let mut out = String::with_capacity(body.len() + '…'.len_utf8());
    out.push_str(body);
    out.push('…');
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_cell_width_ascii_is_one() {
        assert_eq!(char_cell_width('a'), 1);
        assert_eq!(char_cell_width(' '), 1);
    }

    #[test]
    fn char_cell_width_cjk_is_two() {
        assert_eq!(char_cell_width('日'), 2);
        assert_eq!(char_cell_width('中'), 2);
    }

    #[test]
    fn char_cell_width_pua_glyphs_are_width_one_and_consistent_across_blocks() {
        // No codepoint-range special cases: both Nerd Font PUA blocks
        // measure the same width (1), per unicode-width / Nerd Font Mono.
        // BMP PUA, e.g. nf-fa-github (U+F09B).
        let bmp_pua = char_cell_width('\u{F09B}');
        // Supplementary PUA-A, U+F0000 — inside the range this override
        // used to hardcode to width 2.
        let spua_a = char_cell_width('\u{F0000}');
        assert_eq!(bmp_pua, 1);
        assert_eq!(spua_a, 1);
        assert_eq!(bmp_pua, spua_a);
    }

    #[test]
    fn char_cell_width_combining_mark_is_zero() {
        // U+0301 COMBINING ACUTE ACCENT.
        assert_eq!(char_cell_width('\u{0301}'), 0);
    }

    #[test]
    fn display_width_sums_mixed_ascii_and_cjk() {
        assert_eq!(display_width("ab日本"), 2 + 2 + 2);
        assert_eq!(display_width(""), 0);
        assert_eq!(display_width("hello"), 5);
    }

    #[test]
    fn truncate_to_width_returns_whole_string_when_it_fits() {
        assert_eq!(truncate_to_width("hi", 10), "hi");
        assert_eq!(truncate_to_width("hi", 2), "hi");
    }

    #[test]
    fn truncate_to_width_cuts_on_char_boundary_never_panics() {
        // "日" is a 3-byte codepoint; byte-indexed slicing at n=1 or n=2
        // would panic. A budget of 1 column can't fit it at all.
        assert_eq!(truncate_to_width("日本語", 1), "");
        assert_eq!(truncate_to_width("日本語", 2), "日");
        assert_eq!(truncate_to_width("日本語", 3), "日");
        assert_eq!(truncate_to_width("日本語", 4), "日本");
    }

    #[test]
    fn truncate_to_width_never_splits_a_wide_glyph() {
        // Budget lands exactly between two columns of a wide glyph: the
        // glyph is dropped whole rather than emitting a half-glyph.
        let out = truncate_to_width("a日", 2);
        assert_eq!(out, "a");
        assert!(display_width(out) <= 2);
    }

    #[test]
    fn truncate_to_width_zero_budget_is_empty() {
        assert_eq!(truncate_to_width("hello", 0), "");
    }

    #[test]
    fn truncate_to_width_ellipsis_leaves_short_strings_untouched() {
        let out = truncate_to_width_ellipsis("hi", 10);
        assert_eq!(out, "hi");
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn truncate_to_width_ellipsis_clips_and_marks_long_strings() {
        let out = truncate_to_width_ellipsis("hello world", 6);
        assert_eq!(out, "hello…");
        assert_eq!(display_width(&out), 6);
        assert!(matches!(out, Cow::Owned(_)));
    }

    #[test]
    fn truncate_to_width_ellipsis_respects_wide_glyphs() {
        let out = truncate_to_width_ellipsis("日本語", 3);
        // 1 wide char (2 cols) + ellipsis (1 col) = 3 cols.
        assert_eq!(out, "日…");
        assert!(display_width(&out) <= 3);
    }

    #[test]
    fn truncate_to_width_ellipsis_zero_budget_is_empty() {
        assert_eq!(truncate_to_width_ellipsis("hello", 0), "");
    }
}
