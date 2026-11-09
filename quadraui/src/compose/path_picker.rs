//! `PathPickerCore` — the state machine shared by
//! [`FolderPickerController`] and [`FilePickerController`].
//!
//! Both pickers browse a filesystem tree with the same query/list/
//! selection/scroll mechanics and the same key map; they differ only in
//! *how* they walk the filesystem (directories only vs. files + directories,
//! extension filters, the `"."` row) and in what `Enter` resolves to. This
//! module owns the shared half so the two facades stay in lock-step instead
//! of drifting apart one small fix at a time.
//!
//! [`FolderPickerController`]: crate::compose::FolderPickerController
//! [`FilePickerController`]: crate::compose::FilePickerController

use std::cmp::Reverse;
use std::path::PathBuf;

use crate::{Key, Modifiers, NamedKey};

/// The outcome of classifying a raw `Key` + `Modifiers` pair against the
/// shared path-picker key map.
///
/// Both facades route every `KeyPressed` event through
/// [`classify_nav_key`] and translate the result into their own
/// `*PickerEvent`, so the key map itself — including the vim-style `j`/`k`/
/// `-` shortcuts — can never drift between the two pickers again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NavKey {
    /// Dismiss the picker.
    Escape,
    /// Resolve the current selection (open/save/navigate-into).
    Confirm,
    /// Move the selection up one row (`Up` arrow or vim `k`).
    Up,
    /// Move the selection down one row (`Down` arrow or vim `j`).
    Down,
    /// Navigate to the parent directory (vim netrw `-`).
    NavigateUp,
    /// Remove the last query character.
    Backspace,
    /// Append a printable character to the query.
    Char(char),
}

/// Classify a `KeyPressed` event against the shared path-picker key map.
///
/// `cmd` is treated the same as `ctrl`: every backend delivers a
/// Cmd-modified letter (the macOS accelerator convention) as a plain
/// `Key::Char` with the modifier set, not a dedicated key variant, so it
/// must be excluded here or it would be typed into the query / trigger a
/// vim-style shortcut instead of falling through `None` for an app-level
/// accelerator.
///
/// Trade-off (shared by both pickers): the vim shortcuts `-` (parent dir),
/// `k` (up), and `j` (down) are matched *before* the generic
/// `Key::Char(c) => Char(c)` arm. That means the query can never begin
/// with — or contain — those three characters when typed through
/// `handle()`; a consumer that needs typing-priority (e.g. filenames that
/// routinely start with `j`/`k`/`-`) can call the state-mutation methods
/// (`push_char`, `move_up`, …) directly instead of routing through
/// `handle()`.
pub(crate) fn classify_nav_key(key: &Key, modifiers: &Modifiers) -> Option<NavKey> {
    let modified = modifiers.ctrl || modifiers.cmd;
    match key {
        Key::Named(NamedKey::Escape) => Some(NavKey::Escape),
        Key::Named(NamedKey::Enter) => Some(NavKey::Confirm),

        Key::Named(NamedKey::Up) => Some(NavKey::Up),
        Key::Char('k') if !modified => Some(NavKey::Up),

        Key::Named(NamedKey::Down) => Some(NavKey::Down),
        Key::Char('j') if !modified => Some(NavKey::Down),

        Key::Char('-') if !modified => Some(NavKey::NavigateUp),

        Key::Named(NamedKey::Backspace) => Some(NavKey::Backspace),

        Key::Char(c) if !modified => Some(NavKey::Char(*c)),

        _ => None,
    }
}

/// Subsequence fuzzy match returning a relevance score, or `None` if the
/// query is not a subsequence of `path`.
///
/// Higher scores mean the query characters appear closer together and/or
/// at word boundaries (`/`, `\`, `_`, `-`, `.`). Both forward and backward
/// slashes count so that Windows paths get the same boundary bonus as
/// POSIX paths.
///
/// Thin wrapper over [`crate::text_util::fuzzy_score`] — the match
/// positions it also returns aren't needed here (neither picker
/// highlights per-character matches), so this drops them and keeps the
/// score-only signature [`filter_entries`] expects.
pub(crate) fn fuzzy_score_path(path: &str, query: &str) -> Option<i32> {
    crate::text_util::fuzzy_score(path, query).map(|(score, _positions)| score)
}

/// Filter `all` by `query` using subsequence matching with a relevance
/// score. Shared by [`FolderPickerController`](crate::compose::FolderPickerController)
/// and [`FilePickerController`](crate::compose::FilePickerController) so
/// both rank matches identically.
///
/// Returns up to 50 entries, sorted by descending score. If `query` is
/// empty, the first 50 entries are returned unchanged.
pub(crate) fn filter_entries(all: &[PathBuf], query: &str) -> Vec<PathBuf> {
    const CAP: usize = 50;
    if query.is_empty() {
        return all.iter().take(CAP).cloned().collect();
    }
    let q = query.to_lowercase();
    let mut scored: Vec<(i32, &PathBuf)> = all
        .iter()
        .filter_map(|p| {
            let display = p.to_string_lossy().to_lowercase();
            fuzzy_score_path(&display, &q).map(|s| (s, p))
        })
        .collect();
    scored.sort_by_key(|&(s, _)| Reverse(s));
    scored
        .into_iter()
        .take(CAP)
        .map(|(_, p)| p.clone())
        .collect()
}

/// Shared query/list/selection/scroll state for a filesystem-browsing
/// picker.
///
/// Owns nothing filesystem-specific: callers (the two facades) are
/// responsible for walking the filesystem and handing the resulting entry
/// list to [`PathPickerCore::new`] / [`PathPickerCore::navigate_to`] /
/// [`PathPickerCore::set_all_entries`]. Fields are `pub(crate)` rather than
/// behind accessors because both facades (and their test modules) live in
/// sibling files under `compose::` and need direct read/write access the
/// same way the pre-extraction structs did.
pub(crate) struct PathPickerCore {
    /// Current browsing root.
    pub(crate) root: PathBuf,
    /// Live query string.
    pub(crate) query: String,
    /// All candidate entries relative to `root` (unfiltered, stable).
    pub(crate) all_entries: Vec<PathBuf>,
    /// Currently filtered + ranked entries (subset of `all_entries`).
    pub(crate) filtered: Vec<PathBuf>,
    /// Index into `filtered` of the highlighted row.
    pub(crate) selected: usize,
    /// First visible row index (scroll offset into `filtered`).
    pub(crate) scroll_top: usize,
}

impl PathPickerCore {
    /// Create a new core rooted at `root` with `all_entries` already
    /// walked by the caller.
    pub(crate) fn new(root: PathBuf, all_entries: Vec<PathBuf>) -> Self {
        let filtered = all_entries.iter().take(50).cloned().collect();
        Self {
            root,
            query: String::new(),
            all_entries,
            filtered,
            selected: 0,
            scroll_top: 0,
        }
    }

    /// Navigate to `new_root`: clear the query, replace `all_entries` with
    /// the caller's fresh walk, and reset selection/scroll.
    pub(crate) fn navigate_to(&mut self, new_root: PathBuf, all_entries: Vec<PathBuf>) {
        self.root = new_root;
        self.query.clear();
        self.all_entries = all_entries;
        self.filtered = self.all_entries.iter().take(50).cloned().collect();
        self.selected = 0;
        self.scroll_top = 0;
    }

    /// Replace `all_entries` in place (same `root`, same `query`) and
    /// refilter — used when a builder override (ignore dirs, max depth)
    /// changes what a re-walk of the *current* root surfaces.
    pub(crate) fn set_all_entries(&mut self, all_entries: Vec<PathBuf>) {
        self.all_entries = all_entries;
        self.refilter();
    }

    /// Append `c` to the query and refilter.
    pub(crate) fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.refilter();
    }

    /// Remove the last character from the query and refilter.
    pub(crate) fn pop_char(&mut self) {
        self.query.pop();
        self.refilter();
    }

    /// Re-compute `filtered` from `all_entries` and the current `query`.
    pub(crate) fn refilter(&mut self) {
        self.filtered = filter_entries(&self.all_entries, &self.query);
        self.selected = 0;
        self.scroll_top = 0;
    }

    /// Move the selection one row up (clamped at 0).
    pub(crate) fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Move the selection one row down (clamped at last row).
    pub(crate) fn move_down(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = (self.selected + 1).min(self.filtered.len() - 1);
        }
    }

    /// Clamp `scroll_top` so `selected` is always visible within
    /// `visible_rows` rows.
    pub(crate) fn sync_scroll(&mut self, visible_rows: usize) {
        if visible_rows == 0 {
            return;
        }
        if self.selected < self.scroll_top {
            self.scroll_top = self.selected;
        }
        if self.selected >= self.scroll_top + visible_rows {
            self.scroll_top = self.selected + 1 - visible_rows;
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Modifiers;

    // ── fuzzy_score_path ─────────────────────────────────────────────

    #[test]
    fn fuzzy_score_exact_match_scores_high() {
        let s = fuzzy_score_path("src/main.rs", "src/main.rs");
        assert!(s.is_some());
        assert!(s.unwrap() > 0);
    }

    #[test]
    fn fuzzy_score_subsequence_matches() {
        // "sm" is a subsequence of "src/main"
        let s = fuzzy_score_path("src/main", "sm");
        assert!(s.is_some());
    }

    #[test]
    fn fuzzy_score_non_subsequence_is_none() {
        let s = fuzzy_score_path("src/main", "xyz");
        assert!(s.is_none());
    }

    #[test]
    fn fuzzy_score_boundary_bonus() {
        // "m" starting at a "/" boundary should score higher than mid-word.
        let at_boundary = fuzzy_score_path("src/main", "m");
        let mid_word = fuzzy_score_path("abcmain", "m");
        assert!(at_boundary.unwrap() > mid_word.unwrap());
    }

    #[test]
    fn fuzzy_score_treats_backslash_as_boundary() {
        // On Windows, paths use `\\` as the separator. Both
        // `src\\main.rs`-style and `src/main.rs`-style paths should get
        // the same boundary bonus when matching at a directory boundary.
        let posix = fuzzy_score_path("src/main", "m");
        let windows = fuzzy_score_path("src\\main", "m");
        let mid = fuzzy_score_path("abcmain", "m");
        assert!(posix.unwrap() > mid.unwrap());
        assert!(windows.unwrap() > mid.unwrap());
        assert_eq!(posix.unwrap(), windows.unwrap());
    }

    #[test]
    fn fuzzy_score_kbs_matches_kube_system() {
        // Non-contiguous subsequence across a word boundary, the shape
        // both pickers must rank as a match.
        let s = fuzzy_score_path("kube-system", "kbs");
        assert!(
            s.is_some(),
            "\"kbs\" should subsequence-match \"kube-system\""
        );
    }

    // ── filter_entries ────────────────────────────────────────────────

    #[test]
    fn filter_returns_all_on_empty_query() {
        let paths: Vec<PathBuf> = (0..10).map(|i| PathBuf::from(format!("dir{i}"))).collect();
        let result = filter_entries(&paths, "");
        assert_eq!(result.len(), 10);
    }

    #[test]
    fn filter_caps_at_50() {
        let paths: Vec<PathBuf> = (0..100).map(|i| PathBuf::from(format!("dir{i}"))).collect();
        let result = filter_entries(&paths, "");
        assert_eq!(result.len(), 50);
    }

    #[test]
    fn filter_excludes_non_matching() {
        let paths = vec![
            PathBuf::from("src/main"),
            PathBuf::from("docs/readme"),
            PathBuf::from("tests/unit"),
        ];
        let result = filter_entries(&paths, "src");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], PathBuf::from("src/main"));
    }

    #[test]
    fn filter_sorts_by_score() {
        // "sr" matches both "src/main" (boundary) and "usr/local" (mid-word).
        let paths = vec![PathBuf::from("usr/local"), PathBuf::from("src/main")];
        let result = filter_entries(&paths, "sr");
        // "src/main" should rank higher (boundary bonus on 's').
        assert_eq!(result[0], PathBuf::from("src/main"));
    }

    // ── classify_nav_key ──────────────────────────────────────────────

    #[test]
    fn classify_escape_and_enter() {
        assert_eq!(
            classify_nav_key(&Key::Named(NamedKey::Escape), &Modifiers::default()),
            Some(NavKey::Escape)
        );
        assert_eq!(
            classify_nav_key(&Key::Named(NamedKey::Enter), &Modifiers::default()),
            Some(NavKey::Confirm)
        );
    }

    #[test]
    fn classify_vim_keys_map_to_navigation() {
        assert_eq!(
            classify_nav_key(&Key::Char('j'), &Modifiers::default()),
            Some(NavKey::Down)
        );
        assert_eq!(
            classify_nav_key(&Key::Char('k'), &Modifiers::default()),
            Some(NavKey::Up)
        );
        assert_eq!(
            classify_nav_key(&Key::Char('-'), &Modifiers::default()),
            Some(NavKey::NavigateUp)
        );
    }

    #[test]
    fn classify_cmd_modified_char_is_none() {
        let modifiers = Modifiers {
            cmd: true,
            ..Default::default()
        };
        assert_eq!(classify_nav_key(&Key::Char('j'), &modifiers), None);
    }

    #[test]
    fn classify_plain_char_falls_through_to_char() {
        assert_eq!(
            classify_nav_key(&Key::Char('x'), &Modifiers::default()),
            Some(NavKey::Char('x'))
        );
    }
}
