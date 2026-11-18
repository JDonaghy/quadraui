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
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
use crate::primitives::data_table::DataTablePaintState;
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

// Shared GUI (GTK/macOS/Win) paint implementation. A `TreeTable` paints
// exactly as `DataTable` does —
// `data_table::native_surface_paint::paint_with_layout`'s shared
// header/body/scrollbar/footer skeleton — with column 0 of every row
// replaced by the shared tree cell
// (`tree::native_surface_paint::tree_cell::paint_tree_cell`) followed by
// its label through the same `paint_cell` every other column uses. No
// parallel row, column or scrollbar logic of its own, mirroring
// `tui::data_table`'s identical "no `tree_table.rs`" story for TUI.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{TreeTable, TreeTableLayout, TreeTableNode};
    use crate::event::Rect;
    use crate::paint_surface::PaintSurface;
    use crate::primitives::data_table::native_surface_paint::{
        paint_cell, paint_row, paint_with_layout, SurfaceTextMeasure, TablePaintCtx,
    };
    use crate::primitives::data_table::{ColumnAlign, DataTablePaintState};
    use crate::primitives::layout_metrics::TextMeasure;
    use crate::primitives::tree::native_surface_paint::tree_cell::paint_tree_cell;
    use crate::primitives::tree::tree_chevron_glyph;
    use crate::theme::Theme;
    use crate::types::{Decoration, TreeStyle};

    /// Pixel width of one tree indent level — the same
    /// `(line_height * 0.9).round()` formula
    /// [`crate::primitives::tree::native_surface_paint::paint_with_scroll_px`]
    /// uses for `TreeView`'s own indent.
    fn indent_px(line_height: f32) -> f32 {
        (line_height * 0.9).round()
    }

    /// Resolve the layout [`paint`] paints `table` at, without touching a
    /// buffer. Thin forwarding wrapper over
    /// [`crate::primitives::layout_metrics::pixel_tree_table_layout`] (the
    /// pixel counterpart of [`crate::tui::data_table::tree_table_layout`],
    /// and the shared home for this measuring glue — see that fn's own
    /// doc) kept at this path so every GUI backend's existing
    /// `native_surface_paint::pixel_tree_table_layout` call site needs no
    /// change.
    pub(crate) fn pixel_tree_table_layout(
        table: &TreeTable,
        w: f32,
        h: f32,
        line_height: f32,
        measure: &dyn TextMeasure,
    ) -> TreeTableLayout {
        crate::primitives::layout_metrics::pixel_tree_table_layout(
            table,
            w,
            h,
            line_height,
            measure,
        )
    }

    /// Paint a [`TreeTable`] into `rect` on `surface` and return the
    /// resolved [`TreeTableLayout`] — the entry point every GUI backend's
    /// `draw_tree_table` goes through. Identical to
    /// [`crate::primitives::data_table::native_surface_paint::paint_with_state`]
    /// except column 0 of every row with a `table.tree` entry is painted
    /// as indent + chevron + icon via [`paint_tree_cell`], followed by its
    /// label through the same [`paint_cell`] every other column uses —
    /// rows past `table.tree`'s length paint as a plain [`crate::DataTable`]
    /// row. No sort-glyph override (unlike `DataTable`'s own
    /// `paint_with_state`), mirroring [`crate::tui::draw_tree_table`] not
    /// taking one either.
    pub(crate) fn paint(
        table: &TreeTable,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        rect: Rect,
        line_height: f32,
        state: &DataTablePaintState,
        nerd_fonts_enabled: bool,
    ) -> TreeTableLayout {
        let style = TreeStyle::default();
        let ipx = indent_px(line_height);
        let leaf_gap = line_height * 0.8;
        let tt_layout = {
            let measure = SurfaceTextMeasure(&*surface);
            pixel_tree_table_layout(table, rect.width, rect.height, line_height, &measure)
        };
        let dt_layout = paint_with_layout(
            &table.table,
            surface,
            theme,
            rect,
            line_height,
            state,
            None,
            nerd_fonts_enabled,
            tt_layout.table.clone(),
            |surface, ctx, row_y, idx, hovered, striped| match table.tree.get(idx) {
                Some(node) => paint_tree_table_row(
                    surface,
                    ctx,
                    row_y,
                    idx,
                    node,
                    hovered,
                    &style,
                    ipx,
                    leaf_gap,
                    nerd_fonts_enabled,
                ),
                None => paint_row(surface, ctx, row_y, idx, hovered, striped, None),
            },
        );
        TreeTableLayout {
            table: dt_layout,
            chevrons: tt_layout.chevrons,
        }
    }

    /// Paint one `TreeTable` body row: [`paint_row`]'s selection/hover tint
    /// (zebra striping isn't offered — mirrors
    /// [`crate::tui::data_table::draw_tree_table_row`] never taking a
    /// `striped` argument) and every column but 0 exactly as [`paint_cell`]
    /// already paints it, with column 0 painted as indent + chevron + icon
    /// via [`paint_tree_cell`], followed by its label through the same
    /// [`paint_cell`] every other column uses — offset past the tree-cell
    /// prefix rather than re-implementing text painting.
    #[allow(clippy::too_many_arguments)]
    fn paint_tree_table_row(
        surface: &mut dyn PaintSurface,
        ctx: &TablePaintCtx<'_>,
        row_y: f32,
        abs_idx: usize,
        node: &TreeTableNode,
        hovered_idx: Option<usize>,
        style: &TreeStyle,
        indent_px: f32,
        leaf_gap: f32,
        nerd_fonts_enabled: bool,
    ) {
        let mut hook = |surface: &mut dyn PaintSurface, col_idx: usize, cell_rect: Rect| -> bool {
            if col_idx != 0 {
                return false;
            }
            let row = &ctx.table.rows[abs_idx];
            let is_muted = row.decoration == Decoration::Muted;
            let fg = if is_muted {
                ctx.theme.muted_fg
            } else {
                ctx.theme.foreground
            };
            let chevron_glyph = node
                .is_expanded
                .filter(|_| style.show_chevrons)
                .map(|expanded| tree_chevron_glyph(style, expanded));
            let cell = paint_tree_cell(
                surface,
                cell_rect,
                0.0,
                node.indent,
                indent_px,
                node.is_expanded,
                chevron_glyph.as_deref(),
                leaf_gap,
                node.icon.as_ref(),
                nerd_fonts_enabled,
                fg,
            );
            let align = ctx
                .table
                .columns
                .first()
                .map(|c| c.align)
                .unwrap_or(ColumnAlign::Left);
            paint_cell(
                surface,
                ctx.theme,
                row.cells.first(),
                cell.label_rect,
                align,
                is_muted,
            );
            true
        };
        paint_row(
            surface,
            ctx,
            row_y,
            abs_idx,
            hovered_idx,
            false,
            Some(&mut hook),
        );
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::primitives::data_table::{Column, ColumnWidth, DataRow, DataTable};
        use crate::types::{Color, Icon, StyledText, WidgetId};
        use crate::Image;

        /// The default expanded chevron paints as the sharper codicon
        /// glyph on every GUI backend (see [`tree_chevron_glyph`]'s doc) —
        /// a `char`, not a `&str`, so tests compare against its owned
        /// `String` form.
        fn chevron_down() -> String {
            crate::codicon::CHEVRON_DOWN.to_string()
        }
        fn chevron_right() -> String {
            crate::codicon::CHEVRON_RIGHT.to_string()
        }

        /// Records every surface verb this module's paint uses — mirrors
        /// [`crate::primitives::data_table::native_surface_paint`]'s own
        /// identical test double, so this test runs on any host without
        /// Core Graphics/Direct2D/Pango.
        #[derive(Default)]
        struct RecordingSurface {
            text_runs: Vec<(Rect, String, Color)>,
        }

        impl PaintSurface for RecordingSurface {
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
            fn surface_fill_rect(&mut self, _rect: Rect, _color: Color) {}
            fn surface_fill_rounded_rect(&mut self, _rect: Rect, _radius: f32, _color: Color) {}
            fn surface_fill_rect_alpha(&mut self, _rect: Rect, _color: Color, _alpha: f32) {}
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.text_runs.push((rect, text.to_string(), color));
            }
            fn surface_draw_line(
                &mut self,
                _from: crate::Point,
                _to: crate::Point,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        fn node(path: &[u16], indent: u16, expanded: Option<bool>) -> TreeTableNode {
            let mut n = TreeTableNode::new(path.to_vec(), indent);
            n.is_expanded = expanded;
            n
        }

        fn table(labels: &[&str]) -> DataTable {
            DataTable {
                id: WidgetId::new("tt"),
                columns: vec![Column {
                    title: "Name".into(),
                    width: ColumnWidth::Flex(1.0),
                    align: ColumnAlign::Left,
                }],
                rows: labels
                    .iter()
                    .map(|l| DataRow {
                        cells: vec![StyledText::plain(*l)],
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

        /// A branch row paints its chevron glyph, then the label offset
        /// past it — the same ordering [`crate::tui::draw_tree_table`]
        /// paints, just through [`PaintSurface`] instead of a `Buffer`.
        #[test]
        fn branch_row_paints_chevron_then_label_offset_past_it() {
            let tt = TreeTable {
                table: table(&["src"]),
                tree: vec![node(&[0], 0, Some(true))],
            };
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            let state = DataTablePaintState::default();
            let layout = paint(
                &tt,
                &mut surface,
                &theme,
                Rect::new(0.0, 0.0, 200.0, 100.0),
                16.0,
                &state,
                false,
            );
            // The default codicon expanded chevron paints as its own text
            // run, and "src" paints as a separate run strictly to its right.
            let chevron = surface
                .text_runs
                .iter()
                .find(|(_, t, _)| *t == chevron_down())
                .expect("branch row must paint its chevron glyph");
            let label = surface
                .text_runs
                .iter()
                .find(|(_, t, _)| t == "src")
                .expect("branch row must still paint its label");
            assert!(
                label.0.x > chevron.0.x,
                "label must start right of the chevron, got label.x={} chevron.x={}",
                label.0.x,
                chevron.0.x
            );
            // The hit-test bounds `TreeTable::layout` resolves must land
            // exactly on the glyph that was actually painted, not a few
            // pixels to either side.
            assert_eq!(layout.chevrons.len(), 1);
            assert_eq!(
                layout.chevrons[0].1.x, chevron.0.x,
                "chevron hit bounds must start where the glyph was painted"
            );
        }

        /// A leaf row (`is_expanded: None`) paints no chevron at all, but
        /// still paints its label.
        #[test]
        fn leaf_row_paints_no_chevron_but_still_paints_label() {
            let tt = TreeTable {
                table: table(&["main.rs"]),
                tree: vec![node(&[0], 0, None)],
            };
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            let state = DataTablePaintState::default();
            paint(
                &tt,
                &mut surface,
                &theme,
                Rect::new(0.0, 0.0, 200.0, 100.0),
                16.0,
                &state,
                false,
            );
            assert!(
                !surface
                    .text_runs
                    .iter()
                    .any(|(_, t, _)| *t == chevron_down() || *t == chevron_right()),
                "a leaf row must not paint a chevron glyph"
            );
            assert!(
                surface.text_runs.iter().any(|(_, t, _)| t == "main.rs"),
                "a leaf row must still paint its label"
            );
        }

        /// Deeper indent pushes the chevron (and therefore the label)
        /// further right — the same monotonic-with-depth contract
        /// [`crate::primitives::tree::native_surface_paint::tree_cell`]'s
        /// own tests assert for `TreeView`.
        #[test]
        fn deeper_indent_shifts_the_chevron_right() {
            let shallow = TreeTable {
                table: table(&["a"]),
                tree: vec![node(&[0], 0, Some(true))],
            };
            let deep = TreeTable {
                table: table(&["a"]),
                tree: vec![node(&[0], 2, Some(true))],
            };
            let chevron_x = |tt: &TreeTable| -> f32 {
                let mut surface = RecordingSurface::default();
                let theme = Theme::default();
                let state = DataTablePaintState::default();
                paint(
                    tt,
                    &mut surface,
                    &theme,
                    Rect::new(0.0, 0.0, 200.0, 100.0),
                    16.0,
                    &state,
                    false,
                );
                surface
                    .text_runs
                    .iter()
                    .find(|(_, t, _)| *t == chevron_down())
                    .expect("branch row must paint its chevron glyph")
                    .0
                    .x
            };
            assert!(chevron_x(&deep) > chevron_x(&shallow));
        }

        /// A row past `tree`'s length falls through to a plain
        /// [`paint_row`] body row — same "degrade without panicking"
        /// contract [`TreeTable::layout`] already has, exercised here at
        /// the paint layer: the label still paints even with no tree
        /// metadata to drive a chevron.
        #[test]
        fn row_missing_a_tree_entry_still_paints_its_label() {
            let mut t = table(&["a", "b"]);
            t.rows[1].cells = vec![StyledText::plain("orphan")];
            let tt = TreeTable {
                table: t,
                tree: vec![node(&[0], 0, Some(true))], // shorter than `table.rows`
            };
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            let state = DataTablePaintState::default();
            paint(
                &tt,
                &mut surface,
                &theme,
                Rect::new(0.0, 0.0, 200.0, 100.0),
                16.0,
                &state,
                false,
            );
            assert!(surface.text_runs.iter().any(|(_, t, _)| t == "orphan"));
        }

        /// An icon paints between the chevron and the label.
        #[test]
        fn icon_paints_between_chevron_and_label() {
            let mut tree_node = node(&[0], 0, Some(true));
            tree_node.icon = Some(Icon::new("f", "f"));
            let tt = TreeTable {
                table: table(&["main.rs"]),
                tree: vec![tree_node],
            };
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            let state = DataTablePaintState::default();
            paint(
                &tt,
                &mut surface,
                &theme,
                Rect::new(0.0, 0.0, 200.0, 100.0),
                16.0,
                &state,
                false,
            );
            let chevron_x = surface
                .text_runs
                .iter()
                .find(|(_, t, _)| *t == chevron_down())
                .expect("branch row must paint its chevron glyph")
                .0
                .x;
            let icon_x = surface
                .text_runs
                .iter()
                .find(|(_, t, _)| t == "f")
                .expect("icon must paint")
                .0
                .x;
            let label_x = surface
                .text_runs
                .iter()
                .find(|(_, t, _)| t == "main.rs")
                .expect("label must still paint")
                .0
                .x;
            assert!(chevron_x < icon_x && icon_x < label_x);
        }

        /// [`pixel_tree_table_layout`]'s narrow-rect floor still applies
        /// through the pixel entry point: a `Flex` tree column crowded by
        /// several `FixedCells` columns at a narrow viewport drops the
        /// rightmost columns rather than starving column 0, mirroring
        /// [`super::super::tests::narrow_rect_drops_rightmost_columns_before_starving_the_tree_column`]
        /// at the primitive layer.
        #[test]
        fn pixel_layout_applies_the_narrow_rect_floor() {
            let mut t = table(&["row0"]);
            t.columns = vec![
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
                    title: "Size".into(),
                    width: ColumnWidth::FixedCells(10.0),
                    align: ColumnAlign::Right,
                },
            ];
            t.rows[0].cells = vec![
                StyledText::plain("row0"),
                StyledText::plain("—"),
                StyledText::plain("—"),
            ];
            let tt = TreeTable {
                table: t,
                tree: vec![node(&[0], 0, None)],
            };
            let surface = RecordingSurface::default();
            let layout =
                pixel_tree_table_layout(&tt, 2000.0, 10.0, 16.0, &SurfaceTextMeasure(&surface));
            assert_eq!(
                layout.table.columns.len(),
                3,
                "a generous viewport is wide enough for every column at this font size"
            );

            let narrow_layout =
                pixel_tree_table_layout(&tt, 20.0, 10.0, 16.0, &SurfaceTextMeasure(&surface));
            assert!(
                narrow_layout.table.columns.len() < 3,
                "a narrow viewport must drop a rightmost column rather than \
                 starve column 0 below its floor"
            );
        }
    }
}

// ── Accessibility ────────────────────────────────────────────────────

/// A `TreeTable` reports exactly what its wrapped [`DataTable`] does —
/// pure delegation, no tree-specific role/state of its own. The private
/// per-row helpers in `crate::a11y::tree` that would give each row a
/// `TreeItem` role plus level/expanded state (the way
/// [`crate::primitives::tree::TreeView`]'s own `A11yPrimitive` impl
/// does) aren't reachable from this module; richer per-row semantics
/// are a follow-up there, not a layout or paint concern this type's
/// [`Primitive`] impl below needs to wait on.
impl crate::a11y::A11yPrimitive for TreeTable {
    fn a11y_info(&self) -> crate::a11y::A11yInfo {
        self.table.a11y_info()
    }

    fn a11y_children(&self) -> Vec<crate::a11y::A11yChild> {
        self.table.a11y_children()
    }

    fn a11y_focus_key(&self) -> Option<String> {
        self.table.a11y_focus_key()
    }
}

// ── `crate::primitive::Primitive` impl ──────────────────────────────
//
// Composes `DataTable`'s own layout/paint (via `TreeTable::layout`,
// which already delegates to `DataTable::layout` for every column but
// the chevron) and the shared tree cell
// (`crate::primitives::tree::native_surface_paint::tree_cell`) — no
// parallel row, column or scrollbar logic of its own, mirroring this
// module's existing "no `tree_table.rs`" story for TUI (see
// `crate::tui::data_table::draw_tree_table`'s doc).

/// [`crate::primitive::Primitive::Layout`] for [`TreeTable`] — the
/// resolved [`TreeTableLayout`] paired with the absolute area it was
/// laid out into plus the row count/scroll offset
/// [`TreeTableLayout::hit_test`]'s fixed signature needs but doesn't
/// itself carry.
///
/// `#[non_exhaustive]`: backend-returned, never constructed by a
/// consumer via struct literal, same convention as [`TreeTableLayout`]
/// itself.
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct TreeTablePrimitiveLayout {
    /// Absolute area [`TreeTable::layout`] was (indirectly) called
    /// with — [`Self::hit_test`] subtracts this rect's origin before
    /// delegating to [`TreeTableLayout::hit_test`].
    pub area: Rect,
    /// Row height resolved from the [`crate::primitive::LayoutCtx`]
    /// [`Primitive::layout`](crate::primitive::Primitive::layout) was
    /// called with.
    pub line_height: f32,
    /// Nerd-Font toggle resolved the same way.
    pub nerd_fonts_enabled: bool,
    /// The table-local layout itself.
    pub layout: TreeTableLayout,
    /// [`DataTable::scroll_offset`] (via [`TreeTable::table`]) at the
    /// time [`Primitive::layout`] was called — [`Self::hit_test`] needs
    /// this to resolve a `y` into an absolute row index.
    pub scroll_offset: usize,
    /// `self.table.rows.len()` at the same time, for the same reason.
    pub total_rows: usize,
}

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
impl TreeTablePrimitiveLayout {
    /// Hit-test at an absolute `(x, y)` — subtracts [`Self::area`]'s
    /// origin before delegating to [`TreeTableLayout::hit_test`].
    pub fn hit_test(&self, x: f32, y: f32) -> TreeTableHit {
        self.layout.hit_test(
            x - self.area.x,
            y - self.area.y,
            self.scroll_offset,
            self.total_rows,
        )
    }
}

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
impl crate::primitive::Primitive for TreeTable {
    type Layout = TreeTablePrimitiveLayout;
    type Event = TreeTableEvent;

    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> Self::Layout {
        // Same TUI-vs-pixel-backend discriminator `ListView`'s own
        // `Primitive::layout` uses — see that impl's doc for why
        // `ctx.line_height` is reliable even when both features are
        // compiled into the same binary.
        #[cfg(feature = "tui")]
        if ctx.line_height <= 1.0 {
            let area = crate::tui::backend::q_rect_to_ratatui(rect);
            let layout = crate::tui::tree_table_layout(area, self);
            return TreeTablePrimitiveLayout {
                area: rect,
                line_height: ctx.line_height,
                nerd_fonts_enabled: ctx.nerd_fonts_enabled,
                scroll_offset: self.table.scroll_offset,
                total_rows: self.table.rows.len(),
                layout,
            };
        }
        // No live text measurer reaches `Primitive::layout` (its
        // signature carries only `LayoutCtx`'s flat `char_width`), so
        // column titles and the chevron glyph are both estimated from
        // it — the same no-live-font fallback
        // [`crate::primitives::layout_metrics::pixel_tree_table_layout`]
        // already applies for any pixel backend with no live glyph
        // measurer reachable. `Self::layout` (this method) contributes
        // no layout math of its own beyond resolving that one measurer.
        let measure = crate::primitives::layout_metrics::CharWidthMeasure(ctx.char_width);
        let layout = crate::primitives::layout_metrics::pixel_tree_table_layout(
            self,
            rect.width,
            rect.height,
            ctx.line_height,
            &measure,
        );
        TreeTablePrimitiveLayout {
            area: rect,
            line_height: ctx.line_height,
            nerd_fonts_enabled: ctx.nerd_fonts_enabled,
            scroll_offset: self.table.scroll_offset,
            total_rows: self.table.rows.len(),
            layout,
        }
    }

    fn paint(
        &self,
        layout: &Self::Layout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        theme: &crate::theme::Theme,
        _style: &crate::style::Style,
    ) {
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            let state = DataTablePaintState::default();
            native_surface_paint::paint(
                self,
                surface,
                theme,
                layout.area,
                layout.line_height,
                &state,
                layout.nerd_fonts_enabled,
            );
        }
        // A `tui`-only build has no `PaintSurface` to paint into at all —
        // `self` reaching the screen on TUI goes through `Self::paint_cells`
        // instead, which `DrawsPrimitives::draw_primitive`'s default body
        // calls in place of this method whenever `Backend::cell_buffer`
        // answers `Some`.
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        {
            let _ = (layout, surface, theme, _style);
        }
    }

    /// Delegates straight to [`crate::tui::draw_tree_table`] — the exact
    /// function [`crate::tui::backend::TuiBackend::draw_tree_table`]
    /// itself calls — so painting `TreeTable` through
    /// [`crate::primitive::Primitive`] on TUI is byte-for-byte identical
    /// to painting it through `Backend::draw_tree_table` by construction.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &Self::Layout,
        buf: &mut ratatui::buffer::Buffer,
        theme: &crate::theme::Theme,
    ) {
        let area = crate::tui::backend::q_rect_to_ratatui(layout.area);
        crate::tui::draw_tree_table(
            buf,
            area,
            self,
            theme,
            /* hovered_idx */ None,
            layout.nerd_fonts_enabled,
        );
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

    /// `TreeTable`'s `A11yPrimitive` impl is pure delegation to the
    /// wrapped `DataTable` — every method must answer exactly what
    /// calling it on `self.table` directly would.
    #[test]
    fn a11y_primitive_delegates_to_the_wrapped_data_table() {
        use crate::a11y::A11yPrimitive;

        let mut dt = table(3);
        dt.selected_idx = Some(1);
        let tree: Vec<TreeTableNode> = (0..3).map(|i| node(&[i as u16], 0, None)).collect();
        let tt = TreeTable {
            table: dt.clone(),
            tree,
        };

        assert_eq!(tt.a11y_info(), dt.a11y_info());
        assert_eq!(tt.a11y_children(), dt.a11y_children());
        assert_eq!(tt.a11y_focus_key(), dt.a11y_focus_key());
        assert_eq!(tt.a11y_focus_key(), Some("row.1".to_string()));
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

#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[cfg(test)]
mod primitive_parity_tests {
    use super::*;
    use crate::backend::ImagePaintResult;
    use crate::event::{Point, Viewport};
    use crate::primitive::{LayoutCtx, Primitive};
    use crate::primitives::data_table::{Column, ColumnAlign, ColumnWidth, DataRow};
    use crate::style::Style;
    use crate::theme::Theme;
    use crate::types::{Color, StyledText, WidgetId};
    use crate::Image;
    use crate::PaintSurface;

    /// Records every verb call as a formatted line, in call order — a
    /// byte-for-byte equal log from two different call paths is strong
    /// evidence they painted the same thing. Mirrors
    /// `crate::primitives::list::primitive_parity_tests::LogSurface`.
    #[derive(Default)]
    struct LogSurface {
        calls: Vec<String>,
    }

    impl PaintSurface for LogSurface {
        fn surface_begin_frame(&mut self, _viewport: Viewport) {}
        fn surface_end_frame(&mut self) {}
        fn surface_viewport(&self) -> Viewport {
            Viewport::new(300.0, 200.0, 1.0)
        }
        fn surface_line_height(&self) -> f32 {
            16.0
        }
        fn surface_char_width(&self) -> f32 {
            8.0
        }
        fn surface_measure_text(&self, text: &str) -> (f32, f32) {
            (text.chars().count() as f32 * 8.0, 14.0)
        }
        fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
            self.calls.push(format!("fill_rect {rect:?} {color:?}"));
        }
        fn surface_fill_rounded_rect(&mut self, rect: Rect, radius: f32, color: Color) {
            self.calls
                .push(format!("fill_rounded_rect {rect:?} {radius} {color:?}"));
        }
        fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
            self.calls
                .push(format!("stroke_rect {rect:?} {color:?} {stroke_width}"));
        }
        fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
            self.calls
                .push(format!("draw_text_run {rect:?} {text:?} {color:?}"));
        }
        fn surface_draw_line(&mut self, from: Point, to: Point, color: Color, stroke_width: f32) {
            self.calls.push(format!(
                "draw_line {from:?} {to:?} {color:?} {stroke_width}"
            ));
        }
        fn surface_push_clip(&mut self, rect: Rect) {
            self.calls.push(format!("push_clip {rect:?}"));
        }
        fn surface_pop_clip(&mut self) {
            self.calls.push("pop_clip".to_string());
        }
        fn surface_draw_image(&mut self, rect: Rect, _image: &Image) -> ImagePaintResult {
            self.calls.push(format!("draw_image {rect:?}"));
            ImagePaintResult::Unsupported
        }
    }

    fn node(path: &[u16], indent: u16, expanded: Option<bool>) -> TreeTableNode {
        let mut n = TreeTableNode::new(path.to_vec(), indent);
        n.is_expanded = expanded;
        n
    }

    fn sample_tree_table() -> TreeTable {
        let table = DataTable {
            id: WidgetId::new("files"),
            columns: vec![Column {
                title: "Name".to_string(),
                width: ColumnWidth::Flex(1.0),
                align: ColumnAlign::Left,
            }],
            rows: vec!["src", "main.rs", "Cargo.toml"]
                .into_iter()
                .map(|l| DataRow {
                    cells: vec![StyledText::plain(l)],
                    decoration: Default::default(),
                })
                .collect(),
            selected_idx: Some(1),
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
        let tree = vec![
            node(&[0], 0, Some(true)),
            node(&[0, 0], 1, None),
            node(&[1], 0, None),
        ];
        TreeTable { table, tree }
    }

    /// The acceptance criterion: `TreeTable`'s [`Primitive::layout`] +
    /// [`Primitive::paint`] must produce the exact same verb log as
    /// calling [`native_surface_paint::paint`] directly with the
    /// equivalent arguments.
    #[test]
    fn tree_table_primitive_paint_matches_native_surface_paint() {
        let tt = sample_tree_table();
        let rect = Rect::new(10.0, 20.0, 200.0, 80.0);
        let ctx = LayoutCtx {
            line_height: 16.0,
            char_width: 8.0,
            nerd_fonts_enabled: true,
        };
        let theme = Theme::default();
        let style = Style::default();

        let via_primitive_layout = Primitive::layout(&tt, &ctx, rect);
        let mut via_primitive_surface = LogSurface::default();
        Primitive::paint(
            &tt,
            &via_primitive_layout,
            &mut via_primitive_surface,
            &theme,
            &style,
        );

        let state = DataTablePaintState::default();
        let mut direct_surface = LogSurface::default();
        native_surface_paint::paint(
            &tt,
            &mut direct_surface,
            &theme,
            rect,
            ctx.line_height,
            &state,
            ctx.nerd_fonts_enabled,
        );

        assert!(
            !via_primitive_surface.calls.is_empty(),
            "the tree table must paint something for this comparison to be meaningful"
        );
        assert_eq!(via_primitive_surface.calls, direct_surface.calls);

        // Not just the same paint verbs: the exact geometry `Primitive::layout`
        // resolved (and handed to a caller to hit-test against) must equal
        // what the shared pixel resolver returns for this surface's own
        // metrics — catches a `Primitive::layout` that estimated a glyph
        // width (e.g. the chevron) differently than the measurer `paint`
        // actually used, even though both still painted identical verbs.
        let resolver_layout = crate::primitives::layout_metrics::pixel_tree_table_layout(
            &tt,
            rect.width,
            rect.height,
            ctx.line_height,
            &crate::primitives::data_table::native_surface_paint::SurfaceTextMeasure(
                &LogSurface::default(),
            ),
        );
        assert_eq!(via_primitive_layout.layout, resolver_layout);
    }

    /// [`TreeTablePrimitiveLayout::hit_test`] subtracts the area's
    /// origin before delegating — a click at the layout's absolute
    /// origin must resolve to the same hit as `(0, 0)` on the
    /// table-local [`TreeTableLayout::hit_test`].
    #[test]
    fn primitive_layout_hit_test_is_relative_to_area_origin() {
        let tt = sample_tree_table();
        let rect = Rect::new(10.0, 20.0, 200.0, 80.0);
        let ctx = LayoutCtx {
            line_height: 16.0,
            char_width: 8.0,
            nerd_fonts_enabled: true,
        };

        let layout = Primitive::layout(&tt, &ctx, rect);
        let local = layout.layout.hit_test(0.0, 0.0, 0, tt.table.rows.len());
        let absolute = layout.hit_test(rect.x, rect.y);
        assert_eq!(local, absolute);
    }
}
