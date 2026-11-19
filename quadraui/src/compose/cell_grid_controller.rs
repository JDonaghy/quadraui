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

use crate::backend::Backend;
use crate::primitive::{LayoutCtx, Primitive};
use crate::primitives::cell_grid::{Cell, CellGrid, CellGridHit, CellGridPrimitiveLayout};
use crate::primitives::scrollbar::scroll_by_clamped;
use crate::{ButtonMask, Key, Modifiers, MouseButton, NamedKey, Point, Rect, UiEvent, WidgetId};

/// What happened after [`CellGridController::handle`] processed an event,
/// or after calling one of its navigation methods directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
        }
    }

    // ── Per-frame / model data ────────────────────────────────────────

    /// Set the sheet's full size. The cursor is re-clamped into the new
    /// bounds and any selection beyond it collapses — a resize (e.g. a
    /// filtered-out set of rows) always leaves a single, valid cursor
    /// cell rather than a stale out-of-range anchor.
    pub fn set_dimensions(&mut self, row_count: usize, col_count: usize) {
        self.row_count = row_count;
        self.col_count = col_count;
        self.cursor = (
            self.cursor.0.min(row_count.saturating_sub(1)),
            self.cursor.1.min(col_count.saturating_sub(1)),
        );
        self.selection_anchor = None;
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

    // ── Rendering ─────────────────────────────────────────────────────

    /// Build the [`CellGrid`] this controller's current state describes,
    /// carrying `cells` (the host's windowed cell text for this frame —
    /// see [`crate::primitives::cell_grid`]'s module doc for what that
    /// window covers). What a host paints via
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
            has_focus: self.has_focus,
            show_scrollbar: self.show_scrollbar,
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

    // ── Handle ────────────────────────────────────────────────────────

    pub fn handle(
        &mut self,
        event: &UiEvent,
        backend: &dyn Backend,
        rect: Rect,
    ) -> CellGridControllerEvent {
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

            _ => CellGridControllerEvent::Ignored,
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
}
