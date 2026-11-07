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

use crate::event::Rect;
use crate::primitives::scrollbar::Scrollbar;
use crate::types::{
    Badge, Decoration, Icon, Modifiers, SelectionMode, StyledText, TreePath, TreeStyle, WidgetId,
};
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
///     style: TreeStyle::default(),
///     has_focus: true,
/// };
///
/// let layout = tree.layout(40.0, 10.0, |_| TreeRowMeasure::new(1.0));
/// assert_eq!(layout.hit_test(0.0, 0.0), TreeViewHit::Row(0));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// focus-visible ring instead reads
    /// [`crate::Backend::draw_tree_with_state`]'s own
    /// `focus_ring_visible` parameter — pass
    /// `ring.is_focused(&tree.id) && ring.is_focus_visible()` there.
    #[serde(default)]
    pub has_focus: bool,
}

/// One visible row in a `TreeView`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
/// (`tui::tree::draw_tree`) always reads the two fields directly and
/// never calls this.
///
/// [`crate::primitives::layout_metrics::tree_layout_measured`] calls
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
        let mut visible_rows: Vec<VisibleTreeRow> = Vec::new();
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

        // Resolve `scroll_offset` vs. `scroll_px` (the latter wins when
        // set); we still report the clamped row index so the app can
        // write it back and self-correct. The first row's height is the
        // uniform row-height basis for the pixel conversion — the same
        // approximation `vscrollbar` already makes for thumb sizing on
        // trees with per-row-varying heights (leaf vs. branch). Only
        // measured when `scroll_px` is actually in play — `measure_row`
        // can do real text-measurement work (Pango layout on GTK), and
        // `None` never reads this value.
        let row_height_basis = if scroll_px.is_none() || self.rows.is_empty() {
            1.0
        } else {
            measure_row(0).height
        };
        let (resolved_scroll_offset, scroll_sub_px) =
            crate::primitives::scrollbar::resolve_scroll_px(
                self.scroll_offset,
                scroll_px,
                self.rows.len(),
                row_height_basis,
            );

        let rows_y_end = inset_y + rows_h_max;
        let mut y = inset_y - scroll_sub_px;
        for i in resolved_scroll_offset..self.rows.len() {
            if y >= rows_y_end {
                break;
            }
            let m = measure_row(i);
            // Clip the last row's height to fit inside the viewport.
            let remaining = rows_y_end - y;
            let height = m.height.min(remaining).max(0.0);
            if height <= 0.0 {
                break;
            }
            let bounds = Rect::new(inset_x, y, row_w, height);
            visible_rows.push(VisibleTreeRow { row_idx: i, bounds });
            // Split the hit region into Chevron + Row when the backend
            // supplied a chevron boundary for this branch row.
            let row = &self.rows[i];
            if let (Some(_), Some(chev_x)) = (row.is_expanded, m.chevron_end_x) {
                let chev_x = chev_x.clamp(0.0, row_w);
                if chev_x > 0.0 {
                    hit_regions.push((
                        Rect::new(inset_x, y, chev_x, height),
                        TreeViewHit::Chevron(i),
                    ));
                }
                let body_w = (row_w - chev_x).max(0.0);
                if body_w > 0.0 {
                    hit_regions.push((
                        Rect::new(inset_x + chev_x, y, body_w, height),
                        TreeViewHit::Row(i),
                    ));
                }
            } else {
                hit_regions.push((bounds, TreeViewHit::Row(i)));
            }
            y += m.height;
        }

        TreeViewLayout {
            viewport_width,
            viewport_height,
            visible_rows,
            hit_regions,
            resolved_scroll_offset,
            scroll_sub_px,
        }
    }

    /// Vertical scrollbar geometry for this tree rendered into `area`, or
    /// `None` when the tree is empty or all rows fit in the viewport.
    ///
    /// This is the single source of truth shared by any rasteriser that
    /// paints a tree scrollbar and consumers that hit-test the resolved
    /// `track`/`thumb_start`/`thumb_len` to implement thumb dragging —
    /// mirrors [`ListView::vscrollbar`](crate::primitives::list::ListView::vscrollbar).
    /// Reach it backend-agnostically via
    /// [`crate::Backend::tree_vscrollbar`].
    ///
    /// Unlike `ListView`, `TreeView` rows can have mixed heights on pixel
    /// backends (`Decoration::Header` rows paint shorter than others —
    /// see [`crate::primitives::layout_metrics::tree_layout`]). This
    /// method still sizes the thumb against a uniform `row_height`: a
    /// pixel-exact extent would require summing every row's real
    /// measured height every frame just to position a thumb, and the
    /// thumb only needs to be proportional, not a promise of pixel-exact
    /// travel. Unlike `ListView` (whose items really do paint at exactly
    /// `line_height`, with no multiplier), a `TreeView`'s non-header rows
    /// paint at `line_height * 1.4` (or [`crate::types::TreeStyle::row_height`]
    /// when the host set one) — so callers must pass that pitch, not raw
    /// `line_height`, or the computed "does this overflow" answer and the
    /// thumb geometry won't match what [`crate::primitives::layout_metrics::tree_layout`]
    /// (and therefore `draw_tree`) actually paints.
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
        if row_height <= 0.0 || track_width <= 0.0 {
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
        let (resolved_scroll_offset, scroll_sub_px) =
            crate::primitives::scrollbar::resolve_scroll_px(
                self.scroll_offset,
                scroll_px,
                self.rows.len(),
                row_height,
            );
        let scroll_rows = resolved_scroll_offset as f32 + scroll_sub_px / row_height;
        Some(Scrollbar::vertical(
            self.id,
            track,
            scroll_rows,
            total,
            visible,
            row_height,
        ))
    }
}

/// Per-frame paint-time state for a `TreeView` — hover/press row
/// tracking plus a pixel-precise scroll position, none of which live on
/// the declarative [`TreeView`] descriptor itself (see
/// [`crate::primitives::list::ListPaintState`]'s doc for why — the same
/// rule applies here).
///
/// `#[non_exhaustive]`: construct via [`Default::default`] plus the
/// `with_*` builders, so a future field here is additive.
///
/// `scroll_px: None` paints exactly as [`TreeView::layout`] /
/// [`TreeView::vscrollbar`] already do (whole-row from
/// `scroll_offset`); `Some(px)` wins and is resolved via
/// [`crate::primitives::scrollbar::resolve_scroll_px`] into a
/// `scroll_offset` plus a sub-row remainder — see
/// [`TreeView::layout_with_scroll_px`].
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TreePaintState {
    pub hovered_idx: Option<usize>,
    pub pressed_idx: Option<usize>,
    pub scroll_px: Option<f32>,
}

impl TreePaintState {
    pub fn with_hovered_idx(mut self, idx: Option<usize>) -> Self {
        self.hovered_idx = idx;
        self
    }

    pub fn with_pressed_idx(mut self, idx: Option<usize>) -> Self {
        self.pressed_idx = idx;
        self
    }

    pub fn with_scroll_px(mut self, px: Option<f32>) -> Self {
        self.scroll_px = px;
        self
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
    /// `&self.current_style`; `win::tree::draw_tree` passes
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

        let indent_px = (line_height * 0.9).round();
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
            // `draw_tree` paints these unconditionally, a difference this
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

            // Focus-visible ring: a square stroke flush with the
            // focused row's own bounds, in `Theme::focus_ring_color` at
            // `Style::focus_ring_width` — see
            // `crate::primitives::list::native_surface_paint::paint`'s
            // matching comment for why this one isn't outset like
            // `Button`'s/`TextInput`'s. Gated on `focus_ring_visible`,
            // not `tree.has_focus` — see this fn's own doc and
            // `TreeView::has_focus`'s doc for why the two are kept
            // separate.
            if is_selected && focus_ring_visible {
                surface.surface_stroke_rect(
                    row_rect,
                    theme.focus_ring_color(),
                    style.focus_ring_width,
                );
            }

            let mut cursor_x = row_x + style.row_padding_x + row.indent as f32 * indent_px;

            if let Some(expanded) = row.is_expanded {
                if tree.style.show_chevrons {
                    let chevron = super::tree_chevron_glyph(&tree.style, expanded);
                    let (cw, ch) = surface.surface_measure_text(&chevron);
                    let cy = row_y + (row_h - ch) / 2.0;
                    surface.surface_draw_icon_glyph(
                        Rect::new(cursor_x, cy, cw, ch),
                        &chevron,
                        def_fg,
                    );
                    cursor_x += cw + 4.0;
                }
            } else {
                cursor_x += line_height * 0.8;
            }

            if let Some(ref icon) = row.icon {
                let glyph = if nerd_fonts_enabled {
                    icon.glyph.as_str()
                } else {
                    icon.fallback.as_str()
                };
                let icon_fg = icon.color.unwrap_or(def_fg);
                let (iw, ih) = surface.surface_measure_text(glyph);
                let iy = row_y + (row_h - ih) / 2.0;
                surface.surface_draw_text_run(Rect::new(cursor_x, iy, iw, ih), glyph, icon_fg);
                cursor_x += iw + 6.0;
            }

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
            let total_w: f32 = row
                .text
                .spans
                .iter()
                .map(|span| surface.surface_measure_text_styled(&span.text, span.bold).0)
                .sum();
            let needs_ellipsis = total_w > avail;

            surface.surface_push_clip(Rect::new(cursor_x, row_y, avail, row_h));
            let mut remaining = avail;
            for span in &row.text.spans {
                if remaining <= 0.0 {
                    break;
                }
                let span_fg = if let Some(c) = span.fg {
                    c
                } else if matches!(row.decoration, Decoration::Muted) {
                    theme.muted_fg
                } else {
                    def_fg
                };
                let (sw, sh) = surface.surface_measure_text_styled(&span.text, span.bold);
                let (paint_text, paint_w, truncated) = if !needs_ellipsis || sw <= remaining {
                    (span.text.clone(), sw, false)
                } else {
                    let (t, w) = crate::paint_surface::fit_text_prefix(
                        surface, &span.text, span.bold, remaining,
                    );
                    (t, w, true)
                };
                if truncated && paint_w <= 0.0 {
                    // Truncation left no room even for an ellipsis-only
                    // prefix, so every later span is fully overflowed too.
                    break;
                }
                if let Some(sbg) = span.bg {
                    surface.surface_fill_rect(Rect::new(cursor_x, row_y, paint_w, row_h), sbg);
                }
                let sy = row_y + (row_h - sh) / 2.0;
                surface.surface_draw_text_run_styled(
                    Rect::new(cursor_x, sy, paint_w, sh),
                    &paint_text,
                    span_fg,
                    span.bold,
                    false,
                    false,
                    1.0,
                );
                cursor_x += paint_w;
                remaining -= paint_w;
                if paint_text != span.text {
                    // This span was truncated with an ellipsis — any
                    // later spans are fully overflowed, so stop here.
                    break;
                }
            }
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
                style: TreeStyle::default(),
                has_focus: true,
            }
        }

        const LINE_HEIGHT: f32 = 16.0;
        const AREA: Rect = Rect::new(0.0, 0.0, 100.0, 80.0);

        /// Before this migration, none of the three
        /// backends' `draw_tree` painted a vertical scrollbar even
        /// though `Backend::tree_vscrollbar` already exposed
        /// real geometry for hit-testing. Paints through the shared fn
        /// and asserts a fill lands at the exact track rect
        /// `TreeView::vscrollbar` resolves.
        #[test]
        fn paints_vertical_scrollbar_track_when_overflowing() {
            let tree = make_tree((0..50).map(|i| leaf(i, &format!("row{i}"))).collect());
            let layout = crate::primitives::layout_metrics::tree_layout(
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
            let layout = crate::primitives::layout_metrics::tree_layout(
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
            let layout = crate::primitives::layout_metrics::tree_layout(
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
            let layout = crate::primitives::layout_metrics::tree_layout(
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

        /// `win::tree::draw_tree` never painted
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
            let layout = crate::primitives::layout_metrics::tree_layout(
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
            let layout = crate::primitives::layout_metrics::tree_layout(
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
            let layout = crate::primitives::layout_metrics::tree_layout(
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
            let layout = crate::primitives::layout_metrics::tree_layout(
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
            let layout = crate::primitives::layout_metrics::tree_layout(
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
            let layout = crate::primitives::layout_metrics::tree_layout(
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
            let layout = crate::primitives::layout_metrics::tree_layout(
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
            let layout = crate::primitives::layout_metrics::tree_layout(
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
            let layout = crate::primitives::layout_metrics::tree_layout(
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
            let layout = crate::primitives::layout_metrics::tree_layout(
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

/// Events a `TreeView` emits back to the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TreeEvent {
    /// Single-click (or Enter on the keyboard) on a row.
    RowClicked {
        path: TreePath,
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
