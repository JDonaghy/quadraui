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
//! in [`DataTable::layout`] — already matches. The TUI backend is
//! cell-granular: it paints at `h_scroll.round()` (`tui::data_table`'s
//! `h_off`), so `tui::data_table::draw_data_table` overwrites the
//! returned layout's `h_scroll` with that same rounded value before
//! returning it, keeping every `hit_test`/`column_hit` call downstream
//! in agreement with the paint. A caller that hand-builds a
//! `DataTableLayout` for a cell-granular surface without going through
//! that backend function must apply the same rounding itself.

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
    /// crosses 0.5 (#550 round 2). See `tui::data_table::draw_data_table`.
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

    /// Compute the `column_overrides` for a divider drag (#521 defect 1).
    ///
    /// `col` is the column to the LEFT of the dragged divider, as
    /// returned by [`DataTableHit::HeaderDivider`]. `pointer_x` is the
    /// drag pointer's current position in **viewport space** — the same
    /// space [`Self::hit_test`] took to produce `col`, so a caller keeps
    /// forwarding the raw pointer `x` and this converts once, internally
    /// (#550). `min_width` clamps both halves of the dragged pair.
    ///
    /// A divider is the boundary between column `col` and `col + 1`, so
    /// a drag must only ever move width between *those two* columns,
    /// combined width held constant, and leave every other column's
    /// resolved geometry untouched — including the dragged column's own
    /// left edge, which is fixed by the columns before it.
    ///
    /// `overrides` is the `column_overrides` in effect *before* this
    /// call (typically the in-progress drag's current state, or the
    /// table's existing overrides at drag start). Any column that does
    /// not already have an override is frozen here at its *currently
    /// resolved* width before the pair is adjusted — this must happen
    /// unconditionally, not just for `Flex` columns, because leaving an
    /// unrelated `Flex` column unresolved would let pass 2's
    /// redistribution reshuffle it the moment the pair's weights are
    /// pulled out of `total_flex` (the exact "moving the left column
    /// makes the problem disappear" mechanism reported in #521: whichever
    /// columns are still unpinned divide up whatever space the pinned
    /// ones didn't claim, so the split among *them* changes even though
    /// the user never touched them). Freezing every column up front makes
    /// the result independent of drag history: whatever the table's
    /// current resolved widths are, that's what gets pinned, regardless
    /// of which dividers produced them.
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
        let pair_total = self.columns[col].width + self.columns[col + 1].width;
        let min_width = min_width.max(0.0);
        let lo = min_width.min(pair_total);
        let hi = (pair_total - min_width).max(lo);
        let col_x = self.columns[col].x;
        let new_left = (self.content_x(pointer_x) - col_x).clamp(lo, hi);
        next[col] = Some(new_left);
        next[col + 1] = Some(pair_total - new_left);
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
    for (i, col) in columns.iter().enumerate() {
        if let Some(Some(ow)) = overrides.get(i) {
            let w = ow.min(remaining).max(0.0);
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
    // exceed the viewport (e.g. `min_total_width`), `x + width` here is
    // already `>= viewport_width` and this is a no-op, so h-scroll is
    // untouched.
    if let Some(last) = resolved.last_mut() {
        let shortfall = viewport_width - (last.x + last.width);
        if shortfall > 0.0 {
            last.width += shortfall;
        }
    }

    resolved
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

    // ── #521 defect 1: pair-resize (a divider drag moves only the two
    //    columns it separates) ────────────────────────────────────────

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
        // and drag it right by 20 units.
        let pointer_x = baseline.columns[2].x + baseline.columns[2].width + 20.0;
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
        assert_eq!(dragged[2], Some(20.0), "the pair's 60 total is conserved");
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
