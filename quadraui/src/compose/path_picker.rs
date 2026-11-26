//! [`PathPickerState`] — the browsing model shared by
//! [`FolderPicker`] and [`FilePicker`], and the input handling both
//! pickers route through [`Palette`]'s own [`Interactive`].
//!
//! Both pickers browse a filesystem tree with the same query/list/
//! selection/scroll mechanics and the same key map; they differ only in
//! *how* they walk the filesystem (directories only vs. files + directories,
//! extension filters, the `"."` row), in what `Enter` resolves to, and in
//! whether the vim-style `-`/`j`/`k` shortcuts take priority over typing
//! them (off in [`FilePicker`]'s Save mode, where the query *is* the
//! destination filename). This module owns the shared half so the two
//! pickers stay in lock-step instead of drifting apart one small fix at a
//! time.
//!
//! [`FolderPicker`]: crate::compose::FolderPicker
//! [`FilePicker`]: crate::compose::FilePicker

use std::cmp::Reverse;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::behaviour::nav::{FlatNav, NavKey as FlatNavKey};
use crate::behaviour::scroll::ensure_visible;
use crate::compose::Handled;
use crate::interactive::{EventCx, Interactive, RoutedEvent, WidgetAction};
use crate::primitives::palette::{Palette, PaletteEvent, PalettePrimitiveLayout, PaletteState};
use crate::types::WidgetId;
use crate::{Key, NamedKey, UiEvent};

/// What a path picker reports: a confirmed path or a dismissal, plus the
/// browsing changes the host applies onto the picker's
/// [`PathPickerState`] (the pickers' `apply` does it).
///
/// Shared by [`crate::compose::FolderPicker`] and
/// [`crate::compose::FilePicker`], and by their controllers'
/// `handle_event`, which apply the browsing changes themselves and so
/// only ever report [`Self::Confirmed`] and [`Self::Cancelled`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PathPickerAction {
    /// The user confirmed a path — an existing entry for the folder
    /// picker and `FilePickerMode::Open`, or `root.join(query)` for
    /// `FilePickerMode::Save`.
    Confirmed {
        /// The resolved, absolute path.
        path: PathBuf,
    },
    /// The user dismissed the picker (Escape).
    Cancelled,
    /// The query text changed: the host sets
    /// [`PathPickerState::query`] and refilters.
    QueryChanged {
        /// The new query.
        query: String,
    },
    /// The highlight moved to row `idx` of [`PathPickerState::filtered`].
    SelectionChanged {
        /// The newly highlighted row.
        idx: usize,
    },
    /// The picker asked to browse `root` instead — the parent directory,
    /// or a directory row the user confirmed. The host re-walks the
    /// filesystem there.
    Navigated {
        /// The new browsing root.
        root: PathBuf,
    },
}

impl WidgetAction for PathPickerAction {
    /// Rides [`UiEvent::PathPicker`] under the picker's own id.
    fn into_ui_event(self, id: WidgetId) -> UiEvent {
        UiEvent::PathPicker(id, self)
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
/// score. Shared by both pickers so they rank matches identically.
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

/// Whether `entry` names a directory, preferring the file type the
/// directory enumeration already handed us over a second trip to the
/// filesystem.
///
/// Shared by both pickers' filesystem walks
/// ([`crate::compose::file_picker::walk_file_entries_recursive`] and
/// [`crate::compose::folder_picker::walk_dir_entries_recursive`]) so a
/// transient stat failure is handled identically in both, rather than one
/// walk getting the robustness fix below and the other silently dropping
/// or misclassifying the entry.
///
/// # Why not just `path.is_dir()`
///
/// [`Path::is_dir`] issues a *fresh* `stat`/`GetFileAttributes` per entry
/// and — because it returns `bool`, not `io::Result<bool>` — reports any
/// failure as plain `false`. A directory that momentarily can't be
/// stat'ed is therefore silently reclassified as *not a directory*: the
/// file picker then runs it through its extension filter (which may drop
/// the row entirely) and the folder picker drops it outright, either way
/// costing the user the ability to navigate through that directory.
/// Transient stat failures are routine on Windows, where an on-access
/// virus scanner holds a brief exclusive handle on freshly created files
/// and directories and unrelated opens come back
/// `ERROR_SHARING_VIOLATION`.
///
/// [`std::fs::DirEntry::file_type`] answers from the data the directory
/// read already returned, so on Windows (and Linux, and macOS) it costs
/// no syscall at all and cannot fail for that reason. Its one documented
/// gap is symlinks — it describes the *link*, never its target — so
/// those alone fall through to the `is_dir()` probe, which follows the
/// link the way both walks want.
pub(crate) fn dir_entry_is_dir(entry: &std::fs::DirEntry, path: &Path) -> bool {
    match entry.file_type() {
        Ok(ft) if !ft.is_symlink() => ft.is_dir(),
        _ => path.is_dir(),
    }
}

/// The browsing model a filesystem picker paints: root, query, the
/// walked entries, the filtered rows, the highlight and the scroll
/// offset.
///
/// Every field is visible state, so it lives on the picker descriptor
/// and the host owns it: a picker reports [`PathPickerAction`]s and the
/// host applies them (the pickers' own `apply` does it). Owns nothing
/// filesystem-specific — the pickers walk the filesystem and hand the
/// resulting entry list to [`Self::new`] / [`Self::navigate_to`] /
/// [`Self::set_all_entries`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathPickerState {
    /// Current browsing root.
    pub root: PathBuf,
    /// Live query string.
    pub query: String,
    /// All candidate entries relative to `root` (unfiltered, stable).
    pub all_entries: Vec<PathBuf>,
    /// Currently filtered + ranked entries (subset of `all_entries`).
    pub filtered: Vec<PathBuf>,
    /// Index into `filtered` of the highlighted row.
    pub selected: usize,
    /// First visible row index (scroll offset into `filtered`).
    pub scroll_top: usize,
}

impl PathPickerState {
    /// A model rooted at `root` with `all_entries` already walked by the
    /// caller.
    pub fn new(root: PathBuf, all_entries: Vec<PathBuf>) -> Self {
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
    pub fn navigate_to(&mut self, new_root: PathBuf, all_entries: Vec<PathBuf>) {
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
    pub fn set_all_entries(&mut self, all_entries: Vec<PathBuf>) {
        self.all_entries = all_entries;
        self.refilter();
    }

    /// Replace the query and refilter.
    pub fn set_query(&mut self, query: String) {
        self.query = query;
        self.refilter();
    }

    /// Append `c` to the query and refilter.
    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.refilter();
    }

    /// Remove the last character from the query and refilter.
    pub fn pop_char(&mut self) {
        self.query.pop();
        self.refilter();
    }

    /// Re-compute `filtered` from `all_entries` and the current `query`.
    pub fn refilter(&mut self) {
        self.filtered = filter_entries(&self.all_entries, &self.query);
        self.selected = 0;
        self.scroll_top = 0;
    }

    /// The row one step up from the highlight (clamped at 0).
    pub fn row_above(&self) -> usize {
        FlatNav::mv(
            self.filtered.len(),
            Some(self.selected),
            FlatNavKey::Up,
            false,
            |_| true,
        )
        .unwrap_or(self.selected)
    }

    /// The row one step down from the highlight (clamped at the last row).
    pub fn row_below(&self) -> usize {
        FlatNav::mv(
            self.filtered.len(),
            Some(self.selected),
            FlatNavKey::Down,
            false,
            |_| true,
        )
        .unwrap_or(self.selected)
    }

    /// Move the selection one row up (clamped at 0).
    pub fn move_up(&mut self) {
        self.selected = self.row_above();
    }

    /// Move the selection one row down (clamped at last row).
    pub fn move_down(&mut self) {
        self.selected = self.row_below();
    }

    /// Clamp `scroll_top` so `selected` is always visible within
    /// `visible_rows` rows.
    pub fn sync_scroll(&mut self, visible_rows: usize) {
        self.scroll_top = ensure_visible(self.scroll_top, visible_rows, self.selected);
    }

    /// Highlight row `idx` (clamped into `filtered`).
    pub fn select(&mut self, idx: usize) {
        self.selected = idx.min(self.filtered.len().saturating_sub(1));
    }

    /// The highlighted row, relative to `root`.
    pub fn highlighted(&self) -> Option<&PathBuf> {
        self.filtered.get(self.selected)
    }

    /// The root's parent, unless the root is a filesystem root.
    pub fn parent(&self) -> Option<PathBuf> {
        self.root.parent().map(Path::to_path_buf)
    }

    /// `root` for a title of `width` units, cut from the left with a
    /// leading `…` when it would not fit beside the palette's chrome.
    pub(crate) fn root_display(&self, width: f32) -> String {
        let r = self.root.to_string_lossy();
        // Reserve ~30 cells for palette chrome (borders + count chip +
        // padding).
        let max = (width as usize).saturating_sub(30).max(10);
        if r.len() <= max {
            return r.into_owned();
        }
        // `r.len()` is bytes, so drop leading chars until the remaining
        // suffix fits, splitting on a char boundary so a non-ASCII path
        // never panics.
        let s: &str = r.as_ref();
        let target_drop = s.len().saturating_sub(max);
        let split_at = s
            .char_indices()
            .map(|(idx, _)| idx)
            .find(|idx| *idx >= target_drop)
            .unwrap_or(s.len());
        format!("…{}", &s[split_at..])
    }
}

/// The query caret a picker paints: where the palette's own editing put
/// it, else the end of the query.
pub(crate) fn painted_caret(query: &str, state: Option<&PaletteState>) -> usize {
    state
        .and_then(|s| s.cursor)
        .unwrap_or(query.len())
        .min(query.len())
}

/// One routed event, resolved for a picker showing `paths` through
/// `palette`.
///
/// Escape cancels and Enter resolves through `confirm`; with `vim_keys`,
/// an unmodified `k`/`j`/`-` moves the highlight or browses up instead of
/// typing. Everything else is the palette's own input: its query editing
/// and list navigation, reported as the browsing change it names.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_picker_event(
    paths: &PathPickerState,
    palette: &Palette,
    state: &mut PaletteState,
    event: &RoutedEvent,
    layout: &PalettePrimitiveLayout,
    cx: &mut EventCx,
    vim_keys: bool,
    confirm: impl Fn() -> Handled<PathPickerAction>,
) -> Handled<PathPickerAction> {
    if let RoutedEvent::Key(key) = event {
        let modified = key.modifiers.ctrl || key.modifiers.cmd || key.modifiers.alt;
        match key.key {
            Key::Named(NamedKey::Escape) => return Handled::Event(PathPickerAction::Cancelled),
            Key::Named(NamedKey::Enter) => return confirm(),
            Key::Char('k') if vim_keys && !modified => return select(paths, paths.row_above()),
            Key::Char('j') if vim_keys && !modified => return select(paths, paths.row_below()),
            Key::Char('-') if vim_keys && !modified => {
                return match paths.parent() {
                    Some(root) => Handled::Event(PathPickerAction::Navigated { root }),
                    None => Handled::Consumed,
                };
            }
            _ if is_chorded_typing(key) => return Handled::Ignored,
            _ => {}
        }
    }
    match Interactive::handle(palette, state, event, layout, cx) {
        Handled::Event(PaletteEvent::QueryChanged { value }) => {
            Handled::Event(PathPickerAction::QueryChanged { query: value })
        }
        Handled::Event(PaletteEvent::SelectionChanged { idx }) => select(paths, idx),
        Handled::Event(PaletteEvent::ItemConfirmed { .. }) => confirm(),
        Handled::Event(PaletteEvent::Closed) => Handled::Event(PathPickerAction::Cancelled),
        Handled::Event(_) | Handled::Consumed => Handled::Consumed,
        Handled::Ignored => Handled::Ignored,
    }
}

/// Whether `key` is a Ctrl/Cmd-chorded character. Every backend delivers
/// a chorded letter as a plain `Key::Char` with the modifier set (the
/// macOS accelerator convention for Cmd), so a picker's query leaves it
/// to the app's accelerators rather than typing a literal or claiming it
/// as a clipboard chord.
pub(crate) fn is_chorded_typing(key: &crate::interactive::KeyEvent) -> bool {
    (key.modifiers.ctrl || key.modifiers.cmd) && matches!(key.key, Key::Char(_))
}

/// Report a move of the highlight to `idx`, or nothing when it is
/// already there.
fn select(paths: &PathPickerState, idx: usize) -> Handled<PathPickerAction> {
    if idx == paths.selected || paths.filtered.is_empty() {
        Handled::Consumed
    } else {
        Handled::Event(PathPickerAction::SelectionChanged { idx })
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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

    /// An empty query scores `Some(100)` rather than `None` — harmless
    /// only because [`filter_entries`]'s own empty-query fast path never
    /// calls [`fuzzy_score_path`] in that case. Pinned so a future change
    /// to either function notices if that invariant stops holding.
    #[test]
    fn fuzzy_score_empty_query_none() {
        let s = fuzzy_score_path("anything", "");
        assert!(s.is_some());
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

    // ── Selection movement ───────────────────────────────────────────

    fn core_with(entries: usize) -> PathPickerState {
        let all = (0..entries)
            .map(|i| PathBuf::from(format!("f{i}.rs")))
            .collect();
        PathPickerState::new(PathBuf::from("/root"), all)
    }

    #[test]
    fn move_down_walks_the_filtered_list_and_clamps_at_the_last_row() {
        let mut core = core_with(3);
        core.move_down();
        assert_eq!(core.selected, 1);
        for _ in 0..5 {
            core.move_down();
        }
        assert_eq!(core.selected, 2, "clamps instead of running off the end");
    }

    #[test]
    fn move_up_walks_back_and_clamps_at_the_first_row() {
        let mut core = core_with(3);
        core.move_down();
        core.move_down();
        core.move_up();
        assert_eq!(core.selected, 1);
        core.move_up();
        core.move_up();
        assert_eq!(core.selected, 0, "clamps instead of wrapping to the end");
    }

    #[test]
    fn moving_an_empty_filtered_list_leaves_the_selection_alone() {
        let mut core = core_with(0);
        core.move_down();
        assert_eq!(core.selected, 0);
        core.move_up();
        assert_eq!(core.selected, 0);
    }

    #[test]
    fn sync_scroll_pulls_the_selection_back_into_the_window() {
        let mut core = core_with(20);
        core.selected = 12;
        core.sync_scroll(5);
        assert_eq!(core.scroll_top, 8, "selection becomes the last visible row");
        core.selected = 3;
        core.sync_scroll(5);
        assert_eq!(
            core.scroll_top, 3,
            "selection becomes the first visible row"
        );
    }
}
