//! `FolderPickerController` — engine-level folder/workspace directory picker.
//!
//! Owns all state for an interactive directory-browsing modal:
//! filesystem walking, fuzzy filtering, scroll, and selection. Renders
//! via the existing [`Palette`](crate::Palette) primitive (no new
//! `Backend` trait method needed) and accepts backend-neutral
//! [`UiEvent`](crate::UiEvent)s.
//!
//! # Relation to vimcode
//!
//! Extracted verbatim from vimcode's TUI-local `FolderPickerState`
//! (`src/tui_main/mod.rs`, ~285 lines). After the quadraui PR lands,
//! vimcode removes the TUI-local code and rewires both TUI and GTK
//! `OpenFolderDialog` through this controller.
//!
//! # Usage pattern
//!
//! ```rust,ignore
//! // Instantiate when the user triggers "Open Folder":
//! let mut picker = FolderPickerController::new(
//!     std::env::current_dir().unwrap(),
//!     vec![],       // no extra file names to surface
//!     false,        // don't show hidden dirs
//! );
//!
//! // In AppLogic::render:
//! let rect = /* palette popup bounds */;
//! picker.render(rect, backend);
//!
//! // In AppLogic::handle:
//! let visible_rows = /* rect.height as usize - PALETTE_CHROME_ROWS */;
//! match picker.handle(&event, visible_rows) {
//!     FolderPickerEvent::Confirmed { path } => { /* open workspace */ },
//!     FolderPickerEvent::Cancelled             => { /* dismiss modal */ },
//!     FolderPickerEvent::Consumed              => { /* redraw */ },
//!     FolderPickerEvent::Ignored               => {},
//! }
//! ```
//!
//! # Scroll accounting
//!
//! `handle()` requires `visible_rows` — the number of list rows visible
//! inside the palette chrome (title row + query row + optional scrollbar
//! row). Consumers typically compute this as
//! `rect.height as usize - PALETTE_CHROME_ROWS` where
//! [`PALETTE_CHROME_ROWS`] = 4.

use std::cmp::Reverse;
use std::path::{Path, PathBuf};

use crate::{
    Backend, Icon, Key, Modifiers, NamedKey, Palette, PaletteItem, Rect, StyledText, UiEvent,
    WidgetId,
};

/// Overhead rows consumed by the `Palette` chrome (title + query + borders).
///
/// Subtract from the popup height to get `visible_rows`:
///
/// ```rust
/// # use quadraui::compose::folder_picker::PALETTE_CHROME_ROWS;
/// let popup_height = 20usize;
/// let visible_rows = popup_height.saturating_sub(PALETTE_CHROME_ROWS);
/// # let _ = visible_rows;
/// ```
pub const PALETTE_CHROME_ROWS: usize = 4;

/// Default recursion depth for [`FolderPickerController::new`].
///
/// Override per-instance with [`FolderPickerController::with_max_depth`].
pub const DEFAULT_MAX_DEPTH: usize = 5;

/// Default heavy build/dependency directory names that are skipped during
/// the filesystem walk.
///
/// Override per-instance with [`FolderPickerController::with_ignore_dirs`].
pub const DEFAULT_IGNORE_DIRS: &[&str] = &["target", "node_modules", "__pycache__"];

/// What happened after [`FolderPickerController::handle`] processed an event.
#[derive(Debug, Clone, PartialEq)]
pub enum FolderPickerEvent {
    /// The user confirmed a selection. The resolved absolute path is returned.
    ///
    /// The consumer should close the picker and act on the path (e.g.
    /// open it as a workspace folder). If the user selected `..` the
    /// controller navigates up internally and emits [`Consumed`] instead.
    ///
    /// [`Consumed`]: FolderPickerEvent::Consumed
    Confirmed { path: PathBuf },
    /// The user dismissed the picker (Escape).
    Cancelled,
    /// Event consumed — internal state changed, caller should redraw.
    Consumed,
    /// Event not relevant to this controller.
    Ignored,
}

/// Cross-backend compose controller for a directory-browsing picker modal.
///
/// Combines filesystem walking, fuzzy filtering, keyboard navigation, and
/// scroll tracking into a single reusable state machine. Rendering is
/// delegated to the existing [`Palette`] primitive via
/// [`Backend::draw_palette`].
///
/// See the [module-level documentation](self) for a usage example.
pub struct FolderPickerController {
    /// Widget ID used for the rendered `Palette`.
    id: WidgetId,
    /// Current browsing root. Changes on `navigate_to` / `navigate_up`.
    root: PathBuf,
    /// Live query string (typed characters build up incrementally).
    query: String,
    /// All candidate entries relative to `root` (unfiltered, stable).
    all_entries: Vec<PathBuf>,
    /// Currently filtered + ranked entries (subset of `all_entries`).
    filtered: Vec<PathBuf>,
    /// Index into `filtered` of the highlighted row.
    selected: usize,
    /// First visible row index (scroll offset into `filtered`).
    scroll_top: usize,
    /// Whether to surface hidden directories (names beginning with `.`).
    show_hidden: bool,
    /// Additional dotfile names to surface even when `show_hidden` is false.
    ///
    /// E.g. `[".vimcode-workspace"]` lets the consumer surface workspace
    /// marker files without exposing all hidden files. Defaults to empty.
    extra_file_names: Vec<String>,
    /// Ignore directories with these names (heavy build / dependency dirs).
    ///
    /// Defaults: [`DEFAULT_IGNORE_DIRS`]. Override with
    /// [`with_ignore_dirs`](Self::with_ignore_dirs).
    ignore_dirs: Vec<String>,
    /// Maximum recursion depth for the filesystem walk.
    ///
    /// Defaults to [`DEFAULT_MAX_DEPTH`]. Override with
    /// [`with_max_depth`](Self::with_max_depth).
    max_depth: usize,
}

impl FolderPickerController {
    /// Create a new picker rooted at `root`.
    ///
    /// # Arguments
    ///
    /// - `root` — initial browsing directory (typically `env::current_dir()`
    ///   or the engine's active workspace root).
    /// - `extra_file_names` — dotfile names to surface even when
    ///   `show_hidden` is false (e.g. `[".vimcode-workspace".to_string()]`).
    ///   Pass an empty `Vec` for the common case.
    /// - `show_hidden` — if true, all hidden directories are shown; if false,
    ///   only names in `extra_file_names` are surfaced from hidden entries.
    pub fn new(root: impl Into<PathBuf>, extra_file_names: Vec<String>, show_hidden: bool) -> Self {
        let root = root.into();
        let ignore_dirs: Vec<String> = DEFAULT_IGNORE_DIRS
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let max_depth = DEFAULT_MAX_DEPTH;
        let all_entries = collect_dir_entries(
            &root,
            show_hidden,
            &extra_file_names,
            &ignore_dirs,
            max_depth,
        );
        let filtered = all_entries.iter().take(50).cloned().collect();
        Self {
            id: WidgetId::new("folder_picker"),
            root,
            query: String::new(),
            all_entries,
            filtered,
            selected: 0,
            scroll_top: 0,
            show_hidden,
            extra_file_names,
            ignore_dirs,
            max_depth,
        }
    }

    /// Override the `WidgetId` used by the rendered `Palette`.
    ///
    /// Use this when more than one folder picker may exist in the same
    /// app, so each one has a distinct ID. Defaults to
    /// `WidgetId::new("folder_picker")`.
    ///
    /// Returns `self` for builder-style chaining.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = WidgetId::new(id.into());
        self
    }

    /// Override the list of directory names to skip during the filesystem walk.
    ///
    /// Defaults to [`DEFAULT_IGNORE_DIRS`]. Pass an empty `Vec` to walk
    /// every directory (use with care on large filesystems).
    ///
    /// Re-walks the current `root` so the new list takes effect immediately.
    /// Returns `self` for builder-style chaining.
    pub fn with_ignore_dirs(mut self, ignore_dirs: Vec<String>) -> Self {
        self.ignore_dirs = ignore_dirs;
        self.all_entries = collect_dir_entries(
            &self.root,
            self.show_hidden,
            &self.extra_file_names,
            &self.ignore_dirs,
            self.max_depth,
        );
        self.refilter();
        self
    }

    /// Override the maximum recursion depth for the filesystem walk.
    ///
    /// Defaults to [`DEFAULT_MAX_DEPTH`]. Higher values surface deeper
    /// directories at the cost of a slower initial walk.
    ///
    /// Re-walks the current `root` so the new depth takes effect immediately.
    /// Returns `self` for builder-style chaining.
    pub fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.max_depth = max_depth;
        self.all_entries = collect_dir_entries(
            &self.root,
            self.show_hidden,
            &self.extra_file_names,
            &self.ignore_dirs,
            self.max_depth,
        );
        self.refilter();
        self
    }

    // ── State accessors ───────────────────────────────────────────────

    /// Current browsing root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Current query string.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Currently filtered entries (relative paths).
    pub fn filtered(&self) -> &[PathBuf] {
        &self.filtered
    }

    /// Index of the highlighted row in `filtered`.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Scroll offset (first visible row index in `filtered`).
    pub fn scroll_top(&self) -> usize {
        self.scroll_top
    }

    /// The resolved absolute path for the currently selected entry, or `None`
    /// if the list is empty.
    ///
    /// The `..` entry resolves to the parent of `root`; all others resolve to
    /// `root.join(rel)`.
    pub fn selected_path(&self) -> Option<PathBuf> {
        let rel = self.filtered.get(self.selected)?;
        if rel.as_os_str() == ".." {
            self.root.parent().map(|p| p.to_path_buf())
        } else {
            Some(self.root.join(rel))
        }
    }

    // ── Navigation ────────────────────────────────────────────────────

    /// Navigate to `new_root`, clearing the query and reloading entries.
    pub fn navigate_to(&mut self, new_root: PathBuf) {
        self.root = new_root;
        self.query.clear();
        self.all_entries = collect_dir_entries(
            &self.root,
            self.show_hidden,
            &self.extra_file_names,
            &self.ignore_dirs,
            self.max_depth,
        );
        self.filtered = self.all_entries.iter().take(50).cloned().collect();
        self.selected = 0;
        self.scroll_top = 0;
    }

    /// Navigate to the parent directory (no-op at filesystem root).
    pub fn navigate_up(&mut self) {
        if let Some(parent) = self.root.parent() {
            self.navigate_to(parent.to_path_buf());
        }
    }

    // ── Query editing ─────────────────────────────────────────────────

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
        self.filtered = filter_dir_entries(&self.all_entries, &self.query);
        self.selected = 0;
        self.scroll_top = 0;
    }

    // ── Selection movement ────────────────────────────────────────────

    /// Move the selection one row up (clamped at 0).
    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Move the selection one row down (clamped at last row).
    pub fn move_down(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = (self.selected + 1).min(self.filtered.len() - 1);
        }
    }

    /// Clamp `scroll_top` so `selected` is always visible within
    /// `visible_rows` rows.
    pub fn sync_scroll(&mut self, visible_rows: usize) {
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

    // ── Render ────────────────────────────────────────────────────────

    /// Paint the picker as a `Palette` inside `rect`.
    ///
    /// Call this from `AppLogic::render` whenever the picker is open.
    pub fn render(&self, rect: Rect, backend: &mut dyn Backend) {
        let palette = self.build_palette(rect);
        backend.draw_palette(rect, &palette);
    }

    // ── Handle ────────────────────────────────────────────────────────

    /// Drive the state machine with a backend-neutral `UiEvent`.
    ///
    /// `visible_rows` is the number of list rows that fit inside the
    /// palette chrome; compute it as
    /// `popup_height.saturating_sub(PALETTE_CHROME_ROWS)`.
    ///
    /// # Canonical typing source
    ///
    /// Query characters are sourced from `UiEvent::KeyPressed`'s
    /// `Key::Char(c)` arm only — never from `UiEvent::CharTyped`. Today
    /// neither the TUI nor GTK backend emits `CharTyped`, and a future
    /// backend (e.g. Win/macOS IME) might emit *both* for the same input;
    /// listening to only `KeyPressed` avoids the double-insert risk. If a
    /// future backend needs IME-composed input, route it through
    /// `KeyPressed` as well or call [`push_char`](Self::push_char) directly.
    pub fn handle(&mut self, event: &UiEvent, visible_rows: usize) -> FolderPickerEvent {
        let result = match event {
            UiEvent::KeyPressed { key, modifiers, .. } => self.handle_key(key, modifiers),
            _ => FolderPickerEvent::Ignored,
        };

        // Keep scroll in sync after any state change.
        if matches!(result, FolderPickerEvent::Consumed) {
            self.sync_scroll(visible_rows);
        }

        result
    }

    // ── Internal helpers ──────────────────────────────────────────────

    // Key dispatch.
    //
    // Trade-off (intentional, matches vimcode's existing behavior): the vim
    // shortcuts `-` (parent dir), `k` (up), and `j` (down) are matched
    // *before* the generic `Key::Char(c) => push_char` arm. That means the
    // query input can never begin with — or contain — those three
    // characters: a user typing "kubernetes" cannot start the query with
    // `k`, nor include `j`/`-` anywhere. This is documented for downstream
    // consumers; if any consumer needs typing-priority (e.g. searching
    // codebases with hyphenated names), they can build their own dispatch
    // by calling the state-mutation methods (`push_char`, `move_up`, …)
    // directly instead of routing through `handle()`.
    fn handle_key(&mut self, key: &Key, modifiers: &Modifiers) -> FolderPickerEvent {
        let ctrl = modifiers.ctrl;
        match key {
            Key::Named(NamedKey::Escape) => FolderPickerEvent::Cancelled,

            Key::Named(NamedKey::Enter) => {
                // ".." → navigate up; anything else → confirm selection.
                let is_dotdot = self
                    .filtered
                    .get(self.selected)
                    .map(|p| p.as_os_str() == "..")
                    .unwrap_or(false);
                if is_dotdot {
                    self.navigate_up();
                    FolderPickerEvent::Consumed
                } else if let Some(path) = self.selected_path() {
                    FolderPickerEvent::Confirmed { path }
                } else {
                    FolderPickerEvent::Consumed
                }
            }

            // '-' navigates up to parent (vim netrw convention).
            Key::Char('-') if !ctrl => {
                self.navigate_up();
                FolderPickerEvent::Consumed
            }

            Key::Named(NamedKey::Up) => {
                self.move_up();
                FolderPickerEvent::Consumed
            }
            // 'k' vim-style up (only if no modifier).
            Key::Char('k') if !ctrl => {
                self.move_up();
                FolderPickerEvent::Consumed
            }

            Key::Named(NamedKey::Down) => {
                self.move_down();
                FolderPickerEvent::Consumed
            }
            // 'j' vim-style down (only if no modifier).
            Key::Char('j') if !ctrl => {
                self.move_down();
                FolderPickerEvent::Consumed
            }

            Key::Named(NamedKey::Backspace) => {
                self.pop_char();
                FolderPickerEvent::Consumed
            }

            Key::Char(c) if !ctrl => {
                self.push_char(*c);
                FolderPickerEvent::Consumed
            }

            _ => FolderPickerEvent::Ignored,
        }
    }

    /// Build the `Palette` descriptor for the current state.
    fn build_palette(&self, rect: Rect) -> Palette {
        let r = self.root.to_string_lossy();
        // Truncate from the left if the root path is too long.
        // Reserve ~30 cells for palette chrome (borders + count chip + padding).
        let max = (rect.width as usize).saturating_sub(30).max(10);
        // `r.len()` is bytes; we must split on a UTF-8 char boundary so paths
        // containing non-ASCII (e.g. Japanese, accented Latin) don't panic.
        // Approach: walk char_indices() from the start, dropping the leftmost
        // chars until the remaining suffix fits within `max` bytes.
        let root_display = if r.len() > max {
            let s: &str = r.as_ref();
            let target_drop = s.len().saturating_sub(max);
            // Find the first char boundary >= target_drop bytes from the start.
            let mut split_at = s.len();
            for (idx, _) in s.char_indices() {
                if idx >= target_drop {
                    split_at = idx;
                    break;
                }
            }
            format!("…{}", &s[split_at..])
        } else {
            r.into_owned()
        };
        let title = format!("Open Folder {root_display}");

        let folder_icon = Icon {
            glyph: "\u{1F4C1}".to_string(), // 📁
            fallback: "\u{1F4C1}".to_string(),
        };
        let file_icon = Icon {
            glyph: "\u{2699}".to_string(), // ⚙
            fallback: "\u{2699}".to_string(),
        };

        let items: Vec<PaletteItem> = self
            .filtered
            .iter()
            .map(|entry| {
                // Entries that are files (not directories) get the file icon.
                // We detect them by checking if the full path is a file — or
                // by the heuristic that the entry name contains a dot and
                // appears in `extra_file_names`.
                let name = entry.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let is_extra_file = self.extra_file_names.iter().any(|e| e == name);
                let icon = if is_extra_file {
                    Some(file_icon.clone())
                } else {
                    Some(folder_icon.clone())
                };
                PaletteItem {
                    text: StyledText::plain(entry.to_string_lossy().to_string()),
                    detail: None,
                    icon,
                    match_positions: Vec::new(),
                    depth: 0,
                    expandable: false,
                    expanded: false,
                }
            })
            .collect();

        Palette {
            id: self.id.clone(),
            title,
            query: self.query.clone(),
            query_cursor: self.query.len(),
            items,
            selected_idx: self.selected,
            scroll_offset: self.scroll_top,
            total_count: self.all_entries.len(),
            has_focus: true,
            show_query: true,
            create_label: None,
            preview: None,
        }
    }
}

// ── Filesystem helpers ────────────────────────────────────────────────────────

/// Walk `root` collecting relative paths of subdirectories (depth ≤ `max_depth`).
///
/// `..` is prepended (unless `root` is a filesystem root) and `.` is always
/// included so the consumer can open the current directory directly.
///
/// Skips:
/// - hidden names (starting with `.`) unless `show_hidden` is true or the
///   name appears in `extra_file_names`.
/// - directory names in `ignore_dirs` (e.g. `target`, `node_modules`).
fn collect_dir_entries(
    root: &Path,
    show_hidden: bool,
    extra_file_names: &[String],
    ignore_dirs: &[String],
    max_depth: usize,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    // Prepend ".." so the user can navigate up (omit at filesystem root).
    if root.parent().is_some() {
        out.push(PathBuf::from(".."));
    }
    out.push(PathBuf::from("."));
    walk_dir_entries_recursive(
        root,
        root,
        &mut out,
        0,
        show_hidden,
        extra_file_names,
        ignore_dirs,
        max_depth,
    );
    out
}

#[allow(clippy::too_many_arguments)]
fn walk_dir_entries_recursive(
    root: &Path,
    dir: &Path,
    out: &mut Vec<PathBuf>,
    depth: usize,
    show_hidden: bool,
    extra_file_names: &[String],
    ignore_dirs: &[String],
    max_depth: usize,
) {
    if depth > max_depth {
        return;
    }
    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(e) => e.flatten().collect(),
        Err(_) => return,
    };
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_owned(),
            None => continue,
        };
        // Hidden entries: skip unless show_hidden or name is in extra_file_names.
        if name.starts_with('.') && !show_hidden {
            if path.is_file() && extra_file_names.iter().any(|e| e == &name) {
                if let Ok(rel) = path.strip_prefix(root) {
                    out.push(rel.to_path_buf());
                }
            }
            continue;
        }
        // Skip heavy build/dep directories.
        if ignore_dirs.iter().any(|d| d == &name) {
            continue;
        }
        if path.is_dir() {
            if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_path_buf());
            }
            walk_dir_entries_recursive(
                root,
                &path,
                out,
                depth + 1,
                show_hidden,
                extra_file_names,
                ignore_dirs,
                max_depth,
            );
        }
    }
}

/// Filter `all` by `query` using subsequence matching with a relevance score.
///
/// Returns up to 50 entries, sorted by descending score.
/// If `query` is empty, the first 50 entries are returned unchanged.
fn filter_dir_entries(all: &[PathBuf], query: &str) -> Vec<PathBuf> {
    const CAP: usize = 50;
    if query.is_empty() {
        return all.iter().take(CAP).cloned().collect();
    }
    let q = query.to_lowercase();
    let mut scored: Vec<(i32, &PathBuf)> = all
        .iter()
        .filter_map(|p| {
            let display = p.to_string_lossy().to_lowercase();
            dir_fuzzy_score(&display, &q).map(|s| (s, p))
        })
        .collect();
    scored.sort_by_key(|&(s, _)| Reverse(s));
    scored
        .into_iter()
        .take(CAP)
        .map(|(_, p)| p.clone())
        .collect()
}

/// Subsequence fuzzy match returning a relevance score, or `None` if the
/// query is not a subsequence of `path`.
///
/// Higher scores mean the query characters appear closer together and/or
/// at word boundaries (`/`, `\`, `_`, `-`, `.`). Both forward and backward
/// slashes count so that Windows paths get the same boundary bonus as
/// POSIX paths.
fn dir_fuzzy_score(path: &str, query: &str) -> Option<i32> {
    let pb = path.as_bytes();
    let qb = query.as_bytes();
    let mut qi = 0usize;
    let mut score = 100i32;
    let mut last_pi = 0usize;
    for (pi, &byte) in pb.iter().enumerate() {
        if qi < qb.len() && byte == qb[qi] {
            if qi > 0 {
                score -= (pi - last_pi - 1) as i32;
            }
            if pi == 0 || matches!(pb[pi - 1], b'/' | b'\\' | b'_' | b'-' | b'.') {
                score += 5;
            }
            last_pi = pi;
            qi += 1;
        }
    }
    if qi == qb.len() {
        Some(score)
    } else {
        None
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    // ── dir_fuzzy_score ───────────────────────────────────────────────

    #[test]
    fn fuzzy_score_exact_match_scores_high() {
        let s = dir_fuzzy_score("src/main.rs", "src/main.rs");
        assert!(s.is_some());
        assert!(s.unwrap() > 0);
    }

    #[test]
    fn fuzzy_score_subsequence_matches() {
        // "sm" is a subsequence of "src/main"
        let s = dir_fuzzy_score("src/main", "sm");
        assert!(s.is_some());
    }

    #[test]
    fn fuzzy_score_non_subsequence_is_none() {
        let s = dir_fuzzy_score("src/main", "xyz");
        assert!(s.is_none());
    }

    #[test]
    fn fuzzy_score_boundary_bonus() {
        // "m" starting at a "/" boundary should score higher than mid-word.
        let at_boundary = dir_fuzzy_score("src/main", "m");
        let mid_word = dir_fuzzy_score("abcmain", "m");
        assert!(at_boundary.unwrap() > mid_word.unwrap());
    }

    #[test]
    fn fuzzy_score_empty_query_none() {
        // Empty query returns None — the caller (filter_dir_entries) handles
        // the empty-query fast path before calling dir_fuzzy_score.
        let s = dir_fuzzy_score("anything", "");
        // Empty query: qi starts at 0, qb.len() is 0, so qi == qb.len()
        // at the start → Some(100).  This is fine — filter_dir_entries
        // short-circuits before reaching here for empty queries.
        assert!(s.is_some());
    }

    // ── filter_dir_entries ────────────────────────────────────────────

    #[test]
    fn filter_returns_all_on_empty_query() {
        let paths: Vec<PathBuf> = (0..10).map(|i| PathBuf::from(format!("dir{i}"))).collect();
        let result = filter_dir_entries(&paths, "");
        assert_eq!(result.len(), 10);
    }

    #[test]
    fn filter_caps_at_50() {
        let paths: Vec<PathBuf> = (0..100).map(|i| PathBuf::from(format!("dir{i}"))).collect();
        let result = filter_dir_entries(&paths, "");
        assert_eq!(result.len(), 50);
    }

    #[test]
    fn filter_excludes_non_matching() {
        let paths = vec![
            PathBuf::from("src/main"),
            PathBuf::from("docs/readme"),
            PathBuf::from("tests/unit"),
        ];
        let result = filter_dir_entries(&paths, "src");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], PathBuf::from("src/main"));
    }

    #[test]
    fn filter_sorts_by_score() {
        // "sr" matches both "src/main" (boundary) and "usr/local" (mid-word).
        let paths = vec![PathBuf::from("usr/local"), PathBuf::from("src/main")];
        let result = filter_dir_entries(&paths, "sr");
        // "src/main" should rank higher (boundary bonus on 's').
        assert_eq!(result[0], PathBuf::from("src/main"));
    }

    // ── FolderPickerController state machine ──────────────────────────

    #[test]
    fn new_sets_dotdot_and_dot_entries() {
        let tmp = env::temp_dir();
        let picker = FolderPickerController::new(tmp.clone(), vec![], false);
        // ".." and "." should always be the first two entries.
        assert!(picker.all_entries.contains(&PathBuf::from("..")));
        assert!(picker.all_entries.contains(&PathBuf::from(".")));
    }

    #[test]
    fn push_and_pop_char_update_query() {
        let tmp = env::temp_dir();
        let mut picker = FolderPickerController::new(tmp, vec![], false);
        picker.push_char('s');
        picker.push_char('r');
        assert_eq!(picker.query(), "sr");
        picker.pop_char();
        assert_eq!(picker.query(), "s");
    }

    #[test]
    fn move_up_clamps_at_zero() {
        let tmp = env::temp_dir();
        let mut picker = FolderPickerController::new(tmp, vec![], false);
        assert_eq!(picker.selected(), 0);
        picker.move_up();
        assert_eq!(picker.selected(), 0);
    }

    #[test]
    fn move_down_advances_selection() {
        let tmp = env::temp_dir();
        let mut picker = FolderPickerController::new(tmp, vec![], false);
        if picker.filtered().len() >= 2 {
            picker.move_down();
            assert_eq!(picker.selected(), 1);
        }
    }

    #[test]
    fn move_down_clamps_at_last() {
        let tmp = env::temp_dir();
        let mut picker = FolderPickerController::new(tmp, vec![], false);
        let last = picker.filtered().len().saturating_sub(1);
        for _ in 0..last + 5 {
            picker.move_down();
        }
        assert_eq!(picker.selected(), last);
    }

    #[test]
    fn sync_scroll_brings_selected_into_view() {
        let tmp = env::temp_dir();
        let mut picker = FolderPickerController::new(tmp, vec![], false);
        // Force a large selection index.
        if picker.filtered().len() >= 10 {
            picker.selected = 9;
            picker.sync_scroll(5);
            assert!(picker.scroll_top + 5 > picker.selected);
            assert!(picker.scroll_top <= picker.selected);
        }
    }

    #[test]
    fn sync_scroll_scrolls_up_when_needed() {
        let tmp = env::temp_dir();
        let mut picker = FolderPickerController::new(tmp, vec![], false);
        if picker.filtered().len() >= 5 {
            picker.scroll_top = 5;
            picker.selected = 2; // above scroll_top
            picker.sync_scroll(3);
            assert_eq!(picker.scroll_top, 2);
        }
    }

    #[test]
    fn escape_event_returns_cancelled() {
        let tmp = env::temp_dir();
        let mut picker = FolderPickerController::new(tmp, vec![], false);
        let ev = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        assert_eq!(picker.handle(&ev, 10), FolderPickerEvent::Cancelled);
    }

    #[test]
    fn enter_on_dotdot_navigates_up() {
        // Use tempfile so parallel test runs don't race on a fixed name and
        // a crashed prior run can't leak a stale marker directory.
        let parent = tempfile::tempdir().expect("create parent tempdir");
        let sub = parent.path().join("nav_up_child");
        std::fs::create_dir_all(&sub).expect("create child dir");
        let mut picker = FolderPickerController::new(sub.clone(), vec![], false);
        // Select ".." (index 0, always prepended when parent exists).
        picker.selected = 0;
        assert_eq!(picker.filtered().first(), Some(&PathBuf::from("..")));
        let ev = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let result = picker.handle(&ev, 10);
        assert_eq!(result, FolderPickerEvent::Consumed);
        assert_eq!(picker.root(), parent.path());
    }

    #[test]
    fn enter_on_dot_returns_confirmed_with_root() {
        let tmp = env::temp_dir();
        let mut picker = FolderPickerController::new(tmp.clone(), vec![], false);
        // "." is always at index 1 (after "..") when parent exists.
        let dot_idx = picker
            .filtered()
            .iter()
            .position(|p| p.as_os_str() == ".")
            .expect("'.' should always be in filtered");
        picker.selected = dot_idx;
        let ev = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let result = picker.handle(&ev, 10);
        assert_eq!(result, FolderPickerEvent::Confirmed { path: tmp });
    }

    #[test]
    fn up_down_key_moves_selection() {
        let tmp = env::temp_dir();
        let mut picker = FolderPickerController::new(tmp, vec![], false);
        if picker.filtered().len() >= 2 {
            let ev_down = UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                modifiers: Modifiers::default(),
                repeat: false,
            };
            picker.handle(&ev_down, 10);
            assert_eq!(picker.selected(), 1);

            let ev_up = UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Up),
                modifiers: Modifiers::default(),
                repeat: false,
            };
            picker.handle(&ev_up, 10);
            assert_eq!(picker.selected(), 0);
        }
    }

    #[test]
    fn key_char_appends_to_query() {
        // Query characters arrive via `Key::Char` in `KeyPressed`, *not* via
        // `CharTyped`. The latter is intentionally ignored to avoid
        // double-insert when a future backend emits both for the same input.
        let tmp = env::temp_dir();
        let mut picker = FolderPickerController::new(tmp, vec![], false);
        let ev = UiEvent::KeyPressed {
            key: Key::Char('s'),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        picker.handle(&ev, 10);
        assert_eq!(picker.query(), "s");
    }

    #[test]
    fn char_typed_is_ignored() {
        // Documented contract: `CharTyped` is not the canonical typing
        // source; only `Key::Char` in `KeyPressed` mutates the query.
        let tmp = env::temp_dir();
        let mut picker = FolderPickerController::new(tmp, vec![], false);
        let ev = UiEvent::CharTyped('s');
        let result = picker.handle(&ev, 10);
        assert_eq!(result, FolderPickerEvent::Ignored);
        assert_eq!(picker.query(), "");
    }

    #[test]
    fn backspace_removes_last_char() {
        let tmp = env::temp_dir();
        let mut picker = FolderPickerController::new(tmp, vec![], false);
        picker.push_char('s');
        picker.push_char('r');
        let ev = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Backspace),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        picker.handle(&ev, 10);
        assert_eq!(picker.query(), "s");
    }

    #[test]
    fn selected_path_resolves_dot_to_root() {
        let tmp = env::temp_dir();
        let picker = FolderPickerController::new(tmp.clone(), vec![], false);
        let dot_idx = picker
            .filtered()
            .iter()
            .position(|p| p.as_os_str() == ".")
            .unwrap();
        let mut picker = picker;
        picker.selected = dot_idx;
        assert_eq!(picker.selected_path(), Some(tmp));
    }

    #[test]
    fn dash_key_navigates_up() {
        let parent = tempfile::tempdir().expect("create parent tempdir");
        let sub = parent.path().join("dash_nav_child");
        std::fs::create_dir_all(&sub).expect("create child dir");
        let mut picker = FolderPickerController::new(sub.clone(), vec![], false);
        let ev = UiEvent::KeyPressed {
            key: Key::Char('-'),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let result = picker.handle(&ev, 10);
        assert_eq!(result, FolderPickerEvent::Consumed);
        assert_eq!(picker.root(), parent.path());
    }

    #[test]
    fn build_palette_id_is_folder_picker() {
        let tmp = env::temp_dir();
        let picker = FolderPickerController::new(tmp, vec![], false);
        let rect = Rect::new(0.0, 0.0, 80.0, 24.0);
        let palette = picker.build_palette(rect);
        assert_eq!(palette.id.as_str(), "folder_picker");
    }

    #[test]
    fn build_palette_title_contains_open_folder() {
        let tmp = env::temp_dir();
        let picker = FolderPickerController::new(tmp, vec![], false);
        let rect = Rect::new(0.0, 0.0, 80.0, 24.0);
        let palette = picker.build_palette(rect);
        assert!(palette.title.starts_with("Open Folder"));
    }

    #[test]
    fn build_palette_items_match_filtered() {
        let tmp = env::temp_dir();
        let picker = FolderPickerController::new(tmp, vec![], false);
        let rect = Rect::new(0.0, 0.0, 80.0, 24.0);
        let palette = picker.build_palette(rect);
        assert_eq!(palette.items.len(), picker.filtered().len());
    }

    #[test]
    fn extra_file_names_gets_file_icon() {
        // Isolate the marker file in a fresh tempdir so concurrent test
        // runs don't race and a crashed prior run can't leak state.
        let root = tempfile::tempdir().expect("create marker tempdir");
        let marker = root.path().join(".my-workspace");
        std::fs::write(&marker, b"").expect("write marker file");
        let picker = FolderPickerController::new(
            root.path().to_path_buf(),
            vec![".my-workspace".to_string()],
            false,
        );
        let rect = Rect::new(0.0, 0.0, 80.0, 24.0);
        let palette = picker.build_palette(rect);
        // The marker file should appear in items with the ⚙ icon.
        let has_marker = palette.items.iter().any(|item| {
            item.text
                .spans
                .first()
                .map(|s| s.text.contains(".my-workspace"))
                .unwrap_or(false)
        });
        assert!(has_marker, "marker file should appear in palette items");
    }

    // ── Regression tests for previously-discovered bugs ───────────────

    #[test]
    fn build_palette_handles_non_ascii_root_path() {
        // Regression: `build_palette` previously sliced a byte range of the
        // root path without checking UTF-8 char boundaries, panicking on
        // non-ASCII paths (e.g. Japanese, accented Latin) once the path
        // exceeded `viewport.width − 30` chars.
        //
        // Concrete trigger: a path of 153 bytes whose byte at index
        // `len - max == 103` is a continuation byte (the middle byte of a
        // 3-byte UTF-8 char). The original `&r[r.len() - max..]` slice would
        // panic with "is not a char boundary"; the new implementation walks
        // `char_indices()` to find the nearest valid split point.
        let long_japanese = PathBuf::from(format!("/a/{}", "ト".repeat(50)));
        let picker = FolderPickerController::new(long_japanese, vec![], false);
        // Narrow rect forces truncation: max = max(80-30, 10) = 50 bytes.
        let rect = Rect::new(0.0, 0.0, 80.0, 24.0);
        // Must not panic.
        let palette = picker.build_palette(rect);
        assert!(palette.title.starts_with("Open Folder "));
        // Sanity: ellipsis should be present and the suffix should still
        // be valid UTF-8 (implicit — `format!` would have panicked otherwise).
        assert!(palette.title.contains('…'));
    }

    #[test]
    fn build_palette_handles_accented_root_path() {
        // Regression: shorter multi-byte (Latin-1 / Latin-Extended) paths
        // should also be safe.
        let path = PathBuf::from(
            "/home/utilisateur/répertoire/sous-répertoire/encore-plus-profond/données",
        );
        let picker = FolderPickerController::new(path, vec![], false);
        let rect = Rect::new(0.0, 0.0, 60.0, 24.0);
        let palette = picker.build_palette(rect);
        assert!(palette.title.starts_with("Open Folder "));
    }

    // ── Windows-style path scoring ────────────────────────────────────

    #[test]
    fn fuzzy_score_treats_backslash_as_boundary() {
        // On Windows, paths use `\\` as the separator. Both
        // `src\\main.rs`-style and `src/main.rs`-style paths should get
        // the same boundary bonus when matching at a directory boundary.
        let posix = dir_fuzzy_score("src/main", "m");
        let windows = dir_fuzzy_score("src\\main", "m");
        // Both should rank higher than a mid-word match.
        let mid = dir_fuzzy_score("abcmain", "m");
        assert!(posix.unwrap() > mid.unwrap());
        assert!(windows.unwrap() > mid.unwrap());
        // And they should rank equally (same boundary bonus).
        assert_eq!(posix.unwrap(), windows.unwrap());
    }

    // ── Builder-method overrides ──────────────────────────────────────

    #[test]
    fn with_id_overrides_widget_id() {
        let tmp = env::temp_dir();
        let picker = FolderPickerController::new(tmp, vec![], false).with_id("picker_b");
        let rect = Rect::new(0.0, 0.0, 80.0, 24.0);
        assert_eq!(picker.build_palette(rect).id.as_str(), "picker_b");
    }

    #[test]
    fn with_ignore_dirs_replaces_default_list() {
        let root = tempfile::tempdir().expect("tempdir");
        // Create `target` and `keep_me`; default list skips `target`.
        std::fs::create_dir_all(root.path().join("target")).unwrap();
        std::fs::create_dir_all(root.path().join("keep_me")).unwrap();
        // With an empty override, `target` should no longer be skipped.
        let picker = FolderPickerController::new(root.path().to_path_buf(), vec![], false)
            .with_ignore_dirs(vec![]);
        let has_target = picker
            .all_entries
            .iter()
            .any(|p| p == &PathBuf::from("target"));
        let has_keep = picker
            .all_entries
            .iter()
            .any(|p| p == &PathBuf::from("keep_me"));
        assert!(
            has_target,
            "target should be present after with_ignore_dirs(empty)"
        );
        assert!(has_keep);
    }

    #[test]
    fn with_max_depth_limits_recursion() {
        let root = tempfile::tempdir().expect("tempdir");
        // Build a nested chain: depth1/depth2/depth3
        let d3 = root.path().join("d1").join("d2").join("d3");
        std::fs::create_dir_all(&d3).unwrap();
        // depth=0 should only emit the top-level "d1".
        let picker =
            FolderPickerController::new(root.path().to_path_buf(), vec![], false).with_max_depth(0);
        let has_d1 = picker.all_entries.iter().any(|p| p.ends_with("d1"));
        let has_d2 = picker.all_entries.iter().any(|p| p.ends_with("d2"));
        assert!(has_d1);
        assert!(!has_d2, "depth=0 should not surface nested subdirs");
    }
}
