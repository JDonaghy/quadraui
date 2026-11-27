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
//! # Controlled widget
//!
//! Everything visible is descriptor content the app owns: the flattened
//! rows (including lazily loaded children and the placeholder row that
//! stands in for them while they load), each branch's expand state, the
//! selection, the scroll offset and the sort. The widget's
//! [`crate::interactive::Interactive`] impl reports every change as a
//! [`TreeTableEvent`]; the app applies it and hands back the next
//! descriptor. [`TreeTableState`] holds only gesture data.
//!
//! A branch whose children the app has not supplied yet is marked
//! [`TreeTableNode::children_pending`]; expanding it reports
//! [`TreeTableEvent::ChildrenRequested`] once, until the app answers by
//! clearing the flag. A header click reports
//! [`TreeTableEvent::SortChanged`]; the app re-orders the rows (keeping
//! each level's children together) and sets `table.sort`.
//!
//! Layout and paint compose [`DataTable`]'s own: the header, body rows,
//! scrollbars and footer are `DataTable`'s, and column 0 of each row is
//! the shared tree cell (`crate::primitives::tree::tree_cell`). Pointer
//! gestures that are not tree-specific are forwarded to `DataTable`'s own
//! `Interactive` impl.

use crate::behaviour::nav::NavRow;
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

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
mod contract;
mod layout;
#[cfg(feature = "tui")]
pub(crate) mod paint_cells;

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub use contract::{TreeTablePrimitiveLayout, TreeTableState, TreeTableSubPart};

/// Per-row tree metadata for one [`TreeTable`] row, parallel to
/// [`DataTable::rows`] on [`TreeTable::table`].
///
/// `#[non_exhaustive]` with [`TreeTableNode::new`] as the constructor, so
/// a future field can land without breaking a downstream struct literal.
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
    /// A branch whose children the app has not supplied yet: expanding
    /// it reports [`TreeTableEvent::ChildrenRequested`] as well as the
    /// expand itself.
    #[serde(default)]
    pub children_pending: bool,
    /// A row standing in for a pending branch's children while they load
    /// (typically a dim `…`): painted like any row, but never selected,
    /// activated or expanded.
    #[serde(default)]
    pub placeholder: bool,
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
            children_pending: false,
            placeholder: false,
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

    /// Mark this branch's children as not supplied yet — see
    /// [`Self::children_pending`].
    #[must_use]
    pub fn with_children_pending(mut self) -> Self {
        self.children_pending = true;
        self
    }

    /// Mark this row a loading placeholder — see [`Self::placeholder`].
    #[must_use]
    pub fn as_placeholder(mut self) -> Self {
        self.placeholder = true;
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
/// overrides, footer, `bordered` and `striped` all behave exactly as they
/// do on a plain `DataTable`. `tree` only ever adds the column-0 tree cell
/// on top.
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
///     striped: false,
///     glyphs: None,
/// };
/// let tree = vec![
///     TreeTableNode::new(vec![0], 0).with_expanded(true),
///     TreeTableNode::new(vec![0, 0], 1),
/// ];
/// let tree_table = TreeTable::new(table, tree);
///
/// let layout = tree_table.layout(80.0, 10.0, 1.0, 1.0, 1.0, |_| ColumnMeasure::new(20.0), |n| {
///     n.is_expanded.map(|_| (0.0, 1.0))
/// }, 0.0);
/// assert_eq!(
///     layout.hit_test(0.0, 1.0, 0, tree_table.table.rows.len()),
///     TreeTableHit::Chevron(0)
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
    /// `j`/`k` move the selection like Down/Up (while no Ctrl/Alt/Cmd is
    /// held), instead of starting a type-to-jump search.
    #[serde(default)]
    pub vim_keys: bool,
}

impl TreeTable {
    /// `table` with per-row tree shape `tree`, vim keys off.
    #[must_use]
    pub fn new(table: DataTable, tree: Vec<TreeTableNode>) -> Self {
        Self {
            table,
            tree,
            vim_keys: false,
        }
    }

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

    /// Whether row `idx` is a loading placeholder.
    pub(crate) fn is_placeholder(&self, idx: usize) -> bool {
        self.tree.get(idx).is_some_and(|n| n.placeholder)
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
    /// width)` — both relative to column 0's left edge, in the same
    /// content-space unit [`crate::ResolvedColumn::x`] uses — for a row
    /// whose chevron paints, or `None` for a leaf or a row whose chevron
    /// is hidden. A chevron is clipped to column 0's resolved width, the
    /// same clip the tree cell paints with, so a click past the visible
    /// part of a clipped chevron is a row click.
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
                if start >= col0.width {
                    continue;
                }
                let width = width.min(col0.width - start);
                let y = table.inset_y + table.header_height + i as f32 * table.row_height;
                chevrons.push((abs, Rect::new(col0.x + start, y, width, table.row_height)));
            }
        }
        TreeTableLayout { table, chevrons }
    }
}

/// Hit-test result for a `TreeTable`: the expand/collapse chevron, or
/// whatever [`DataTableHit`] resolves everywhere else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeTableHit {
    /// Row `idx`'s expand/collapse chevron.
    Chevron(usize),
    /// Anything the wrapped [`DataTable`] resolves: a header, a header
    /// divider, a body row outside its chevron, the footer, the
    /// scrollbar, or empty space.
    Table(DataTableHit),
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
    /// shifts the raw pointer position into this same content space via
    /// [`DataTableLayout::content_x`] before comparing against these
    /// bounds, rather than shifting the bounds themselves. Sparse: only
    /// branch rows with a chevron get an entry.
    pub chevrons: Vec<(usize, Rect)>,
}

impl TreeTableLayout {
    /// Resolve a click to a chevron, or to whatever
    /// [`DataTableLayout::hit_test`] resolves it to. Chevron bounds are
    /// checked first; `x`/`y` are viewport-relative, as a caller's raw
    /// pointer position.
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
                return TreeTableHit::Chevron(*idx);
            }
        }
        TreeTableHit::Table(self.table.hit_test(x, y, scroll_offset, total_rows))
    }

    /// The event a [`TreeTableHit::Chevron`] resolution should emit:
    /// `tree[idx]`'s current [`TreeTableNode::is_expanded`], flipped.
    /// `None` when `idx` has no entry in `tree` or that entry is a leaf
    /// (`is_expanded: None`) — the same "degrade without panicking" rule
    /// [`TreeTable::layout`] already applies to a short `tree`.
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

/// Events a `TreeTable` emits back to the app. Row events carry the
/// row's [`TreePath`] alongside its flat index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TreeTableEvent {
    /// A header click asked for a sort by `col`: ascending, unless the
    /// table is already sorted ascending by `col`. The app re-orders each
    /// level's children and sets `table.sort`.
    SortChanged { col: usize, ascending: bool },
    /// Enter (or the assistive "activate" action) on the selected row.
    RowActivated { idx: usize, path: TreePath },
    /// User selected a row — arrow-key navigation or a plain single click.
    RowSelected { idx: usize, path: TreePath },
    /// User double-clicked a row.
    RowDoubleClicked { idx: usize, path: TreePath },
    /// User scrolled the table.
    Scroll { delta: i32, modifiers: Modifiers },
    /// The widget scrolled — a wheel, a scrollbar gesture, or a move that
    /// brought the selection into view. The app stores `scroll_offset`
    /// as `table.scroll_offset`.
    ScrollChanged { scroll_offset: usize },
    /// User dragged a column divider to resize.
    ColumnResized { col: usize, width: f32 },
    /// The full multi-selection changed. `indices`/`paths` are the
    /// complete, sorted, de-duplicated selection, parallel to each other.
    SelectionSet {
        indices: Vec<usize>,
        paths: Vec<TreePath>,
    },
    /// The chevron was clicked, or Left/Right/Space resolved to an
    /// expand/collapse. `expand: true` means the app should set this
    /// row's [`TreeTableNode::is_expanded`] to `Some(true)` and show its
    /// children (or a placeholder while they load); `false` means
    /// collapse.
    ToggleExpanded { path: TreePath, expand: bool },
    /// Expanding a [`TreeTableNode::children_pending`] branch: the app
    /// should load its children and supply them, clearing the flag.
    /// Reported once per branch until the flag clears.
    ChildrenRequested { idx: usize, path: TreePath },
    /// The user asked for row `idx`'s context menu (right-click);
    /// `position` is where, in the coordinates the table was laid out in.
    ContextMenuRequested {
        idx: usize,
        path: TreePath,
        position: crate::event::Point,
    },
}

impl TreeTableEvent {
    /// Map [`crate::behaviour::nav::HorizontalNav`]'s branch-toggle
    /// variants — the Left/Right-on-the-keyboard counterpart of
    /// [`TreeTableLayout::chevron_toggle`]'s mouse path — to
    /// [`Self::ToggleExpanded`]. `None` for
    /// `SelectParent`/`SelectFirstChild`/`None`, which are selection
    /// moves rather than an expand/collapse.
    #[must_use]
    pub fn from_horizontal_nav(nav: crate::behaviour::nav::HorizontalNav) -> Option<Self> {
        use crate::behaviour::nav::HorizontalNav;
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
// (`tree::tree_cell::paint_tree_cell`) followed by
// its label through the same `paint_cell` every other column uses. No
// parallel row, column or scrollbar logic of its own.
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
    use crate::primitives::tree::tree_cell::{paint_tree_cell, tree_indent_px};
    use crate::primitives::tree::tree_chevron_glyph;
    use crate::theme::Theme;
    use crate::types::{Decoration, TreeStyle};

    /// Resolve the layout [`paint`] paints `table` at, without touching a
    /// buffer — the pixel resolver every GUI backend's
    /// `Backend::tree_table_layout` forwards to.
    pub(crate) fn pixel_tree_table_layout(
        table: &TreeTable,
        w: f32,
        h: f32,
        line_height: f32,
        measure: &dyn TextMeasure,
    ) -> TreeTableLayout {
        super::layout::pixel_layout(table, w, h, line_height, measure)
    }

    /// Paint a [`TreeTable`] into `rect` on `surface` and return the
    /// resolved [`TreeTableLayout`] — the entry point every GUI backend's
    /// `draw_tree_table` goes through. Resolves the layout through
    /// `surface`'s own text measurer, then paints it with
    /// [`paint_laid_out`].
    pub(crate) fn paint(
        table: &TreeTable,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        rect: Rect,
        line_height: f32,
        state: &DataTablePaintState,
        nerd_fonts_enabled: bool,
    ) -> TreeTableLayout {
        let tt_layout = {
            let measure = SurfaceTextMeasure(&*surface);
            pixel_tree_table_layout(table, rect.width, rect.height, line_height, &measure)
        };
        paint_laid_out(
            table,
            surface,
            theme,
            rect,
            line_height,
            state,
            nerd_fonts_enabled,
            tt_layout,
        )
    }

    /// Paint `table` at an already-resolved `tt_layout`: exactly what
    /// [`crate::primitives::data_table::native_surface_paint::paint_laid_out`]
    /// paints, except column 0 of every row with a `table.tree` entry is
    /// indent + chevron + icon via [`paint_tree_cell`], followed by its
    /// label through the same [`paint_cell`] every other column uses.
    /// Rows past `table.tree`'s length paint as a plain [`crate::DataTable`]
    /// row.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint_laid_out(
        table: &TreeTable,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        rect: Rect,
        line_height: f32,
        state: &DataTablePaintState,
        nerd_fonts_enabled: bool,
        tt_layout: TreeTableLayout,
    ) -> TreeTableLayout {
        let style = TreeStyle::default();
        let ipx = tree_indent_px(line_height);
        let leaf_gap = line_height * 0.8;
        let dt_layout = paint_with_layout(
            &table.table,
            surface,
            theme,
            rect,
            line_height,
            state,
            None,
            nerd_fonts_enabled,
            tt_layout.table,
            |surface, ctx, row_y, idx, hovered, striped| match table.tree.get(idx) {
                Some(node) => paint_tree_table_row(
                    surface,
                    ctx,
                    row_y,
                    idx,
                    node,
                    (hovered, striped),
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

    /// Paint one `TreeTable` body row: [`paint_row`]'s selection, hover and
    /// stripe tint and every column but 0 exactly as [`paint_cell`]
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
        (hovered_idx, striped): (Option<usize>, bool),
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
            striped,
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
                striped: false,
                glyphs: None,
            }
        }

        /// A branch row paints its chevron glyph, then the label offset
        /// past it — the same ordering the cell painter
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
        /// [`crate::primitives::tree::tree_cell`]'s
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::data_table::{ColumnAlign, ColumnWidth, DataRow};
    use crate::types::{StyledText, WidgetId};
    use crate::DataTableHit;

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
            striped: false,
            glyphs: None,
        }
    }

    /// Each body row reports its tree level and expand state alongside
    /// the wrapped table's row semantics and cells.
    #[test]
    fn a11y_rows_carry_tree_level_and_expand_state() {
        use crate::a11y::{A11yPrimitive, A11yRole};

        let mut dt = table(3);
        dt.selected_idx = Some(1);
        let tree = vec![
            node(&[0], 0, Some(true)),
            node(&[0, 0], 1, None),
            node(&[1], 0, Some(false)),
        ];
        let tt = TreeTable::new(dt.clone(), tree);
        let children = tt.a11y_children();
        // The header row, then the three body rows.
        assert_eq!(children.len(), 4);
        let row0 = &children[1].info;
        assert_eq!(row0.a11y_role, Some(A11yRole::Row));
        assert_eq!(row0.level, Some(1));
        assert_eq!(row0.expanded, Some(true));
        assert_eq!((row0.position_in_set, row0.set_size), (Some(1), Some(2)));
        let row1 = &children[2].info;
        assert_eq!(row1.level, Some(2));
        assert_eq!(row1.expanded, None);
        assert_eq!(row1.selected, Some(true));
        assert_eq!(children[3].info.expanded, Some(false));
        assert_eq!(children[2].children.len(), 2, "cells stay row children");
        assert_eq!(tt.a11y_info(), dt.a11y_info());
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
        let tt = TreeTable::new(dt.clone(), tree);
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
        let tt = TreeTable::new(dt, tree);
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
        assert_eq!(layout.hit_test(0.0, 1.0, 0, 3), TreeTableHit::Chevron(0));
        // Just past the chevron, same row, resolves to a plain Row hit.
        assert_eq!(
            layout.hit_test(2.0, 1.0, 0, 3),
            TreeTableHit::Table(DataTableHit::Row { idx: 0 })
        );
        // Row 1 is a leaf (no chevron measured) — always a Row hit.
        assert_eq!(
            layout.hit_test(0.0, 2.0, 0, 3),
            TreeTableHit::Table(DataTableHit::Row { idx: 1 })
        );
        // Header row still resolves normally.
        assert_eq!(
            layout.hit_test(0.0, 0.0, 0, 3),
            TreeTableHit::Table(DataTableHit::Header { col: 0 })
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
        let tt = TreeTable::new(dt, tree);
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
        // `tree` is shorter than `table.rows`.
        let tt = TreeTable::new(dt, vec![node(&[0], 0, Some(true))]);
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

    #[test]
    fn a_chevron_is_clipped_to_column_0_and_dropped_past_it() {
        let dt = table(3);
        let tree = vec![
            node(&[0], 0, Some(true)),
            node(&[0, 0], 1, Some(true)),
            node(&[0, 0, 0], 9, Some(true)),
        ];
        let mut tt = TreeTable::new(dt, tree);
        tt.table.columns.truncate(1);
        tt.table.rows.iter_mut().for_each(|r| r.cells.truncate(1));
        let measure = |col: &Column| ColumnMeasure::new(col.title.chars().count() as f32);
        // Column 0 resolves 4 wide; each indent level is 2 and the
        // chevron 3, so row 0's chevron fits whole, row 1's is clipped to
        // the 2 units left and row 2's starts past the column.
        let layout = tt.layout(
            4.0,
            10.0,
            1.0,
            1.0,
            0.0,
            measure,
            |n| n.is_expanded.map(|_| (n.indent as f32 * 2.0, 3.0)),
            0.0,
        );
        let chevrons: Vec<(usize, f32)> =
            layout.chevrons.iter().map(|(i, r)| (*i, r.width)).collect();
        assert_eq!(chevrons, vec![(0, 3.0), (1, 2.0)]);
        assert_eq!(layout.hit_test(3.0, 2.0, 0, 3), TreeTableHit::Chevron(1));
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
            striped: false,
            glyphs: None,
        };
        let tree: Vec<TreeTableNode> = (0..3).map(|i| node(&[i as u16], 0, None)).collect();
        TreeTable::new(dt, tree)
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
            self.calls.push(format!("surface_draw_image {rect:?}"));
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
            striped: false,
            glyphs: None,
        };
        let tree = vec![
            node(&[0], 0, Some(true)),
            node(&[0, 0], 1, None),
            node(&[1], 0, None),
        ];
        TreeTable::new(table, tree)
    }

    /// The acceptance criterion: `TreeTable`'s [`Primitive::layout`] +
    /// [`Primitive::paint`] must produce the exact same verb log as
    /// calling [`native_surface_paint::paint`] directly with the
    /// equivalent arguments.
    #[test]
    fn tree_table_primitive_paint_matches_native_surface_paint() {
        let tt = sample_tree_table();
        let rect = Rect::new(10.0, 20.0, 200.0, 80.0);
        let ctx = LayoutCtx::new(16.0, 8.0, true);
        let theme = Theme::default();
        let style = Style::default();

        let via_primitive_layout = Primitive::layout(&tt, &ctx, rect);
        let mut via_primitive_surface = LogSurface::default();
        Primitive::paint(
            &tt,
            &via_primitive_layout,
            &mut via_primitive_surface,
            &crate::PaintEnv::new(&theme, &style),
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
        let ctx = LayoutCtx::new(16.0, 8.0, true);

        let layout = Primitive::layout(&tt, &ctx, rect);
        let local = layout.layout.hit_test(0.0, 0.0, 0, tt.table.rows.len());
        let absolute = layout.hit_test(rect.x, rect.y);
        assert_eq!(local, absolute);
    }
}
