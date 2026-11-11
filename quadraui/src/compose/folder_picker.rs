//! `FolderPickerController` — engine-level folder/workspace directory picker.
//!
//! Owns all state for an interactive directory-browsing modal:
//! filesystem walking, fuzzy filtering, scroll, and selection. Renders
//! via the existing [`Palette`] primitive (no new
//! `Backend` trait method needed) and accepts backend-neutral
//! [`UiEvent`]s.
//!
//! # Relation to vimcode
//!
//! Extracted verbatim from vimcode's TUI-local `FolderPickerState`
//! (`src/tui_main/mod.rs`, ~285 lines). After the quadraui PR lands,
//! vimcode removes the TUI-local code and rewires both TUI and GTK
//! `OpenFolderDialog` through this controller.
//!
//! # Adoption status
//!
//! The vimcode migration above hasn't landed as of #825's audit — no
//! confirmed external adopter yet. Kept public rather than demoted:
//! full TUI + GTK demo coverage already exists
//! (`examples/tui_folder_picker.rs`, `examples/gtk_folder_picker.rs`,
//! `examples/common/folder_picker_app.rs`) plus a `TuiDriver` end-to-end
//! test in `tests/tui_example_driver.rs`, and the extraction above is a
//! concrete, named migration target, not a speculative one.
//!
//! # Usage pattern
//!
//! ```rust,ignore
//! // Instantiate when the user triggers "Open Folder":
//! let mut picker = FolderPickerController::rooted_at(
//!     std::env::current_dir().unwrap(),
//!     vec![],       // no extra file names to surface
//! );                 // hidden dirs stay off by default; chain
//!                    // `.with_show_hidden(true)` to surface them
//!
//! // In AppLogic::render:
//! let rect = /* palette popup bounds */;
//! picker.render(rect, backend);
//!
//! // In AppLogic::handle:
//! let visible_rows = /* rect.height as usize - PALETTE_CHROME_ROWS */;
//! match picker.handle_event(&event, visible_rows) {
//!     Handled::Event(PathPickerAction::Confirmed { path }) => { /* open workspace */ },
//!     Handled::Event(PathPickerAction::Cancelled)          => { /* dismiss modal */ },
//!     Handled::Consumed                                    => { /* redraw */ },
//!     Handled::Ignored                                      => {},
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

use std::path::{Path, PathBuf};

use super::path_picker::{
    classify_nav_key, dir_entry_is_dir, NavKey, PathPickerAction, PathPickerCore,
};
use crate::compose::Handled;
use crate::{
    Backend, Icon, Key, Modifiers, Palette, PaletteItem, Rect, StyledText, UiEvent, WidgetId,
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

/// Default recursion depth for [`FolderPickerController::rooted_at`].
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
#[deprecated(
    since = "0.9.0",
    note = "use `FolderPickerController::handle_event`'s `Handled<PathPickerAction>` instead — `Consumed`/`Ignored` are `Handled::Consumed`/`Handled::Ignored`, `Confirmed`/`Cancelled` move to `PathPickerAction` under `Handled::Event`"
)]
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
    /// Shared query/list/selection/scroll state.
    core: PathPickerCore,
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
    /// Create a new picker rooted at `root`, with `show_hidden` as a
    /// positional bool.
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
    #[deprecated(
        since = "0.9.0",
        note = "use `rooted_at`, which drops the `show_hidden: bool` positional param — chain `.with_show_hidden(true)` if needed"
    )]
    pub fn new(root: impl Into<PathBuf>, extra_file_names: Vec<String>, show_hidden: bool) -> Self {
        let picker = Self::rooted_at(root, extra_file_names);
        if show_hidden {
            picker.with_show_hidden(true)
        } else {
            picker
        }
    }

    /// Create a new picker rooted at `root`. Hidden directories (names
    /// beginning with `.`) stay hidden unless surfaced via
    /// `extra_file_names` or [`Self::with_show_hidden`].
    ///
    /// # Arguments
    ///
    /// - `root` — initial browsing directory (typically `env::current_dir()`
    ///   or the engine's active workspace root).
    /// - `extra_file_names` — dotfile names to surface even with hidden
    ///   directories hidden (e.g. `[".vimcode-workspace".to_string()]`).
    ///   Pass an empty `Vec` for the common case.
    pub fn rooted_at(root: impl Into<PathBuf>, extra_file_names: Vec<String>) -> Self {
        let root = root.into();
        let ignore_dirs: Vec<String> = DEFAULT_IGNORE_DIRS
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let max_depth = DEFAULT_MAX_DEPTH;
        let show_hidden = false;
        let all_entries = collect_dir_entries(
            &root,
            show_hidden,
            &extra_file_names,
            &ignore_dirs,
            max_depth,
        );
        Self {
            id: WidgetId::new("folder_picker"),
            core: PathPickerCore::new(root, all_entries),
            show_hidden,
            extra_file_names,
            ignore_dirs,
            max_depth,
        }
    }

    /// Toggle whether hidden directories (names beginning with `.`) are
    /// surfaced. Defaults to `false` — see [`Self::rooted_at`].
    ///
    /// Re-walks the current `root` so the change takes effect immediately.
    /// Returns `self` for builder-style chaining.
    pub fn with_show_hidden(mut self, show_hidden: bool) -> Self {
        self.show_hidden = show_hidden;
        let all_entries = collect_dir_entries(
            &self.core.root,
            self.show_hidden,
            &self.extra_file_names,
            &self.ignore_dirs,
            self.max_depth,
        );
        self.core.set_all_entries(all_entries);
        self
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
        let all_entries = collect_dir_entries(
            &self.core.root,
            self.show_hidden,
            &self.extra_file_names,
            &self.ignore_dirs,
            self.max_depth,
        );
        self.core.set_all_entries(all_entries);
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
        let all_entries = collect_dir_entries(
            &self.core.root,
            self.show_hidden,
            &self.extra_file_names,
            &self.ignore_dirs,
            self.max_depth,
        );
        self.core.set_all_entries(all_entries);
        self
    }

    // ── State accessors ───────────────────────────────────────────────

    /// Current browsing root.
    pub fn root(&self) -> &Path {
        &self.core.root
    }

    /// Current query string.
    pub fn query(&self) -> &str {
        &self.core.query
    }

    /// Currently filtered entries (relative paths).
    pub fn filtered(&self) -> &[PathBuf] {
        &self.core.filtered
    }

    /// Index of the highlighted row in `filtered`.
    pub fn selected(&self) -> usize {
        self.core.selected
    }

    /// Scroll offset (first visible row index in `filtered`).
    pub fn scroll_top(&self) -> usize {
        self.core.scroll_top
    }

    /// The resolved absolute path for the currently selected entry, or `None`
    /// if the list is empty.
    ///
    /// The `..` entry resolves to the parent of `root`; all others resolve to
    /// `root.join(rel)`.
    pub fn selected_path(&self) -> Option<PathBuf> {
        let rel = self.core.filtered.get(self.core.selected)?;
        if rel.as_os_str() == ".." {
            self.core.root.parent().map(|p| p.to_path_buf())
        } else {
            Some(self.core.root.join(rel))
        }
    }

    // ── Navigation ────────────────────────────────────────────────────

    /// Navigate to `new_root`, clearing the query and reloading entries.
    pub fn navigate_to(&mut self, new_root: PathBuf) {
        let all_entries = collect_dir_entries(
            &new_root,
            self.show_hidden,
            &self.extra_file_names,
            &self.ignore_dirs,
            self.max_depth,
        );
        self.core.navigate_to(new_root, all_entries);
    }

    /// Navigate to the parent directory (no-op at filesystem root).
    pub fn navigate_up(&mut self) {
        if let Some(parent) = self.core.root.parent() {
            self.navigate_to(parent.to_path_buf());
        }
    }

    // ── Query editing ─────────────────────────────────────────────────

    /// Append `c` to the query and refilter.
    pub fn push_char(&mut self, c: char) {
        self.core.push_char(c);
    }

    /// Remove the last character from the query and refilter.
    pub fn pop_char(&mut self) {
        self.core.pop_char();
    }

    /// Re-compute `filtered` from `all_entries` and the current `query`.
    pub fn refilter(&mut self) {
        self.core.refilter();
    }

    // ── Selection movement ────────────────────────────────────────────

    /// Move the selection one row up (clamped at 0).
    pub fn move_up(&mut self) {
        self.core.move_up();
    }

    /// Move the selection one row down (clamped at last row).
    pub fn move_down(&mut self) {
        self.core.move_down();
    }

    /// Clamp `scroll_top` so `selected` is always visible within
    /// `visible_rows` rows.
    pub fn sync_scroll(&mut self, visible_rows: usize) {
        self.core.sync_scroll(visible_rows);
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
    pub fn handle_event(
        &mut self,
        event: &UiEvent,
        visible_rows: usize,
    ) -> Handled<PathPickerAction> {
        let result = match event {
            UiEvent::KeyPressed { key, modifiers, .. } => self.handle_key(key, modifiers),
            _ => Handled::Ignored,
        };

        // Keep scroll in sync after any state change.
        if matches!(result, Handled::Consumed) {
            self.sync_scroll(visible_rows);
        }

        result
    }

    /// Drive the state machine with a backend-neutral `UiEvent`.
    #[deprecated(
        since = "0.9.0",
        note = "use `handle_event`, which returns `Handled<PathPickerAction>` instead of `FolderPickerEvent`"
    )]
    #[allow(deprecated)] // `FolderPickerEvent` is `#[deprecated]`; this method is its own shim.
    pub fn handle(&mut self, event: &UiEvent, visible_rows: usize) -> FolderPickerEvent {
        match self.handle_event(event, visible_rows) {
            Handled::Event(PathPickerAction::Confirmed { path }) => {
                FolderPickerEvent::Confirmed { path }
            }
            Handled::Event(PathPickerAction::Cancelled) => FolderPickerEvent::Cancelled,
            Handled::Consumed => FolderPickerEvent::Consumed,
            Handled::Ignored => FolderPickerEvent::Ignored,
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────

    /// Key dispatch, shared with [`FilePickerController`] via
    /// [`classify_nav_key`](super::path_picker::classify_nav_key) so the
    /// two pickers' key maps (including the vim-style `-`/`j`/`k`
    /// shortcuts) can't drift apart.
    ///
    /// [`FilePickerController`]: crate::compose::FilePickerController
    fn handle_key(&mut self, key: &Key, modifiers: &Modifiers) -> Handled<PathPickerAction> {
        // The folder picker's query is never a destination filename, so
        // the vim shortcuts always take priority over typing them.
        match classify_nav_key(key, modifiers, true) {
            Some(NavKey::Escape) => Handled::Event(PathPickerAction::Cancelled),

            Some(NavKey::Confirm) => {
                // ".." → navigate up; anything else → confirm selection.
                let is_dotdot = self
                    .core
                    .filtered
                    .get(self.core.selected)
                    .map(|p| p.as_os_str() == "..")
                    .unwrap_or(false);
                if is_dotdot {
                    self.navigate_up();
                    Handled::Consumed
                } else if let Some(path) = self.selected_path() {
                    Handled::Event(PathPickerAction::Confirmed { path })
                } else {
                    Handled::Consumed
                }
            }

            Some(NavKey::NavigateUp) => {
                self.navigate_up();
                Handled::Consumed
            }

            Some(NavKey::Up) => {
                self.move_up();
                Handled::Consumed
            }

            Some(NavKey::Down) => {
                self.move_down();
                Handled::Consumed
            }

            Some(NavKey::Backspace) => {
                self.pop_char();
                Handled::Consumed
            }

            Some(NavKey::Char(c)) => {
                self.push_char(c);
                Handled::Consumed
            }

            None => Handled::Ignored,
        }
    }

    /// Build the `Palette` descriptor for the current state.
    fn build_palette(&self, rect: Rect) -> Palette {
        let r = self.core.root.to_string_lossy();
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

        // Codicon glyph on a GUI backend (`nerd_fonts_enabled` defaults
        // `true` there), falling back to the pre-existing emoji on TUI
        // (`nerd_fonts_enabled` defaults `false`), so TUI output is
        // unchanged while GUI gets the vector folder/gear icon.
        let folder_icon = Icon::codicon("folder", "\u{1F4C1}")
            .unwrap_or_else(|| Icon::new("\u{1F4C1}", "\u{1F4C1}")); // 📁
        let file_icon =
            Icon::codicon("gear", "\u{2699}").unwrap_or_else(|| Icon::new("\u{2699}", "\u{2699}")); // ⚙

        let items: Vec<PaletteItem> = self
            .core
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
            id: self.id,
            title,
            query: self.core.query.clone(),
            query_cursor: self.core.query.len(),
            items,
            selected_idx: self.core.selected,
            scroll_offset: self.core.scroll_top,
            total_count: self.core.all_entries.len(),
            has_focus: true,
            show_query: true,
            create_label: None,
            preview: None,
            mode: crate::primitives::palette::PaletteMode::List,
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
        if dir_entry_is_dir(&entry, &path) {
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NamedKey;

    /// Number of child directories [`picker_root`] creates. Chosen so
    /// `filtered()` (which also carries `".."` and `"."`) holds well over
    /// ten entries, the largest selection index any test below drives.
    const PICKER_ROOT_CHILDREN: usize = 12;

    /// A throwaway browsing root holding [`PICKER_ROOT_CHILDREN`] empty
    /// child directories.
    ///
    /// Every controller test roots its picker here rather than at the
    /// ambient `env::temp_dir()`: `FolderPickerController::new` walks the
    /// root recursively to `DEFAULT_MAX_DEPTH`, so a shared system temp
    /// directory makes both the entry list and the walk's cost depend on
    /// whatever else happens to live there. The returned `TempDir` must
    /// stay bound for the lifetime of the picker — dropping it deletes
    /// the directory.
    fn picker_root() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("create picker tempdir");
        for i in 0..PICKER_ROOT_CHILDREN {
            std::fs::create_dir(dir.path().join(format!("child{i:02}"))).expect("create child dir");
        }
        dir
    }

    // Fuzzy-scoring and filtering live in `path_picker` now — see
    // `super::path_picker::tests` for their coverage. This module only
    // tests `FolderPickerController`'s own state machine.

    // ── FolderPickerController state machine ──────────────────────────

    #[test]
    fn new_sets_dotdot_and_dot_entries() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let picker = FolderPickerController::rooted_at(tmp, vec![]);
        // ".." and "." should always be the first two entries.
        assert!(picker.core.all_entries.contains(&PathBuf::from("..")));
        assert!(picker.core.all_entries.contains(&PathBuf::from(".")));
        // An isolated root makes the whole walk deterministic: "..", "."
        // and one entry per child directory, nothing else.
        assert_eq!(
            picker.filtered().len(),
            PICKER_ROOT_CHILDREN + 2,
            "isolated root must yield exactly \"..\", \".\" and one entry per child"
        );
    }

    #[test]
    fn with_show_hidden_surfaces_dot_directories() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        std::fs::create_dir(tmp.join(".hidden-child")).expect("create hidden child dir");

        let hidden_off = FolderPickerController::rooted_at(tmp.clone(), vec![]);
        assert!(
            !hidden_off
                .core
                .all_entries
                .contains(&PathBuf::from(".hidden-child")),
            "hidden directories stay hidden by default"
        );

        let hidden_on = FolderPickerController::rooted_at(tmp, vec![]).with_show_hidden(true);
        assert!(
            hidden_on
                .core
                .all_entries
                .contains(&PathBuf::from(".hidden-child")),
            "with_show_hidden(true) surfaces hidden directories"
        );
    }

    #[test]
    #[allow(deprecated)] // pins the deprecated bool-taking shim still forwards correctly.
    fn deprecated_new_still_forwards_to_rooted_at_and_with_show_hidden() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        std::fs::create_dir(tmp.join(".hidden-child")).expect("create hidden child dir");

        let hidden_off = FolderPickerController::new(tmp.clone(), vec![], false);
        assert!(!hidden_off
            .core
            .all_entries
            .contains(&PathBuf::from(".hidden-child")));

        let hidden_on = FolderPickerController::new(tmp, vec![], true);
        assert!(hidden_on
            .core
            .all_entries
            .contains(&PathBuf::from(".hidden-child")));
    }

    #[test]
    fn push_and_pop_char_update_query() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let mut picker = FolderPickerController::rooted_at(tmp, vec![]);
        picker.push_char('s');
        picker.push_char('r');
        assert_eq!(picker.query(), "sr");
        picker.pop_char();
        assert_eq!(picker.query(), "s");
    }

    #[test]
    fn move_up_clamps_at_zero() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let mut picker = FolderPickerController::rooted_at(tmp, vec![]);
        assert_eq!(picker.selected(), 0);
        picker.move_up();
        assert_eq!(picker.selected(), 0);
    }

    #[test]
    fn move_down_advances_selection() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let mut picker = FolderPickerController::rooted_at(tmp, vec![]);
        assert!(picker.filtered().len() >= 2);
        picker.move_down();
        assert_eq!(picker.selected(), 1);
    }

    #[test]
    fn move_down_clamps_at_last() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let mut picker = FolderPickerController::rooted_at(tmp, vec![]);
        let last = picker.filtered().len().saturating_sub(1);
        for _ in 0..last + 5 {
            picker.move_down();
        }
        assert_eq!(picker.selected(), last);
    }

    #[test]
    fn sync_scroll_brings_selected_into_view() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let mut picker = FolderPickerController::rooted_at(tmp, vec![]);
        // Force a large selection index.
        assert!(picker.filtered().len() >= 10);
        picker.core.selected = 9;
        picker.sync_scroll(5);
        assert!(picker.core.scroll_top + 5 > picker.core.selected);
        assert!(picker.core.scroll_top <= picker.core.selected);
    }

    #[test]
    fn sync_scroll_scrolls_up_when_needed() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let mut picker = FolderPickerController::rooted_at(tmp, vec![]);
        assert!(picker.filtered().len() >= 5);
        picker.core.scroll_top = 5;
        picker.core.selected = 2; // above scroll_top
        picker.sync_scroll(3);
        assert_eq!(picker.core.scroll_top, 2);
    }

    #[test]
    fn escape_event_returns_cancelled() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let mut picker = FolderPickerController::rooted_at(tmp, vec![]);
        let ev = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        assert_eq!(
            picker.handle_event(&ev, 10),
            Handled::Event(PathPickerAction::Cancelled)
        );
    }

    #[test]
    fn enter_on_dotdot_navigates_up() {
        // Use tempfile so parallel test runs don't race on a fixed name and
        // a crashed prior run can't leak a stale marker directory.
        let parent = tempfile::tempdir().expect("create parent tempdir");
        let sub = parent.path().join("nav_up_child");
        std::fs::create_dir_all(&sub).expect("create child dir");
        let mut picker = FolderPickerController::rooted_at(sub.clone(), vec![]);
        // Select ".." (index 0, always prepended when parent exists).
        picker.core.selected = 0;
        assert_eq!(picker.filtered().first(), Some(&PathBuf::from("..")));
        let ev = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let result = picker.handle_event(&ev, 10);
        assert_eq!(result, Handled::Consumed);
        assert_eq!(picker.root(), parent.path());
    }

    #[test]
    fn enter_on_dot_returns_confirmed_with_root() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let mut picker = FolderPickerController::rooted_at(tmp.clone(), vec![]);
        // "." is always at index 1 (after "..") when parent exists.
        let dot_idx = picker
            .filtered()
            .iter()
            .position(|p| p.as_os_str() == ".")
            .expect("'.' should always be in filtered");
        picker.core.selected = dot_idx;
        let ev = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let result = picker.handle_event(&ev, 10);
        assert_eq!(
            result,
            Handled::Event(PathPickerAction::Confirmed { path: tmp })
        );
    }

    #[test]
    fn up_down_key_moves_selection() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let mut picker = FolderPickerController::rooted_at(tmp, vec![]);
        assert!(picker.filtered().len() >= 2);
        let ev_down = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Down),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let _ = picker.handle_event(&ev_down, 10);
        assert_eq!(picker.selected(), 1);

        let ev_up = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Up),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let _ = picker.handle_event(&ev_up, 10);
        assert_eq!(picker.selected(), 0);
    }

    #[test]
    fn key_char_appends_to_query() {
        // Query characters arrive via `Key::Char` in `KeyPressed`, *not* via
        // `CharTyped`. The latter is intentionally ignored to avoid
        // double-insert when a future backend emits both for the same input.
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let mut picker = FolderPickerController::rooted_at(tmp, vec![]);
        let ev = UiEvent::KeyPressed {
            key: Key::Char('s'),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let _ = picker.handle_event(&ev, 10);
        assert_eq!(picker.query(), "s");
    }

    // macOS delivers a Cmd-modified letter as a plain `Key::Char` with
    // `modifiers.cmd` set, the same shape Ctrl uses on every other
    // backend, so it must not be typed into the filter query or trigger
    // a vim-style navigation shortcut.
    #[test]
    fn cmd_char_is_ignored_not_typed_into_query() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let mut picker = FolderPickerController::rooted_at(tmp, vec![]);
        let ev = UiEvent::KeyPressed {
            key: Key::Char('j'),
            modifiers: Modifiers {
                cmd: true,
                ..Default::default()
            },
            repeat: false,
        };
        let result = picker.handle_event(&ev, 10);
        assert_eq!(result, Handled::Ignored);
        assert_eq!(picker.query(), "");
    }

    #[test]
    fn char_typed_is_ignored() {
        // Documented contract: `CharTyped` is not the canonical typing
        // source; only `Key::Char` in `KeyPressed` mutates the query.
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let mut picker = FolderPickerController::rooted_at(tmp, vec![]);
        let ev = UiEvent::CharTyped('s');
        let result = picker.handle_event(&ev, 10);
        assert_eq!(result, Handled::Ignored);
        assert_eq!(picker.query(), "");
    }

    #[test]
    fn backspace_removes_last_char() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let mut picker = FolderPickerController::rooted_at(tmp, vec![]);
        picker.push_char('s');
        picker.push_char('r');
        let ev = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Backspace),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let _ = picker.handle_event(&ev, 10);
        assert_eq!(picker.query(), "s");
    }

    #[test]
    fn selected_path_resolves_dot_to_root() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let picker = FolderPickerController::rooted_at(tmp.clone(), vec![]);
        let dot_idx = picker
            .filtered()
            .iter()
            .position(|p| p.as_os_str() == ".")
            .unwrap();
        let mut picker = picker;
        picker.core.selected = dot_idx;
        assert_eq!(picker.selected_path(), Some(tmp));
    }

    #[test]
    fn dash_key_navigates_up() {
        let parent = tempfile::tempdir().expect("create parent tempdir");
        let sub = parent.path().join("dash_nav_child");
        std::fs::create_dir_all(&sub).expect("create child dir");
        let mut picker = FolderPickerController::rooted_at(sub.clone(), vec![]);
        let ev = UiEvent::KeyPressed {
            key: Key::Char('-'),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let result = picker.handle_event(&ev, 10);
        assert_eq!(result, Handled::Consumed);
        assert_eq!(picker.root(), parent.path());
    }

    #[test]
    fn build_palette_id_is_folder_picker() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let picker = FolderPickerController::rooted_at(tmp, vec![]);
        let rect = Rect::new(0.0, 0.0, 80.0, 24.0);
        let palette = picker.build_palette(rect);
        assert_eq!(palette.id.as_str(), "folder_picker");
    }

    #[test]
    fn build_palette_title_contains_open_folder() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let picker = FolderPickerController::rooted_at(tmp, vec![]);
        let rect = Rect::new(0.0, 0.0, 80.0, 24.0);
        let palette = picker.build_palette(rect);
        assert!(palette.title.starts_with("Open Folder"));
    }

    #[test]
    fn build_palette_items_match_filtered() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let picker = FolderPickerController::rooted_at(tmp, vec![]);
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
        let picker = FolderPickerController::rooted_at(
            root.path().to_path_buf(),
            vec![".my-workspace".to_string()],
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

    #[test]
    fn build_palette_handles_non_ascii_root_path() {
        // `build_palette` must handle non-ASCII paths correctly: it must
        // validate UTF-8 char boundaries when slicing the root path to fit
        // the viewport width, rather than panicking on multi-byte characters.
        let long_japanese = PathBuf::from(format!("/a/{}", "ト".repeat(50)));
        let picker = FolderPickerController::rooted_at(long_japanese, vec![]);
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
        let picker = FolderPickerController::rooted_at(path, vec![]);
        let rect = Rect::new(0.0, 0.0, 60.0, 24.0);
        let palette = picker.build_palette(rect);
        assert!(palette.title.starts_with("Open Folder "));
    }

    // ── Builder-method overrides ──────────────────────────────────────

    #[test]
    fn with_id_overrides_widget_id() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let picker = FolderPickerController::rooted_at(tmp, vec![]).with_id("picker_b");
        let rect = Rect::new(0.0, 0.0, 80.0, 24.0);
        assert_eq!(picker.build_palette(rect).id.as_str(), "picker_b");
    }

    #[test]
    fn with_ignore_dirs_replaces_default_list() {
        let root = tempfile::tempdir().expect("tempdir");
        // Create `target` and `keep_me`; default list skips `target`.
        std::fs::create_dir_all(root.path().join("target")).unwrap();
        std::fs::create_dir_all(root.path().join("keep_me")).unwrap();
        // With an empty override list, `target` is no longer skipped.
        let picker = FolderPickerController::rooted_at(root.path().to_path_buf(), vec![])
            .with_ignore_dirs(vec![]);
        let has_target = picker
            .core
            .all_entries
            .iter()
            .any(|p| p == &PathBuf::from("target"));
        let has_keep = picker
            .core
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
            FolderPickerController::rooted_at(root.path().to_path_buf(), vec![]).with_max_depth(0);
        let has_d1 = picker.core.all_entries.iter().any(|p| p.ends_with("d1"));
        let has_d2 = picker.core.all_entries.iter().any(|p| p.ends_with("d2"));
        assert!(has_d1);
        assert!(!has_d2, "depth=0 should not surface nested subdirs");
    }
}
