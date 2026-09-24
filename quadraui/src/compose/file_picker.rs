//! `FilePickerController` — engine-level file open/save picker.
//!
//! Modelled directly on [`crate::compose::FolderPickerController`] (same
//! fuzzy-filter + key-routing shape, rendered via the same [`Palette`]
//! primitive) — but browses **files**, not just directories, and adds a
//! save-mode where the typed query doubles as the destination filename.
//!
//! # Relation to issue #965
//!
//! Before this controller existed, `compose/` had a folder picker but no
//! file picker, so `TuiPlatformServices::show_file_open_dialog` /
//! `show_file_save_dialog` had nothing to degrade to and returned `None`
//! unconditionally. `crate::tui::services` now drives this controller
//! through a nested draw-and-read loop (see that module's doc) so both
//! methods return a real path on TUI too — see
//! `examples/tui_file_picker.rs` for the same controller used directly
//! by an app (no `PlatformServices` involved), and `crate::tui::services`'s
//! own tests for the `PlatformServices`-level round trip.
//!
//! # Deliberately drops the folder picker's vim navigation keys
//!
//! [`FolderPickerController`] intercepts `-`/`j`/`k` as parent-dir/up/down
//! shortcuts *before* the generic typing arm — a documented trade-off
//! there because directory names rarely start with those characters.
//! Real filenames routinely do (`my-notes.txt`, `journal.md`), especially
//! in save mode where the query *is* the destination name, so this
//! controller does not repeat that trade-off: only the named `Up`/`Down`
//! arrow keys navigate the list, and every printable character reaches
//! the query/filename field unconditionally.
//!
//! [`FolderPickerController`]: crate::compose::FolderPickerController
//!
//! # Save-mode semantics
//!
//! In [`FilePickerMode::Save`], `query` is simultaneously the fuzzy
//! filter over existing entries (so the user can see what they'd
//! overwrite) *and* the destination filename. Enter on a directory row
//! navigates into it (clearing `query`, matching Open mode); Enter
//! otherwise confirms `root.join(query)` regardless of whether an entry
//! with that name currently exists — the standard "Save As" contract.
//! In [`FilePickerMode::Open`], Enter only ever confirms an existing,
//! currently-highlighted **file** row; there is nothing to "create".
//!
//! # Usage pattern
//!
//! ```rust,ignore
//! let mut picker = FilePickerController::new(
//!     FilePickerMode::Open,
//!     std::env::current_dir().unwrap(),
//!     vec![("Rust".to_string(), vec!["rs".to_string()])],
//! );
//!
//! // In AppLogic::render:
//! picker.render(popup_rect, backend);
//!
//! // In AppLogic::handle:
//! let visible_rows = popup_rect.height as usize - PALETTE_CHROME_ROWS;
//! match picker.handle(&event, visible_rows) {
//!     FilePickerEvent::Confirmed { path } => { /* open/save `path` */ }
//!     FilePickerEvent::Cancelled           => { /* dismiss modal */ }
//!     FilePickerEvent::Consumed            => { /* redraw */ }
//!     FilePickerEvent::Ignored             => {}
//! }
//! ```

use std::cmp::Reverse;
use std::path::{Path, PathBuf};

use crate::compose::folder_picker::{DEFAULT_IGNORE_DIRS, DEFAULT_MAX_DEPTH};
use crate::{
    Backend, Icon, Key, Modifiers, NamedKey, Palette, PaletteItem, Rect, StyledText, UiEvent,
    WidgetId,
};

/// Whether a [`FilePickerController`] is choosing a file to open (must
/// exist) or a destination to save to (may not exist yet).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilePickerMode {
    /// Enter only confirms an existing, highlighted file.
    Open,
    /// Enter confirms `root.join(query)`, whether or not that name
    /// already exists — the query field doubles as the filename.
    Save,
}

/// What happened after [`FilePickerController::handle`] processed an event.
#[derive(Debug, Clone, PartialEq)]
pub enum FilePickerEvent {
    /// The user confirmed a file path — an existing file in
    /// [`FilePickerMode::Open`], or `root.join(query)` in
    /// [`FilePickerMode::Save`].
    Confirmed { path: PathBuf },
    /// The user dismissed the picker (Escape).
    Cancelled,
    /// Event consumed — internal state changed, caller should redraw.
    Consumed,
    /// Event not relevant to this controller.
    Ignored,
}

/// Cross-backend compose controller for a file open/save picker modal.
///
/// See the [module-level documentation](self) for a usage example.
pub struct FilePickerController {
    id: WidgetId,
    mode: FilePickerMode,
    /// Current browsing root. Changes on `navigate_to`.
    root: PathBuf,
    /// Live query string — fuzzy filter (`Open`) and/or destination
    /// filename (`Save`).
    query: String,
    /// Extension filters — `(display_name, &[ext])` pairs, mirroring
    /// [`crate::backend::FileDialogOptions::filters`]. Empty means "show
    /// every file". Directories are never filtered out.
    filters: Vec<(String, Vec<String>)>,
    /// All candidate entries relative to `root` (unfiltered, stable) —
    /// directories and files both.
    all_entries: Vec<PathBuf>,
    /// Currently filtered + ranked entries (subset of `all_entries`).
    filtered: Vec<PathBuf>,
    selected: usize,
    scroll_top: usize,
    show_hidden: bool,
    ignore_dirs: Vec<String>,
    max_depth: usize,
}

impl FilePickerController {
    /// Create a new picker rooted at `root`.
    ///
    /// `filters` restricts which **files** are listed (directories are
    /// always shown, so the user can navigate through them regardless of
    /// filter) — `(display_name, extensions)` pairs; an empty `Vec`
    /// shows every file.
    pub fn new(
        mode: FilePickerMode,
        root: impl Into<PathBuf>,
        filters: Vec<(String, Vec<String>)>,
    ) -> Self {
        let root = root.into();
        let ignore_dirs: Vec<String> = DEFAULT_IGNORE_DIRS
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let max_depth = DEFAULT_MAX_DEPTH;
        let show_hidden = false;
        let all_entries =
            collect_file_entries(&root, show_hidden, &ignore_dirs, &filters, max_depth);
        let filtered = all_entries.iter().take(50).cloned().collect();
        Self {
            id: WidgetId::new("file_picker"),
            mode,
            root,
            query: String::new(),
            filters,
            all_entries,
            filtered,
            selected: 0,
            scroll_top: 0,
            show_hidden,
            ignore_dirs,
            max_depth,
        }
    }

    /// Seed [`Self::query`] (the Save-mode destination filename) with an
    /// initial value, mirroring
    /// [`crate::backend::FileDialogOptions::initial_filename`]. Returns
    /// `self` for builder-style chaining.
    pub fn with_initial_filename(mut self, name: impl Into<String>) -> Self {
        self.query = name.into();
        self
    }

    /// Override the `WidgetId` used by the rendered `Palette`. Defaults
    /// to `WidgetId::new("file_picker")`. Returns `self` for
    /// builder-style chaining.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = WidgetId::new(id.into());
        self
    }

    // ── State accessors ───────────────────────────────────────────────

    pub fn mode(&self) -> FilePickerMode {
        self.mode
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn filtered(&self) -> &[PathBuf] {
        &self.filtered
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn scroll_top(&self) -> usize {
        self.scroll_top
    }

    fn is_dir_entry(&self, rel: &Path) -> bool {
        rel.as_os_str() == ".." || self.root.join(rel).is_dir()
    }

    // ── Navigation ────────────────────────────────────────────────────

    /// Navigate to `new_root`, clearing the query and reloading entries.
    pub fn navigate_to(&mut self, new_root: PathBuf) {
        self.root = new_root;
        self.query.clear();
        self.all_entries = collect_file_entries(
            &self.root,
            self.show_hidden,
            &self.ignore_dirs,
            &self.filters,
            self.max_depth,
        );
        self.filtered = self.all_entries.iter().take(50).cloned().collect();
        self.selected = 0;
        self.scroll_top = 0;
    }

    // ── Query editing ─────────────────────────────────────────────────

    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.refilter();
    }

    pub fn pop_char(&mut self) {
        self.query.pop();
        self.refilter();
    }

    fn refilter(&mut self) {
        self.filtered = filter_file_entries(&self.all_entries, &self.query);
        self.selected = 0;
        self.scroll_top = 0;
    }

    // ── Selection movement ────────────────────────────────────────────

    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn move_down(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = (self.selected + 1).min(self.filtered.len() - 1);
        }
    }

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
    pub fn render(&self, rect: Rect, backend: &mut dyn Backend) {
        let palette = self.build_palette(rect);
        backend.draw_palette(rect, &palette);
    }

    // ── Handle ────────────────────────────────────────────────────────

    /// Drive the state machine with a backend-neutral `UiEvent`.
    ///
    /// `visible_rows` is the number of list rows that fit inside the
    /// palette chrome; compute it as
    /// `popup_height.saturating_sub(PALETTE_CHROME_ROWS)` (see
    /// [`crate::compose::folder_picker::PALETTE_CHROME_ROWS`]).
    pub fn handle(&mut self, event: &UiEvent, visible_rows: usize) -> FilePickerEvent {
        let result = match event {
            UiEvent::KeyPressed { key, modifiers, .. } => self.handle_key(key, modifiers),
            _ => FilePickerEvent::Ignored,
        };
        if matches!(result, FilePickerEvent::Consumed) {
            self.sync_scroll(visible_rows);
        }
        result
    }

    fn handle_key(&mut self, key: &Key, modifiers: &Modifiers) -> FilePickerEvent {
        let ctrl = modifiers.ctrl;
        match key {
            Key::Named(NamedKey::Escape) => FilePickerEvent::Cancelled,

            Key::Named(NamedKey::Enter) => self.confirm_selection(),

            Key::Named(NamedKey::Up) => {
                self.move_up();
                FilePickerEvent::Consumed
            }
            Key::Named(NamedKey::Down) => {
                self.move_down();
                FilePickerEvent::Consumed
            }
            Key::Named(NamedKey::Backspace) => {
                self.pop_char();
                FilePickerEvent::Consumed
            }
            Key::Char(c) if !ctrl => {
                self.push_char(*c);
                FilePickerEvent::Consumed
            }
            _ => FilePickerEvent::Ignored,
        }
    }

    /// Enter's resolution logic — shared by [`Self::handle_key`], split
    /// out so it stays unit-testable without constructing a `UiEvent`.
    ///
    /// The ".." row needs special-casing beyond "it's a directory": it's
    /// the row `selected` defaults to (index 0) whenever the entry list
    /// hasn't been touched, so treating it exactly like any other
    /// directory would mean a Save dialog opened with a pre-filled
    /// [`Self::with_initial_filename`] and confirmed with an *immediate*
    /// Enter — no arrowing, no typing — would silently navigate up a
    /// directory instead of saving. A typed/seeded Save-mode filename
    /// takes priority over that unmoved default; explicitly arrowing
    /// onto a *real* directory row (handled below, not here) still
    /// navigates regardless of mode or query, matching native save
    /// dialogs' "Enter on a folder row browses" convention.
    fn confirm_selection(&mut self) -> FilePickerEvent {
        let highlighted = self.filtered.get(self.selected).cloned();
        let highlighted_is_dotdot = highlighted
            .as_deref()
            .map(|p| p.as_os_str() == "..")
            .unwrap_or(false);

        if highlighted_is_dotdot {
            if self.mode == FilePickerMode::Save && !self.query.is_empty() {
                return FilePickerEvent::Confirmed {
                    path: self.root.join(&self.query),
                };
            }
            if let Some(parent) = self.root.parent() {
                self.navigate_to(parent.to_path_buf());
            }
            return FilePickerEvent::Consumed;
        }

        if let Some(rel) = &highlighted {
            if self.is_dir_entry(rel) {
                self.navigate_to(self.root.join(rel));
                return FilePickerEvent::Consumed;
            }
        }

        match self.mode {
            FilePickerMode::Open => match highlighted {
                Some(rel) => FilePickerEvent::Confirmed {
                    path: self.root.join(rel),
                },
                None => FilePickerEvent::Ignored,
            },
            FilePickerMode::Save => {
                if self.query.is_empty() {
                    FilePickerEvent::Ignored
                } else {
                    FilePickerEvent::Confirmed {
                        path: self.root.join(&self.query),
                    }
                }
            }
        }
    }

    /// Build the `Palette` descriptor for the current state.
    fn build_palette(&self, rect: Rect) -> Palette {
        let r = self.root.to_string_lossy();
        let max = (rect.width as usize).saturating_sub(30).max(10);
        let root_display = if r.len() > max {
            let s: &str = r.as_ref();
            let target_drop = s.len().saturating_sub(max);
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
        let verb = match self.mode {
            FilePickerMode::Open => "Open File",
            FilePickerMode::Save => "Save File",
        };
        let title = format!("{verb} {root_display}");

        let folder_icon = Icon::new("\u{1F4C1}", "\u{1F4C1}"); // 📁
        let file_icon = Icon::new("\u{1F4C4}", "\u{1F4C4}"); // 📄

        let items: Vec<PaletteItem> = self
            .filtered
            .iter()
            .map(|entry| {
                let icon = if self.is_dir_entry(entry) {
                    Some(folder_icon.clone())
                } else {
                    Some(file_icon.clone())
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

        let create_label = match self.mode {
            FilePickerMode::Save if !self.query.is_empty() => {
                Some(format!("Save as \"{}\"", self.query))
            }
            _ => None,
        };

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
            create_label,
            preview: None,
            mode: crate::primitives::palette::PaletteMode::List,
        }
    }
}

// ── Filesystem helpers ────────────────────────────────────────────────────────

/// Walk `root` collecting relative paths of both subdirectories and
/// files (depth ≤ `max_depth`), filtering files by `filters`.
///
/// `..` is prepended (unless `root` is a filesystem root). Unlike
/// [`crate::compose::folder_picker::collect_dir_entries`], `.` is not
/// included — selecting the current directory itself is meaningless for
/// a *file* picker.
fn collect_file_entries(
    root: &Path,
    show_hidden: bool,
    ignore_dirs: &[String],
    filters: &[(String, Vec<String>)],
    max_depth: usize,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if root.parent().is_some() {
        out.push(PathBuf::from(".."));
    }
    walk_file_entries_recursive(
        root,
        root,
        &mut out,
        0,
        show_hidden,
        ignore_dirs,
        filters,
        max_depth,
    );
    out
}

#[allow(clippy::too_many_arguments)]
fn walk_file_entries_recursive(
    root: &Path,
    dir: &Path,
    out: &mut Vec<PathBuf>,
    depth: usize,
    show_hidden: bool,
    ignore_dirs: &[String],
    filters: &[(String, Vec<String>)],
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
        if name.starts_with('.') && !show_hidden {
            continue;
        }
        if dir_entry_is_dir(&entry, &path) {
            if ignore_dirs.iter().any(|d| d == &name) {
                continue;
            }
            if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_path_buf());
            }
            walk_file_entries_recursive(
                root,
                &path,
                out,
                depth + 1,
                show_hidden,
                ignore_dirs,
                filters,
                max_depth,
            );
        } else if extension_matches(filters, &name) {
            if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_path_buf());
            }
        }
    }
}

/// Whether `entry` names a directory, preferring the file type the
/// directory enumeration already handed us over a second trip to the
/// filesystem.
///
/// # Why not just `path.is_dir()`
///
/// [`Path::is_dir`] issues a *fresh* `stat`/`GetFileAttributes` per
/// entry and — because it returns `bool`, not `io::Result<bool>` —
/// reports any failure as plain `false`. A directory that momentarily
/// can't be stat'ed is therefore silently reclassified as a *file*,
/// which is not a harmless downgrade here: files run the
/// [`extension_matches`] gauntlet, so an active extension filter drops
/// the row entirely and the user loses the ability to navigate through
/// that directory (the exact invariant
/// `filters_restrict_files_not_dirs` pins). Transient stat failures are
/// routine on Windows, where an on-access virus scanner holds a brief
/// exclusive handle on freshly created files and directories and
/// unrelated opens come back `ERROR_SHARING_VIOLATION`.
///
/// [`std::fs::DirEntry::file_type`] answers from the data the directory
/// read already returned, so on Windows (and Linux, and macOS) it costs
/// no syscall at all and cannot fail for that reason. Its one documented
/// gap is symlinks — it describes the *link*, never its target — so
/// those alone fall through to the `is_dir()` probe, which follows the
/// link the way this walk wants.
fn dir_entry_is_dir(entry: &std::fs::DirEntry, path: &Path) -> bool {
    match entry.file_type() {
        Ok(ft) if !ft.is_symlink() => ft.is_dir(),
        _ => path.is_dir(),
    }
}

/// Whether `name` passes `filters` — an empty `filters` list accepts
/// everything; otherwise `name`'s extension (case-insensitive, without
/// the leading `.`) must match at least one extension in at least one
/// filter, or a filter must carry the wildcard `"*"`.
fn extension_matches(filters: &[(String, Vec<String>)], name: &str) -> bool {
    if filters.is_empty() {
        return true;
    }
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    filters.iter().any(|(_, exts)| {
        exts.iter()
            .any(|e| e == "*" || e.trim_start_matches('.').to_lowercase() == ext)
    })
}

/// Filter `all` by `query` using subsequence matching with a relevance
/// score — thin wrapper over [`crate::text_util::fuzzy_score`], same
/// approach as [`crate::compose::folder_picker::filter_dir_entries`].
fn filter_file_entries(all: &[PathBuf], query: &str) -> Vec<PathBuf> {
    const CAP: usize = 50;
    if query.is_empty() {
        return all.iter().take(CAP).cloned().collect();
    }
    let q = query.to_lowercase();
    let mut scored: Vec<(i32, &PathBuf)> = all
        .iter()
        .filter_map(|p| {
            let display = p.to_string_lossy().to_lowercase();
            crate::text_util::fuzzy_score(&display, &q).map(|(s, _)| (s, p))
        })
        .collect();
    scored.sort_by_key(|&(s, _)| Reverse(s));
    scored
        .into_iter()
        .take(CAP)
        .map(|(_, p)| p.clone())
        .collect()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir_with(files: &[&str], dirs: &[&str]) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        for f in files {
            std::fs::write(tmp.path().join(f), b"").expect("write file");
        }
        for d in dirs {
            std::fs::create_dir_all(tmp.path().join(d)).expect("mkdir");
        }
        tmp
    }

    #[test]
    fn new_lists_files_and_dirs() {
        let tmp = scratch_dir_with(&["a.rs", "b.txt"], &["sub"]);
        let picker = FilePickerController::new(FilePickerMode::Open, tmp.path(), vec![]);
        assert!(picker.all_entries.contains(&PathBuf::from("a.rs")));
        assert!(picker.all_entries.contains(&PathBuf::from("b.txt")));
        assert!(picker.all_entries.contains(&PathBuf::from("sub")));
    }

    #[test]
    fn filters_restrict_files_not_dirs() {
        let tmp = scratch_dir_with(&["a.rs", "b.txt"], &["sub"]);
        let picker = FilePickerController::new(
            FilePickerMode::Open,
            tmp.path(),
            vec![("Rust".to_string(), vec!["rs".to_string()])],
        );
        assert!(picker.all_entries.contains(&PathBuf::from("a.rs")));
        assert!(!picker.all_entries.contains(&PathBuf::from("b.txt")));
        assert!(
            picker.all_entries.contains(&PathBuf::from("sub")),
            "directories bypass file filters so the user can still navigate through them"
        );
    }

    #[test]
    fn open_mode_enter_on_file_confirms_it() {
        let tmp = scratch_dir_with(&["a.rs"], &[]);
        let mut picker = FilePickerController::new(FilePickerMode::Open, tmp.path(), vec![]);
        let idx = picker
            .filtered()
            .iter()
            .position(|p| p == &PathBuf::from("a.rs"))
            .expect("a.rs listed");
        picker.selected = idx;
        let result = picker.confirm_selection();
        assert_eq!(
            result,
            FilePickerEvent::Confirmed {
                path: tmp.path().join("a.rs")
            }
        );
    }

    #[test]
    fn open_mode_enter_on_dir_navigates_into_it() {
        let tmp = scratch_dir_with(&[], &["sub"]);
        let mut picker = FilePickerController::new(FilePickerMode::Open, tmp.path(), vec![]);
        let idx = picker
            .filtered()
            .iter()
            .position(|p| p == &PathBuf::from("sub"))
            .expect("sub listed");
        picker.selected = idx;
        let result = picker.confirm_selection();
        assert_eq!(result, FilePickerEvent::Consumed);
        assert_eq!(picker.root(), tmp.path().join("sub"));
    }

    #[test]
    fn enter_on_dotdot_navigates_up() {
        let parent = tempfile::tempdir().expect("parent tempdir");
        let sub = parent.path().join("child");
        std::fs::create_dir_all(&sub).expect("mkdir child");
        let mut picker = FilePickerController::new(FilePickerMode::Open, sub.clone(), vec![]);
        picker.selected = 0;
        assert_eq!(picker.filtered().first(), Some(&PathBuf::from("..")));
        let result = picker.confirm_selection();
        assert_eq!(result, FilePickerEvent::Consumed);
        assert_eq!(picker.root(), parent.path());
    }

    #[test]
    fn save_mode_enter_confirms_typed_name_even_if_absent() {
        let tmp = scratch_dir_with(&["existing.txt"], &[]);
        let mut picker = FilePickerController::new(FilePickerMode::Save, tmp.path(), vec![]);
        picker.push_char('n');
        picker.push_char('e');
        picker.push_char('w');
        let result = picker.confirm_selection();
        assert_eq!(
            result,
            FilePickerEvent::Confirmed {
                path: tmp.path().join("new")
            }
        );
    }

    #[test]
    fn save_mode_enter_with_empty_query_and_no_selection_is_ignored() {
        let tmp = scratch_dir_with(&[], &[]);
        let mut picker = FilePickerController::new(FilePickerMode::Save, tmp.path(), vec![]);
        // No entries at all (empty dir, no "..": tmp roots have a parent,
        // so ".." is still present) — clear it to hit the empty-query,
        // no-highlighted-row path directly.
        picker.filtered.clear();
        let result = picker.confirm_selection();
        assert_eq!(result, FilePickerEvent::Ignored);
    }

    #[test]
    fn save_mode_immediate_enter_confirms_seeded_name_not_navigate_up() {
        // Regression: pressing Enter with no prior navigation/typing used
        // to always act on the highlighted row, which defaults to "..".
        // A Save dialog opened with `with_initial_filename` and confirmed
        // immediately must save under that name, not silently navigate
        // up a directory.
        let parent = tempfile::tempdir().expect("parent tempdir");
        let sub = parent.path().join("child");
        std::fs::create_dir_all(&sub).expect("mkdir child");
        let mut picker = FilePickerController::new(FilePickerMode::Save, sub.clone(), vec![])
            .with_initial_filename("untitled.txt");
        assert_eq!(picker.filtered().first(), Some(&PathBuf::from("..")));
        let result = picker.confirm_selection();
        assert_eq!(
            result,
            FilePickerEvent::Confirmed {
                path: sub.join("untitled.txt")
            }
        );
        assert_eq!(picker.root(), sub, "must not have navigated");
    }

    #[test]
    fn save_mode_enter_on_dir_navigates_not_confirms() {
        let tmp = scratch_dir_with(&[], &["sub"]);
        let mut picker = FilePickerController::new(FilePickerMode::Save, tmp.path(), vec![]);
        picker.push_char('x'); // non-empty query — would confirm if not for the dir row
        let idx = picker
            .all_entries
            .iter()
            .position(|p| p == &PathBuf::from("sub"))
            .expect("sub listed");
        // Force-select the dir row directly (typing "x" filtered it out of
        // `filtered`, since "sub" doesn't fuzzy-match "x"); this test only
        // cares about `confirm_selection`'s directory-vs-file branch, so
        // bypass the filter machinery.
        picker.filtered = vec![picker.all_entries[idx].clone()];
        picker.selected = 0;
        let result = picker.confirm_selection();
        assert_eq!(result, FilePickerEvent::Consumed);
        assert_eq!(picker.root(), tmp.path().join("sub"));
    }

    #[test]
    fn with_initial_filename_seeds_query() {
        let tmp = scratch_dir_with(&[], &[]);
        let picker = FilePickerController::new(FilePickerMode::Save, tmp.path(), vec![])
            .with_initial_filename("untitled.txt");
        assert_eq!(picker.query(), "untitled.txt");
    }

    #[test]
    fn push_and_pop_char_no_vim_key_hijack() {
        // Unlike `FolderPickerController`, '-'/'j'/'k' must reach the
        // query unchanged — filenames routinely contain them.
        let tmp = scratch_dir_with(&[], &[]);
        let mut picker = FilePickerController::new(FilePickerMode::Save, tmp.path(), vec![]);
        for c in "my-journal.md".chars() {
            picker.push_char(c);
        }
        assert_eq!(picker.query(), "my-journal.md");
    }

    #[test]
    fn move_up_clamps_at_zero() {
        let tmp = scratch_dir_with(&["a.rs"], &[]);
        let mut picker = FilePickerController::new(FilePickerMode::Open, tmp.path(), vec![]);
        picker.move_up();
        assert_eq!(picker.selected(), 0);
    }

    #[test]
    fn move_down_clamps_at_last() {
        let tmp = scratch_dir_with(&["a.rs", "b.rs"], &[]);
        let mut picker = FilePickerController::new(FilePickerMode::Open, tmp.path(), vec![]);
        let last = picker.filtered().len().saturating_sub(1);
        for _ in 0..last + 5 {
            picker.move_down();
        }
        assert_eq!(picker.selected(), last);
    }

    #[test]
    fn escape_cancels() {
        let tmp = scratch_dir_with(&[], &[]);
        let mut picker = FilePickerController::new(FilePickerMode::Open, tmp.path(), vec![]);
        let ev = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        assert_eq!(picker.handle(&ev, 10), FilePickerEvent::Cancelled);
    }

    #[test]
    fn build_palette_open_mode_has_no_create_label() {
        let tmp = scratch_dir_with(&["a.rs"], &[]);
        let mut picker = FilePickerController::new(FilePickerMode::Open, tmp.path(), vec![]);
        picker.push_char('a');
        let rect = Rect::new(0.0, 0.0, 80.0, 24.0);
        assert!(picker.build_palette(rect).create_label.is_none());
    }

    #[test]
    fn build_palette_save_mode_has_create_label_when_query_nonempty() {
        let tmp = scratch_dir_with(&[], &[]);
        let mut picker = FilePickerController::new(FilePickerMode::Save, tmp.path(), vec![]);
        picker.push_char('x');
        let rect = Rect::new(0.0, 0.0, 80.0, 24.0);
        let label = picker.build_palette(rect).create_label;
        assert_eq!(label.as_deref(), Some("Save as \"x\""));
    }

    #[test]
    fn build_palette_title_reflects_mode() {
        let tmp = scratch_dir_with(&[], &[]);
        let open = FilePickerController::new(FilePickerMode::Open, tmp.path(), vec![]);
        let save = FilePickerController::new(FilePickerMode::Save, tmp.path(), vec![]);
        let rect = Rect::new(0.0, 0.0, 80.0, 24.0);
        assert!(open.build_palette(rect).title.starts_with("Open File"));
        assert!(save.build_palette(rect).title.starts_with("Save File"));
    }

    #[test]
    fn with_id_overrides_widget_id() {
        let tmp = scratch_dir_with(&[], &[]);
        let picker =
            FilePickerController::new(FilePickerMode::Open, tmp.path(), vec![]).with_id("picker_b");
        let rect = Rect::new(0.0, 0.0, 80.0, 24.0);
        assert_eq!(picker.build_palette(rect).id.as_str(), "picker_b");
    }

    /// A symlink pointing at a directory must still behave like a
    /// directory: listed even under an extension filter that its name
    /// fails, and navigable with Enter.
    ///
    /// This is the case [`dir_entry_is_dir`] deliberately falls back to
    /// `Path::is_dir` for — `DirEntry::file_type` describes the *link*,
    /// so consulting it alone would classify this row as a file and an
    /// active filter would then hide it. Unix-only because creating a
    /// symlink on Windows needs either developer mode or
    /// `SeCreateSymbolicLinkPrivilege`, neither of which a test may
    /// assume.
    #[cfg(unix)]
    #[test]
    fn symlinked_directory_is_listed_and_navigable_as_a_directory() {
        let tmp = scratch_dir_with(&["a.rs"], &["real_dir"]);
        std::os::unix::fs::symlink(tmp.path().join("real_dir"), tmp.path().join("link"))
            .expect("symlink");

        let mut picker = FilePickerController::new(
            FilePickerMode::Open,
            tmp.path(),
            vec![("Rust".to_string(), vec!["rs".to_string()])],
        );
        assert!(
            picker.all_entries.contains(&PathBuf::from("link")),
            "a symlink to a directory must bypass the file extension filter, \
             the same as a real directory"
        );

        let idx = picker
            .filtered()
            .iter()
            .position(|p| p == &PathBuf::from("link"))
            .expect("link listed");
        picker.selected = idx;
        assert_eq!(picker.confirm_selection(), FilePickerEvent::Consumed);
        assert_eq!(picker.root(), tmp.path().join("link"));
    }

    #[test]
    fn extension_matches_wildcard_accepts_everything() {
        assert!(extension_matches(
            &[("Any".to_string(), vec!["*".to_string()])],
            "whatever.zzz"
        ));
    }

    #[test]
    fn extension_matches_case_insensitive() {
        assert!(extension_matches(
            &[("Rust".to_string(), vec!["RS".to_string()])],
            "main.rs"
        ));
    }

    #[test]
    fn extension_matches_rejects_non_matching() {
        assert!(!extension_matches(
            &[("Rust".to_string(), vec!["rs".to_string()])],
            "main.txt"
        ));
    }
}
