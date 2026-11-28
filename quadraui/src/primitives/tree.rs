//! `TreeView` primitive: hierarchical rows with expand/collapse, optional
//! icons, styled text, badges, and keyboard-driven selection.
//!
//! Trees are pre-flattened by the app: each `TreeRow` carries its
//! `TreePath`, visual `indent`, and an `is_expanded` flag (`None` for
//! leaves). Backends iterate `rows` in order; the primitive does not store
//! tree structure of its own. This keeps the data model plain and
//! plugin-friendly while letting apps control exactly which rows are
//! visible at any given frame.
//!
//! # Backend contract
//!
//! **Purely declarative** — render `rows[scroll_offset..]` until the
//! viewport is full. Click → row index → emit `TreeEvent::RowActivated`
//! with the row's `path`. Keyboard navigation (`j`/`k`/`h`/`l`/`Enter`)
//! emits the corresponding event; the *app* updates `selected_path` and
//! `scroll_offset` for the next frame.
//!
//! No measurement-dependent state — backends pick a uniform row height
//! (often `line_height` for leaves, `line_height * 1.4` for branches in
//! GUI backends, exactly `1` cell for TUI). Per-backend row cadence is
//! allowed; the primitive only constrains data shape.
//! [`TreeStyle::row_height`](crate::types::TreeStyle::row_height) lets a
//! host pin that cadence to a fixed value instead of letting it float
//! with the editor font — see #623.
//!
//! Apps that need "scroll selection into view" do it themselves by
//! adjusting `scroll_offset` based on the selected row's flat index and
//! the viewport row count.

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
use crate::behaviour::nav::NavOutcome;
use crate::event::Rect;
use crate::primitives::row_stack::{self, RowPaintState};
use crate::primitives::scrollbar::Scrollbar;
use crate::types::{
    Badge, Decoration, Icon, Modifiers, SelectionMode, StyledText, TreePath, TreeStyle, WidgetId,
};
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
use crate::Handled;
use serde::{Deserialize, Serialize};

/// Declarative description of a `TreeView` widget.
///
/// # Examples
///
/// ```
/// use quadraui::{
///     SelectionMode, StyledText, TreeRow, TreeRowMeasure, TreeStyle, TreeView, TreeViewHit,
///     WidgetId,
/// };
///
/// let tree = TreeView {
///     id: WidgetId::new("tree:explorer"),
///     rows: vec![TreeRow {
///         path: vec![0],
///         indent: 0,
///         icon: None,
///         text: StyledText::plain("src/"),
///         badge: None,
///         is_expanded: Some(true),
///         decoration: Default::default(),
///         edit: None,
///     }],
///     selection_mode: SelectionMode::Single,
///     selected_path: Some(vec![0]),
///     scroll_offset: 0,
///     items_window: None,
///     scroll_px: None,
///     style: TreeStyle::default(),
///     has_focus: true,
/// };
///
/// let layout = tree.layout(40.0, 10.0, |_| TreeRowMeasure::new(1.0));
/// assert_eq!(layout.hit_test(0.0, 0.0), TreeViewHit::Row(0));
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TreeView {
    pub id: WidgetId,
    /// Pre-flattened, pre-expanded rows in visual order.
    pub rows: Vec<TreeRow>,
    pub selection_mode: SelectionMode,
    pub selected_path: Option<TreePath>,
    /// How many rows have been scrolled past (app-owned in v1; primitive-owned
    /// scroll state with `ScrollState::id(widget_id)` is a later stage per
    /// `docs/UI_CRATE_DESIGN.md` §3.1).
    #[serde(default)]
    pub scroll_offset: usize,
    /// Where the loaded rows sit in the real collection: when set, the
    /// payload holds rows `start..` of `total`, and `scroll_offset`, the selection and
    /// every row index in this widget's events are absolute indices into
    /// those `total` rows.
    ///
    /// `None`: the payload is the whole collection. A windowed widget asks
    /// for rows it scrolls to with [`crate::UiEvent::RowsRequested`], after
    /// input only, so the app sends the rows its first viewport shows (and
    /// any a larger viewport shows after a resize) without being asked;
    /// unloaded rows paint as placeholders. See
    /// [`crate::behaviour::row_window`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items_window: Option<crate::behaviour::row_window::ItemsWindow>,
    /// Scroll position finer than a whole row, in the surface's native
    /// units — pixels on GTK, macOS and Windows, rows on TUI (where 1.0 is
    /// one row); when set it wins over `scroll_offset`.
    ///
    /// A caller's [`crate::primitive::PaintState::scroll_px`] wins over
    /// this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll_px: Option<f32>,
    pub style: TreeStyle,
    /// Whether this tree currently holds keyboard focus. Gates the
    /// focused row's highlight (`Theme::selected_bg` vs. the unfocused
    /// `Theme::inactive_selected_bg`).
    ///
    /// Deliberately **not** the same "host pre-composes
    /// `ring.is_focused(id) && ring.is_focus_visible()`" convention
    /// [`crate::Button::focused`]/[`crate::TextInput::has_focus`]
    /// follow — see [`crate::ListView::has_focus`]'s matching doc for
    /// why a focus-visible ring can't ride on this field. The GUI
    /// focus-visible ring instead reads the paint state's
    /// [`crate::primitive::PaintState::focus_ring_visible`].
    #[serde(default)]
    pub has_focus: bool,
}

/// One visible row in a `TreeView`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TreeRow {
    pub path: TreePath,
    /// Visual indent level in `style.indent` units. Usually equals
    /// `path.len() - 1` but apps may flatten (e.g. show a child as indent 0
    /// when rendering a subtree in isolation).
    pub indent: u16,
    pub icon: Option<Icon>,
    pub text: StyledText,
    /// Right-aligned status indicator (e.g. git status letter, item count).
    pub badge: Option<Badge>,
    /// `None` marks a leaf; `Some(true)` marks an expanded branch;
    /// `Some(false)` marks a collapsed branch.
    pub is_expanded: Option<bool>,
    #[serde(default)]
    pub decoration: Decoration,
    /// When `Some`, backends render an inline text input in place of
    /// `text` and `badge`. The row's indent, icon, and chevron are
    /// still rendered normally.
    #[serde(default)]
    pub edit: Option<TreeRowEditState>,
}

/// Inline editing state for a tree row. When present on a `TreeRow`,
/// backends render a text input in place of the normal row label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TreeRowEditState {
    pub text: String,
    /// Cursor position as a byte offset into `text`.
    pub cursor: usize,
    /// Selection anchor as a byte offset. When `Some(n)` and `n != cursor`,
    /// the range between anchor and cursor is selected.
    pub selection_anchor: Option<usize>,
    /// Shown in muted style when `text` is empty (e.g. "New file name...").
    #[serde(default)]
    pub placeholder: Option<String>,
}

// ── D6 Layout API ───────────────────────────────────────────────────────────
//
// Per Decision D6 in `docs/decisions/BACKEND_TRAIT_PROPOSAL.md` §9: primitives return
// fully-resolved `Layout` structs; backends rasterise verbatim. Third
// primitive to gain the new shape after `TabBar` and `StatusBar`. TreeView
// is purely vertical — rows stack from `scroll_offset` until the viewport
// fills. Sub-row layout (chevron / icon / text / badge positions within a
// row) is still backend-owned in v1 because each backend has native
// conventions for those elements (see the A.1c lesson in PLAN.md: "When
// porting a primitive's draw function to a new backend, match the new
// backend's pre-migration row cadence, not the other backend's").

/// Per-row measurement supplied by the backend.
///
/// `height` is the row's height in the backend's native unit — 1 cell for
/// TUI, `line_height` or `line_height * 1.4` for GTK (leaves vs branches),
/// similar for other native backends.
///
/// `chevron_end_x` — when `Some(w)` and the row has `is_expanded.is_some()`,
/// [`TreeView::layout`] splits the row's hit region into a
/// [`TreeViewHit::Chevron`] zone for `x ∈ [0, w)` and a
/// [`TreeViewHit::Row`] zone for the remainder. Backends set this to the
/// x coordinate (in tree-local units) where the painted chevron ends.
/// `None` means no chevron split (leaf rows, or `show_chevrons = false`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TreeRowMeasure {
    pub height: f32,
    pub chevron_end_x: Option<f32>,
}

impl TreeRowMeasure {
    pub fn new(height: f32) -> Self {
        Self {
            height,
            chevron_end_x: None,
        }
    }

    /// Convenience constructor: row with an explicit chevron boundary.
    pub fn with_chevron(height: f32, chevron_end_x: f32) -> Self {
        Self {
            height,
            chevron_end_x: Some(chevron_end_x),
        }
    }
}

/// Resolved position of one visible tree row after layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleTreeRow {
    /// Index into the original `TreeView.rows` Vec (absolute, not visible).
    pub row_idx: usize,
    /// Full row bounds. `height` is clipped to the viewport if the row
    /// would extend past the bottom edge.
    pub bounds: Rect,
}

/// Classification of a hit-test result.
///
/// When [`TreeRowMeasure::chevron_end_x`] is set for a branch row the layout
/// emits two adjacent hit regions for that row: a [`Chevron`] region on the
/// left (covering the painted expand/collapse glyph) and a [`Row`] region for
/// the remainder. Backends that do not distinguish the two leave
/// `chevron_end_x` as `None`; clicking anywhere on the row returns `Row`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeViewHit {
    /// Click landed on the body portion of a row.
    /// Carries the `row_idx` into `TreeView.rows`.
    Row(usize),
    /// Click landed on the expand/collapse chevron of a branch row.
    /// Carries the same `row_idx` as the companion `Row` region.
    Chevron(usize),
    /// Click landed in the viewport's empty region (below the last row).
    Empty,
}

/// Fully-resolved tree-view layout. Backends iterate `visible_rows` for
/// painting and call [`Self::hit_test`] for clicks.
///
/// `#[non_exhaustive]`: a backend-returned value, never constructed by a
/// consumer via struct literal (same convention as the sibling
/// [`crate::DataTableLayout`]), so a future field add here is additive —
/// no `cargo-semver-checks` `constructible_struct_adds_field` finding.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct TreeViewLayout {
    /// Viewport width in the measurer's unit.
    pub viewport_width: f32,
    /// Viewport height in the measurer's unit.
    pub viewport_height: f32,
    /// Rows that are at least partially visible, top to bottom.
    pub visible_rows: Vec<VisibleTreeRow>,
    /// Ordered hit-region list. One region per visible row.
    pub hit_regions: Vec<(Rect, TreeViewHit)>,
    /// Scroll offset actually used. Clamped to `[0, rows.len())` so the
    /// backend never iterates past the end of the row slice.
    pub resolved_scroll_offset: usize,
    /// Sub-row pixel remainder when a non-`None` `scroll_px` (see
    /// [`TreeView::layout_with_scroll_px`]) is in effect — see
    /// [`crate::primitives::scrollbar::resolve_scroll_px`]. `0.0` from
    /// plain [`TreeView::layout`], so a TUI rasteriser that never reads
    /// this field keeps the pre-existing whole-row behaviour. A GUI
    /// painter shifts the first visible row's top up by this many units
    /// (relying on the surface's own clip to show only the portion still
    /// inside the viewport), producing the "partial first row clipped"
    /// look of pixel-smooth scrolling.
    pub scroll_sub_px: f32,
}

/// The chevron glyph a pixel backend's shared paint path
/// (`native_surface_paint::paint`, below) actually draws for a row at
/// the given expand state, given `style`.
///
/// An app-supplied `chevron_expanded`/`chevron_collapsed` override is
/// honoured verbatim; only the *default* `▾`/`▸` strings are swapped
/// for the sharper codicon glyph (`crate::codicon::CHEVRON_DOWN` /
/// `CHEVRON_RIGHT`) — a codicon-patched font can't be assumed on a
/// terminal cell grid, so TUI's own paint path
/// (`TreeView`'s cell paint) always reads the two fields directly and
/// never calls this.
///
/// [`crate::primitives::layout_metrics::pixel_tree_layout_measured`] calls
/// this too, so the glyph it measures is the one pixel paint actually
/// substitutes — keeping layout and paint in lockstep for the default
/// style, not just for app overrides.
///
/// Gated to the same feature set as `crate::codicon`: GTK/win/macos are
/// the only backends that ever substitute a codicon in the first place.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub(crate) fn tree_chevron_glyph(style: &TreeStyle, expanded: bool) -> std::borrow::Cow<'_, str> {
    let default_style = TreeStyle::default();
    if expanded {
        if style.chevron_expanded == default_style.chevron_expanded {
            std::borrow::Cow::Owned(crate::codicon::CHEVRON_DOWN.to_string())
        } else {
            std::borrow::Cow::Borrowed(style.chevron_expanded.as_str())
        }
    } else if style.chevron_collapsed == default_style.chevron_collapsed {
        std::borrow::Cow::Owned(crate::codicon::CHEVRON_RIGHT.to_string())
    } else {
        std::borrow::Cow::Borrowed(style.chevron_collapsed.as_str())
    }
}

/// `row_padding_x + indent * indent_px + glyph_w + 4.0` — the x offset,
/// relative to a row's own left edge, immediately past a painted
/// chevron glyph `glyph_w` wide and its trailing 4px gap.
///
/// Not gated to any backend feature set (unlike [`tree_chevron_glyph`])
/// because [`crate::primitives::layout_metrics::tree_layout_inner`]
/// calls it unconditionally — on `tui`-only builds it still needs this
/// formula for its glyph-width-estimate fallback. The pixel painter
/// (`native_surface_paint::tree_cell::paint_tree_cell`) calls it too,
/// so a `TreeViewHit::Chevron` region computed from this function's
/// result and the rect that painter actually draws into can never
/// drift apart.
pub(crate) fn chevron_end_x(row_padding_x: f32, indent: u16, indent_px: f32, glyph_w: f32) -> f32 {
    row_padding_x + indent as f32 * indent_px + glyph_w + 4.0
}

impl TreeViewLayout {
    /// Test which row (if any) contains point `(x, y)`. Returns
    /// `TreeViewHit::Empty` when the point is below the last visible row.
    pub fn hit_test(&self, x: f32, y: f32) -> TreeViewHit {
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        TreeViewHit::Empty
    }
}

impl TreeView {
    /// Compute the full rendering + hit-test layout for this tree.
    ///
    /// Per D6: layout decisions live here; backends consume the returned
    /// `TreeViewLayout` verbatim — iterate `visible_rows` for painting;
    /// call `hit_test` for clicks. Sub-row elements (chevron, icon, text,
    /// badge) are still backend-owned in v1 because their positions
    /// depend heavily on native conventions (TUI char cells vs GTK Pango
    /// pixel metrics).
    ///
    /// # Arguments
    ///
    /// - `viewport_width`, `viewport_height` — available area in the
    ///   measurer's unit.
    /// - `measure_row(i)` — height for row `i` (index into `self.rows`).
    ///   Receives the row index (not the row itself) so backends can
    ///   vary height by decoration, indent, or other row state they know
    ///   about via their copy of `self.rows`.
    ///
    /// # Row clipping
    ///
    /// The last visible row's `bounds.height` is clipped to whatever
    /// fits inside the viewport. Backends that want to skip partially-
    /// visible rows can check `row.bounds.height < measure_row(row.row_idx).height`.
    pub fn layout<F>(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        measure_row: F,
    ) -> TreeViewLayout
    where
        F: Fn(usize) -> TreeRowMeasure,
    {
        self.layout_with_scroll_px(viewport_width, viewport_height, None, measure_row)
    }

    /// Like [`Self::layout`], but with an explicit pixel-precise scroll
    /// position (`scroll_px`) a GUI painter has accumulated in a
    /// [`TreePaintState`], instead of only [`TreeView::scroll_offset`]'s
    /// whole-row value.
    ///
    /// `scroll_px` wins over `scroll_offset` when it is `Some` and usable
    /// — see [`crate::primitives::scrollbar::resolve_scroll_px`] — and
    /// the sub-row remainder is reported back as
    /// [`TreeViewLayout::scroll_sub_px`] for the painter to clip the
    /// first visible row against. `None` (what [`Self::layout`] passes)
    /// resolves to exactly the pre-existing whole-row behaviour.
    pub fn layout_with_scroll_px<F>(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        scroll_px: Option<f32>,
        measure_row: F,
    ) -> TreeViewLayout
    where
        F: Fn(usize) -> TreeRowMeasure,
    {
        let mut hit_regions: Vec<(Rect, TreeViewHit)> = Vec::new();

        // Border insets: 1 cell/pixel on each side when `style.bordered`
        // is set, reserving room for a decorative frame (e.g. a focus
        // ring) so it never overwrites row content — mirrors
        // `ListView::layout`'s `bordered` handling.
        let (inset_x, inset_y, row_w, rows_h_max) = if self.style.bordered {
            let iw = (viewport_width - 2.0).max(0.0);
            let ih = (viewport_height - 2.0).max(0.0);
            (1.0, 1.0, iw, ih)
        } else {
            (0.0, 0.0, viewport_width, viewport_height)
        };

        let rows_y_end = inset_y + rows_h_max;
        // Per-row extra data is the chevron boundary (if any) — carried
        // alongside the height so the hit-region split below doesn't
        // need to re-measure the row.
        let stack = row_stack::layout_row_stack(
            self.scroll_offset,
            scroll_px,
            self.rows.len(),
            inset_x,
            inset_y,
            rows_y_end,
            row_w,
            |i| {
                let m = measure_row(i);
                (m.height, m.chevron_end_x)
            },
        );

        let visible_rows: Vec<VisibleTreeRow> = stack
            .visible
            .iter()
            .map(|r| VisibleTreeRow {
                row_idx: r.idx,
                bounds: r.bounds,
            })
            .collect();

        for r in &stack.visible {
            let i = r.idx;
            let bounds = r.bounds;
            // Split the hit region into Chevron + Row when the backend
            // supplied a chevron boundary for this branch row.
            let row = &self.rows[i];
            if let (Some(_), Some(chev_x)) = (row.is_expanded, r.extra) {
                let chev_x = chev_x.clamp(0.0, row_w);
                if chev_x > 0.0 {
                    hit_regions.push((
                        Rect::new(bounds.x, bounds.y, chev_x, bounds.height),
                        TreeViewHit::Chevron(i),
                    ));
                }
                let body_w = (row_w - chev_x).max(0.0);
                if body_w > 0.0 {
                    hit_regions.push((
                        Rect::new(bounds.x + chev_x, bounds.y, body_w, bounds.height),
                        TreeViewHit::Row(i),
                    ));
                }
            } else {
                hit_regions.push((bounds, TreeViewHit::Row(i)));
            }
        }

        TreeViewLayout {
            viewport_width,
            viewport_height,
            visible_rows,
            hit_regions,
            resolved_scroll_offset: stack.resolved_scroll_offset,
            scroll_sub_px: stack.scroll_sub_px,
        }
    }

    /// Vertical scrollbar geometry for this tree rendered into `area`, or
    /// `None` when the tree is empty, all rows fit in the viewport, or
    /// [`crate::types::TreeStyle::show_scrollbar`] is off.
    ///
    /// This is the single source of truth shared by any rasteriser that
    /// paints a tree scrollbar and consumers that hit-test the resolved
    /// `track`/`thumb_start`/`thumb_len` to implement thumb dragging —
    /// mirrors [`ListView::vscrollbar`](crate::primitives::list::ListView::vscrollbar).
    /// The laid-out tree carries it as
    /// [`TreeViewPrimitiveLayout::vscrollbar`].
    ///
    /// Unlike `ListView`, `TreeView` rows can have mixed heights on pixel
    /// backends (`Decoration::Header` rows paint shorter than others —
    /// see [`crate::primitives::layout_metrics::pixel_tree_layout`]). This
    /// method still sizes the thumb against a uniform `row_height`: a
    /// pixel-exact extent would require summing every row's real
    /// measured height every frame just to position a thumb, and the
    /// thumb only needs to be proportional, not a promise of pixel-exact
    /// travel. Unlike `ListView` (whose items really do paint at exactly
    /// `line_height`, with no multiplier), a `TreeView`'s non-header rows
    /// paint at `line_height * 1.4` (or [`crate::types::TreeStyle::row_height`]
    /// when the host set one) — so callers must pass that pitch, not raw
    /// `line_height`, or the computed "does this overflow" answer and the
    /// thumb geometry won't match what [`crate::primitives::layout_metrics::pixel_tree_layout`]
    /// (and therefore the tree's own paint) actually lays out.
    ///
    /// # Arguments
    ///
    /// - `area` — the tree surface rect, in surface-native units (TUI
    ///   cells, GTK / macOS / Windows pixels).
    /// - `row_height` — height of one (non-header) row: `1.0` on TUI,
    ///   or [`crate::primitives::layout_metrics::tree_row_pitch`]'s result
    ///   on pixel backends. Used only as the minimum thumb length, matching
    ///   [`ListView::vscrollbar`](crate::primitives::list::ListView::vscrollbar).
    pub fn vscrollbar(&self, area: Rect, row_height: f32) -> Option<Scrollbar> {
        self.vscrollbar_with_track_width(area, row_height, row_height)
    }

    /// Like [`vscrollbar`](Self::vscrollbar), but with an explicit
    /// `track_width` instead of defaulting it to `row_height`.
    ///
    /// - `track_width` — width of the scrollbar column, occupying the
    ///   rightmost `track_width` of `area`: `1.0` on TUI (one cell), or
    ///   [`crate::style::Style::scrollbar_width`] on pixel backends —
    ///   [`native_surface_paint::paint`] passes `style.scrollbar_width`.
    ///   Deliberately independent of `row_height`: VS Code's list/tree
    ///   scrollbar is a fixed-width column regardless of row pitch.
    pub fn vscrollbar_with_track_width(
        &self,
        area: Rect,
        row_height: f32,
        track_width: f32,
    ) -> Option<Scrollbar> {
        self.vscrollbar_with_scroll_px(area, row_height, track_width, None)
    }

    /// Like [`Self::vscrollbar_with_track_width`], but with an explicit
    /// pixel-precise scroll position (`scroll_px`) from a
    /// [`TreePaintState`] so the thumb tracks the continuous pixel
    /// position (whole row + sub-row fraction) instead of snapping to a
    /// whole row — the scrollbar-geometry counterpart of
    /// [`Self::layout_with_scroll_px`]. `None` resolves to exactly the
    /// pre-existing whole-row behaviour.
    pub fn vscrollbar_with_scroll_px(
        &self,
        area: Rect,
        row_height: f32,
        track_width: f32,
        scroll_px: Option<f32>,
    ) -> Option<Scrollbar> {
        if !self.style.show_scrollbar || row_height <= 0.0 || track_width <= 0.0 {
            return None;
        }
        let total = self.rows.len() as f32;
        if total == 0.0 {
            return None;
        }
        let visible = (area.height / row_height).floor();
        if total <= visible {
            return None;
        }
        let track = Rect::new(
            area.x + area.width - track_width,
            area.y,
            track_width,
            area.height,
        );
        // `scroll_px` wins over `scroll_offset` when set, so the thumb
        // moves smoothly alongside the sub-row paint shift
        // `layout_with_scroll_px` applies.
        Some(row_stack::vscrollbar_thumb(
            self.id,
            track,
            self.scroll_offset,
            scroll_px,
            self.rows.len(),
            row_height,
            visible,
        ))
    }
}

/// Per-frame paint-time state for a `TreeView` — hover/press row
/// tracking plus a pixel-precise scroll position, none of which live on
/// the declarative [`TreeView`] descriptor itself. Field-identical to
/// [`crate::primitives::list::ListPaintState`], so both are aliases of
/// the shared [`RowPaintState`] — see that type's doc for the full
/// rationale and the `scroll_px` resolution rule.
pub type TreePaintState = RowPaintState;

#[cfg(feature = "tui")]
pub(crate) mod paint_cells;

impl crate::behaviour::nav::NavRow for TreeRow {
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

/// Tree cell: indent + chevron + icon, the column-0 content of a
/// [`TreeView`] row and of a [`crate::TreeTable`] row. The geometry here
/// is the one source both widgets, both unit systems and both layout and
/// paint read; the pixel painter sits beside it.
pub(crate) mod tree_cell {
    // The cell-grid geometry and the floor are read only by the backends'
    // own layouts.
    #![cfg_attr(
        not(any(
            feature = "gtk",
            feature = "tui",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )),
        allow(dead_code)
    )]
    use crate::primitives::layout_metrics::TextMeasure;
    use crate::types::TreeStyle;

    /// The unit a tree cell's geometry resolves in.
    #[derive(Clone, Copy)]
    pub(crate) enum TreeCellUnits<'a> {
        /// A cell grid: one indent level is [`TreeStyle::indent`] columns
        /// and a chevron is as wide as its glyph's character count.
        Cells,
        /// Pixels: one indent level is [`tree_indent_px`] (the style's
        /// cell indent does not apply) and a chevron is measured through
        /// `measure`.
        Pixels {
            line_height: f32,
            measure: &'a dyn TextMeasure,
        },
    }

    /// Pixel width of one indent level at `line_height`.
    pub(crate) fn tree_indent_px(line_height: f32) -> f32 {
        (line_height * 0.9).round()
    }

    /// Offset of an `indent`-deep row's chevron from the cell's left edge.
    pub(crate) fn indent_offset(style: &TreeStyle, indent: u16, units: TreeCellUnits) -> f32 {
        match units {
            TreeCellUnits::Cells => indent as f32 * style.indent as f32,
            TreeCellUnits::Pixels { line_height, .. } => {
                indent as f32 * tree_indent_px(line_height)
            }
        }
    }

    /// Width of the expanded or collapsed chevron glyph. On pixels the
    /// real glyph is measured where a codicon substitution exists (the
    /// GUI backends); otherwise it is estimated as `line_height * 0.65`.
    pub(crate) fn chevron_width(style: &TreeStyle, expanded: bool, units: TreeCellUnits) -> f32 {
        let glyph = if expanded {
            &style.chevron_expanded
        } else {
            &style.chevron_collapsed
        };
        match units {
            TreeCellUnits::Cells => glyph.chars().count() as f32,
            TreeCellUnits::Pixels {
                line_height,
                measure,
            } => {
                let _ = (measure, glyph);
                #[cfg(any(
                    feature = "gtk",
                    feature = "win",
                    all(feature = "macos", target_os = "macos")
                ))]
                {
                    let _ = line_height;
                    measure.width_of(&super::tree_chevron_glyph(style, expanded))
                }
                #[cfg(not(any(
                    feature = "gtk",
                    feature = "win",
                    all(feature = "macos", target_os = "macos")
                )))]
                {
                    line_height * 0.65
                }
            }
        }
    }

    /// `(start, width)` of the chevron an `indent`-deep row paints,
    /// relative to the cell's left edge; `None` for a leaf or when the
    /// style hides chevrons.
    pub(crate) fn chevron_extent(
        style: &TreeStyle,
        indent: u16,
        is_expanded: Option<bool>,
        units: TreeCellUnits,
    ) -> Option<(f32, f32)> {
        if !style.show_chevrons {
            return None;
        }
        let expanded = is_expanded?;
        Some((
            indent_offset(style, indent, units),
            chevron_width(style, expanded, units),
        ))
    }

    /// The narrowest a tree cell may get with rows `max_indent` deep:
    /// that indent, the wider of the two chevrons (when shown) and one
    /// label character.
    pub(crate) fn floor_width(style: &TreeStyle, max_indent: u16, units: TreeCellUnits) -> f32 {
        let chevron = if style.show_chevrons {
            chevron_width(style, true, units).max(chevron_width(style, false, units))
        } else {
            0.0
        };
        let one_char = match units {
            TreeCellUnits::Cells => 1.0,
            TreeCellUnits::Pixels { measure, .. } => measure.width_of("0"),
        };
        indent_offset(style, max_indent, units) + chevron + one_char
    }

    #[cfg(any(
        feature = "gtk",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    pub(crate) use paint::*;

    #[cfg(any(
        feature = "gtk",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    #[allow(dead_code)]
    mod paint {
        use crate::paint_surface::PaintSurface;
        use crate::types::{Color, Icon};
        use crate::Rect;

        /// Chevron hit bounds (`None` for a leaf or hidden chevron) plus
        /// the rect left over for the caller's own label.
        pub(crate) struct TreeCellLayout {
            pub chevron_bounds: Option<Rect>,
            pub label_rect: Rect,
        }

        /// Paint indent + chevron (already resolved by the caller, e.g.
        /// via [`crate::primitives::tree::tree_chevron_glyph`]) + icon from
        /// `row_rect.x + row_padding_x`; `leaf_gap` substitutes for a
        /// `None` chevron on a leaf (`is_expanded.is_none()`). Never
        /// paints the label — callers keep their own `StyledText`/badge
        /// logic in `label_rect`.
        #[allow(clippy::too_many_arguments)]
        pub(crate) fn paint_tree_cell(
            surface: &mut dyn PaintSurface,
            row_rect: Rect,
            row_padding_x: f32,
            indent: u16,
            indent_px: f32,
            is_expanded: Option<bool>,
            chevron: Option<&str>,
            leaf_gap: f32,
            icon: Option<&Icon>,
            nerd_fonts_enabled: bool,
            fg: Color,
        ) -> TreeCellLayout {
            let mut cursor_x = row_rect.x + row_padding_x + indent as f32 * indent_px;
            let mut chevron_bounds = None;
            if let Some(glyph) = chevron {
                let (cw, ch) = surface.surface_measure_text(glyph);
                let bounds = Rect::new(cursor_x, row_rect.y + (row_rect.height - ch) / 2.0, cw, ch);
                surface.surface_draw_icon_glyph(bounds, glyph, fg);
                chevron_bounds = Some(bounds);
                // Same formula `layout_metrics::tree_layout_inner` uses to
                // size the `Chevron` hit region for this row.
                cursor_x = row_rect.x
                    + crate::primitives::tree::chevron_end_x(row_padding_x, indent, indent_px, cw);
            } else if is_expanded.is_none() {
                cursor_x += leaf_gap;
            }
            cursor_x = crate::primitives::row_stack::row_content_paint::paint_row_icon(
                surface,
                icon,
                nerd_fonts_enabled,
                fg,
                row_rect.y,
                row_rect.height,
                cursor_x,
                |s, r, g, c| s.surface_draw_text_run(r, g, c),
            );
            // Measured, not `surface_char_width()` — GTK's `CairoSurface`
            // panics on that call (no fixed-width font assumption on a
            // Pango surface).
            let label_w =
                (row_rect.x + row_rect.width - cursor_x).max(surface.surface_measure_text("0").0);
            TreeCellLayout {
                chevron_bounds,
                label_rect: Rect::new(cursor_x, row_rect.y, label_w, row_rect.height),
            }
        }

        #[cfg(test)]
        mod tests {
            use super::*;
            use crate::backend::ImagePaintResult;
            use crate::event::{Point, Viewport};
            use crate::Image;

            /// Minimal `PaintSurface` double: 8-unit-wide chars, 16-unit
            /// lines — matches the mocks every other `native_surface_paint`
            /// test module in this crate uses.
            #[derive(Default)]
            struct StubSurface;

            impl PaintSurface for StubSurface {
                fn surface_begin_frame(&mut self, _viewport: Viewport) {}
                fn surface_end_frame(&mut self) {}
                fn surface_viewport(&self) -> Viewport {
                    Viewport::new(200.0, 100.0, 1.0)
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
                fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
                fn surface_draw_text_run(&mut self, _rect: Rect, _text: &str, _color: Color) {}
                fn surface_draw_line(
                    &mut self,
                    _from: Point,
                    _to: Point,
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

            const ROW: Rect = Rect::new(0.0, 0.0, 200.0, 16.0);

            #[test]
            fn branch_at_indent_2_gets_chevron_bounds() {
                let mut surface = StubSurface;
                let cell = paint_tree_cell(
                    &mut surface,
                    ROW,
                    4.0,
                    2,
                    18.0,
                    Some(true),
                    Some("▾"),
                    0.0,
                    None,
                    false,
                    Color::rgb(0, 0, 0),
                );
                let bounds = cell
                    .chevron_bounds
                    .expect("branch row must get chevron bounds");
                // row_padding_x(4) + indent(2) * indent_px(18) = 40.
                assert_eq!(bounds.x, 40.0);
                assert!(
                    cell.label_rect.x > bounds.x,
                    "label must start right of the chevron"
                );
            }

            #[test]
            fn leaf_gets_no_chevron_bounds() {
                let mut surface = StubSurface;
                let cell = paint_tree_cell(
                    &mut surface,
                    ROW,
                    4.0,
                    0,
                    18.0,
                    None,
                    None,
                    12.0,
                    None,
                    false,
                    Color::rgb(0, 0, 0),
                );
                assert!(
                    cell.chevron_bounds.is_none(),
                    "leaf row must not get chevron bounds"
                );
            }

            #[test]
            fn label_never_shrinks_below_one_char_on_a_narrow_rect() {
                let mut surface = StubSurface;
                // A row barely wider than indent + chevron + icon leaves
                // almost no room for the label.
                let narrow = Rect::new(0.0, 0.0, 30.0, 16.0);
                let cell = paint_tree_cell(
                    &mut surface,
                    narrow,
                    0.0,
                    0,
                    18.0,
                    Some(true),
                    Some("▾"),
                    0.0,
                    Some(&Icon::new("f", "f")),
                    false,
                    Color::rgb(0, 0, 0),
                );
                let (one_char_w, _) = surface.surface_measure_text("0");
                assert!(
                    cell.label_rect.width >= one_char_w,
                    "label_rect must never be narrower than one char, got {}",
                    cell.label_rect.width
                );
            }
        }
    }
}

// Shared `PaintSurface` implementation for rendering rows, chevrons,
// icons, badges, and scrollbars across GTK, macOS, and Windows backends.
// Every backend gets the full inline-rename treatment for `TreeRow::edit`
// (caret, selection highlight, placeholder), and `Decoration::Error` /
// `Warning` map to `theme.error_fg` / `warning_fg`, matching `ListView`.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{TreeRowEditState, TreeView, TreeViewLayout};
    use crate::paint_surface::PaintSurface;
    use crate::style::Style;
    use crate::text_util::{safe_prefix, snap_to_char_boundary};
    use crate::theme::Theme;
    use crate::types::Decoration;
    use crate::Rect;

    use super::tree_cell;

    /// Paint a [`TreeView`]'s content — background, rows
    /// (selection/decoration/chevron/icon/badge/text or inline-edit),
    /// and vertical scrollbar — onto `surface` at `area`.
    ///
    /// `tree_layout` must be the same [`TreeViewLayout`] the caller uses
    /// for hit-testing (typically `gtk_tree_layout`/`mac_tree_layout`/
    /// `win_tree_layout`'s return value) so paint and hit-test can never
    /// disagree. `line_height` is one text row's height in `surface`'s
    /// native units; row pitch (header vs. leaf/branch) is derived from
    /// it via [`crate::primitives::layout_metrics::tree_row_pitch`], the
    /// same formula `tree_layout` itself used. `nerd_fonts_enabled`
    /// selects `Icon::glyph` vs. `Icon::fallback`. `style` supplies
    /// [`Style::row_padding_x`], the horizontal inset a row's leading
    /// chevron/icon/text starts at — GTK/macOS callers pass
    /// `&self.current_style`; the shared tree paint passes
    /// `&self.current_style` the same way (see that fn's own doc).
    ///
    /// `hovered_idx` / `pressed_idx` — index into `tree.rows` to tint via
    /// [`Theme::tree_hover_bg`] / [`Theme::tree_pressed_bg`]. `None`
    /// paints byte-for-byte what this function painted before hover
    /// support existed. See `primitives::list::native_surface_paint::paint`'s
    /// matching doc for the priority rule (pressed over hover, applied
    /// on top of whatever background the row already has).
    ///
    /// `focus_ring_visible` — when `true`, strokes a
    /// `Theme::focus_ring_color` ring at `Style::focus_ring_width`,
    /// flush with the row at [`TreeView::selected_path`]'s bounds, on
    /// top of whatever background that row already has. Separate from
    /// [`TreeView::has_focus`] (see that field's doc for why) — the
    /// caller passes `ring.is_focused(&tree.id) &&
    /// ring.is_focus_visible()` here, not `tree.has_focus`. `false`
    /// paints byte-for-byte what this function painted before the ring
    /// existed.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint(
        tree: &TreeView,
        area: Rect,
        tree_layout: &TreeViewLayout,
        line_height: f32,
        nerd_fonts_enabled: bool,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        style: &Style,
        hovered_idx: Option<usize>,
        pressed_idx: Option<usize>,
        focus_ring_visible: bool,
    ) {
        paint_with_scroll_px(
            tree,
            area,
            tree_layout,
            line_height,
            nerd_fonts_enabled,
            surface,
            theme,
            style,
            hovered_idx,
            pressed_idx,
            focus_ring_visible,
            None,
        )
    }

    /// Like [`paint`], plus an explicit pixel-precise scroll position
    /// (`scroll_px`) that wins over [`TreeView::scroll_offset`] for the
    /// vertical scrollbar thumb painted here (via
    /// [`TreeView::vscrollbar_with_scroll_px`]) — row positions come from
    /// `tree_layout.visible_rows`, already resolved against `scroll_px`
    /// by whichever of [`TreeView::layout_with_scroll_px`]/
    /// `mac_tree_layout_with_scroll_px` built `tree_layout`.
    ///
    /// `scroll_px: None` (what [`paint`] passes) resolves the thumb
    /// identically to `paint`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint_with_scroll_px(
        tree: &TreeView,
        area: Rect,
        tree_layout: &TreeViewLayout,
        line_height: f32,
        nerd_fonts_enabled: bool,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        style: &Style,
        hovered_idx: Option<usize>,
        pressed_idx: Option<usize>,
        focus_ring_visible: bool,
        scroll_px: Option<f32>,
    ) {
        if area.width <= 0.0 || area.height <= 0.0 {
            return;
        }

        surface.surface_push_clip(area);
        surface.surface_fill_rect(area, theme.tab_bar_bg);

        let indent_px = tree_cell::tree_indent_px(line_height);
        let header_height = (line_height * 1.2).round();
        let item_height =
            crate::primitives::layout_metrics::tree_row_pitch(tree, line_height as f64) as f32;

        for vis_row in &tree_layout.visible_rows {
            let row = &tree.rows[vis_row.row_idx];
            let row_x = area.x + vis_row.bounds.x;
            let row_y = area.y + vis_row.bounds.y;
            let row_w = vis_row.bounds.width;
            let row_h = vis_row.bounds.height;

            // Skip rows the layout clipped to a partial height — painting
            // them produces a compressed background band at the section
            // boundary (matches GTK/macOS's whole-row behaviour; Win's
            // `TreeView`'s paint paints these unconditionally, a difference this
            // unification resolves in GTK/macOS's favor).
            //
            // When a sub-row `scroll_px` is in effect, `layout_with_scroll_px`
            // always height-clips the last visible row by construction (it
            // is the row the sub-row shift pushes partway below the
            // viewport), so skipping it here would leave a bare
            // `tab_bar_bg` band at the bottom whose height breathes as the
            // scroll position changes. Paint it instead.
            let is_header = matches!(row.decoration, Decoration::Header);
            let full_h = if is_header {
                header_height
            } else {
                item_height
            };
            if scroll_px.is_none() && row_h < full_h - 0.5 {
                continue;
            }
            // Paint at the row's full, unclipped height rather than the
            // layout's possibly bottom-clipped `row_h` — the first row
            // gets the same treatment for free via its negative `y` and
            // full `height` from `layout_with_scroll_px`. Using the
            // clipped height here would vertically compress the row's
            // chevron/icon/badge/text toward the viewport edge as the
            // scroll position changes; using the full height and letting
            // the `area` clip pushed above trim the overflow keeps every
            // row's content at its normal size, just partially hidden.
            let row_h = full_h;

            let path_selected = tree.selected_path.as_ref().is_some_and(|p| p == &row.path);
            let is_selected = tree.has_focus && path_selected;
            let is_inactive_selected = !tree.has_focus && path_selected;

            let (def_fg, row_bg) = if is_selected {
                (theme.header_fg, theme.selected_bg)
            } else if is_inactive_selected {
                (theme.foreground, theme.inactive_selected_bg)
            } else if is_header {
                (theme.header_fg, theme.header_bg)
            } else {
                match row.decoration {
                    Decoration::Muted => (theme.muted_fg, theme.tab_bar_bg),
                    Decoration::Error => (theme.error_fg, theme.tab_bar_bg),
                    Decoration::Warning => (theme.warning_fg, theme.tab_bar_bg),
                    _ => (theme.foreground, theme.tab_bar_bg),
                }
            };

            let row_bg = if Some(vis_row.row_idx) == pressed_idx {
                theme.tree_pressed_bg(row_bg)
            } else if Some(vis_row.row_idx) == hovered_idx {
                theme.tree_hover_bg(row_bg)
            } else {
                row_bg
            };

            let row_rect = Rect::new(row_x, row_y, row_w, row_h);
            surface.surface_fill_rect(row_rect, row_bg);

            // Gated on `focus_ring_visible`, not `tree.has_focus` — see
            // this fn's own doc and `TreeView::has_focus`'s doc for why
            // the two are kept separate.
            if is_selected && focus_ring_visible {
                crate::primitives::row_stack::row_content_paint::paint_focus_ring(
                    surface, row_rect, theme, style,
                );
            }

            let chevron_glyph = row
                .is_expanded
                .filter(|_| tree.style.show_chevrons)
                .map(|expanded| super::tree_chevron_glyph(&tree.style, expanded));
            let cell = tree_cell::paint_tree_cell(
                surface,
                Rect::new(row_x, row_y, row_w, row_h),
                style.row_padding_x,
                row.indent,
                indent_px,
                row.is_expanded,
                chevron_glyph.as_deref(),
                line_height * 0.8,
                row.icon.as_ref(),
                nerd_fonts_enabled,
                def_fg,
            );
            let cursor_x = cell.label_rect.x;

            if let Some(ref edit) = row.edit {
                paint_edit_input(
                    surface,
                    cursor_x,
                    row_y,
                    row_h,
                    row_x + row_w,
                    edit,
                    def_fg,
                    theme.selection_bg,
                    theme.muted_fg,
                );
                continue;
            }

            let badge_info = row.badge.as_ref().map(|badge| {
                let (bw, _) = surface.surface_measure_text(&badge.text);
                let bfg = badge.fg.unwrap_or(theme.muted_fg);
                let bbg = badge.bg.unwrap_or(row_bg);
                (badge.text.clone(), bw, bfg, bbg)
            });
            let badge_reserve = badge_info
                .as_ref()
                .map(|(_, bw, ..)| *bw + 8.0)
                .unwrap_or(0.0);
            let text_right_limit = row_x + row_w - badge_reserve - 4.0;
            let text_start_x = cursor_x;

            // Reserve `text_right_limit` as the label's budget so
            // a long span can never bleed into the badge's reserved gap.
            // A label that doesn't fit the budget is ellipsized —
            // `fit_text_prefix` binary-searches the longest prefix of the
            // overflowing span that, with a trailing `…` appended, still
            // fits — rather than hard-clipped mid-glyph, so GTK/macOS/Win
            // all show the same VS Code-style truncation TUI already did.
            let avail = (text_right_limit - cursor_x).max(0.0);

            surface.surface_push_clip(Rect::new(cursor_x, row_y, avail, row_h));
            let label_fg = if matches!(row.decoration, Decoration::Muted) {
                theme.muted_fg
            } else {
                def_fg
            };
            crate::primitives::row_stack::row_content_paint::paint_row_spans(
                surface,
                &row.text,
                cursor_x,
                text_right_limit,
                (row_y, row_h),
                label_fg,
            );
            surface.surface_pop_clip();

            if let Some((btext, bw, bfg, bbg)) = badge_info {
                let bx = row_x + row_w - bw - 4.0;
                // Check if there is room to reserve the badge and gap:
                // a label just barely longer than the row must clamp and
                // still show the badge rather than hiding it outright.
                if text_right_limit > text_start_x {
                    if bbg != row_bg {
                        surface.surface_fill_rect(Rect::new(bx - 2.0, row_y, bw + 4.0, row_h), bbg);
                    }
                    let (_, bh) = surface.surface_measure_text(&btext);
                    let by = row_y + (row_h - bh) / 2.0;
                    surface.surface_draw_text_run(Rect::new(bx, by, bw, bh), &btext, bfg);
                }
            }
        }

        // Paint the vertical scrollbar when present. The track width is
        // `Style::scrollbar_width`, independent of row pitch — see that
        // field's own doc. `scroll_px` wins over `scroll_offset` when
        // set, so the thumb tracks the same sub-row position the rows
        // above were painted at.
        if let Some(vsb) =
            tree.vscrollbar_with_scroll_px(area, item_height, style.scrollbar_width, scroll_px)
        {
            crate::primitives::scrollbar::native_surface_paint::paint(&vsb, surface, theme);
        }

        surface.surface_pop_clip();
    }

    /// Paint an inline-rename [`TreeRowEditState`]: placeholder (when
    /// `edit.text` is empty), selection highlight, text, and a thin
    /// vertical caret bar. Ported from GTK's original
    /// `paint_edit_input_gtk` (the only one of the three pre-migration
    /// rasterisers with the full treatment — see this module's doc).
    #[allow(clippy::too_many_arguments)]
    fn paint_edit_input(
        surface: &mut dyn PaintSurface,
        text_x: f32,
        row_y: f32,
        row_h: f32,
        right_edge: f32,
        edit: &TreeRowEditState,
        fg: crate::types::Color,
        sel_bg: crate::types::Color,
        dim: crate::types::Color,
    ) {
        let text_w = right_edge - text_x - 4.0;
        if text_w <= 0.0 {
            return;
        }

        if edit.text.is_empty() {
            if let Some(ref ph) = edit.placeholder {
                let (_, th) = surface.surface_measure_text(ph);
                surface.surface_draw_text_run(
                    Rect::new(text_x, row_y + (row_h - th) / 2.0, text_w, th),
                    ph,
                    dim,
                );
            }
            // Caret at position 0.
            surface.surface_fill_rect(Rect::new(text_x, row_y + 3.0, 1.5, row_h - 6.0), fg);
            return;
        }

        // Selection highlight.
        if let Some(anchor) = edit.selection_anchor {
            if anchor != edit.cursor {
                let lo = snap_to_char_boundary(&edit.text, anchor.min(edit.cursor));
                let hi = snap_to_char_boundary(&edit.text, anchor.max(edit.cursor));
                let prefix = &edit.text[..lo];
                let sel_text = &edit.text[lo..hi];
                let (prefix_w, _) = surface.surface_measure_text(prefix);
                let (sel_w, _) = surface.surface_measure_text(sel_text);
                surface.surface_fill_rect(
                    Rect::new(text_x + prefix_w, row_y + 2.0, sel_w, row_h - 4.0),
                    sel_bg,
                );
            }
        }

        // Text.
        let (_, th) = surface.surface_measure_text(&edit.text);
        surface.surface_draw_text_run(
            Rect::new(text_x, row_y + (row_h - th) / 2.0, text_w, th),
            &edit.text,
            fg,
        );

        // Thin vertical caret bar.
        let cursor_prefix = safe_prefix(&edit.text, edit.cursor);
        let (cx_off, _) = surface.surface_measure_text(cursor_prefix);
        let caret_x = text_x + cx_off;
        surface.surface_fill_rect(Rect::new(caret_x, row_y + 3.0, 1.5, row_h - 6.0), fg);
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::{Point, Viewport};
        use crate::primitives::tree::TreeRow;
        use crate::types::{
            Badge, Color, SelectionMode, StyledSpan, StyledText, TreeStyle, WidgetId,
        };
        use crate::Image;

        /// Records every fill + stroke + text-run call — mirrors
        /// `primitives::list::native_surface_paint`'s `RecordingSurface`
        /// test double, scoped to the verbs this primitive uses, so this
        /// test runs on any host without Cairo/Core Graphics/Direct2D.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            strokes: Vec<(Rect, Color, f32)>,
            texts: Vec<(Rect, String, Color)>,
            clips: Vec<Rect>,
        }

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(200.0, 100.0, 1.0)
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
                self.fills.push((rect, color));
            }
            fn surface_fill_rounded_rect(&mut self, rect: Rect, _radius: f32, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
                self.strokes.push((rect, color, stroke_width));
            }
            fn surface_stroke_rounded_rect(
                &mut self,
                rect: Rect,
                _radius: f32,
                color: Color,
                stroke_width: f32,
            ) {
                self.strokes.push((rect, color, stroke_width));
            }
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.texts.push((rect, text.to_string(), color));
            }
            fn surface_draw_line(&mut self, _from: Point, _to: Point, _color: Color, _sw: f32) {}
            fn surface_push_clip(&mut self, rect: Rect) {
                self.clips.push(rect);
            }
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        fn leaf(idx: u16, label: &str) -> TreeRow {
            TreeRow {
                path: vec![idx],
                indent: 0,
                icon: None,
                text: StyledText::plain(label.to_string()),
                badge: None,
                is_expanded: None,
                decoration: Decoration::Normal,
                edit: None,
            }
        }

        fn make_tree(rows: Vec<TreeRow>) -> TreeView {
            TreeView {
                id: WidgetId::new("t"),
                rows,
                selection_mode: SelectionMode::Single,
                selected_path: None,
                scroll_offset: 0,
                items_window: None,
                scroll_px: None,
                style: TreeStyle::default(),
                has_focus: true,
            }
        }

        const LINE_HEIGHT: f32 = 16.0;
        const AREA: Rect = Rect::new(0.0, 0.0, 100.0, 80.0);

        /// Before this migration, none of the three
        /// backends' `TreeView`'s paint painted a vertical scrollbar even
        /// though `TreeView::vscrollbar` already exposed
        /// real geometry for hit-testing. Paints through the shared fn
        /// and asserts a fill lands at the exact track rect
        /// `TreeView::vscrollbar` resolves.
        #[test]
        fn paints_vertical_scrollbar_track_when_overflowing() {
            let tree = make_tree((0..50).map(|i| leaf(i, &format!("row{i}"))).collect());
            let layout = crate::primitives::layout_metrics::pixel_tree_layout(
                &tree,
                AREA,
                LINE_HEIGHT as f64,
                8.0,
            );
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(
                &tree,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                &mut surface,
                &theme,
                &Style::default(),
                None,
                None,
                false,
            );

            let item_height =
                crate::primitives::layout_metrics::tree_row_pitch(&tree, LINE_HEIGHT as f64) as f32;
            let expected = tree
                .vscrollbar_with_track_width(
                    AREA,
                    item_height,
                    crate::primitives::layout_metrics::pixel::TREE_VSCROLLBAR_WIDTH,
                )
                .expect("50 rows in an 80px/22px viewport must need a v-scrollbar");
            assert!(
                surface
                    .fills
                    .iter()
                    .any(|(r, _)| (r.x - expected.track.x).abs() < 0.01
                        && (r.y - expected.track.y).abs() < 0.01
                        && (r.width - expected.track.width).abs() < 0.01
                        && (r.height - expected.track.height).abs() < 0.01),
                "expected a fill at the v-scrollbar track {:?}, got fills: {:?}",
                expected.track,
                surface.fills
            );
        }

        /// A headless, end-to-end proof that `focus_ring_visible` reflects
        /// [`crate::focus::FocusTree`]'s real Tab-vs-click `:focus-visible`
        /// convention — composed as `tree.is_focused(id, None) &&
        /// tree.is_focus_visible()`, the bool a host passes here — and
        /// that the painted stroke actually tracks it, not just a literal
        /// `true`/`false` this test assigns itself.
        #[test]
        fn focused_tree_row_paints_a_ring_after_tab_but_not_after_click() {
            use crate::focus::FocusTree;

            let id = WidgetId::new("sidebar:tree");
            let mut focus = FocusTree::new();
            focus.sync_tab_order(&[(id, Rect::new(0.0, 0.0, 0.0, 0.0))]);
            let theme = Theme::default();
            let style = Style::default();
            let mut tree = make_tree(vec![leaf(0, "alpha")]);
            tree.id = id;
            tree.selected_path = Some(vec![0]);

            let paint_and_count_strokes = |focus_ring_visible: bool| -> usize {
                let layout = crate::primitives::layout_metrics::pixel_tree_layout(
                    &tree,
                    AREA,
                    LINE_HEIGHT as f64,
                    8.0,
                );
                let mut surface = RecordingSurface::default();
                paint(
                    &tree,
                    AREA,
                    &layout,
                    LINE_HEIGHT,
                    false,
                    &mut surface,
                    &theme,
                    &style,
                    None,
                    None,
                    focus_ring_visible,
                );
                surface.strokes.len()
            };

            // Tab: `focus_next()` is the real keyboard-move path.
            focus.focus_next();
            let focus_ring_visible = focus.is_focused(&id, None) && focus.is_focus_visible();
            assert!(
                focus_ring_visible,
                "a Tab-reached tree must report focus-visible"
            );
            assert_eq!(
                paint_and_count_strokes(focus_ring_visible),
                1,
                "a Tab-reached tree's selected row must paint a ring"
            );

            // Click: `set_focus` re-focuses the same widget via the pointer path.
            focus.set_focus(Some((id, None)));
            let focus_ring_visible = focus.is_focused(&id, None) && focus.is_focus_visible();
            assert!(
                focus.is_focused(&id, None),
                "the click did move focus to this tree"
            );
            assert!(
                !focus_ring_visible,
                "a mouse click must suppress the tree's focus-visible ring"
            );
            assert_eq!(
                paint_and_count_strokes(focus_ring_visible),
                0,
                "a clicked tree's selected row must paint no ring"
            );

            // Shift+Tab afterwards restores the ring.
            focus.focus_prev();
            let focus_ring_visible = focus.is_focused(&id, None) && focus.is_focus_visible();
            assert!(
                focus_ring_visible,
                "a subsequent keyboard move restores focus-visible"
            );
            assert_eq!(
                paint_and_count_strokes(focus_ring_visible),
                1,
                "Shift+Tab repaints the same ring Tab did"
            );
        }

        /// A `scroll_px` amount smaller than the row height
        /// (`layout_with_scroll_px`'s uniform row pitch here) shifts
        /// every painted row's `y` up by that exact remainder — the
        /// "partial first row clipped" GUI look of pixel-smooth
        /// scrolling. `RecordingSurface` doesn't actually clip (that's
        /// `surface_push_clip(area)`, called right before painting rows,
        /// which a live GUI surface honours), so the shifted row is
        /// recorded with `y` above `area.y` directly.
        #[test]
        fn scroll_px_shifts_rows_by_the_sub_row_remainder() {
            let mut tree = make_tree((0..50).map(|i| leaf(i, &format!("row{i}"))).collect());
            tree.selected_path = Some(vec![0]);
            // `paint` below recomputes its own uniform row height via
            // `tree_row_pitch` (independent of whatever `layout`'s own
            // measurer reports) to decide whether a row was clipped to a
            // partial height and should be skipped — match it here so
            // the test's row 0 isn't spuriously treated as clipped.
            let item_height =
                crate::primitives::layout_metrics::tree_row_pitch(&tree, LINE_HEIGHT as f64) as f32;
            let sub_row_px = 8.0; // < item_height row pitch, so row stays 0
            let layout =
                tree.layout_with_scroll_px(AREA.width, AREA.height, Some(sub_row_px), |_| {
                    super::super::TreeRowMeasure::new(item_height)
                });
            assert_eq!(layout.resolved_scroll_offset, 0);
            assert!((layout.scroll_sub_px - sub_row_px).abs() < 0.01);

            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(
                &tree,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                &mut surface,
                &theme,
                &Style::default(),
                None,
                None,
                false,
            );

            let selected_fill = surface
                .fills
                .iter()
                .find(|(_, c)| *c == theme.selected_bg)
                .expect("row 0's selected background should still be painted, just shifted up");
            assert!(
                selected_fill.0.y < AREA.y,
                "a sub-row scroll_px should shift row 0 above the viewport top, got y={}",
                selected_fill.0.y
            );
        }

        /// The painted track's width follows `Style::scrollbar_width`,
        /// not the fixed `layout_metrics::pixel::TREE_VSCROLLBAR_WIDTH`
        /// constant — a host-set non-default width must actually reach
        /// the paint surface.
        #[test]
        fn vertical_scrollbar_track_width_follows_style_token() {
            let tree = make_tree((0..50).map(|i| leaf(i, &format!("row{i}"))).collect());
            let layout = crate::primitives::layout_metrics::pixel_tree_layout(
                &tree,
                AREA,
                LINE_HEIGHT as f64,
                8.0,
            );
            let style = Style::default().with_scrollbar_width(20.0);
            let mut surface = RecordingSurface::default();
            paint(
                &tree,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                &mut surface,
                &Theme::default(),
                &style,
                None,
                None,
                false,
            );

            let track_fill = surface
                .fills
                .iter()
                .find(|(r, _)| (r.width - 20.0).abs() < 0.01)
                .expect("expected a fill at the 20px-wide scrollbar track");
            assert_eq!(track_fill.0.x, AREA.x + AREA.width - 20.0);
        }

        /// A branch row's default-style chevron paints as the codicon
        /// glyph on GUI, not the plain `▾`/`▸` `TreeStyle::default()`
        /// carries — the "Done when" behaviour this swap exists for.
        #[test]
        fn default_style_chevron_paints_as_codicon_glyph() {
            let mut rows = vec![leaf(0, "branch")];
            rows[0].is_expanded = Some(true);
            let tree = make_tree(rows);
            let layout = crate::primitives::layout_metrics::pixel_tree_layout(
                &tree,
                AREA,
                LINE_HEIGHT as f64,
                8.0,
            );
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(
                &tree,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                &mut surface,
                &theme,
                &Style::default(),
                None,
                None,
                false,
            );

            assert!(
                surface
                    .texts
                    .iter()
                    .any(|(_, t, _)| t == crate::codicon::CHEVRON_DOWN.to_string().as_str()),
                "expected the codicon expanded-chevron glyph among painted text runs, got: {:?}",
                surface.texts
            );
            assert!(
                !surface.texts.iter().any(|(_, t, _)| t == "▾"),
                "the plain `▾` default must not be painted verbatim on GUI, got: {:?}",
                surface.texts
            );
        }

        /// An app-supplied, non-default `chevron_expanded`/
        /// `chevron_collapsed` override is still honoured on GUI — only
        /// the *default* string gets swapped for a codicon glyph (see
        /// the previous test).
        #[test]
        fn non_default_chevron_override_is_honoured_verbatim_on_gui() {
            let mut rows = vec![leaf(0, "branch")];
            rows[0].is_expanded = Some(true);
            let mut tree = make_tree(rows);
            tree.style.chevron_expanded = "+".into();
            tree.style.chevron_collapsed = "-".into();
            let layout = crate::primitives::layout_metrics::pixel_tree_layout(
                &tree,
                AREA,
                LINE_HEIGHT as f64,
                8.0,
            );
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(
                &tree,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                &mut surface,
                &theme,
                &Style::default(),
                None,
                None,
                false,
            );

            assert!(
                surface.texts.iter().any(|(_, t, _)| t == "+"),
                "expected the app's own override glyph among painted text runs, got: {:?}",
                surface.texts
            );
            assert!(
                !surface
                    .texts
                    .iter()
                    .any(|(_, t, _)| t == crate::codicon::CHEVRON_DOWN.to_string().as_str()),
                "an explicit override must not be silently replaced by the codicon glyph, got: {:?}",
                surface.texts
            );
        }

        /// the shared tree paint never painted
        /// `TreeRow::edit` at all (see this module's doc). This test
        /// paints a row mid-rename through the shared fn and asserts a
        /// caret bar (a thin, distinctly-sized fill) shows up — it would
        /// have failed against the pre-migration Win rasteriser, which
        /// rendered the row's stale label instead of any edit-state
        /// paint at all.
        #[test]
        fn paints_caret_for_row_being_edited() {
            let mut rows = vec![leaf(0, "alpha"), leaf(1, "old-name"), leaf(2, "gamma")];
            rows[1].edit = Some(TreeRowEditState {
                text: "new-name".into(),
                cursor: 3,
                selection_anchor: None,
                placeholder: None,
            });
            let tree = make_tree(rows);
            let layout = crate::primitives::layout_metrics::pixel_tree_layout(
                &tree,
                AREA,
                LINE_HEIGHT as f64,
                8.0,
            );
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(
                &tree,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                &mut surface,
                &theme,
                &Style::default(),
                None,
                None,
                false,
            );

            let row1 = &layout.visible_rows[1];
            let caret_fill = surface.fills.iter().find(|(r, _)| {
                (r.width - 1.5).abs() < 0.01
                    && r.y >= row1.bounds.y
                    && r.y < row1.bounds.y + row1.bounds.height
            });
            assert!(
                caret_fill.is_some(),
                "expected a 1.5px-wide caret fill inside row 1's bounds {:?}, got fills: {:?}",
                row1.bounds,
                surface.fills
            );
        }

        /// Selection highlight inside an edited row paints a fill in
        /// `theme.selection_bg` sized to the selected substring, distinct
        /// from the caret fill.
        #[test]
        fn paints_selection_highlight_for_edited_row_with_selection() {
            let mut rows = vec![leaf(0, "old-name")];
            rows[0].edit = Some(TreeRowEditState {
                text: "new-name".into(),
                cursor: 3,
                selection_anchor: Some(0),
                placeholder: None,
            });
            let tree = make_tree(rows);
            let layout = crate::primitives::layout_metrics::pixel_tree_layout(
                &tree,
                AREA,
                LINE_HEIGHT as f64,
                8.0,
            );
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(
                &tree,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                &mut surface,
                &theme,
                &Style::default(),
                None,
                None,
                false,
            );

            assert!(
                surface
                    .fills
                    .iter()
                    .any(|(r, c)| *c == theme.selection_bg && r.width > 1.5),
                "expected a selection-highlight fill in theme.selection_bg wider than \
                 the caret bar, got fills: {:?}",
                surface.fills
            );
        }

        #[test]
        fn selected_row_paints_selected_bg() {
            let mut tree = make_tree(vec![leaf(0, "alpha"), leaf(1, "beta"), leaf(2, "gamma")]);
            tree.selected_path = Some(vec![1]);
            tree.has_focus = true;
            let layout = crate::primitives::layout_metrics::pixel_tree_layout(
                &tree,
                AREA,
                LINE_HEIGHT as f64,
                8.0,
            );
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(
                &tree,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                &mut surface,
                &theme,
                &Style::default(),
                None,
                None,
                false,
            );

            let row1 = &layout.visible_rows[1];
            assert!(
                surface
                    .fills
                    .iter()
                    .any(|(r, c)| (r.x - row1.bounds.x).abs() < 0.01
                        && (r.y - row1.bounds.y).abs() < 0.01
                        && *c == theme.selected_bg),
                "selected row must paint theme.selected_bg at its own bounds"
            );
        }

        /// The focused row's ring strokes exactly at the selected row's
        /// own bounds, in `Theme::focus_ring_color` at
        /// `Style::focus_ring_width` — and only there. Discriminated
        /// from `selected_row_paints_selected_bg` above by checking
        /// `surface.strokes` (a distinct call) rather than
        /// `surface.fills`, so this test can't pass on the pre-existing
        /// fill alone.
        #[test]
        fn focused_row_paints_a_ring_at_its_own_bounds() {
            let mut tree = make_tree(vec![leaf(0, "alpha"), leaf(1, "beta"), leaf(2, "gamma")]);
            tree.selected_path = Some(vec![1]);
            tree.has_focus = true;
            let layout = crate::primitives::layout_metrics::pixel_tree_layout(
                &tree,
                AREA,
                LINE_HEIGHT as f64,
                8.0,
            );
            let style = Style::default().with_focus_ring_width(3.0);
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(
                &tree,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                &mut surface,
                &theme,
                &style,
                None,
                None,
                /* focus_ring_visible */ true,
            );

            let row1 = &layout.visible_rows[1];
            assert!(
                surface
                    .strokes
                    .iter()
                    .any(|(r, c, w)| (r.x - row1.bounds.x).abs() < 0.01
                        && (r.y - row1.bounds.y).abs() < 0.01
                        && (r.width - row1.bounds.width).abs() < 0.01
                        && (r.height - row1.bounds.height).abs() < 0.01
                        && *c == theme.focus_ring_color()
                        && (*w - 3.0).abs() < 0.01),
                "focused row must stroke a ring at its own bounds with \
                 Style::focus_ring_width, got strokes: {:?}",
                surface.strokes
            );
            assert_eq!(
                surface.strokes.len(),
                1,
                "only the focused row gets a ring, got strokes: {:?}",
                surface.strokes
            );
        }

        /// `focus_ring_visible: false` paints no ring even on a row
        /// that is genuinely selected (`tree.has_focus: true` here) —
        /// the default a call site gets unless it explicitly composes
        /// `ring.is_focused(id) && ring.is_focus_visible()`. Paired
        /// with `no_ring_without_a_selected_row_even_when_focus_ring_visible`
        /// below: either gate alone must suppress the ring.
        #[test]
        fn no_ring_when_focus_ring_visible_is_false() {
            let mut tree = make_tree(vec![leaf(0, "alpha"), leaf(1, "beta"), leaf(2, "gamma")]);
            tree.selected_path = Some(vec![1]);
            tree.has_focus = true;
            let layout = crate::primitives::layout_metrics::pixel_tree_layout(
                &tree,
                AREA,
                LINE_HEIGHT as f64,
                8.0,
            );
            let mut surface = RecordingSurface::default();
            paint(
                &tree,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                &mut surface,
                &Theme::default(),
                &Style::default(),
                None,
                None,
                /* focus_ring_visible */ false,
            );
            assert!(
                surface.strokes.is_empty(),
                "focus_ring_visible: false must paint no ring even on a \
                 selected, has_focus: true row, got strokes: {:?}",
                surface.strokes
            );
        }

        /// `focus_ring_visible: true` alone doesn't paint a ring when
        /// nothing is selected-and-focused (`tree.has_focus: false`
        /// here) — the ring stays scoped to
        /// [`TreeView::selected_path`]'s row via `is_selected`. The row
        /// still shows the unfocused, muted `inactive_selected_bg`
        /// fill — only the ring is gated this way.
        #[test]
        fn no_ring_without_a_selected_row_even_when_focus_ring_visible() {
            let mut tree = make_tree(vec![leaf(0, "alpha"), leaf(1, "beta"), leaf(2, "gamma")]);
            tree.selected_path = Some(vec![1]);
            tree.has_focus = false;
            let layout = crate::primitives::layout_metrics::pixel_tree_layout(
                &tree,
                AREA,
                LINE_HEIGHT as f64,
                8.0,
            );
            let mut surface = RecordingSurface::default();
            paint(
                &tree,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                &mut surface,
                &Theme::default(),
                &Style::default(),
                None,
                None,
                /* focus_ring_visible */ true,
            );
            assert!(
                surface.strokes.is_empty(),
                "an unfocused tree must paint no ring even when \
                 focus_ring_visible: true, got strokes: {:?}",
                surface.strokes
            );
        }

        /// Error/Warning decoration rows paint their dedicated fg colour
        /// (adopted from Win's pre-migration mapping — see this module's
        /// doc for why GTK/macOS's narrower `Muted`-only mapping wasn't
        /// preserved instead).
        #[test]
        fn error_and_warning_decoration_use_dedicated_fg() {
            let mut err_row = leaf(0, "boom");
            err_row.decoration = Decoration::Error;
            let mut warn_row = leaf(1, "careful");
            warn_row.decoration = Decoration::Warning;
            let tree = make_tree(vec![err_row, warn_row]);
            let layout = crate::primitives::layout_metrics::pixel_tree_layout(
                &tree,
                AREA,
                LINE_HEIGHT as f64,
                8.0,
            );
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(
                &tree,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                &mut surface,
                &theme,
                &Style::default(),
                None,
                None,
                false,
            );

            assert!(surface
                .texts
                .iter()
                .any(|(_, t, c)| t == "boom" && *c == theme.error_fg));
            assert!(surface
                .texts
                .iter()
                .any(|(_, t, c)| t == "careful" && *c == theme.warning_fg));
        }

        /// GUI backends (GTK/Win/macOS all share this `paint`
        /// fn) must ellipsize an over-long label before a right-aligned
        /// badge the same way TUI does — a literal trailing `…` that fits
        /// within the label's budget, never a mid-glyph hard clip and
        /// never painted through/past the badge's reserved gap.
        #[test]
        fn long_label_with_badge_ellipsizes_before_badge() {
            let mut row = leaf(0, "BACKEND_SPECULATIVE_EXECUTION_MODULE");
            row.badge = Some(Badge::plain("U"));
            let tree = make_tree(vec![row]);
            let layout = crate::primitives::layout_metrics::pixel_tree_layout(
                &tree,
                AREA,
                LINE_HEIGHT as f64,
                8.0,
            );
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(
                &tree,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                &mut surface,
                &theme,
                &Style::default(),
                None,
                None,
                false,
            );

            let (badge_bw, _) = surface.surface_measure_text("U");
            let row_bounds = layout.visible_rows[0].bounds;
            let row_x = AREA.x + row_bounds.x;
            let row_w = row_bounds.width;
            let text_right_limit = row_x + row_w - (badge_bw + 8.0) - 4.0;

            let ellipsized = surface
                .texts
                .iter()
                .find(|(_, t, _)| t.ends_with('…'))
                .expect("an over-long label must paint a run ending in an ellipsis");
            let (rect, text, _color) = ellipsized;
            assert!(
                text.chars().count() < "BACKEND_SPECULATIVE_EXECUTION_MODULE".chars().count(),
                "the ellipsized run must be a strict prefix of the full label, got {text:?}"
            );
            assert!(
                rect.x + rect.width <= text_right_limit + 0.01,
                "the ellipsized run's right edge ({}) must fit within the label's \
                 budget ({text_right_limit}), leaving the badge's reserved gap intact",
                rect.x + rect.width
            );

            // The badge itself must still be painted (there was room for it).
            assert!(
                surface.texts.iter().any(|(_, t, _)| t == "U"),
                "badge text should still paint when the label is ellipsized, got texts: {:?}",
                surface.texts
            );
        }

        /// A span whose own `text` is simply `""` (a legitimate pattern —
        /// see `compose::help_layer`'s `StyledSpan::plain("")`) must not be
        /// mistaken for a span truncated to nothing: its natural zero
        /// width must not stop later, non-overflowing spans in the same
        /// row from painting.
        #[test]
        fn empty_leading_span_does_not_swallow_later_spans() {
            let mut row = leaf(0, "placeholder");
            row.text = StyledText {
                spans: vec![StyledSpan::plain(""), StyledSpan::plain("ok")],
            };
            let tree = make_tree(vec![row]);
            let layout = crate::primitives::layout_metrics::pixel_tree_layout(
                &tree,
                AREA,
                LINE_HEIGHT as f64,
                8.0,
            );
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(
                &tree,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                &mut surface,
                &theme,
                &Style::default(),
                None,
                None,
                false,
            );

            assert!(
                surface.texts.iter().any(|(_, t, _)| t == "ok"),
                "a span after an empty, non-truncated span must still paint, got texts: {:?}",
                surface.texts
            );
        }

        /// The issue's first acceptance bullet: a row's text starts
        /// `row_padding_x` in from its rect. `show_chevrons = false` on
        /// an otherwise-branch row (`is_expanded: Some(_)`) isolates the
        /// leading inset from the chevron glyph and the leaf-only
        /// `line_height * 0.8` offset, so the only thing between the
        /// row's edge and the label is `row_padding_x` itself. Painted
        /// with a non-default value (15.0, not the 8.0 default) so
        /// reverting the token back to the old hardcoded `2.0` literal
        /// in `paint` would fail this test.
        #[test]
        fn row_text_starts_row_padding_x_in_from_the_row_rect() {
            let mut row = leaf(0, "alpha");
            row.is_expanded = Some(true);
            let mut tree = make_tree(vec![row]);
            tree.style.show_chevrons = false;

            let style = Style::default().with_row_padding_x(15.0);
            let layout = crate::primitives::layout_metrics::pixel_tree_layout(
                &tree,
                AREA,
                LINE_HEIGHT as f64,
                style.row_padding_x,
            );
            let row_bounds = layout.visible_rows[0].bounds;

            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(
                &tree,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                &mut surface,
                &theme,
                &style,
                None,
                None,
                false,
            );

            let (text_rect, _, _) = surface
                .texts
                .iter()
                .find(|(_, t, _)| t == "alpha")
                .expect("row text painted");
            assert_eq!(
                text_rect.x,
                AREA.x + row_bounds.x + style.row_padding_x,
                "row text must start row_padding_x in from the row's left edge"
            );
        }
    }
}

// ── `crate::primitive::Primitive` impl ──────────────────────────────
//
// `paint`'s pixel-backend branch needs `native_surface_paint`;
// `paint_cells` needs the cell-grid rasteriser in `paint_cells`/`ratatui` — gated to
// whichever of those a build actually has, same as those modules
// themselves.

/// [`crate::primitive::Primitive::Layout`] for [`TreeView`] — the
/// tree-local [`TreeViewLayout`] paired with the absolute area it was
/// laid out into, the resolved row-paint metrics, and the vertical
/// scrollbar the layout decided on.
///
/// `#[non_exhaustive]`: backend-returned, never constructed by a
/// consumer via struct literal, same convention as [`TreeViewLayout`]
/// itself.
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct TreeViewPrimitiveLayout {
    /// Absolute area the rows were laid out into — `layout`'s own
    /// `visible_rows`/`hit_regions` are relative to this rect's origin.
    /// On a cell grid this excludes the scrollbar column the layout
    /// reserved (see [`Self::vscrollbar`]); on a pixel surface the
    /// scrollbar overlays the rows, so this is the whole widget rect.
    pub area: Rect,
    /// Row height resolved from the [`crate::primitive::LayoutCtx`]
    /// [`Primitive::layout`](crate::primitive::Primitive::layout) was
    /// called with.
    pub line_height: f32,
    /// Nerd-Font toggle resolved the same way.
    pub nerd_fonts_enabled: bool,
    /// The tree-local layout itself.
    pub layout: TreeViewLayout,
    /// Height of one non-header row in the layout's own unit: `1.0` on
    /// a cell grid, [`crate::primitives::layout_metrics::tree_row_pitch`]
    /// on a pixel surface. The unit a wheel delta and a scrollbar drag
    /// are converted through.
    pub row_pitch: f32,
    /// How many whole rows fit in the viewport — the page size for
    /// Page Up/Down and the window `ensure_visible` keeps the selection
    /// inside.
    pub viewport_rows: usize,
    /// The vertical scrollbar, in absolute coordinates, when the rows
    /// overflow the viewport; `None` when they all fit.
    pub vscrollbar: Option<Scrollbar>,
    /// [`Self::vscrollbar`]'s own resolved [`Primitive`] layout, so the
    /// scrollbar paints through its own widget.
    ///
    /// [`Primitive`]: crate::primitive::Primitive
    pub vscrollbar_layout: Option<crate::primitives::scrollbar::ScrollbarLayout>,
    /// The band a windowed tree ([`TreeView::items_window`]) was laid out
    /// through, `None` for an unwindowed one. [`Self::layout`]'s row
    /// indices are relative to its band; [`Self::abs_row`] maps them back.
    /// [`Self::vscrollbar`] is sized from the real extent.
    pub window: Option<Box<crate::behaviour::row_window::WindowedLayout<TreeView>>>,
}

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
impl TreeViewPrimitiveLayout {
    /// Hit-test at an absolute `(x, y)` — subtracts [`Self::area`]'s
    /// origin before delegating to [`TreeViewLayout::hit_test`], whose
    /// `hit_regions` are tree-local.
    pub fn hit_test(&self, x: f32, y: f32) -> TreeViewHit {
        self.layout.hit_test(x - self.area.x, y - self.area.y)
    }

    /// The absolute row index of [`Self::layout`]'s row `idx`.
    pub fn abs_row(&self, idx: usize) -> usize {
        match &self.window {
            Some(w) => w.to_abs(idx),
            None => idx,
        }
    }

    /// Whether the absolute point `(x, y)` lands on the vertical
    /// scrollbar's track.
    fn on_scrollbar(&self, x: f32, y: f32) -> Option<&Scrollbar> {
        self.vscrollbar
            .as_ref()
            .filter(|sb| sb.track.contains(crate::event::Point::new(x, y)))
    }
}

/// One addressable part of a laid-out [`TreeView`], named as a
/// [`crate::interactive::SubTarget`] so the runtime can report hover and
/// press per row and a driver can target a row's chevron.
///
/// Round-trips through [`Self::sub_target`]/[`Self::from_sub_target`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TreeViewSubPart {
    /// A row's body. Index is into `TreeView.rows`.
    Row(usize),
    /// A branch row's expand/collapse chevron. Index is into
    /// `TreeView.rows`.
    Chevron(usize),
    /// The vertical scrollbar.
    Scrollbar,
}

impl TreeViewSubPart {
    /// This part's [`crate::interactive::SubTarget`]: the kind in the
    /// high 32 bits, the row index in the low 32.
    pub fn sub_target(self) -> crate::interactive::SubTarget {
        let (kind, idx) = match self {
            Self::Row(i) => (1u64, i as u64),
            Self::Chevron(i) => (2, i as u64),
            Self::Scrollbar => (3, 0),
        };
        crate::interactive::SubTarget((kind << 32) | (idx & 0xffff_ffff))
    }

    /// The part `sub` names, or `None` for a target this widget never
    /// produces.
    pub fn from_sub_target(sub: crate::interactive::SubTarget) -> Option<Self> {
        let idx = (sub.0 & 0xffff_ffff) as usize;
        match sub.0 >> 32 {
            1 => Some(Self::Row(idx)),
            2 => Some(Self::Chevron(idx)),
            3 => Some(Self::Scrollbar),
            _ => None,
        }
    }

    /// The row this part belongs to, if any.
    pub fn row(self) -> Option<usize> {
        match self {
            Self::Row(i) | Self::Chevron(i) => Some(i),
            Self::Scrollbar => None,
        }
    }
}

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
impl TreeView {
    /// The unwindowed layout at an explicit sub-row `scroll_px`.
    fn layout_at(
        &self,
        ctx: &crate::primitive::LayoutCtx,
        rect: Rect,
        scroll_px: Option<f32>,
    ) -> TreeViewPrimitiveLayout {
        let border = if self.style.bordered { 2.0 } else { 0.0 };
        // `ctx.units()` separates TUI's whole-cell `line_height`
        // convention from a real pixel font metric, even when both
        // features are compiled into the same binary.
        #[cfg(feature = "tui")]
        if ctx.units() == crate::primitive::Units::Cells {
            let viewport_rows = (rect.height - border).max(0.0).floor() as usize;
            // A cell grid has no overlay: the scrollbar takes the
            // rightmost column away from the rows when they overflow,
            // unless the style turns the scrollbar off.
            let (area, vscrollbar) =
                if self.style.show_scrollbar && self.rows.len() > viewport_rows && rect.width > 1.0
                {
                    (
                        Rect::new(rect.x, rect.y, rect.width - 1.0, rect.height),
                        // A cell grid scrolls by whole rows.
                        self.vscrollbar_with_scroll_px(rect, 1.0, 1.0, scroll_px.map(f32::floor)),
                    )
                } else {
                    (rect, None)
                };
            let layout = crate::tui::tui_tree_layout_with_scroll_px(
                self,
                crate::tui::backend::q_rect_to_ratatui(area),
                scroll_px,
            );
            let vscrollbar_layout = vscrollbar
                .as_ref()
                .map(|sb| crate::primitive::Primitive::layout(sb, ctx, sb.track));
            return TreeViewPrimitiveLayout {
                area,
                line_height: ctx.line_height,
                nerd_fonts_enabled: ctx.nerd_fonts_enabled,
                layout,
                row_pitch: 1.0,
                viewport_rows,
                vscrollbar,
                vscrollbar_layout,
                window: None,
            };
        }
        // The host's live `Style` sets the row padding and scrollbar
        // width, and the chevron is measured in the chrome font the
        // painter draws it in, so the chevron hit band ends where the
        // painted glyph does.
        let style = ctx.style();
        let measure = crate::primitives::layout_metrics::LayoutCtxMeasure {
            ctx,
            role: crate::FontRole::Chrome,
        };
        let layout = crate::primitives::layout_metrics::pixel_tree_layout_with_scroll_px(
            self,
            rect,
            ctx.line_height as f64,
            &measure,
            style.row_padding_x,
            scroll_px,
        );
        let row_pitch =
            crate::primitives::layout_metrics::tree_row_pitch(self, ctx.line_height as f64) as f32;
        let viewport_rows = if row_pitch > 0.0 {
            ((rect.height - border).max(0.0) / row_pitch).floor() as usize
        } else {
            0
        };
        // A pixel surface overlays the scrollbar on the rows (the shared
        // pixel painter draws it), so no width is reserved for it here.
        let vscrollbar =
            self.vscrollbar_with_scroll_px(rect, row_pitch, style.scrollbar_width, scroll_px);
        let vscrollbar_layout = vscrollbar
            .as_ref()
            .map(|sb| crate::primitive::Primitive::layout(sb, ctx, sb.track));
        TreeViewPrimitiveLayout {
            area: rect,
            line_height: ctx.line_height,
            nerd_fonts_enabled: ctx.nerd_fonts_enabled,
            layout,
            row_pitch,
            viewport_rows,
            vscrollbar,
            vscrollbar_layout,
            window: None,
        }
    }

    /// The layout of a windowed tree: only the band of rows the viewport
    /// shows is materialised (empty placeholder rows outside the loaded
    /// slice), laid out at offset `0`, with the scrollbar sized from the
    /// real extent.
    fn windowed_layout(
        &self,
        ctx: &crate::primitive::LayoutCtx,
        rect: Rect,
        scroll_px: Option<f32>,
        window: crate::behaviour::row_window::ItemsWindow,
    ) -> TreeViewPrimitiveLayout {
        use crate::behaviour::row_window;
        let cells = ctx.units() == crate::primitive::Units::Cells;
        let pitch = if cells {
            1.0
        } else {
            crate::primitives::layout_metrics::tree_row_pitch(self, ctx.line_height as f64) as f32
        };
        let (scroll, rem) = row_window::split_scroll_px(self.scroll_offset, scroll_px, pitch);
        let rem = if cells { None } else { rem };
        let loaded = self.rows.len();
        let total = window.row_count(loaded);
        let band_rows = row_window::band_rows_for(rect.height, pitch);
        let band = row_window::band(window, loaded, scroll, band_rows);
        let mut rows = row_window::materialise(&self.rows, window, band, |_| placeholder_row());
        // Padded past the last row near the end of an overflowing tree, so
        // the band still overflows and reserves the scrollbar the real
        // extent needs; rows resolve only through the loaded slice, so the
        // padding never reaches an event.
        if total > band_rows - row_window::BAND_PAD_ROWS {
            rows.resize_with(band_rows, placeholder_row);
        }
        let band_tree = TreeView {
            rows,
            scroll_offset: 0,
            items_window: None,
            scroll_px: None,
            ..self.clone()
        };
        let mut layout = band_tree.layout_at(ctx, rect, rem);
        let viewport_rows = layout.viewport_rows;
        layout.vscrollbar = layout.vscrollbar.as_ref().and_then(|sb| {
            row_window::extent_scrollbar(
                self.id,
                sb.track,
                total,
                band.start,
                rem,
                viewport_rows,
                pitch,
            )
        });
        layout.vscrollbar_layout = layout
            .vscrollbar
            .as_ref()
            .map(|sb| crate::primitive::Primitive::layout(sb, ctx, sb.track));
        layout.window = Some(Box::new(row_window::WindowedLayout {
            band,
            total,
            loaded: (window.start, loaded),
            scroll: band.start,
            viewport_rows,
            scroll_px: rem,
            live: Box::new(band_tree),
        }));
        layout
    }
}

/// The empty row a windowed tree paints where it has no loaded row. Its
/// empty path matches no real row (and no selection), and a windowed
/// tree resolves rows only through its loaded slice, so it never reaches
/// an event.
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
fn placeholder_row() -> TreeRow {
    TreeRow {
        path: Vec::new(),
        indent: 0,
        icon: None,
        text: StyledText::plain(""),
        badge: None,
        is_expanded: None,
        decoration: Decoration::default(),
        edit: None,
    }
}

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
impl crate::primitive::Primitive for TreeView {
    type Layout = TreeViewPrimitiveLayout;
    type Event = TreeEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// One line-height row per row of the real collection — what a
    /// container sizing this tree to its content gives it. No intrinsic
    /// width.
    fn measure(
        &self,
        ctx: &crate::primitive::LayoutCtx,
        avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        let _ = avail;
        crate::NaturalSize::new(
            None,
            Some(self.row_count() as f32 * ctx.line_height.max(1.0)),
        )
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::Tree(self.clone())
    }

    /// The tree resolves through its own state first ([`TreeView::live`]),
    /// and a windowed one ([`TreeView::items_window`]) lays out only the
    /// band its viewport shows — see [`crate::behaviour::row_window`].
    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> Self::Layout {
        let live = crate::widget_state::with_widget_state_ref(Some(&self.id), |state| {
            let state = state.and_then(|s| s.downcast_ref::<TreeViewState>());
            match self.live(state) {
                std::borrow::Cow::Owned(live) => Some(live),
                std::borrow::Cow::Borrowed(_) => None,
            }
        });
        let live = live.as_ref().unwrap_or(self);
        // The paint state's `scroll_px` shifts the rows and the thumb
        // exactly as the painters shift them, and wins over the
        // descriptor's own.
        let scroll_px = ctx.paint_state().scroll_px().or(live.scroll_px);
        match live.items_window {
            None => live.layout_at(ctx, rect, scroll_px),
            Some(window) => live.windowed_layout(ctx, rect, scroll_px, window),
        }
    }

    fn paint(
        &self,
        layout: &Self::Layout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        let style = env.style();
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            let live = self.live(env.widget_state_as::<TreeViewState>());
            let (painted, band_start, scroll_px) = tree_paint_target(&live, layout, env);
            let pointer = TreePointer::from_env(painted, env, band_start);
            native_surface_paint::paint_with_scroll_px(
                painted,
                layout.area,
                &layout.layout,
                layout.line_height,
                layout.nerd_fonts_enabled,
                surface,
                theme,
                style,
                pointer.hovered_row,
                pointer.pressed_row,
                env.state.focus_ring_visible(),
                scroll_px,
            );
            if layout.window.is_some() {
                if let (Some(sb), Some(sb_layout)) = (&layout.vscrollbar, &layout.vscrollbar_layout)
                {
                    crate::primitive::Primitive::paint(sb, sb_layout, surface, env);
                }
            }
        }
        // A `tui`-only build has no `PaintSurface` to paint into at all —
        // `self` reaching the screen on TUI goes through `Self::paint_cells`
        // instead, which `DrawsPrimitives::draw_primitive`'s default body
        // calls in place of this method whenever `Backend::cell_surface`
        // answers `Some`.
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        {
            let _ = (layout, surface, theme, style);
        }
    }

    /// Paints the rows through the cell-grid rasteriser in
    /// [`paint_cells`] into the row area, then the reserved scrollbar
    /// column through [`Scrollbar`]'s own `Primitive`.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &Self::Layout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        let live = self.live(env.widget_state_as::<TreeViewState>());
        let (painted, _, scroll_px) = tree_paint_target(&live, layout, env);
        paint_cells::paint_tree_rows_with_scroll_px(
            cells,
            crate::tui::backend::q_rect_to_ratatui(layout.area),
            painted,
            theme,
            layout.nerd_fonts_enabled,
            scroll_px,
        );
        if let (Some(sb), Some(sb_layout)) = (&layout.vscrollbar, &layout.vscrollbar_layout) {
            crate::primitive::Primitive::paint_cells(sb, sb_layout, cells, env);
        }
    }
}

/// What a tree paints for `layout`: a windowed tree's band (with its first
/// absolute row) or `live`, and the sub-row scroll it paints at.
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
fn tree_paint_target<'a>(
    live: &'a TreeView,
    layout: &'a TreeViewPrimitiveLayout,
    env: &crate::PaintEnv,
) -> (&'a TreeView, usize, Option<f32>) {
    match &layout.window {
        Some(w) => (w.live.as_ref(), w.band.start, w.scroll_px),
        None => (live, 0, env.state.scroll_px().or(live.scroll_px)),
    }
}

/// The pointer-driven slice of a tree's paint state, decoded from a
/// [`crate::PaintEnv`]'s hover and press for one tree, falling back to
/// the caller's own [`crate::primitive::PaintState`] when the runtime
/// reports nothing for it.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
struct TreePointer {
    hovered_row: Option<usize>,
    pressed_row: Option<usize>,
}

#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
impl TreePointer {
    fn from_env(tree: &TreeView, env: &crate::PaintEnv, band_start: usize) -> Self {
        let in_range = |i: usize| {
            i.checked_sub(band_start)
                .filter(|rel| *rel < tree.rows.len())
        };
        let row_of = |sub: Option<crate::interactive::SubTarget>| {
            sub.and_then(TreeViewSubPart::from_sub_target)
                .and_then(TreeViewSubPart::row)
                .and_then(in_range)
        };
        Self {
            hovered_row: row_of(env.hovered_sub(&tree.id)).or(env.state.hovered_idx()),
            pressed_row: row_of(env.pressed_sub(&tree.id)).or(env.state.pressed_idx()),
        }
    }
}

// ── Interactive: keyboard nav, typeahead, wheel, chevrons, rename ────
//
// Controlled mode: `selected_path`, `scroll_offset` and every row's
// `edit` stay descriptor fields the app owns. `handle` computes the next
// value from the descriptor and reports it as a `TreeEvent`; the app
// applies it before the next frame. `TreeViewState` holds only gesture
// data: the navigation scratch the core computes in, the typeahead
// buffer, a thumb drag and the multi-selection anchor.

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
impl crate::interactive::WidgetAction for TreeEvent {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        crate::UiEvent::Tree(id, self)
    }
}

/// [`crate::interactive::Interactive::State`] for [`TreeView`].
///
/// `nav` is the navigation core every move is computed in; it is
/// re-seeded from the tree's resolved selection and scroll
/// ([`TreeView::live`]) whenever those differ from what it last produced.
/// Every move is latched into [`Self::selected`]/[`Self::scroll`], so the
/// descriptor wins only when the app changes it. Its pixel-precise wheel
/// position survives as long as the resolved whole-row offset stays the
/// one it reported.
///
/// `#[non_exhaustive]`: a future gesture field is additive.
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[non_exhaustive]
#[derive(Debug, Default)]
pub struct TreeViewState {
    /// Selection/scroll scratch and the scrollbar thumb drag.
    pub nav: crate::behaviour::nav::TreeNavCore,
    /// The debounced "type to jump" buffer.
    pub typeahead: crate::behaviour::typeahead::Typeahead,
    /// The multi-selection a Ctrl/Cmd or Shift gesture grew, as indices
    /// into `TreeView.rows`, and the row a Shift range extends from.
    pub selection: crate::types::SelectionCore,
    /// The selection the last move landed on, against the descriptor's
    /// `selected_path` — see [`crate::behaviour::row_window::HostLatch`].
    pub selected: crate::behaviour::row_window::HostLatch<Option<TreePath>>,
    /// The scroll row the last move or scroll landed on, against the
    /// descriptor's `scroll_offset`.
    pub scroll: crate::behaviour::row_window::HostLatch<usize>,
    /// A windowed tree's cursor, as an absolute row index — it may sit on
    /// a row that is not loaded yet. [`TreeView::live`] trusts it only
    /// while [`Self::window_cursor_anchor`] still matches the descriptor.
    pub window_cursor: Option<usize>,
    /// What the descriptor and the cursor's row said when the widget last
    /// handled input, which decides whether [`Self::window_cursor`] still
    /// stands.
    pub window_cursor_anchor: Option<WindowCursorAnchor>,
    /// Each branch row's expansion the widget toggled, against the
    /// descriptor's `is_expanded` for that row. Applied to an unwindowed
    /// tree only: a windowed tree's rows are flattened by the app.
    pub expanded:
        std::collections::BTreeMap<TreePath, crate::behaviour::row_window::HostLatch<bool>>,
    /// Repeat suppression for a windowed tree's
    /// [`crate::UiEvent::RowsRequested`].
    pub rows_requested: crate::behaviour::row_window::RowsRequestLatch,
    /// The descriptor `selected_path` the last [`Interactive::settle`]
    /// saw: a new one is the app moving the selection, which scrolls it
    /// into view.
    ///
    /// [`Interactive::settle`]: crate::interactive::Interactive::settle
    pub followed_selection: Option<Option<TreePath>>,
}

/// A windowed tree's cursor anchor: the descriptor `selected_path` the
/// cursor was set against, and the path of the row it sat on (`None`
/// when that row was not loaded).
///
/// The cursor stands while the descriptor still says `seen` or names the
/// cursor's own row (the app echoed the move). A descriptor that changed
/// to another path wins, the way [`crate::behaviour::row_window::HostLatch`]
/// lets an app's own new value win. When the row under the cursor is no
/// longer `path` (the app re-flattened the rows around it), the cursor
/// follows `path` to its new loaded position.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowCursorAnchor {
    /// The descriptor's `selected_path` when the cursor last moved.
    pub seen: Option<TreePath>,
    /// The path of the loaded row the cursor sat on.
    pub path: Option<TreePath>,
}

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
impl TreeViewState {
    /// Re-seed the navigation core from `tree`'s resolved fields — the
    /// descriptor [`TreeView::live`] produced from this state.
    fn sync(&mut self, tree: &TreeView) {
        if tree.selected_path.is_some() && self.nav.selected_path() != tree.selected_path.as_ref() {
            self.nav.set_selected_path(tree.selected_path.clone());
        }
        if self.nav.scroll_offset() != tree.scroll_offset {
            self.nav.set_scroll_offset(tree.scroll_offset);
        }
    }
}

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
impl TreeView {
    /// This tree with `state`'s resolved selection, scroll and (when
    /// unwindowed) expansion applied — the descriptor it lays out, paints
    /// and handles input as. A collapsed branch's descendants are dropped
    /// from the rows, since a row's `is_expanded` drives only its own
    /// chevron.
    pub fn live(&self, state: Option<&TreeViewState>) -> std::borrow::Cow<'_, Self> {
        let Some(state) = state else {
            return std::borrow::Cow::Borrowed(self);
        };
        let scroll = state.scroll.get(&self.scroll_offset);
        let selected = match self.items_window {
            Some(_) => self
                .resolved_window_cursor(state)
                .and_then(|abs| self.loaded_row(abs))
                .map(|row| Some(row.path.clone()))
                .unwrap_or_else(|| state.selected.get(&self.selected_path)),
            None => state.selected.get(&self.selected_path),
        };
        let expansion_differs = self.items_window.is_none()
            && self.rows.iter().any(
                |row| match (row.is_expanded, state.expanded.get(&row.path)) {
                    (Some(desc), Some(latch)) => latch.get(&desc) != desc,
                    _ => false,
                },
            );
        if scroll == self.scroll_offset && selected == self.selected_path && !expansion_differs {
            return std::borrow::Cow::Borrowed(self);
        }
        let mut live = Self {
            scroll_offset: scroll,
            selected_path: selected,
            ..self.clone()
        };
        if expansion_differs {
            for row in &mut live.rows {
                if let (Some(desc), Some(latch)) = (row.is_expanded, state.expanded.get(&row.path))
                {
                    row.is_expanded = Some(latch.get(&desc));
                }
            }
            let collapsed: Vec<TreePath> = live
                .rows
                .iter()
                .filter(|row| row.is_expanded == Some(false))
                .map(|row| row.path.clone())
                .collect();
            live.rows.retain(|row| {
                !collapsed.iter().any(|ancestor| {
                    row.path.len() > ancestor.len() && row.path.starts_with(ancestor)
                })
            });
        }
        std::borrow::Cow::Owned(live)
    }

    /// How many rows the real collection has: the window's total when
    /// [`Self::items_window`] is set, else [`Self::rows`]' length.
    pub fn row_count(&self) -> usize {
        match self.items_window {
            Some(w) => w.row_count(self.rows.len()),
            None => self.rows.len(),
        }
    }

    /// A windowed tree's cursor row against this descriptor — see
    /// [`WindowCursorAnchor`] for when the stored cursor stands. Without
    /// one, the descriptor's `selected_path` at its loaded position.
    /// `None` for an unwindowed tree.
    fn resolved_window_cursor(&self, state: &TreeViewState) -> Option<usize> {
        let window = self.items_window?;
        let loaded_at = |path: &TreePath| {
            self.rows
                .iter()
                .position(|row| &row.path == path)
                .map(|i| window.start + i)
        };
        let from_descriptor = || self.selected_path.as_ref().and_then(loaded_at);
        let Some(cursor) = state.window_cursor else {
            return from_descriptor();
        };
        let anchor = state.window_cursor_anchor.as_ref();
        let row_path = self.loaded_row(cursor).map(|row| &row.path);
        if row_path.is_some() && row_path == self.selected_path.as_ref() {
            return Some(cursor);
        }
        if anchor.is_some_and(|a| a.seen != self.selected_path) {
            return from_descriptor();
        }
        match (anchor.and_then(|a| a.path.as_ref()), row_path) {
            (Some(was), Some(now)) if was != now => loaded_at(was).or_else(from_descriptor),
            _ => Some(cursor),
        }
    }

    /// The loaded row at absolute index `abs` of a windowed tree (or row
    /// `abs` of an unwindowed one), `None` for a row not loaded.
    fn loaded_row(&self, abs: usize) -> Option<&TreeRow> {
        match self.items_window {
            Some(window) => window
                .loaded_index(self.rows.len(), abs)
                .map(|i| &self.rows[i]),
            None => self.rows.get(abs),
        }
    }

    /// Latch what `handle` just did against the descriptor `desc`: the
    /// navigation core's selection and scroll, and a toggled branch's
    /// expansion.
    fn latch(&self, desc: &TreeView, state: &mut TreeViewState, out: &Handled<TreeEvent>) {
        let selected = state.nav.selected_path().cloned();
        if selected.is_some() && selected != self.selected_path {
            state.selected.set(&desc.selected_path, selected);
        }
        let scroll = state.nav.scroll_offset();
        if scroll != self.scroll_offset {
            state.scroll.set(&desc.scroll_offset, scroll);
        }
        if let Handled::Event(TreeEvent::RowToggleExpand { path }) = out {
            desc.toggle_expanded(state, path);
        }
    }

    /// Flip branch `path`'s expansion in `state`, against this
    /// descriptor's own `is_expanded` for it, and drop the latches of
    /// paths that are no longer branches. A windowed tree's rows are
    /// flattened by the app, so it keeps no expansion of its own.
    fn toggle_expanded(&self, state: &mut TreeViewState, path: &TreePath) {
        if self.items_window.is_some() {
            return;
        }
        if let Some(Some(desc_expanded)) = self
            .rows
            .iter()
            .find(|row| &row.path == path)
            .map(|row| row.is_expanded)
        {
            let latch = state.expanded.entry(path.clone()).or_default();
            let current = latch.get(&desc_expanded);
            latch.set(&desc_expanded, !current);
        }
        let branches: std::collections::BTreeSet<&TreePath> = self
            .rows
            .iter()
            .filter(|row| row.is_expanded.is_some())
            .map(|row| &row.path)
            .collect();
        state.expanded.retain(|path, _| branches.contains(path));
    }

    /// One windowed-tree navigation key: the cursor moves over the real
    /// extent by absolute index. A loaded target reports its path; an
    /// unloaded one only moves the cursor, and the tree asks for its rows
    /// through [`crate::interactive::Interactive::data_requested`].
    fn handle_windowed_key(
        &self,
        state: &mut TreeViewState,
        event: &crate::interactive::KeyEvent,
        layout: &TreeViewPrimitiveLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> Handled<TreeEvent> {
        use crate::behaviour::nav::{FlatNav, NavKey};
        use crate::event::{Key, NamedKey};
        use crate::interactive::WidgetAction;
        let page = layout.viewport_rows.max(1);
        let vim = self.style.vim_keys
            && !(event.modifiers.ctrl || event.modifiers.alt || event.modifiers.cmd);
        let nav = match event.key {
            Key::Char('j') if vim => Some(NavKey::Down),
            Key::Char('k') if vim => Some(NavKey::Up),
            _ => NavKey::from_key(event).map(|nav| match nav {
                NavKey::PageUp(_) => NavKey::PageUp(page.saturating_sub(1).max(1)),
                NavKey::PageDown(_) => NavKey::PageDown(page.saturating_sub(1).max(1)),
                other => other,
            }),
        };
        let cursor = state.window_cursor;
        if let Some(nav) = nav {
            let Some(target) = FlatNav::mv_all(self.row_count(), cursor, nav, false) else {
                return Handled::Ignored;
            };
            state.window_cursor = Some(target);
            let scroll = state.nav.scroll_offset();
            state
                .nav
                .set_scroll_offset(crate::behaviour::scroll::ensure_visible(
                    scroll, page, target,
                ));
            return match self.loaded_row(target) {
                Some(row) => {
                    state.nav.set_selected_path(Some(row.path.clone()));
                    Handled::Event(TreeEvent::SelectionChanged {
                        path: row.path.clone(),
                    })
                }
                None => Handled::Consumed,
            };
        }
        let row = cursor.and_then(|abs| self.loaded_row(abs));
        match (&event.key, row) {
            (Key::Named(NamedKey::Enter), Some(row)) => Handled::Event(TreeEvent::RowActivated {
                path: row.path.clone(),
            }),
            (Key::Named(NamedKey::Left), Some(row)) if row.is_expanded == Some(true) => {
                Handled::Event(TreeEvent::RowToggleExpand {
                    path: row.path.clone(),
                })
            }
            (Key::Named(NamedKey::Right), Some(row)) if row.is_expanded == Some(false) => {
                Handled::Event(TreeEvent::RowToggleExpand {
                    path: row.path.clone(),
                })
            }
            (key, _) => {
                cx.emit(
                    TreeEvent::KeyPressed {
                        key: crate::primitives::list::key_to_event_string(key),
                        modifiers: event.modifiers,
                    }
                    .into_ui_event(cx.id),
                );
                Handled::Ignored
            }
        }
    }

    /// The row currently in the edit (rename) session, if any.
    fn editing_row(&self) -> Option<(usize, &TreeRowEditState)> {
        self.rows
            .iter()
            .enumerate()
            .find_map(|(i, r)| r.edit.as_ref().map(|e| (i, e)))
    }

    /// Report a scroll offset change, or just a redraw when it did not
    /// move.
    fn scroll_outcome(&self, before: usize, state: &TreeViewState) -> Handled<TreeEvent> {
        let after = state.nav.scroll_offset();
        if after == before {
            Handled::Consumed
        } else {
            Handled::Event(TreeEvent::ScrollChanged {
                scroll_offset: after,
            })
        }
    }

    /// Report a selection move the navigation core resolved.
    fn nav_outcome(&self, state: &mut TreeViewState, outcome: NavOutcome) -> Handled<TreeEvent> {
        match outcome {
            NavOutcome::Empty => Handled::Ignored,
            NavOutcome::Unchanged => Handled::Consumed,
            NavOutcome::Selected(path) => {
                if let Some(idx) = state.nav.selected_row_index(&self.rows) {
                    state
                        .selection
                        .on_gesture(None, idx, crate::types::SelectionGesture::Replace);
                }
                Handled::Event(TreeEvent::SelectionChanged { path })
            }
        }
    }

    /// Select `path` (already a row of `self.rows`) and scroll it into
    /// view.
    fn select_path(
        &self,
        state: &mut TreeViewState,
        path: TreePath,
        layout: &TreeViewPrimitiveLayout,
    ) -> Handled<TreeEvent> {
        let Some(idx) = self.rows.iter().position(|r| r.path == path) else {
            return Handled::Ignored;
        };
        state.nav.set_selected_path(Some(path.clone()));
        state.nav.scroll_to_visible(idx, layout.viewport_rows);
        state
            .selection
            .on_gesture(None, idx, crate::types::SelectionGesture::Replace);
        Handled::Event(TreeEvent::SelectionChanged { path })
    }

    /// One keystroke while a row's rename session is open: the shared
    /// text editor applies it to the row's [`TreeRowEditState`].
    fn handle_rename_key(
        &self,
        idx: usize,
        edit: &TreeRowEditState,
        key: &crate::event::Key,
        modifiers: Modifiers,
    ) -> Handled<TreeEvent> {
        let path = self.rows[idx].path.clone();
        match tree_rename_key(edit, key, modifiers) {
            TreeRenameKey::Commit => Handled::Event(TreeEvent::RowRenameCommitted {
                path,
                text: edit.text.clone(),
            }),
            TreeRenameKey::Cancel => Handled::Event(TreeEvent::RowRenameCancelled { path }),
            TreeRenameKey::Edited(next) | TreeRenameKey::Moved(next) => {
                Handled::Event(TreeEvent::RowRenameChanged {
                    path,
                    text: next.text,
                    cursor: next.cursor,
                    selection_anchor: next.selection_anchor,
                })
            }
            // A rename session swallows every other key, so Tab or a
            // letter never escapes into tree navigation mid-edit.
            TreeRenameKey::Unhandled => Handled::Consumed,
        }
    }

    /// One idle (no rename session) keystroke.
    fn handle_nav_key(
        &self,
        state: &mut TreeViewState,
        event: &crate::interactive::KeyEvent,
        layout: &TreeViewPrimitiveLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> Handled<TreeEvent> {
        use crate::behaviour::nav::{HorizontalNav, TreeKey};
        use crate::event::Key;
        use crate::interactive::WidgetAction;

        let no_accel = !(event.modifiers.ctrl || event.modifiers.alt || event.modifiers.cmd);
        match state
            .nav
            .handle_key(&self.rows, event, layout.viewport_rows, self.style.vim_keys)
        {
            TreeKey::Moved(outcome) => return self.nav_outcome(state, outcome),
            TreeKey::Horizontal(nav) => {
                return match nav {
                    HorizontalNav::Collapse(path) | HorizontalNav::Expand(path) => {
                        Handled::Event(TreeEvent::RowToggleExpand { path })
                    }
                    HorizontalNav::SelectParent(path) | HorizontalNav::SelectFirstChild(path) => {
                        self.select_path(state, path, layout)
                    }
                    HorizontalNav::None => Handled::Consumed,
                }
            }
            TreeKey::Activate(Some(path)) => {
                return Handled::Event(TreeEvent::RowActivated { path })
            }
            TreeKey::Activate(None) => return Handled::Ignored,
            TreeKey::ToggleExpand(Some(path)) => {
                return Handled::Event(TreeEvent::RowToggleExpand { path })
            }
            TreeKey::ToggleExpand(None) => return Handled::Consumed,
            TreeKey::Unhandled => {}
        }
        match &event.key {
            Key::Char(c) if no_accel && !c.is_control() => {
                let from = state.nav.selected_row_index(&self.rows).unwrap_or(0);
                let labels: Vec<String> = self
                    .rows
                    .iter()
                    .map(|r| r.text.spans.iter().map(|s| s.text.as_str()).collect())
                    .collect();
                let hit = state
                    .typeahead
                    .push_now(*c, |i| labels[i].as_str(), labels.len(), from);
                match hit {
                    Some(idx) if Some(idx) != state.nav.selected_row_index(&self.rows) => {
                        self.select_path(state, self.rows[idx].path.clone(), layout)
                    }
                    Some(_) => Handled::Consumed,
                    None => Handled::Consumed,
                }
            }
            key => {
                cx.emit(
                    TreeEvent::KeyPressed {
                        key: crate::primitives::list::key_to_event_string(key),
                        modifiers: event.modifiers,
                    }
                    .into_ui_event(cx.id),
                );
                Handled::Ignored
            }
        }
    }

    /// A press of `button` on a row body, resolved by
    /// [`crate::SelectionCore::press`]: a left click selects, Ctrl/Cmd and
    /// Shift grow a multi-selection when the descriptor allows one, a
    /// right click asks for the context menu and any other button reports
    /// [`TreeEvent::RowClicked`] with that button.
    fn handle_row_press(
        &self,
        state: &mut TreeViewState,
        idx: usize,
        button: crate::event::MouseButton,
        mods: Modifiers,
        position: crate::event::Point,
    ) -> Handled<TreeEvent> {
        let path = self.rows[idx].path.clone();
        let current = state.nav.selected_row_index(&self.rows).or(Some(idx));
        let press = state
            .selection
            .press(self.selection_mode, current, idx, button, mods);
        if button == crate::event::MouseButton::Left {
            state.nav.set_selected_path(Some(path.clone()));
        }
        Handled::Event(TreeEvent::from_row_press(
            press,
            path,
            mods,
            position,
            |indices| selected_paths_for_indices(&self.rows, indices),
        ))
    }

    /// Left press on the scrollbar track: the thumb starts a captured
    /// drag, either side of it pages.
    fn handle_scrollbar_press(
        &self,
        state: &mut TreeViewState,
        sb: &Scrollbar,
        y: f32,
        layout: &TreeViewPrimitiveLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> Handled<TreeEvent> {
        use crate::behaviour::nav::ScrollbarPress;
        let before = state.nav.scroll_offset();
        match state.nav.press_scrollbar(
            sb.press_zone(y),
            y,
            sb.travel(),
            self.row_count(),
            layout.viewport_rows,
        ) {
            ScrollbarPress::DragStarted => {
                cx.capture_pointer();
                Handled::Consumed
            }
            ScrollbarPress::Paged => self.scroll_outcome(before, state),
            ScrollbarPress::Ignored => Handled::Consumed,
        }
    }
}

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
impl crate::interactive::Interactive for TreeView {
    type State = TreeViewState;
    type Action = TreeEvent;

    const SCROLLS_OWN_ROWS: bool = true;

    /// One focus stop as a whole — rows are navigated inside
    /// [`Self::handle`]. An empty tree has nothing to land on.
    fn focus(&self) -> crate::interactive::FocusSpec {
        if self.row_count() == 0 {
            crate::interactive::FocusSpec::None
        } else {
            crate::interactive::FocusSpec::Stop
        }
    }

    /// Every row's chevron and body, in the order
    /// [`TreeViewLayout::hit_test`] resolves them, then the scrollbar.
    fn sub_regions(&self, layout: &Self::Layout) -> Vec<(Rect, crate::interactive::SubTarget)> {
        // `layout.area` and the widget rect share their origin, so the
        // tree-local hit regions are already widget-local; only the
        // absolute scrollbar track needs translating.
        let mut out: Vec<(Rect, crate::interactive::SubTarget)> = layout
            .layout
            .hit_regions
            .iter()
            .filter_map(|(r, hit)| {
                let part = match hit {
                    TreeViewHit::Row(i) => TreeViewSubPart::Row(layout.abs_row(*i)),
                    TreeViewHit::Chevron(i) => TreeViewSubPart::Chevron(layout.abs_row(*i)),
                    TreeViewHit::Empty => return None,
                };
                Some((*r, part.sub_target()))
            })
            .collect();
        if let Some(sb) = &layout.vscrollbar {
            out.push((
                Rect::new(
                    sb.track.x - layout.area.x,
                    sb.track.y - layout.area.y,
                    sb.track.width,
                    sb.track.height,
                ),
                TreeViewSubPart::Scrollbar.sub_target(),
            ));
        }
        out
    }

    /// Handles `event` as the tree's resolved descriptor
    /// ([`TreeView::live`]), then latches the move against `self`.
    fn handle(
        &self,
        state: &mut TreeViewState,
        event: &crate::interactive::RoutedEvent,
        layout: &Self::Layout,
        cx: &mut crate::interactive::EventCx,
    ) -> Handled<TreeEvent> {
        if self.items_window.is_some() {
            state.window_cursor = self.resolved_window_cursor(state);
        }
        let live = self.live(Some(state));
        state.sync(&live);
        let out = live.handle_live(state, event, layout, cx);
        live.latch(self, state, &out);
        if self.items_window.is_some() {
            state.window_cursor_anchor = Some(WindowCursorAnchor {
                seen: self.selected_path.clone(),
                path: state
                    .window_cursor
                    .and_then(|abs| self.loaded_row(abs))
                    .map(|row| row.path.clone()),
            });
        }
        out
    }

    /// A windowed tree asks for the rows around its viewport once it
    /// scrolls or moves its cursor outside the loaded slice — see
    /// [`crate::behaviour::row_window::RowsRequestLatch`].
    fn data_requested(
        &self,
        state: &mut TreeViewState,
        layout: &TreeViewPrimitiveLayout,
    ) -> Option<crate::UiEvent> {
        let window = layout.window.as_ref()?;
        let scroll = match state.window_cursor {
            // A cursor outside the viewport asks for the rows around it.
            Some(cursor)
                if cursor < window.scroll || cursor >= window.scroll + window.viewport_rows =>
            {
                cursor
            }
            _ => window.scroll,
        };
        state.rows_requested.request(
            self.id,
            window.window(),
            window.loaded.1,
            scroll,
            window.viewport_rows,
        )
    }

    /// A selection the app moved (a new descriptor `selected_path`) that
    /// lies outside the viewport scrolls into view, the way a selection
    /// the tree's own keys moved does — so an app that owns the selection
    /// but leaves the scroll to the tree keeps its selection visible.
    fn settle(&self, state: &mut TreeViewState, layout: &TreeViewPrimitiveLayout) -> bool {
        if state.followed_selection.as_ref() == Some(&self.selected_path) {
            return false;
        }
        state.followed_selection = Some(self.selected_path.clone());
        let Some(path) = &self.selected_path else {
            return false;
        };
        let live = self.live(Some(state));
        if live.selected_path.as_ref() != Some(path) {
            return false;
        }
        let Some(row) = live.rows.iter().position(|r| &r.path == path) else {
            return false;
        };
        let row = row + self.items_window.map_or(0, |w| w.start);
        state
            .scroll
            .follow_selection(self.scroll_offset, layout.viewport_rows, row)
    }

    /// A `RowToggleExpand` for this tree the host delivered on the app's
    /// behalf toggles that branch exactly as a chevron click would.
    fn follow_event(&self, state: &mut TreeViewState, event: &crate::UiEvent) {
        if let crate::UiEvent::Tree(id, TreeEvent::RowToggleExpand { path }) = event {
            if *id == self.id {
                self.toggle_expanded(state, path);
            }
        }
    }

    /// This widget scrolled so `row` is its first visible row.
    fn scrolled_to_row(&self, row: usize) -> Option<Self> {
        Some(Self {
            scroll_offset: row,
            ..self.clone()
        })
    }

    fn state_summary(&self, state: &TreeViewState) -> crate::interactive::StateSummary {
        let live = self.live(Some(state));
        let resolves_expansion =
            self.items_window.is_none() && self.rows.iter().any(|row| row.is_expanded.is_some());
        let expanded = resolves_expansion.then(|| {
            live.rows
                .iter()
                .filter(|row| row.is_expanded == Some(true))
                .map(|row| row.path.clone())
                .collect()
        });
        crate::interactive::StateSummary::scroll(live.scroll_offset)
            .with_selected_path(live.selected_path.clone())
            .with_expanded(expanded)
    }

    fn a11y_actions(&self, _state: &TreeViewState) -> Vec<crate::a11y::A11yAction> {
        use crate::a11y::A11yAction;
        if self.rows.is_empty() {
            Vec::new()
        } else {
            vec![
                A11yAction::Click,
                A11yAction::Focus,
                A11yAction::Expand,
                A11yAction::Collapse,
                A11yAction::ScrollIntoView,
            ]
        }
    }
}

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
impl TreeView {
    /// [`crate::interactive::Interactive::handle`] for the resolved
    /// descriptor `self`, with `state`'s navigation core already seeded
    /// from it.
    fn handle_live(
        &self,
        state: &mut TreeViewState,
        event: &crate::interactive::RoutedEvent,
        layout: &TreeViewPrimitiveLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> Handled<TreeEvent> {
        use crate::a11y::A11yAction;
        use crate::event::MouseButton;
        use crate::interactive::{KeyEvent, PointerKind, RoutedEvent};

        let abs = |local: &crate::event::Point| (local.x + layout.area.x, local.y + layout.area.y);
        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                local,
                mods,
                ..
            } => {
                let (x, y) = abs(local);
                cx.request_focus(None);
                if let Some(sb) = layout.on_scrollbar(x, y) {
                    let sb = sb.clone();
                    return self.handle_scrollbar_press(state, &sb, y, layout, cx);
                }
                if self.items_window.is_some() {
                    return self.handle_windowed_press(state, layout.hit_test(x, y), layout, *mods);
                }
                match layout.hit_test(x, y) {
                    TreeViewHit::Row(idx) => self.handle_row_press(
                        state,
                        idx,
                        MouseButton::Left,
                        *mods,
                        crate::event::Point::new(x, y),
                    ),
                    TreeViewHit::Chevron(idx) => Handled::Event(TreeEvent::RowToggleExpand {
                        path: self.rows[idx].path.clone(),
                    }),
                    TreeViewHit::Empty => Handled::Consumed,
                }
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Down(button),
                local,
                mods,
                ..
            } => {
                let (x, y) = abs(local);
                let position = crate::event::Point::new(x, y);
                if self.items_window.is_some() {
                    // A windowed tree has no multi-selection, so the press
                    // only names its loaded row.
                    return match self.hit_row(layout, x, y) {
                        Some(row) => {
                            let press = state.selection.press(
                                SelectionMode::Single,
                                None,
                                0,
                                *button,
                                *mods,
                            );
                            Handled::Event(TreeEvent::from_row_press(
                                press,
                                row.path.clone(),
                                *mods,
                                position,
                                |_| Vec::new(),
                            ))
                        }
                        None => Handled::Ignored,
                    };
                }
                match layout.hit_test(x, y) {
                    TreeViewHit::Row(idx) | TreeViewHit::Chevron(idx) => {
                        self.handle_row_press(state, idx, *button, *mods, position)
                    }
                    TreeViewHit::Empty => Handled::Ignored,
                }
            }
            RoutedEvent::Pointer {
                kind: PointerKind::DoubleClick(MouseButton::Left),
                local,
                ..
            } => {
                let (x, y) = abs(local);
                match self.hit_row(layout, x, y) {
                    Some(row) => Handled::Event(TreeEvent::RowDoubleClicked {
                        path: row.path.clone(),
                    }),
                    None => Handled::Ignored,
                }
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Move,
                local,
                ..
            } if state.nav.is_dragging() => {
                let (_, y) = abs(local);
                match state.nav.drag_to(y) {
                    Some(scroll_offset) => {
                        Handled::Event(TreeEvent::ScrollChanged { scroll_offset })
                    }
                    None => Handled::Consumed,
                }
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Up(MouseButton::Left),
                ..
            } if state.nav.is_dragging() => {
                state.nav.end_scrollbar_drag();
                cx.release_pointer();
                Handled::Consumed
            }
            RoutedEvent::Wheel { dy, .. } => {
                let before = state.nav.scroll_offset();
                state.nav.scroll_by_wheel(
                    *dy,
                    layout.row_pitch,
                    self.row_count(),
                    layout.viewport_rows,
                );
                self.scroll_outcome(before, state)
            }
            RoutedEvent::Key(key_event @ KeyEvent { key, modifiers, .. }) => {
                match self.editing_row() {
                    Some((idx, edit)) => self.handle_rename_key(idx, edit, key, *modifiers),
                    None => {
                        let before = state.nav.scroll_offset();
                        let handled = if self.items_window.is_some() {
                            self.handle_windowed_key(state, key_event, layout, cx)
                        } else {
                            self.handle_nav_key(state, key_event, layout, cx)
                        };
                        // A move that scrolled the selection into view
                        // reports the new offset alongside the selection,
                        // so the app's controlled `scroll_offset` follows.
                        let after = state.nav.scroll_offset();
                        if after != before {
                            cx.emit(crate::UiEvent::Tree(
                                self.id,
                                TreeEvent::ScrollChanged {
                                    scroll_offset: after,
                                },
                            ));
                        }
                        handled
                    }
                }
            }
            RoutedEvent::Text(text) => match self.editing_row() {
                Some((idx, edit)) => {
                    let next = tree_rename_insert(edit, text);
                    Handled::Event(TreeEvent::RowRenameChanged {
                        path: self.rows[idx].path.clone(),
                        text: next.text,
                        cursor: next.cursor,
                        selection_anchor: next.selection_anchor,
                    })
                }
                None => Handled::Ignored,
            },
            // `Click` is the tree's "open" action, the same
            // `RowDoubleClicked` the assistive-technology router resolves
            // a row's default action to.
            RoutedEvent::Action(A11yAction::Click) => match state.nav.activate() {
                Some(path) => Handled::Event(TreeEvent::RowDoubleClicked { path }),
                None => Handled::Ignored,
            },
            RoutedEvent::Action(action @ (A11yAction::Expand | A11yAction::Collapse)) => {
                let want_expanded = *action == A11yAction::Expand;
                let row = state
                    .nav
                    .selected_row_index(&self.rows)
                    .map(|i| &self.rows[i]);
                match row {
                    Some(row) if row.is_expanded == Some(!want_expanded) => {
                        Handled::Event(TreeEvent::RowToggleExpand {
                            path: row.path.clone(),
                        })
                    }
                    Some(_) => Handled::Consumed,
                    None => Handled::Ignored,
                }
            }
            RoutedEvent::Action(A11yAction::Focus) => {
                cx.request_focus(None);
                Handled::Consumed
            }
            RoutedEvent::Action(A11yAction::ScrollIntoView) => {
                let Some(idx) = state.nav.selected_row_index(&self.rows) else {
                    return Handled::Ignored;
                };
                let before = state.nav.scroll_offset();
                state.nav.scroll_to_visible(idx, layout.viewport_rows);
                self.scroll_outcome(before, state)
            }
            _ => Handled::Ignored,
        }
    }
}

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
impl TreeView {
    /// The row painted under absolute `(x, y)` — through the loaded slice
    /// for a windowed tree, so a placeholder never resolves to a row.
    fn hit_row(&self, layout: &TreeViewPrimitiveLayout, x: f32, y: f32) -> Option<&TreeRow> {
        match layout.hit_test(x, y) {
            TreeViewHit::Row(idx) | TreeViewHit::Chevron(idx) => {
                self.loaded_row(layout.abs_row(idx))
            }
            TreeViewHit::Empty => None,
        }
    }

    /// A left press on a windowed tree's rows: a loaded row is clicked (or
    /// its chevron toggled) by path; a placeholder only takes focus.
    fn handle_windowed_press(
        &self,
        state: &mut TreeViewState,
        hit: TreeViewHit,
        layout: &TreeViewPrimitiveLayout,
        mods: Modifiers,
    ) -> Handled<TreeEvent> {
        let (idx, chevron) = match hit {
            TreeViewHit::Row(idx) => (idx, false),
            TreeViewHit::Chevron(idx) => (idx, true),
            TreeViewHit::Empty => return Handled::Consumed,
        };
        let abs = layout.abs_row(idx);
        let Some(row) = self.loaded_row(abs) else {
            return Handled::Consumed;
        };
        let path = row.path.clone();
        if chevron {
            return Handled::Event(TreeEvent::RowToggleExpand { path });
        }
        state.window_cursor = Some(abs);
        state.nav.set_selected_path(Some(path.clone()));
        Handled::Event(TreeEvent::RowClicked {
            path,
            button: crate::event::MouseButton::Left,
            modifiers: mods,
        })
    }
}

// ── Shared rename editing ────────────────────────────────────────────

/// What one keystroke does to an open rename session — the answer both
/// [`TreeView`]'s `Interactive::handle` and a host editing a row in
/// place act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeRenameKey {
    /// Enter: apply the edited text.
    Commit,
    /// Escape: discard the session.
    Cancel,
    /// The text changed; this is the session's next state.
    Edited(TreeRowEditState),
    /// Only the caret or selection moved; this is the session's next
    /// state.
    Moved(TreeRowEditState),
    /// The key does nothing to the session.
    Unhandled,
}

/// Resolve one keystroke against a rename session through the shared
/// text editor ([`crate::behaviour::text_edit`]): Enter commits, Escape
/// cancels, and every other key the editor understands is applied to
/// `edit` as a single-line edit.
pub fn tree_rename_key(
    edit: &TreeRowEditState,
    key: &crate::event::Key,
    modifiers: Modifiers,
) -> TreeRenameKey {
    use crate::behaviour::text_edit::{EditIntent, EditOp};
    use crate::event::{Key, NamedKey};
    match key {
        Key::Named(NamedKey::Enter) => return TreeRenameKey::Commit,
        Key::Named(NamedKey::Escape) => return TreeRenameKey::Cancel,
        _ => {}
    }
    let op = match EditIntent::from_key(key, modifiers) {
        Some(EditIntent::SelectAll) => EditOp::SelectAll,
        Some(EditIntent::Op(op)) => op,
        _ => return TreeRenameKey::Unhandled,
    };
    // A rename is one line: vertical motion has nowhere to go.
    if matches!(op, EditOp::MoveUp { .. } | EditOp::MoveDown { .. }) {
        return TreeRenameKey::Unhandled;
    }
    apply_rename_op(edit, op)
}

/// Insert `text` (typed IME text or a clipboard paste) at the rename
/// session's caret, replacing any selection.
pub fn tree_rename_insert(edit: &TreeRowEditState, text: &str) -> TreeRowEditState {
    use crate::behaviour::text_edit::EditOp;
    match apply_rename_op(edit, EditOp::InsertText(text.replace('\n', " "))) {
        TreeRenameKey::Edited(next) | TreeRenameKey::Moved(next) => next,
        _ => edit.clone(),
    }
}

fn apply_rename_op(
    edit: &TreeRowEditState,
    op: crate::behaviour::text_edit::EditOp,
) -> TreeRenameKey {
    let (text, cursor, selection_anchor) = crate::behaviour::text_edit::apply_flat_edit_op(
        &edit.text,
        edit.cursor,
        edit.selection_anchor,
        op,
    );
    let selection_anchor = selection_anchor.filter(|a| *a != cursor);
    let next = TreeRowEditState {
        text,
        cursor,
        selection_anchor,
        placeholder: edit.placeholder.clone(),
    };
    if next.text != edit.text {
        TreeRenameKey::Edited(next)
    } else if next.cursor != edit.cursor || next.selection_anchor != edit.selection_anchor {
        TreeRenameKey::Moved(next)
    } else {
        TreeRenameKey::Unhandled
    }
}

/// Events a `TreeView` emits back to the app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum TreeEvent {
    /// A press on a row with any button but the secondary (right) one,
    /// which asks for [`Self::ContextMenuRequested`] instead. `button`
    /// tells a plain (left) click from a middle or `X1`/`X2` one.
    RowClicked {
        path: TreePath,
        button: crate::event::MouseButton,
        modifiers: Modifiers,
    },
    /// Double-click on a row (typically "open" / "activate").
    RowDoubleClicked { path: TreePath },
    /// The chevron was clicked, or Space/arrow-keys expanded/collapsed a branch.
    RowToggleExpand { path: TreePath },
    /// Keyboard selection moved to a new row.
    SelectionChanged { path: TreePath },
    /// A key was pressed while the tree had focus and the primitive did not
    /// consume it. App may interpret it (e.g. `s` stages a file).
    KeyPressed { key: String, modifiers: Modifiers },
    /// The full multi-selection changed (`SelectionMode::Multi`): Ctrl/Cmd
    /// click toggled a row, Shift click/extend selected a range, or a
    /// keyboard extension grew the set. `paths` is the complete, de-duplicated
    /// selection, not just the row that changed, in the same visual order as
    /// `rows`. [`crate::types::apply_selection_gesture`] computes the
    /// selection as flattened-row indices; use [`selected_paths_for_indices`]
    /// to map those indices back to `TreePath`s before emitting this event.
    SelectionSet { paths: Vec<TreePath> },
    /// A keystroke was applied to the row at `path`'s [`TreeRow::edit`] —
    /// a character typed/deleted, or the cursor/selection moved. `text`,
    /// `cursor`, and `selection_anchor` are the new [`TreeRowEditState`]
    /// fields; an app's handler writes them back onto that row's `edit` so
    /// the next render paints the caret where this keystroke left it,
    /// mirroring `FormEvent::TextInputChanged`'s "fires every keystroke"
    /// shape but — unlike that event — carrying the cursor/selection too,
    /// since there is no separate per-widget cursor cache for a tree
    /// rename to fall back on.
    RowRenameChanged {
        path: TreePath,
        text: String,
        cursor: usize,
        selection_anchor: Option<usize>,
    },
    /// Enter was pressed while renaming the row at `path`: the app should
    /// apply `text` as the row's new name and clear `edit` back to `None`.
    RowRenameCommitted { path: TreePath, text: String },
    /// Escape was pressed while renaming the row at `path`: the app should
    /// discard the in-progress text and clear `edit` back to `None`
    /// without renaming anything.
    RowRenameCancelled { path: TreePath },
    /// Enter was pressed on the selected row (typically "open").
    RowActivated { path: TreePath },
    /// A wheel, scrollbar or keyboard move scrolled the viewport: the app
    /// should store `scroll_offset` as the tree's new
    /// [`TreeView::scroll_offset`].
    ScrollChanged { scroll_offset: usize },
    /// Right-click on a row. `position` is in the same window coordinates
    /// the pointer event carried, for anchoring the context menu.
    ContextMenuRequested {
        path: TreePath,
        position: crate::event::Point,
    },
}

impl TreeEvent {
    /// The event a press on the row at `path`, resolved by
    /// [`crate::SelectionCore::press`], reports. `paths` maps a
    /// multi-selection's row indices back to their paths (see
    /// [`selected_paths_for_indices`]); `position` is the press point.
    pub fn from_row_press(
        press: crate::RowPress,
        path: TreePath,
        modifiers: Modifiers,
        position: crate::event::Point,
        paths: impl FnOnce(&[usize]) -> Vec<TreePath>,
    ) -> Self {
        use crate::event::MouseButton;
        use crate::RowPress;
        match press {
            RowPress::Select => Self::RowClicked {
                path,
                button: MouseButton::Left,
                modifiers,
            },
            RowPress::Set(indices) => Self::SelectionSet {
                paths: paths(&indices),
            },
            RowPress::ContextMenu => Self::ContextMenuRequested { path, position },
            RowPress::Click(button) => Self::RowClicked {
                path,
                button,
                modifiers,
            },
        }
    }
}

/// Maps flattened-row indices (as produced by
/// [`crate::types::apply_selection_gesture`]) back to the `TreePath`s of
/// those rows, in `rows`' visual order.
///
/// `rows` must be the same pre-flattened, visual-order slice the indices
/// were computed against; out-of-range indices are skipped.
pub fn selected_paths_for_indices(rows: &[TreeRow], indices: &[usize]) -> Vec<TreePath> {
    indices
        .iter()
        .filter_map(|&idx| rows.get(idx).map(|row| row.path.clone()))
        .collect()
}

#[cfg(test)]
mod multi_select_tests {
    use super::{selected_paths_for_indices, Decoration, TreeEvent, TreeRow};
    use crate::types::{apply_selection_gesture, Modifiers, SelectionGesture, StyledText};

    fn leaf(idx: u16, label: &str) -> TreeRow {
        TreeRow {
            path: vec![idx],
            indent: 0,
            icon: None,
            text: StyledText::plain(label.to_string()),
            badge: None,
            is_expanded: None,
            decoration: Decoration::Normal,
            edit: None,
        }
    }

    #[test]
    fn shift_range_maps_flattened_indices_back_to_paths() {
        let rows = vec![leaf(0, "a"), leaf(1, "b"), leaf(2, "c"), leaf(3, "d")];
        let gesture = SelectionGesture::from_modifiers(Modifiers {
            shift: true,
            ..Default::default()
        });
        let indices = apply_selection_gesture(&[], 0, 2, gesture);
        let paths = selected_paths_for_indices(&rows, &indices);
        assert_eq!(paths, vec![vec![0], vec![1], vec![2]]);
        assert_eq!(
            TreeEvent::SelectionSet {
                paths: paths.clone()
            },
            TreeEvent::SelectionSet { paths }
        );
    }

    #[test]
    fn ctrl_toggle_removes_a_previously_selected_row() {
        let rows = vec![leaf(0, "a"), leaf(1, "b"), leaf(2, "c")];
        let gesture = SelectionGesture::from_modifiers(Modifiers {
            ctrl: true,
            ..Default::default()
        });
        let indices = apply_selection_gesture(&[0, 1, 2], 1, 1, gesture);
        let paths = selected_paths_for_indices(&rows, &indices);
        assert_eq!(paths, vec![vec![0], vec![2]]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_view_roundtrip_serde() {
        let tree = TreeView {
            id: WidgetId::new("sc"),
            rows: vec![TreeRow {
                path: vec![0],
                indent: 0,
                icon: None,
                text: StyledText::plain("Staged Changes"),
                badge: Some(Badge::plain("3")),
                is_expanded: Some(true),
                decoration: Decoration::Normal,
                edit: None,
            }],
            selection_mode: SelectionMode::Single,
            selected_path: Some(vec![0]),
            scroll_offset: 0,
            items_window: None,
            scroll_px: None,
            style: TreeStyle::default(),
            has_focus: true,
        };
        let json = serde_json::to_string(&tree).unwrap();
        let back: TreeView = serde_json::from_str(&json).unwrap();
        assert_eq!(tree, back);
    }

    /// `TreeStyle::row_height` is additive, not a required field.
    ///
    /// A `TreeStyle` deserialized without a `row_height` key must decode
    /// it to the same px-first default as `TreeStyle::default()`
    /// (`Some(22)`) rather than failing the whole `TreeView` deserialization
    /// with a missing-field error.
    #[test]
    fn tree_style_row_height_defaults_to_22_when_absent_from_json() {
        let legacy = r#"{
            "indent": 2,
            "show_chevrons": true,
            "chevron_expanded": "▾",
            "chevron_collapsed": "▸"
        }"#;
        let style: TreeStyle =
            serde_json::from_str(legacy).expect("pre-#623 TreeStyle must decode");
        assert_eq!(
            style.row_height,
            Some(22),
            "absent row_height falls back to the px-first default, not a decode error"
        );
        assert_eq!(style, TreeStyle::default());
    }

    /// An explicit override survives a serde round-trip, so a host
    /// that pins the row pitch keeps it across a save/restore of its UI
    /// state (and across the plugin JSON boundary).
    #[test]
    fn tree_style_row_height_override_roundtrips_serde() {
        let style = TreeStyle {
            row_height: Some(22),
            ..TreeStyle::default()
        };
        let json = serde_json::to_string(&style).unwrap();
        let back: TreeStyle = serde_json::from_str(&json).unwrap();
        assert_eq!(back.row_height, Some(22));
        assert_eq!(style, back);
    }

    // ── D6 TreeView layout API tests ──────────────────────────────────

    fn make_tree_row(path: &[u16], indent: u16, label: &str) -> TreeRow {
        TreeRow {
            path: path.to_vec(),
            indent,
            icon: None,
            text: StyledText::plain(label),
            badge: None,
            is_expanded: None,
            decoration: Decoration::Normal,
            edit: None,
        }
    }

    fn make_tree(rows: Vec<TreeRow>, scroll: usize) -> TreeView {
        TreeView {
            id: WidgetId::new("t"),
            rows,
            selection_mode: SelectionMode::Single,
            selected_path: None,
            scroll_offset: scroll,
            items_window: None,
            scroll_px: None,
            style: TreeStyle::default(),
            has_focus: true,
        }
    }

    #[test]
    fn tree_view_layout_empty() {
        let tree = make_tree(vec![], 0);
        let layout = tree.layout(40.0, 20.0, |_| TreeRowMeasure::new(1.0));
        assert_eq!(layout.visible_rows.len(), 0);
        assert_eq!(layout.hit_regions.len(), 0);
        assert_eq!(layout.resolved_scroll_offset, 0);
        assert_eq!(layout.hit_test(5.0, 5.0), TreeViewHit::Empty);
    }

    #[test]
    fn tree_view_layout_all_rows_fit() {
        let tree = make_tree(
            (0..3)
                .map(|i| make_tree_row(&[i], 0, &format!("row{i}")))
                .collect(),
            0,
        );
        let layout = tree.layout(40.0, 10.0, |_| TreeRowMeasure::new(1.0));
        assert_eq!(layout.visible_rows.len(), 3);
        assert_eq!(layout.visible_rows[0].bounds.y, 0.0);
        assert_eq!(layout.visible_rows[1].bounds.y, 1.0);
        assert_eq!(layout.visible_rows[2].bounds.y, 2.0);
        // Hit-test each row by y coord.
        assert_eq!(layout.hit_test(10.0, 0.5), TreeViewHit::Row(0));
        assert_eq!(layout.hit_test(10.0, 1.5), TreeViewHit::Row(1));
        assert_eq!(layout.hit_test(10.0, 2.5), TreeViewHit::Row(2));
        // Below last row → Empty.
        assert_eq!(layout.hit_test(10.0, 5.0), TreeViewHit::Empty);
    }

    #[test]
    fn tree_view_layout_scroll_offset_applies() {
        let tree = make_tree(
            (0..5)
                .map(|i| make_tree_row(&[i], 0, &format!("row{i}")))
                .collect(),
            2, // skip first 2
        );
        let layout = tree.layout(40.0, 10.0, |_| TreeRowMeasure::new(1.0));
        assert_eq!(layout.resolved_scroll_offset, 2);
        assert_eq!(layout.visible_rows.len(), 3);
        assert_eq!(layout.visible_rows[0].row_idx, 2);
        assert_eq!(layout.visible_rows[1].row_idx, 3);
        assert_eq!(layout.visible_rows[2].row_idx, 4);
    }

    #[test]
    fn tree_view_layout_viewport_overflow_clips() {
        // 10 rows of height 2.0 each; viewport 5.0 tall → only 3 rows
        // fit (one partially clipped).
        let tree = make_tree(
            (0..10)
                .map(|i| make_tree_row(&[i], 0, &format!("row{i}")))
                .collect(),
            0,
        );
        let layout = tree.layout(40.0, 5.0, |_| TreeRowMeasure::new(2.0));
        // Rows 0 (y=0..2), 1 (y=2..4), 2 (y=4..5 clipped) — three visible.
        assert_eq!(layout.visible_rows.len(), 3);
        // Last row clipped to height 1.0 (remaining = 5 - 4 = 1).
        assert_eq!(layout.visible_rows[2].bounds.height, 1.0);
        // A click below all visible rows returns Empty.
        assert_eq!(layout.hit_test(10.0, 5.0), TreeViewHit::Empty);
    }

    #[test]
    fn tree_view_layout_varying_row_heights() {
        // Branches (is_expanded != None) get height 1.4 * base; leaves get
        // 1.0. Proves the measurer can consult row state.
        let mut rows = vec![
            make_tree_row(&[0], 0, "branch"),
            make_tree_row(&[0, 0], 1, "leaf0"),
            make_tree_row(&[0, 1], 1, "leaf1"),
        ];
        rows[0].is_expanded = Some(true);
        let tree = make_tree(rows.clone(), 0);
        let layout = tree.layout(40.0, 10.0, |i| {
            let h = if rows[i].is_expanded.is_some() {
                1.4
            } else {
                1.0
            };
            TreeRowMeasure::new(h)
        });
        assert_eq!(layout.visible_rows.len(), 3);
        assert_eq!(layout.visible_rows[0].bounds.height, 1.4);
        assert_eq!(layout.visible_rows[1].bounds.y, 1.4);
        assert_eq!(layout.visible_rows[1].bounds.height, 1.0);
        assert!((layout.visible_rows[2].bounds.y - 2.4).abs() < 0.001);
    }

    #[test]
    fn tree_view_layout_pixel_units_fractional() {
        // GTK-style: line_height = 18.5 px leaf, 25.9 px branch. Proves
        // fractional row heights flow through correctly.
        let tree = make_tree(
            (0..5)
                .map(|i| make_tree_row(&[i], 0, &format!("r{i}")))
                .collect(),
            0,
        );
        let layout = tree.layout(300.0, 60.0, |_| TreeRowMeasure::new(18.5));
        // 3 full rows fit (55.5), 4th starts at y=55.5 and gets clipped to 4.5 px.
        assert_eq!(layout.visible_rows.len(), 4);
        assert!((layout.visible_rows[3].bounds.height - 4.5).abs() < 0.001);
    }

    #[test]
    fn tree_view_layout_scroll_offset_clamped() {
        // scroll_offset beyond rows.len() — resolved to rows.len()-1 so
        // the single remaining row is visible.
        let tree = make_tree(
            (0..3)
                .map(|i| make_tree_row(&[i], 0, &format!("r{i}")))
                .collect(),
            99,
        );
        let layout = tree.layout(40.0, 10.0, |_| TreeRowMeasure::new(1.0));
        assert_eq!(layout.resolved_scroll_offset, 2);
        assert_eq!(layout.visible_rows.len(), 1);
        assert_eq!(layout.visible_rows[0].row_idx, 2);
    }

    mod vscrollbar_tests {
        use super::*;

        /// Build a tree with `n_rows` leaf rows, scrolled to `scroll_offset`.
        fn vtree(n_rows: usize, scroll_offset: usize) -> TreeView {
            make_tree(
                (0..n_rows)
                    .map(|i| make_tree_row(&[i as u16], 0, &format!("row{i}")))
                    .collect(),
                scroll_offset,
            )
        }

        #[test]
        fn none_when_tree_is_empty() {
            let t = vtree(0, 0);
            assert!(t
                .vscrollbar_with_track_width(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0, 1.0)
                .is_none());
        }

        #[test]
        fn none_when_rows_fit_in_viewport() {
            // 5 rows, 10-row viewport — all fit, no scrollbar.
            let t = vtree(5, 0);
            assert!(t
                .vscrollbar_with_track_width(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0, 1.0)
                .is_none());
        }

        #[test]
        fn none_when_rows_exactly_fill_viewport() {
            // 10 rows, 10-row viewport — total == visible, no scrollbar.
            let t = vtree(10, 0);
            assert!(t
                .vscrollbar_with_track_width(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0, 1.0)
                .is_none());
        }

        #[test]
        fn none_when_row_height_is_zero_or_negative() {
            let t = vtree(50, 0);
            assert!(t
                .vscrollbar_with_track_width(Rect::new(0.0, 0.0, 20.0, 10.0), 0.0, 1.0)
                .is_none());
            assert!(t
                .vscrollbar_with_track_width(Rect::new(0.0, 0.0, 20.0, 10.0), -1.0, 1.0)
                .is_none());
        }

        #[test]
        fn none_when_track_width_is_zero_or_negative() {
            let t = vtree(50, 0);
            assert!(t
                .vscrollbar_with_track_width(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0, 0.0)
                .is_none());
            assert!(t
                .vscrollbar_with_track_width(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0, -1.0)
                .is_none());
        }

        #[test]
        fn track_spans_rightmost_column() {
            let t = vtree(20, 0);
            let sb = t
                .vscrollbar_with_track_width(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0, 1.0)
                .expect("overflow should yield a scrollbar");
            // Rightmost column (x=19), full height, 1 unit wide.
            assert_eq!(sb.track.x, 19.0);
            assert_eq!(sb.track.y, 0.0);
            assert_eq!(sb.track.width, 1.0);
            assert_eq!(sb.track.height, 10.0);
            // Thumb starts at the top when scroll_offset == 0.
            assert_eq!(sb.thumb_start, 0.0);
            assert!(sb.thumb_len > 0.0 && sb.thumb_len < sb.track.height);
        }

        #[test]
        fn thumb_advances_with_scroll_offset() {
            let t = vtree(20, 10);
            let sb = t
                .vscrollbar_with_track_width(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0, 1.0)
                .expect("overflow should yield a scrollbar");
            assert!(
                sb.thumb_start > 0.0,
                "thumb should have travelled from the top once scrolled"
            );
        }

        #[test]
        fn pixel_backend_track_is_10px_wide_independent_of_row_pitch() {
            // GTK/macOS/Win-style: row_height is a 20px row pitch (the
            // rounded `line_height * 1.4` tree_row_pitch produces), but
            // the track column must stay VS Code's fixed 10px — not grow
            // to match the row pitch.
            let t = vtree(50, 0);
            let sb = t
                .vscrollbar_with_track_width(
                    Rect::new(0.0, 0.0, 300.0, 200.0),
                    20.0,
                    crate::primitives::layout_metrics::pixel::TREE_VSCROLLBAR_WIDTH,
                )
                .expect("overflow should yield a scrollbar");
            assert_eq!(sb.track.width, 10.0);
            assert_eq!(sb.track.x, 300.0 - 10.0);
            // The minimum thumb length still ties to row_height, not
            // track_width.
            assert!(sb.thumb_len >= 20.0);
        }

        #[test]
        fn tui_track_is_one_cell_wide() {
            // TUI-style: row_height = 1.0 cell, track_width = 1.0 cell —
            // the track stays exactly one cell, matching TUI's uniform
            // row pitch.
            let t = vtree(50, 0);
            let sb = t
                .vscrollbar_with_track_width(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0, 1.0)
                .expect("overflow should yield a scrollbar");
            assert_eq!(sb.track.width, 1.0);
        }

        #[test]
        fn scroll_px_moves_the_thumb_smoothly_between_whole_rows() {
            let row0 = vtree(30, 0)
                .vscrollbar_with_track_width(Rect::new(0.0, 0.0, 20.0, 10.0 * 22.0), 22.0, 1.0)
                .unwrap();
            let row1 = vtree(30, 1)
                .vscrollbar_with_track_width(Rect::new(0.0, 0.0, 20.0, 10.0 * 22.0), 22.0, 1.0)
                .unwrap();
            let half_sb = vtree(30, 0)
                .vscrollbar_with_scroll_px(
                    Rect::new(0.0, 0.0, 20.0, 10.0 * 22.0),
                    22.0,
                    1.0,
                    Some(11.0),
                )
                .unwrap();
            assert!(
                half_sb.thumb_start > row0.thumb_start && half_sb.thumb_start < row1.thumb_start,
                "half-row scroll_px should land the thumb strictly between rows 0 and 1"
            );
        }

        #[test]
        fn scroll_px_wins_over_scroll_offset_for_the_thumb() {
            let mut t = vtree(30, 0);
            t.scroll_offset = 5; // stale, overridden by scroll_px
            let sb = t
                .vscrollbar_with_scroll_px(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0, 1.0, Some(0.0))
                .unwrap();
            let at_zero = vtree(30, 0)
                .vscrollbar_with_track_width(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0, 1.0)
                .unwrap();
            assert_eq!(sb.thumb_start, at_zero.thumb_start);
        }
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
    use crate::style::Style;
    use crate::theme::Theme;
    use crate::types::Color;
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

    fn row(path: &[u16], indent: u16, label: &str, expanded: Option<bool>) -> TreeRow {
        TreeRow {
            path: path.to_vec(),
            indent,
            icon: None,
            text: StyledText::plain(label),
            badge: None,
            is_expanded: expanded,
            decoration: Decoration::default(),
            edit: None,
        }
    }

    fn sample_tree() -> TreeView {
        TreeView {
            id: WidgetId::new("explorer"),
            rows: vec![
                row(&[0], 0, "src", Some(true)),
                row(&[0, 0], 1, "main.rs", None),
                row(&[1], 0, "Cargo.toml", None),
            ],
            selection_mode: SelectionMode::Single,
            selected_path: Some(vec![0, 0]),
            scroll_offset: 0,
            items_window: None,
            scroll_px: None,
            style: TreeStyle::default(),
            has_focus: true,
        }
    }

    /// The acceptance criterion: `TreeView`'s [`Primitive::layout`] +
    /// [`Primitive::paint`] must produce the exact same verb log as
    /// calling [`native_surface_paint::paint`] directly with the
    /// equivalent arguments.
    #[test]
    fn tree_view_primitive_paint_matches_native_surface_paint() {
        let tree = sample_tree();
        let rect = Rect::new(10.0, 20.0, 200.0, 80.0);
        let ctx = LayoutCtx::new(16.0, 8.0, true);
        let theme = Theme::default();
        let style = Style::default();

        let via_primitive_layout = Primitive::layout(&tree, &ctx, rect);
        let mut via_primitive_surface = LogSurface::default();
        Primitive::paint(
            &tree,
            &via_primitive_layout,
            &mut via_primitive_surface,
            &crate::PaintEnv::new(&theme, &style),
        );

        let direct_layout = crate::primitives::layout_metrics::pixel_tree_layout(
            &tree,
            rect,
            ctx.line_height as f64,
            style.row_padding_x,
        );
        let mut direct_surface = LogSurface::default();
        native_surface_paint::paint(
            &tree,
            rect,
            &direct_layout,
            ctx.line_height,
            ctx.nerd_fonts_enabled,
            &mut direct_surface,
            &theme,
            &style,
            None,
            None,
            false,
        );

        assert!(
            !via_primitive_surface.calls.is_empty(),
            "the tree must paint something for this comparison to be meaningful"
        );
        assert_eq!(via_primitive_surface.calls, direct_surface.calls);
    }

    /// [`TreeViewPrimitiveLayout::hit_test`] subtracts the area's
    /// origin before delegating — a click at the layout's absolute
    /// origin must resolve to the same hit as `(0, 0)` on the
    /// tree-local [`TreeViewLayout::hit_test`].
    #[test]
    fn primitive_layout_hit_test_is_relative_to_area_origin() {
        let tree = sample_tree();
        let rect = Rect::new(10.0, 20.0, 200.0, 80.0);
        let ctx = LayoutCtx::new(16.0, 8.0, true);

        let layout = Primitive::layout(&tree, &ctx, rect);
        let local = layout.layout.hit_test(0.0, 0.0);
        let absolute = layout.hit_test(rect.x, rect.y);
        assert_eq!(local, absolute);
    }
}

#[cfg(all(
    test,
    any(
        feature = "gtk",
        feature = "tui",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    )
))]
mod interactive_tests {
    use super::*;
    use crate::a11y::A11yAction;
    use crate::event::{Key, NamedKey, Point};
    use crate::interactive::{EventCx, EventHost, Interactive, RoutedEvent, SubTarget};
    use crate::primitive::{LayoutCtx, Primitive, Units};
    use crate::PointerShape;

    #[derive(Default)]
    struct MockHost {
        emitted: Vec<crate::UiEvent>,
        captured: bool,
    }

    impl EventHost for MockHost {
        fn is_focused(&self, _id: &WidgetId, _sub: Option<SubTarget>) -> bool {
            true
        }
        fn request_focus(&mut self, _id: &WidgetId, _sub: Option<SubTarget>) {}
        fn capture_pointer(&mut self, _id: &WidgetId) {
            self.captured = true;
        }
        fn release_pointer(&mut self) {
            self.captured = false;
        }
        fn set_cursor(&mut self, _cursor: PointerShape) {}
        fn request_redraw(&mut self) {}
        fn emit(&mut self, event: crate::UiEvent) {
            self.emitted.push(event);
        }
        fn units(&self) -> Units {
            Units::Cells
        }
    }

    fn row(path: &[u16], label: &str, is_expanded: Option<bool>) -> TreeRow {
        TreeRow {
            path: path.to_vec(),
            indent: (path.len() - 1) as u16,
            icon: None,
            text: StyledText::plain(label),
            badge: None,
            is_expanded,
            decoration: Decoration::Normal,
            edit: None,
        }
    }

    fn tree(n: usize) -> TreeView {
        let mut rows = vec![row(&[0], "dir", Some(false))];
        rows.extend((1..n).map(|i| row(&[i as u16], &format!("item{i}"), None)));
        TreeView {
            id: WidgetId::new("tree:unit"),
            rows,
            selection_mode: SelectionMode::Single,
            selected_path: None,
            scroll_offset: 0,
            items_window: None,
            scroll_px: None,
            style: TreeStyle::default(),
            has_focus: true,
        }
    }

    fn layout_for(tree: &TreeView, height: f32) -> TreeViewPrimitiveLayout {
        Primitive::layout(
            tree,
            &LayoutCtx::new(1.0, 1.0, false),
            Rect::new(0.0, 0.0, 20.0, height),
        )
    }

    fn run(
        tree: &TreeView,
        state: &mut TreeViewState,
        event: RoutedEvent,
        height: f32,
    ) -> (Handled<TreeEvent>, MockHost) {
        let layout = layout_for(tree, height);
        let mut host = MockHost::default();
        let handled = {
            let mut cx = EventCx::new(tree.id, &mut host);
            Interactive::handle(tree, state, &event, &layout, &mut cx)
        };
        (handled, host)
    }

    /// A selection the app moves below the viewport scrolls into view
    /// through `settle`, once per new selection, and the scroll it records
    /// is what the tree resolves next.
    #[test]
    fn settle_scrolls_a_selection_the_app_moved_into_view_once() {
        let mut tree = tree(30);
        tree.selected_path = Some(vec![20]);
        let mut state = TreeViewState::default();
        let layout = layout_for(&tree, 5.0);
        assert!(Interactive::settle(&tree, &mut state, &layout));
        let scroll = tree.live(Some(&state)).scroll_offset;
        assert!(
            scroll <= 20 && 20 < scroll + layout.viewport_rows,
            "{scroll}"
        );
        // The same selection again records nothing more.
        assert!(!Interactive::settle(&tree, &mut state, &layout));
    }

    /// A `RowToggleExpand` the host delivers on the app's behalf expands
    /// the branch it names, and one naming another tree is ignored.
    #[test]
    fn follow_event_toggles_the_named_branch_only_for_this_tree() {
        let tree = tree(3);
        let mut state = TreeViewState::default();
        let toggle = |id: &str| {
            crate::UiEvent::Tree(
                WidgetId::new(id),
                TreeEvent::RowToggleExpand { path: vec![0] },
            )
        };
        Interactive::follow_event(&tree, &mut state, &toggle("tree:other"));
        assert_eq!(tree.live(Some(&state)).rows[0].is_expanded, Some(false));
        Interactive::follow_event(&tree, &mut state, &toggle("tree:unit"));
        assert_eq!(tree.live(Some(&state)).rows[0].is_expanded, Some(true));
        let summary = Interactive::state_summary(&tree, &state);
        assert_eq!(summary.expanded, Some(vec![vec![0]]));
    }

    fn key(named: NamedKey) -> RoutedEvent {
        RoutedEvent::Key(crate::interactive::KeyEvent {
            key: Key::Named(named),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    #[test]
    fn sub_parts_round_trip_through_their_sub_targets() {
        for part in [
            TreeViewSubPart::Row(0),
            TreeViewSubPart::Row(41),
            TreeViewSubPart::Chevron(7),
            TreeViewSubPart::Scrollbar,
        ] {
            assert_eq!(
                TreeViewSubPart::from_sub_target(part.sub_target()),
                Some(part)
            );
        }
        assert_eq!(TreeViewSubPart::from_sub_target(SubTarget(0)), None);
    }

    #[test]
    fn sub_regions_name_every_visible_row_and_the_scrollbar() {
        let t = tree(30);
        let layout = layout_for(&t, 10.0);
        let parts: Vec<_> = Interactive::sub_regions(&t, &layout)
            .into_iter()
            .filter_map(|(_, sub)| TreeViewSubPart::from_sub_target(sub))
            .collect();
        assert!(parts.contains(&TreeViewSubPart::Row(1)));
        assert!(parts.contains(&TreeViewSubPart::Scrollbar));
    }

    #[test]
    fn empty_tree_has_no_focus_stop() {
        let mut t = tree(1);
        t.rows.clear();
        assert_eq!(t.focus(), crate::interactive::FocusSpec::None);
        assert_eq!(tree(3).focus(), crate::interactive::FocusSpec::Stop);
    }

    #[test]
    fn descriptor_selection_wins_over_stale_state() {
        let mut t = tree(5);
        let mut state = TreeViewState::default();
        state.nav.set_selected_path(Some(vec![1]));
        t.selected_path = Some(vec![3]);
        let (handled, _) = run(&t, &mut state, key(NamedKey::Down), 10.0);
        assert_eq!(
            handled,
            Handled::Event(TreeEvent::SelectionChanged { path: vec![4] })
        );
    }

    #[test]
    fn page_down_moves_by_one_viewport_less_one_row() {
        let mut t = tree(30);
        t.selected_path = Some(vec![0]);
        let mut state = TreeViewState::default();
        let (handled, host) = run(&t, &mut state, key(NamedKey::PageDown), 10.0);
        assert_eq!(
            handled,
            Handled::Event(TreeEvent::SelectionChanged { path: vec![9] })
        );
        assert!(host.emitted.is_empty(), "row 9 is still in view");
    }

    #[test]
    fn a_fractional_wheel_accumulates_before_reporting_a_whole_row() {
        let t = tree(30);
        let mut state = TreeViewState::default();
        let wheel = |dy| RoutedEvent::Wheel {
            dx: 0.0,
            dy,
            local: Point::new(1.0, 1.0),
            mods: Modifiers::default(),
        };
        let (first, _) = run(&t, &mut state, wheel(-0.5), 10.0);
        assert_eq!(first, Handled::Consumed);
        let (second, _) = run(&t, &mut state, wheel(-0.5), 10.0);
        assert_eq!(
            second,
            Handled::Event(TreeEvent::ScrollChanged { scroll_offset: 1 })
        );
    }

    #[test]
    fn scrollbar_track_press_pages_and_thumb_press_captures() {
        let t = tree(30);
        let layout = layout_for(&t, 10.0);
        let sb = layout.vscrollbar.clone().expect("30 rows overflow 10");
        let mut state = TreeViewState::default();
        let below_thumb = Point::new(sb.track.x, sb.track.y + sb.track.height - 1.0);
        let press = |local| RoutedEvent::Pointer {
            kind: crate::interactive::PointerKind::Down(crate::event::MouseButton::Left),
            sub: None,
            local,
            mods: Modifiers::default(),
        };
        let (handled, _) = run(&t, &mut state, press(below_thumb), 10.0);
        assert_eq!(
            handled,
            Handled::Event(TreeEvent::ScrollChanged { scroll_offset: 10 })
        );

        let mut state = TreeViewState::default();
        let (handled, host) = run(&t, &mut state, press(Point::new(sb.track.x, 0.0)), 10.0);
        assert_eq!(handled, Handled::Consumed);
        assert!(host.captured, "a thumb press captures the pointer");
        assert!(state.nav.is_dragging());
    }

    #[test]
    fn a11y_expand_and_collapse_only_toggle_toward_their_direction() {
        let mut t = tree(3);
        t.selected_path = Some(vec![0]);
        let mut state = TreeViewState::default();
        let (collapse, _) = run(
            &t,
            &mut state,
            RoutedEvent::Action(A11yAction::Collapse),
            10.0,
        );
        assert_eq!(collapse, Handled::Consumed, "already collapsed");
        let (expand, _) = run(
            &t,
            &mut state,
            RoutedEvent::Action(A11yAction::Expand),
            10.0,
        );
        assert_eq!(
            expand,
            Handled::Event(TreeEvent::RowToggleExpand { path: vec![0] })
        );
    }

    #[test]
    fn a11y_click_opens_the_selected_row_like_the_a11y_router() {
        let mut t = tree(3);
        t.selected_path = Some(vec![2]);
        let mut state = TreeViewState::default();
        let (handled, _) = run(&t, &mut state, RoutedEvent::Action(A11yAction::Click), 10.0);
        assert_eq!(
            handled,
            Handled::Event(TreeEvent::RowDoubleClicked { path: vec![2] })
        );
    }

    #[test]
    fn unclaimed_keys_are_reported_and_bubble() {
        let mut t = tree(3);
        t.selected_path = Some(vec![1]);
        let mut state = TreeViewState::default();
        let (handled, host) = run(&t, &mut state, key(NamedKey::Tab), 10.0);
        assert_eq!(handled, Handled::Ignored);
        assert!(matches!(
            host.emitted.as_slice(),
            [crate::UiEvent::Tree(_, TreeEvent::KeyPressed { .. })]
        ));
    }

    fn edit(text: &str, cursor: usize, anchor: Option<usize>) -> TreeRowEditState {
        TreeRowEditState {
            text: text.into(),
            cursor,
            selection_anchor: anchor,
            placeholder: None,
        }
    }

    #[test]
    fn rename_select_all_then_type_replaces_the_text() {
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        let TreeRenameKey::Moved(all) =
            tree_rename_key(&edit("main.rs", 2, None), &Key::Char('a'), ctrl)
        else {
            panic!("Ctrl-A selects all");
        };
        assert_eq!((all.cursor, all.selection_anchor), (7, Some(0)));
        let typed = tree_rename_insert(&all, "lib");
        assert_eq!(typed.text, "lib");
        assert_eq!(typed.cursor, 3);
    }

    #[test]
    fn rename_ignores_vertical_motion_and_word_deletes() {
        assert_eq!(
            tree_rename_key(
                &edit("abc", 1, None),
                &Key::Named(NamedKey::Up),
                Modifiers::default()
            ),
            TreeRenameKey::Unhandled
        );
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        let TreeRenameKey::Edited(next) = tree_rename_key(
            &edit("foo bar", 7, None),
            &Key::Named(NamedKey::Backspace),
            ctrl,
        ) else {
            panic!("Ctrl-Backspace deletes a word");
        };
        assert_eq!(next.text, "foo ");
    }

    /// Event parity for the assistive-technology legs: an `Expand` on a
    /// selected collapsed branch, routed through the live widget
    /// (`a11y::route_widget_action`), through the captured-frame snapshot
    /// (`a11y::route_action`) and straight into `Interactive::handle`,
    /// resolves to the same `RowToggleExpand`; a `Collapse` on that
    /// collapsed row resolves to no event on all three. The `TuiDriver`
    /// keyboard leg (Right on the same row) lives in
    /// `tests/widget_contract_tree.rs`.
    #[test]
    fn expand_and_collapse_agree_across_the_a11y_routes_and_direct_handle() {
        let t = TreeView {
            selected_path: Some(vec![0]),
            ..tree(3)
        };
        let id = t.id;
        let area = Rect::new(0.0, 0.0, 20.0, 5.0);
        let toggle = vec![crate::UiEvent::Tree(
            id,
            TreeEvent::RowToggleExpand { path: vec![0] },
        )];

        let direct = |action| {
            let mut state = TreeViewState::default();
            let mut host = MockHost::default();
            let layout = layout_for(&t, area.height);
            let handled = {
                let mut cx = EventCx::new(id, &mut host);
                Interactive::handle(
                    &t,
                    &mut state,
                    &RoutedEvent::Action(action),
                    &layout,
                    &mut cx,
                )
            };
            match handled {
                Handled::Event(ev) => vec![crate::interactive::WidgetAction::into_ui_event(ev, id)],
                _ => Vec::new(),
            }
        };
        let live = |action| {
            let mut frame = crate::frame_tree::FrameTree::new();
            frame.push(id, area, Box::new(t.clone()));
            let mut states = crate::widget_state::WidgetStateStore::new();
            crate::a11y::route_widget_action(
                &frame,
                &mut states,
                &LayoutCtx::new(1.0, 1.0, false),
                id,
                action,
            )
        };
        let snapshot = crate::testing::a11y::A11yScenario::new()
            .with_surface(crate::frame::Surface::Widget {
                rect: area,
                widget: Box::new(t.clone()),
            })
            .snapshot();
        let node = snapshot.find("dir").expect("branch row node").id;
        let captured = |action| crate::a11y::route_action(snapshot.tree(), node, action, None);

        for route in [
            &direct as &dyn Fn(A11yAction) -> Vec<crate::UiEvent>,
            &live,
            &captured,
        ] {
            assert_eq!(route(A11yAction::Expand), toggle);
            assert_eq!(route(A11yAction::Collapse), Vec::new());
        }
    }
}
