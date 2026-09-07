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
//!
//! [`char_cell_width`] and [`display_width`] measure real terminal
//! display width (CJK/emoji count double, combining marks count zero) —
//! see issue #471, which found `StyledText::visible_width`
//! (`crate::types`) still counting `chars()` years after #206 fixed the
//! equivalent terminal-cell logic, because that fix landed only in the
//! `tui`-feature-gated `tui::text` module while `types` is core and
//! unconditionally compiled. These live here instead so core code can
//! use them without depending on `tui`/`gtk`; `tui::text` re-exports the
//! same two functions for API stability.
//!
//! [`wrap_spans`] is the one display-width-aware line wrapper in the
//! crate — see issue #821. Before it, three independent wrappers existed,
//! all budgeting rows by `chars().count()` instead of [`display_width`]:
//! this module's own [`word_wrap`], `compose::markdown`'s private
//! `wrap_plain_line`, and `compose::chat_controller`'s private
//! `wrap_spans`. `chars().count()` wraps a row of CJK or emoji text too
//! late — a 10-cell-wide row of double-width characters was let through
//! at 10 *characters* (20 cells), overflowing the budget it was supposed
//! to respect. [`word_wrap`] is now a thin adapter over [`wrap_spans`]
//! (same signature, same behaviour, now display-width-correct); the
//! other two copies are gone — their call sites
//! (`markdown::render_markdown_to_styled_wrapped`,
//! `chat_controller::build_transcript_rows`) call [`wrap_spans`]
//! directly.

use crate::types::StyledSpan;

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

/// Terminal cell width of a single character (0, 1, or 2).
///
/// Uses the `unicode-width` crate's UAX#11 tables directly, with no
/// codepoint-range overrides. Private-Use-Area codepoints — including
/// both Nerd Font PUA blocks (BMP `U+E000`–`U+F8FF` and Supplementary-A
/// `U+F0000`–`U+FFFFD`) — measure as width 1, matching `unicode-width`
/// and `Nerd Font Mono` (the terminal-recommended, single-cell variant).
/// Non-Mono Nerd Font variants are genuinely double-width, but that is a
/// font/theme property, not something derivable from the codepoint
/// alone; if double-width PUA glyphs ever need supporting, it must come
/// in as an explicit input (theme/config/probe), not a range guess here.
/// See issue #545.
///
/// Lives here (rather than `tui::text`) so core types like
/// [`crate::types::StyledText`] can measure real display width without
/// depending on the `tui` or `gtk` features — see #471.
/// [`crate::tui::char_cell_width`] re-exports this same function; it
/// is not a separate implementation.
pub fn char_cell_width(c: char) -> u16 {
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

/// Whether `c` is a double-width glyph (CJK, emoji, ...) per
/// [`char_cell_width`] — i.e. `char_cell_width(c) == 2`.
///
/// Named for callers that only need the wide/narrow classification, not
/// the numeric width: pixel-based terminal rasterisers (`gtk`, `macos`)
/// use this to decide whether a cell claims one grid column or two — see
/// [`crate::terminal_style::wide_cell_advance`], issue #500.
pub fn is_wide_char(c: char) -> bool {
    char_cell_width(c) == 2
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
/// `col_budget` **display cells** (see [`display_width`] — CJK and emoji
/// count as 2), breaking at whitespace where possible.
///
/// A single word longer than `col_budget` doesn't fit on any row no
/// matter where lines break, so it falls back to a hard display-width
/// break (chunked to `col_budget` cells) — this is the only case where a
/// word is split mid-word.
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
///
/// A thin adapter over [`wrap_spans`] with [`WrapPolicy::Word`] — see
/// issue #821. The signature and behaviour are unchanged from before
/// that issue; only the row-budget accounting changed, from
/// `chars().count()` to [`display_width`], so CJK/emoji text now wraps
/// at the correct column instead of overflowing.
pub fn word_wrap(text: &str, col_budget: usize) -> Vec<String> {
    let spans = [StyledSpan::plain(text)];
    wrap_spans(&spans, col_budget, WrapPolicy::Word)
        .into_iter()
        .map(|group| group.iter().map(|s| s.text.as_str()).collect())
        .collect()
}

/// How [`wrap_spans`] should treat word boundaries when a row must break.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrapPolicy {
    /// Break at whitespace where possible; a single word wider than the
    /// row budget hard-breaks at the display-width budget. This is
    /// [`word_wrap`]'s policy, generalised to styled spans.
    Word,
    /// Ignore word boundaries entirely: break exactly at the
    /// display-width budget, mid-word or mid-span if necessary. Matches
    /// the pre-#821 `chat_controller::wrap_spans` behaviour, used for
    /// markdown-styled chat turns where per-span word-splitting was
    /// previously considered too costly — now that [`wrap_spans`] does
    /// it directly, [`WrapPolicy::Word`] is available there too, but
    /// `Char` is kept as an explicit, distinct policy so that switching a
    /// call site's wrap behaviour is a one-word decision, not a silent
    /// side effect of this unification.
    Char,
}

/// Wrap a styled-span line to `col_budget` **display cells** per row (see
/// [`display_width`]), honouring `policy`.
///
/// Returns one `Vec<StyledSpan>` per visual row; a span that crosses a
/// wrap boundary is split, with the original style (fg/bg/bold/italic/
/// underline) cloned onto every resulting chunk. The concatenation of
/// each row's span texts, in order, reconstructs the corresponding slice
/// of the original line's plain text (`WrapPolicy::Word` drops the single
/// whitespace character consumed at each wrap point, same as
/// [`word_wrap`]).
///
/// `spans` empty, or every span's `text` empty, returns `vec![vec![]]`
/// (one empty row) — same convention as `col_budget == 0` or the whole
/// line already fitting, both of which return `vec![spans.to_vec()]`
/// (the input unmodified, as a single row).
///
/// This is the crate's one line wrapper — see the module doc and issue
/// #821.
pub fn wrap_spans(
    spans: &[StyledSpan],
    col_budget: usize,
    policy: WrapPolicy,
) -> Vec<Vec<StyledSpan>> {
    // Flatten to (char, index into `spans` of the span that owns it) so
    // wrapping can work at cell-width granularity while still knowing
    // which style to reapply when it re-groups a row into spans.
    let chars: Vec<(char, usize)> = spans
        .iter()
        .enumerate()
        .flat_map(|(idx, span)| span.text.chars().map(move |c| (c, idx)))
        .collect();

    if spans.is_empty() || chars.is_empty() {
        return vec![vec![]];
    }

    let total_width: usize = chars
        .iter()
        .map(|&(c, _)| char_cell_width(c) as usize)
        .sum();
    if col_budget == 0 || total_width <= col_budget {
        return vec![spans.to_vec()];
    }

    let rows: Vec<Vec<(char, usize)>> = match policy {
        WrapPolicy::Char => wrap_chars_by_width(&chars, col_budget),
        WrapPolicy::Word => wrap_chars_by_word(&chars, col_budget),
    };

    rows.iter()
        .map(|row| group_into_spans(row, spans))
        .collect()
}

/// `WrapPolicy::Char`: break strictly at `budget` display cells, ignoring
/// word boundaries. Always makes progress — a single character wider
/// than `budget` (a lone CJK/emoji glyph in a narrow budget) still lands
/// alone on its own row rather than looping forever.
fn wrap_chars_by_width(chars: &[(char, usize)], budget: usize) -> Vec<Vec<(char, usize)>> {
    let mut rows = Vec::new();
    let mut row: Vec<(char, usize)> = Vec::new();
    let mut row_w = 0usize;
    for &(c, idx) in chars {
        let w = char_cell_width(c) as usize;
        if !row.is_empty() && row_w + w > budget {
            rows.push(std::mem::take(&mut row));
            row_w = 0;
        }
        row.push((c, idx));
        row_w += w;
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

/// `WrapPolicy::Word`: pack whitespace-delimited words onto rows greedily,
/// joining consecutive words with a single space (collapsing any wider
/// run of literal `' '` characters at that boundary — same collapsing
/// [`word_wrap`] has always done). A word wider than `budget` hard-breaks
/// via [`wrap_chars_by_width`], the same as a plain [`WrapPolicy::Char`]
/// chunk, and any leftover chunk becomes the start of the next row so
/// later words can still pack onto it.
fn wrap_chars_by_word(chars: &[(char, usize)], budget: usize) -> Vec<Vec<(char, usize)>> {
    // Tokenize into alternating whitespace / non-whitespace runs. Only
    // the literal space character `' '` is treated as a separator,
    // matching `word_wrap`'s pre-#821 `text.split(' ')`.
    enum TokenKind {
        Word,
        Space,
    }
    let mut tokens: Vec<(TokenKind, Vec<(char, usize)>)> = Vec::new();
    for &(c, idx) in chars {
        let kind_is_space = c == ' ';
        match tokens.last_mut() {
            Some((TokenKind::Space, run)) if kind_is_space => run.push((c, idx)),
            Some((TokenKind::Word, run)) if !kind_is_space => run.push((c, idx)),
            _ => tokens.push((
                if kind_is_space {
                    TokenKind::Space
                } else {
                    TokenKind::Word
                },
                vec![(c, idx)],
            )),
        }
    }

    let mut rows: Vec<Vec<(char, usize)>> = Vec::new();
    let mut row: Vec<(char, usize)> = Vec::new();
    let mut row_w = 0usize;
    // The single space (collapsed from however many literal spaces were
    // in the source run) pending insertion before the next word, if any
    // word actually follows it on the same row.
    let mut pending_space: Option<(char, usize)> = None;

    for (kind, content) in tokens {
        match kind {
            TokenKind::Space => {
                pending_space = content.first().copied();
            }
            TokenKind::Word => {
                let word_w: usize = content
                    .iter()
                    .map(|&(c, _)| char_cell_width(c) as usize)
                    .sum();
                if word_w > budget {
                    // Doesn't fit on any row by itself — flush the
                    // current row (dropping any pending space, since a
                    // fresh row follows regardless) and hard-break the
                    // word by display width.
                    if !row.is_empty() {
                        rows.push(std::mem::take(&mut row));
                    }
                    pending_space = None;
                    let mut chunks = wrap_chars_by_width(&content, budget);
                    // Leave the last chunk as the new current row so a
                    // following short word may still pack onto it.
                    row = chunks.pop().unwrap_or_default();
                    row_w = row.iter().map(|&(c, _)| char_cell_width(c) as usize).sum();
                    rows.extend(chunks);
                } else {
                    let extra = if row.is_empty() || pending_space.is_none() {
                        0
                    } else {
                        1
                    };
                    if row_w + extra + word_w <= budget {
                        if extra == 1 {
                            row.push(pending_space.expect("extra == 1 implies Some"));
                            row_w += 1;
                        }
                        row.extend_from_slice(&content);
                        row_w += word_w;
                    } else {
                        if !row.is_empty() {
                            rows.push(std::mem::take(&mut row));
                        }
                        row = content;
                        row_w = word_w;
                    }
                    pending_space = None;
                }
            }
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }
    if rows.is_empty() {
        rows.push(Vec::new());
    }
    rows
}

/// Re-group a flattened `(char, owning-span-index)` row into
/// [`StyledSpan`]s, merging consecutive characters that share the same
/// owning span into one chunk and cloning that span's style onto it.
fn group_into_spans(row: &[(char, usize)], spans: &[StyledSpan]) -> Vec<StyledSpan> {
    let mut result = Vec::new();
    let mut i = 0usize;
    while i < row.len() {
        let idx = row[i].1;
        let mut text = String::new();
        while i < row.len() && row[i].1 == idx {
            text.push(row[i].0);
            i += 1;
        }
        result.push(StyledSpan {
            text,
            ..spans[idx].clone()
        });
    }
    result
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

    // ── char_cell_width / display_width ────────────────────────────────

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
    fn is_wide_char_classifies_cjk_and_ascii() {
        assert!(is_wide_char('日'));
        assert!(is_wide_char('中'));
        assert!(!is_wide_char('a'));
        assert!(!is_wide_char(' '));
        assert!(!is_wide_char('\u{0301}'));
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

    // ── word_wrap / wrap_spans: CJK + emoji display-width correctness ────
    //
    // RED against a `chars().count()`-based wrapper (issue #821): five CJK
    // characters are 5 *characters* but 10 *display cells*. A
    // `chars().count()` budget of 6 would let all 5 through as "6 chars or
    // fewer" and never wrap at all; a display-width budget of 6 must wrap
    // after 3 (6 cells), not 5.

    #[test]
    fn word_wrap_cjk_wraps_at_display_width_not_char_count() {
        let text = "中文中文中文"; // 6 chars, 12 display cells
        let wrapped = word_wrap(text, 6);
        // `chars().count() <= 6` would wrongly treat this as a single
        // short row (it's 6 *characters*) instead of wrapping at 6 *cells*
        // (3 CJK characters).
        assert_eq!(
            wrapped,
            vec!["中文中".to_string(), "文中文".to_string()],
            "must wrap by display width (2 cells/char), not char count"
        );
        for row in &wrapped {
            assert!(
                display_width(row) <= 6,
                "row {row:?} exceeds the 6-cell budget"
            );
        }
    }

    #[test]
    fn word_wrap_emoji_wraps_at_display_width() {
        // Each emoji below is a single `char` but a 2-cell glyph.
        let text = "🎉🎉🎉🎉🎉";
        let wrapped = word_wrap(text, 4);
        for row in &wrapped {
            assert!(
                display_width(row) <= 4,
                "row {row:?} exceeds the 4-cell budget"
            );
        }
        // 5 emoji * 2 cells = 10 cells; budget 4 must produce at least 3 rows.
        assert!(wrapped.len() >= 3, "expected wrapping, got {wrapped:?}");
    }

    // ── wrap_spans ─────────────────────────────────────────────────────

    #[test]
    fn wrap_spans_char_policy_short_line_not_split() {
        let spans = vec![StyledSpan::plain("hello")];
        let groups = wrap_spans(&spans, 80, WrapPolicy::Char);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 1);
        assert_eq!(groups[0][0].text, "hello");
    }

    #[test]
    fn wrap_spans_char_policy_splits_single_span_at_budget() {
        let spans = vec![StyledSpan::plain("abcde")];
        let groups = wrap_spans(&spans, 3, WrapPolicy::Char);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0][0].text, "abc");
        assert_eq!(groups[1][0].text, "de");
    }

    #[test]
    fn wrap_spans_char_policy_preserves_style_on_split_chunks() {
        use crate::types::Color;
        let bold_span = StyledSpan {
            text: "abcdef".to_string(),
            fg: Some(Color::rgb(255, 0, 0)),
            bg: None,
            bold: true,
            italic: false,
            underline: false,
        };
        let groups = wrap_spans(&[bold_span], 4, WrapPolicy::Char);
        assert_eq!(groups.len(), 2);
        assert!(groups[0][0].bold, "first chunk must retain bold");
        assert!(groups[1][0].bold, "second chunk must retain bold");
        assert_eq!(groups[0][0].fg, Some(Color::rgb(255, 0, 0)));
        assert_eq!(groups[0][0].text, "abcd");
        assert_eq!(groups[1][0].text, "ef");
    }

    #[test]
    fn wrap_spans_char_policy_multi_span_boundary() {
        // Two 3-char spans at budget=4 → first row gets span-A (3) + one
        // char of span-B; second row gets remaining 2 chars of span-B.
        let a = StyledSpan::plain("abc");
        let b = StyledSpan::plain("def");
        let groups = wrap_spans(&[a, b], 4, WrapPolicy::Char);
        assert_eq!(groups.len(), 2);
        let row0_text: String = groups[0].iter().map(|s| s.text.as_str()).collect();
        let row1_text: String = groups[1].iter().map(|s| s.text.as_str()).collect();
        assert_eq!(row0_text, "abcd");
        assert_eq!(row1_text, "ef");
    }

    #[test]
    fn wrap_spans_empty_returns_one_empty_group() {
        let groups = wrap_spans(&[], 80, WrapPolicy::Char);
        assert_eq!(groups.len(), 1);
        assert!(groups[0].is_empty());
        let groups = wrap_spans(&[], 80, WrapPolicy::Word);
        assert_eq!(groups.len(), 1);
        assert!(groups[0].is_empty());
    }

    #[test]
    fn wrap_spans_char_policy_is_display_width_aware() {
        // Old `chars().count()`-based per-span wrapping would let a
        // 6-cell-wide (3-char) CJK span through under a 6-char budget
        // without ever consulting cell width. It must wrap at 3 chars (6
        // cells), matching `WrapPolicy::Char`'s "break exactly at the
        // display-width budget" contract.
        let spans = vec![StyledSpan::plain("中文中文")]; // 4 chars, 8 cells
        let groups = wrap_spans(&spans, 6, WrapPolicy::Char);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0][0].text, "中文中");
        assert_eq!(groups[1][0].text, "文");
    }

    #[test]
    fn wrap_spans_word_policy_preserves_style_across_word_boundary() {
        // "aaa " (plain) + "bold text" (bold) + " bbb" (plain), wrapped at
        // a budget that must split inside the bold span.
        let a = StyledSpan::plain("aaa ");
        let bold = StyledSpan {
            text: "bold text".to_string(),
            fg: None,
            bg: None,
            bold: true,
            italic: false,
            underline: false,
        };
        let c = StyledSpan::plain(" bbb");
        let groups = wrap_spans(&[a, bold, c], 8, WrapPolicy::Word);
        // Row texts must reconstruct to the original words, and the bold
        // style must survive the split between "bold" and "text".
        let row_texts: Vec<String> = groups
            .iter()
            .map(|g| g.iter().map(|s| s.text.as_str()).collect())
            .collect();
        assert_eq!(
            row_texts,
            vec!["aaa bold".to_string(), "text bbb".to_string()]
        );
        assert!(
            groups[0].iter().any(|s| s.bold && s.text == "bold"),
            "bold must survive onto row 0: {:?}",
            groups[0]
        );
        assert!(
            groups[1].iter().any(|s| s.bold && s.text == "text"),
            "bold must survive onto row 1: {:?}",
            groups[1]
        );
        assert!(
            groups[1].iter().any(|s| !s.bold && s.text.trim() == "bbb"),
            "trailing plain text must stay non-bold: {:?}",
            groups[1]
        );
    }
}
