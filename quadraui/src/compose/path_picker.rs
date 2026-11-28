//! [`PathPicker`] — the filesystem picker on the widget contract, one
//! generic widget behind both [`FolderPicker`] and [`FilePicker`] — plus
//! [`PathPickerState`], the browsing model it paints, and the one
//! filesystem walk both kinds share.
//!
//! Both kinds browse a filesystem tree with the same query/list/
//! selection/scroll mechanics, the same walk and the same key map; a
//! [`PickerKind`] supplies only what differs: which files the walk lists,
//! whether a `"."` row names the root itself, what `Enter` resolves to,
//! the title and icons, and whether the vim-style `-`/`j`/`k` shortcuts
//! take priority over typing them (off in [`FilePicker`]'s Save mode,
//! where the query *is* the destination filename).
//!
//! # Hosting
//!
//! An app on the runtime's routing engine pushes the picker itself as a
//! `Surface::Widget` and names it in `AppLogic::frame_tree`; its actions
//! arrive as [`UiEvent::PathPicker`], and the app hands each browsing
//! change to [`PathPicker::apply`]. A host that reads raw [`UiEvent`]s
//! instead (a nested dialog loop, a gallery demo) paints the picker with
//! `draw_dyn` and resolves each event through [`PathPicker::handle_event`],
//! which runs the same [`Interactive`] against the state the runtime
//! would keep for the picker's id.
//!
//! [`FolderPicker`]: crate::compose::FolderPicker
//! [`FilePicker`]: crate::compose::FilePicker

use std::cmp::Reverse;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::a11y::{A11yChild, A11yInfo, A11yPrimitive};
use crate::behaviour::nav::{FlatNav, NavKey as FlatNavKey};
use crate::behaviour::scroll::ensure_visible;
use crate::compose::palette_host::{route_palette_host, PaletteHostInput};
use crate::compose::widget_host;
use crate::compose::Handled;
use crate::interactive::{EventCx, FocusSpec, Interactive, RoutedEvent, SubTarget, WidgetAction};
use crate::primitive::{LayoutCtx, Primitive, Size};
use crate::primitives::palette::{Palette, PaletteItem, PalettePrimitiveLayout, PaletteState};
use crate::types::WidgetId;
use crate::{Icon, Key, Rect, StyledText, UiEvent};

/// Overhead rows consumed by the `Palette` chrome (title + query + borders).
///
/// Subtract from the popup height to get the `visible_rows` that
/// [`PathPicker::apply`] and [`PathPicker::handle_event`] take:
///
/// ```rust
/// # use quadraui::compose::PALETTE_CHROME_ROWS;
/// let popup_height = 20usize;
/// let visible_rows = popup_height.saturating_sub(PALETTE_CHROME_ROWS);
/// # let _ = visible_rows;
/// ```
pub const PALETTE_CHROME_ROWS: usize = 4;

/// Default recursion depth of a picker's walk.
///
/// Override per-instance with [`PathPicker::with_max_depth`].
pub const DEFAULT_MAX_DEPTH: usize = 5;

/// Default heavy build/dependency directory names that are skipped during
/// the filesystem walk.
///
/// Override per-instance with [`PathPicker::with_ignore_dirs`].
pub const DEFAULT_IGNORE_DIRS: &[&str] = &["target", "node_modules", "__pycache__"];

/// What a path picker reports: a confirmed path or a dismissal, plus the
/// browsing changes the host applies onto the picker's
/// [`PathPickerState`] (the pickers' `apply` does it).
///
/// Shared by [`crate::compose::FolderPicker`] and
/// [`crate::compose::FilePicker`], and by their controllers'
/// `handle_event`, which apply the browsing changes themselves and so
/// only ever report [`Self::Confirmed`] and [`Self::Cancelled`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
/// Used by [`walk_entries`], so a transient stat failure is handled the
/// same way for every picker kind.
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
/// With `vim_keys`, an unmodified `-` browses up to the root's parent
/// instead of typing. Everything else goes through the shared palette-host
/// prelude ([`route_palette_host`]): Escape cancels, Enter resolves through
/// `confirm`, `vim_keys` makes an unmodified `k`/`j` move the highlight, and
/// the palette's own input reports the browsing change it names. A reported
/// navigation clears `state`, because the query it was editing is cleared
/// with the old root.
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
    let handled = resolve_picker_event(paths, palette, state, event, layout, cx, vim_keys, confirm);
    if matches!(handled, Handled::Event(PathPickerAction::Navigated { .. })) {
        *state = PaletteState::default();
    }
    handled
}

#[allow(clippy::too_many_arguments)]
fn resolve_picker_event(
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
        if vim_keys && !modified && key.key == Key::Char('-') {
            return match paths.parent() {
                Some(root) => Handled::Event(PathPickerAction::Navigated { root }),
                None => Handled::Consumed,
            };
        }
    }
    route_palette_host(
        palette,
        state,
        event,
        layout,
        cx,
        (paths.filtered.len(), paths.selected),
        vim_keys,
        |input| match input {
            PaletteHostInput::Cancel => Handled::Event(PathPickerAction::Cancelled),
            PaletteHostInput::Confirm(_) => confirm(),
            PaletteHostInput::Select(idx) => select(paths, idx),
            PaletteHostInput::QueryChanged(query) => {
                Handled::Event(PathPickerAction::QueryChanged { query })
            }
        },
    )
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

// ── Walk ──────────────────────────────────────────────────────────────────────

/// What a picker's filesystem walk surfaces, shared by every
/// [`PickerKind`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkOptions {
    /// Whether to surface hidden entries (names beginning with `.`).
    pub show_hidden: bool,
    /// File names surfaced even when hidden entries are not and even when
    /// the kind lists no other files — e.g. `[".vimcode-workspace"]`
    /// lets a folder picker show a workspace marker file without exposing
    /// every hidden entry.
    pub extra_file_names: Vec<String>,
    /// Directory names the walk skips (heavy build / dependency dirs).
    /// Defaults to [`DEFAULT_IGNORE_DIRS`]. Files are never skipped by
    /// name.
    pub ignore_dirs: Vec<String>,
    /// Maximum recursion depth. Defaults to [`DEFAULT_MAX_DEPTH`].
    pub max_depth: usize,
}

impl Default for WalkOptions {
    fn default() -> Self {
        Self {
            show_hidden: false,
            extra_file_names: Vec::new(),
            ignore_dirs: DEFAULT_IGNORE_DIRS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            max_depth: DEFAULT_MAX_DEPTH,
        }
    }
}

/// Walk `root` for a `K` picker: relative paths of every directory (depth
/// ≤ [`WalkOptions::max_depth`], minus [`WalkOptions::ignore_dirs`]) and of
/// the files `kind` lists or [`WalkOptions::extra_file_names`] names,
/// sorted by name within each directory.
///
/// `..` comes first unless `root` is a filesystem root, then `.` when
/// [`PickerKind::ROOT_ROW`] asks for it. A hidden name is skipped unless
/// [`WalkOptions::show_hidden`] is set or it is one of the extra files.
pub fn walk_entries<K: PickerKind>(root: &Path, opts: &WalkOptions, kind: &K) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if root.parent().is_some() {
        out.push(PathBuf::from(".."));
    }
    if K::ROOT_ROW {
        out.push(PathBuf::from("."));
    }
    walk_dir(root, root, 0, opts, kind, &mut out);
    out
}

fn walk_dir<K: PickerKind>(
    root: &Path,
    dir: &Path,
    depth: usize,
    opts: &WalkOptions,
    kind: &K,
    out: &mut Vec<PathBuf>,
) {
    if depth > opts.max_depth {
        return;
    }
    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(e) => e.flatten().collect(),
        Err(_) => return,
    };
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let is_dir = dir_entry_is_dir(&entry, &path);
        let extra_file = !is_dir && opts.extra_file_names.iter().any(|e| e == name);
        if name.starts_with('.') && !opts.show_hidden && !extra_file {
            continue;
        }
        let listed = if is_dir {
            !opts.ignore_dirs.iter().any(|d| d == name)
        } else {
            extra_file || kind.lists_file(name)
        };
        if !listed {
            continue;
        }
        if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_path_buf());
        }
        if is_dir {
            walk_dir(root, &path, depth + 1, opts, kind, out);
        }
    }
}

// ── Widget ────────────────────────────────────────────────────────────────────

/// What sets one filesystem picker apart from another: the
/// [`PathPicker`] behind both [`crate::compose::FolderPicker`] and
/// [`crate::compose::FilePicker`] asks its kind these questions and
/// shares everything else.
pub trait PickerKind: std::fmt::Debug + Clone + PartialEq + Eq + 'static {
    /// The picker's default [`WidgetId`].
    const DEFAULT_ID: &'static str;
    /// Whether the walk lists a `.` row naming the browsing root itself.
    const ROOT_ROW: bool;

    /// Whether the walk lists the file `name`. Directories are always
    /// listed, so the user can browse through them.
    fn lists_file(&self, name: &str) -> bool;

    /// Whether an unmodified `-`/`j`/`k` browses up / moves the highlight
    /// instead of typing into the query.
    fn vim_keys(&self) -> bool;

    /// The title verb, shown before the browsing root.
    fn title(&self) -> &'static str;

    /// The icon of a row that is not a directory.
    fn file_icon(&self) -> Icon;

    /// The palette's create row for `query`, if this kind offers one.
    fn create_label(&self, query: &str) -> Option<String>;

    /// What Enter means for `picker` right now.
    fn confirm(picker: &PathPicker<Self>) -> Handled<PathPickerAction>;
}

/// A filesystem picker, painted as a [`Palette`].
///
/// [`Self::paths`] is the visible browsing model and belongs to the host,
/// which applies the [`PathPickerAction`]s the picker reports through
/// [`Self::apply`]; [`Self::walk`] decides what a re-walk surfaces and
/// [`Self::kind`] what the rows mean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathPicker<K: PickerKind> {
    /// The picker's id, also the painted palette's.
    pub id: WidgetId,
    /// What this picker browses for.
    pub kind: K,
    /// Root, query, entries, highlight and scroll.
    pub paths: PathPickerState,
    /// What the walk surfaces.
    pub walk: WalkOptions,
}

/// [`Interactive::State`] for [`PathPicker`]: the composed palette's own
/// editing state (the query caret and selection anchor).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PathPickerEditState {
    /// The composed palette's state.
    pub palette: PaletteState,
}

impl<K: PickerKind> PathPicker<K> {
    /// A `kind` picker rooted at `root`, walked with the default
    /// [`WalkOptions`].
    pub fn with_kind(kind: K, root: impl Into<PathBuf>) -> Self {
        let mut picker = Self {
            id: WidgetId::new(K::DEFAULT_ID),
            kind,
            paths: PathPickerState::new(root.into(), Vec::new()),
            walk: WalkOptions::default(),
        };
        picker.rewalk();
        picker
    }

    /// Override the picker's [`WidgetId`]. Returns `self` for
    /// builder-style chaining.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = WidgetId::new(id.into());
        self
    }

    /// Surface (or hide) hidden entries, re-walking the current root.
    /// Returns `self` for builder-style chaining.
    pub fn with_show_hidden(mut self, show_hidden: bool) -> Self {
        self.walk.show_hidden = show_hidden;
        self.rewalk();
        self
    }

    /// Replace the always-surfaced file names, re-walking the current
    /// root. Returns `self` for builder-style chaining.
    pub fn with_extra_file_names(mut self, names: Vec<String>) -> Self {
        self.walk.extra_file_names = names;
        self.rewalk();
        self
    }

    /// Replace the skipped directory names, re-walking the current root.
    /// Pass an empty `Vec` to walk every directory (use with care on large
    /// filesystems). Returns `self` for builder-style chaining.
    pub fn with_ignore_dirs(mut self, ignore_dirs: Vec<String>) -> Self {
        self.walk.ignore_dirs = ignore_dirs;
        self.rewalk();
        self
    }

    /// Replace the walk's recursion depth, re-walking the current root.
    /// Returns `self` for builder-style chaining.
    pub fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.walk.max_depth = max_depth;
        self.rewalk();
        self
    }

    /// Re-walk the current root and refilter, keeping the query.
    fn rewalk(&mut self) {
        let entries = walk_entries(&self.paths.root, &self.walk, &self.kind);
        self.paths.set_all_entries(entries);
    }

    /// Browse `new_root`: clear the query and re-walk.
    pub fn navigate_to(&mut self, new_root: PathBuf) {
        let entries = walk_entries(&new_root, &self.walk, &self.kind);
        self.paths.navigate_to(new_root, entries);
    }

    /// Whether row `rel` is a directory (`..` and `.` always are).
    pub fn is_dir_entry(&self, rel: &Path) -> bool {
        rel.as_os_str() == ".." || rel.as_os_str() == "." || self.paths.root.join(rel).is_dir()
    }

    /// The absolute path a row resolves to: `..` is the root's parent,
    /// anything else is `root.join(rel)`.
    fn resolve(&self, rel: &Path) -> Option<PathBuf> {
        if rel.as_os_str() == ".." {
            self.paths.parent()
        } else {
            Some(self.paths.root.join(rel))
        }
    }

    /// The resolved absolute path for the highlighted row, or `None` if
    /// the list is empty.
    pub fn selected_path(&self) -> Option<PathBuf> {
        self.resolve(self.paths.highlighted()?)
    }

    /// What Enter means right now ([`PickerKind::confirm`]). With nothing
    /// to confirm it is [`Handled::Consumed`] for every kind: the picker is
    /// modal, so Enter never falls through to the view behind it.
    pub fn confirm(&self) -> Handled<PathPickerAction> {
        K::confirm(self)
    }

    /// Apply a browsing change this picker reported: a query edit
    /// refilters, a highlight move scrolls it into a `visible_rows`-tall
    /// list, and a navigation re-walks the new root. A confirmation or a
    /// dismissal changes nothing here.
    pub fn apply(&mut self, action: &PathPickerAction, visible_rows: usize) {
        match action {
            PathPickerAction::QueryChanged { query } => self.paths.set_query(query.clone()),
            PathPickerAction::SelectionChanged { idx } => {
                self.paths.select(*idx);
                self.paths.sync_scroll(visible_rows);
            }
            PathPickerAction::Navigated { root } => self.navigate_to(root.clone()),
            _ => {}
        }
    }

    /// Resolve one raw [`UiEvent`] through this picker's own
    /// [`Interactive`], laid out on a cell grid tall enough for
    /// `visible_rows` list rows, for a host that is not on the runtime's
    /// routing engine.
    ///
    /// The editing state lives where the runtime keeps it — the
    /// per-thread widget-state store, under [`Self::id`] — so a `draw_dyn`
    /// paint shows the caret this moved. A confirmation or a dismissal
    /// drops that state, so the next picker under the same id starts
    /// fresh. The host applies a reported browsing change with
    /// [`Self::apply`].
    ///
    /// Query characters come from [`UiEvent::KeyPressed`]'s `Key::Char`
    /// only, never from [`UiEvent::CharTyped`]: a backend that emits both
    /// for one keystroke would otherwise type it twice. Every event other
    /// than a key press is [`Handled::Ignored`].
    pub fn handle_event(&self, event: &UiEvent, visible_rows: usize) -> Handled<PathPickerAction> {
        use crate::widget_state::with_widget_states;
        let UiEvent::KeyPressed { .. } = event else {
            return Handled::Ignored;
        };
        let mut state = with_widget_states(|states| {
            states
                .get(&self.id)
                .and_then(|s| s.downcast_ref::<PathPickerEditState>())
                .copied()
                .unwrap_or_default()
        });
        let rect = Rect::new(0.0, 0.0, 80.0, (visible_rows + PALETTE_CHROME_ROWS) as f32);
        let handled = widget_host::drive_in_cells(self, &mut state, event, rect)
            .map_or(Handled::Ignored, |driven| driven.handled);
        with_widget_states(|states| {
            if matches!(
                handled,
                Handled::Event(PathPickerAction::Confirmed { .. } | PathPickerAction::Cancelled)
            ) {
                states.remove(&self.id);
            } else if let Some(slot) = states
                .state_for(self.id, self)
                .downcast_mut::<PathPickerEditState>()
            {
                *slot = state;
            }
        });
        handled
    }

    /// The [`Palette`] this picker paints in a rect `width` units wide,
    /// with the query caret at `caret`.
    pub fn palette(&self, width: f32, caret: usize) -> Palette {
        let title = format!("{} {}", self.kind.title(), self.paths.root_display(width));

        // Codicon glyph on a GUI backend (`nerd_fonts_enabled` defaults
        // `true` there), falling back to the emoji on TUI
        // (`nerd_fonts_enabled` defaults `false`).
        let folder_icon = Icon::codicon("folder", "\u{1F4C1}")
            .unwrap_or_else(|| Icon::new("\u{1F4C1}", "\u{1F4C1}")); // 📁
        let file_icon = self.kind.file_icon();

        let items: Vec<PaletteItem> = self
            .paths
            .filtered
            .iter()
            .map(|entry| {
                let icon = if self.is_dir_entry(entry) {
                    folder_icon.clone()
                } else {
                    file_icon.clone()
                };
                PaletteItem {
                    text: StyledText::plain(entry.to_string_lossy().to_string()),
                    detail: None,
                    icon: Some(icon),
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
            query: self.paths.query.clone(),
            query_cursor: caret,
            items,
            selected_idx: self.paths.selected,
            scroll_offset: self.paths.scroll_top,
            total_count: self.paths.all_entries.len(),
            has_focus: true,
            show_query: true,
            create_label: self.kind.create_label(&self.paths.query),
            preview: None,
            mode: crate::primitives::palette::PaletteMode::List,
        }
    }

    /// The palette as painted for `layout`, with `state`'s caret.
    fn painted_palette(
        &self,
        layout: &PalettePrimitiveLayout,
        state: Option<&PaletteState>,
    ) -> Palette {
        self.palette(layout.area.width, painted_caret(&self.paths.query, state))
    }
}

impl<K: PickerKind> A11yPrimitive for PathPicker<K> {
    fn a11y_info(&self) -> A11yInfo {
        self.palette(0.0, self.paths.query.len()).a11y_info()
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        self.palette(0.0, self.paths.query.len()).a11y_children()
    }
}

impl<K: PickerKind> Primitive for PathPicker<K> {
    type Layout = PalettePrimitiveLayout;
    type Event = PathPickerAction;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn paint_bleed(&self, style: &crate::Style) -> f32 {
        Primitive::paint_bleed(&self.palette(0.0, 0), style)
    }

    fn measure(&self, ctx: &LayoutCtx, avail: Size) -> crate::NaturalSize {
        Primitive::measure(&self.palette(avail.width, 0), ctx, avail)
    }

    /// The palette's own layout for `rect`.
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> PalettePrimitiveLayout {
        Primitive::layout(&self.palette(rect.width, self.paths.query.len()), ctx, rect)
    }

    fn paint(
        &self,
        layout: &PalettePrimitiveLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        let state = env
            .widget_state_as::<PathPickerEditState>()
            .map(|s| &s.palette);
        Primitive::paint(&self.painted_palette(layout, state), layout, surface, env);
    }

    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &PalettePrimitiveLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        let state = env
            .widget_state_as::<PathPickerEditState>()
            .map(|s| &s.palette);
        Primitive::paint_cells(&self.painted_palette(layout, state), layout, cells, env);
    }
}

impl<K: PickerKind> Interactive for PathPicker<K> {
    type State = PathPickerEditState;
    type Action = PathPickerAction;

    /// One stop: the palette is modal, so keys go to it as a whole.
    fn focus(&self) -> FocusSpec {
        FocusSpec::Stop
    }

    fn sub_regions(&self, layout: &PalettePrimitiveLayout) -> Vec<(Rect, SubTarget)> {
        Interactive::sub_regions(&self.palette(layout.area.width, 0), layout)
    }

    /// The vim shortcuts follow [`PickerKind::vim_keys`].
    fn handle(
        &self,
        state: &mut PathPickerEditState,
        event: &RoutedEvent,
        layout: &PalettePrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<PathPickerAction> {
        let palette = self.painted_palette(layout, Some(&state.palette));
        handle_picker_event(
            &self.paths,
            &palette,
            &mut state.palette,
            event,
            layout,
            cx,
            self.kind.vim_keys(),
            || self.confirm(),
        )
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Test-only host loop: apply a reported browsing change the way an app
/// would and report it as [`Handled::Consumed`], leaving a confirmation or
/// a dismissal for the caller.
#[cfg(test)]
pub(crate) fn settle<K: PickerKind>(
    picker: &mut PathPicker<K>,
    handled: Handled<PathPickerAction>,
    visible_rows: usize,
) -> Handled<PathPickerAction> {
    match handled {
        Handled::Event(
            action @ (PathPickerAction::Confirmed { .. } | PathPickerAction::Cancelled),
        ) => Handled::Event(action),
        Handled::Event(action) => {
            picker.apply(&action, visible_rows);
            Handled::Consumed
        }
        other => other,
    }
}

/// Test-only: [`PathPicker::handle_event`] for `event`, then [`settle`].
#[cfg(test)]
pub(crate) fn drive<K: PickerKind>(
    picker: &mut PathPicker<K>,
    event: &UiEvent,
    visible_rows: usize,
) -> Handled<PathPickerAction> {
    let handled = picker.handle_event(event, visible_rows);
    settle(picker, handled, visible_rows)
}

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
