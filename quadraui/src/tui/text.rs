//! Terminal cell-width and safe-truncation helpers.
//!
//! Every TUI consumer measuring or truncating text needs to reckon in
//! terminal display columns, not bytes or `char`s: CJK and wide-emoji
//! glyphs occupy two columns, Nerd Font Private-Use-Area glyphs render
//! double-width in terminals regardless of what `unicode-width` reports
//! for them, and a byte-indexed slice (`&s[..n]`) can land mid-codepoint
//! and panic. Before this module existed, every consumer re-rolled this
//! by hand (coord-tui alone had ~24 inline `s.chars().take(n).collect()`
//! sites, none of which accounted for display width). See issue #472.
//!
//! [`char_cell_width`] and [`display_width`] measure; [`truncate_to_width`]
//! and [`truncate_to_width_ellipsis`] clip a string to a column budget
//! without ever splitting a codepoint or a double-width glyph's two
//! columns.

use std::borrow::Cow;

/// Terminal cell width of a single character (0, 1, or 2).
///
/// Uses the `unicode-width` crate's UAX#11 tables, with a range-based
/// override for the Nerd Font Supplement PUA range (`U+F0000`–`U+F9999`),
/// which terminals render as double-width. This is checked *before*
/// falling through to `unicode-width` rather than only as a fallback for
/// `None`: some `unicode-width` releases report `Some(1)` rather than
/// `None` for this range, which would otherwise silently undersize
/// Nerd Font glyphs depending on the exact dependency version resolved.
pub fn char_cell_width(c: char) -> u16 {
    if ('\u{F0000}'..='\u{F9999}').contains(&c) {
        return 2;
    }
    unicode_width::UnicodeWidthChar::width(c).unwrap_or(1) as u16
}

/// Terminal display width of `s` in cells: the sum of each character's
/// [`char_cell_width`].
///
/// Not the same as `s.chars().count()` (CJK/emoji count double) or
/// `s.len()` (UTF-8 byte length).
pub fn display_width(s: &str) -> usize {
    s.chars().map(|c| char_cell_width(c) as usize).sum()
}

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
    fn char_cell_width_nerd_font_pua_override_is_two() {
        // Always 2, regardless of what unicode-width reports for these
        // codepoints (it varies by release — Some(1) or None).
        assert_eq!(char_cell_width('\u{F0000}'), 2);
        assert_eq!(char_cell_width('\u{F0001}'), 2);
        assert_eq!(char_cell_width('\u{F9999}'), 2);
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
