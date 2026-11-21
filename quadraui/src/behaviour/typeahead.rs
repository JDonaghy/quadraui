//! Buffered, debounced "type letters to jump to a matching item" — the
//! behaviour a focused list/combo box offers when the user types a few
//! characters in quick succession instead of using arrow keys.
//!
//! [`Typeahead`] accumulates typed characters into a buffer, resetting it
//! whenever more than 500ms elapse between keystrokes (so "a" then,
//! 2 seconds later, "b" searches for "b", not "ab"). [`Typeahead::push`]
//! resolves the current buffer against a caller-supplied `len`-item list
//! (`items(i)` names item `i`'s text), searching forward from `from` with
//! wraparound, case-insensitively — the same case-insensitivity
//! [`crate::ComboBox::filtered_indices`] matches its own `text` query
//! with, applied here to a debounced keystroke buffer instead of a live,
//! continuously-edited query. The match itself is a *prefix* match, which
//! is the type-to-jump semantic; `filtered_indices` matches a substring,
//! which is the filter-as-you-type semantic.

/// A debounced keystroke buffer for typeahead ("type to jump") navigation.
/// Stateless aside from the buffer itself — the caller owns one `Typeahead`
/// per focused list, the same way it owns every other piece of input-timing
/// state this crate exposes (see [`crate::CaretBlinkState`]) rather than a
/// widget descriptor carrying it on the wire.
#[derive(Debug, Clone, Default)]
pub struct Typeahead {
    buffer: String,
    last_ms: u64,
}

/// How long a gap between keystrokes resets the buffer instead of
/// extending it.
const RESET_AFTER_MS: u64 = 500;

impl Typeahead {
    /// An empty buffer, as if no key had been typed yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// The buffer's current contents (lowercased on push, so this is
    /// already the needle [`Self::push`] searches with).
    pub fn buffer(&self) -> &str {
        &self.buffer
    }

    /// Append `ch` to the buffer (resetting it first if `now_ms` is more
    /// than 500ms after the last keystroke), then search `len` items —
    /// `items(i)` names item `i`'s text — forward from `from`, wrapping,
    /// for the first whose text starts with the buffer, case-insensitively.
    ///
    /// Returns `None` when `len == 0` or nothing matches; the buffer still
    /// updates either way, so the next keystroke extends (or correctly
    /// resets) it.
    pub fn push<'a>(
        &mut self,
        ch: char,
        now_ms: u64,
        items: impl Fn(usize) -> &'a str,
        len: usize,
        from: usize,
    ) -> Option<usize> {
        if now_ms.saturating_sub(self.last_ms) > RESET_AFTER_MS {
            self.buffer.clear();
        }
        for lower in ch.to_lowercase() {
            self.buffer.push(lower);
        }
        self.last_ms = now_ms;

        if len == 0 {
            return None;
        }
        let from = from.min(len - 1);
        (0..len)
            .map(|offset| (from + offset) % len)
            .find(|&i| items(i).to_lowercase().starts_with(self.buffer.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPTIONS: &[&str] = &["Apple", "Apricot", "Banana", "Blueberry", "Cherry"];
    fn item(i: usize) -> &'static str {
        OPTIONS[i]
    }

    #[test]
    fn single_char_matches_first_from_start() {
        let mut t = Typeahead::new();
        assert_eq!(t.push('b', 0, item, OPTIONS.len(), 0), Some(2));
    }

    #[test]
    fn accumulates_within_the_debounce_window() {
        let mut t = Typeahead::new();
        assert_eq!(t.push('a', 0, item, OPTIONS.len(), 0), Some(0));
        // "ap" still matches "Apple" (index 0) before "Apricot".
        assert_eq!(t.push('p', 100, item, OPTIONS.len(), 0), Some(0));
        // "apr" skips "Apple", matches "Apricot".
        assert_eq!(t.push('r', 200, item, OPTIONS.len(), 0), Some(1));
    }

    #[test]
    fn resets_after_500ms_of_inactivity() {
        let mut t = Typeahead::new();
        assert_eq!(t.push('a', 0, item, OPTIONS.len(), 0), Some(0));
        // 600ms later: "p" alone, not "ap" — no option starts with "p".
        assert_eq!(t.push('p', 600, item, OPTIONS.len(), 0), None);
        assert_eq!(t.buffer(), "p");
    }

    #[test]
    fn exactly_at_the_boundary_still_resets() {
        let mut t = Typeahead::new();
        t.push('a', 0, item, OPTIONS.len(), 0);
        // `now_ms - last_ms == 500` is not `> 500`, so this still extends.
        assert_eq!(t.push('p', 500, item, OPTIONS.len(), 0), Some(0));
        assert_eq!(t.buffer(), "ap");
    }

    #[test]
    fn search_wraps_from_the_given_index() {
        let mut t = Typeahead::new();
        // Starting from index 3 ("Blueberry"), "c" should wrap around to
        // "Cherry" (index 4) rather than stopping at the end of the list.
        assert_eq!(t.push('c', 0, item, OPTIONS.len(), 3), Some(4));
    }

    #[test]
    fn no_match_is_none_but_buffer_still_updates() {
        let mut t = Typeahead::new();
        assert_eq!(t.push('z', 0, item, OPTIONS.len(), 0), None);
        assert_eq!(t.buffer(), "z");
    }

    #[test]
    fn empty_list_is_none() {
        let mut t = Typeahead::new();
        assert_eq!(t.push('a', 0, item, 0, 0), None);
    }

    #[test]
    fn matching_is_case_insensitive() {
        let mut t = Typeahead::new();
        assert_eq!(t.push('B', 0, item, OPTIONS.len(), 0), Some(2));
    }
}
