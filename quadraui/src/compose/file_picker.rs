//! [`FilePicker`] — a file open/save picker on the widget contract, plus
//! [`FilePickerController`], its thin event-driven wrapper.
//!
//! Modelled on [`crate::compose::FolderPicker`] — same fuzzy filter and
//! the same [`PathPickerState`] browsing model, composed from the same
//! [`Palette`] — but browses **files**, not just directories, and adds a
//! save mode where the typed query doubles as the destination filename.
//!
//! `TuiPlatformServices::show_file_open_dialog` / `show_file_save_dialog`
//! drive the controller through a nested draw-and-read loop (see
//! `crate::tui::services`' module doc) so both methods return a real path
//! on TUI too — see `examples/tui_file_picker.rs` for the controller used
//! directly by an app.
//!
//! # Shares its key map with the folder picker — mode-aware
//!
//! Both pickers route keys through the same palette input, so they never
//! drift apart on the keys they share. The vim-style `-`/`j`/`k`
//! shortcuts follow the mode:
//!
//! - [`FilePickerMode::Open`]: `-`/`j`/`k` navigate (parent-dir/up/down),
//!   matching the folder picker, because there is nothing to type a name
//!   into.
//! - [`FilePickerMode::Save`]: `-`/`j`/`k` type into the query like any
//!   other character, because the query *is* the destination filename
//!   there (see "Save-mode semantics" below) and real filenames routinely
//!   contain all three (`my-notes.txt`, `journal.md`, `Makefile-k8s`).
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
//! match picker.handle_event(&event, visible_rows) {
//!     Handled::Event(PathPickerAction::Confirmed { path }) => { /* open/save `path` */ }
//!     Handled::Event(PathPickerAction::Cancelled)          => { /* dismiss modal */ }
//!     Handled::Consumed                                    => { /* redraw */ }
//!     _                                                    => {}
//! }
//! ```

use std::path::{Path, PathBuf};

use super::path_picker::{
    dir_entry_is_dir, handle_picker_event, painted_caret, PathPickerAction, PathPickerState,
};
use crate::a11y::{A11yChild, A11yInfo, A11yPrimitive};
use crate::compose::folder_picker::{drive_picker, DEFAULT_IGNORE_DIRS, DEFAULT_MAX_DEPTH};
use crate::compose::Handled;
use crate::interactive::{EventCx, FocusSpec, Interactive, RoutedEvent, SubTarget};
use crate::primitive::{LayoutCtx, Primitive, Size};
use crate::primitives::palette::{PalettePrimitiveLayout, PaletteState};
use crate::{Backend, Icon, Palette, PaletteItem, Rect, StyledText, UiEvent, WidgetId};

/// Whether a [`FilePicker`] is choosing a file to open (must exist) or a
/// destination to save to (may not exist yet).
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
#[deprecated(
    since = "0.9.0",
    note = "use `FilePickerController::handle_event`'s `Handled<PathPickerAction>` instead — `Consumed`/`Ignored` are `Handled::Consumed`/`Handled::Ignored`, `Confirmed`/`Cancelled` move to `PathPickerAction` under `Handled::Event`"
)]
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

// ── Widget ────────────────────────────────────────────────────────────────

/// A file open/save picker, painted as a [`Palette`].
///
/// [`Self::paths`] is the visible browsing model and belongs to the host,
/// which applies the [`PathPickerAction`]s the picker reports through
/// [`Self::apply`]; the walk options decide what a re-walk surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePicker {
    /// The picker's id, also the painted palette's.
    pub id: WidgetId,
    /// Open or Save.
    pub mode: FilePickerMode,
    /// Root, query, entries, highlight and scroll.
    pub paths: PathPickerState,
    /// Extension filters — `(display_name, &[ext])` pairs, mirroring
    /// [`crate::backend::FileDialogOptions::filters`]. Empty means "show
    /// every file". Directories are never filtered out.
    pub filters: Vec<(String, Vec<String>)>,
    /// Whether to surface hidden entries (names beginning with `.`).
    pub show_hidden: bool,
    /// Directory names skipped by the walk.
    pub ignore_dirs: Vec<String>,
    /// Maximum recursion depth for the walk.
    pub max_depth: usize,
}

/// [`Interactive::State`] for [`FilePicker`]: the composed palette's own
/// editing state (the query caret and selection anchor).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FilePickerState {
    /// The composed palette's state.
    pub palette: PaletteState,
}

impl FilePicker {
    /// A picker rooted at `root`.
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
        let mut picker = Self {
            id: WidgetId::new("file_picker"),
            mode,
            paths: PathPickerState::new(root.into(), Vec::new()),
            filters,
            show_hidden: false,
            ignore_dirs: DEFAULT_IGNORE_DIRS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            max_depth: DEFAULT_MAX_DEPTH,
        };
        let entries = picker.walk(&picker.paths.root);
        picker.paths = PathPickerState::new(picker.paths.root.clone(), entries);
        picker
    }

    /// Seed the query (the Save-mode destination filename), mirroring
    /// [`crate::backend::FileDialogOptions::initial_filename`]. Returns
    /// `self` for builder-style chaining.
    pub fn with_initial_filename(mut self, name: impl Into<String>) -> Self {
        self.paths.query = name.into();
        self
    }

    /// Override the picker's [`WidgetId`]. Returns `self` for
    /// builder-style chaining.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = WidgetId::new(id.into());
        self
    }

    /// `root`'s entries under this picker's walk options.
    fn walk(&self, root: &Path) -> Vec<PathBuf> {
        collect_file_entries(
            root,
            self.show_hidden,
            &self.ignore_dirs,
            &self.filters,
            self.max_depth,
        )
    }

    /// Whether row `rel` is a directory (`..` always is).
    fn is_dir_entry(&self, rel: &Path) -> bool {
        rel.as_os_str() == ".." || self.paths.root.join(rel).is_dir()
    }

    /// Browse `new_root`: clear the query and re-walk.
    pub fn navigate_to(&mut self, new_root: PathBuf) {
        let entries = self.walk(&new_root);
        self.paths.navigate_to(new_root, entries);
    }

    /// What Enter means right now.
    ///
    /// The ".." row needs special-casing beyond "it's a directory": it's
    /// the row `selected` defaults to (index 0) whenever the entry list
    /// hasn't been touched, so treating it exactly like any other
    /// directory would mean a Save dialog opened with a pre-filled
    /// [`Self::with_initial_filename`] and confirmed with an *immediate*
    /// Enter — no arrowing, no typing — would silently navigate up a
    /// directory instead of saving. A typed/seeded Save-mode filename
    /// takes priority over that unmoved default; explicitly arrowing onto
    /// a *real* directory row still browses into it regardless of mode or
    /// query, matching native save dialogs' "Enter on a folder row
    /// browses" convention.
    pub fn confirm(&self) -> Handled<PathPickerAction> {
        let highlighted = self.paths.highlighted();
        let save_as = || {
            Handled::Event(PathPickerAction::Confirmed {
                path: self.paths.root.join(&self.paths.query),
            })
        };

        if highlighted.is_some_and(|p| p.as_os_str() == "..") {
            if self.mode == FilePickerMode::Save && !self.paths.query.is_empty() {
                return save_as();
            }
            return match self.paths.parent() {
                Some(root) => Handled::Event(PathPickerAction::Navigated { root }),
                None => Handled::Consumed,
            };
        }

        if let Some(rel) = highlighted {
            if self.is_dir_entry(rel) {
                return Handled::Event(PathPickerAction::Navigated {
                    root: self.paths.root.join(rel),
                });
            }
        }

        match self.mode {
            FilePickerMode::Open => match highlighted {
                Some(rel) => Handled::Event(PathPickerAction::Confirmed {
                    path: self.paths.root.join(rel),
                }),
                None => Handled::Ignored,
            },
            FilePickerMode::Save if self.paths.query.is_empty() => Handled::Ignored,
            FilePickerMode::Save => save_as(),
        }
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

    /// The [`Palette`] this picker paints in a rect `width` units wide,
    /// with the query caret at `caret`.
    pub fn palette(&self, width: f32, caret: usize) -> Palette {
        let verb = match self.mode {
            FilePickerMode::Open => "Open File",
            FilePickerMode::Save => "Save File",
        };
        let title = format!("{verb} {}", self.paths.root_display(width));

        // Codicon glyph on a GUI backend (`nerd_fonts_enabled` defaults
        // `true` there), falling back to the emoji on TUI
        // (`nerd_fonts_enabled` defaults `false`).
        let folder_icon = Icon::codicon("folder", "\u{1F4C1}")
            .unwrap_or_else(|| Icon::new("\u{1F4C1}", "\u{1F4C1}")); // 📁
        let file_icon = Icon::codicon("file", "\u{1F4C4}")
            .unwrap_or_else(|| Icon::new("\u{1F4C4}", "\u{1F4C4}")); // 📄

        let items: Vec<PaletteItem> = self
            .paths
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
            FilePickerMode::Save if !self.paths.query.is_empty() => {
                Some(format!("Save as \"{}\"", self.paths.query))
            }
            _ => None,
        };

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
            create_label,
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

impl A11yPrimitive for FilePicker {
    fn a11y_info(&self) -> A11yInfo {
        self.palette(0.0, self.paths.query.len()).a11y_info()
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        self.palette(0.0, self.paths.query.len()).a11y_children()
    }
}

impl Primitive for FilePicker {
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
        let state = env.widget_state_as::<FilePickerState>().map(|s| &s.palette);
        Primitive::paint(&self.painted_palette(layout, state), layout, surface, env);
    }

    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &PalettePrimitiveLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        let state = env.widget_state_as::<FilePickerState>().map(|s| &s.palette);
        Primitive::paint_cells(&self.painted_palette(layout, state), layout, cells, env);
    }
}

impl Interactive for FilePicker {
    type State = FilePickerState;
    type Action = PathPickerAction;

    /// One stop: the palette is modal, so keys go to it as a whole.
    fn focus(&self) -> FocusSpec {
        FocusSpec::Stop
    }

    fn sub_regions(&self, layout: &PalettePrimitiveLayout) -> Vec<(Rect, SubTarget)> {
        Interactive::sub_regions(&self.palette(layout.area.width, 0), layout)
    }

    /// `Open` mode's query is never a filename, so the vim shortcuts take
    /// priority there; in `Save` mode they type.
    fn handle(
        &self,
        state: &mut FilePickerState,
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
            self.mode == FilePickerMode::Open,
            || self.confirm(),
        )
    }
}

// ── Controller ────────────────────────────────────────────────────────────

/// Event-driven wrapper over a [`FilePicker`] and its state, for a host
/// that calls it with raw [`UiEvent`]s: it applies every browsing change
/// itself and reports only a confirmation or a dismissal.
///
/// See the [module-level documentation](self) for a usage example.
pub struct FilePickerController {
    widget: FilePicker,
    state: FilePickerState,
}

impl FilePickerController {
    /// Create a controller over [`FilePicker::new`]'s picker.
    pub fn new(
        mode: FilePickerMode,
        root: impl Into<PathBuf>,
        filters: Vec<(String, Vec<String>)>,
    ) -> Self {
        Self {
            widget: FilePicker::new(mode, root, filters),
            state: FilePickerState::default(),
        }
    }

    /// See [`FilePicker::with_initial_filename`].
    pub fn with_initial_filename(mut self, name: impl Into<String>) -> Self {
        self.widget = self.widget.with_initial_filename(name);
        self
    }

    /// Override the `WidgetId` used by the rendered `Palette`. Defaults
    /// to `WidgetId::new("file_picker")`. Returns `self` for
    /// builder-style chaining.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.widget = self.widget.with_id(id);
        self
    }

    /// The [`FilePicker`] widget this controller drives.
    pub fn widget(&self) -> &FilePicker {
        &self.widget
    }

    // ── State accessors ───────────────────────────────────────────────

    pub fn mode(&self) -> FilePickerMode {
        self.widget.mode
    }

    pub fn root(&self) -> &Path {
        &self.widget.paths.root
    }

    pub fn query(&self) -> &str {
        &self.widget.paths.query
    }

    pub fn filtered(&self) -> &[PathBuf] {
        &self.widget.paths.filtered
    }

    pub fn selected(&self) -> usize {
        self.widget.paths.selected
    }

    pub fn scroll_top(&self) -> usize {
        self.widget.paths.scroll_top
    }

    // ── Navigation ────────────────────────────────────────────────────

    /// Navigate to `new_root`, clearing the query and reloading entries.
    pub fn navigate_to(&mut self, new_root: PathBuf) {
        self.widget.navigate_to(new_root);
        self.state.palette = PaletteState::default();
    }

    /// Navigate to the parent directory (no-op at filesystem root) — the
    /// `-` key in `Open` mode.
    pub fn navigate_up(&mut self) {
        if let Some(parent) = self.widget.paths.parent() {
            self.navigate_to(parent);
        }
    }

    // ── Query editing ─────────────────────────────────────────────────

    pub fn push_char(&mut self, c: char) {
        self.widget.paths.push_char(c);
        self.state.palette = PaletteState::default();
    }

    pub fn pop_char(&mut self) {
        self.widget.paths.pop_char();
        self.state.palette = PaletteState::default();
    }

    /// Re-compute `filtered` from `all_entries` and the current `query`.
    pub fn refilter(&mut self) {
        self.widget.paths.refilter();
    }

    // ── Selection movement ────────────────────────────────────────────

    pub fn move_up(&mut self) {
        self.widget.paths.move_up();
    }

    pub fn move_down(&mut self) {
        self.widget.paths.move_down();
    }

    pub fn sync_scroll(&mut self, visible_rows: usize) {
        self.widget.paths.sync_scroll(visible_rows);
    }

    // ── Render ────────────────────────────────────────────────────────

    /// The palette as painted in `rect`.
    fn build_palette(&self, rect: Rect) -> Palette {
        self.widget.palette(
            rect.width,
            painted_caret(&self.widget.paths.query, Some(&self.state.palette)),
        )
    }

    /// Paint the picker as a `Palette` inside `rect`.
    pub fn render(&self, rect: Rect, backend: &mut dyn Backend) {
        crate::backend::DrawsPrimitives::draw_primitive(backend, rect, &self.build_palette(rect));
    }

    // ── Handle ────────────────────────────────────────────────────────

    /// Apply one picker answer: a browsing change is applied here and
    /// reported as [`Handled::Consumed`]; a confirmation or a dismissal is
    /// the caller's.
    fn settle(
        &mut self,
        handled: Handled<PathPickerAction>,
        visible_rows: usize,
    ) -> Handled<PathPickerAction> {
        match handled {
            Handled::Event(
                action @ (PathPickerAction::Confirmed { .. } | PathPickerAction::Cancelled),
            ) => Handled::Event(action),
            Handled::Event(action) => {
                if matches!(action, PathPickerAction::Navigated { .. }) {
                    self.state.palette = PaletteState::default();
                }
                self.widget.apply(&action, visible_rows);
                Handled::Consumed
            }
            other => other,
        }
    }

    /// Enter's resolution, applied.
    #[cfg(test)]
    fn confirm_selection(&mut self) -> Handled<PathPickerAction> {
        let handled = self.widget.confirm();
        self.settle(handled, 0)
    }

    /// Drive the picker with a backend-neutral `UiEvent`.
    ///
    /// `visible_rows` is the number of list rows that fit inside the
    /// palette chrome; compute it as
    /// `popup_height.saturating_sub(PALETTE_CHROME_ROWS)` (see
    /// [`crate::compose::folder_picker::PALETTE_CHROME_ROWS`]).
    pub fn handle_event(
        &mut self,
        event: &UiEvent,
        visible_rows: usize,
    ) -> Handled<PathPickerAction> {
        let UiEvent::KeyPressed { .. } = event else {
            return Handled::Ignored;
        };
        let handled = drive_picker(&self.widget, &mut self.state, event, visible_rows);
        self.settle(handled, visible_rows)
    }

    /// Drive the state machine with a backend-neutral `UiEvent`.
    #[deprecated(
        since = "0.9.0",
        note = "use `handle_event`, which returns `Handled<PathPickerAction>` instead of `FilePickerEvent`"
    )]
    #[allow(deprecated)] // `FilePickerEvent` is `#[deprecated]`; this method is its own shim.
    pub fn handle(&mut self, event: &UiEvent, visible_rows: usize) -> FilePickerEvent {
        match self.handle_event(event, visible_rows) {
            Handled::Event(PathPickerAction::Confirmed { path }) => {
                FilePickerEvent::Confirmed { path }
            }
            Handled::Event(PathPickerAction::Cancelled) => FilePickerEvent::Cancelled,
            Handled::Event(_) | Handled::Consumed => FilePickerEvent::Consumed,
            Handled::Ignored => FilePickerEvent::Ignored,
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

/// Whether `name` passes `filters` — an empty `filters` list accepts
/// everything; otherwise `name`'s extension must match at least one
/// extension in at least one filter (via the same
/// [`crate::backend::normalize_filter_extension`] every backend's own
/// filter mapping runs its extensions through, so `".rs"`, `"rs"`, and
/// `"RS"` all mean the same filter here too), or a filter must carry the
/// wildcard `"*"`. The wildcard check runs on the *normalized* filter
/// entry, not the raw string a caller wrote, so `".*"` is a wildcard here
/// too — [`crate::backend::normalize_filter_extension`] strips every
/// leading dot before comparing, and a literal `".*"` extension would
/// never match a real file's extension (`Path::extension` never returns
/// `"*"`) otherwise.
fn extension_matches(filters: &[(String, Vec<String>)], name: &str) -> bool {
    if filters.is_empty() {
        return true;
    }
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(crate::backend::normalize_filter_extension)
        .unwrap_or_default();
    filters.iter().any(|(_, exts)| {
        exts.iter().any(|e| {
            let e = crate::backend::normalize_filter_extension(e);
            e == "*" || e == ext
        })
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Key, Modifiers, NamedKey};

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
        assert!(picker
            .widget
            .paths
            .all_entries
            .contains(&PathBuf::from("a.rs")));
        assert!(picker
            .widget
            .paths
            .all_entries
            .contains(&PathBuf::from("b.txt")));
        assert!(picker
            .widget
            .paths
            .all_entries
            .contains(&PathBuf::from("sub")));
    }

    /// A directory row's icon is `Icon::codicon("folder", "📁")` and a
    /// file row's is `Icon::codicon("file", "📄")` — the GUI glyph
    /// resolves to the real codicon, and the fallback is the pre-existing
    /// emoji, so a TUI host (`nerd_fonts_enabled` defaults `false`) still
    /// paints exactly what it painted before this glyph was added.
    #[test]
    fn row_icons_resolve_to_codicon_on_gui_and_emoji_fallback_on_tui() {
        let tmp = scratch_dir_with(&["a.rs"], &["sub"]);
        let picker = FilePickerController::new(FilePickerMode::Open, tmp.path(), vec![]);
        let rect = Rect::new(0.0, 0.0, 80.0, 24.0);
        let palette = picker.build_palette(rect);

        let file_item = palette
            .items
            .iter()
            .find(|i| i.text.spans.first().is_some_and(|s| s.text == "a.rs"))
            .expect("a.rs row present");
        let dir_item = palette
            .items
            .iter()
            .find(|i| i.text.spans.first().is_some_and(|s| s.text == "sub"))
            .expect("sub row present");

        let file_icon = file_item.icon.as_ref().expect("file row has an icon");
        let dir_icon = dir_item.icon.as_ref().expect("dir row has an icon");

        assert_eq!(file_icon.fallback, "\u{1F4C4}");
        assert_eq!(file_icon.resolve(false), "\u{1F4C4}");
        assert_eq!(file_icon.resolve(true), file_icon.glyph);
        assert_ne!(file_icon.glyph, "\u{1F4C4}");

        assert_eq!(dir_icon.fallback, "\u{1F4C1}");
        assert_eq!(dir_icon.resolve(false), "\u{1F4C1}");
        assert_eq!(dir_icon.resolve(true), dir_icon.glyph);
        assert_ne!(dir_icon.glyph, "\u{1F4C1}");
    }

    #[test]
    fn filters_restrict_files_not_dirs() {
        let tmp = scratch_dir_with(&["a.rs", "b.txt"], &["sub"]);
        let picker = FilePickerController::new(
            FilePickerMode::Open,
            tmp.path(),
            vec![("Rust".to_string(), vec!["rs".to_string()])],
        );
        assert!(picker
            .widget
            .paths
            .all_entries
            .contains(&PathBuf::from("a.rs")));
        assert!(!picker
            .widget
            .paths
            .all_entries
            .contains(&PathBuf::from("b.txt")));
        assert!(
            picker
                .widget
                .paths
                .all_entries
                .contains(&PathBuf::from("sub")),
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
        picker.widget.paths.selected = idx;
        let result = picker.confirm_selection();
        assert_eq!(
            result,
            Handled::Event(PathPickerAction::Confirmed {
                path: tmp.path().join("a.rs")
            })
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
        picker.widget.paths.selected = idx;
        let result = picker.confirm_selection();
        assert_eq!(result, Handled::Consumed);
        assert_eq!(picker.root(), tmp.path().join("sub"));
    }

    #[test]
    fn enter_on_dotdot_navigates_up() {
        let parent = tempfile::tempdir().expect("parent tempdir");
        let sub = parent.path().join("child");
        std::fs::create_dir_all(&sub).expect("mkdir child");
        let mut picker = FilePickerController::new(FilePickerMode::Open, sub.clone(), vec![]);
        picker.widget.paths.selected = 0;
        assert_eq!(picker.filtered().first(), Some(&PathBuf::from("..")));
        let result = picker.confirm_selection();
        assert_eq!(result, Handled::Consumed);
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
            Handled::Event(PathPickerAction::Confirmed {
                path: tmp.path().join("new")
            })
        );
    }

    #[test]
    fn save_mode_enter_with_empty_query_and_no_selection_is_ignored() {
        let tmp = scratch_dir_with(&[], &[]);
        let mut picker = FilePickerController::new(FilePickerMode::Save, tmp.path(), vec![]);
        // No entries at all (empty dir, no "..": tmp roots have a parent,
        // so ".." is still present) — clear it to hit the empty-query,
        // no-highlighted-row path directly.
        picker.widget.paths.filtered.clear();
        let result = picker.confirm_selection();
        assert_eq!(result, Handled::Ignored);
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
            Handled::Event(PathPickerAction::Confirmed {
                path: sub.join("untitled.txt")
            })
        );
        assert_eq!(picker.root(), sub, "must not have navigated");
    }

    #[test]
    fn save_mode_enter_on_dir_navigates_not_confirms() {
        let tmp = scratch_dir_with(&[], &["sub"]);
        let mut picker = FilePickerController::new(FilePickerMode::Save, tmp.path(), vec![]);
        picker.push_char('x'); // non-empty query — would confirm if not for the dir row
        let idx = picker
            .widget
            .paths
            .all_entries
            .iter()
            .position(|p| p == &PathBuf::from("sub"))
            .expect("sub listed");
        // Force-select the dir row directly (typing "x" filtered it out of
        // `filtered`, since "sub" doesn't fuzzy-match "x"); this test only
        // cares about `confirm_selection`'s directory-vs-file branch, so
        // bypass the filter machinery.
        picker.widget.paths.filtered = vec![picker.widget.paths.all_entries[idx].clone()];
        picker.widget.paths.selected = 0;
        let result = picker.confirm_selection();
        assert_eq!(result, Handled::Consumed);
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
        assert_eq!(
            picker.handle_event(&ev, 10),
            Handled::Event(PathPickerAction::Cancelled)
        );
    }

    // macOS delivers a Cmd-modified letter as a plain `Key::Char` with
    // `modifiers.cmd` set, the same shape Ctrl uses on every other
    // backend, so it must not be typed into the filter query.
    #[test]
    fn cmd_char_is_ignored_not_typed_into_query() {
        let tmp = scratch_dir_with(&["a.rs"], &[]);
        let mut picker = FilePickerController::new(FilePickerMode::Open, tmp.path(), vec![]);
        let ev = UiEvent::KeyPressed {
            key: Key::Char('f'),
            modifiers: Modifiers {
                cmd: true,
                ..Default::default()
            },
            repeat: false,
        };
        assert_eq!(picker.handle_event(&ev, 10), Handled::Ignored);
        assert_eq!(picker.query(), "");
    }

    // ── Open mode: shared vim-key navigation (unified with FolderPickerController) ─

    #[test]
    fn vim_j_and_k_move_selection_through_handle() {
        let tmp = scratch_dir_with(&["a.rs", "b.rs"], &[]);
        let mut picker = FilePickerController::new(FilePickerMode::Open, tmp.path(), vec![]);
        assert!(picker.filtered().len() >= 2);

        let down = UiEvent::KeyPressed {
            key: Key::Char('j'),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        assert_eq!(picker.handle_event(&down, 10), Handled::Consumed);
        assert_eq!(picker.selected(), 1);

        let up = UiEvent::KeyPressed {
            key: Key::Char('k'),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        assert_eq!(picker.handle_event(&up, 10), Handled::Consumed);
        assert_eq!(picker.selected(), 0);
    }

    #[test]
    fn dash_key_navigates_up() {
        let parent = tempfile::tempdir().expect("parent tempdir");
        let sub = parent.path().join("dash_nav_child");
        std::fs::create_dir_all(&sub).expect("mkdir child");
        let mut picker = FilePickerController::new(FilePickerMode::Open, sub.clone(), vec![]);
        let ev = UiEvent::KeyPressed {
            key: Key::Char('-'),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        let result = picker.handle_event(&ev, 10);
        assert_eq!(result, Handled::Consumed);
        assert_eq!(picker.root(), parent.path());
    }

    // ── Save mode: vim shortcuts disabled, typing wins ──────────────────

    /// In `Save` mode, `-`/`j`/`k` must reach the query through the full
    /// `handle()` path (not just `push_char` directly) — Save's query is
    /// the destination filename, so routing them as navigation would make
    /// `journal.md`, `my-notes.txt`, and `Makefile-k8s` untypeable and
    /// `-` actively destructive (it would call `navigate_up`, which
    /// clears the query the user had already typed).
    #[test]
    fn save_mode_vim_chars_type_into_query_through_handle() {
        let tmp = scratch_dir_with(&[], &[]);
        let mut picker = FilePickerController::new(FilePickerMode::Save, tmp.path(), vec![]);

        for c in "my-journal.md".chars() {
            let ev = UiEvent::KeyPressed {
                key: Key::Char(c),
                modifiers: Modifiers::default(),
                repeat: false,
            };
            assert_eq!(picker.handle_event(&ev, 10), Handled::Consumed);
        }

        assert_eq!(picker.query(), "my-journal.md");
        assert_eq!(picker.root(), tmp.path(), "must not have navigated up");
    }

    /// Named arrow keys still navigate the selection in Save mode — only
    /// the vim letter/dash aliases are gated, not `Up`/`Down` themselves.
    #[test]
    fn save_mode_arrow_keys_still_navigate_selection() {
        let tmp = scratch_dir_with(&["a.txt", "b.txt"], &[]);
        let mut picker = FilePickerController::new(FilePickerMode::Save, tmp.path(), vec![]);
        assert!(picker.filtered().len() >= 2);

        let down = UiEvent::KeyPressed {
            key: Key::Named(NamedKey::Down),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        assert_eq!(picker.handle_event(&down, 10), Handled::Consumed);
        assert_eq!(picker.selected(), 1);
    }

    #[test]
    fn fuzzy_match_finds_kube_system_style_entry() {
        // "kbs" as a non-contiguous subsequence should surface a
        // "kube-system"-shaped entry, the same ranking both pickers share
        // via `path_picker::filter_entries`.
        let tmp = scratch_dir_with(&[], &["kube-system"]);
        let mut picker = FilePickerController::new(FilePickerMode::Open, tmp.path(), vec![]);
        picker.push_char('k');
        picker.push_char('b');
        picker.push_char('s');
        assert!(
            picker
                .filtered()
                .iter()
                .any(|p| p == &PathBuf::from("kube-system")),
            "\"kbs\" should fuzzy-match \"kube-system\""
        );
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
            picker
                .widget
                .paths
                .all_entries
                .contains(&PathBuf::from("link")),
            "a symlink to a directory must bypass the file extension filter, \
             the same as a real directory"
        );

        let idx = picker
            .filtered()
            .iter()
            .position(|p| p == &PathBuf::from("link"))
            .expect("link listed");
        picker.widget.paths.selected = idx;
        assert_eq!(picker.confirm_selection(), Handled::Consumed);
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

    /// `".*"` is a wildcard too, not just the bare `"*"` — the normalized
    /// comparison in [`extension_matches`] strips its leading dot before
    /// checking for the wildcard, so a filter written either way accepts
    /// everything.
    #[test]
    fn extension_matches_treats_a_dotted_wildcard_as_a_wildcard_too() {
        assert!(extension_matches(
            &[("Any".to_string(), vec![".*".to_string()])],
            "whatever.zzz"
        ));
    }
}
