//! `CellGridController` — cursor movement and rectangular range selection
//! for a single [`CellGrid`], the way a spreadsheet user expects. It owns
//! no cell content and does no editing: it tracks exactly the geometry
//! [`CellGrid`] itself tracks (row/column counts, frozen panes, scroll
//! offsets, sizes) plus the cursor and selection anchor, and translates
//! [`UiEvent`]s into cursor/selection changes a host applies to the
//! [`CellGrid`] it paints next frame.
//!
//! # Why this doesn't go through a bespoke `Backend` method
//!
//! [`CellGrid`] has no `Backend::draw_cell_grid`/`cell_grid_layout` pair
//! (see that primitive's module doc) — it paints exclusively through
//! [`crate::primitive::Primitive`]/[`crate::backend::DrawsPrimitives::draw_primitive`].
//! [`Self::layout`] resolves the same [`crate::primitives::cell_grid::CellGridPrimitiveLayout`]
//! that painting would, via `Primitive::layout` directly (never
//! `draw_primitive` itself, which also paints — calling it here, with a
//! throwaway geometry-only [`CellGrid`] carrying no cell text, would
//! paint blank cells over whatever a host already painted this frame).
//! [`Self::geometry_grid`]'s own doc explains why an empty `cells` is
//! correct for this purpose.
//!
//! # Scroll-to-follow and frozen panes
//!
//! [`Self::ensure_cursor_visible`] mirrors
//! [`crate::compose::tree_nav::TreeNavCore::scroll_to_visible`]'s
//! "if before the window, snap to top; if after, snap to bottom" rule,
//! run independently on the row and column axis, and skipped entirely
//! for a cursor inside the frozen rows/columns — those are always
//! visible by construction, scrolled or not. The final clamp into
//! `[0, max]` reuses [`scroll_by_clamped`] rather than hand-rolling the
//! same arithmetic a third time.
//!
//! # No shared key classifier
//!
//! [`crate::compose::tree_controller::classify_tree_nav_key`] is the
//! closest existing "which navigation key is this" helper, but its shape
//! doesn't fit a 2-D grid: it has no Left/Right (a tree spends those on
//! expand/collapse, routed through
//! [`crate::compose::tree_nav::TreeNavCore::horizontal_nav`] instead), no
//! Tab/Shift+Tab, and no Ctrl-modified "jump to an edge" variant. This
//! controller classifies its own keys in [`Self::handle_key`] instead of
//! forcing tree-shaped navigation onto cells.
//!
//! # Ctrl+Arrow jumps to the sheet edge, not the data edge
//!
//! A real spreadsheet's Ctrl+Arrow jumps to the edge of a contiguous run
//! of non-empty cells. This controller has no cell-content awareness at
//! all — see [`crate::primitives::cell_grid`]'s module doc, "the grid
//! only presents" — so it can only jump to the edge of the sheet itself
//! ([`Self::row_count`]/[`Self::col_count`]'s bounds). A host that wants
//! real data-aware jumping layers its own Ctrl+Arrow handling in front of
//! this controller's.

use std::collections::BTreeMap;

use crate::backend::{Backend, Clipboard};
use crate::primitive::{LayoutCtx, Primitive};
use crate::primitives::cell_grid::{
    Cell, CellEditState, CellGrid, CellGridHit, CellGridPrimitiveLayout,
};
use crate::primitives::scrollbar::scroll_by_clamped;
use crate::primitives::text_input::EditOp;
use crate::{
    ButtonMask, Key, Modifiers, MouseButton, NamedKey, Point, Rect, TextField, UiEvent, WidgetId,
};

/// What happened after [`CellGridController::handle`] processed an event,
/// or after calling one of its navigation methods directly.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CellGridControllerEvent {
    /// The cursor moved and the selection is just that one cell (no
    /// [`CellGridController::selection_anchor`] beyond the cursor
    /// itself).
    CursorMoved { row: usize, col: usize },
    /// The selection now spans more than the cursor cell — all bounds
    /// inclusive, already normalized (`top <= bottom`, `left <= right`).
    SelectionChanged {
        top: usize,
        left: usize,
        bottom: usize,
        right: usize,
    },
    /// Event consumed (e.g. a drag update) — caller should redraw.
    Consumed,
    /// Event not relevant to this controller.
    Ignored,
    /// F2 (or any other host-chosen "edit this cell" trigger routed into
    /// [`CellGridController::handle`]) was pressed at the cursor cell,
    /// with no active edit yet. The controller has no cell content (see
    /// the module doc) so it cannot seed the editor itself — the host
    /// responds by calling [`CellGridController::start_editing`] with
    /// that cell's current raw text.
    CellEditRequested { row: usize, col: usize },
    /// An inline edit began at `(row, col)` — either [`Self::start_editing`]
    /// was called, or a plain character typed at the cursor with no
    /// active edit started a fresh one seeded with just that character.
    CellEditStarted { row: usize, col: usize },
    /// The in-progress edit's text buffer changed (insert/delete/paste).
    CellEditChanged {
        row: usize,
        col: usize,
        text: String,
    },
    /// The user confirmed the in-progress edit — Enter, Tab or an arrow
    /// key. The cursor has already moved to the next cell (as Excel
    /// does); read [`CellGridController::cursor`] for where. The host
    /// hands `text` to its engine (e.g. `cell.set`) for `(row, col)`.
    CellEditCommitted {
        row: usize,
        col: usize,
        text: String,
    },
    /// The user cancelled the in-progress edit (Escape). The cell's
    /// value is unchanged; the cursor has not moved.
    CellEditCancelled { row: usize, col: usize },
    /// [`CellGridController::paste`] found text on the clipboard.
    /// `anchor_row`/`anchor_col` is the current selection's top-left
    /// corner (the cursor itself when there's no multi-cell selection).
    /// The controller has no cell content of its own (module doc) so it
    /// never applies the paste itself: the host parses `text` — by
    /// convention TSV, matching [`CellGridController::copy_selection`]'s
    /// own output — and applies it through its engine starting at the
    /// anchor.
    PasteRequested {
        anchor_row: usize,
        anchor_col: usize,
        text: String,
    },
    /// [`CellGridController::cut_selection`] copied the current
    /// selection to the clipboard; the host additionally clears every
    /// cell in `[top, bottom] x [left, right]` (all inclusive) through
    /// its engine — this controller has no cell content to clear itself
    /// (module doc).
    CutRequested {
        top: usize,
        left: usize,
        bottom: usize,
        right: usize,
    },
}

/// The active inline edit's text buffer — a [`TextField`] reused wholesale
/// (per the module doc's "Reuse `TextField`" convention) rather than a
/// second, hand-rolled line editor. Always at [`CellGridController::cursor`]:
/// unlike [`crate::compose::tree_controller::TreeController`]'s renaming
/// state (which carries its own `path`, independent of the tree's
/// selection), a `CellGrid` edit has no identity apart from "the cursor
/// cell", so there is nothing else to track here.
struct CellEditing {
    field: TextField,
}

/// Cursor-movement and rectangular-selection state for a single
/// [`CellGrid`]. See the module doc for the full contract.
pub struct CellGridController {
    id: WidgetId,
    row_count: usize,
    col_count: usize,
    frozen_rows: usize,
    frozen_cols: usize,
    row_scroll: usize,
    col_scroll: usize,
    default_row_height: f32,
    default_col_width: f32,
    row_heights: BTreeMap<usize, f32>,
    col_widths: BTreeMap<usize, f32>,
    show_row_headers: bool,
    show_col_headers: bool,
    row_labels: BTreeMap<usize, String>,
    col_labels: BTreeMap<usize, String>,
    cursor: (usize, usize),
    selection_anchor: Option<(usize, usize)>,
    has_focus: bool,
    show_scrollbar: bool,
    /// The selection's fixed corner for an in-progress mouse drag, set on
    /// a `Cell`/`ColumnHeader`/`RowHeader`/`Corner` press and cleared on
    /// release. `None` whenever no drag is active — [`Self::mouse_drag`]'s
    /// own guard.
    drag_anchor: Option<(usize, usize)>,
    /// The active inline edit at [`Self::cursor`], if any — see
    /// [`CellEditing`]'s own doc.
    editing: Option<CellEditing>,
}

impl CellGridController {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: WidgetId::new(id),
            row_count: 0,
            col_count: 0,
            frozen_rows: 0,
            frozen_cols: 0,
            row_scroll: 0,
            col_scroll: 0,
            default_row_height: 1.0,
            default_col_width: 8.0,
            row_heights: BTreeMap::new(),
            col_widths: BTreeMap::new(),
            show_row_headers: true,
            show_col_headers: true,
            row_labels: BTreeMap::new(),
            col_labels: BTreeMap::new(),
            cursor: (0, 0),
            selection_anchor: None,
            has_focus: true,
            show_scrollbar: false,
            drag_anchor: None,
            editing: None,
        }
    }

    // ── Per-frame / model data ────────────────────────────────────────

    /// Set the sheet's full size. The cursor is re-clamped into the new
    /// bounds, and a selection anchor outside those bounds collapses — a
    /// shrink past the anchor (e.g. a filtered-out set of rows) always
    /// leaves a single, valid cursor cell rather than a stale
    /// out-of-range anchor. An anchor still inside the new bounds is
    /// left untouched, so an idempotent re-declaration of the same
    /// dimensions (the expected per-frame host call) never disturbs an
    /// active range selection.
    pub fn set_dimensions(&mut self, row_count: usize, col_count: usize) {
        self.row_count = row_count;
        self.col_count = col_count;
        let new_cursor = (
            self.cursor.0.min(row_count.saturating_sub(1)),
            self.cursor.1.min(col_count.saturating_sub(1)),
        );
        // An in-progress edit is always anchored to `self.cursor` (see
        // `CellEditing`'s own doc) — if a shrink is about to move the
        // cursor out from under it, the edit buffer would silently start
        // describing a different cell's text. Drop it rather than carry
        // that mismatch forward; same "collapse past the bound, leave an
        // idempotent re-declaration alone" rule `selection_anchor` below
        // already follows.
        if new_cursor != self.cursor {
            self.editing = None;
        }
        self.cursor = new_cursor;
        self.selection_anchor = self
            .selection_anchor
            .filter(|&(row, col)| row < row_count && col < col_count);
    }

    pub fn set_frozen(&mut self, frozen_rows: usize, frozen_cols: usize) {
        self.frozen_rows = frozen_rows;
        self.frozen_cols = frozen_cols;
    }

    pub fn set_sizing(&mut self, default_row_height: f32, default_col_width: f32) {
        self.default_row_height = default_row_height;
        self.default_col_width = default_col_width;
    }

    pub fn set_row_heights(&mut self, row_heights: BTreeMap<usize, f32>) {
        self.row_heights = row_heights;
    }

    pub fn set_col_widths(&mut self, col_widths: BTreeMap<usize, f32>) {
        self.col_widths = col_widths;
    }

    pub fn set_show_row_headers(&mut self, show: bool) {
        self.show_row_headers = show;
    }

    pub fn set_show_col_headers(&mut self, show: bool) {
        self.show_col_headers = show;
    }

    pub fn set_row_labels(&mut self, row_labels: BTreeMap<usize, String>) {
        self.row_labels = row_labels;
    }

    pub fn set_col_labels(&mut self, col_labels: BTreeMap<usize, String>) {
        self.col_labels = col_labels;
    }

    pub fn set_has_focus(&mut self, has_focus: bool) {
        self.has_focus = has_focus;
    }

    pub fn has_focus(&self) -> bool {
        self.has_focus
    }

    pub fn set_show_scrollbar(&mut self, show: bool) {
        self.show_scrollbar = show;
    }

    // ── State accessors ───────────────────────────────────────────────

    pub fn cursor(&self) -> (usize, usize) {
        self.cursor
    }

    pub fn selection_anchor(&self) -> Option<(usize, usize)> {
        self.selection_anchor
    }

    /// The resolved rectangular selection — see
    /// [`CellGrid::selection_bounds`] (reused rather than re-derived).
    pub fn selection_bounds(&self) -> (usize, usize, usize, usize) {
        self.geometry_grid().selection_bounds()
    }

    /// Set the cursor/selection directly (e.g. restoring a persisted
    /// selection) without going through a navigation key — clamped into
    /// the current sheet bounds the same way every other mutation is.
    pub fn set_selection(&mut self, cursor: (usize, usize), anchor: Option<(usize, usize)>) {
        self.cursor = self.clamp_cell(cursor);
        self.selection_anchor = anchor.map(|a| self.clamp_cell(a));
    }

    pub fn row_scroll(&self) -> usize {
        self.row_scroll
    }

    pub fn col_scroll(&self) -> usize {
        self.col_scroll
    }

    pub fn set_scroll(&mut self, row_scroll: usize, col_scroll: usize) {
        self.row_scroll = row_scroll;
        self.col_scroll = col_scroll;
    }

    // ── Inline editing ───────────────────────────────────────────────

    /// Whether [`Self::cursor`] is currently being edited.
    pub fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    /// The in-progress edit's text buffer, or `None` when not editing.
    pub fn editing_text(&self) -> Option<&str> {
        self.editing.as_ref().map(|e| e.field.value.as_str())
    }

    /// The in-progress edit's cursor, as a byte offset into
    /// [`Self::editing_text`].
    pub fn editing_cursor(&self) -> Option<usize> {
        self.editing.as_ref().and_then(|e| e.field.cursor)
    }

    /// The in-progress edit's selection anchor, as a byte offset into
    /// [`Self::editing_text`].
    pub fn editing_selection_anchor(&self) -> Option<usize> {
        self.editing.as_ref().and_then(|e| e.field.selection_anchor)
    }

    /// Start editing [`Self::cursor`], seeded with `text` (the cell's
    /// current raw value) and the cursor placed at its end — the F2
    /// convention. A host calls this in response to
    /// [`CellGridControllerEvent::CellEditRequested`] (the controller
    /// has no cell content of its own to seed with, see the module doc)
    /// or whenever else it wants to open the editor programmatically
    /// (e.g. the formula bar gaining focus — see [`Self::sync_formula_bar`]).
    pub fn start_editing(&mut self, text: impl Into<String>) -> CellGridControllerEvent {
        if self.row_count == 0 || self.col_count == 0 {
            return CellGridControllerEvent::Ignored;
        }
        let text = text.into();
        let cursor = text.len();
        self.begin_edit(text, cursor);
        let (row, col) = self.cursor;
        CellGridControllerEvent::CellEditStarted { row, col }
    }

    /// Cancel the in-progress edit programmatically, discarding its text
    /// — e.g. a host-level "click away" rule the controller's own
    /// [`Self::handle`] doesn't implement (see that method's doc on why
    /// mouse input is inert while editing).
    pub fn cancel_editing(&mut self) -> CellGridControllerEvent {
        match self.cancel_edit() {
            Some((row, col)) => CellGridControllerEvent::CellEditCancelled { row, col },
            None => CellGridControllerEvent::Ignored,
        }
    }

    /// Mirror the in-progress edit's text/cursor/selection onto an
    /// external `TextField` — the formula bar. Only `value`/`cursor`/
    /// `selection_anchor` are touched; `id`/`placeholder`/`has_focus`/
    /// the icon/clear-button fields stay whatever the host already set.
    ///
    /// When not editing, `cursor`/`selection_anchor` are cleared to
    /// `None` (no stale caret/selection survives a previous edit) but
    /// `value` is left untouched — the host sets it from its own lookup
    /// of the cursor cell's raw text, which this controller doesn't
    /// have. Call this once per frame before painting the formula bar.
    pub fn sync_formula_bar(&self, field: &mut TextField) {
        match &self.editing {
            Some(editing) => {
                field.value = editing.field.value.clone();
                field.cursor = editing.field.cursor;
                field.selection_anchor = editing.field.selection_anchor;
            }
            None => {
                field.cursor = None;
                field.selection_anchor = None;
            }
        }
    }

    /// Route one key event that arrived at the formula bar `TextField`
    /// — not the grid itself — into the same in-progress edit
    /// [`Self::handle`] would update, so typing there commits the same
    /// way (Enter/Tab/arrows commit and move, Escape cancels). Returns
    /// [`CellGridControllerEvent::Ignored`] when nothing is being
    /// edited yet; a host opens the editor first (typically on the
    /// formula bar gaining focus), via [`Self::start_editing`].
    pub fn handle_formula_bar_key(
        &mut self,
        key: &Key,
        modifiers: &Modifiers,
        backend: &dyn Backend,
        rect: Rect,
    ) -> CellGridControllerEvent {
        if self.editing.is_none() {
            return CellGridControllerEvent::Ignored;
        }
        self.handle_edit_key(key, modifiers, backend, rect)
    }

    // ── Rendering ─────────────────────────────────────────────────────

    /// Build the [`CellGrid`] this controller's current state describes,
    /// carrying `cells` (the host's windowed cell text for this frame,
    /// loaded from the controller's current scroll position — see
    /// [`crate::primitives::cell_grid`]'s module doc for what that window
    /// covers). What a host paints via
    /// [`crate::backend::DrawsPrimitives::draw_primitive`].
    pub fn grid(&self, cells: Vec<Vec<Cell>>) -> CellGrid {
        CellGrid {
            id: self.id,
            row_count: self.row_count,
            col_count: self.col_count,
            frozen_rows: self.frozen_rows,
            frozen_cols: self.frozen_cols,
            row_scroll: self.row_scroll,
            col_scroll: self.col_scroll,
            default_row_height: self.default_row_height,
            default_col_width: self.default_col_width,
            row_heights: self.row_heights.clone(),
            col_widths: self.col_widths.clone(),
            show_row_headers: self.show_row_headers,
            show_col_headers: self.show_col_headers,
            row_labels: self.row_labels.clone(),
            col_labels: self.col_labels.clone(),
            cursor: self.cursor,
            selection_anchor: self.selection_anchor,
            cells,
            // `row_scroll`/`col_scroll` are ranks *past* the frozen
            // prefix (see `ensure_cursor_visible`'s `rank = cr -
            // frozen_rows`), but `loaded_row_start`/`loaded_col_start`
            // are absolute sheet indices (see `CellGrid::cells_window`'s
            // doc) — add the frozen count back so the two conventions
            // agree, or every windowed lookup past a frozen pane is off
            // by `frozen_rows`/`frozen_cols`.
            loaded_row_start: self.frozen_rows.min(self.row_count) + self.row_scroll,
            loaded_col_start: self.frozen_cols.min(self.col_count) + self.col_scroll,
            has_focus: self.has_focus,
            show_scrollbar: self.show_scrollbar,
            editing: self.editing.as_ref().map(|e| CellEditState {
                text: e.field.value.clone(),
                cursor: e.field.cursor.unwrap_or(0),
                selection_anchor: e.field.selection_anchor,
                placeholder: None,
            }),
        }
    }

    /// `Self::grid` with no cell text — every geometry query
    /// ([`Self::layout`], [`Self::selection_bounds`]) needs only the
    /// counts/sizes/cursor/selection fields, never `cells` itself (see
    /// [`CellGrid::layout`]'s own doc), so building one with an empty
    /// `cells` for those queries never risks painting blank content —
    /// nothing here ever reaches a paint call.
    fn geometry_grid(&self) -> CellGrid {
        self.grid(Vec::new())
    }

    /// Resolve this controller's current [`CellGridPrimitiveLayout`] at
    /// `rect`, the same geometry a host's own `Primitive::layout`/
    /// `draw_primitive` call would resolve for the equivalent
    /// [`Self::grid`] — built directly through `Primitive::layout` so no
    /// paint call ever runs against the empty [`Self::geometry_grid`].
    ///
    /// A host calls this before [`Self::grid`] to learn which absolute
    /// rows and columns are on screen this frame —
    /// [`crate::primitives::cell_grid::CellGridLayout::rows`] /
    /// [`crate::primitives::cell_grid::CellGridLayout::cols`], in the same
    /// frozen-then-scrolled order [`CellGrid::cells`] is indexed in — so
    /// it can window exactly those cells instead of materialising the
    /// whole sheet. See [`crate::primitives::cell_grid`]'s module doc,
    /// "`cells` is a viewport-sized window".
    pub fn layout(&self, backend: &dyn Backend, rect: Rect) -> CellGridPrimitiveLayout {
        let ctx = LayoutCtx {
            line_height: backend.chrome_line_height(),
            char_width: backend.char_width(),
            nerd_fonts_enabled: backend.nerd_fonts_enabled(),
        };
        Primitive::layout(&self.geometry_grid(), &ctx, rect)
    }

    /// How many body (non-frozen) rows/columns fit in `rect`, plus the
    /// resolved (viewport-clamped) frozen row/column counts — measured
    /// with both scroll offsets zeroed so the capacity doesn't drift
    /// depending on where the current scroll position happens to sit,
    /// mirroring [`crate::compose::tree_table_controller::TreeTableController::layout`]'s
    /// identical "zero the offset before measuring" rule.
    fn body_metrics(&self, backend: &dyn Backend, rect: Rect) -> (usize, usize, usize, usize) {
        let mut probe = self.geometry_grid();
        probe.row_scroll = 0;
        probe.col_scroll = 0;
        let ctx = LayoutCtx {
            line_height: backend.chrome_line_height(),
            char_width: backend.char_width(),
            nerd_fonts_enabled: backend.nerd_fonts_enabled(),
        };
        let layout = Primitive::layout(&probe, &ctx, rect).layout;
        let body_rows = layout.rows.len().saturating_sub(layout.frozen_rows);
        let body_cols = layout.cols.len().saturating_sub(layout.frozen_cols);
        (body_rows, body_cols, layout.frozen_rows, layout.frozen_cols)
    }

    /// Scroll the minimum amount on each axis to bring [`Self::cursor`]
    /// into view, leaving a frozen row/column untouched — it's always
    /// visible regardless of scroll. See the module doc's "Scroll-to-follow"
    /// section.
    fn ensure_cursor_visible(&mut self, backend: &dyn Backend, rect: Rect) {
        let (body_rows, body_cols, frozen_rows, frozen_cols) = self.body_metrics(backend, rect);
        let (cr, cc) = self.cursor;

        if cr >= frozen_rows {
            let rank = cr - frozen_rows;
            if rank < self.row_scroll {
                self.row_scroll = rank;
            } else if body_rows > 0 && rank >= self.row_scroll + body_rows {
                self.row_scroll = rank + 1 - body_rows;
            }
            let max_scroll = (self.row_count.saturating_sub(frozen_rows)).saturating_sub(body_rows);
            self.row_scroll = scroll_by_clamped(self.row_scroll, 0, max_scroll);
        }

        if cc >= frozen_cols {
            let rank = cc - frozen_cols;
            if rank < self.col_scroll {
                self.col_scroll = rank;
            } else if body_cols > 0 && rank >= self.col_scroll + body_cols {
                self.col_scroll = rank + 1 - body_cols;
            }
            let max_scroll = (self.col_count.saturating_sub(frozen_cols)).saturating_sub(body_cols);
            self.col_scroll = scroll_by_clamped(self.col_scroll, 0, max_scroll);
        }
    }

    fn clamp_cell(&self, (row, col): (usize, usize)) -> (usize, usize) {
        (
            row.min(self.row_count.saturating_sub(1)),
            col.min(self.col_count.saturating_sub(1)),
        )
    }

    fn result_for_current_selection(&self) -> CellGridControllerEvent {
        let (top, left, bottom, right) = self.selection_bounds();
        if top == bottom && left == right {
            CellGridControllerEvent::CursorMoved {
                row: self.cursor.0,
                col: self.cursor.1,
            }
        } else {
            CellGridControllerEvent::SelectionChanged {
                top,
                left,
                bottom,
                right,
            }
        }
    }

    // ── Navigation primitives ─────────────────────────────────────────

    /// Move the cursor to `(row, col)` (clamped into the sheet), scroll
    /// it into view, and update the selection anchor: `extend` keeps (or
    /// starts, from the old cursor) an anchor; anything else collapses
    /// the selection back to just the new cursor cell.
    fn set_cursor(
        &mut self,
        row: usize,
        col: usize,
        extend: bool,
        backend: &dyn Backend,
        rect: Rect,
    ) -> CellGridControllerEvent {
        if self.row_count == 0 || self.col_count == 0 {
            return CellGridControllerEvent::Ignored;
        }
        if extend && self.selection_anchor.is_none() {
            self.selection_anchor = Some(self.cursor);
        } else if !extend {
            self.selection_anchor = None;
        }
        self.cursor = self.clamp_cell((row, col));
        self.ensure_cursor_visible(backend, rect);
        self.result_for_current_selection()
    }

    fn move_by(
        &mut self,
        d_row: isize,
        d_col: isize,
        extend: bool,
        backend: &dyn Backend,
        rect: Rect,
    ) -> CellGridControllerEvent {
        let (r, c) = self.cursor;
        let nr = (r as isize + d_row).clamp(0, self.row_count.saturating_sub(1) as isize) as usize;
        let nc = (c as isize + d_col).clamp(0, self.col_count.saturating_sub(1) as isize) as usize;
        self.set_cursor(nr, nc, extend, backend, rect)
    }

    /// Tab (`forward`) / Shift+Tab: across the row, wrapping to the
    /// start of the next/previous row at a row boundary — never extends
    /// the selection, matching every spreadsheet's own Tab convention.
    fn move_tab(
        &mut self,
        forward: bool,
        backend: &dyn Backend,
        rect: Rect,
    ) -> CellGridControllerEvent {
        let (r, c) = self.cursor;
        let (nr, nc) = if forward {
            if c + 1 < self.col_count {
                (r, c + 1)
            } else if r + 1 < self.row_count {
                (r + 1, 0)
            } else {
                (r, c)
            }
        } else if c > 0 {
            (r, c - 1)
        } else if r > 0 {
            (r - 1, self.col_count.saturating_sub(1))
        } else {
            (r, c)
        };
        self.set_cursor(nr, nc, false, backend, rect)
    }

    /// Enter (`forward`) / Shift+Enter: down/up within the same column —
    /// never extends the selection.
    fn move_enter(
        &mut self,
        forward: bool,
        backend: &dyn Backend,
        rect: Rect,
    ) -> CellGridControllerEvent {
        let (r, c) = self.cursor;
        let nr = if forward {
            (r + 1).min(self.row_count.saturating_sub(1))
        } else {
            r.saturating_sub(1)
        };
        self.set_cursor(nr, c, false, backend, rect)
    }

    fn page(
        &mut self,
        forward: bool,
        extend: bool,
        backend: &dyn Backend,
        rect: Rect,
    ) -> CellGridControllerEvent {
        let (body_rows, ..) = self.body_metrics(backend, rect);
        let page = body_rows.max(1) as isize;
        let delta = if forward { page } else { -page };
        self.move_by(delta, 0, extend, backend, rect)
    }

    fn select_all(&mut self, backend: &dyn Backend, rect: Rect) -> CellGridControllerEvent {
        if self.row_count == 0 || self.col_count == 0 {
            return CellGridControllerEvent::Ignored;
        }
        self.cursor = (0, 0);
        self.selection_anchor = Some((self.row_count - 1, self.col_count - 1));
        self.ensure_cursor_visible(backend, rect);
        self.result_for_current_selection()
    }

    fn select_row(
        &mut self,
        row: usize,
        backend: &dyn Backend,
        rect: Rect,
    ) -> CellGridControllerEvent {
        if self.col_count == 0 {
            return CellGridControllerEvent::Ignored;
        }
        self.cursor = self.clamp_cell((row, 0));
        self.selection_anchor = Some(self.clamp_cell((row, self.col_count - 1)));
        self.ensure_cursor_visible(backend, rect);
        self.result_for_current_selection()
    }

    fn select_column(
        &mut self,
        col: usize,
        backend: &dyn Backend,
        rect: Rect,
    ) -> CellGridControllerEvent {
        if self.row_count == 0 {
            return CellGridControllerEvent::Ignored;
        }
        self.cursor = self.clamp_cell((0, col));
        self.selection_anchor = Some(self.clamp_cell((self.row_count - 1, col)));
        self.ensure_cursor_visible(backend, rect);
        self.result_for_current_selection()
    }

    // ── Clipboard ────────────────────────────────────────────────────
    //
    // None of these three go through `Self::handle`'s key dispatch:
    // `Self::handle` only ever receives a `backend` and a `rect`, never
    // the host's real, cell-filled `CellGrid` or a `Clipboard` — this
    // controller has no cell content of its own (module doc), and
    // clipboard IO is the one thing every other method on this
    // controller never needed a platform handle for. A host calls these
    // directly from its own Ctrl/Cmd+C/X/V key handling instead.

    /// Ctrl/Cmd+C: copy the current selection's displayed text onto
    /// `clipboard` as TSV (tab-separated within a row, newline-separated
    /// across rows) — [`CellGrid::range_as_tsv`] does the actual
    /// windowed-to-absolute text lookup. `grid` must be the host's real,
    /// cell-filled [`CellGrid`] — never [`Self::geometry_grid`]'s empty
    /// one — since this controller owns no cell content itself.
    ///
    /// `Ignored` on an empty sheet, or while [`Self::is_editing`] — the
    /// range to copy is the *selection*, which is a different cell range
    /// than whatever's being edited inside one cell, so routing Ctrl+C
    /// here mid-edit would copy the wrong thing. Otherwise always
    /// `Consumed`, even for a single-cell "selection".
    pub fn copy_selection(
        &self,
        grid: &CellGrid,
        clipboard: &dyn Clipboard,
    ) -> CellGridControllerEvent {
        if self.row_count == 0 || self.col_count == 0 || self.is_editing() {
            return CellGridControllerEvent::Ignored;
        }
        let (top, left, bottom, right) = self.selection_bounds();
        clipboard.write_text(&grid.range_as_tsv(top, left, bottom, right));
        CellGridControllerEvent::Consumed
    }

    /// Ctrl/Cmd+X: the same copy [`Self::copy_selection`] does, then
    /// report [`CellGridControllerEvent::CutRequested`] for the host to
    /// clear through its engine — this controller has no cell content
    /// to clear itself (module doc). Shares one [`Self::selection_bounds`]
    /// call with the copy rather than calling through
    /// [`Self::copy_selection`] and recomputing it — each call clones
    /// every row-height/col-width/label `BTreeMap` via
    /// [`Self::geometry_grid`].
    pub fn cut_selection(
        &self,
        grid: &CellGrid,
        clipboard: &dyn Clipboard,
    ) -> CellGridControllerEvent {
        if self.row_count == 0 || self.col_count == 0 || self.is_editing() {
            return CellGridControllerEvent::Ignored;
        }
        let (top, left, bottom, right) = self.selection_bounds();
        clipboard.write_text(&grid.range_as_tsv(top, left, bottom, right));
        CellGridControllerEvent::CutRequested {
            top,
            left,
            bottom,
            right,
        }
    }

    /// Ctrl/Cmd+V: read `clipboard`'s current text and report
    /// [`CellGridControllerEvent::PasteRequested`], anchored at the
    /// current selection's top-left corner. `Ignored` when the
    /// clipboard holds no text, the sheet is empty (no cell for the
    /// anchor to name), or [`Self::is_editing`] — [`Self::handle`]
    /// already routes a paste into the in-progress edit itself in that
    /// case (`UiEvent::ClipboardPaste`), so a host calling this
    /// unconditionally on Ctrl+V must not also report a *range* paste
    /// at the selection.
    pub fn paste(&self, clipboard: &dyn Clipboard) -> CellGridControllerEvent {
        if self.row_count == 0 || self.col_count == 0 || self.is_editing() {
            return CellGridControllerEvent::Ignored;
        }
        let Some(text) = clipboard.read_text() else {
            return CellGridControllerEvent::Ignored;
        };
        let (top, left, ..) = self.selection_bounds();
        CellGridControllerEvent::PasteRequested {
            anchor_row: top,
            anchor_col: left,
            text,
        }
    }

    // ── Handle ────────────────────────────────────────────────────────

    pub fn handle(
        &mut self,
        event: &UiEvent,
        backend: &dyn Backend,
        rect: Rect,
    ) -> CellGridControllerEvent {
        // An in-progress edit is always anchored to `self.cursor` (see
        // `CellEditing`'s own doc) — a mouse click that moved the cursor
        // without first committing or cancelling would leave the
        // painted editor and the text buffer describing different
        // cells. So while editing, only keyboard input (which this
        // controller fully owns the semantics of — commit, cancel,
        // insert, delete) is routed anywhere; mouse input is inert. A
        // host that wants click-to-commit calls
        // [`Self::cancel_editing`] or relies on [`Self::handle_edit_key`]'s
        // own commit keys before ever routing a click here.
        if self.editing.is_some() {
            return match event {
                UiEvent::KeyPressed { key, modifiers, .. } => {
                    self.handle_edit_key(key, modifiers, backend, rect)
                }
                UiEvent::CharTyped(ch) => self.apply_edit_op(EditOp::InsertChar(*ch)),
                UiEvent::ClipboardPaste(text) => {
                    self.apply_edit_op(EditOp::InsertText(text.clone()))
                }
                _ => CellGridControllerEvent::Consumed,
            };
        }
        match event {
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                modifiers,
                ..
            } => self.mouse_down(backend, rect, *position, modifiers),

            UiEvent::MouseMoved {
                position,
                buttons: ButtonMask { left: true, .. },
            } => self.mouse_drag(backend, rect, *position),

            UiEvent::MouseUp {
                button: MouseButton::Left,
                ..
            } => {
                self.drag_anchor = None;
                CellGridControllerEvent::Consumed
            }

            UiEvent::KeyPressed { key, modifiers, .. } => {
                self.handle_key(key, modifiers, backend, rect)
            }

            // Terminal bracketed paste (and any other backend that
            // delivers pasted text as a `UiEvent` rather than a literal
            // Ctrl+V keypress) arrives here even with no active edit —
            // unlike `Self::paste`, the text is already in the event,
            // so no `Clipboard` handle is needed to report it.
            UiEvent::ClipboardPaste(text) if self.row_count > 0 && self.col_count > 0 => {
                let (top, left, ..) = self.selection_bounds();
                CellGridControllerEvent::PasteRequested {
                    anchor_row: top,
                    anchor_col: left,
                    text: text.clone(),
                }
            }

            _ => CellGridControllerEvent::Ignored,
        }
    }

    fn mouse_down(
        &mut self,
        backend: &dyn Backend,
        rect: Rect,
        position: Point,
        modifiers: &Modifiers,
    ) -> CellGridControllerEvent {
        if !rect.contains(position) {
            return CellGridControllerEvent::Ignored;
        }
        let layout = self.layout(backend, rect);
        let result = match layout.hit_test(position.x, position.y) {
            CellGridHit::Cell(r, c) => self.set_cursor(r, c, modifiers.shift, backend, rect),
            CellGridHit::ColumnHeader(c) => self.select_column(c, backend, rect),
            CellGridHit::RowHeader(r) => self.select_row(r, backend, rect),
            CellGridHit::Corner => self.select_all(backend, rect),
            CellGridHit::VScrollbar | CellGridHit::HScrollbar | CellGridHit::Empty => {
                return CellGridControllerEvent::Ignored;
            }
        };
        // The drag anchor is always the *fixed* corner of whatever
        // selection the press just produced — `self.cursor` for a plain
        // click (no prior anchor), or the still-standing anchor for a
        // Shift-click/header/corner press — never the just-clicked cell
        // itself when that differs, so a drag that follows continues
        // extending from the same corner the click established.
        self.drag_anchor = Some(self.selection_anchor.unwrap_or(self.cursor));
        result
    }

    fn mouse_drag(
        &mut self,
        backend: &dyn Backend,
        rect: Rect,
        position: Point,
    ) -> CellGridControllerEvent {
        let Some(anchor) = self.drag_anchor else {
            return CellGridControllerEvent::Ignored;
        };
        let layout = self.layout(backend, rect);
        let CellGridHit::Cell(r, c) = layout.hit_test(position.x, position.y) else {
            return CellGridControllerEvent::Consumed;
        };
        self.selection_anchor = Some(anchor);
        self.cursor = (r, c);
        self.ensure_cursor_visible(backend, rect);
        self.result_for_current_selection()
    }

    fn handle_key(
        &mut self,
        key: &Key,
        modifiers: &Modifiers,
        backend: &dyn Backend,
        rect: Rect,
    ) -> CellGridControllerEvent {
        let extend = modifiers.shift;
        let jump = modifiers.ctrl || modifiers.cmd;
        match key {
            Key::Named(NamedKey::Up) if jump => {
                self.set_cursor(0, self.cursor.1, extend, backend, rect)
            }
            Key::Named(NamedKey::Down) if jump => {
                let row = self.row_count.saturating_sub(1);
                self.set_cursor(row, self.cursor.1, extend, backend, rect)
            }
            Key::Named(NamedKey::Left) if jump => {
                self.set_cursor(self.cursor.0, 0, extend, backend, rect)
            }
            Key::Named(NamedKey::Right) if jump => {
                let col = self.col_count.saturating_sub(1);
                self.set_cursor(self.cursor.0, col, extend, backend, rect)
            }
            Key::Named(NamedKey::Up) => self.move_by(-1, 0, extend, backend, rect),
            Key::Named(NamedKey::Down) => self.move_by(1, 0, extend, backend, rect),
            Key::Named(NamedKey::Left) => self.move_by(0, -1, extend, backend, rect),
            Key::Named(NamedKey::Right) => self.move_by(0, 1, extend, backend, rect),

            // A translating backend reports Shift+Tab as `BackTab`
            // (see `NamedKey::BackTab`'s doc); a raw `Tab` with the
            // shift bit still set is handled the same way so either
            // convention works.
            Key::Named(NamedKey::Tab) if !modifiers.shift => self.move_tab(true, backend, rect),
            Key::Named(NamedKey::Tab) | Key::Named(NamedKey::BackTab) => {
                self.move_tab(false, backend, rect)
            }

            Key::Named(NamedKey::Enter) => self.move_enter(!modifiers.shift, backend, rect),

            Key::Named(NamedKey::PageUp) => self.page(false, extend, backend, rect),
            Key::Named(NamedKey::PageDown) => self.page(true, extend, backend, rect),

            Key::Named(NamedKey::Home) if jump => self.set_cursor(0, 0, extend, backend, rect),
            Key::Named(NamedKey::End) if jump => {
                let row = self.row_count.saturating_sub(1);
                let col = self.col_count.saturating_sub(1);
                self.set_cursor(row, col, extend, backend, rect)
            }
            Key::Named(NamedKey::Home) => {
                let row = self.cursor.0;
                self.set_cursor(row, 0, extend, backend, rect)
            }
            Key::Named(NamedKey::End) => {
                let row = self.cursor.0;
                let col = self.col_count.saturating_sub(1);
                self.set_cursor(row, col, extend, backend, rect)
            }

            Key::Char('a') if jump => self.select_all(backend, rect),

            // F2: the controller has no cell content to seed the editor
            // with (see the module doc), so it only reports the
            // request — the host calls `start_editing` with that cell's
            // current raw text.
            Key::Named(NamedKey::F(2)) => self.request_edit(),

            // Any other plain character starts a fresh edit seeded with
            // just that character, replacing whatever was there — the
            // same "typing replaces" convention
            // `TreeController::start_editing`'s own doc describes for a
            // rename. Must come after the `Key::Char('a') if jump` arm
            // above so Ctrl/Cmd+A still selects the whole sheet instead
            // of starting an edit.
            Key::Char(c) if !jump => self.begin_typing(*c),

            _ => CellGridControllerEvent::Ignored,
        }
    }

    /// The controller has no cell content of its own (see the module
    /// doc), so F2 only reports the request; see
    /// [`CellGridControllerEvent::CellEditRequested`]'s own doc.
    fn request_edit(&self) -> CellGridControllerEvent {
        if self.row_count == 0 || self.col_count == 0 {
            return CellGridControllerEvent::Ignored;
        }
        let (row, col) = self.cursor;
        CellGridControllerEvent::CellEditRequested { row, col }
    }

    /// A plain character typed at the cursor with no edit in progress —
    /// starts a fresh one seeded with just that character.
    fn begin_typing(&mut self, c: char) -> CellGridControllerEvent {
        if self.row_count == 0 || self.col_count == 0 {
            return CellGridControllerEvent::Ignored;
        }
        let text = c.to_string();
        let cursor = text.len();
        self.begin_edit(text, cursor);
        let (row, col) = self.cursor;
        CellGridControllerEvent::CellEditStarted { row, col }
    }

    /// Open the editor at [`Self::cursor`], seeded with `text` and the
    /// cursor placed at its end. Shared by [`Self::start_editing`]
    /// (host-initiated, e.g. F2) and [`Self::begin_typing`]
    /// (self-initiated, from a plain keypress).
    fn begin_edit(&mut self, text: String, cursor: usize) {
        let field = TextField::new(WidgetId::new(format!("{}:editor", self.id.as_str())))
            .with_value(text)
            .with_cursor(Some(cursor))
            .with_has_focus(true);
        self.editing = Some(CellEditing { field });
    }

    /// Take the in-progress edit's text, if any — `(row, col)` is
    /// always [`Self::cursor`] (see `CellEditing`'s own doc), captured
    /// *before* a caller moves it.
    fn commit_edit(&mut self) -> Option<(usize, usize, String)> {
        let editing = self.editing.take()?;
        let (row, col) = self.cursor;
        Some((row, col, editing.field.value))
    }

    /// Discard the in-progress edit, if any, returning the cell it was
    /// editing.
    fn cancel_edit(&mut self) -> Option<(usize, usize)> {
        self.editing.take()?;
        Some(self.cursor)
    }

    /// Apply one [`EditOp`] to the in-progress edit's text buffer —
    /// insert/delete/move/select, reusing [`TextField::apply_edit_op`]
    /// rather than re-deriving multibyte-safe cursor/selection math (see
    /// the module doc's "Reuse `TextField`" convention).
    fn apply_edit_op(&mut self, op: EditOp) -> CellGridControllerEvent {
        let Some(editing) = self.editing.as_mut() else {
            return CellGridControllerEvent::Ignored;
        };
        if !editing.field.apply_edit_op(op) {
            return CellGridControllerEvent::Consumed;
        }
        let (row, col) = self.cursor;
        CellGridControllerEvent::CellEditChanged {
            row,
            col,
            text: editing.field.value.clone(),
        }
    }

    fn commit_and_move_enter(
        &mut self,
        forward: bool,
        backend: &dyn Backend,
        rect: Rect,
    ) -> CellGridControllerEvent {
        let Some((row, col, text)) = self.commit_edit() else {
            return CellGridControllerEvent::Ignored;
        };
        self.move_enter(forward, backend, rect);
        CellGridControllerEvent::CellEditCommitted { row, col, text }
    }

    fn commit_and_move_tab(
        &mut self,
        forward: bool,
        backend: &dyn Backend,
        rect: Rect,
    ) -> CellGridControllerEvent {
        let Some((row, col, text)) = self.commit_edit() else {
            return CellGridControllerEvent::Ignored;
        };
        self.move_tab(forward, backend, rect);
        CellGridControllerEvent::CellEditCommitted { row, col, text }
    }

    fn commit_and_move_by(
        &mut self,
        d_row: isize,
        d_col: isize,
        backend: &dyn Backend,
        rect: Rect,
    ) -> CellGridControllerEvent {
        let Some((row, col, text)) = self.commit_edit() else {
            return CellGridControllerEvent::Ignored;
        };
        self.move_by(d_row, d_col, false, backend, rect);
        CellGridControllerEvent::CellEditCommitted { row, col, text }
    }

    /// Dispatch one key event while an edit is in progress — Escape
    /// cancels; Enter, Tab and the arrow keys commit and move (as in
    /// Excel, per the module doc); everything else edits the text
    /// buffer. Shared by [`Self::handle`] (typing directly into the
    /// grid) and [`Self::handle_formula_bar_key`] (typing into the
    /// formula bar), so both commit the same way.
    fn handle_edit_key(
        &mut self,
        key: &Key,
        modifiers: &Modifiers,
        backend: &dyn Backend,
        rect: Rect,
    ) -> CellGridControllerEvent {
        match key {
            Key::Named(NamedKey::Escape) => match self.cancel_edit() {
                Some((row, col)) => CellGridControllerEvent::CellEditCancelled { row, col },
                None => CellGridControllerEvent::Ignored,
            },

            Key::Named(NamedKey::Enter) => {
                self.commit_and_move_enter(!modifiers.shift, backend, rect)
            }

            Key::Named(NamedKey::Tab) if !modifiers.shift => {
                self.commit_and_move_tab(true, backend, rect)
            }
            Key::Named(NamedKey::Tab) | Key::Named(NamedKey::BackTab) => {
                self.commit_and_move_tab(false, backend, rect)
            }

            Key::Named(NamedKey::Up) => self.commit_and_move_by(-1, 0, backend, rect),
            Key::Named(NamedKey::Down) => self.commit_and_move_by(1, 0, backend, rect),
            Key::Named(NamedKey::Left) => self.commit_and_move_by(0, -1, backend, rect),
            Key::Named(NamedKey::Right) => self.commit_and_move_by(0, 1, backend, rect),

            Key::Named(NamedKey::Backspace) => self.apply_edit_op(EditOp::DeleteBackward),
            Key::Named(NamedKey::Delete) => self.apply_edit_op(EditOp::DeleteForward),
            Key::Named(NamedKey::Home) => {
                self.apply_edit_op(EditOp::MoveLineStart { extend: false })
            }
            Key::Named(NamedKey::End) => self.apply_edit_op(EditOp::MoveLineEnd { extend: false }),

            Key::Char('a') if modifiers.ctrl || modifiers.cmd => {
                self.apply_edit_op(EditOp::SelectAll)
            }
            Key::Char(c) => self.apply_edit_op(EditOp::InsertChar(*c)),

            _ => CellGridControllerEvent::Consumed,
        }
    }
}

#[cfg(all(test, feature = "tui"))]
mod tests {
    use super::*;
    use crate::tui::TuiBackend;

    const RECT: Rect = Rect::new(0.0, 0.0, 30.0, 10.0);

    fn controller(rows: usize, cols: usize) -> CellGridController {
        let mut c = CellGridController::new("grid");
        c.set_dimensions(rows, cols);
        c
    }

    fn key(named: NamedKey, modifiers: Modifiers) -> UiEvent {
        UiEvent::KeyPressed {
            key: Key::Named(named),
            modifiers,
            repeat: false,
        }
    }

    fn shift() -> Modifiers {
        Modifiers {
            shift: true,
            ..Default::default()
        }
    }

    fn ctrl() -> Modifiers {
        Modifiers {
            ctrl: true,
            ..Default::default()
        }
    }

    fn ctrl_shift() -> Modifiers {
        Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        }
    }

    fn backend() -> TuiBackend {
        TuiBackend::new()
    }

    // ── Arrows ───────────────────────────────────────────────────────

    #[test]
    fn arrows_move_the_cursor_one_cell_and_clamp_at_the_sheet_edge() {
        let mut c = controller(5, 5);
        let b = backend();
        assert_eq!(
            c.handle(&key(NamedKey::Down, Modifiers::default()), &b, RECT),
            CellGridControllerEvent::CursorMoved { row: 1, col: 0 }
        );
        assert_eq!(
            c.handle(&key(NamedKey::Right, Modifiers::default()), &b, RECT),
            CellGridControllerEvent::CursorMoved { row: 1, col: 1 }
        );
        // Up past the top clamps instead of going negative.
        c.handle(&key(NamedKey::Up, Modifiers::default()), &b, RECT);
        assert_eq!(
            c.handle(&key(NamedKey::Up, Modifiers::default()), &b, RECT),
            CellGridControllerEvent::CursorMoved { row: 0, col: 1 }
        );
    }

    #[test]
    fn shift_arrow_extends_the_selection_from_the_cursor() {
        let mut c = controller(10, 10);
        let b = backend();
        c.handle(&key(NamedKey::Down, Modifiers::default()), &b, RECT);
        c.handle(&key(NamedKey::Right, Modifiers::default()), &b, RECT);
        assert_eq!(c.cursor(), (1, 1));

        let ev = c.handle(&key(NamedKey::Down, shift()), &b, RECT);
        assert_eq!(c.selection_anchor(), Some((1, 1)));
        assert_eq!(
            ev,
            CellGridControllerEvent::SelectionChanged {
                top: 1,
                left: 1,
                bottom: 2,
                right: 1,
            }
        );

        // A further plain (non-Shift) arrow collapses the selection back
        // to a single cursor cell.
        let ev = c.handle(&key(NamedKey::Down, Modifiers::default()), &b, RECT);
        assert_eq!(c.selection_anchor(), None);
        assert_eq!(ev, CellGridControllerEvent::CursorMoved { row: 3, col: 1 });
    }

    #[test]
    fn dragging_back_past_the_anchor_keeps_bounds_normalized() {
        let mut c = controller(10, 10);
        let b = backend();
        c.set_selection((5, 5), None);
        c.handle(&key(NamedKey::Up, shift()), &b, RECT);
        c.handle(&key(NamedKey::Up, shift()), &b, RECT);
        assert_eq!(c.cursor(), (3, 5));
        assert_eq!(c.selection_anchor(), Some((5, 5)));
        assert_eq!(c.selection_bounds(), (3, 5, 5, 5));
    }

    // ── Tab / Shift+Tab ─────────────────────────────────────────────

    #[test]
    fn tab_moves_across_the_row_and_wraps_to_the_next_row() {
        let mut c = controller(3, 2);
        let b = backend();
        c.handle(&key(NamedKey::Tab, Modifiers::default()), &b, RECT);
        assert_eq!(c.cursor(), (0, 1));
        let ev = c.handle(&key(NamedKey::Tab, Modifiers::default()), &b, RECT);
        assert_eq!(c.cursor(), (1, 0));
        assert_eq!(ev, CellGridControllerEvent::CursorMoved { row: 1, col: 0 });
    }

    #[test]
    fn shift_tab_moves_backward_and_wraps_to_the_previous_row() {
        let mut c = controller(3, 2);
        let b = backend();
        c.set_selection((1, 0), None);
        let ev = c.handle(&key(NamedKey::BackTab, Modifiers::default()), &b, RECT);
        assert_eq!(c.cursor(), (0, 1));
        assert_eq!(ev, CellGridControllerEvent::CursorMoved { row: 0, col: 1 });
    }

    #[test]
    fn tab_never_extends_a_selection_even_while_one_is_active() {
        let mut c = controller(5, 5);
        let b = backend();
        c.handle(&key(NamedKey::Right, shift()), &b, RECT);
        assert!(c.selection_anchor().is_some());
        c.handle(&key(NamedKey::Tab, Modifiers::default()), &b, RECT);
        assert_eq!(c.selection_anchor(), None);
    }

    // ── Enter / Shift+Enter ──────────────────────────────────────────

    #[test]
    fn enter_moves_down_and_shift_enter_moves_up_within_the_column() {
        let mut c = controller(5, 5);
        let b = backend();
        c.set_selection((2, 3), None);
        c.handle(&key(NamedKey::Enter, Modifiers::default()), &b, RECT);
        assert_eq!(c.cursor(), (3, 3));
        c.handle(&key(NamedKey::Enter, shift()), &b, RECT);
        assert_eq!(c.cursor(), (2, 3));
    }

    // ── Home / End / Ctrl+Home / Ctrl+End ────────────────────────────

    #[test]
    fn home_and_end_move_within_the_current_row_only() {
        let mut c = controller(5, 8);
        let b = backend();
        c.set_selection((2, 4), None);
        c.handle(&key(NamedKey::End, Modifiers::default()), &b, RECT);
        assert_eq!(c.cursor(), (2, 7));
        c.handle(&key(NamedKey::Home, Modifiers::default()), &b, RECT);
        assert_eq!(c.cursor(), (2, 0));
    }

    #[test]
    fn ctrl_home_and_ctrl_end_jump_to_the_sheet_corners() {
        let mut c = controller(5, 8);
        let b = backend();
        c.set_selection((2, 4), None);
        c.handle(&key(NamedKey::End, ctrl()), &b, RECT);
        assert_eq!(c.cursor(), (4, 7));
        c.handle(&key(NamedKey::Home, ctrl()), &b, RECT);
        assert_eq!(c.cursor(), (0, 0));
    }

    // ── Ctrl+Arrow jumps to the sheet edge ───────────────────────────

    #[test]
    fn ctrl_arrow_jumps_along_one_axis_to_the_sheet_edge() {
        let mut c = controller(20, 20);
        let b = backend();
        c.set_selection((5, 5), None);
        c.handle(&key(NamedKey::Right, ctrl()), &b, RECT);
        assert_eq!(c.cursor(), (5, 19));
        c.handle(&key(NamedKey::Down, ctrl()), &b, RECT);
        assert_eq!(c.cursor(), (19, 19));
    }

    #[test]
    fn ctrl_shift_arrow_jumps_and_extends_the_selection() {
        let mut c = controller(20, 20);
        let b = backend();
        c.set_selection((5, 5), None);
        let ev = c.handle(&key(NamedKey::Right, ctrl_shift()), &b, RECT);
        assert_eq!(
            ev,
            CellGridControllerEvent::SelectionChanged {
                top: 5,
                left: 5,
                bottom: 5,
                right: 19,
            }
        );
    }

    // ── Page Up / Page Down ───────────────────────────────────────────

    #[test]
    fn page_down_and_page_up_move_by_the_visible_row_count() {
        // TUI row height == 1.0, no headers/scrollbar reserved below —
        // `RECT`'s 10-row height is the exact body row capacity.
        let mut c = controller(100, 5);
        c.set_show_row_headers(false);
        c.set_show_col_headers(false);
        let b = backend();
        c.handle(&key(NamedKey::PageDown, Modifiers::default()), &b, RECT);
        assert_eq!(c.cursor(), (10, 0));
        c.handle(&key(NamedKey::PageUp, Modifiers::default()), &b, RECT);
        assert_eq!(c.cursor(), (0, 0));
    }

    // ── Select all / row / column ─────────────────────────────────────

    #[test]
    fn ctrl_a_selects_the_whole_sheet() {
        let mut c = controller(50, 20);
        let b = backend();
        c.set_selection((5, 5), None);
        let ev = c.handle(
            &UiEvent::KeyPressed {
                key: Key::Char('a'),
                modifiers: ctrl(),
                repeat: false,
            },
            &b,
            RECT,
        );
        assert_eq!(
            ev,
            CellGridControllerEvent::SelectionChanged {
                top: 0,
                left: 0,
                bottom: 49,
                right: 19,
            }
        );
    }

    // ── Click / click-drag ────────────────────────────────────────────

    fn mouse_down(x: f32, y: f32, modifiers: Modifiers) -> UiEvent {
        UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(x, y),
            modifiers,
        }
    }

    fn mouse_moved(x: f32, y: f32) -> UiEvent {
        UiEvent::MouseMoved {
            position: Point::new(x, y),
            buttons: ButtonMask {
                left: true,
                ..Default::default()
            },
        }
    }

    fn mouse_up(x: f32, y: f32) -> UiEvent {
        UiEvent::MouseUp {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(x, y),
        }
    }

    /// Headers on (defaults), so body cells start 1 column/1 row in.
    #[test]
    fn click_moves_the_cursor_to_the_clicked_cell() {
        let mut c = controller(20, 20);
        let b = backend();
        let ev = c.handle(&mouse_down(3.0, 3.0, Modifiers::default()), &b, RECT);
        assert!(matches!(ev, CellGridControllerEvent::CursorMoved { .. }));
        assert_eq!(c.selection_anchor(), None);
    }

    #[test]
    fn click_drag_selects_a_rectangular_range() {
        // With default headers on, body cells for a 20-row/20-col sheet
        // start at `x >= 3.0` (2-digit row numbers + 1 gutter column)
        // and `y >= 1.0` (the column-header row) — `(5.0, 2.0)` lands on
        // the first body column's second row.
        let mut c = controller(20, 20);
        let b = backend();
        c.handle(&mouse_down(5.0, 2.0, Modifiers::default()), &b, RECT);
        let start = c.cursor();
        let ev = c.handle(&mouse_moved(12.0, 5.0), &b, RECT);
        assert!(matches!(
            ev,
            CellGridControllerEvent::SelectionChanged { .. }
        ));
        assert_eq!(c.selection_anchor(), Some(start));
        c.handle(&mouse_up(12.0, 5.0), &b, RECT);
        // Once released, further drag-shaped `MouseMoved`s have nothing
        // to extend.
        let ev = c.handle(&mouse_moved(5.0, 2.0), &b, RECT);
        assert_eq!(ev, CellGridControllerEvent::Ignored);
    }

    #[test]
    fn shift_click_extends_from_the_existing_cursor() {
        let mut c = controller(20, 20);
        let b = backend();
        c.handle(&mouse_down(5.0, 2.0, Modifiers::default()), &b, RECT);
        let start = c.cursor();
        c.handle(
            &mouse_down(
                12.0,
                5.0,
                Modifiers {
                    shift: true,
                    ..Default::default()
                },
            ),
            &b,
            RECT,
        );
        assert_eq!(c.selection_anchor(), Some(start));
    }

    // ── Header / corner clicks ────────────────────────────────────────

    #[test]
    fn clicking_a_column_header_selects_the_whole_column() {
        let mut c = controller(20, 20);
        let b = backend();
        // Column header band is the top `col_header_height` rows; row
        // headers occupy the leftmost column, so x just past that lands
        // on the first body column's header.
        let ev = c.handle(&mouse_down(3.0, 0.0, Modifiers::default()), &b, RECT);
        match ev {
            CellGridControllerEvent::SelectionChanged {
                top,
                bottom,
                left,
                right,
            } => {
                assert_eq!((top, bottom), (0, 19));
                assert_eq!(left, right);
            }
            other => panic!("expected SelectionChanged, got {other:?}"),
        }
    }

    #[test]
    fn clicking_a_row_header_selects_the_whole_row() {
        let mut c = controller(20, 20);
        let b = backend();
        let ev = c.handle(&mouse_down(0.0, 3.0, Modifiers::default()), &b, RECT);
        match ev {
            CellGridControllerEvent::SelectionChanged {
                left,
                right,
                top,
                bottom,
            } => {
                assert_eq!((left, right), (0, 19));
                assert_eq!(top, bottom);
            }
            other => panic!("expected SelectionChanged, got {other:?}"),
        }
    }

    #[test]
    fn clicking_the_corner_selects_everything() {
        let mut c = controller(20, 30);
        let b = backend();
        let ev = c.handle(&mouse_down(0.0, 0.0, Modifiers::default()), &b, RECT);
        assert_eq!(
            ev,
            CellGridControllerEvent::SelectionChanged {
                top: 0,
                left: 0,
                bottom: 19,
                right: 29,
            }
        );
    }

    // ── Keep-visible scrolling across a frozen boundary ───────────────

    #[test]
    fn moving_the_cursor_past_the_frozen_boundary_scrolls_the_body_only() {
        let mut c = controller(100, 100);
        c.set_frozen(1, 1);
        c.set_show_row_headers(false);
        c.set_show_col_headers(false);
        let b = backend();
        // `RECT` is 10 rows tall: 1 frozen + 9 body rows fit.
        for _ in 0..12 {
            c.handle(&key(NamedKey::Down, Modifiers::default()), &b, RECT);
        }
        assert_eq!(c.cursor(), (12, 0));
        // Body rank is `12 - 1 == 11`; a 9-row body window following the
        // cursor sits with its last row at that rank, i.e. offset `3`.
        assert_eq!(c.row_scroll(), 3);
    }

    #[test]
    fn a_cursor_inside_the_frozen_rows_never_scrolls_the_body() {
        let mut c = controller(100, 100);
        c.set_frozen(2, 0);
        c.set_show_row_headers(false);
        c.set_show_col_headers(false);
        let b = backend();
        c.set_selection((50, 0), None);
        c.handle(&key(NamedKey::Up, Modifiers::default()), &b, RECT);
        let scrolled = c.row_scroll();
        assert!(scrolled > 0, "scrolling down first should have moved it");
        // Jumping straight into the frozen band leaves the body scroll
        // wherever it already was — a frozen cell is always visible, no
        // scroll adjustment needed.
        c.handle(&key(NamedKey::Up, ctrl()), &b, RECT);
        assert_eq!(c.cursor(), (0, 0));
        assert_eq!(c.row_scroll(), scrolled);
    }

    #[test]
    fn moving_left_past_the_frozen_column_scrolls_back_into_view() {
        let mut c = controller(10, 100);
        c.set_frozen(0, 1);
        c.set_show_row_headers(false);
        c.set_show_col_headers(false);
        let b = backend();
        c.set_selection((0, 50), None);
        // Force a scroll far to the right first.
        c.handle(&key(NamedKey::Right, ctrl()), &b, RECT);
        assert!(c.col_scroll() > 0);
        c.set_selection((0, 2), None);
        c.handle(&key(NamedKey::Left, Modifiers::default()), &b, RECT);
        assert_eq!(c.cursor(), (0, 1));
        assert_eq!(c.col_scroll(), 0);
    }

    // ── set_dimensions ───────────────────────────────────────────────

    #[test]
    fn idempotent_set_dimensions_preserves_an_active_range_selection() {
        let mut c = controller(10, 10);
        let b = backend();
        // Shift-extend a B2:C3 range (0-indexed: anchor (2, 2), cursor
        // (1, 1) after extending back up and to the left).
        c.handle(&key(NamedKey::Down, Modifiers::default()), &b, RECT);
        c.handle(&key(NamedKey::Down, Modifiers::default()), &b, RECT);
        c.handle(&key(NamedKey::Right, Modifiers::default()), &b, RECT);
        c.handle(&key(NamedKey::Right, Modifiers::default()), &b, RECT);
        c.handle(&key(NamedKey::Up, shift()), &b, RECT);
        c.handle(&key(NamedKey::Left, shift()), &b, RECT);
        assert_eq!(c.selection_bounds(), (1, 1, 2, 2));

        // A host re-declaring the same dimensions every frame must not
        // disturb the selection.
        c.set_dimensions(10, 10);
        assert_eq!(c.selection_anchor(), Some((2, 2)));
        assert_eq!(c.cursor(), (1, 1));
        assert_eq!(c.selection_bounds(), (1, 1, 2, 2));
    }

    #[test]
    fn set_dimensions_drops_the_anchor_only_when_a_shrink_puts_it_out_of_range() {
        let mut c = controller(10, 10);
        let b = backend();
        c.handle(&key(NamedKey::Down, Modifiers::default()), &b, RECT);
        c.handle(&key(NamedKey::Down, Modifiers::default()), &b, RECT);
        c.handle(&key(NamedKey::Right, Modifiers::default()), &b, RECT);
        c.handle(&key(NamedKey::Right, Modifiers::default()), &b, RECT);
        c.handle(&key(NamedKey::Up, shift()), &b, RECT);
        c.handle(&key(NamedKey::Left, shift()), &b, RECT);
        assert_eq!(c.selection_bounds(), (1, 1, 2, 2));

        // Shrinking to a size that still contains both cursor and anchor
        // leaves the selection intact.
        c.set_dimensions(3, 3);
        assert_eq!(c.selection_anchor(), Some((2, 2)));

        // Shrinking past the anchor collapses it, leaving a single valid
        // cursor cell rather than a stale out-of-range anchor.
        c.set_dimensions(2, 2);
        assert_eq!(c.selection_anchor(), None);
        assert_eq!(c.cursor(), (1, 1));
    }

    // ── Inline editing ────────────────────────────────────────────────

    fn char_key(c: char, modifiers: Modifiers) -> UiEvent {
        UiEvent::KeyPressed {
            key: Key::Char(c),
            modifiers,
            repeat: false,
        }
    }

    #[test]
    fn typing_a_character_starts_an_edit_seeded_with_just_that_character() {
        let mut c = controller(5, 5);
        let b = backend();
        let ev = c.handle(&char_key('x', Modifiers::default()), &b, RECT);
        assert_eq!(
            ev,
            CellGridControllerEvent::CellEditStarted { row: 0, col: 0 }
        );
        assert!(c.is_editing());
        assert_eq!(c.editing_text(), Some("x"));
        assert_eq!(c.editing_cursor(), Some(1));
    }

    #[test]
    fn f2_reports_a_request_instead_of_seeding_the_editor_itself() {
        let mut c = controller(5, 5);
        let b = backend();
        let ev = c.handle(&key(NamedKey::F(2), Modifiers::default()), &b, RECT);
        assert_eq!(
            ev,
            CellGridControllerEvent::CellEditRequested { row: 0, col: 0 }
        );
        assert!(!c.is_editing());
    }

    #[test]
    fn start_editing_seeds_the_cursor_at_the_end_of_the_supplied_text() {
        let mut c = controller(5, 5);
        let ev = c.start_editing("existing");
        assert_eq!(
            ev,
            CellGridControllerEvent::CellEditStarted { row: 0, col: 0 }
        );
        assert_eq!(c.editing_text(), Some("existing"));
        assert_eq!(c.editing_cursor(), Some("existing".len()));
        assert_eq!(c.editing_selection_anchor(), None);
    }

    #[test]
    fn escape_cancels_the_edit_and_leaves_the_cursor_untouched() {
        let mut c = controller(5, 5);
        let b = backend();
        c.handle(&char_key('x', Modifiers::default()), &b, RECT);
        let ev = c.handle(&key(NamedKey::Escape, Modifiers::default()), &b, RECT);
        assert_eq!(
            ev,
            CellGridControllerEvent::CellEditCancelled { row: 0, col: 0 }
        );
        assert!(!c.is_editing());
        assert_eq!(c.cursor(), (0, 0));
    }

    #[test]
    fn typed_characters_append_and_backspace_deletes() {
        let mut c = controller(5, 5);
        let b = backend();
        c.handle(&char_key('a', Modifiers::default()), &b, RECT);
        c.handle(&char_key('b', Modifiers::default()), &b, RECT);
        let ev = c.handle(&char_key('c', Modifiers::default()), &b, RECT);
        assert_eq!(
            ev,
            CellGridControllerEvent::CellEditChanged {
                row: 0,
                col: 0,
                text: "abc".into(),
            }
        );
        c.handle(&key(NamedKey::Backspace, Modifiers::default()), &b, RECT);
        assert_eq!(c.editing_text(), Some("ab"));
    }

    #[test]
    fn enter_commits_and_moves_down() {
        let mut c = controller(5, 5);
        let b = backend();
        c.handle(&char_key('=', Modifiers::default()), &b, RECT);
        c.handle(&char_key('3', Modifiers::default()), &b, RECT);
        let ev = c.handle(&key(NamedKey::Enter, Modifiers::default()), &b, RECT);
        assert_eq!(
            ev,
            CellGridControllerEvent::CellEditCommitted {
                row: 0,
                col: 0,
                text: "=3".into(),
            }
        );
        assert!(!c.is_editing());
        assert_eq!(c.cursor(), (1, 0));
    }

    #[test]
    fn shift_enter_commits_and_moves_up() {
        let mut c = controller(5, 5);
        let b = backend();
        c.set_selection((2, 0), None);
        c.handle(&char_key('y', Modifiers::default()), &b, RECT);
        c.handle(&key(NamedKey::Enter, shift()), &b, RECT);
        assert_eq!(c.cursor(), (1, 0));
    }

    #[test]
    fn tab_commits_and_moves_right_shift_tab_moves_left() {
        let mut c = controller(5, 5);
        let b = backend();
        c.handle(&char_key('v', Modifiers::default()), &b, RECT);
        let ev = c.handle(&key(NamedKey::Tab, Modifiers::default()), &b, RECT);
        assert_eq!(
            ev,
            CellGridControllerEvent::CellEditCommitted {
                row: 0,
                col: 0,
                text: "v".into(),
            }
        );
        assert_eq!(c.cursor(), (0, 1));

        c.handle(&char_key('w', Modifiers::default()), &b, RECT);
        c.handle(&key(NamedKey::BackTab, Modifiers::default()), &b, RECT);
        assert_eq!(c.cursor(), (0, 0));
    }

    #[test]
    fn arrow_keys_commit_and_move_in_every_direction() {
        let mut c = controller(5, 5);
        let b = backend();
        c.set_selection((2, 2), None);

        c.handle(&char_key('n', Modifiers::default()), &b, RECT);
        c.handle(&key(NamedKey::Up, Modifiers::default()), &b, RECT);
        assert_eq!(c.cursor(), (1, 2));

        c.handle(&char_key('s', Modifiers::default()), &b, RECT);
        c.handle(&key(NamedKey::Down, Modifiers::default()), &b, RECT);
        assert_eq!(c.cursor(), (2, 2));

        c.handle(&char_key('e', Modifiers::default()), &b, RECT);
        let ev = c.handle(&key(NamedKey::Right, Modifiers::default()), &b, RECT);
        assert_eq!(
            ev,
            CellGridControllerEvent::CellEditCommitted {
                row: 2,
                col: 2,
                text: "e".into(),
            }
        );
        assert_eq!(c.cursor(), (2, 3));

        c.handle(&char_key('w', Modifiers::default()), &b, RECT);
        c.handle(&key(NamedKey::Left, Modifiers::default()), &b, RECT);
        assert_eq!(c.cursor(), (2, 2));
    }

    #[test]
    fn cancel_editing_discards_the_text_without_moving_the_cursor() {
        let mut c = controller(5, 5);
        c.start_editing("scratch");
        let ev = c.cancel_editing();
        assert_eq!(
            ev,
            CellGridControllerEvent::CellEditCancelled { row: 0, col: 0 }
        );
        assert!(!c.is_editing());
        assert_eq!(c.cursor(), (0, 0));
    }

    #[test]
    fn set_dimensions_drops_an_in_progress_edit_if_the_cursor_is_forced_to_move() {
        let mut c = controller(5, 5);
        c.set_selection((4, 4), None);
        c.start_editing("stale");
        assert!(c.is_editing());
        // Shrinking past the cursor forces it to move, so the edit (which
        // has no identity apart from "the cursor cell") can't survive —
        // see `set_dimensions`'s own doc.
        c.set_dimensions(2, 2);
        assert!(!c.is_editing());
    }

    #[test]
    fn mouse_input_is_inert_while_editing() {
        let mut c = controller(20, 20);
        let b = backend();
        c.handle(&char_key('z', Modifiers::default()), &b, RECT);
        let ev = c.handle(&mouse_down(5.0, 2.0, Modifiers::default()), &b, RECT);
        assert_eq!(ev, CellGridControllerEvent::Consumed);
        assert!(c.is_editing());
        assert_eq!(c.cursor(), (0, 0));
    }

    #[test]
    fn formula_bar_sync_mirrors_the_in_progress_edit() {
        let mut c = controller(5, 5);
        c.start_editing("hello");
        let mut field = TextField::new(WidgetId::new("formula-bar"));
        c.sync_formula_bar(&mut field);
        assert_eq!(field.value, "hello");
        assert_eq!(field.cursor, Some(5));

        let b = backend();
        c.handle_formula_bar_key(&Key::Char('!'), &Modifiers::default(), &b, RECT);
        c.sync_formula_bar(&mut field);
        assert_eq!(field.value, "hello!");

        let ev = c.handle_formula_bar_key(
            &Key::Named(NamedKey::Enter),
            &Modifiers::default(),
            &b,
            RECT,
        );
        assert_eq!(
            ev,
            CellGridControllerEvent::CellEditCommitted {
                row: 0,
                col: 0,
                text: "hello!".into(),
            }
        );
        assert!(!c.is_editing());

        // Once committed, syncing again clears the caret rather than
        // leaving it pointed at the now-stale edit buffer.
        c.sync_formula_bar(&mut field);
        assert_eq!(field.cursor, None);
    }

    #[test]
    fn handle_formula_bar_key_is_ignored_with_no_active_edit() {
        let mut c = controller(5, 5);
        let b = backend();
        let ev = c.handle_formula_bar_key(&Key::Char('x'), &Modifiers::default(), &b, RECT);
        assert_eq!(ev, CellGridControllerEvent::Ignored);
        assert!(!c.is_editing());
    }

    // ── Clipboard ───────────────────────────────────────────────────

    /// A [`Clipboard`] test double backed by an in-process
    /// `RefCell<Option<String>>` — `read_text`/`write_text` round-trip
    /// through it so a test can assert exactly what a copy wrote,
    /// without touching the real OS clipboard the way a `TuiBackend`'s
    /// own [`crate::tui::services::TuiClipboard`] would.
    #[derive(Default)]
    struct FakeClipboard {
        text: std::cell::RefCell<Option<String>>,
    }

    impl Clipboard for FakeClipboard {
        fn read_text(&self) -> Option<String> {
            self.text.borrow().clone()
        }

        fn write_text(&self, text: &str) {
            *self.text.borrow_mut() = Some(text.to_string());
        }
    }

    /// A 3×3 [`CellGrid`] with real, fully-loaded cell text — unlike
    /// [`controller`]'s own geometry-only grid, copy needs real text to
    /// put on the clipboard.
    fn grid_3x3_with_text() -> CellGrid {
        let row = |label: &str| {
            (0..3)
                .map(|c| Cell::plain(format!("{label}{c}")))
                .collect::<Vec<_>>()
        };
        CellGrid {
            id: WidgetId::new("grid"),
            row_count: 3,
            col_count: 3,
            frozen_rows: 0,
            frozen_cols: 0,
            row_scroll: 0,
            col_scroll: 0,
            default_row_height: 1.0,
            default_col_width: 4.0,
            row_heights: BTreeMap::new(),
            col_widths: BTreeMap::new(),
            show_row_headers: true,
            show_col_headers: true,
            row_labels: BTreeMap::new(),
            col_labels: BTreeMap::new(),
            cursor: (0, 0),
            selection_anchor: None,
            cells: vec![row("r0"), row("r1"), row("r2")],
            loaded_row_start: 0,
            loaded_col_start: 0,
            has_focus: true,
            show_scrollbar: false,
            editing: None,
        }
    }

    #[test]
    fn copy_selection_writes_a_2x3_range_to_the_clipboard_as_tsv() {
        let mut c = controller(3, 3);
        c.set_selection((0, 0), Some((1, 2)));
        let grid = grid_3x3_with_text();
        let clipboard = FakeClipboard::default();

        let ev = c.copy_selection(&grid, &clipboard);

        assert_eq!(ev, CellGridControllerEvent::Consumed);
        assert_eq!(
            clipboard.read_text(),
            Some("r00\tr01\tr02\nr10\tr11\tr12".to_string())
        );
    }

    #[test]
    fn copy_selection_is_ignored_on_an_empty_sheet() {
        let c = controller(0, 0);
        let grid = grid_3x3_with_text();
        let clipboard = FakeClipboard::default();

        let ev = c.copy_selection(&grid, &clipboard);

        assert_eq!(ev, CellGridControllerEvent::Ignored);
        assert_eq!(clipboard.read_text(), None);
    }

    #[test]
    fn cut_selection_copies_then_reports_the_range_to_clear() {
        let mut c = controller(3, 3);
        c.set_selection((0, 0), Some((1, 2)));
        let grid = grid_3x3_with_text();
        let clipboard = FakeClipboard::default();

        let ev = c.cut_selection(&grid, &clipboard);

        assert_eq!(
            clipboard.read_text(),
            Some("r00\tr01\tr02\nr10\tr11\tr12".to_string())
        );
        assert_eq!(
            ev,
            CellGridControllerEvent::CutRequested {
                top: 0,
                left: 0,
                bottom: 1,
                right: 2,
            }
        );
    }

    #[test]
    fn copy_selection_reads_the_right_cells_with_a_frozen_pane_and_a_scroll() {
        // Regression for `grid()`'s `loaded_row_start`/`loaded_col_start`:
        // `row_scroll`/`col_scroll` are ranks *past* the frozen prefix
        // (`ensure_cursor_visible`'s own convention), so `grid()` must
        // add `frozen_rows`/`frozen_cols` back to land on an absolute
        // index — otherwise every copied cell is shifted by the frozen
        // count. `cells` is laid out positionally: row/col 0 is the
        // frozen pane, rows/cols 1.. are the scrolled body starting at
        // absolute `frozen + scroll` = 6.
        let mut c = controller(20, 20);
        c.set_frozen(1, 1);
        c.row_scroll = 5;
        c.col_scroll = 5;
        c.set_selection((6, 6), Some((7, 7)));

        let row = |label: &str| {
            (0..3)
                .map(|c| Cell::plain(format!("{label}{c}")))
                .collect::<Vec<_>>()
        };
        let grid = c.grid(vec![row("frozen"), row("r6"), row("r7")]);
        let clipboard = FakeClipboard::default();

        let ev = c.copy_selection(&grid, &clipboard);

        assert_eq!(ev, CellGridControllerEvent::Consumed);
        assert_eq!(
            clipboard.read_text(),
            Some("r61\tr62\nr71\tr72".to_string())
        );
    }

    #[test]
    fn paste_emits_the_selection_anchor_plus_the_clipboard_text() {
        let mut c = controller(5, 5);
        c.set_selection((2, 1), None);
        let clipboard = FakeClipboard::default();
        clipboard.write_text("a\tb\nc\td");

        let ev = c.paste(&clipboard);

        assert_eq!(
            ev,
            CellGridControllerEvent::PasteRequested {
                anchor_row: 2,
                anchor_col: 1,
                text: "a\tb\nc\td".to_string(),
            }
        );
    }

    #[test]
    fn paste_is_ignored_when_the_clipboard_holds_no_text() {
        let c = controller(5, 5);
        let clipboard = FakeClipboard::default();
        assert_eq!(c.paste(&clipboard), CellGridControllerEvent::Ignored);
    }

    #[test]
    fn handle_reports_a_clipboard_paste_event_even_with_no_active_edit() {
        let mut c = controller(5, 5);
        c.set_selection((2, 1), None);
        let b = backend();

        let ev = c.handle(&UiEvent::ClipboardPaste("a\tb".to_string()), &b, RECT);

        assert_eq!(
            ev,
            CellGridControllerEvent::PasteRequested {
                anchor_row: 2,
                anchor_col: 1,
                text: "a\tb".to_string(),
            }
        );
    }

    #[test]
    fn handle_ignores_a_clipboard_paste_event_on_an_empty_sheet() {
        let mut c = controller(0, 0);
        let b = backend();
        let ev = c.handle(&UiEvent::ClipboardPaste("a".to_string()), &b, RECT);
        assert_eq!(ev, CellGridControllerEvent::Ignored);
    }

    #[test]
    fn copy_cut_and_paste_are_ignored_while_editing() {
        let mut c = controller(3, 3);
        c.set_selection((0, 0), Some((1, 2)));
        c.start_editing("hello");
        let grid = grid_3x3_with_text();
        let clipboard = FakeClipboard::default();
        clipboard.write_text("preexisting");

        assert_eq!(
            c.copy_selection(&grid, &clipboard),
            CellGridControllerEvent::Ignored
        );
        assert_eq!(
            c.cut_selection(&grid, &clipboard),
            CellGridControllerEvent::Ignored
        );
        assert_eq!(c.paste(&clipboard), CellGridControllerEvent::Ignored);
        // Nothing wrote over the clipboard's prior contents.
        assert_eq!(clipboard.read_text(), Some("preexisting".to_string()));
    }

    #[test]
    fn paste_is_ignored_on_an_empty_sheet_even_with_clipboard_text() {
        let c = controller(0, 0);
        let clipboard = FakeClipboard::default();
        clipboard.write_text("a\tb");
        assert_eq!(c.paste(&clipboard), CellGridControllerEvent::Ignored);
    }
}
