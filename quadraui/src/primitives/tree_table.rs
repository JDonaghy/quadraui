//! `TreeTable` primitive: a [`DataTable`] whose column 0 is a tree cell —
//! hierarchical rows with expand/collapse, built **by composition** from
//! `DataTable` and the shared tree-cell helpers rather than as a parallel
//! primitive with its own row type, layout or painter.
//!
//! Like [`crate::TreeView`], rows are pre-flattened by the app: [`TreeTable::tree`]
//! is parallel to [`DataTable::rows`] on [`TreeTable::table`] — `tree[i]`
//! describes the tree shape (path, indent, expand state, icon) of
//! `table.rows[i]`, whose cell 0 carries that row's label text.
//!
//! # Backend contract
//!
//! Render exactly as [`DataTable`] does, except column 0 of every row is
//! prefixed with indent + expand/collapse chevron + icon before its label
//! — see [`crate::tui::draw_tree_table`] for the TUI rasteriser. Clicking
//! the chevron (or Left/Right on the keyboard) is the one interaction
//! `DataTable` doesn't already have; apps compute it from
//! [`TreeTableLayout::hit_test`]'s [`TreeTableHit::Chevron`] and
//! [`crate::compose::tree_nav::TreeNavCore::horizontal_nav`] — the same
//! navigation core [`crate::compose::TreeController`] already uses for
//! `TreeView`, reused here via [`TreeTableNode`]'s [`NavRow`] impl rather
//! than re-derived.

use crate::compose::tree_nav::NavRow;
use crate::event::Rect;
use crate::primitives::data_table::{
    Column, ColumnMeasure, DataTable, DataTableHit, DataTableLayout,
};
use crate::types::{Icon, Modifiers, TreePath};
use serde::{Deserialize, Serialize};

/// Per-row tree metadata for one [`TreeTable`] row, parallel to
/// [`DataTable::rows`] on [`TreeTable::table`].
///
/// `#[non_exhaustive]` with [`TreeTableNode::new`] as the constructor, so
/// a future field (per [`crate::primitives::tree::TreeRow`]'s `badge`, for
/// example) can land without breaking a downstream struct literal — see
/// `CLAUDE.md`'s *Downstream consumers* section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TreeTableNode {
    /// This row's path. Mirrors [`crate::TreeRow::path`].
    pub path: TreePath,
    /// Visual indent level, in the backend's indent unit. Mirrors
    /// [`crate::TreeRow::indent`].
    pub indent: u16,
    /// `None` marks a leaf; `Some(true)`/`Some(false)` an expanded/collapsed
    /// branch. Mirrors [`crate::TreeRow::is_expanded`].
    pub is_expanded: Option<bool>,
    /// Optional leading icon, painted between the chevron and the label.
    pub icon: Option<Icon>,
}

impl TreeTableNode {
    /// A leaf node at `path`/`indent` with no icon. Use
    /// [`Self::with_expanded`]/[`Self::with_icon`] to turn it into a
    /// branch or add an icon.
    #[must_use]
    pub fn new(path: TreePath, indent: u16) -> Self {
        Self {
            path,
            indent,
            is_expanded: None,
            icon: None,
        }
    }

    /// Mark this node a branch, expanded or collapsed.
    #[must_use]
    pub fn with_expanded(mut self, expanded: bool) -> Self {
        self.is_expanded = Some(expanded);
        self
    }

    /// Attach a leading icon.
    #[must_use]
    pub fn with_icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }
}

impl NavRow for TreeTableNode {
    fn path(&self) -> &TreePath {
        &self.path
    }
    fn indent(&self) -> u16 {
        self.indent
    }
    fn is_expanded(&self) -> Option<bool> {
        self.is_expanded
    }
}

/// Declarative description of a `TreeTable` widget: a [`DataTable`] plus
/// per-row tree metadata. Every [`DataTable`] field keeps its existing
/// meaning unchanged — selection, scroll, sort, focus, h-scroll, column
/// overrides, footer and `bordered` all behave exactly as they do on a
/// plain `DataTable`. `tree` only ever adds the column-0 tree cell on top.
///
/// # Examples
///
/// ```
/// use quadraui::{
///     Column, ColumnMeasure, DataRow, DataTable, StyledText, TreeTable, TreeTableHit,
///     TreeTableNode, WidgetId,
/// };
///
/// let table = DataTable {
///     id: WidgetId::new("tree_table:files"),
///     columns: vec![Column {
///         title: "Name".to_string(),
///         width: Default::default(),
///         align: Default::default(),
///     }],
///     rows: vec![
///         DataRow {
///             cells: vec![StyledText::plain("src")],
///             decoration: Default::default(),
///         },
///         DataRow {
///             cells: vec![StyledText::plain("main.rs")],
///             decoration: Default::default(),
///         },
///     ],
///     selected_idx: None,
///     scroll_offset: 0,
///     sort: None,
///     has_focus: true,
///     show_scrollbar: false,
///     min_total_width: None,
///     h_scroll: 0.0,
///     column_overrides: vec![],
///     footer: None,
///     bordered: false,
/// };
/// let tree = vec![
///     TreeTableNode::new(vec![0], 0).with_expanded(true),
///     TreeTableNode::new(vec![0, 0], 1),
/// ];
/// let tree_table = TreeTable { table, tree };
///
/// let layout = tree_table.layout(80.0, 10.0, 1.0, 1.0, 1.0, |_| ColumnMeasure::new(20.0), |n| {
///     n.is_expanded.map(|_| (0.0, 1.0))
/// }, 0.0);
/// assert_eq!(
///     layout.hit_test(0.0, 1.0, 0, tree_table.table.rows.len()),
///     TreeTableHit::Chevron { idx: 0 }
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TreeTable {
    pub table: DataTable,
    /// Parallel to `table.rows`: `tree[i]` is `table.rows[i]`'s tree shape.
    /// Shorter than `table.rows` is tolerated (missing entries paint/lay
    /// out as a plain `DataTable` row, with no chevron); the two are
    /// expected to be the same length in practice.
    pub tree: Vec<TreeTableNode>,
}

impl TreeTable {
    /// `self.table` with only its first `kept` columns (and the parallel
    /// `column_overrides` entries) — the subset [`Self::layout`] resolves
    /// against while probing how many rightmost columns the narrow-rect
    /// rule needs to drop. Never touches `rows`, `footer`, `bordered` or
    /// any other field, so the probed layout differs from the real one
    /// only in which columns exist.
    fn table_with_columns(&self, kept: usize) -> DataTable {
        let mut table = self.table.clone();
        table.columns.truncate(kept);
        table.column_overrides.truncate(kept);
        table
    }

    /// Compute layout from viewport dimensions plus column and
    /// chevron measurers.
    ///
    /// Calls [`DataTable::layout`] verbatim for `table` (possibly more
    /// than once, to apply the narrow-rect rule below), then lays out
    /// column 0 of every visible row with `measure_chevron` — writing no
    /// row-stacking or scrolling code of its own, and no column-resolving
    /// math beyond deciding how many trailing columns reach
    /// [`DataTable::layout`]. `measure_chevron(node)` returns `(start,
    /// width)` — both relative to
    /// column 0's left edge, in the same content-space unit
    /// [`crate::ResolvedColumn::x`] uses — for a row whose chevron paints,
    /// or `None` for a leaf or a row whose chevron is hidden.
    ///
    /// `min_tree_col_width` is the narrow-rect floor: the smallest
    /// content-space width column 0 (the tree cell) may resolve to. When
    /// [`DataTable::layout`]'s own resolution would give column 0 less
    /// than that, the rightmost column is dropped and resolution retried
    /// — repeating until column 0 clears the floor or every other column
    /// is gone. If even a single surviving column 0 still can't clear it
    /// (the viewport itself is narrower than the floor), column 0 is
    /// widened to the floor rather than left truncated, overflowing the
    /// viewport instead of clipping the tree cell. Pass `0.0` to disable
    /// the rule entirely (never drop, never widen).
    #[allow(clippy::too_many_arguments)]
    pub fn layout<F, G>(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        row_height: f32,
        header_height: f32,
        scrollbar_width: f32,
        measure_col: F,
        measure_chevron: G,
        min_tree_col_width: f32,
    ) -> TreeTableLayout
    where
        F: Fn(&Column) -> ColumnMeasure,
        G: Fn(&TreeTableNode) -> Option<(f32, f32)>,
    {
        let mut kept = self.table.columns.len();
        let mut table = self.table.layout(
            viewport_width,
            viewport_height,
            row_height,
            header_height,
            scrollbar_width,
            &measure_col,
        );
        while kept > 1 {
            let col0_width = table.columns.first().map(|c| c.width).unwrap_or(0.0);
            if col0_width >= min_tree_col_width {
                break;
            }
            kept -= 1;
            table = self.table_with_columns(kept).layout(
                viewport_width,
                viewport_height,
                row_height,
                header_height,
                scrollbar_width,
                &measure_col,
            );
        }
        if let Some(col0) = table.columns.first_mut() {
            if col0.width < min_tree_col_width {
                col0.width = min_tree_col_width;
                table.content_width = table.content_width.max(col0.x + col0.width);
            }
        }
        let scroll_offset = self.table.scroll_offset.min(self.table.rows.len());
        let mut chevrons = Vec::new();
        if let Some(col0) = table.columns.first() {
            for i in 0..table.visible_rows {
                let abs = scroll_offset + i;
                let Some(node) = self.tree.get(abs) else {
                    break;
                };
                let Some((start, width)) = measure_chevron(node) else {
                    continue;
                };
                let y = table.inset_y + table.header_height + i as f32 * table.row_height;
                chevrons.push((abs, Rect::new(col0.x + start, y, width, table.row_height)));
            }
        }
        TreeTableLayout { table, chevrons }
    }
}

/// Hit-test result for a `TreeTable` — everything [`DataTableHit`] already
/// resolves, plus [`Self::Chevron`] for the expand/collapse glyph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeTableHit {
    /// Click on a column header.
    Header { col: usize },
    /// Click on a column header divider — start a resize drag.
    HeaderDivider { col: usize },
    /// Click on the row's expand/collapse chevron.
    Chevron { idx: usize },
    /// Click on a body row (outside the chevron).
    Row { idx: usize },
    /// Click on the pinned footer/summary row.
    Footer,
    /// Click on empty space below the last row.
    Empty,
}

/// Fully-resolved `TreeTable` layout: [`DataTable`]'s own layout, plus
/// chevron hit bounds.
///
/// `#[non_exhaustive]`, mirroring [`DataTableLayout`] — never constructed
/// by a consumer via struct literal, only ever received from
/// [`TreeTable::layout`] or a `Backend::tree_table_layout` implementation.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct TreeTableLayout {
    /// The wrapped [`DataTable`]'s own layout — columns, row/header
    /// geometry, scrollbars, h-scroll, insets — unchanged.
    pub table: DataTableLayout,
    /// `(absolute row index, chevron hit bounds)` for every visible row
    /// with a painted chevron, in [`DataTableLayout::columns`]' content
    /// space (the same space [`crate::ResolvedColumn::x`] uses) — pre-
    /// `h_scroll`, pre-[`DataTableLayout::inset_x`]. [`Self::hit_test`]
    /// is what reconciles the two: it shifts the raw pointer position
    /// into this same content space via [`DataTableLayout::content_x`]
    /// before comparing against these bounds, rather than shifting the
    /// bounds themselves. A future backend storing its own chevron
    /// bounds should follow the same convention — store pre-`h_scroll`
    /// content space, shift the click instead. Sparse: only branch rows
    /// with a chevron get an entry. [`Self::hit_test`] is the only
    /// intended reader; a caller that wants to paint from this instead
    /// should go through [`crate::tui::draw_tree_table`] (or a GTK/
    /// macOS/Win equivalent, once one exists) rather than re-deriving
    /// pixel bounds from these.
    pub chevrons: Vec<(usize, Rect)>,
}

impl TreeTableLayout {
    /// Resolve a click to a chevron / header / divider / row / footer.
    /// Chevron bounds are checked before falling through to
    /// [`DataTable`]'s own [`DataTableLayout::hit_test`], same coordinate
    /// contract: `x`/`y` are viewport-relative, as a caller's raw pointer
    /// position.
    pub fn hit_test(
        &self,
        x: f32,
        y: f32,
        scroll_offset: usize,
        total_rows: usize,
    ) -> TreeTableHit {
        let cx = self.table.content_x(x);
        for (idx, bounds) in &self.chevrons {
            if cx >= bounds.x
                && cx < bounds.x + bounds.width
                && y >= bounds.y
                && y < bounds.y + bounds.height
            {
                return TreeTableHit::Chevron { idx: *idx };
            }
        }
        match self.table.hit_test(x, y, scroll_offset, total_rows) {
            DataTableHit::Header { col } => TreeTableHit::Header { col },
            DataTableHit::HeaderDivider { col } => TreeTableHit::HeaderDivider { col },
            DataTableHit::Row { idx } => TreeTableHit::Row { idx },
            DataTableHit::Footer => TreeTableHit::Footer,
            DataTableHit::Empty => TreeTableHit::Empty,
        }
    }

    /// The event a [`TreeTableHit::Chevron { idx }`](TreeTableHit::Chevron)
    /// resolution should emit: `tree[idx]`'s current
    /// [`TreeTableNode::is_expanded`], flipped. `None` when `idx` has no
    /// entry in `tree` or that entry is a leaf (`is_expanded: None`) —
    /// the same "degrade without panicking" rule [`TreeTable::layout`]
    /// already applies to a short `tree`. The app applies the returned
    /// event the same way it would apply one built by
    /// [`TreeTableEvent::from_horizontal_nav`], so a chevron click and
    /// Left/Right on the keyboard both funnel through one handler.
    #[must_use]
    pub fn chevron_toggle(idx: usize, tree: &[TreeTableNode]) -> Option<TreeTableEvent> {
        let node = tree.get(idx)?;
        let expand = !node.is_expanded?;
        Some(TreeTableEvent::ToggleExpanded {
            path: node.path.clone(),
            expand,
        })
    }
}

/// Events a `TreeTable` emits back to the app — everything
/// [`crate::DataTableEvent`] already documents, plus [`Self::ToggleExpanded`]
/// for the chevron/Left/Right interaction `DataTable` doesn't have.
/// `RowSelected`/`RowActivated`/`RowDoubleClicked`/`SelectionSet` carry the
/// row's [`TreePath`] alongside the flat index every `DataTableEvent`
/// counterpart already carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TreeTableEvent {
    /// User clicked a column header — app should toggle sort.
    HeaderClicked { col: usize },
    /// User pressed Enter while the table had focus and a row was selected.
    RowActivated { idx: usize, path: TreePath },
    /// User selected a row — arrow-key navigation or a plain single click.
    RowSelected { idx: usize, path: TreePath },
    /// User double-clicked a row.
    RowDoubleClicked { idx: usize, path: TreePath },
    /// User scrolled the table.
    Scroll { delta: i32, modifiers: Modifiers },
    /// User dragged a column divider to resize.
    ColumnResized { col: usize, width: f32 },
    /// The full multi-selection changed. `indices`/`paths` are the
    /// complete, sorted, de-duplicated selection, parallel to each other.
    SelectionSet {
        indices: Vec<usize>,
        paths: Vec<TreePath>,
    },
    /// The chevron was clicked, or Left/Right on the keyboard resolved to
    /// an expand/collapse (as opposed to a parent/first-child selection
    /// move) via [`crate::compose::tree_nav::TreeNavCore::horizontal_nav`].
    /// `expand: true` means the app should set this row's
    /// [`TreeTableNode::is_expanded`] to `Some(true)` (and may need to
    /// push newly-visible child rows next frame); `false` means collapse.
    ToggleExpanded { path: TreePath, expand: bool },
}

impl TreeTableEvent {
    /// Map [`crate::compose::tree_nav::HorizontalNav`]'s branch-toggle
    /// variants — the Left/Right-on-the-keyboard counterpart of
    /// [`TreeTableLayout::chevron_toggle`]'s mouse path — to
    /// [`Self::ToggleExpanded`]. `None` for
    /// `SelectParent`/`SelectFirstChild`/`None`, which are selection
    /// moves rather than an expand/collapse and stay the app's own
    /// [`Self::RowSelected`] handling.
    #[must_use]
    pub fn from_horizontal_nav(nav: crate::compose::tree_nav::HorizontalNav) -> Option<Self> {
        use crate::compose::tree_nav::HorizontalNav;
        match nav {
            HorizontalNav::Collapse(path) => Some(Self::ToggleExpanded {
                path,
                expand: false,
            }),
            HorizontalNav::Expand(path) => Some(Self::ToggleExpanded { path, expand: true }),
            HorizontalNav::SelectParent(_)
            | HorizontalNav::SelectFirstChild(_)
            | HorizontalNav::None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::data_table::{ColumnAlign, ColumnWidth, DataRow};
    use crate::types::{StyledText, WidgetId};

    fn node(path: &[u16], indent: u16, expanded: Option<bool>) -> TreeTableNode {
        let mut n = TreeTableNode::new(path.to_vec(), indent);
        n.is_expanded = expanded;
        n
    }

    fn table(rows: usize) -> DataTable {
        DataTable {
            id: WidgetId::new("tt"),
            columns: vec![
                Column {
                    title: "Name".into(),
                    width: ColumnWidth::Flex(2.0),
                    align: ColumnAlign::Left,
                },
                Column {
                    title: "Size".into(),
                    width: ColumnWidth::FixedCells(6.0),
                    align: ColumnAlign::Right,
                },
            ],
            rows: (0..rows)
                .map(|i| DataRow {
                    cells: vec![StyledText::plain(format!("row{i}")), StyledText::plain("1")],
                    decoration: Default::default(),
                })
                .collect(),
            selected_idx: None,
            scroll_offset: 0,
            sort: None,
            has_focus: true,
            show_scrollbar: false,
            min_total_width: None,
            h_scroll: 0.0,
            column_overrides: vec![],
            footer: None,
            bordered: false,
        }
    }

    /// A `TreeTable` with every row at indent 0 and all leaves lays out
    /// identically to the equivalent `DataTable`, modulo column 0's
    /// content offset — here, with a `measure_chevron` that always
    /// returns `None` (no chevrons at all, since every row is a leaf),
    /// there is no content offset either, so the two layouts match
    /// exactly.
    #[test]
    fn all_leaves_layout_matches_the_equivalent_data_table() {
        let dt = table(5);
        let tree: Vec<TreeTableNode> = (0..5).map(|i| node(&[i as u16], 0, None)).collect();
        let tt = TreeTable {
            table: dt.clone(),
            tree,
        };
        let measure = |col: &Column| ColumnMeasure::new(col.title.chars().count() as f32);
        let plain = dt.layout(40.0, 10.0, 1.0, 1.0, 1.0, measure);
        let composed = tt.layout(40.0, 10.0, 1.0, 1.0, 1.0, measure, |_| None, 0.0);
        assert_eq!(composed.table, plain);
        assert!(composed.chevrons.is_empty());
    }

    #[test]
    fn chevron_hit_test_takes_priority_over_the_row_it_sits_on() {
        let dt = table(3);
        let tree = vec![
            node(&[0], 0, Some(true)),
            node(&[0, 0], 1, None),
            node(&[1], 0, Some(false)),
        ];
        let tt = TreeTable { table: dt, tree };
        let measure = |col: &Column| ColumnMeasure::new(col.title.chars().count() as f32);
        // Row 0 is a branch: chevron occupies content-space x in [0, 1).
        let layout = tt.layout(
            40.0,
            10.0,
            1.0,
            1.0,
            1.0,
            measure,
            |n| n.is_expanded.map(|_| (0.0, 1.0)),
            0.0,
        );
        assert_eq!(
            layout.hit_test(0.0, 1.0, 0, 3),
            TreeTableHit::Chevron { idx: 0 }
        );
        // Just past the chevron, same row, resolves to a plain Row hit.
        assert_eq!(
            layout.hit_test(2.0, 1.0, 0, 3),
            TreeTableHit::Row { idx: 0 }
        );
        // Row 1 is a leaf (no chevron measured) — always a Row hit.
        assert_eq!(
            layout.hit_test(0.0, 2.0, 0, 3),
            TreeTableHit::Row { idx: 1 }
        );
        // Header row still resolves normally.
        assert_eq!(
            layout.hit_test(0.0, 0.0, 0, 3),
            TreeTableHit::Header { col: 0 }
        );
    }

    #[test]
    fn scrolled_chevrons_use_absolute_row_indices() {
        let mut dt = table(4);
        dt.scroll_offset = 2;
        let tree = vec![
            node(&[0], 0, Some(true)),
            node(&[1], 0, Some(true)),
            node(&[2], 0, Some(true)),
            node(&[3], 0, Some(true)),
        ];
        let tt = TreeTable { table: dt, tree };
        let measure = |col: &Column| ColumnMeasure::new(col.title.chars().count() as f32);
        let layout = tt.layout(
            40.0,
            10.0,
            1.0,
            1.0,
            1.0,
            measure,
            |_| Some((0.0, 1.0)),
            0.0,
        );
        let indices: Vec<usize> = layout.chevrons.iter().map(|(idx, _)| *idx).collect();
        assert_eq!(indices, vec![2, 3]);
    }

    #[test]
    fn missing_tree_entries_degrade_to_no_chevron_without_panicking() {
        let dt = table(3);
        let tt = TreeTable {
            table: dt,
            tree: vec![node(&[0], 0, Some(true))], // shorter than `table.rows`
        };
        let measure = |col: &Column| ColumnMeasure::new(col.title.chars().count() as f32);
        let layout = tt.layout(
            40.0,
            10.0,
            1.0,
            1.0,
            1.0,
            measure,
            |_| Some((0.0, 1.0)),
            0.0,
        );
        assert_eq!(layout.chevrons.len(), 1);
    }

    /// Reproduces the narrow-rect scenario the acceptance criteria name:
    /// a `Flex` tree column followed by several `FixedCells` columns,
    /// resolved at a viewport too narrow to give all of them anything.
    /// With no floor, column 0 collapses to width `0.0` (the fixed
    /// columns eat the whole viewport before the flex pass ever runs);
    /// with a floor, the rightmost fixed columns are dropped one at a
    /// time until column 0 clears it.
    fn narrow_tree_table() -> TreeTable {
        let dt = DataTable {
            id: WidgetId::new("tt:narrow"),
            columns: vec![
                Column {
                    title: "Name".into(),
                    width: ColumnWidth::Flex(3.0),
                    align: ColumnAlign::Left,
                },
                Column {
                    title: "Modified".into(),
                    width: ColumnWidth::FixedCells(14.0),
                    align: ColumnAlign::Left,
                },
                Column {
                    title: "Kind".into(),
                    width: ColumnWidth::FixedCells(8.0),
                    align: ColumnAlign::Left,
                },
                Column {
                    title: "Size".into(),
                    width: ColumnWidth::FixedCells(10.0),
                    align: ColumnAlign::Right,
                },
            ],
            rows: (0..3)
                .map(|i| DataRow {
                    cells: vec![
                        StyledText::plain(format!("row{i}")),
                        StyledText::plain("—"),
                        StyledText::plain("—"),
                        StyledText::plain("—"),
                    ],
                    decoration: Default::default(),
                })
                .collect(),
            selected_idx: None,
            scroll_offset: 0,
            sort: None,
            has_focus: true,
            show_scrollbar: false,
            min_total_width: None,
            h_scroll: 0.0,
            column_overrides: vec![],
            footer: None,
            bordered: false,
        };
        let tree: Vec<TreeTableNode> = (0..3).map(|i| node(&[i as u16], 0, None)).collect();
        TreeTable { table: dt, tree }
    }

    #[test]
    fn with_no_floor_a_narrow_rect_can_starve_the_tree_column() {
        let tt = narrow_tree_table();
        let measure = |col: &Column| ColumnMeasure::new(col.title.chars().count() as f32);
        let layout = tt.layout(20.0, 10.0, 1.0, 1.0, 1.0, measure, |_| None, 0.0);
        assert_eq!(layout.table.columns.len(), 4);
        assert_eq!(layout.table.columns[0].width, 0.0);
    }

    #[test]
    fn narrow_rect_drops_rightmost_columns_before_starving_the_tree_column() {
        let tt = narrow_tree_table();
        let measure = |col: &Column| ColumnMeasure::new(col.title.chars().count() as f32);
        let layout = tt.layout(20.0, 10.0, 1.0, 1.0, 1.0, measure, |_| None, 5.0);
        // The two rightmost (`Kind`, `Size`) columns are dropped — only
        // `Name` and `Modified` survive — and column 0 clears the floor.
        assert_eq!(layout.table.columns.len(), 2);
        assert!(layout.table.columns[0].width >= 5.0);
    }

    #[test]
    fn narrow_rect_widens_the_tree_column_past_the_floor_when_dropping_everything_else_still_isnt_enough(
    ) {
        let mut tt = narrow_tree_table();
        tt.table.columns.truncate(1); // only the tree column itself is left
        tt.table.rows.iter_mut().for_each(|r| r.cells.truncate(1));
        let measure = |col: &Column| ColumnMeasure::new(col.title.chars().count() as f32);
        // Viewport narrower than the floor: even the lone surviving
        // column can't clear it by resolution alone.
        let layout = tt.layout(2.0, 10.0, 1.0, 1.0, 1.0, measure, |_| None, 5.0);
        assert_eq!(layout.table.columns.len(), 1);
        assert_eq!(layout.table.columns[0].width, 5.0);
    }
}
