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
}

/// Events a `DataTable` emits back to the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataTableEvent {
    /// User clicked a column header — app should toggle sort.
    HeaderClicked { col: usize },
    /// User activated a row (click or Enter).
    RowActivated { idx: usize },
    /// User selected a row (arrow key navigation).
    RowSelected { idx: usize },
    /// User scrolled the table.
    Scroll { delta: i32, modifiers: Modifiers },
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
    /// Click on empty space below the last row.
    Empty,
}

/// Fully-resolved DataTable layout.
#[derive(Debug, Clone, PartialEq)]
pub struct DataTableLayout {
    pub header_height: f32,
    pub row_height: f32,
    pub columns: Vec<ResolvedColumn>,
    /// Number of rows that fit in the viewport (excluding header).
    pub visible_rows: usize,
    pub viewport_width: f32,
    pub viewport_height: f32,
    /// Width reserved for the scrollbar (0 when hidden).
    pub scrollbar_width: f32,
}

/// Grab zone half-width for column divider detection (surface units).
const DIVIDER_GRAB_PX: f32 = 3.0;

impl DataTableLayout {
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
            // Check dividers first (higher priority than header body).
            for (i, rc) in self.columns.iter().enumerate() {
                let right_edge = rc.x + rc.width;
                if (x - right_edge).abs() <= DIVIDER_GRAB_PX && i + 1 < self.columns.len() {
                    return DataTableHit::HeaderDivider { col: i };
                }
            }
            let col = self
                .columns
                .iter()
                .position(|c| x >= c.x && x < c.x + c.width);
            return match col {
                Some(col) => DataTableHit::Header { col },
                None => DataTableHit::Empty,
            };
        }
        let row_in_viewport = ((y - self.header_height) / self.row_height).floor() as usize;
        let abs_idx = scroll_offset + row_in_viewport;
        if abs_idx < total_rows {
            DataTableHit::Row { idx: abs_idx }
        } else {
            DataTableHit::Empty
        }
    }

    pub fn column_hit(&self, x: f32) -> Option<usize> {
        self.columns
            .iter()
            .position(|c| x >= c.x && x < c.x + c.width)
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
        let col_area = (viewport_width - sb_w).max(0.0);
        let resolved = resolve_columns(&self.columns, col_area, &measure);
        let body_height = (viewport_height - header_height).max(0.0);
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
        }
    }
}

/// Resolve column widths from definitions + viewport width.
/// Shared logic that TreeTable will also use.
fn resolve_columns<F>(columns: &[Column], viewport_width: f32, measure: &F) -> Vec<ResolvedColumn>
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
    for col in columns {
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
    if total_flex > 0.0 && remaining > 0.0 {
        for (i, col) in columns.iter().enumerate() {
            if let ColumnWidth::Flex(weight) = col.width {
                widths[i] = (weight.max(0.0) / total_flex) * remaining;
            }
        }
    }

    // Pass 3: compute x positions.
    let mut x = 0.0_f32;
    widths
        .iter()
        .map(|&w| {
            let rc = ResolvedColumn { x, width: w };
            x += w;
            rc
        })
        .collect()
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
}
