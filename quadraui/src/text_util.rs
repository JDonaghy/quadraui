//! Pure text-processing utilities: fuzzy subsequence matching and
//! word-aware line wrapping.
//!
//! Both functions are framework-shaped — no platform dependencies, no
//! `Backend` coupling — so they're unconditionally compiled and free of
//! feature gates. They exist here (rather than in a consuming app) so
//! [`fuzzy_score`] backs every fuzzy matcher in the crate
//! ([`crate::compose::folder_picker`]'s directory filter,
//! [`crate::compose::filter_help_actions`]) instead of each one carrying
//! its own ad-hoc scorer, and so [`word_wrap`] gives
//! [`crate::compose::chat_controller`] (and any other consumer that
//! needs to wrap plain text to a column budget) real word-aware
//! wrapping instead of a mid-word hard break.
//!
//! See issue #474 ("Text-util gaps") for the audit that found three
//! independent fuzzy matchers (two of them weaker ad-hoc versions) and a
//! hard-break wrap living app-side while quadraui had no shared version.
//!
//! The boundary-snap helpers below ([`snap_to_char_boundary`],
//! [`prev_char_boundary`], [`next_char_boundary`], [`safe_prefix`],
//! [`safe_slice`]) close a related gap tracked by issue #503: GUI
//! rasterisers (gtk, macos) receive byte-offset cursor/selection
//! positions from host apps and consumers of this crate (see
//! `primitives/palette.rs`'s `query_cursor` field) with no guarantee
//! those offsets land on a UTF-8 char boundary — slicing a `String`
//! directly at such an offset panics the paint pass the moment a
//! multibyte character (é, CJK, emoji) sits left of the cursor. Before
//! this module these existed as seven byte-identical private copies
//! (`tui/editor.rs`, `compose/chat_controller.rs`,
//! `compose/tree_controller.rs`); they're unified here so every
//! caller — in-crate and, eventually, downstream — gets the same
//! panic-free behaviour.

/// Snap `byte_idx` to the nearest UTF-8 char boundary in `s` at or
/// before `byte_idx`, clamping `byte_idx` to `s.len()` first.
///
/// Use this to make an arbitrary (possibly host-supplied, possibly
/// stale) byte offset safe to slice with: `&s[..snap_to_char_boundary(s,
/// byte_idx)]` never panics, regardless of where `byte_idx` originally
/// pointed.
pub fn snap_to_char_boundary(s: &str, byte_idx: usize) -> usize {
    let byte_idx = byte_idx.min(s.len());
    let mut i = byte_idx;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Return the byte offset of the char boundary immediately before
/// `byte_idx` (i.e. the start of the previous character). Returns `0`
/// if `byte_idx == 0`.
///
/// Intended for "move cursor left one char" style operations, where the
/// caller then slices or indexes at the returned offset.
pub fn prev_char_boundary(s: &str, byte_idx: usize) -> usize {
    let byte_idx = byte_idx.min(s.len());
    if byte_idx == 0 {
        return 0;
    }
    let mut i = byte_idx - 1;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Return the byte offset of the char boundary immediately after
/// `byte_idx` (i.e. the start of the next character), clamped to
/// `s.len()`.
///
/// Intended for "move cursor right one char" style operations.
pub fn next_char_boundary(s: &str, byte_idx: usize) -> usize {
    let byte_idx = byte_idx.min(s.len());
    if byte_idx >= s.len() {
        return s.len();
    }
    let mut i = byte_idx + 1;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Return `&s[..byte_idx]`, with `byte_idx` snapped to the nearest char
/// boundary at or before it — a panic-free replacement for `&s[..byte_idx]`
/// when `byte_idx` isn't known to be char-boundary-aligned (e.g. a
/// host-supplied cursor position).
pub fn safe_prefix(s: &str, byte_idx: usize) -> &str {
    &s[..snap_to_char_boundary(s, byte_idx)]
}

/// Return `&s[lo..hi]`, with both bounds snapped to char boundaries and
/// swapped if `lo > hi` — a panic-free replacement for `&s[lo..hi]` when
/// the bounds aren't known to be char-boundary-aligned or correctly
/// ordered.
pub fn safe_slice(s: &str, lo: usize, hi: usize) -> &str {
    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    let lo = snap_to_char_boundary(s, lo);
    let hi = snap_to_char_boundary(s, hi);
    &s[lo..hi]
}

/// Case-sensitive subsequence fuzzy match with a relevance score and
/// per-match byte positions.
///
/// Returns `None` if `query`'s characters do not all appear in
/// `haystack`, in order (not necessarily contiguously); otherwise
/// returns `Some((score, positions))`:
///
/// - `score` starts at a neutral baseline and is adjusted per matched
///   character: consecutive matches (no gap between them) earn a
///   "run" bonus, non-consecutive matches are penalised by the size of
///   the gap, and matches that land right after a word-boundary byte
///   (`/`, `\`, `_`, `-`, `.`, whitespace, or the start of the string)
///   earn a boundary bonus. Higher is a better match.
/// - `positions` are the **byte offsets** into `haystack` of each
///   matched character, in order — feed these straight into
///   [`crate::primitives::palette::PaletteItem::match_positions`] (or
///   an equivalent highlight field) so backends can highlight *why* a
///   row matched.
///
/// Matching is case-sensitive by design (mirroring the pre-existing
/// `dir_fuzzy_score` this replaces) — callers that want
/// case-insensitive matching should lowercase both `haystack` and
/// `query` before calling, same as
/// [`crate::compose::folder_picker`]'s filter already does.
///
/// An empty `query` is a trivial match against anything: `Some((0,
/// vec![]))`. Callers that want an "empty query shows everything,
/// unranked, in original order" fast path (as opposed to a
/// zero-scored, unsorted match) should special-case `query.is_empty()`
/// before calling — see [`crate::compose::filter_help_actions`] for
/// that convention.
pub fn fuzzy_score(haystack: &str, query: &str) -> Option<(i32, Vec<usize>)> {
    if query.is_empty() {
        return Some((0, Vec::new()));
    }

    let query_chars: Vec<char> = query.chars().collect();
    let mut qi = 0usize;
    let mut score = 100i32;
    let mut positions = Vec::with_capacity(query_chars.len());
    let mut prev_matched_char_idx: Option<usize> = None;
    let mut prev_char: Option<char> = None;

    for (char_idx, (byte_idx, ch)) in haystack.char_indices().enumerate() {
        if qi < query_chars.len() && ch == query_chars[qi] {
            if let Some(prev_idx) = prev_matched_char_idx {
                let gap = char_idx - prev_idx - 1;
                if gap == 0 {
                    score += 15; // consecutive-run bonus
                } else {
                    score -= gap as i32;
                }
            }
            let at_boundary = prev_char.is_none_or(|c| c.is_whitespace())
                || matches!(prev_char, Some('/' | '\\' | '_' | '-' | '.'));
            if at_boundary {
                score += 10;
            }
            positions.push(byte_idx);
            prev_matched_char_idx = Some(char_idx);
            qi += 1;
        }
        prev_char = Some(ch);
    }

    if qi == query_chars.len() {
        Some((score, positions))
    } else {
        None
    }
}

/// Word-aware soft-wrap: break `text` into rows no wider than
/// `col_budget` **characters**, breaking at whitespace where possible.
///
/// A single word longer than `col_budget` doesn't fit on any row no
/// matter where lines break, so it falls back to a hard character-index
/// break (chunked to `col_budget`) — this is the only case where a word
/// is split mid-word.
///
/// Runs of whitespace between words collapse to a single space when
/// they land inside a wrapped row (standard word-wrap behaviour); a
/// line that fits within `col_budget` unmodified is returned verbatim,
/// so single-space text with no wrapping needed round-trips exactly.
///
/// `text` is assumed to be a single logical line (no `\n`) — callers
/// that need to wrap multi-line text should split on `\n` first and
/// call this per line, same as
/// [`crate::compose::chat_controller`] does.
///
/// Zero budget or empty input is handled gracefully: `col_budget == 0`
/// returns `text` unmodified as a single row (there's no sane way to
/// wrap to zero columns), and empty `text` returns one empty row.
pub fn word_wrap(text: &str, col_budget: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    if col_budget == 0 {
        return vec![text.to_string()];
    }
    if text.chars().count() <= col_budget {
        return vec![text.to_string()];
    }

    let mut lines = Vec::new();
    let mut line = String::new();
    let mut line_len = 0usize;

    for word in text.split(' ') {
        let mut word_chars: Vec<char> = word.chars().collect();

        // A word that can't fit on an empty row no matter what — hard-break
        // it into budget-sized chunks, flushing any pending row first.
        while word_chars.len() > col_budget {
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
                line_len = 0;
            }
            let chunk: String = word_chars.drain(..col_budget).collect();
            lines.push(chunk);
        }

        let word_len = word_chars.len();
        let word_str: String = word_chars.into_iter().collect();

        if line.is_empty() {
            line.push_str(&word_str);
            line_len = word_len;
        } else if line_len + 1 + word_len <= col_budget {
            line.push(' ');
            line.push_str(&word_str);
            line_len += 1 + word_len;
        } else {
            lines.push(std::mem::take(&mut line));
            line.push_str(&word_str);
            line_len = word_len;
        }
    }

    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── snap_to_char_boundary / prev_char_boundary / next_char_boundary ──

    #[test]
    fn snap_to_char_boundary_already_on_boundary_is_noop() {
        let s = "héllo";
        assert_eq!(snap_to_char_boundary(s, 0), 0);
        assert_eq!(snap_to_char_boundary(s, 1), 1);
        // 'é' is 2 bytes starting at byte 1, boundary after it is byte 3.
        assert_eq!(snap_to_char_boundary(s, 3), 3);
    }

    #[test]
    fn snap_to_char_boundary_mid_char_walks_back() {
        let s = "héllo";
        // byte 2 is inside 'é' (bytes 1..3) — must walk back to 1.
        assert_eq!(snap_to_char_boundary(s, 2), 1);
    }

    #[test]
    fn snap_to_char_boundary_clamps_past_end() {
        let s = "abc";
        assert_eq!(snap_to_char_boundary(s, 100), 3);
    }

    #[test]
    fn snap_to_char_boundary_cjk_and_emoji() {
        let s = "中文🎉end";
        // Every offset, however it lands mid-char, should snap to a valid
        // boundary and never panic when used to slice.
        for i in 0..=s.len() {
            let snapped = snap_to_char_boundary(s, i);
            assert!(s.is_char_boundary(snapped));
            let _ = &s[..snapped]; // must not panic
        }
    }

    #[test]
    fn prev_char_boundary_at_zero_stays_zero() {
        assert_eq!(prev_char_boundary("abc", 0), 0);
    }

    #[test]
    fn prev_char_boundary_steps_back_one_multibyte_char() {
        let s = "héllo";
        // Cursor right after 'é' (byte 3) should move to right before it (byte 1).
        assert_eq!(prev_char_boundary(s, 3), 1);
    }

    #[test]
    fn prev_char_boundary_from_mid_char_lands_before_that_char() {
        let s = "héllo";
        // byte 2 is mid-'é'; prev boundary is the start of 'é' at byte 1.
        assert_eq!(prev_char_boundary(s, 2), 1);
    }

    #[test]
    fn prev_char_boundary_clamps_past_end() {
        let s = "héllo";
        // Same clamp-then-walk-back behaviour as snap_to_char_boundary /
        // next_char_boundary for an out-of-range byte_idx.
        assert_eq!(prev_char_boundary(s, 999), prev_char_boundary(s, s.len()));
    }

    #[test]
    fn next_char_boundary_at_end_stays_at_end() {
        let s = "abc";
        assert_eq!(next_char_boundary(s, 3), 3);
        assert_eq!(next_char_boundary(s, 100), 3);
    }

    #[test]
    fn next_char_boundary_steps_forward_one_multibyte_char() {
        let s = "héllo";
        // Cursor right before 'é' (byte 1) should move past it to byte 3.
        assert_eq!(next_char_boundary(s, 1), 3);
    }

    #[test]
    fn next_char_boundary_from_mid_char_lands_after_that_char() {
        let s = "héllo";
        assert_eq!(next_char_boundary(s, 2), 3);
    }

    // ── safe_prefix / safe_slice ───────────────────────────────────────

    #[test]
    fn safe_prefix_on_boundary_matches_manual_slice() {
        let s = "héllo";
        assert_eq!(safe_prefix(s, 3), &s[..3]);
    }

    #[test]
    fn safe_prefix_mid_char_does_not_panic() {
        let s = "héllo";
        assert_eq!(safe_prefix(s, 2), "h");
    }

    #[test]
    fn safe_prefix_past_end_returns_whole_string() {
        let s = "héllo";
        assert_eq!(safe_prefix(s, 999), s);
    }

    #[test]
    fn safe_slice_mid_char_bounds_do_not_panic() {
        let s = "中文🎉end";
        // Arbitrary byte offsets landing inside multibyte chars must still
        // produce a valid (possibly empty) slice, never panic.
        for lo in 0..=s.len() {
            for hi in 0..=s.len() {
                let slice = safe_slice(s, lo, hi);
                let _ = slice; // must not panic; content already validated by &str type
            }
        }
    }

    #[test]
    fn safe_slice_swaps_reversed_bounds() {
        let s = "abcdef";
        assert_eq!(safe_slice(s, 4, 1), &s[1..4]);
    }

    #[test]
    fn safe_slice_on_boundaries_matches_manual_slice() {
        let s = "héllo world";
        assert_eq!(safe_slice(s, 0, 3), &s[0..3]);
    }

    // ── fuzzy_score ─────────────────────────────────────────────────────

    #[test]
    fn fuzzy_score_exact_match_scores_high() {
        let s = fuzzy_score("src/main.rs", "src/main.rs");
        assert!(s.is_some());
        assert!(s.unwrap().0 > 0);
    }

    #[test]
    fn fuzzy_score_subsequence_matches() {
        // "sm" is a subsequence of "src/main"
        let s = fuzzy_score("src/main", "sm");
        assert!(s.is_some());
    }

    #[test]
    fn fuzzy_score_non_subsequence_is_none() {
        assert!(fuzzy_score("src/main", "xyz").is_none());
    }

    #[test]
    fn fuzzy_score_boundary_bonus() {
        // "m" starting right after a "/" boundary should score higher than
        // a "m" buried mid-word.
        let at_boundary = fuzzy_score("src/main", "m").unwrap().0;
        let mid_word = fuzzy_score("abcmain", "m").unwrap().0;
        assert!(at_boundary > mid_word);
    }

    #[test]
    fn fuzzy_score_consecutive_run_beats_scattered() {
        // "ab" contiguous in "xxabxx" should outscore "ab" scattered
        // across "axbxxx" (a...b with a gap).
        let contiguous = fuzzy_score("xxabxx", "ab").unwrap().0;
        let scattered = fuzzy_score("axbxxx", "ab").unwrap().0;
        assert!(contiguous > scattered);
    }

    #[test]
    fn fuzzy_score_returns_match_positions() {
        let (_, positions) = fuzzy_score("abcdef", "ace").unwrap();
        assert_eq!(positions, vec![0, 2, 4]);
    }

    #[test]
    fn fuzzy_score_empty_query_matches_trivially() {
        let s = fuzzy_score("anything", "");
        assert_eq!(s, Some((0, Vec::new())));
    }

    #[test]
    fn fuzzy_score_empty_haystack_non_empty_query_is_none() {
        assert!(fuzzy_score("", "x").is_none());
    }

    #[test]
    fn fuzzy_score_treats_backslash_as_boundary() {
        let posix = fuzzy_score("src/main", "m").unwrap().0;
        let windows = fuzzy_score("src\\main", "m").unwrap().0;
        let mid = fuzzy_score("abcmain", "m").unwrap().0;
        assert_eq!(posix, windows);
        assert!(posix > mid);
    }

    // ── word_wrap ───────────────────────────────────────────────────────

    #[test]
    fn word_wrap_short_line_is_not_split() {
        assert_eq!(word_wrap("hello", 80), vec!["hello".to_string()]);
    }

    #[test]
    fn word_wrap_empty_returns_one_empty_row() {
        assert_eq!(word_wrap("", 80), vec![String::new()]);
    }

    #[test]
    fn word_wrap_zero_budget_returns_text_unmodified() {
        assert_eq!(word_wrap("hello world", 0), vec!["hello world".to_string()]);
    }

    #[test]
    fn word_wrap_breaks_at_word_boundary() {
        let wrapped = word_wrap("hello world", 7);
        assert_eq!(wrapped, vec!["hello".to_string(), "world".to_string()]);
    }

    #[test]
    fn word_wrap_packs_multiple_words_per_row() {
        let wrapped = word_wrap("the quick brown fox", 10);
        // "the quick" = 9 chars fits in 10; "brown" (5) doesn't fit after
        // "the quick" (9 + 1 + 5 = 15 > 10); "brown fox" = 9 fits.
        assert_eq!(
            wrapped,
            vec!["the quick".to_string(), "brown fox".to_string()]
        );
    }

    #[test]
    fn word_wrap_hard_breaks_a_word_longer_than_budget() {
        // No spaces at all — must degrade to the old char-chunk behaviour.
        let wrapped = word_wrap("abcde", 3);
        assert_eq!(wrapped, vec!["abc".to_string(), "de".to_string()]);
    }

    #[test]
    fn word_wrap_hard_breaks_overlong_word_within_wrapped_text() {
        // "implementation" (14 chars) alone exceeds a 10-col budget and must
        // hard-break, but short neighbouring words still wrap on spaces.
        let wrapped = word_wrap("a implementation b", 10);
        assert_eq!(
            wrapped,
            vec![
                "a".to_string(),
                "implementa".to_string(),
                "tion b".to_string(),
            ]
        );
    }

    #[test]
    fn word_wrap_never_exceeds_budget() {
        let text = "the quick brown fox jumps over the lazy dog and then some more words follow";
        for budget in 1..12 {
            for row in word_wrap(text, budget) {
                assert!(
                    row.chars().count() <= budget,
                    "row {row:?} exceeds budget {budget}"
                );
            }
        }
    }

    #[test]
    fn word_wrap_no_budget_loses_no_non_space_characters() {
        let text = "the quick brown fox jumps over the lazy dog";
        let wrapped = word_wrap(text, 6);
        let rejoined: String = wrapped.join(" ");
        let original_non_space: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        let rejoined_non_space: String = rejoined.chars().filter(|c| !c.is_whitespace()).collect();
        assert_eq!(original_non_space, rejoined_non_space);
    }
}
