//! `DataTable` primitive: a flat, scrollable multi-column table with
//! sortable headers and row selection.
//!
//! Distinct from `TreeTable` (hierarchical rows with expand/collapse)
//! per Decision D-002: list:tree :: DataTable:TreeTable. Column-sizing
//! helpers are shared via `quadraui::internal::columns` (not public).
//!
//! # Backend contract
//!
//! Render column headers (with sort indicators when `sort_column` is
//! set), then `rows[scroll_offset..]` until the viewport fills. Each
//! cell is a `StyledText` positioned within its column bounds.
//! Click on header → `DataTableEvent::HeaderClicked { col }`.
//! Click on row → `DataTableEvent::RowActivated { idx }`.
//! The app updates `selected_idx`, `scroll_offset`, and sort state
//! for the next frame.
//!
//! When `footer` is `Some`, render it pinned below the (possibly
//! shorter) visible body — laid out against the same resolved
//! columns, separated by a divider rule. The footer never scrolls,
//! is excluded from `visible_rows` / scrollbar math (reserved via
//! `DataTableLayout::footer_height`, which is `row_height * 2.0` — a
//! divider row plus the content row), and is not hit-testable as a
//! row (`DataTableHit::Footer`, not `Row`).
//!
//! # Coordinate spaces (`h_scroll`)
//!
//! [`ResolvedColumn::x`] lives in **content space**: it always starts at
//! `0.0` for the first column and runs to `content_width`, which may be
//! wider than the viewport when `min_total_width` is set. Backends paint
//! a column at `rc.x - h_scroll` (see `tui::data_table::draw_data_table`
//! and its GTK/macOS peers), so **viewport space** — the space every
//! click arrives in — is content space shifted left by `h_scroll`.
//!
//! Everything on [`DataTableLayout`] that takes an `x` from a pointer
//! ([`DataTableLayout::hit_test`], [`DataTableLayout::column_hit`],
//! [`DataTableLayout::drag_divider`]) therefore takes it in **viewport
//! space** and adds [`DataTableLayout::h_scroll`] back before comparing
//! against `columns`. At `h_scroll == 0.0` the two spaces coincide and
//! the conversion is a no-op (#550).
//!
//! ## `h_scroll` must already agree with what was painted
//!
//! Pointer positions are **raw pixel/cell coordinates** — for the TUI
//! backend, `Point::new(event.column as f32, event.row as f32)`
//! (`tui::events`), an integer cell index, *not* a cell-centre `+ 0.5`.
//! `DataTableLayout::h_scroll` therefore has to be exactly the value the
//! renderer subtracted when it painted, not merely "close" to it — any
//! rounding a backend applies at paint time must already be baked into
//! the `h_scroll` carried on the layout `hit_test` runs against, because
//! `hit_test` does no rounding of its own (#550 round 2).
//!
//! Pixel backends (GTK/macOS) paint at the exact fractional `h_scroll`,
//! so the layout's `h_scroll` — copied verbatim from `DataTable::h_scroll`
//! in [`DataTable::layout`] — already matches, and both their
//! `draw_data_table` and their `Backend::data_table_layout` are
//! self-consistent for free.
//!
//! The TUI backend is cell-granular: it paints at `h_scroll.round()`
//! (`tui::data_table`'s `h_off`), so it must overwrite the returned
//! layout's `h_scroll` with that same rounded value. It does that in one
//! place — [`tui::data_table_layout`](crate::tui::data_table_layout) —
//! which **both** `tui::data_table::draw_data_table` and
//! `Backend::data_table_layout` route through, so the layout-on-demand
//! hit-test path (no repaint) agrees with the paint path bit-for-bit.
//! Adding a third TUI entry point that calls [`DataTable::layout`]
//! directly would reopen #550; go through `tui::data_table_layout`
//! instead. The same applies to any caller that hand-builds a
//! `DataTableLayout` for a cell-granular surface: it must apply the
//! rounding itself.
//!
//! One residual, pre-existing and unrelated to `h_scroll`: the TUI paint
//! loop rounds each column's `rc.x` and the scroll offset independently
//! (`rc.x.round() - h_off`), while `hit_test` compares against the
//! continuous `rc.x`. For columns whose resolved boundaries aren't
//! integers (`Flex`/`Content` widths that don't divide evenly) the two
//! can disagree by up to one cell. That predates #550 and is not
//! addressed here.

use crate::types::{Decoration, Modifiers, StyledText, WidgetId};
use serde::{Deserialize, Serialize};

/// Column definition for a `DataTable`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub title: String,
    /// Sizing strategy for this column.
    #[serde(default)]
    pub width: ColumnWidth,
    /// Horizontal text alignment within the column.
    #[serde(default)]
    pub align: ColumnAlign,
}

/// Column width strategy.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ColumnWidth {
    /// Fixed width in surface-native units (cells for TUI, pixels for
    /// GTK). Not affected by flex distribution.
    Fixed(f32),
    /// Flex weight — columns share remaining space proportionally.
    /// `Flex(1.0)` and `Flex(2.0)` in the same table give a 1:2 split.
    Flex(f32),
    /// Size to content with optional min/max clamps. The measurer
    /// determines the natural width; the layout clamps to `[min, max]`.
    Content { min: f32, max: f32 },
}

impl Default for ColumnWidth {
    fn default() -> Self {
        ColumnWidth::Flex(1.0)
    }
}

/// Horizontal text alignment within a column cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ColumnAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// One row in a `DataTable`. `cells` must have the same length as the
/// table's `columns`. Missing cells are treated as empty; extra cells
/// are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataRow {
    pub cells: Vec<StyledText>,
    #[serde(default)]
    pub decoration: Decoration,
}

/// Sort direction indicator for column headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SortDirection {
    Ascending,
    Descending,
}

/// Declarative description of a `DataTable` widget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataTable {
    pub id: WidgetId,
    pub columns: Vec<Column>,
    pub rows: Vec<DataRow>,
    #[serde(default)]
    pub selected_idx: Option<usize>,
    #[serde(default)]
    pub scroll_offset: usize,
    /// Which column is sorted, and in which direction. `None` = no
    /// sort indicator shown.
    #[serde(default)]
    pub sort: Option<(usize, SortDirection)>,
    #[serde(default)]
    pub has_focus: bool,
    /// Show a vertical scrollbar when rows exceed the viewport.
    #[serde(default)]
    pub show_scrollbar: bool,
    /// Minimum total width for all columns. When the viewport is
    /// narrower, columns are laid out at this width and a horizontal
    /// scrollbar appears. `None` = columns squeeze to fit.
    #[serde(default)]
    pub min_total_width: Option<f32>,
    /// Horizontal scroll offset in surface-native units (pixels for
    /// GTK, cells for TUI). Only meaningful when `min_total_width`
    /// causes the content to be wider than the viewport.
    #[serde(default)]
    pub h_scroll: f32,
    /// Per-column width overrides from user drag. When set, an override
    /// replaces the column's `ColumnWidth` strategy with `Fixed(w)`.
    /// `None` entries mean the column uses its original strategy.
    /// Must be the same length as `columns` or empty.
    #[serde(default)]
    pub column_overrides: Vec<Option<f32>>,
    /// Optional pinned summary/totals row, laid out against the same
    /// resolved columns as the body. Rendered below the visible body
    /// rows regardless of `scroll_offset`; excluded from selection,
    /// sort, and row hit-testing. `None` (the default) renders
    /// byte-for-byte identical to a table with no footer.
    #[serde(default)]
    pub footer: Option<DataRow>,
}

/// Events a `DataTable` emits back to the app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DataTableEvent {
    /// User clicked a column header — app should toggle sort.
    HeaderClicked { col: usize },
    /// User activated a row (click or Enter).
    RowActivated { idx: usize },
    /// User selected a row (arrow key navigation).
    RowSelected { idx: usize },
    /// User scrolled the table.
    Scroll { delta: i32, modifiers: Modifiers },
    /// User dragged a column divider to resize. `col` is the column to
    /// the left of the divider. `width` is the new width in surface
    /// units. App should update `column_overrides[col]`.
    ColumnResized { col: usize, width: f32 },
}

// ── Layout ──────────────────────────────────────────────────────────────

/// Measure result for a single column — returned by the measurer
/// callback in [`DataTable::layout`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColumnMeasure {
    pub content_width: f32,
}

impl ColumnMeasure {
    pub fn new(content_width: f32) -> Self {
        Self { content_width }
    }
}

/// Resolved column position after layout.
///
/// `x` is in **content space** — measured from the left edge of the
/// first column, *not* from the left edge of the viewport. When
/// `h_scroll` is non-zero the two differ; see the module header.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedColumn {
    pub x: f32,
    pub width: f32,
}

/// Hit-test result for a `DataTable`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataTableHit {
    /// Click on a column header.
    Header { col: usize },
    /// Click on a column header divider — start a resize drag.
    /// The column index is the column to the LEFT of the divider.
    HeaderDivider { col: usize },
    /// Click on a body row.
    Row { idx: usize },
    /// Click on the pinned footer/summary row.
    Footer,
    /// Click on empty space below the last row.
    Empty,
}

/// Fully-resolved DataTable layout.
///
/// `#[non_exhaustive]`: per PRIMITIVE_RULES rule 8, this keeps future
/// field additions non-breaking regardless of what downstream ends up
/// doing with the struct. Today (#550) no downstream crate constructs or
/// pattern-matches this type directly — both `coord-tui` and `vimcode`
/// only ever receive it from `.layout()` — but there's no reason to
/// leave that door open for free.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct DataTableLayout {
    pub header_height: f32,
    pub row_height: f32,
    pub columns: Vec<ResolvedColumn>,
    /// Number of rows that fit in the viewport (excluding header).
    pub visible_rows: usize,
    pub viewport_width: f32,
    pub viewport_height: f32,
    /// Width reserved for the vertical scrollbar (0 when hidden).
    pub scrollbar_width: f32,
    /// Total content width after column layout. When this exceeds
    /// `viewport_width`, horizontal scrolling is active.
    pub content_width: f32,
    /// Height reserved for the horizontal scrollbar (0 when not
    /// scrolling horizontally).
    pub h_scrollbar_height: f32,
    /// Height reserved for the pinned footer (0 when `footer` is
    /// `None`). Always `row_height * 2.0` when present — one row for
    /// the divider rule, one for the summary content — so the divider
    /// never overwrites the last body row in cell-granular backends
    /// (TUI) and stays visually breathing-room'd in pixel backends.
    pub footer_height: f32,
    /// The horizontal scroll offset this layout was **painted** at — not
    /// necessarily a bit-for-bit copy of [`DataTable::h_scroll`], carried
    /// here so hit-testing can undo the same shift the renderer applied
    /// (#550).
    ///
    /// Backends paint column `i` at `columns[i].x - h_scroll`, so a
    /// pointer `x` in viewport space maps to `x + h_scroll` in the
    /// content space `columns` is expressed in. `0.0` (the overwhelmingly
    /// common case) makes every conversion an identity.
    ///
    /// For pixel backends this is exactly `DataTable::h_scroll`. For
    /// cell-granular backends (TUI) it is `DataTable::h_scroll.round()`
    /// — the same rounding the renderer applies before subtracting it
    /// from each column's `x` — because `hit_test`/`column_hit` add this
    /// field back with no rounding of their own; a mismatch here
    /// misroutes clicks whenever `DataTable::h_scroll`'s fractional part
    /// crosses 0.5 (#550 round 2). Both TUI entry points that produce a
    /// layout — `tui::data_table::draw_data_table` and
    /// `Backend::data_table_layout` — apply it via
    /// [`tui::data_table_layout`](crate::tui::data_table_layout).
    pub h_scroll: f32,
}

/// Grab zone half-width for column divider detection (surface units).
const DIVIDER_GRAB_PX: f32 = 3.0;

impl DataTableLayout {
    /// Convert a pointer `x` in **viewport space** into the **content
    /// space** [`ResolvedColumn::x`] is expressed in, undoing the same
    /// `- h_scroll` shift the renderer applies when painting (#550).
    ///
    /// Identity when `h_scroll == 0.0`.
    ///
    /// This performs no rounding of its own — it trusts `self.h_scroll`
    /// to already be exactly the value the renderer subtracted at paint
    /// time (see the module-level "`h_scroll` must already agree with
    /// what was painted" section). Pointer coordinates are raw pixel/cell
    /// positions, not cell-centres, so there is no `+ 0.5` to lean on:
    /// a cell-granular backend that fed this an un-rounded `h_scroll`
    /// while painting at a rounded offset would misroute clicks whenever
    /// `h_scroll`'s fractional part crosses 0.5 (#550 round 2).
    #[inline]
    fn content_x(&self, x: f32) -> f32 {
        x + self.h_scroll
    }

    /// Is viewport-space `x` inside the vertical scrollbar's track, on a
    /// table whose columns only reach there *because* of `h_scroll`?
    ///
    /// The strip is the rightmost `scrollbar_width` of the viewport —
    /// painted over by the scrollbar itself, so nothing under it is
    /// clickable column geometry. Adding `h_scroll` back would otherwise
    /// slide a real column beneath the track and let a track click sort
    /// the wrong header.
    ///
    /// Deliberately inert at `h_scroll == 0.0`. A `min_total_width`
    /// table's columns already extend past the strip's left edge at zero
    /// scroll, so an unconditional exclusion would change what such a
    /// table's *unscrolled* clicks resolve to — and acceptance bullet 3
    /// of #550 requires zero-scroll routing to stay bit-identical for
    /// the many callers that pin `h_scroll` at 0. Pre-existing
    /// strip-fall-through at zero scroll is the callers' to intercept
    /// (coord-tui already does, via `audit_scrollbar_hit`); this fix's
    /// job is only to avoid *introducing* a new one.
    #[inline]
    fn in_v_scrollbar_strip(&self, x: f32) -> bool {
        self.h_scroll != 0.0
            && self.scrollbar_width > 0.0
            && x >= (self.viewport_width - self.scrollbar_width).max(0.0)
    }

    /// Resolve a click to a header / divider / row / footer.
    ///
    /// `x` and `y` are **viewport-relative** (see the module header):
    /// `x` is measured from the table's left edge, *before* `h_scroll`
    /// is added back, so callers pass the raw pointer position exactly
    /// as they did before #550.
    pub fn hit_test(
        &self,
        x: f32,
        y: f32,
        scroll_offset: usize,
        total_rows: usize,
    ) -> DataTableHit {
        if x < 0.0 || y < 0.0 || x >= self.viewport_width || y >= self.viewport_height {
            return DataTableHit::Empty;
        }
        if y < self.header_height {
            // The vertical scrollbar owns its strip outright — never let
            // horizontally-scrolled column geometry leak underneath it.
            if self.in_v_scrollbar_strip(x) {
                return DataTableHit::Empty;
            }
            let cx = self.content_x(x);
            // Check dividers first (higher priority than header body).
            for (i, rc) in self.columns.iter().enumerate() {
                let right_edge = rc.x + rc.width;
                if (cx - right_edge).abs() <= DIVIDER_GRAB_PX && i + 1 < self.columns.len() {
                    return DataTableHit::HeaderDivider { col: i };
                }
            }
            // No clamping: a `cx` before the first column's left edge or
            // past the last column's right edge is genuinely *no* column,
            // not column 0 and not the last column.
            let col = self
                .columns
                .iter()
                .position(|c| cx >= c.x && cx < c.x + c.width);
            return match col {
                Some(col) => DataTableHit::Header { col },
                None => DataTableHit::Empty,
            };
        }
        // Row resolution is intentionally purely `y`-based and does NOT
        // consult `in_v_scrollbar_strip` — a v-scrollbar-track click here
        // already fell through to `Row { .. }` before #550 (row
        // resolution never looked at `x` at all), so this fix does not
        // regress it. It is, however, still uncovered by this layer: the
        // issue's acceptance bullet ("a track click must not fall
        // through to a header or a row") is only satisfied for rows by a
        // caller intercepting the strip itself before calling
        // `hit_test` — e.g. coord-tui's `audit_scrollbar_hit`. See
        // `scrollbar_strips_keep_priority_when_horizontally_scrolled`
        // (header case) and `v_scrollbar_strip_row_click_is_a_caller_concern`
        // (documents this row-branch gap) in the tests below.
        let body_bottom = self.header_height + self.visible_rows as f32 * self.row_height;
        if y < body_bottom {
            let row_in_viewport = ((y - self.header_height) / self.row_height).floor() as usize;
            let abs_idx = scroll_offset + row_in_viewport;
            return if abs_idx < total_rows {
                DataTableHit::Row { idx: abs_idx }
            } else {
                DataTableHit::Empty
            };
        }
        if self.footer_height > 0.0 && y < body_bottom + self.footer_height {
            return DataTableHit::Footer;
        }
        DataTableHit::Empty
    }

    /// Which column is painted under viewport-space `x`, if any.
    ///
    /// This is the cell-resolution counterpart to [`Self::hit_test`]
    /// (which reports rows, not cells) and takes `x` in the same
    /// **viewport space**: `h_scroll` is added back before the lookup,
    /// and the vertical scrollbar strip resolves to `None` (#550).
    pub fn column_hit(&self, x: f32) -> Option<usize> {
        if self.in_v_scrollbar_strip(x) {
            return None;
        }
        let cx = self.content_x(x);
        self.columns
            .iter()
            .position(|c| cx >= c.x && cx < c.x + c.width)
    }

    /// Compute the `column_overrides` for a divider drag (#521 defect 1,
    /// #1031).
    ///
    /// `col` is the column to the LEFT of the dragged divider, as
    /// returned by [`DataTableHit::HeaderDivider`]. `pointer_x` is the
    /// drag pointer's current position in **viewport space** — the same
    /// space [`Self::hit_test`] took to produce `col`, so a caller keeps
    /// forwarding the raw pointer `x` and this converts once, internally
    /// (#550). `min_width` is the floor both the dragged column and the
    /// last column are clamped to.
    ///
    /// **The rightmost column absorbs the slack (#1031).** Widening `col`
    /// shrinks the *last* column (`self.columns.len() - 1`), not `col +
    /// 1` — every column strictly between `col` and the last one keeps
    /// its currently-resolved width bit-for-bit (only its `x` origin
    /// shifts, which is unavoidable). This is a deliberate change from
    /// the #521 "pair" invariant (`col` and `col + 1` only): giving an
    /// early column more room no longer eats the column immediately
    /// after it, it eats the one the grid already treats as flexible —
    /// the rightmost. When `col + 1` *is* the last column this reduces to
    /// the old pair behaviour exactly (see below).
    ///
    /// **Once the last column bottoms out at `min_width`, the table is
    /// allowed to overflow** instead of refusing the drag: the dragged
    /// column keeps growing, total content width grows past the
    /// viewport, and [`DataTable::layout`] derives `content_width` from
    /// the resolved columns and flips `h_scrolling` on its own — nothing
    /// special is needed here. Dragging back the other way reclaims that
    /// overflow into the last column first and shrinks the total back
    /// down to fit, before the dragged column itself narrows.
    ///
    /// No cascade to the second-from-last column, or any other column
    /// beyond the last, is ever recruited. A cascade (shrink the neighbour
    /// to its floor, then start eating the one past it) was considered
    /// and rejected: it is lossy in one direction — dragging back does not
    /// return the columns it pushed into to their original widths, because
    /// which column absorbed what now depends on drag history, which is
    /// exactly the #521 defect ("moving the left column makes the problem
    /// disappear") in a new shape. The rule here is instead a pure
    /// function of the pointer position (`target`/`last_width` below),
    /// which makes it reversible by construction: drag out past the
    /// overflow point and back to the same pointer position and every
    /// column — including the last one and `h_scrolling` — returns to
    /// exactly its starting value, with no transfer history to unwind.
    ///
    /// A `Fixed`-declared last column still absorbs: `drag_divider`
    /// already pins every column as a numeric override regardless of its
    /// declared [`ColumnWidth`] (see the freeze loop below), so this
    /// falls out of the existing model rather than being a new special
    /// case. If a `Fixed` last column should instead skip straight to
    /// overflowing without absorbing anything, that is a follow-up, not a
    /// silent variation of this rule.
    ///
    /// "Rightmost" means the last column in the table (`self.columns.len()
    /// - 1`), not the last one currently visible — unchanged by
    /// `min_total_width` or an already-h-scrolling table.
    ///
    /// `overrides` is the `column_overrides` in effect *before* this
    /// call (typically the in-progress drag's current state, or the
    /// table's existing overrides at drag start). Any column that does
    /// not already have an override is frozen here at its *currently
    /// resolved* width before `col` and the last column are adjusted —
    /// this must happen unconditionally, not just for `Flex` columns,
    /// because leaving an unrelated `Flex` column unresolved would let
    /// pass 2's redistribution reshuffle it the moment the touched
    /// columns' weights are pulled out of `total_flex` (the exact "moving
    /// the left column makes the problem disappear" mechanism reported in
    /// #521: whichever columns are still unpinned divide up whatever
    /// space the pinned ones didn't claim, so the split among *them*
    /// changes even though the user never touched them). Freezing every
    /// column up front makes the result independent of drag history:
    /// whatever the table's current resolved widths are, that's what gets
    /// pinned, regardless of which dividers produced them.
    pub fn drag_divider(
        &self,
        overrides: &[Option<f32>],
        col: usize,
        pointer_x: f32,
        min_width: f32,
    ) -> Vec<Option<f32>> {
        let mut next: Vec<Option<f32>> = if overrides.len() == self.columns.len() {
            overrides.to_vec()
        } else {
            vec![None; self.columns.len()]
        };
        if col + 1 >= self.columns.len() {
            return next;
        }
        for (i, rc) in self.columns.iter().enumerate() {
            if next[i].is_none() {
                next[i] = Some(rc.width);
            }
        }
        let last = self.columns.len() - 1;
        let min_width = min_width.max(0.0);
        // Pure function of the pointer position: `target` is where `col`
        // wants to land (floored at `min_width`, otherwise unbounded —
        // that's what lets the table overflow instead of refusing the
        // drag), and `last_width` is whatever's left of the pair after
        // that, floored at `min_width` so the last column stops shrinking
        // rather than going negative. No accumulated transfer, so this is
        // exactly reversible by construction (see doc comment above).
        let pair = self.columns[col].width + self.columns[last].width;
        let col_x = self.columns[col].x;
        let target = (self.content_x(pointer_x) - col_x).max(min_width);
        let last_width = (pair - target).max(min_width);
        next[col] = Some(target);
        next[last] = Some(last_width);
        next
    }
}

impl DataTable {
    /// Compute layout from viewport dimensions and a column measurer.
    ///
    /// `row_height` is the backend's row height (1.0 for TUI, line_height
    /// for GTK). `header_height` is typically `row_height` or
    /// `row_height * 1.2`.
    ///
    /// The measurer receives each `Column` and returns a `ColumnMeasure`
    /// with the content width. Only used for `ColumnWidth::Content`
    /// columns; `Fixed` and `Flex` columns ignore the measure.
    pub fn layout<F>(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        row_height: f32,
        header_height: f32,
        scrollbar_width: f32,
        measure: F,
    ) -> DataTableLayout
    where
        F: Fn(&Column) -> ColumnMeasure,
    {
        let sb_w = if self.show_scrollbar {
            scrollbar_width
        } else {
            0.0
        };
        let visible_col_area = (viewport_width - sb_w).max(0.0);
        let layout_width = match self.min_total_width {
            Some(min) if min > visible_col_area => min,
            _ => visible_col_area,
        };
        let resolved = resolve_columns(
            &self.columns,
            layout_width,
            &measure,
            &self.column_overrides,
        );
        let content_width = resolved.last().map(|c| c.x + c.width).unwrap_or(0.0);
        let h_scrolling = content_width > visible_col_area + 0.5;
        let h_sb_h = if h_scrolling {
            if row_height > 1.5 {
                (row_height * 0.5).round()
            } else {
                row_height
            }
        } else {
            0.0
        };
        let footer_height = if self.footer.is_some() {
            row_height * 2.0
        } else {
            0.0
        };
        let body_height = (viewport_height - header_height - h_sb_h - footer_height).max(0.0);
        let visible_rows = if row_height > 0.0 {
            (body_height / row_height).floor() as usize
        } else {
            0
        };
        DataTableLayout {
            header_height,
            row_height,
            columns: resolved,
            visible_rows,
            viewport_width,
            viewport_height,
            scrollbar_width: sb_w,
            content_width,
            h_scrollbar_height: h_sb_h,
            footer_height,
            h_scroll: self.h_scroll,
        }
    }
}

/// Resolve column widths from definitions + viewport width.
/// Shared logic that TreeTable will also use.
fn resolve_columns<F>(
    columns: &[Column],
    viewport_width: f32,
    measure: &F,
    overrides: &[Option<f32>],
) -> Vec<ResolvedColumn>
where
    F: Fn(&Column) -> ColumnMeasure,
{
    if columns.is_empty() {
        return Vec::new();
    }

    let mut widths: Vec<f32> = Vec::with_capacity(columns.len());
    let mut remaining = viewport_width;
    let mut total_flex = 0.0_f32;

    // Pass 1: resolve Fixed and Content columns, accumulate flex weight.
    // Column overrides replace the original strategy with Fixed(w).
    //
    // Overrides are honored at face value — NOT clamped to `remaining`
    // (#1031). Fixed/Content columns below are still clamped: they come
    // from the table's own declared shape, which is expected to fit the
    // budget it was resolved against. An override, by contrast, is a
    // deliberate divider-drag request (`DataTableLayout::drag_divider`)
    // that may legitimately ask for more than `viewport_width` has to
    // give — that's exactly how #1031's "rightmost column absorbs the
    // slack, then the table overflows and h-scrolls" falls out: once
    // every column's override sum exceeds `viewport_width`, `remaining`
    // goes negative (harmless — it only gates pass 2's flex distribution
    // below) and the resolved `x + width` of the last column comes out
    // bigger than `viewport_width`, which `DataTable::layout` already
    // reads as `content_width` and compares against `visible_col_area` to
    // flip `h_scrolling`. Clamping here would silently truncate that
    // overflow away instead of letting it scroll.
    for (i, col) in columns.iter().enumerate() {
        if let Some(Some(ow)) = overrides.get(i) {
            let w = ow.max(0.0);
            widths.push(w);
            remaining -= w;
            continue;
        }
        match col.width {
            ColumnWidth::Fixed(w) => {
                let w = w.min(remaining).max(0.0);
                widths.push(w);
                remaining -= w;
            }
            ColumnWidth::Content { min, max } => {
                let m = measure(col);
                let w = m.content_width.clamp(min, max).min(remaining).max(0.0);
                widths.push(w);
                remaining -= w;
            }
            ColumnWidth::Flex(weight) => {
                widths.push(0.0); // placeholder
                total_flex += weight.max(0.0);
            }
        }
    }

    // Pass 2: distribute remaining space among Flex columns.
    //
    // Must skip any column with an active override (#516 defect 3): pass 1
    // already resolved that column's width from the override and folded
    // its contribution *out* of `total_flex` (the `continue` above skips
    // the `Flex` arm for overridden columns). But `col.width` here is
    // still the column's *original* declared strategy — overriding a
    // column never rewrites it, only layers a width on top — so a
    // dragged column whose original strategy is `Flex` matches this `if
    // let` too. Without this guard its pass-1 width gets clobbered by a
    // flex share computed from a `total_flex` that already excludes its
    // own weight, which can land smaller than its *original* pre-drag
    // width — i.e. the column visibly *shrinks* while being dragged
    // wider. This is the root cause of the "divider before the last
    // column resizes backward" symptom: it reproduces on the divider
    // before any column whose left-hand neighbour is Flex-declared, not
    // just the last one, but a trailing pair of Flex text columns (the
    // common server-data-driven layout) puts it right where the last
    // divider lives.
    if total_flex > 0.0 && remaining > 0.0 {
        for (i, col) in columns.iter().enumerate() {
            if matches!(overrides.get(i), Some(Some(_))) {
                continue;
            }
            if let ColumnWidth::Flex(weight) = col.width {
                widths[i] = (weight.max(0.0) / total_flex) * remaining;
            }
        }
    }

    // Pass 3: compute x positions.
    let mut x = 0.0_f32;
    let mut resolved: Vec<ResolvedColumn> = widths
        .iter()
        .map(|&w| {
            let rc = ResolvedColumn { x, width: w };
            x += w;
            rc
        })
        .collect();

    // Pass 4: fill any leftover space into the last column (#521 defect
    // 2). Pass 2 is gated on `total_flex > 0.0`: once every `Flex`
    // column has an active override, `total_flex` is `0` (pass 1's
    // `continue` for overridden columns never contributes to it), so
    // pass 2 is skipped entirely and whatever space `remaining` still
    // held goes unclaimed — the resolved widths sum to less than
    // `viewport_width` and the table visibly stops filling its area.
    // One-directional (only ever *grows* the last column to reach
    // `viewport_width`, never shrinks it): when columns legitimately
    // exceed the viewport (e.g. `min_total_width`, or an overridden
    // column whose requested width pass 1 above now honors uncapped,
    // #1031), `x + width` here is already `>= viewport_width` and this
    // is a no-op, so h-scroll is
    // untouched.
    if let Some(last) = resolved.last_mut() {
        let shortfall = viewport_width - (last.x + last.width);
        if shortfall > 0.0 {
            last.width += shortfall;
        }
    }

    resolved
}

// ── NativeSurface paint (issue #1084, NativeSurface Phase 4 7/8) ───────────
//
// `paint` below is shared by the **macOS and Windows** rasterisers only —
// `gtk::data_table::draw_data_table` is **not** migrated and stays a full,
// bespoke Cairo + Pango implementation. Same exception, same reason,
// already documented on
// [`crate::primitives::rich_text_popup::native_surface_paint`] and
// [`crate::primitives::message_list::native_surface_paint`]: every header/
// body/footer cell GTK paints goes through **one** Pango `show_layout`
// call per cell, with per-span colour expressed as a byte-ranged
// `AttrList` rather than "measure each span, advance x by its width" —
// exactly the shape issue #214 fixed `rich_text_popup` away from, because
// summed per-span widths can drift from one line's real shaped glyph
// positions for a proportional font. `NativeSurface` has no "shape one
// line with N attribute ranges" verb, only single-style runs
// (`surface_draw_text_run(_styled)`), so migrating GTK onto `paint` below
// would mean reintroducing that bug class for the sake of a mechanical
// "move the code." Per this issue's "do not tranche silently"
// instruction: this is that call, made explicitly, not a silent skip.
//
// macOS and Windows never had that problem — both already painted
// per-span with manual x-advance (the same shape `paint` below takes) —
// so consolidating *their* two copies carries no such risk, and closes
// real, pre-existing gaps between them (2-of-3 majority pattern, same
// convention `crate::primitives::palette`/`tooltip`/`dialog`/
// `rich_text_popup`'s `native_surface_paint` modules already establish):
//
// - **Selection / hover row tint alpha-blending.** GTK and Windows both
//   intend a translucent tint (`theme.selection_alpha` / a hardcoded
//   `0.5` hover mix) — Windows approximates it with a CPU-side
//   [`crate::types::Color::blend`] against an assumed `theme.background`
//   because its `fill_rect` only took an opaque colour before
//   `NativeSurface` existed. `macos::data_table`'s own module doc named
//   this outright: "macOS paints a solid `selection_bg` pixel today"
//   (no blending at all) — a real, documented scope omission. `paint`
//   uses [`NativeSurface::surface_fill_rect_alpha`] uniformly now, which
//   `CgSurface`'s default forwards to a real alpha-blended
//   `surface_fill_rect` (see that adapter's own doc — no CPU-side
//   approximation needed, unlike the pre-#1084 Windows shape), so both
//   backends now composite a true translucent tint over whatever is
//   already painted underneath, matching GTK's Cairo `set_source_rgba`
//   behaviour instead of assuming what the background was.
// - **Scrollbar treatment.** `gtk::data_table` already paints its
//   vertical/horizontal scrollbars through
//   [`crate::primitives::scrollbar::native_surface_paint::paint`] — a
//   translucent, hover/drag-aware track+thumb. macOS and Windows each
//   hand-rolled their own flat, always-opaque, never-hover-aware
//   track+thumb fills instead (Windows's own module doc calls this out:
//   "Scrollbar track/thumb paint as flat fills (no `win::draw_scrollbar`
//   dependency...)"). `paint` calls the same shared scrollbar paint GTK
//   already uses for both backends now, via a
//   [`crate::primitives::scrollbar::Scrollbar`] built with
//   `hovered`/`dragging` both left at their `false` default — identical
//   to what `gtk::data_table` itself passes, so this is a pure quality
//   upgrade (translucency, matching every other scrollbar-bearing
//   primitive in this crate) with no new interaction surface.
// - **Footer per-span colour.** GTK and macOS both painted each footer
//   span in its own `fg` (falling back to `theme.foreground`); Windows's
//   pre-#1084 footer painted the *entire* cell as one bold run in
//   `theme.foreground`, silently discarding every span's own `fg`. This
//   was not a documented scope omission — the footer path simply never
//   split by span the way the header/body paths didn't need to (no
//   per-span colour there either... except the footer's very own
//   `styled.spans` loop existed for exactly this purpose and Windows's
//   footer never used it). `paint` splits by span, honouring `span.fg`,
//   on both backends now.
// - **Header/footer separator + vertical alignment.** GTK and macOS
//   already draw the header separator at `sep_x - 0.5` (a half-pixel
//   inset so antialiasing centres the 1-unit line on the boundary);
//   Windows drew it at `sep_x` with no inset. `paint` uses `sep_x - 0.5`
//   uniformly (2-of-3 majority — GTK is uncounted here since its own
//   copy is untouched, but its choice agrees with macOS's). Vertical
//   alignment goes the other way: GTK and Windows both paint the
//   header/footer text flush with the band's top edge (`rect.y`/
//   `footer_y` directly); macOS alone vertically centred it within the
//   taller header/footer band. `paint` adopts the GTK/Windows top-flush
//   convention uniformly (2-of-3 majority again), which is also simpler:
//   one fewer height measurement needed per header/footer cell.
// - **Vertical scrollbar track height.** GTK and Windows both reserve
//   `rect.height - header_height - footer_height` for the vertical
//   track — the same height regardless of whether a horizontal
//   scrollbar is *also* showing. macOS alone additionally subtracted
//   `h_scrollbar_height`, which would otherwise leave the vertical
//   track short of the horizontal scrollbar's row on every other
//   backend. `paint` adopts the GTK/Windows convention (2-of-3 majority)
//   for both migrated backends, rather than introducing a third,
//   unreviewed geometry choice.
//
// Not changed: per-span **bold** in header/footer text is carried via
// [`NativeSurface::surface_draw_text_run_styled`] on both backends now —
// `D2dSurface` honours it for real (matching this module's pre-#1084
// behaviour exactly); `CgSurface` takes that verb's *default*, which
// drops style entirely — inert on macOS (no visual regression, no new
// bold either), the same posture `crate::primitives::status_bar` and
// `crate::primitives::rich_text_popup` already document for the
// identical default. Body-cell text was never bold on any backend before
// this migration and stays that way (no `StyledSpan::bold` read for body
// cells anywhere, matching all three pre-#1084 copies, `gtk::data_table`
// included).
#[cfg(any(feature = "win", all(feature = "macos", target_os = "macos")))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{ColumnAlign, DataTable, DataTableLayout, SortDirection};
    use crate::event::Rect;
    use crate::native_surface::NativeSurface;
    use crate::primitives::layout_metrics::{pixel_data_table_layout, TextMeasure};
    use crate::primitives::scrollbar::Scrollbar;
    use crate::theme::Theme;
    use crate::types::Decoration;

    /// Hover tint alpha for a non-selected hovered row — matches
    /// `gtk::data_table`'s hardcoded `0.5`.
    const HOVER_ALPHA: f32 = 0.5;

    /// Adapts a live `&dyn NativeSurface`'s plain (non-bold) text
    /// measurement into the [`TextMeasure`] `pixel_data_table_layout`
    /// needs for `ColumnWidth::Content` sizing — mirrors what
    /// `mac_data_table_layout`/`win_data_table_layout` already pass
    /// (`&CTFont` / `&DWrite`, both plain-metric measurers; neither
    /// backend's *layout* pass ever needed bold-aware widths, only the
    /// separate paint-time header/footer alignment measurement does).
    struct SurfaceTextMeasure<'a>(&'a dyn NativeSurface);

    impl TextMeasure for SurfaceTextMeasure<'_> {
        fn width_of(&self, text: &str) -> f32 {
            self.0.surface_measure_text(text).0
        }
    }

    fn align_text_x(col_x: f32, col_w: f32, text_w: f32, align: ColumnAlign) -> f32 {
        match align {
            ColumnAlign::Left => col_x,
            ColumnAlign::Center => col_x + (col_w - text_w) / 2.0,
            ColumnAlign::Right => col_x + col_w - text_w,
        }
    }

    /// Paint a [`DataTable`] into `rect` on `surface` and return the
    /// resolved [`DataTableLayout`] — same contract as every backend's
    /// pre-#1084 `draw_data_table`. See this module's doc for the full
    /// per-backend divergence survey this closes/preserves.
    ///
    /// A non-positive `rect.width`/`rect.height` short-circuits to the
    /// no-paint layout without touching `surface` at all, matching
    /// `macos::data_table`'s pre-#1084 guard (`win::data_table` had none
    /// to preserve — this is a superset, not a behaviour change, since a
    /// non-positive Direct2D fill/clip rect was already a no-op there).
    pub(crate) fn paint(
        table: &DataTable,
        surface: &mut dyn NativeSurface,
        theme: &Theme,
        rect: Rect,
        line_height: f32,
        hovered_idx: Option<usize>,
    ) -> DataTableLayout {
        let layout = {
            let measure = SurfaceTextMeasure(&*surface);
            pixel_data_table_layout(table, rect.width, rect.height, line_height, &measure)
        };

        if rect.width <= 0.0 || rect.height <= 0.0 {
            return layout;
        }

        let h_off = table.h_scroll;
        let header_height = layout.header_height;
        let footer_h = layout.footer_height;

        surface.surface_push_clip(rect);

        // ── Header ───────────────────────────────────────────────────────
        surface.surface_fill_rect(
            Rect::new(rect.x, rect.y, rect.width, header_height),
            theme.tab_bar_bg,
        );

        for (col_idx, rc) in layout.columns.iter().enumerate() {
            let Some(col) = table.columns.get(col_idx) else {
                break;
            };
            if rc.width <= 0.0 {
                continue;
            }
            let sort_suffix = match &table.sort {
                Some((si, dir)) if *si == col_idx => match dir {
                    SortDirection::Ascending => " \u{25B2}",
                    SortDirection::Descending => " \u{25BC}",
                },
                _ => "",
            };
            let title = format!("{}{}", col.title, sort_suffix);
            let col_x = rect.x + rc.x - h_off;
            let col_w = rc.width;

            surface.surface_push_clip(Rect::new(col_x, rect.y, col_w, header_height));
            let (tw, th) = surface.surface_measure_text_styled(&title, true);
            let text_x = align_text_x(col_x, col_w, tw, col.align);
            surface.surface_draw_text_run_styled(
                Rect::new(text_x, rect.y, tw.max(1.0), th.max(1.0)),
                &title,
                theme.foreground,
                true,
                false,
                false,
                1.0,
            );
            surface.surface_pop_clip();
        }

        for (col_idx, rc) in layout.columns.iter().enumerate() {
            if col_idx + 1 >= layout.columns.len() {
                break;
            }
            let sep_x = rect.x + rc.x + rc.width - h_off;
            surface.surface_fill_rect(
                Rect::new(sep_x - 0.5, rect.y, 1.0, header_height),
                theme.separator,
            );
        }

        // ── Body ─────────────────────────────────────────────────────────
        let body_y = rect.y + header_height;
        let visible = layout
            .visible_rows
            .min(table.rows.len().saturating_sub(table.scroll_offset));

        for row_idx in 0..visible {
            let abs_idx = table.scroll_offset + row_idx;
            let row = &table.rows[abs_idx];
            let row_y = body_y + row_idx as f32 * line_height;
            let is_selected = table.selected_idx == Some(abs_idx);
            let is_hovered = hovered_idx == Some(abs_idx) && !is_selected;

            if is_selected {
                surface.surface_fill_rect_alpha(
                    Rect::new(rect.x, row_y, rect.width, line_height),
                    theme.selection_bg,
                    theme.selection_alpha,
                );
            } else if is_hovered {
                surface.surface_fill_rect_alpha(
                    Rect::new(rect.x, row_y, rect.width, line_height),
                    theme.tab_bar_bg,
                    HOVER_ALPHA,
                );
            }

            let is_muted = row.decoration == Decoration::Muted;

            for (col_idx, rc) in layout.columns.iter().enumerate() {
                let Some(styled) = row.cells.get(col_idx).filter(|c| !c.spans.is_empty()) else {
                    continue;
                };
                if rc.width <= 0.0 {
                    continue;
                }
                let col_x = rect.x + rc.x - h_off;
                let col_w = rc.width;
                surface.surface_push_clip(Rect::new(col_x, row_y, col_w, line_height));

                let full_text: String = styled.spans.iter().map(|s| s.text.as_str()).collect();
                let (tw, th) = surface.surface_measure_text(&full_text);
                let align = table
                    .columns
                    .get(col_idx)
                    .map(|c| c.align)
                    .unwrap_or(ColumnAlign::Left);
                let text_x = align_text_x(col_x, col_w, tw, align);
                let text_y = row_y + (line_height - th) / 2.0;

                if is_muted {
                    surface.surface_draw_text_run(
                        Rect::new(text_x, text_y, tw.max(1.0), th.max(1.0)),
                        &full_text,
                        theme.muted_fg,
                    );
                } else {
                    let mut run_x = text_x;
                    for span in &styled.spans {
                        let (sw, sh) = surface.surface_measure_text(&span.text);
                        let span_fg = span.fg.unwrap_or(theme.foreground);
                        surface.surface_draw_text_run(
                            Rect::new(run_x, text_y, sw.max(1.0), sh.max(1.0)),
                            &span.text,
                            span_fg,
                        );
                        run_x += sw;
                    }
                }
                surface.surface_pop_clip();
            }

            for (col_idx, rc) in layout.columns.iter().enumerate() {
                if col_idx + 1 >= layout.columns.len() || rc.width <= 0.0 {
                    continue;
                }
                let sep_x = rect.x + rc.x + rc.width - h_off;
                surface.surface_fill_rect(
                    Rect::new(sep_x - 0.5, row_y, 1.0, line_height),
                    theme.separator,
                );
            }
        }

        // ── Scrollbars ───────────────────────────────────────────────────
        if table.show_scrollbar
            && table.rows.len() > layout.visible_rows
            && layout.scrollbar_width > 0.0
        {
            let sb_x = rect.x + rect.width - layout.scrollbar_width;
            let track = Rect::new(
                sb_x,
                rect.y + header_height,
                layout.scrollbar_width,
                (rect.height - header_height - footer_h).max(0.0),
            );
            let sb = Scrollbar::vertical(
                table.id.clone(),
                track,
                table.scroll_offset as f32,
                table.rows.len() as f32,
                layout.visible_rows as f32,
                line_height,
            );
            crate::primitives::scrollbar::native_surface_paint::paint(&sb, surface, theme);
        }
        if layout.h_scrollbar_height > 0.0 && layout.content_width > 0.0 {
            let hsb_y = rect.y + rect.height - footer_h - layout.h_scrollbar_height;
            let track_w = (rect.width - layout.scrollbar_width).max(1.0);
            let track = Rect::new(rect.x, hsb_y, track_w, layout.h_scrollbar_height);
            let sb = Scrollbar::horizontal(
                table.id.clone(),
                track,
                table.h_scroll,
                layout.content_width,
                track_w,
                line_height,
            );
            crate::primitives::scrollbar::native_surface_paint::paint(&sb, surface, theme);
        }

        // ── Footer ───────────────────────────────────────────────────────
        if let Some(footer) = &table.footer {
            if footer_h > 0.0 {
                let footer_band_top = rect.y + rect.height - footer_h;
                let footer_y = rect.y + rect.height - line_height;

                surface.surface_fill_rect(
                    Rect::new(rect.x, footer_band_top - 0.5, rect.width, 1.0),
                    theme.separator,
                );
                surface.surface_fill_rect(
                    Rect::new(rect.x, footer_band_top, rect.width, footer_h),
                    theme.tab_bar_bg,
                );

                for (col_idx, rc) in layout.columns.iter().enumerate() {
                    let Some(styled) = footer.cells.get(col_idx).filter(|c| !c.spans.is_empty())
                    else {
                        continue;
                    };
                    if rc.width <= 0.0 {
                        continue;
                    }
                    let col_x = rect.x + rc.x - h_off;
                    let col_w = rc.width;
                    surface.surface_push_clip(Rect::new(col_x, footer_y, col_w, line_height));

                    let full_text: String = styled.spans.iter().map(|s| s.text.as_str()).collect();
                    let (tw, _th) = surface.surface_measure_text_styled(&full_text, true);
                    let align = table
                        .columns
                        .get(col_idx)
                        .map(|c| c.align)
                        .unwrap_or(ColumnAlign::Left);
                    let text_x = align_text_x(col_x, col_w, tw, align);

                    let mut run_x = text_x;
                    for span in &styled.spans {
                        let (sw, sh) = surface.surface_measure_text_styled(&span.text, true);
                        let span_fg = span.fg.unwrap_or(theme.foreground);
                        surface.surface_draw_text_run_styled(
                            Rect::new(run_x, footer_y, sw.max(1.0), sh.max(1.0)),
                            &span.text,
                            span_fg,
                            true,
                            false,
                            false,
                            1.0,
                        );
                        run_x += sw;
                    }
                    surface.surface_pop_clip();
                }
            }
        }

        surface.surface_pop_clip();
        layout
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::primitives::data_table::{Column, ColumnWidth, DataRow};
        use crate::theme::Theme;
        use crate::types::{Color, StyledSpan, StyledText, WidgetId};
        use crate::Image;

        /// Records every surface verb this primitive's paint uses —
        /// mirrors `primitives::command_line`'s identical test double, so
        /// this test runs on any host without Core Graphics/Direct2D.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            fills_alpha: Vec<(Rect, Color, f32)>,
            text_runs: Vec<(Rect, String, Color)>,
            styled_runs: Vec<(Rect, String, Color, bool)>,
            clip_pushes: Vec<Rect>,
            clip_pops: usize,
        }

        impl NativeSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: crate::Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> crate::Viewport {
                crate::Viewport::new(300.0, 200.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                16.0
            }
            fn surface_char_width(&self) -> f32 {
                8.0
            }
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 8.0, 16.0)
            }
            fn surface_measure_text_styled(&self, text: &str, _bold: bool) -> (f32, f32) {
                self.surface_measure_text(text)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_fill_rounded_rect(&mut self, rect: Rect, _radius: f32, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_fill_rect_alpha(&mut self, rect: Rect, color: Color, alpha: f32) {
                self.fills_alpha.push((rect, color, alpha));
            }
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.text_runs.push((rect, text.to_string(), color));
            }
            #[allow(clippy::too_many_arguments)]
            fn surface_draw_text_run_styled(
                &mut self,
                rect: Rect,
                text: &str,
                color: Color,
                bold: bool,
                _italic: bool,
                _underline: bool,
                _scale_x: f32,
            ) {
                self.styled_runs.push((rect, text.to_string(), color, bold));
            }
            fn surface_draw_line(
                &mut self,
                _from: crate::Point,
                _to: crate::Point,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_push_clip(&mut self, rect: Rect) {
                self.clip_pushes.push(rect);
            }
            fn surface_pop_clip(&mut self) {
                self.clip_pops += 1;
            }
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        fn two_col_table() -> DataTable {
            DataTable {
                id: WidgetId::new("dt"),
                columns: vec![
                    Column {
                        title: "Name".into(),
                        width: ColumnWidth::Flex(2.0),
                        align: ColumnAlign::Left,
                    },
                    Column {
                        title: "Value".into(),
                        width: ColumnWidth::Flex(1.0),
                        align: ColumnAlign::Right,
                    },
                ],
                rows: vec![
                    DataRow {
                        cells: vec![StyledText::plain("alpha"), StyledText::plain("1")],
                        decoration: Decoration::Normal,
                    },
                    DataRow {
                        cells: vec![StyledText::plain("beta"), StyledText::plain("2")],
                        decoration: Decoration::Normal,
                    },
                ],
                selected_idx: None,
                scroll_offset: 0,
                sort: None,
                has_focus: false,
                show_scrollbar: false,
                min_total_width: None,
                h_scroll: 0.0,
                column_overrides: Vec::new(),
                footer: None,
            }
        }

        #[test]
        fn header_bg_then_body_paint_in_order() {
            let table = two_col_table();
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let rect = Rect::new(0.0, 0.0, 200.0, 100.0);
            paint(&table, &mut surface, &theme, rect, 16.0, None);

            assert_eq!(surface.fills[0].1, theme.tab_bar_bg);
            // Header text painted bold.
            assert!(surface.styled_runs.iter().any(|r| r.1 == "Name" && r.3));
            assert!(surface.styled_runs.iter().any(|r| r.1 == "Value" && r.3));
            // Body cell text via plain (non-bold) runs.
            assert!(surface.text_runs.iter().any(|r| r.1 == "alpha"));
            assert!(surface.text_runs.iter().any(|r| r.1 == "beta"));
        }

        /// #1084's RED-before-the-port case: `macos::data_table` used to
        /// paint the selected row's background as a fully opaque
        /// `selection_bg` fill (its own module doc named this a "Scope
        /// omission" — no alpha blending at all). The shared `paint` now
        /// always goes through `surface_fill_rect_alpha`, so every
        /// migrated backend composites a real translucent tint.
        #[test]
        fn selected_row_uses_alpha_blended_fill_not_opaque() {
            let mut table = two_col_table();
            table.selected_idx = Some(0);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let rect = Rect::new(0.0, 0.0, 200.0, 100.0);
            paint(&table, &mut surface, &theme, rect, 16.0, None);

            assert_eq!(surface.fills_alpha.len(), 1);
            assert_eq!(surface.fills_alpha[0].1, theme.selection_bg);
            assert_eq!(surface.fills_alpha[0].2, theme.selection_alpha);
            // The selection tint must never appear as an *opaque* fill —
            // only via `surface_fill_rect_alpha` above.
            assert!(
                surface.fills.iter().all(|(_, c)| *c != theme.selection_bg),
                "selection colour must only be painted through the alpha-blended fill"
            );
        }

        #[test]
        fn hovered_non_selected_row_uses_alpha_blended_tab_bar_bg() {
            let table = two_col_table();
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let rect = Rect::new(0.0, 0.0, 200.0, 100.0);
            paint(&table, &mut surface, &theme, rect, 16.0, Some(1));

            assert_eq!(surface.fills_alpha.len(), 1);
            assert_eq!(surface.fills_alpha[0].1, theme.tab_bar_bg);
            assert_eq!(surface.fills_alpha[0].2, HOVER_ALPHA);
        }

        #[test]
        fn selected_row_takes_priority_over_hover() {
            let mut table = two_col_table();
            table.selected_idx = Some(0);
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let rect = Rect::new(0.0, 0.0, 200.0, 100.0);
            // Row 0 is both selected and hovered — selection wins, no
            // double tint.
            paint(&table, &mut surface, &theme, rect, 16.0, Some(0));
            assert_eq!(surface.fills_alpha.len(), 1);
            assert_eq!(surface.fills_alpha[0].1, theme.selection_bg);
        }

        /// #1084's RED-before-the-port case: `win::data_table`'s footer
        /// painted the *entire* cell as one run in `theme.foreground`,
        /// discarding every span's own `fg`. The shared `paint` now
        /// splits by span on every migrated backend.
        #[test]
        fn footer_honours_per_span_fg_override() {
            let mut table = two_col_table();
            table.footer = Some(DataRow {
                cells: vec![
                    StyledText {
                        spans: vec![StyledSpan {
                            text: "Total".into(),
                            fg: Some(Color::rgb(255, 0, 0)),
                            bg: None,
                            bold: false,
                            italic: false,
                            underline: false,
                        }],
                    },
                    StyledText::plain("3"),
                ],
                decoration: Decoration::Normal,
            });
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let rect = Rect::new(0.0, 0.0, 200.0, 100.0);
            paint(&table, &mut surface, &theme, rect, 16.0, None);

            let total_run = surface
                .styled_runs
                .iter()
                .find(|r| r.1 == "Total")
                .expect("footer 'Total' span should paint as its own styled run");
            assert_eq!(total_run.2, Color::rgb(255, 0, 0));
            assert!(total_run.3, "footer text paints bold");
        }

        #[test]
        fn muted_row_ignores_per_span_fg_and_uses_muted_fg() {
            let mut table = two_col_table();
            table.rows[0] = DataRow {
                cells: vec![
                    StyledText {
                        spans: vec![StyledSpan {
                            text: "alpha".into(),
                            fg: Some(Color::rgb(255, 0, 0)),
                            bg: None,
                            bold: false,
                            italic: false,
                            underline: false,
                        }],
                    },
                    StyledText::plain("1"),
                ],
                decoration: Decoration::Muted,
            };
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let rect = Rect::new(0.0, 0.0, 200.0, 100.0);
            paint(&table, &mut surface, &theme, rect, 16.0, None);

            let alpha_run = surface
                .text_runs
                .iter()
                .find(|r| r.1 == "alpha")
                .expect("muted row should still paint its text");
            assert_eq!(alpha_run.2, theme.muted_fg);
        }

        #[test]
        fn zero_size_rect_paints_nothing() {
            let table = two_col_table();
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(
                &table,
                &mut surface,
                &theme,
                Rect::new(0.0, 0.0, 0.0, 100.0),
                16.0,
                None,
            );
            assert!(surface.fills.is_empty());
            assert!(surface.text_runs.is_empty());
            assert!(surface.styled_runs.is_empty());
            assert_eq!(surface.clip_pops, 0);
        }

        #[test]
        fn body_and_header_separators_share_the_half_pixel_inset() {
            let table = two_col_table();
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let rect = Rect::new(0.0, 0.0, 200.0, 100.0);
            let layout = paint(&table, &mut surface, &theme, rect, 16.0, None);

            let expected_sep_x = layout.columns[0].x + layout.columns[0].width - 0.5;
            let sep_fills: Vec<_> = surface
                .fills
                .iter()
                .filter(|(r, c)| *c == theme.separator && (r.x - expected_sep_x).abs() < 0.01)
                .collect();
            // One in the header band, one per visible body row (2 rows).
            assert_eq!(sep_fills.len(), 3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::StyledText;

    fn make_table(ncols: usize, nrows: usize) -> DataTable {
        let columns: Vec<Column> = (0..ncols)
            .map(|i| Column {
                title: format!("Col{i}"),
                width: ColumnWidth::Flex(1.0),
                align: ColumnAlign::Left,
            })
            .collect();
        let rows: Vec<DataRow> = (0..nrows)
            .map(|r| DataRow {
                cells: (0..ncols)
                    .map(|c| StyledText::plain(format!("r{r}c{c}")))
                    .collect(),
                decoration: Decoration::Normal,
            })
            .collect();
        DataTable {
            id: WidgetId::new("test"),
            columns,
            rows,
            selected_idx: None,
            scroll_offset: 0,
            sort: None,
            has_focus: false,
            show_scrollbar: false,
            min_total_width: None,
            h_scroll: 0.0,
            column_overrides: Vec::new(),
            footer: None,
        }
    }

    #[test]
    fn flex_columns_share_space_equally() {
        let table = make_table(4, 0);
        let layout = table.layout(80.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(10.0));
        assert_eq!(layout.columns.len(), 4);
        for rc in &layout.columns {
            assert!(
                (rc.width - 20.0).abs() < 0.01,
                "expected 20.0, got {}",
                rc.width
            );
        }
        assert!((layout.columns[0].x - 0.0).abs() < 0.01);
        assert!((layout.columns[1].x - 20.0).abs() < 0.01);
        assert!((layout.columns[2].x - 40.0).abs() < 0.01);
        assert!((layout.columns[3].x - 60.0).abs() < 0.01);
    }

    #[test]
    fn fixed_column_takes_exact_width() {
        let mut table = make_table(3, 0);
        table.columns[0].width = ColumnWidth::Fixed(10.0);
        let layout = table.layout(80.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));
        assert!((layout.columns[0].width - 10.0).abs() < 0.01);
        // Remaining 70 split between 2 flex columns
        assert!((layout.columns[1].width - 35.0).abs() < 0.01);
        assert!((layout.columns[2].width - 35.0).abs() < 0.01);
    }

    #[test]
    fn content_column_clamps_to_min_max() {
        let mut table = make_table(2, 0);
        table.columns[0].width = ColumnWidth::Content {
            min: 5.0,
            max: 15.0,
        };
        // Measure returns 3.0, which is below min → clamped to 5.0
        let layout = table.layout(80.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(3.0));
        assert!((layout.columns[0].width - 5.0).abs() < 0.01);

        // Measure returns 20.0, which is above max → clamped to 15.0
        let layout = table.layout(80.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(20.0));
        assert!((layout.columns[0].width - 15.0).abs() < 0.01);
    }

    #[test]
    fn visible_rows_computed_from_body_height() {
        let table = make_table(2, 100);
        let layout = table.layout(80.0, 25.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));
        // Body = 25 - 1 = 24 rows
        assert_eq!(layout.visible_rows, 24);
    }

    #[test]
    fn hit_test_header() {
        let table = make_table(3, 10);
        let layout = table.layout(90.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));
        // Columns are 30px each. Click in header row at x=45 → col 1
        assert_eq!(
            layout.hit_test(45.0, 0.5, 0, 10),
            DataTableHit::Header { col: 1 }
        );
    }

    #[test]
    fn hit_test_row() {
        let table = make_table(3, 10);
        let layout = table.layout(90.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));
        // Click in body at y=3.5 (row 2 after 1.0 header), scroll_offset=0 → row 2
        assert_eq!(
            layout.hit_test(10.0, 3.5, 0, 10),
            DataTableHit::Row { idx: 2 }
        );
        // With scroll_offset=5 → row 7
        assert_eq!(
            layout.hit_test(10.0, 3.5, 5, 10),
            DataTableHit::Row { idx: 7 }
        );
    }

    #[test]
    fn hit_test_empty_below_rows() {
        let table = make_table(2, 3);
        let layout = table.layout(80.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));
        // 3 rows + 1 header = 4 rows of content. Click at y=10 → empty
        assert_eq!(layout.hit_test(10.0, 10.0, 0, 3), DataTableHit::Empty);
    }

    #[test]
    fn hit_test_outside_viewport() {
        let table = make_table(2, 10);
        let layout = table.layout(80.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));
        assert_eq!(layout.hit_test(-1.0, 5.0, 0, 10), DataTableHit::Empty);
        assert_eq!(layout.hit_test(5.0, -1.0, 0, 10), DataTableHit::Empty);
        assert_eq!(layout.hit_test(80.0, 5.0, 0, 10), DataTableHit::Empty);
        assert_eq!(layout.hit_test(5.0, 20.0, 0, 10), DataTableHit::Empty);
    }

    #[test]
    fn weighted_flex_distributes_proportionally() {
        let mut table = make_table(3, 0);
        table.columns[0].width = ColumnWidth::Flex(1.0);
        table.columns[1].width = ColumnWidth::Flex(2.0);
        table.columns[2].width = ColumnWidth::Flex(1.0);
        let layout = table.layout(80.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));
        assert!((layout.columns[0].width - 20.0).abs() < 0.01);
        assert!((layout.columns[1].width - 40.0).abs() < 0.01);
        assert!((layout.columns[2].width - 20.0).abs() < 0.01);
    }

    // ── #516 defect 3: divider-before-last-column resize direction ──────

    /// A column override on a `Flex`-declared column must win outright —
    /// pass 2's flex redistribution must not re-derive (and clobber) a
    /// width pass 1 already resolved from the override. This is the
    /// direct regression test for the root cause: before the fix, pass 2
    /// matched on `col.width` (the column's original declared strategy)
    /// with no check for an active override, so an overridden `Flex`
    /// column's width was silently overwritten by a bogus share.
    #[test]
    fn override_on_flex_column_is_not_clobbered_by_flex_redistribution() {
        // Three equal-weight Flex columns, matching the pattern of a
        // trailing pair of text columns with one more before them —
        // dragging the divider before the last column overrides the
        // *second* column (index 1).
        let table = make_table(3, 0);
        let mut overrides = vec![None; 3];
        overrides[1] = Some(45.0_f32);
        let layout = table.layout(90.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));
        assert!(
            (layout.columns[1].width - 30.0).abs() < 0.01,
            "sanity: unoverridden layout gives each Flex(1.0) column an equal 30.0 share"
        );

        let mut table = table;
        table.column_overrides = overrides;
        let layout = table.layout(90.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));
        assert!(
            (layout.columns[1].width - 45.0).abs() < 0.01,
            "override should win outright, not get re-derived by flex redistribution: \
             expected 45.0, got {}",
            layout.columns[1].width
        );
    }

    /// The literal reported symptom: dragging the divider immediately
    /// before the last column must widen that column when the override
    /// grows and narrow it when the override shrinks — the same
    /// direction as every other divider, never inverted.
    #[test]
    fn divider_before_last_column_resizes_in_drag_direction() {
        let table = make_table(3, 0);
        let baseline = table.layout(90.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));
        let baseline_w = baseline.columns[1].width;

        let mut widen = table.clone();
        widen.column_overrides = vec![None, Some(baseline_w + 20.0), None];
        let widen_layout = widen.layout(90.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));

        let mut narrow = table.clone();
        narrow.column_overrides = vec![None, Some((baseline_w - 20.0).max(1.0)), None];
        let narrow_layout = narrow.layout(90.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));

        assert!(
            widen_layout.columns[1].width > baseline_w,
            "dragging the divider right (larger override) must widen the column: \
             baseline={baseline_w}, widened={}",
            widen_layout.columns[1].width
        );
        assert!(
            narrow_layout.columns[1].width < baseline_w,
            "dragging the divider left (smaller override) must narrow the column: \
             baseline={baseline_w}, narrowed={}",
            narrow_layout.columns[1].width
        );
    }

    // ── #521 defect 1 / #1031: a divider drag never displaces a column
    //    it doesn't border. Originally a strict pair-resize (`col` and
    //    `col + 1` only); #1031 changed *which* column is `col`'s
    //    dance partner to the last column in the table instead, with an
    //    overflow fallback once the last column bottoms out — the tests
    //    immediately below (through
    //    `drag_divider_stops_at_minimum_without_displacing_other_columns`)
    //    exercise the divider immediately before the last column, where
    //    the two models coincide (`col + 1 == last`) as long as the last
    //    column has room; #1031's own tests follow after those. ─────────

    /// Builds the same column shape the shipped sample app uses to
    /// reproduce #521: 3 `Flex` columns (weights 3.0, 1.5, 0.5) then one
    /// `Fixed(10.0)` — the divider dragged in these tests is the one
    /// between the 3rd and 4th columns (`col: 2`), matching "Age" |
    /// "Restarts" in the sample.
    fn make_sample_shaped_table() -> DataTable {
        let mut table = make_table(4, 0);
        table.columns[0].width = ColumnWidth::Flex(3.0);
        table.columns[1].width = ColumnWidth::Flex(1.5);
        table.columns[2].width = ColumnWidth::Flex(0.5);
        table.columns[3].width = ColumnWidth::Fixed(10.0);
        table
    }

    #[test]
    fn drag_divider_moves_only_the_two_columns_it_separates() {
        let table = make_sample_shaped_table();
        let baseline = table.layout(100.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));

        // Grab the divider between col 2 ("Age") and col 3 ("Restarts")
        // and drag it right by 5 units — comfortably within col 3's room
        // above its 4.0 floor (col 3 starts at 10.0), so this stays in
        // the col+1==last "behaves exactly as today" regime rather than
        // #1031's overflow case (covered separately below).
        let pointer_x = baseline.columns[2].x + baseline.columns[2].width + 5.0;
        let overrides = baseline.drag_divider(&[], 2, pointer_x, 4.0);

        let mut dragged = table.clone();
        dragged.column_overrides = overrides;
        let after = dragged.layout(100.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));

        // Columns 0 and 1 (Name, Status) are untouched by a divider that
        // doesn't border them: byte-identical x *and* width.
        assert_eq!(
            baseline.columns[0], after.columns[0],
            "column 0 must be untouched"
        );
        assert_eq!(
            baseline.columns[1], after.columns[1],
            "column 1 must be untouched"
        );

        // The dragged column's own left edge doesn't move — only the
        // grabbed boundary (its right edge) does.
        assert_eq!(
            baseline.columns[2].x, after.columns[2].x,
            "dragged column's left edge must not move"
        );
        assert!(
            after.columns[2].width > baseline.columns[2].width,
            "dragging the divider right must widen the column to its left"
        );

        // The pair's combined width is conserved — the drag redistributes
        // width between col 2 and col 3, it doesn't change the total.
        let baseline_pair = baseline.columns[2].width + baseline.columns[3].width;
        let after_pair = after.columns[2].width + after.columns[3].width;
        assert!(
            (baseline_pair - after_pair).abs() < 0.01,
            "pair's combined width must be conserved: before={baseline_pair}, after={after_pair}"
        );
    }

    #[test]
    fn drag_divider_result_is_independent_of_prior_drag_history() {
        let table = make_sample_shaped_table();
        let fresh = table.layout(100.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));

        // Scenario A: drag divider 2 (Age | Restarts) by +15 from a
        // never-touched table.
        let a_pointer = fresh.columns[2].x + fresh.columns[2].width + 15.0;
        let a_overrides = fresh.drag_divider(&[], 2, a_pointer, 4.0);
        let mut a_table = table.clone();
        a_table.column_overrides = a_overrides;
        let a_after = a_table.layout(100.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));
        let a_delta = a_after.columns[2].width - fresh.columns[2].width;

        // Scenario B: first drag divider 0 (Name | Status) by some
        // unrelated amount, *then* drag divider 2 by the same +15.
        let b_pointer0 = fresh.columns[0].x + fresh.columns[0].width - 8.0;
        let b_overrides0 = fresh.drag_divider(&[], 0, b_pointer0, 4.0);
        let mut b_table0 = table.clone();
        b_table0.column_overrides = b_overrides0;
        let b_mid = b_table0.layout(100.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));

        let b_pointer2 = b_mid.columns[2].x + b_mid.columns[2].width + 15.0;
        let b_overrides2 = b_mid.drag_divider(&b_table0.column_overrides, 2, b_pointer2, 4.0);
        let mut b_table2 = table.clone();
        b_table2.column_overrides = b_overrides2;
        let b_after = b_table2.layout(100.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));
        let b_delta = b_after.columns[2].width - b_mid.columns[2].width;

        assert!(
            (a_delta - b_delta).abs() < 0.01,
            "the same +15 divider-2 drag must produce the same width delta \
             regardless of whether divider 0 was dragged first: \
             a_delta={a_delta}, b_delta={b_delta}"
        );
    }

    /// #1031's version of the #521 "independent of drag history" property:
    /// since every divider now shares the *same* last-column dance
    /// partner, dragging divider X, then a different divider Y, then X
    /// again (re-settling X back to its own target after Y disturbed the
    /// shared last column) must land on exactly the same final layout as
    /// simply dragging Y then X once each, in that order — the redundant
    /// re-drag of X is a no-op past what a clean two-step sequence already
    /// gets you. Each drag targets a fixed *resolved width*, re-deriving
    /// its pointer_x from wherever that divider's current `x` happens to
    /// be (since dragging X shifts every column after it, including
    /// wherever Y currently sits) — exactly how a real screen-relative
    /// drag behaves.
    #[test]
    fn drag_divider_redundant_replay_matches_the_equivalent_clean_order() {
        let table = make_sample_shaped_table();
        let fresh = table.layout(100.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));

        // Comfortably inside col 3's 6.0 units of room even combined, so
        // neither drag below ever hits the floor and this stays a clean
        // linear-regime comparison.
        let target_x_width = fresh.columns[0].width + 1.0;
        let target_y_width = fresh.columns[1].width + 1.0;

        // drag_to: drag divider `col` (against the layout `on`'s current
        // state) until its resolved width becomes `target_width`.
        fn drag_to(
            on: &DataTableLayout,
            overrides: &[Option<f32>],
            col: usize,
            target_width: f32,
        ) -> Vec<Option<f32>> {
            let pointer_x = on.columns[col].x + target_width;
            on.drag_divider(overrides, col, pointer_x, 4.0)
        }

        // Sequence 1: X, then Y, then X again (the redundant replay).
        let mut seq1 = table.clone();
        let ov = drag_to(&fresh, &[], 0, target_x_width);
        seq1.column_overrides = ov;
        let after_x = seq1.layout(100.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));

        let ov = drag_to(&after_x, &seq1.column_overrides, 1, target_y_width);
        seq1.column_overrides = ov;
        let after_y = seq1.layout(100.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));

        let ov = drag_to(&after_y, &seq1.column_overrides, 0, target_x_width);
        seq1.column_overrides = ov;
        let seq1_final = seq1.layout(100.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));

        // Sequence 2: Y, then X — the equivalent clean order, no replay.
        let mut seq2 = table.clone();
        let ov = drag_to(&fresh, &[], 1, target_y_width);
        seq2.column_overrides = ov;
        let after_y2 = seq2.layout(100.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));

        let ov = drag_to(&after_y2, &seq2.column_overrides, 0, target_x_width);
        seq2.column_overrides = ov;
        let seq2_final = seq2.layout(100.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));

        for i in 0..seq1_final.columns.len() {
            assert!(
                (seq1_final.columns[i].width - seq2_final.columns[i].width).abs() < 0.01,
                "column {i} must match between [X, Y, X-again] and [Y, X]: \
                 seq1={}, seq2={}",
                seq1_final.columns[i].width,
                seq2_final.columns[i].width
            );
        }
    }

    #[test]
    fn drag_divider_stops_at_minimum_without_displacing_other_columns() {
        let table = make_sample_shaped_table();
        let baseline = table.layout(100.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));

        // Drag divider 2 far left — an enormous negative pointer offset —
        // trying to shrink col 2 to nothing and hand everything to col 3.
        let overrides = baseline.drag_divider(&[], 2, -1000.0, 4.0);
        let mut dragged = table.clone();
        dragged.column_overrides = overrides;
        let after = dragged.layout(100.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));

        assert!(
            (after.columns[2].width - 4.0).abs() < 0.01,
            "col 2 should stop at its 4.0 minimum, got {}",
            after.columns[2].width
        );
        // The freed space all goes to col 3 (the other half of the
        // pair) — never to unrelated columns.
        assert_eq!(baseline.columns[0], after.columns[0]);
        assert_eq!(baseline.columns[1], after.columns[1]);
        let pair_total = baseline.columns[2].width + baseline.columns[3].width;
        assert!((after.columns[3].width - (pair_total - 4.0)).abs() < 0.01);
    }

    // ── #1031: the rightmost column absorbs the slack, then the table
    //    overflows instead of refusing the drag ─────────────────────────

    #[test]
    fn drag_divider_widen_takes_slack_from_the_last_column_not_the_neighbour() {
        // Drag the *first* divider (col 0 | col 1) — its right-hand
        // neighbour (col 1) has plenty of room, so a pair-resize model
        // would have eaten it. #1031 says the give instead comes from
        // the *last* column (col 3), leaving col 1 and col 2 — every
        // column strictly between the dragged one and the last — bit-
        // for-bit unchanged in width (their `x` shifts, which is fine).
        let table = make_sample_shaped_table();
        let baseline = table.layout(100.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));

        // col 3 starts at 10.0 with a 4.0 floor: 6.0 units of room.
        // Widen by 4.0 — comfortably inside that room, no overflow.
        let pointer_x = baseline.columns[0].x + baseline.columns[0].width + 4.0;
        let overrides = baseline.drag_divider(&[], 0, pointer_x, 4.0);

        let mut dragged = table.clone();
        dragged.column_overrides = overrides;
        let after = dragged.layout(100.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));

        assert!(
            (after.columns[0].width - (baseline.columns[0].width + 4.0)).abs() < 0.01,
            "col 0 should widen by exactly 4.0, got {}",
            after.columns[0].width
        );
        assert!(
            (after.columns[3].width - (baseline.columns[3].width - 4.0)).abs() < 0.01,
            "col 3 (the last column) should absorb the 4.0, got {}",
            after.columns[3].width
        );
        assert!(
            (after.columns[1].width - baseline.columns[1].width).abs() < 0.01,
            "col 1 sits strictly between the dragged column and the last one — untouched"
        );
        assert!(
            (after.columns[2].width - baseline.columns[2].width).abs() < 0.01,
            "col 2 sits strictly between the dragged column and the last one — untouched"
        );
        let baseline_total: f32 = baseline.columns.iter().map(|c| c.width).sum();
        let after_total: f32 = after.columns.iter().map(|c| c.width).sum();
        assert!(
            (baseline_total - after_total).abs() < 0.01,
            "total content width must stay constant while the last column has room: \
             before={baseline_total}, after={after_total}"
        );
        assert!(
            after.h_scrollbar_height == 0.0 && after.content_width <= after.viewport_width + 0.5,
            "no overflow yet — the last column still had room"
        );
    }

    #[test]
    fn drag_divider_shrink_returns_slack_to_the_last_column_not_the_neighbour() {
        // The mirror of the widen case above: shrinking col 0 gives its
        // freed width straight to col 3, not col 1.
        let table = make_sample_shaped_table();
        let baseline = table.layout(100.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));

        let pointer_x = baseline.columns[0].x + baseline.columns[0].width - 4.0;
        let overrides = baseline.drag_divider(&[], 0, pointer_x, 4.0);

        let mut dragged = table.clone();
        dragged.column_overrides = overrides;
        let after = dragged.layout(100.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));

        assert!(
            (after.columns[0].width - (baseline.columns[0].width - 4.0)).abs() < 0.01,
            "col 0 should narrow by exactly 4.0, got {}",
            after.columns[0].width
        );
        assert!(
            (after.columns[3].width - (baseline.columns[3].width + 4.0)).abs() < 0.01,
            "col 3 (the last column) should grow by the freed 4.0, got {}",
            after.columns[3].width
        );
        assert!(
            (after.columns[1].width - baseline.columns[1].width).abs() < 0.01,
            "col 1 sits strictly between the dragged column and the last one — untouched"
        );
        assert!(
            (after.columns[2].width - baseline.columns[2].width).abs() < 0.01,
            "col 2 sits strictly between the dragged column and the last one — untouched"
        );
        let baseline_total: f32 = baseline.columns.iter().map(|c| c.width).sum();
        let after_total: f32 = after.columns.iter().map(|c| c.width).sum();
        assert!(
            (baseline_total - after_total).abs() < 0.01,
            "total content width must stay constant: before={baseline_total}, after={after_total}"
        );
    }

    #[test]
    fn drag_divider_overflows_once_the_last_column_bottoms_out() {
        // col 3 only has 6.0 units of room (10.0 down to the 4.0 floor).
        // Ask col 0 to widen by 20.0 — far past that — and the table must
        // overflow rather than refuse the drag: col 3 stops at its floor,
        // the excess (20.0 - 6.0 = 14.0) shows up as *extra* content
        // width, and `h_scrolling` flips on. The second-from-last column
        // (col 2) must not be recruited to make up any more of the
        // difference — only col 3 ever absorbs.
        let table = make_sample_shaped_table();
        let baseline = table.layout(100.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));

        let pointer_x = baseline.columns[0].x + baseline.columns[0].width + 20.0;
        let overrides = baseline.drag_divider(&[], 0, pointer_x, 4.0);

        let mut dragged = table.clone();
        dragged.column_overrides = overrides;
        let after = dragged.layout(100.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));

        assert!(
            (after.columns[0].width - (baseline.columns[0].width + 20.0)).abs() < 0.01,
            "col 0 gets everything it asked for — it's the table that overflows, not the drag \
             that's refused: got {}",
            after.columns[0].width
        );
        assert!(
            (after.columns[3].width - 4.0).abs() < 0.01,
            "col 3 (the last column) stops dead at its 4.0 floor, got {}",
            after.columns[3].width
        );
        assert!(
            (after.columns[1].width - baseline.columns[1].width).abs() < 0.01,
            "col 1 must still be untouched even while the table overflows"
        );
        assert!(
            (after.columns[2].width - baseline.columns[2].width).abs() < 0.01,
            "col 2 (second-from-last) must NOT be recruited to absorb the overflow — only the \
             last column ever does"
        );
        let baseline_total: f32 = baseline.columns.iter().map(|c| c.width).sum();
        let after_total: f32 = after.columns.iter().map(|c| c.width).sum();
        assert!(
            (after_total - (baseline_total + 14.0)).abs() < 0.01,
            "content width should grow by exactly the 14.0 excess: before={baseline_total}, \
             after={after_total}"
        );
        assert!(
            after.content_width > after.viewport_width,
            "content_width must exceed the viewport once overflowed: content_width={}, \
             viewport_width={}",
            after.content_width,
            after.viewport_width
        );
        assert!(
            after.h_scrollbar_height > 0.0,
            "the horizontal scrollbar must appear once the table overflows"
        );
    }

    #[test]
    fn drag_divider_overflow_is_reversible_back_to_the_original_pointer_position() {
        // Drag out past the point col 3 bottoms out, then drag back to
        // the *exact* pointer position the gesture started from: every
        // column — including col 3 and `h_scrolling` — must land back on
        // its original value, with no residue from having overflowed in
        // between.
        //
        // Both calls are made against `baseline` (the layout from
        // *before* this divider's drag began), not a layout re-derived
        // from the first call's own overflowed output — matching the
        // pattern `data_table_app.rs`'s `resize_base` uses for a real,
        // continuous mouse drag. `drag_divider`'s `pair` is read off
        // `self.columns[col]`/`self.columns[last]`, so re-deriving `self`
        // from a state that already reflects this divider's own overflow
        // would feed the *already-floored* last-column width back in as
        // if it were the true pre-drag pair, permanently losing how far
        // past the floor the drag actually went — see `drag_divider`'s
        // doc comment.
        let table = make_sample_shaped_table();
        let baseline = table.layout(100.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));
        let original_pointer_x = baseline.columns[0].x + baseline.columns[0].width;

        let overflow_pointer_x = original_pointer_x + 20.0;
        let out_overrides = baseline.drag_divider(&[], 0, overflow_pointer_x, 4.0);
        // Sanity: this step really did overflow.
        let mut out_table = table.clone();
        out_table.column_overrides = out_overrides.clone();
        let out_layout = out_table.layout(100.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));
        assert!(
            out_layout.h_scrollbar_height > 0.0,
            "test precondition: the outward drag must overflow"
        );

        let back_overrides = baseline.drag_divider(&out_overrides, 0, original_pointer_x, 4.0);
        let mut back_table = table.clone();
        back_table.column_overrides = back_overrides;
        let back_layout =
            back_table.layout(100.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));

        for i in 0..baseline.columns.len() {
            assert!(
                (back_layout.columns[i].width - baseline.columns[i].width).abs() < 0.01,
                "column {i} must return to its baseline width: baseline={}, after round-trip={}",
                baseline.columns[i].width,
                back_layout.columns[i].width
            );
        }
        assert_eq!(
            back_layout.h_scrollbar_height, 0.0,
            "h_scrolling must turn back off once the drag returns to its starting position"
        );
        assert!(
            (back_layout.content_width - baseline.content_width).abs() < 0.01,
            "content_width must return to its baseline value: baseline={}, after round-trip={}",
            baseline.content_width,
            back_layout.content_width
        );
    }

    // ── #521 defect 2: a fully-overridden table must still fill its
    //    viewport ──────────────────────────────────────────────────────

    #[test]
    fn overriding_every_flex_column_still_fills_the_viewport() {
        let mut table = make_sample_shaped_table();
        // Override all 3 Flex columns (0, 1, 2) — zeroing `total_flex`
        // and, before the fix, skipping pass 2 entirely and stranding
        // whatever space these overrides didn't claim.
        table.column_overrides = vec![Some(20.0), Some(15.0), Some(8.0), None];
        let layout = table.layout(100.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));

        let total_width: f32 = layout.columns.iter().map(|c| c.width).sum();
        assert!(
            (total_width - 100.0).abs() < 0.01,
            "resolved widths must still sum to the viewport width, got {total_width}"
        );
        let last = layout.columns.last().unwrap();
        assert!(
            (last.x + last.width - 100.0).abs() < 0.01,
            "the rightmost column's right edge must be flush with the viewport's right edge"
        );
    }

    #[test]
    fn fill_never_shrinks_content_that_legitimately_overflows_the_viewport() {
        // All columns Fixed and their sum (150) exceeds the visible
        // area (40) — the `min_total_width` h-scroll case (see
        // `DataTable::layout`: when `min_total_width` exceeds the
        // visible column area, columns are laid out at
        // `min_total_width` and a horizontal scrollbar appears, rather
        // than being squeezed to fit). The fill must be one-directional:
        // it only ever grows the *last* column to reach the width it's
        // laid out against, never shrinks it back down to the smaller
        // visible area.
        let mut table = make_table(3, 0);
        table.columns[0].width = ColumnWidth::Fixed(50.0);
        table.columns[1].width = ColumnWidth::Fixed(50.0);
        table.columns[2].width = ColumnWidth::Fixed(50.0);
        table.min_total_width = Some(150.0);
        let layout = table.layout(40.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));
        assert!(
            (layout.columns[2].width - 50.0).abs() < 0.01,
            "fixed columns legitimately exceeding the viewport must not be squeezed by the \
             fill: got {}",
            layout.columns[2].width
        );
        assert!(
            layout.content_width > layout.viewport_width,
            "content legitimately exceeding the viewport must still be reported as overflowing \
             (h-scroll), not squeezed to fit: content_width={}, viewport_width={}",
            layout.content_width,
            layout.viewport_width
        );
    }

    #[test]
    fn empty_table_layout_is_valid() {
        let table = make_table(0, 0);
        let layout = table.layout(80.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));
        assert!(layout.columns.is_empty());
        assert_eq!(layout.visible_rows, 19);
    }

    #[test]
    fn serde_round_trip() {
        let table = make_table(2, 3);
        let json = serde_json::to_string(&table).unwrap();
        let back: DataTable = serde_json::from_str(&json).unwrap();
        assert_eq!(table, back);
    }

    fn footer_row(ncols: usize) -> DataRow {
        DataRow {
            cells: (0..ncols)
                .map(|c| StyledText::plain(format!("total{c}")))
                .collect(),
            decoration: Decoration::Normal,
        }
    }

    #[test]
    fn none_footer_is_byte_identical_to_pre_change_layout() {
        // Regression guard (#432 req 5): a table with `footer: None`
        // must lay out exactly as it did before the footer existed.
        let table = make_table(2, 100);
        let layout = table.layout(80.0, 25.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));
        assert_eq!(layout.footer_height, 0.0);
        assert_eq!(layout.visible_rows, 24);
    }

    #[test]
    fn footer_reserves_height_and_shrinks_visible_rows() {
        let mut table = make_table(2, 100);
        table.footer = Some(footer_row(2));
        let layout = table.layout(80.0, 25.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));
        // Same viewport as `visible_rows_computed_from_body_height`
        // (24 rows with no footer) — the footer eats two rows (a
        // divider row + the content row).
        assert_eq!(layout.footer_height, 2.0);
        assert_eq!(layout.visible_rows, 22);
    }

    #[test]
    fn footer_columns_align_with_body_columns() {
        // Column-aligned totals (#432 req 1): the footer is laid out
        // against the *same* resolved columns as the body, so a right
        // -aligned numeric column's total lands directly under it.
        let mut table = make_table(3, 10);
        table.footer = Some(footer_row(3));
        let layout = table.layout(90.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));
        // `make_table` uses Flex(1.0) for every column — 30px each,
        // identical resolved bounds regardless of body vs. footer.
        assert_eq!(layout.columns.len(), 3);
        assert!((layout.columns[0].x - 0.0).abs() < 0.01);
        assert!((layout.columns[1].x - 30.0).abs() < 0.01);
        assert!((layout.columns[2].x - 60.0).abs() < 0.01);
    }

    #[test]
    fn hit_test_footer_is_pinned_regardless_of_scroll_offset() {
        let mut table = make_table(2, 100);
        table.footer = Some(footer_row(2));
        let layout = table.layout(80.0, 25.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));
        // Footer band: header(1) + visible_rows(22) .. +footer_height(2)
        // == y in [23, 25). Same regardless of `scroll_offset`.
        for scroll_offset in [0, 5, 50, 76] {
            assert_eq!(
                layout.hit_test(10.0, 24.0, scroll_offset, 100),
                DataTableHit::Footer,
                "footer hit should be stable at scroll_offset={scroll_offset}"
            );
        }
    }

    #[test]
    fn hit_test_footer_is_not_a_row() {
        // Selection/hit-testing must ignore the footer (#432 req 2/
        // acceptance bullet 4): a click in the footer band is never a
        // `Row` hit, even though `total_rows` exceeds what's visible.
        let mut table = make_table(2, 3);
        table.footer = Some(footer_row(2));
        let layout = table.layout(80.0, 20.0, 1.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));
        // Body has only 3 rows; visible_rows is far larger, so the
        // footer sits right after the header + visible-row band.
        let body_bottom = layout.header_height + layout.visible_rows as f32 * layout.row_height;
        let hit = layout.hit_test(10.0, body_bottom + 0.5, 0, table.rows.len());
        assert_eq!(hit, DataTableHit::Footer);
    }

    #[test]
    fn hit_test_same_band_is_empty_without_footer() {
        // Contrast case for `hit_test_footer_is_not_a_row`: with no
        // footer, the sliver between the last full visible row and the
        // viewport edge (a real gap here — row_height=3 doesn't evenly
        // divide the 19-unit body) is just empty space, not `Footer`.
        let table = make_table(2, 3);
        let layout = table.layout(80.0, 20.0, 3.0, 1.0, 0.0, |_| ColumnMeasure::new(0.0));
        let body_bottom = layout.header_height + layout.visible_rows as f32 * layout.row_height;
        assert!(
            body_bottom + 0.5 < layout.viewport_height,
            "test setup should leave a real gap below the last visible row"
        );
        let hit = layout.hit_test(10.0, body_bottom + 0.5, 0, table.rows.len());
        assert_eq!(hit, DataTableHit::Empty);
    }

    #[test]
    fn footer_serde_round_trip() {
        let mut table = make_table(2, 3);
        table.footer = Some(footer_row(2));
        let json = serde_json::to_string(&table).unwrap();
        let back: DataTable = serde_json::from_str(&json).unwrap();
        assert_eq!(table, back);
    }

    // ── #550: `hit_test` must undo the renderer's `h_scroll` shift ──────
    //
    // Geometry shared by the tests below: 4 × `Fixed(30.0)` columns laid
    // out at `min_total_width = 120` inside a 60-wide viewport, so
    // content space is exactly twice the viewport and every column
    // boundary lands on a round number.
    //
    //   content x:  0───30───60───90───120
    //   column:      c0 │ c1 │ c2 │ c3
    //
    // A backend paints column `i` at `columns[i].x - h_scroll`, so at
    // `h_scroll = 45` the operator sees c1's right half, all of c2, and
    // c3's left half — and c0 not at all.
    fn make_wide_table(nrows: usize) -> DataTable {
        let mut table = make_table(4, nrows);
        for c in &mut table.columns {
            c.width = ColumnWidth::Fixed(30.0);
        }
        table.min_total_width = Some(120.0);
        table
    }

    fn wide_layout(h_scroll: f32, nrows: usize) -> DataTableLayout {
        let mut table = make_wide_table(nrows);
        table.h_scroll = h_scroll;
        table.layout(60.0, 20.0, 1.0, 1.0, 1.0, |_| ColumnMeasure::new(0.0))
    }

    /// The pre-#550 algorithm, verbatim, as the oracle for the
    /// "`h_scroll == 0.0` is bit-identical" acceptance bullet.
    fn legacy_hit_test(
        l: &DataTableLayout,
        x: f32,
        y: f32,
        scroll_offset: usize,
        total_rows: usize,
    ) -> DataTableHit {
        if x < 0.0 || y < 0.0 || x >= l.viewport_width || y >= l.viewport_height {
            return DataTableHit::Empty;
        }
        if y < l.header_height {
            for (i, rc) in l.columns.iter().enumerate() {
                let right_edge = rc.x + rc.width;
                if (x - right_edge).abs() <= DIVIDER_GRAB_PX && i + 1 < l.columns.len() {
                    return DataTableHit::HeaderDivider { col: i };
                }
            }
            return match l.columns.iter().position(|c| x >= c.x && x < c.x + c.width) {
                Some(col) => DataTableHit::Header { col },
                None => DataTableHit::Empty,
            };
        }
        let body_bottom = l.header_height + l.visible_rows as f32 * l.row_height;
        if y < body_bottom {
            let row_in_viewport = ((y - l.header_height) / l.row_height).floor() as usize;
            let abs_idx = scroll_offset + row_in_viewport;
            return if abs_idx < total_rows {
                DataTableHit::Row { idx: abs_idx }
            } else {
                DataTableHit::Empty
            };
        }
        if l.footer_height > 0.0 && y < body_bottom + l.footer_height {
            return DataTableHit::Footer;
        }
        DataTableHit::Empty
    }

    #[test]
    fn layout_carries_h_scroll_through_to_the_layout() {
        assert_eq!(wide_layout(0.0, 10).h_scroll, 0.0);
        assert_eq!(wide_layout(45.0, 10).h_scroll, 45.0);
    }

    #[test]
    fn hit_test_header_resolves_to_the_painted_column_at_every_h_scroll() {
        // For each offset, walk every viewport cell centre and check the
        // hit against the column the renderer paints there — derived
        // from the same `rc.x - h_scroll` the rasterisers use, not from
        // a hardcoded table.
        for h_scroll in [0.0_f32, 10.0, 30.0, 45.0, 62.0] {
            let layout = wide_layout(h_scroll, 10);
            for cell in 0..60u32 {
                let x = cell as f32 + 0.5;
                let content_x = x + h_scroll;
                // Skip the divider grab zones — they take priority and
                // are covered by their own test below.
                let near_divider = layout.columns[..layout.columns.len() - 1]
                    .iter()
                    .any(|rc| (content_x - (rc.x + rc.width)).abs() <= DIVIDER_GRAB_PX);
                if near_divider {
                    continue;
                }
                let painted = layout
                    .columns
                    .iter()
                    .position(|rc| content_x >= rc.x && content_x < rc.x + rc.width);
                let expected = match painted {
                    Some(col) => DataTableHit::Header { col },
                    None => DataTableHit::Empty,
                };
                assert_eq!(
                    layout.hit_test(x, 0.5, 0, 10),
                    expected,
                    "h_scroll={h_scroll}, viewport x={x} (content x={content_x})"
                );
            }
        }
    }

    #[test]
    fn hit_test_header_at_scroll_that_pushes_the_first_column_off_screen() {
        // `h_scroll = 45` puts content x 45 at viewport x 0 — c0 (content
        // 0..30) is entirely off-screen to the left, so *nothing* in the
        // viewport may resolve to column 0 any more.
        let layout = wide_layout(45.0, 10);
        assert_eq!(
            layout.hit_test(5.0, 0.5, 0, 10),
            DataTableHit::Header { col: 1 },
            "viewport x=5 sits over c1's painted right half"
        );
        assert_eq!(
            layout.hit_test(25.0, 0.5, 0, 10),
            DataTableHit::Header { col: 2 }
        );
        assert_eq!(
            layout.hit_test(50.0, 0.5, 0, 10),
            DataTableHit::Header { col: 3 }
        );
        for cell in 0..60u32 {
            assert_ne!(
                layout.hit_test(cell as f32 + 0.5, 0.5, 0, 10),
                DataTableHit::Header { col: 0 },
                "column 0 is scrolled fully off-screen; no viewport x may resolve to it \
                 (viewport x={cell})"
            );
        }
    }

    #[test]
    fn hit_test_header_past_the_last_column_is_no_column() {
        // Over-scrolled to the very end: content x 90..120 fills the left
        // half of the viewport, and the right half is past the last
        // column's right edge — no column, not a clamp to the last one.
        let layout = wide_layout(90.0, 10);
        assert_eq!(
            layout.hit_test(10.0, 0.5, 0, 10),
            DataTableHit::Header { col: 3 }
        );
        assert_eq!(
            layout.hit_test(45.0, 0.5, 0, 10),
            DataTableHit::Empty,
            "content x=135 is past the 120-wide content — no column lives there"
        );
    }

    #[test]
    fn hit_test_header_divider_follows_h_scroll() {
        // Divider between c1 and c2 sits at content x 60 → viewport 15
        // when h_scroll is 45; the one between c2 and c3 (content 90)
        // lands at viewport 45.
        let layout = wide_layout(45.0, 10);
        assert_eq!(
            layout.hit_test(15.0, 0.5, 0, 10),
            DataTableHit::HeaderDivider { col: 1 }
        );
        assert_eq!(
            layout.hit_test(45.0, 0.5, 0, 10),
            DataTableHit::HeaderDivider { col: 2 }
        );
        // The *unscrolled* positions of those dividers must no longer
        // grab: viewport 60 is off-viewport, and viewport 30 is now the
        // middle of c2.
        assert_eq!(
            layout.hit_test(30.0, 0.5, 0, 10),
            DataTableHit::Header { col: 2 }
        );
    }

    #[test]
    fn column_hit_follows_h_scroll() {
        let unscrolled = wide_layout(0.0, 10);
        assert_eq!(unscrolled.column_hit(5.0), Some(0));
        assert_eq!(unscrolled.column_hit(35.0), Some(1));

        let scrolled = wide_layout(45.0, 10);
        assert_eq!(scrolled.column_hit(5.0), Some(1));
        assert_eq!(scrolled.column_hit(25.0), Some(2));
        assert_eq!(scrolled.column_hit(50.0), Some(3));
    }

    #[test]
    fn hit_test_row_index_is_unaffected_by_h_scroll() {
        // Vertical routing is orthogonal — the same body click resolves
        // to the same absolute row at every horizontal offset.
        for h_scroll in [0.0_f32, 30.0, 45.0, 90.0] {
            let layout = wide_layout(h_scroll, 40);
            assert_eq!(
                layout.hit_test(10.0, 3.5, 5, 40),
                DataTableHit::Row { idx: 7 },
                "h_scroll={h_scroll} must not shift row resolution"
            );
        }
    }

    #[test]
    fn scrollbar_strips_keep_priority_when_horizontally_scrolled() {
        let mut table = make_wide_table(200);
        table.show_scrollbar = true;
        table.h_scroll = 45.0;
        let layout = table.layout(60.0, 20.0, 1.0, 1.0, 1.0, |_| ColumnMeasure::new(0.0));
        assert!(layout.scrollbar_width > 0.0);
        assert!(layout.h_scrollbar_height > 0.0);

        // Vertical track: the rightmost `scrollbar_width` columns. Under
        // h_scroll a naive offset would land this on a real column and
        // sort it — the track must stay inert instead.
        let sb_x = layout.viewport_width - layout.scrollbar_width;
        assert_eq!(
            layout.hit_test(sb_x + 0.5, 0.5, 0, 200),
            DataTableHit::Empty,
            "a vertical-scrollbar track click must not fall through to a header"
        );
        assert_eq!(layout.column_hit(sb_x + 0.5), None);

        // Horizontal track: the band below the last body row. It is not
        // a header and not a row.
        let body_bottom = layout.header_height + layout.visible_rows as f32 * layout.row_height;
        let hit = layout.hit_test(10.0, body_bottom + 0.5, 0, 200);
        assert!(
            !matches!(hit, DataTableHit::Row { .. } | DataTableHit::Header { .. }),
            "a horizontal-scrollbar track click must not fall through to a header or a row, \
             got {hit:?}"
        );
    }

    #[test]
    fn v_scrollbar_strip_row_click_is_a_caller_concern() {
        // Documents the non-blocking #550-review gap: unlike the header
        // branch, `hit_test`'s row branch never consults `x` at all (row
        // resolution is purely `y`-based, both before and after #550),
        // so a click over the vertical-scrollbar track on a body row
        // falls through to `Row { .. }` here rather than `Empty`. This
        // is pre-existing behaviour, not a #550 regression — asserted
        // against explicitly so a future change can't silently start
        // relying on `hit_test` filtering this out. Callers that own a
        // vertical scrollbar (e.g. coord-tui's `audit_scrollbar_hit`)
        // are expected to intercept the strip themselves before forwarding
        // to `hit_test`.
        let mut table = make_wide_table(200);
        table.show_scrollbar = true;
        table.h_scroll = 45.0;
        let layout = table.layout(60.0, 20.0, 1.0, 1.0, 1.0, |_| ColumnMeasure::new(0.0));
        assert!(layout.scrollbar_width > 0.0);

        let sb_x = layout.viewport_width - layout.scrollbar_width;
        let y_in_body = layout.header_height + 0.5;
        assert_eq!(
            layout.hit_test(sb_x + 0.5, y_in_body, 0, 200),
            DataTableHit::Row { idx: 0 },
            "row branch does not filter the v-scrollbar strip — intentional, see comment on \
             the row branch in `hit_test`"
        );
    }

    #[test]
    fn drag_divider_reads_pointer_x_in_viewport_space() {
        // A divider drag started from a `HeaderDivider` hit keeps passing
        // the raw viewport pointer x, so the resolved widths must come
        // out the same whether or not the table is scrolled.
        let unscrolled = wide_layout(0.0, 10);
        let baseline = unscrolled.drag_divider(&[], 1, 70.0, 4.0);

        let scrolled = wide_layout(45.0, 10);
        // Same content-space pointer (70), expressed in viewport space.
        let dragged = scrolled.drag_divider(&[], 1, 70.0 - 45.0, 4.0);
        assert_eq!(
            baseline, dragged,
            "the same physical divider position must resize identically at any h_scroll"
        );
        assert_eq!(dragged[1], Some(40.0), "c1 grows from 30 to 70 - 30 = 40");
        assert_eq!(
            dragged[2],
            Some(30.0),
            "c2 sits strictly between the dragged column and the last column, so #1031's \
             last-absorbs rule leaves it untouched (col 1's divider does not border it)"
        );
        assert_eq!(
            dragged[3],
            Some(20.0),
            "c3 (the last column) absorbs the slack instead of c2: 30 - (40 - 30) = 20"
        );
    }

    #[test]
    fn h_scroll_zero_hit_testing_is_bit_identical_to_the_pre_change_algorithm() {
        // Acceptance bullet 3. Swept exhaustively over half-cell
        // positions across the whole viewport for four table shapes —
        // with and without a vertical scrollbar, with and without a
        // footer, narrow and `min_total_width`-wide.
        let mut shapes: Vec<DataTable> = Vec::new();
        shapes.push(make_table(4, 40));
        let mut with_sb = make_table(4, 40);
        with_sb.show_scrollbar = true;
        shapes.push(with_sb);
        let mut with_footer = make_table(3, 40);
        with_footer.show_scrollbar = true;
        with_footer.footer = Some(footer_row(3));
        shapes.push(with_footer);
        let mut wide = make_wide_table(40);
        wide.show_scrollbar = true;
        shapes.push(wide);

        for (i, table) in shapes.iter().enumerate() {
            assert_eq!(table.h_scroll, 0.0, "shape {i} must pin h_scroll at zero");
            let layout = table.layout(60.0, 20.0, 1.0, 1.0, 1.0, |_| ColumnMeasure::new(0.0));
            for cell_x in 0..62u32 {
                for cell_y in 0..22u32 {
                    let x = cell_x as f32 + 0.5;
                    let y = cell_y as f32 + 0.5;
                    for scroll_offset in [0usize, 7] {
                        assert_eq!(
                            layout.hit_test(x, y, scroll_offset, 40),
                            legacy_hit_test(&layout, x, y, scroll_offset, 40),
                            "shape {i}: hit_test({x}, {y}, {scroll_offset}, 40) must match the \
                             pre-#550 algorithm exactly"
                        );
                    }
                    assert_eq!(
                        layout.column_hit(x),
                        layout
                            .columns
                            .iter()
                            .position(|c| x >= c.x && x < c.x + c.width),
                        "shape {i}: column_hit({x}) must match the pre-#550 algorithm exactly"
                    );
                }
            }
        }
    }

    #[test]
    fn footer_defaults_to_none_when_omitted_from_json() {
        // `#[serde(default)]` back-compat (#432 req 5): older payloads
        // with no `footer` key deserialize to `None`.
        let table = make_table(2, 3);
        let mut json: serde_json::Value = serde_json::to_value(&table).unwrap();
        json.as_object_mut().unwrap().remove("footer");
        let back: DataTable = serde_json::from_value(json).unwrap();
        assert_eq!(back.footer, None);
    }
}
