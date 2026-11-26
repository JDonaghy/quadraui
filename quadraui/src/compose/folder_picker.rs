//! [`FolderPicker`] — a directory-browsing picker on the widget contract,
//! plus [`FolderPickerController`], its thin event-driven wrapper.
//!
//! The picker composes [`Palette`]: it paints through the palette's own
//! [`Primitive`] and routes input through the palette's [`Interactive`],
//! which owns query editing and list navigation. What the picker adds is
//! the filesystem: walking, fuzzy filtering, and what Enter on a row
//! means.
//!
//! # Relation to vimcode
//!
//! Extracted from vimcode's TUI-local `FolderPickerState`
//! (`src/tui_main/mod.rs`, ~285 lines). vimcode's TUI and GTK
//! `OpenFolderDialog` are the migration target.
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
//!     _                                                    => {},
//! }
//! ```
//!
//! An app on the runtime's routing engine pushes the [`FolderPicker`]
//! itself as a `Surface::Widget`; its actions arrive as
//! [`UiEvent::PathPicker`], and the app hands each to
//! [`FolderPicker::apply`].
//!
//! # Scroll accounting
//!
//! `handle_event()` requires `visible_rows` — the number of list rows
//! visible inside the palette chrome (title row + query row + optional
//! scrollbar row). Consumers typically compute this as
//! `rect.height as usize - PALETTE_CHROME_ROWS` where
//! [`PALETTE_CHROME_ROWS`] = 4.

use std::path::{Path, PathBuf};

use super::path_picker::{
    dir_entry_is_dir, handle_picker_event, painted_caret, PathPickerAction, PathPickerState,
};
use crate::a11y::{A11yChild, A11yInfo, A11yPrimitive};
use crate::compose::widget_host;
use crate::compose::Handled;
use crate::interactive::{EventCx, FocusSpec, Interactive, RoutedEvent, SubTarget};
use crate::primitive::{LayoutCtx, Primitive, Size};
use crate::primitives::palette::{PalettePrimitiveLayout, PaletteState};
use crate::{Backend, Icon, Palette, PaletteItem, Rect, StyledText, UiEvent, WidgetId};

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

/// Default recursion depth for [`FolderPicker::rooted_at`].
///
/// Override per-instance with [`FolderPicker::with_max_depth`].
pub const DEFAULT_MAX_DEPTH: usize = 5;

/// Default heavy build/dependency directory names that are skipped during
/// the filesystem walk.
///
/// Override per-instance with [`FolderPicker::with_ignore_dirs`].
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

// ── Widget ────────────────────────────────────────────────────────────────

/// A directory-browsing picker, painted as a [`Palette`].
///
/// [`Self::paths`] is the visible browsing model and belongs to the host,
/// which applies the [`PathPickerAction`]s the picker reports through
/// [`Self::apply`]; the walk options decide what a re-walk surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderPicker {
    /// The picker's id, also the painted palette's.
    pub id: WidgetId,
    /// Root, query, entries, highlight and scroll.
    pub paths: PathPickerState,
    /// Whether to surface hidden directories (names beginning with `.`).
    pub show_hidden: bool,
    /// Additional dotfile names to surface even when `show_hidden` is
    /// false — e.g. `[".vimcode-workspace"]` lets the consumer surface
    /// workspace marker files without exposing all hidden files.
    pub extra_file_names: Vec<String>,
    /// Directory names skipped by the walk (heavy build / dependency
    /// dirs). Defaults: [`DEFAULT_IGNORE_DIRS`].
    pub ignore_dirs: Vec<String>,
    /// Maximum recursion depth for the walk. Defaults to
    /// [`DEFAULT_MAX_DEPTH`].
    pub max_depth: usize,
}

/// [`Interactive::State`] for [`FolderPicker`]: the composed palette's
/// own editing state (the query caret and selection anchor).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FolderPickerState {
    /// The composed palette's state.
    pub palette: PaletteState,
}

impl FolderPicker {
    /// A picker rooted at `root`, walked with the default options. Hidden
    /// directories stay hidden unless surfaced via `extra_file_names` or
    /// [`Self::with_show_hidden`].
    ///
    /// - `root` — initial browsing directory (typically
    ///   `env::current_dir()` or the engine's active workspace root).
    /// - `extra_file_names` — dotfile names to surface even with hidden
    ///   directories hidden (e.g. `[".vimcode-workspace".to_string()]`).
    ///   Pass an empty `Vec` for the common case.
    pub fn rooted_at(root: impl Into<PathBuf>, extra_file_names: Vec<String>) -> Self {
        let mut picker = Self {
            id: WidgetId::new("folder_picker"),
            paths: PathPickerState::new(root.into(), Vec::new()),
            show_hidden: false,
            extra_file_names,
            ignore_dirs: DEFAULT_IGNORE_DIRS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            max_depth: DEFAULT_MAX_DEPTH,
        };
        picker.rewalk();
        picker
    }

    /// The current root's entries under this picker's walk options.
    fn walk(&self, root: &Path) -> Vec<PathBuf> {
        collect_dir_entries(
            root,
            self.show_hidden,
            &self.extra_file_names,
            &self.ignore_dirs,
            self.max_depth,
        )
    }

    /// Re-walk the current root and refilter, keeping the query.
    fn rewalk(&mut self) {
        let entries = self.walk(&self.paths.root);
        self.paths.set_all_entries(entries);
    }

    /// Surface (or hide) hidden directories, re-walking the current root.
    /// Returns `self` for builder-style chaining.
    pub fn with_show_hidden(mut self, show_hidden: bool) -> Self {
        self.show_hidden = show_hidden;
        self.rewalk();
        self
    }

    /// Override the picker's [`WidgetId`]. Returns `self` for
    /// builder-style chaining.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = WidgetId::new(id.into());
        self
    }

    /// Replace the skipped directory names, re-walking the current root.
    /// Pass an empty `Vec` to walk every directory (use with care on large
    /// filesystems). Returns `self` for builder-style chaining.
    pub fn with_ignore_dirs(mut self, ignore_dirs: Vec<String>) -> Self {
        self.ignore_dirs = ignore_dirs;
        self.rewalk();
        self
    }

    /// Replace the walk's recursion depth, re-walking the current root.
    /// Returns `self` for builder-style chaining.
    pub fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.max_depth = max_depth;
        self.rewalk();
        self
    }

    /// Browse `new_root`: clear the query and re-walk.
    pub fn navigate_to(&mut self, new_root: PathBuf) {
        let entries = self.walk(&new_root);
        self.paths.navigate_to(new_root, entries);
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

    /// What Enter on the highlighted row means: `..` browses up, anything
    /// else confirms the folder.
    pub fn confirm(&self) -> Handled<PathPickerAction> {
        match self.paths.highlighted() {
            Some(rel) if rel.as_os_str() == ".." => match self.paths.parent() {
                Some(root) => Handled::Event(PathPickerAction::Navigated { root }),
                None => Handled::Consumed,
            },
            Some(rel) => match self.resolve(rel) {
                Some(path) => Handled::Event(PathPickerAction::Confirmed { path }),
                None => Handled::Consumed,
            },
            None => Handled::Consumed,
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
        let title = format!("Open Folder {}", self.paths.root_display(width));

        // Codicon glyph on a GUI backend (`nerd_fonts_enabled` defaults
        // `true` there), falling back to the emoji on TUI
        // (`nerd_fonts_enabled` defaults `false`).
        let folder_icon = Icon::codicon("folder", "\u{1F4C1}")
            .unwrap_or_else(|| Icon::new("\u{1F4C1}", "\u{1F4C1}")); // 📁
        let file_icon =
            Icon::codicon("gear", "\u{2699}").unwrap_or_else(|| Icon::new("\u{2699}", "\u{2699}")); // ⚙

        let items: Vec<PaletteItem> = self
            .paths
            .filtered
            .iter()
            .map(|entry| {
                // A surfaced dotfile (one of `extra_file_names`) is a file;
                // every other row is a directory.
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
            query: self.paths.query.clone(),
            query_cursor: caret,
            items,
            selected_idx: self.paths.selected,
            scroll_offset: self.paths.scroll_top,
            total_count: self.paths.all_entries.len(),
            has_focus: true,
            show_query: true,
            create_label: None,
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

impl A11yPrimitive for FolderPicker {
    fn a11y_info(&self) -> A11yInfo {
        self.palette(0.0, self.paths.query.len()).a11y_info()
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        self.palette(0.0, self.paths.query.len()).a11y_children()
    }
}

impl Primitive for FolderPicker {
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
            .widget_state_as::<FolderPickerState>()
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
            .widget_state_as::<FolderPickerState>()
            .map(|s| &s.palette);
        Primitive::paint_cells(&self.painted_palette(layout, state), layout, cells, env);
    }
}

impl Interactive for FolderPicker {
    type State = FolderPickerState;
    type Action = PathPickerAction;

    /// One stop: the palette is modal, so keys go to it as a whole.
    fn focus(&self) -> FocusSpec {
        FocusSpec::Stop
    }

    fn sub_regions(&self, layout: &PalettePrimitiveLayout) -> Vec<(Rect, SubTarget)> {
        Interactive::sub_regions(&self.palette(layout.area.width, 0), layout)
    }

    /// The folder picker's query is never a destination filename, so the
    /// vim shortcuts always take priority over typing them.
    fn handle(
        &self,
        state: &mut FolderPickerState,
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
            true,
            || self.confirm(),
        )
    }
}

// ── Controller ────────────────────────────────────────────────────────────

/// Event-driven wrapper over a [`FolderPicker`] and its state, for a host
/// that calls it with raw [`UiEvent`]s: it applies every browsing change
/// itself and reports only a confirmation or a dismissal.
///
/// See the [module-level documentation](self) for a usage example.
pub struct FolderPickerController {
    widget: FolderPicker,
    state: FolderPickerState,
}

impl FolderPickerController {
    /// Create a new picker rooted at `root`, with `show_hidden` as a
    /// positional bool.
    ///
    /// - `root` — initial browsing directory.
    /// - `extra_file_names` — dotfile names to surface even when
    ///   `show_hidden` is false. Pass an empty `Vec` for the common case.
    /// - `show_hidden` — if true, all hidden directories are shown.
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

    /// Create a controller over [`FolderPicker::rooted_at`]'s picker.
    pub fn rooted_at(root: impl Into<PathBuf>, extra_file_names: Vec<String>) -> Self {
        Self {
            widget: FolderPicker::rooted_at(root, extra_file_names),
            state: FolderPickerState::default(),
        }
    }

    /// See [`FolderPicker::with_show_hidden`].
    pub fn with_show_hidden(mut self, show_hidden: bool) -> Self {
        self.widget = self.widget.with_show_hidden(show_hidden);
        self
    }

    /// Override the `WidgetId` used by the rendered `Palette`. Defaults to
    /// `WidgetId::new("folder_picker")`. Returns `self` for builder-style
    /// chaining.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.widget = self.widget.with_id(id);
        self
    }

    /// See [`FolderPicker::with_ignore_dirs`].
    pub fn with_ignore_dirs(mut self, ignore_dirs: Vec<String>) -> Self {
        self.widget = self.widget.with_ignore_dirs(ignore_dirs);
        self
    }

    /// See [`FolderPicker::with_max_depth`].
    pub fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.widget = self.widget.with_max_depth(max_depth);
        self
    }

    /// The [`FolderPicker`] widget this controller drives.
    pub fn widget(&self) -> &FolderPicker {
        &self.widget
    }

    // ── State accessors ───────────────────────────────────────────────

    /// Current browsing root.
    pub fn root(&self) -> &Path {
        &self.widget.paths.root
    }

    /// Current query string.
    pub fn query(&self) -> &str {
        &self.widget.paths.query
    }

    /// Currently filtered entries (relative paths).
    pub fn filtered(&self) -> &[PathBuf] {
        &self.widget.paths.filtered
    }

    /// Index of the highlighted row in `filtered`.
    pub fn selected(&self) -> usize {
        self.widget.paths.selected
    }

    /// Scroll offset (first visible row index in `filtered`).
    pub fn scroll_top(&self) -> usize {
        self.widget.paths.scroll_top
    }

    /// See [`FolderPicker::selected_path`].
    pub fn selected_path(&self) -> Option<PathBuf> {
        self.widget.selected_path()
    }

    // ── Navigation ────────────────────────────────────────────────────

    /// Navigate to `new_root`, clearing the query and reloading entries.
    pub fn navigate_to(&mut self, new_root: PathBuf) {
        self.widget.navigate_to(new_root);
    }

    /// Navigate to the parent directory (no-op at filesystem root).
    pub fn navigate_up(&mut self) {
        if let Some(parent) = self.widget.paths.parent() {
            self.navigate_to(parent);
        }
    }

    // ── Query editing ─────────────────────────────────────────────────

    /// Append `c` to the query and refilter.
    pub fn push_char(&mut self, c: char) {
        self.widget.paths.push_char(c);
        self.state.palette = PaletteState::default();
    }

    /// Remove the last character from the query and refilter.
    pub fn pop_char(&mut self) {
        self.widget.paths.pop_char();
        self.state.palette = PaletteState::default();
    }

    /// Re-compute `filtered` from `all_entries` and the current `query`.
    pub fn refilter(&mut self) {
        self.widget.paths.refilter();
    }

    // ── Selection movement ────────────────────────────────────────────

    /// Move the selection one row up (clamped at 0).
    pub fn move_up(&mut self) {
        self.widget.paths.move_up();
    }

    /// Move the selection one row down (clamped at last row).
    pub fn move_down(&mut self) {
        self.widget.paths.move_down();
    }

    /// Clamp `scroll_top` so `selected` is always visible within
    /// `visible_rows` rows.
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
    ///
    /// Call this from `AppLogic::render` whenever the picker is open.
    pub fn render(&self, rect: Rect, backend: &mut dyn Backend) {
        crate::backend::DrawsPrimitives::draw_primitive(backend, rect, &self.build_palette(rect));
    }

    // ── Handle ────────────────────────────────────────────────────────

    /// Drive the picker with a backend-neutral `UiEvent`.
    ///
    /// `visible_rows` is the number of list rows that fit inside the
    /// palette chrome; compute it as
    /// `popup_height.saturating_sub(PALETTE_CHROME_ROWS)`.
    ///
    /// # Canonical typing source
    ///
    /// Query characters are sourced from `UiEvent::KeyPressed`'s
    /// `Key::Char(c)` arm only — never from `UiEvent::CharTyped`. A
    /// backend (e.g. Win/macOS IME) might emit *both* for the same input;
    /// listening to only `KeyPressed` avoids the double-insert risk. If a
    /// backend needs IME-composed input, route it through `KeyPressed` as
    /// well or call [`push_char`](Self::push_char) directly.
    pub fn handle_event(
        &mut self,
        event: &UiEvent,
        visible_rows: usize,
    ) -> Handled<PathPickerAction> {
        let UiEvent::KeyPressed { .. } = event else {
            return Handled::Ignored;
        };
        let handled = drive_picker(&self.widget, &mut self.state, event, visible_rows);
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
            Handled::Event(_) | Handled::Consumed => FolderPickerEvent::Consumed,
            Handled::Ignored => FolderPickerEvent::Ignored,
        }
    }
}

/// Run `widget`'s [`Interactive::handle`] for one keystroke, against the
/// cell-grid layout of a palette tall enough for `visible_rows` list rows.
pub(crate) fn drive_picker<W>(
    widget: &W,
    state: &mut W::State,
    event: &UiEvent,
    visible_rows: usize,
) -> Handled<PathPickerAction>
where
    W: Interactive<Action = PathPickerAction>,
{
    let rect = Rect::new(0.0, 0.0, 80.0, (visible_rows + PALETTE_CHROME_ROWS) as f32);
    widget_host::drive_in_cells(widget, state, event, rect)
        .map_or(Handled::Ignored, |driven| driven.handled)
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
    use crate::{Key, Modifiers, NamedKey};

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
        assert!(picker
            .widget
            .paths
            .all_entries
            .contains(&PathBuf::from("..")));
        assert!(picker
            .widget
            .paths
            .all_entries
            .contains(&PathBuf::from(".")));
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
                .widget
                .paths
                .all_entries
                .contains(&PathBuf::from(".hidden-child")),
            "hidden directories stay hidden by default"
        );

        let hidden_on = FolderPickerController::rooted_at(tmp, vec![]).with_show_hidden(true);
        assert!(
            hidden_on
                .widget
                .paths
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
            .widget
            .paths
            .all_entries
            .contains(&PathBuf::from(".hidden-child")));

        let hidden_on = FolderPickerController::new(tmp, vec![], true);
        assert!(hidden_on
            .widget
            .paths
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
        picker.widget.paths.selected = 9;
        picker.sync_scroll(5);
        assert!(picker.widget.paths.scroll_top + 5 > picker.widget.paths.selected);
        assert!(picker.widget.paths.scroll_top <= picker.widget.paths.selected);
    }

    #[test]
    fn sync_scroll_scrolls_up_when_needed() {
        let dir = picker_root();
        let tmp = dir.path().to_path_buf();
        let mut picker = FolderPickerController::rooted_at(tmp, vec![]);
        assert!(picker.filtered().len() >= 5);
        picker.widget.paths.scroll_top = 5;
        picker.widget.paths.selected = 2; // above scroll_top
        picker.sync_scroll(3);
        assert_eq!(picker.widget.paths.scroll_top, 2);
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
        picker.widget.paths.selected = 0;
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
        picker.widget.paths.selected = dot_idx;
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
        picker.widget.paths.selected = dot_idx;
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
            .widget
            .paths
            .all_entries
            .iter()
            .any(|p| p == &PathBuf::from("target"));
        let has_keep = picker
            .widget
            .paths
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
        let has_d1 = picker
            .widget
            .paths
            .all_entries
            .iter()
            .any(|p| p.ends_with("d1"));
        let has_d2 = picker
            .widget
            .paths
            .all_entries
            .iter()
            .any(|p| p.ends_with("d2"));
        assert!(has_d1);
        assert!(!has_d2, "depth=0 should not surface nested subdirs");
    }
}
