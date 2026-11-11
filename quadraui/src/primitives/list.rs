//! `ListView` primitive: a flat, scrollable list of rows with
//! optional title header, icons, right-aligned detail text, and
//! per-row decoration.
//!
//! Distinct from `TreeView` (hierarchical, expand/collapse) and
//! `Palette` (modal overlay with query input). `ListView` is the
//! right primitive for "flat list of rows rendered into a panel":
//! quickfix lists, symbol lists, reference lists, log panes, buffer
//! switchers (when not rendered as a modal), diagnostics lists.
//!
//! # Backend contract
//!
//! **Purely declarative** — render the optional `title` then
//! `items[scroll_offset..]` until the viewport fills. A single click on a
//! row emits `ListViewEvent::SelectionChanged { idx }` (select); a
//! double-click emits the distinct `ListViewEvent::ItemDoubleClicked { idx }`
//! (open) rather than reusing `ItemActivated`, so an app can tell "select"
//! from "open" apart — see `ItemActivated`'s own doc. Keyboard `j`/`k`
//! move the selection the same way `SelectionChanged` does; Enter activates
//! the current selection as `ItemActivated`, mirroring the mouse's
//! double-click rather than its single click. An app that wants "open this
//! row" from mouse, keyboard, and a screen reader's default action alike
//! listens for both `ItemActivated` and `ItemDoubleClicked`. The app
//! updates `selected_idx` and `scroll_offset` for the next frame.

use crate::event::Rect;
use crate::primitives::row_stack::{self, RowPaintState};
use crate::primitives::scrollbar::Scrollbar;
use crate::types::{Decoration, Icon, Modifiers, StyledText, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a `ListView` widget.
///
/// # Examples
///
/// ```
/// use quadraui::{ListItem, ListItemMeasure, ListView, ListViewHit, StyledText, WidgetId};
///
/// let list = ListView {
///     id: WidgetId::new("list:quickfix"),
///     title: None,
///     items: vec![ListItem {
///         text: StyledText::plain("src/main.rs:10: unused import"),
///         icon: None,
///         detail: None,
///         decoration: Default::default(),
///     }],
///     selected_idx: 0,
///     scroll_offset: 0,
///     has_focus: true,
///     bordered: false,
///     h_scroll: 0,
///     max_content_width: None,
///     show_v_scrollbar: false,
/// };
///
/// let layout = list.layout(80.0, 10.0, 0.0, |_| ListItemMeasure::new(1.0));
/// assert_eq!(layout.hit_test(0.0, 0.0), ListViewHit::Item(0));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListView {
    pub id: WidgetId,
    /// Optional header row shown above the items. `None` = no header.
    #[serde(default)]
    pub title: Option<StyledText>,
    pub items: Vec<ListItem>,
    pub selected_idx: usize,
    #[serde(default)]
    pub scroll_offset: usize,
    /// Whether this list currently holds keyboard focus. Gates the
    /// selected row's highlight (TUI reverse-video/`▶` prefix, GUI
    /// `Theme::selected_bg` fill).
    ///
    /// Deliberately **not** the same "host pre-composes
    /// `ring.is_focused(id) && ring.is_focus_visible()`" convention
    /// [`crate::Button::focused`]/[`crate::TextInput::has_focus`]
    /// follow: several existing hosts (e.g. a sidebar's "currently
    /// showing" nav row) set this unconditionally `true` as a
    /// persistent indicator unrelated to real keyboard focus, so a
    /// focus-visible ring can't ride on this field without painting a
    /// ring on every one of them. The GUI focus-visible ring instead
    /// reads [`crate::Backend::draw_list_with_state`]'s own
    /// `focus_ring_visible` parameter — pass
    /// `ring.is_focused(&list.id) && ring.is_focus_visible()` there.
    #[serde(default)]
    pub has_focus: bool,
    /// When true, backends draw a `╭─╮ │ │ ╰─╯` frame around the list
    /// and inset items by 1 cell on each side. Title (if present)
    /// renders as an overlay on the top border (`╭─ Title ─╮`) instead
    /// of as a separate header strip. Used by modal-style overlays
    /// (tab switcher, file picker). Default `false` matches the flat
    /// header+rows layout used by quickfix and other inline panels.
    #[serde(default)]
    pub bordered: bool,
    /// Horizontal scroll offset in **display cells** (terminal columns —
    /// a CJK/emoji glyph counts as 2, matching
    /// [`crate::text_util::char_cell_width`]), not characters. Number of
    /// content columns to skip from the left before rendering. Default
    /// `0` = no scroll. The caller increments / decrements this in
    /// response to Left / Right key events.
    #[serde(default)]
    pub h_scroll: usize,
    /// Total width (in **display cells**, not characters — see
    /// [`crate::text_util::char_cell_width`]) of the widest item row,
    /// including the 2-cell selection prefix, any icon, and the main
    /// text. When `max_content_width > visible_area_width` the TUI
    /// rasteriser reserves the bottom row of the list area for a
    /// horizontal scrollbar. `None` = caller hasn't measured content; no
    /// scrollbar shown.
    #[serde(default)]
    pub max_content_width: Option<usize>,
    /// When `true`, backends reserve the rightmost column of the list area
    /// for a vertical scrollbar and paint a track/thumb there. The thumb
    /// position is derived from `scroll_offset` and `items.len()`.
    /// `false` (default) = no vertical scrollbar regardless of list length.
    ///
    /// The vertical scrollbar is independent of `h_scroll` / `max_content_width`:
    /// both scrollbars can be active simultaneously.
    #[serde(default)]
    pub show_v_scrollbar: bool,
}

/// One row in a `ListView`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListItem {
    /// Primary row text.
    pub text: StyledText,
    /// Optional left-aligned icon before the text.
    #[serde(default)]
    pub icon: Option<Icon>,
    /// Optional right-aligned secondary text.
    #[serde(default)]
    pub detail: Option<StyledText>,
    #[serde(default)]
    pub decoration: Decoration,
}

// ── D6 Layout API ───────────────────────────────────────────────────────────
//
// Per Decision D6: primitives return fully-resolved `Layout` structs;
// backends rasterise verbatim. Fourth primitive on the new shape, after
// TabBar, StatusBar, and TreeView. ListView is the flat cousin of
// TreeView — same vertical-stacking layout, minus indent and chevrons.
// An optional title row always renders at the top (outside scroll).

/// Horizontal-scrollbar track geometry, shared by every backend's
/// `list_hscrollbar` rasteriser.
///
/// `char_w` and `row_h` are the caller's native horizontal/vertical
/// units: `1.0` / `1.0` on TUI (a cell), `current_char_width` /
/// `current_line_height` in pixels on GTK, Windows, and macOS.
///
/// When `bordered`, the track insets by one `char_w` on the left and
/// right and moves up one `row_h` so it sits inside the box, above the
/// bottom border, rather than overpainting it — matching
/// [`ListView::layout`]'s `inner_w` and the vertical-scrollbar twin's
/// border handling. When not bordered, the track spans the full width
/// on the bottom row.
pub fn hscrollbar_track(rect: Rect, bordered: bool, char_w: f32, row_h: f32) -> Rect {
    if bordered {
        Rect::new(
            rect.x + char_w,
            rect.y + (rect.height - 2.0 * row_h).max(0.0),
            (rect.width - 2.0 * char_w).max(0.0),
            row_h,
        )
    } else {
        Rect::new(
            rect.x,
            rect.y + (rect.height - row_h).max(0.0),
            rect.width,
            row_h,
        )
    }
}

/// Per-item measurement supplied by the backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ListItemMeasure {
    pub height: f32,
}

impl ListItemMeasure {
    pub fn new(height: f32) -> Self {
        Self { height }
    }

    /// Build from the backend's own [`crate::backend::Metrics`]
    /// (`backend.measure()`) — one list row is one text row
    pub fn from_metrics(m: &crate::backend::Metrics) -> Self {
        Self::new(m.line_height)
    }
}

/// Resolved position of one visible list item after layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleListItem {
    /// Index into the original `ListView.items` Vec.
    pub item_idx: usize,
    pub bounds: Rect,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListViewHit {
    /// Click landed on the title row (non-actionable by default; apps
    /// may still consume it for their own purposes).
    Title,
    /// Click landed on an item row. Carries the item's index into
    /// `ListView.items`.
    Item(usize),
    /// Click landed below the last row, in the viewport's empty tail.
    Empty,
}

/// Fully-resolved list-view layout.
///
/// `#[non_exhaustive]`: a backend-returned value, never constructed by a
/// consumer via struct literal (same convention as the sibling
/// [`crate::DataTableLayout`]), so a future field add here is additive —
/// no `cargo-semver-checks` `constructible_struct_adds_field` finding.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ListViewLayout {
    pub viewport_width: f32,
    pub viewport_height: f32,
    /// Present iff `list.title.is_some()` and the caller passed
    /// `title_height > 0.0`.
    pub title_bounds: Option<Rect>,
    /// Items that are at least partially visible, top to bottom.
    pub visible_items: Vec<VisibleListItem>,
    /// Ordered hit-region list: title first (if present), then items
    /// from top to bottom.
    pub hit_regions: Vec<(Rect, ListViewHit)>,
    /// Scroll offset actually used. Clamped to `[0, items.len())`.
    pub resolved_scroll_offset: usize,
    /// Sub-row pixel remainder when a non-`None` `scroll_px` (see
    /// [`ListView::layout_with_scroll_px`]) is in effect — see
    /// [`crate::primitives::scrollbar::resolve_scroll_px`]. `0.0` from
    /// plain [`ListView::layout`], so a TUI rasteriser that never reads
    /// this field keeps the pre-existing whole-row behaviour. A GUI
    /// painter shifts the first visible item's top up by this many units
    /// (relying on the surface's own clip to show only the portion still
    /// inside the viewport), producing the "partial first row clipped"
    /// look of pixel-smooth scrolling.
    pub scroll_sub_px: f32,
}

impl ListViewLayout {
    /// Test which element (title / item / nothing) contains `(x, y)`.
    pub fn hit_test(&self, x: f32, y: f32) -> ListViewHit {
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        ListViewHit::Empty
    }
}

/// Per-frame paint-time state for a `ListView` — hover/press row
/// tracking plus a pixel-precise scroll position, none of which live on
/// the declarative [`ListView`] descriptor itself (consumers build it
/// with full struct literals, so a required field there is a breaking
/// change, per `PRIMITIVE_RULES.md` rule 8). Field-identical to
/// [`crate::primitives::tree::TreePaintState`], so both are aliases of
/// the shared [`RowPaintState`] — see that type's doc for the full
/// rationale and the `scroll_px` resolution rule.
pub type ListPaintState = RowPaintState;

impl ListView {
    /// Compute the full rendering + hit-test layout for this list.
    ///
    /// Per D6: layout decisions live here; backends iterate
    /// `visible_items` for painting and call `hit_test` for clicks.
    ///
    /// # Arguments
    ///
    /// - `viewport_width`, `viewport_height` — available area in the
    ///   measurer's unit.
    /// - `title_height` — height reserved for the title row at the top.
    ///   Pass `0.0` when `self.title` is `None` or when the caller has
    ///   chosen to collapse it. The title is not subject to
    ///   `scroll_offset` — it stays pinned to the top.
    /// - `measure_item(i)` — height for item `i` (index into
    ///   `self.items`). Receives the row index so backends can vary
    ///   height by decoration or other row state.
    ///
    /// # Row clipping
    ///
    /// The last visible item's `bounds.height` is clipped to what fits
    /// inside the viewport (same semantics as `TreeView::layout`).
    pub fn layout<F>(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        title_height: f32,
        measure_item: F,
    ) -> ListViewLayout
    where
        F: Fn(usize) -> ListItemMeasure,
    {
        self.layout_with_scroll_px(
            viewport_width,
            viewport_height,
            title_height,
            None,
            measure_item,
        )
    }

    /// Like [`Self::layout`], but with an explicit pixel-precise scroll
    /// position (`scroll_px`) a GUI painter has accumulated in a
    /// [`ListPaintState`], instead of only [`ListView::scroll_offset`]'s
    /// whole-row value.
    ///
    /// `scroll_px` wins over `scroll_offset` when it is `Some` and usable
    /// — see [`crate::primitives::scrollbar::resolve_scroll_px`] — and
    /// the sub-row remainder is reported back as
    /// [`ListViewLayout::scroll_sub_px`] for the painter to clip the
    /// first visible item against. `None` (what [`Self::layout`] passes)
    /// resolves to exactly the pre-existing whole-row behaviour.
    pub fn layout_with_scroll_px<F>(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        title_height: f32,
        scroll_px: Option<f32>,
        measure_item: F,
    ) -> ListViewLayout
    where
        F: Fn(usize) -> ListItemMeasure,
    {
        let mut visible_items: Vec<VisibleListItem> = Vec::new();
        let mut hit_regions: Vec<(Rect, ListViewHit)> = Vec::new();

        // Border insets: 1 cell on each side and at top + bottom when
        // `bordered` is set. The title (if present) renders as an
        // overlay on the top border, so it does not consume an extra
        // row in bordered mode.
        let (inset_x, inset_y, item_w, items_h_max) = if self.bordered {
            let iw = (viewport_width - 2.0).max(0.0);
            let ih = (viewport_height - 2.0).max(0.0);
            (1.0, 1.0, iw, ih)
        } else {
            (0.0, 0.0, viewport_width, viewport_height)
        };

        // Title row (if present and reserved a height).
        let title_bounds = if self.title.is_some() && title_height > 0.0 {
            if self.bordered {
                // Overlay on top border at y=0; full viewport width so
                // backends can paint the border + title together.
                let title_h = title_height.min(viewport_height);
                let bounds = Rect::new(0.0, 0.0, viewport_width, title_h);
                hit_regions.push((bounds, ListViewHit::Title));
                Some(bounds)
            } else {
                let title_h = title_height.min(viewport_height);
                let bounds = Rect::new(0.0, 0.0, viewport_width, title_h);
                hit_regions.push((bounds, ListViewHit::Title));
                Some(bounds)
            }
        } else {
            None
        };

        // Items start after the title. In bordered mode the title
        // overlay is title_height tall (line_height on GTK, 1 cell on
        // TUI); items must clear it so they don't overpaint.
        let items_y_start = if self.bordered {
            if title_height > 0.0 {
                title_height.max(inset_y)
            } else {
                inset_y
            }
        } else {
            title_bounds.map(|b| b.y + b.height).unwrap_or(0.0)
        };

        let items_y_end = if self.bordered {
            inset_y + items_h_max
        } else {
            viewport_height
        };

        // Resolve `scroll_offset` vs. `scroll_px` (the latter wins when
        // set) and walk the rows — the shared row-stacking math also
        // used by `TreeView::layout_with_scroll_px`. `()` extra: unlike
        // `TreeView`, `ListView` items have no chevron split.
        let stack = row_stack::layout_row_stack(
            self.scroll_offset,
            scroll_px,
            self.items.len(),
            inset_x,
            items_y_start,
            items_y_end,
            item_w,
            |i| (measure_item(i).height, ()),
        );

        for r in &stack.visible {
            visible_items.push(VisibleListItem {
                item_idx: r.idx,
                bounds: r.bounds,
            });
            hit_regions.push((r.bounds, ListViewHit::Item(r.idx)));
        }

        ListViewLayout {
            viewport_width,
            viewport_height,
            title_bounds,
            visible_items,
            hit_regions,
            resolved_scroll_offset: stack.resolved_scroll_offset,
            scroll_sub_px: stack.scroll_sub_px,
        }
    }

    /// Horizontal scrollbar geometry for this list rendered into `area`.
    ///
    /// This is the single source of truth shared by the rasteriser —
    /// which paints the returned [`Scrollbar`] — and consumers, which
    /// hit-test the resolved `track` / `thumb_start` / `thumb_len` to
    /// implement thumb dragging. Keeping it here (rather than inline in
    /// each backend's `draw_*`) means a consumer never re-derives the
    /// track rect and so can never drift out of sync with the paint.
    /// Reach it backend-agnostically via [`crate::Backend::list_hscrollbar`].
    ///
    /// Returns `None` when no horizontal scrollbar is needed:
    /// `max_content_width` is `None`, or the content fits within the
    /// visible width.
    ///
    /// # Arguments
    ///
    /// - `area` — the list surface rect, in surface-native units (TUI
    ///   cells, GTK / macOS pixels).
    /// - `row_height` — height of one row: `1.0` on TUI, `line_height`
    ///   on pixel backends. The scrollbar occupies the bottom row of the
    ///   area (one row above the bottom border in `bordered` mode), and
    ///   `row_height` also serves as the minimum thumb length.
    pub fn hscrollbar(&self, area: Rect, row_height: f32) -> Option<Scrollbar> {
        let total = self.max_content_width? as f32;
        // Visible content width accounts for the 1-cell bordered inset
        // on each side, matching `layout`'s `inner_w`.
        let visible_w = if self.bordered {
            (area.width - 2.0).max(0.0)
        } else {
            area.width
        };
        if total <= visible_w {
            return None;
        }
        // Track sits on the bottom row; in bordered mode it moves up one
        // row and insets 1 cell on each side so it stays inside the box,
        // above the bottom border, rather than overpainting it.
        let track = hscrollbar_track(area, self.bordered, 1.0, row_height);
        Some(Scrollbar::horizontal(
            self.id,
            track,
            self.h_scroll as f32,
            total,
            visible_w,
            row_height,
        ))
    }
    /// Vertical scrollbar geometry for this list rendered into `area`.
    ///
    /// This is the single source of truth shared by the rasteriser — which
    /// paints the returned [`Scrollbar`] — and consumers, which hit-test the
    /// resolved `track` / `thumb_start` / `thumb_len` to implement thumb
    /// dragging. Keeping it here (rather than inline in each backend's
    /// `draw_*`) means a consumer never re-derives the track rect and so can
    /// never drift out of sync with the paint. Reach it backend-agnostically
    /// via [`crate::Backend::list_vscrollbar`].
    ///
    /// Returns `None` when no vertical scrollbar is needed: `show_v_scrollbar`
    /// is `false`, the list is empty, or all items fit in the visible viewport.
    ///
    /// # Arguments
    ///
    /// - `area` — the list surface rect, in surface-native units (TUI cells,
    ///   GTK / macOS pixels).
    /// - `row_height` — height of one row: `1.0` on TUI, `line_height` on
    ///   pixel backends. The scrollbar occupies the rightmost column of the
    ///   area (one column inside the right border in `bordered` mode), and
    ///   `row_height` also serves as both the column width and the minimum
    ///   thumb length (i.e. `1.0` on TUI — one cell wide, one cell tall
    ///   minimum thumb).
    pub fn vscrollbar(&self, area: Rect, row_height: f32) -> Option<Scrollbar> {
        self.vscrollbar_with_track_width(area, row_height, row_height)
    }

    /// Like [`vscrollbar`](Self::vscrollbar), but with an explicit
    /// `track_width` instead of defaulting it to `row_height` — the same
    /// split [`crate::primitives::tree::TreeView::vscrollbar_with_track_width`]
    /// already has.
    ///
    /// - `track_width` — width of the scrollbar column, occupying the
    ///   rightmost `track_width` of `area`: `1.0` on TUI (one cell), or
    ///   [`crate::style::Style::scrollbar_width`] on pixel backends —
    ///   [`native_surface_paint::paint`] passes `style.scrollbar_width`.
    ///   `row_height` still serves as the row-counting pitch (how many
    ///   rows fit the track height) and the minimum thumb length,
    ///   independent of the column's own width.
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
    /// [`ListPaintState`] so the thumb tracks the continuous pixel
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
        if !self.show_v_scrollbar {
            return None;
        }
        if row_height <= 0.0 || track_width <= 0.0 {
            return None;
        }
        let total = self.items.len() as f32;
        if total == 0.0 {
            return None;
        }
        // Track occupies the rightmost content column; its height spans the
        // content rows (below the title in flat mode, between borders in
        // bordered mode). In `bordered` mode the column is inset
        // `row_height` from the right edge (the same inset `layout` uses
        // for its last content column) rather than the 1-unit border
        // stroke itself, `track_width` wide either way.
        let (track_x, track_y, track_h) = if self.bordered {
            let x = area.x + area.width - row_height - track_width;
            let y = area.y + row_height;
            let h = (area.height - 2.0 * row_height).max(0.0);
            (x, y, h)
        } else {
            // In flat mode: track at the rightmost column, starting below
            // the title row (if any).
            let title_h = if self.title.is_some() {
                row_height
            } else {
                0.0
            };
            let x = area.x + area.width - track_width;
            let y = area.y + title_h;
            let h = (area.height - title_h).max(0.0);
            (x, y, h)
        };
        if track_h <= 0.0 {
            return None;
        }
        // Visible rows = how many full rows fit in the track height.
        let visible = (track_h / row_height).floor();
        if total <= visible {
            return None;
        }
        let track = Rect::new(track_x, track_y, track_width, track_h);
        // `scroll_px` wins over `scroll_offset` when set, so the thumb
        // moves smoothly alongside the sub-row paint shift
        // `layout_with_scroll_px` applies.
        Some(row_stack::vscrollbar_thumb(
            self.id,
            track,
            self.scroll_offset,
            scroll_px,
            self.items.len(),
            row_height,
            visible,
        ))
    }
}

#[cfg(test)]
mod hscrollbar_tests {
    use super::*;

    /// Build a flat list whose widest row is `content_width` chars wide,
    /// scrolled to `h_scroll`. `max` toggles whether `max_content_width`
    /// is populated.
    fn list(content_width: usize, h_scroll: usize, max: bool) -> ListView {
        ListView {
            id: WidgetId::new("l"),
            title: None,
            items: vec![ListItem {
                text: StyledText::plain(&"x".repeat(content_width)),
                detail: None,
                icon: None,
                decoration: Decoration::default(),
            }],
            selected_idx: 0,
            scroll_offset: 0,
            has_focus: true,
            bordered: false,
            h_scroll,
            max_content_width: max.then_some(content_width),
            show_v_scrollbar: false,
        }
    }

    #[test]
    fn none_when_max_content_width_unset() {
        let l = list(100, 0, false);
        assert!(l.hscrollbar(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0).is_none());
    }

    #[test]
    fn none_when_content_fits() {
        // content_width 20 == visible 20 → no scrollbar (strictly wider only).
        let l = list(20, 0, true);
        assert!(l.hscrollbar(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0).is_none());
    }

    #[test]
    fn flat_track_spans_bottom_row() {
        let l = list(40, 0, true);
        let sb = l
            .hscrollbar(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0)
            .expect("overflow should yield a scrollbar");
        // Flat: full width, bottom-most row.
        assert_eq!(sb.track.x, 0.0);
        assert_eq!(sb.track.width, 20.0);
        assert_eq!(sb.track.y, 9.0);
        assert_eq!(sb.track.height, 1.0);
        // Thumb starts at the left when h_scroll == 0.
        assert_eq!(sb.thumb_start, 0.0);
        assert!(sb.thumb_len > 0.0 && sb.thumb_len < sb.track.width);
    }

    #[test]
    fn bordered_track_inset_above_bottom_border() {
        let mut l = list(40, 0, true);
        l.bordered = true;
        let sb = l
            .hscrollbar(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0)
            .expect("overflow should yield a scrollbar");
        // Inset 1 cell each side; one row above the bottom border.
        assert_eq!(sb.track.x, 1.0);
        assert_eq!(sb.track.width, 18.0);
        assert_eq!(sb.track.y, 8.0);
    }

    #[test]
    fn h_scroll_advances_thumb() {
        let at_zero = list(40, 0, true)
            .hscrollbar(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0)
            .unwrap();
        let scrolled = list(40, 10, true)
            .hscrollbar(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0)
            .unwrap();
        assert!(
            scrolled.thumb_start > at_zero.thumb_start,
            "scrolling right should move the thumb right"
        );
    }
}

#[cfg(test)]
mod hscrollbar_track_tests {
    use super::*;

    /// Verifies the inset math holds at pixel units (`char_w != 1.0`,
    /// `row_h != 1.0`), not only at TUI-cell units: a bug that only shows
    /// up at real pixel dimensions would slip past other tests entirely.
    #[test]
    fn flat_track_spans_full_width_bottom_row() {
        let rect = Rect::new(0.0, 0.0, 200.0, 100.0);
        let track = hscrollbar_track(rect, false, 8.0, 16.0);
        assert_eq!(track.x, 0.0);
        assert_eq!(track.width, 200.0);
        assert_eq!(track.y, 84.0); // 100 - 16
        assert_eq!(track.height, 16.0);
    }

    #[test]
    fn bordered_track_insets_one_char_width_each_side_and_sits_above_border() {
        let rect = Rect::new(0.0, 0.0, 200.0, 100.0);
        let track = hscrollbar_track(rect, true, 8.0, 16.0);
        // Inset by one char width (8px) on each side — not a hardcoded
        // 1.0, which is what the pre-fix macOS copy effectively used (it
        // ignored `bordered` entirely and always produced the flat track,
        // overpainting the right border by a full char width and the
        // bottom border by a full row).
        assert_eq!(track.x, 8.0);
        assert_eq!(track.width, 184.0); // 200 - 2*8
                                        // Moves up one row so it sits above the bottom border instead of
                                        // overpainting it.
        assert_eq!(track.y, 68.0); // 100 - 2*16
        assert_eq!(track.height, 16.0);
    }

    #[test]
    fn bordered_track_never_overlaps_border_columns() {
        let rect = Rect::new(0.0, 0.0, 200.0, 100.0);
        let track = hscrollbar_track(rect, true, 8.0, 16.0);
        // The thumb bounds (contained within `track`) must sit strictly
        // inside the border on both sides — the acceptance criterion from
        // "thumb bounds sit inside the border on a bordered list."
        assert!(track.x >= rect.x + 8.0);
        assert!(track.x + track.width <= rect.x + rect.width - 8.0);
        assert!(track.y + track.height <= rect.y + rect.height - 16.0);
    }
}

#[cfg(test)]
mod vscrollbar_tests {
    use super::*;

    /// Build a list with `n_items` rows and vertical-scroll enabled,
    /// scrolled to `scroll_offset`.
    fn vlist(n_items: usize, scroll_offset: usize) -> ListView {
        ListView {
            id: WidgetId::new("l"),
            title: None,
            items: (0..n_items)
                .map(|i| ListItem {
                    text: StyledText::plain(&format!("item {}", i)),
                    detail: None,
                    icon: None,
                    decoration: Decoration::default(),
                })
                .collect(),
            selected_idx: 0,
            scroll_offset,
            has_focus: true,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: true,
        }
    }

    #[test]
    fn none_when_show_v_scrollbar_false() {
        let mut l = vlist(50, 0);
        l.show_v_scrollbar = false;
        assert!(l.vscrollbar(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0).is_none());
    }

    #[test]
    fn none_when_items_fit_in_viewport() {
        // 5 items, 10 rows available → all fit, no scrollbar.
        let l = vlist(5, 0);
        assert!(l.vscrollbar(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0).is_none());
    }

    #[test]
    fn none_when_items_exactly_fill_viewport() {
        // 10 items, 10 rows → exactly fits (total == visible), no scrollbar.
        let l = vlist(10, 0);
        assert!(l.vscrollbar(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0).is_none());
    }

    #[test]
    fn flat_track_spans_right_column() {
        let l = vlist(20, 0);
        let sb = l
            .vscrollbar(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0)
            .expect("overflow should yield a scrollbar");
        // Flat: rightmost column (x=19), full height, 1 cell wide.
        assert_eq!(sb.track.x, 19.0);
        assert_eq!(sb.track.y, 0.0);
        assert_eq!(sb.track.width, 1.0);
        assert_eq!(sb.track.height, 10.0);
        // Thumb starts at the top when scroll_offset == 0.
        assert_eq!(sb.thumb_start, 0.0);
        assert!(sb.thumb_len > 0.0 && sb.thumb_len < sb.track.height);
    }

    #[test]
    fn flat_track_below_title_row() {
        let mut l = vlist(20, 0);
        l.title = Some(StyledText::plain("Title"));
        let sb = l
            .vscrollbar(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0)
            .expect("overflow should yield a scrollbar");
        // Track starts at y=1 (below title row) and is 1 row shorter.
        assert_eq!(sb.track.y, 1.0);
        assert_eq!(sb.track.height, 9.0);
    }

    #[test]
    fn bordered_track_inset_inside_right_border() {
        let mut l = vlist(20, 0);
        l.bordered = true;
        let sb = l
            .vscrollbar(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0)
            .expect("overflow should yield a scrollbar");
        // In bordered mode: right border at x=19, track at x=18 (last content col).
        assert_eq!(sb.track.x, 18.0);
        // Track spans between top border (y=0) and bottom border (y=9), so y=1 height=8.
        assert_eq!(sb.track.y, 1.0);
        assert_eq!(sb.track.height, 8.0);
    }

    #[test]
    fn scroll_offset_advances_thumb() {
        let at_zero = vlist(30, 0)
            .vscrollbar(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0)
            .unwrap();
        let scrolled = vlist(30, 15)
            .vscrollbar(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0)
            .unwrap();
        assert!(
            scrolled.thumb_start > at_zero.thumb_start,
            "scrolling down should move the thumb down"
        );
    }

    #[test]
    fn scroll_px_moves_the_thumb_smoothly_between_whole_rows() {
        // Row height 22px (a GUI-ish value): half a row of `scroll_px`
        // moves the thumb strictly between the whole-row-0 and
        // whole-row-1 positions — not snapped to either.
        let row0 = vlist(30, 0)
            .vscrollbar_with_track_width(Rect::new(0.0, 0.0, 20.0, 10.0 * 22.0), 22.0, 22.0)
            .unwrap();
        let row1 = vlist(30, 1)
            .vscrollbar_with_track_width(Rect::new(0.0, 0.0, 20.0, 10.0 * 22.0), 22.0, 22.0)
            .unwrap();
        let half_sb = vlist(30, 0)
            .vscrollbar_with_scroll_px(
                Rect::new(0.0, 0.0, 20.0, 10.0 * 22.0),
                22.0,
                22.0,
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
        let mut l = vlist(30, 0);
        l.scroll_offset = 5; // stale, overridden by scroll_px
        let sb = l
            .vscrollbar_with_scroll_px(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0, 1.0, Some(0.0))
            .unwrap();
        let at_zero = vlist(30, 0)
            .vscrollbar(Rect::new(0.0, 0.0, 20.0, 10.0), 1.0)
            .unwrap();
        assert_eq!(sb.thumb_start, at_zero.thumb_start);
    }
}

// ── PaintSurface paint ─────────────────────────────────────────────
//
// This is the one shared implementation of list painting, written
// against [`crate::paint_surface::PaintSurface`] instead of any one
// backend's API — see `crate::primitives::split_tree::native_surface_paint`
// and `crate::primitives::scrollbar::native_surface_paint` for the same
// pattern applied to earlier primitives.
//
// The vertical scrollbar is now painted via
// [`crate::primitives::scrollbar::native_surface_paint::paint`],
// using real track/thumb geometry that `Backend::list_vscrollbar`
// already provides.
//
// Note: `bordered`'s frame stroke remains backend-specific — GTK
// clips to (and later strokes) a *rounded* rect, Windows fills a plain
// square 1-DIP frame, and macOS renders no frame at all yet (see
// `macos::list`'s module doc, "Scope omissions"). Each backend's thin
// wrapper still paints its own frame immediately before/after calling
// this shared `paint` for the content. `supports_border` below only
// toggles the *content*-side effects of `bordered` (the inset content
// area + overlay title vs. the flat header+rows layout) that a backend
// without frame support should skip — matching what `macos::list`'s
// pre-migration `draw_list` already did.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{ListView, ListViewLayout};
    use crate::paint_surface::PaintSurface;
    use crate::primitives::row_stack;
    use crate::style::Style;
    use crate::theme::Theme;
    use crate::types::Decoration;
    use crate::Rect;

    /// Paint a [`ListView`]'s content — background, optional title,
    /// visible rows (selection/decoration/icon/detail), and h/v
    /// scrollbars — onto `surface` at `area`.
    ///
    /// - `list_layout` must be the same [`ListViewLayout`] the caller
    ///   uses for hit-testing (typically `gtk_list_layout`/
    ///   `mac_list_layout`/`win_list_layout`'s return value) so paint
    ///   and hit-test can never disagree.
    /// - `line_height` is one row's height in `surface`'s native units
    ///   (pixels on every current caller).
    /// - `nerd_fonts_enabled` selects `Icon::glyph` vs. `Icon::fallback`.
    /// - `supports_border` — see this module's doc for what it toggles.
    /// - `supports_hscrollbar` — `false` on Windows only: `win_list_layout`
    ///   never reserves a bottom row for an overflowing
    ///   `ListView::max_content_width` (a real, tracked-separately gap —
    ///   see `win::list`'s module doc, "Known gap: no horizontal
    ///   scrollbar"), so painting one there without a reservation would
    ///   overlap the last content row — a new (and wrong) visual, not a
    ///   fix. `true` on GTK/macOS, which both already reserve the row.
    /// - `style` supplies [`Style::row_padding_x`], the horizontal inset
    ///   a row's leading content (chevron-less here, so its icon/text)
    ///   starts at — callers pass `&self.current_style` (GTK/macOS);
    ///   Windows passes `&Style::default()`, matching `win::list`'s own
    ///   documented "no live theme either" gap.
    /// - `hovered_idx` / `pressed_idx` — index into `list.items` (not
    ///   `vis_item.item_idx` offset by anything) to tint via
    ///   [`Theme::list_hover_bg`] / [`Theme::list_pressed_bg`]. `None`
    ///   paints byte-for-byte what this function painted before hover
    ///   support existed. Applied on top of whatever background the row
    ///   already has (selected, header, or base fill) — pressed takes
    ///   priority over hover, matching every other
    ///   `InteractionState`-adjacent rasteriser in this crate.
    /// - `focus_ring_visible` — when `true`, strokes a
    ///   `Theme::focus_ring_color` ring at `Style::focus_ring_width`,
    ///   flush with [`ListView::selected_idx`]'s row bounds, on top of
    ///   whatever background that row already has. Separate from
    ///   [`ListView::has_focus`] (see that field's doc for why) — the
    ///   caller passes `ring.is_focused(&list.id) &&
    ///   ring.is_focus_visible()` here, not `list.has_focus`. `false`
    ///   paints byte-for-byte what this function painted before the
    ///   ring existed.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint(
        list: &ListView,
        area: Rect,
        list_layout: &ListViewLayout,
        line_height: f32,
        nerd_fonts_enabled: bool,
        supports_border: bool,
        supports_hscrollbar: bool,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        style: &Style,
        hovered_idx: Option<usize>,
        pressed_idx: Option<usize>,
        focus_ring_visible: bool,
    ) {
        paint_with_scroll_px(
            list,
            area,
            list_layout,
            line_height,
            nerd_fonts_enabled,
            supports_border,
            supports_hscrollbar,
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
    /// (`scroll_px`) that wins over [`ListView::scroll_offset`] for both
    /// the rows (via `list_layout.visible_items`, already resolved
    /// against `scroll_px` by whichever of
    /// [`ListView::layout_with_scroll_px`]/`mac_list_layout_with_scroll_px`
    /// built `list_layout`) and the vertical scrollbar thumb (resolved
    /// here, via [`ListView::vscrollbar_with_scroll_px`]).
    ///
    /// `scroll_px: None` (what [`paint`] passes) is pixel-identical to
    /// `paint` — the items-region clip that bounds a sub-row shift
    /// against the pinned title is pushed only when `scroll_px.is_some()`,
    /// so the `None` path never pushes it at all.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint_with_scroll_px(
        list: &ListView,
        area: Rect,
        list_layout: &ListViewLayout,
        line_height: f32,
        nerd_fonts_enabled: bool,
        supports_border: bool,
        supports_hscrollbar: bool,
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

        let base_bg = if list.bordered {
            theme.surface_bg
        } else {
            theme.background
        };
        let border_inset = if list.bordered && supports_border {
            1.0
        } else {
            0.0
        };
        let title_overlay = list.bordered && supports_border;

        surface.surface_push_clip(area);
        surface.surface_fill_rect(area, base_bg);

        let char_w = surface.surface_measure_text("M").0.max(1.0);
        let h_off_px = list.h_scroll as f32 * char_w;
        let visible_px = (area.width - border_inset * 2.0).max(0.0);
        let needs_hscrollbar = supports_hscrollbar
            && list
                .max_content_width
                .is_some_and(|n| n as f32 * char_w > visible_px);

        if title_overlay {
            if let Some(ref title) = list.title {
                let title_text: String = title.spans.iter().map(|s| s.text.as_str()).collect();
                let label = format!(" {} ", title_text.trim());
                let (tw, th) = surface.surface_measure_text(&label);
                let title_x = area.x + 8.0;
                let title_y = area.y + (line_height - th) / 2.0;
                surface.surface_fill_rect(
                    Rect::new(title_x - 2.0, area.y, tw + 4.0, line_height),
                    base_bg,
                );
                surface.surface_draw_text_run(
                    Rect::new(title_x, title_y, tw, th),
                    &label,
                    theme.title_fg,
                );
            }
        } else if let (Some(title_bounds), Some(title)) =
            (list_layout.title_bounds, list.title.as_ref())
        {
            let tb = Rect::new(
                area.x + title_bounds.x,
                area.y + title_bounds.y,
                title_bounds.width,
                title_bounds.height,
            );
            surface.surface_fill_rect(tb, theme.header_bg);
            let title_text: String = title.spans.iter().map(|s| s.text.as_str()).collect();
            let (_, text_h) = surface.surface_measure_text(&title_text);
            surface.surface_draw_text_run(
                Rect::new(
                    tb.x + 2.0,
                    tb.y + (tb.height - text_h) / 2.0,
                    tb.width,
                    text_h,
                ),
                &title_text,
                theme.header_fg,
            );
        }

        let item_x_offset = area.x + border_inset;
        let item_y_offset = area.y + border_inset;

        // Clip row painting to the region below the pinned title —
        // `scroll_px`'s sub-row remainder shifts the first visible
        // item's `bounds.y` up by less than one row (see
        // `ListView::layout_with_scroll_px`), which can land inside the
        // title's own band. Without this clip that item's background
        // would paint over the bottom edge of a still-pinned title.
        //
        // Gated on `scroll_px.is_some()`: with `scroll_px: None`, no item
        // is ever shifted above `title_bounds`'s bottom, so the clip
        // would be a no-op for row painting — but it would still clip
        // anything else that paints outside the items band (e.g. a
        // focus-ring stroke on the first row of an untitled, bordered
        // list), which the `None` path must leave pixel-identical to
        // before this fn existed.
        let items_clip_active = scroll_px.is_some();
        if items_clip_active {
            let title_h = list_layout
                .title_bounds
                .map(|b| b.y + b.height)
                .unwrap_or(0.0);
            let items_region_y = item_y_offset + title_h;
            let items_region_h = (area.height - border_inset * 2.0 - title_h).max(0.0);
            surface.surface_push_clip(Rect::new(
                item_x_offset,
                items_region_y,
                (area.width - border_inset * 2.0).max(0.0),
                items_region_h,
            ));
        }

        for vis_item in &list_layout.visible_items {
            let item = &list.items[vis_item.item_idx];
            let row_x = item_x_offset + vis_item.bounds.x;
            let row_y = item_y_offset + vis_item.bounds.y;
            let row_w = vis_item.bounds.width;
            let row_h = vis_item.bounds.height;

            let is_selected = vis_item.item_idx == list.selected_idx && list.has_focus;
            let decoration_fg = match item.decoration {
                Decoration::Error => theme.error_fg,
                Decoration::Warning => theme.warning_fg,
                Decoration::Muted => theme.muted_fg,
                Decoration::Header => theme.header_fg,
                _ => theme.surface_fg,
            };
            let row_bg = if is_selected {
                theme.selected_bg
            } else if matches!(item.decoration, Decoration::Header) {
                theme.header_bg
            } else {
                base_bg
            };
            let row_bg = if Some(vis_item.item_idx) == pressed_idx {
                theme.list_pressed_bg(row_bg)
            } else if Some(vis_item.item_idx) == hovered_idx {
                theme.list_hover_bg(row_bg)
            } else {
                row_bg
            };

            let row_rect = Rect::new(row_x, row_y, row_w, row_h);
            surface.surface_fill_rect(row_rect, row_bg);

            // Focus-visible ring: a square stroke flush with the
            // selected row's own bounds, in `Theme::focus_ring_color`
            // at `Style::focus_ring_width`. Unlike `Button`'s/
            // `TextInput`'s ring, this one is not outset — adjacent
            // rows have no gap between them, so an outset stroke would
            // bleed into the row above/below. Gated on
            // `focus_ring_visible`, not `list.has_focus` — see this
            // fn's own doc and `ListView::has_focus`'s doc for why the
            // two are kept separate.
            if is_selected && focus_ring_visible {
                surface.surface_stroke_rect(
                    row_rect,
                    theme.focus_ring_color(),
                    style.focus_ring_width,
                );
            }

            // Per-row clip: with h_scroll the cursor starts to the left
            // of `row_x`, so scrolled-off glyphs would paint outside the
            // row band without an explicit clip.
            surface.surface_push_clip(row_rect);
            // No `▶ ` prefix glyph on GUI — `row_bg` above already marks
            // the selected row with `theme.selected_bg`, so the chevron
            // is a redundant terminal affordance here (the TUI painter
            // still draws it — it has no row-fill cue to lean on).
            let mut cursor_x = row_x + style.row_padding_x - h_off_px;

            cursor_x = row_stack::row_content_paint::paint_row_icon(
                surface,
                item.icon.as_ref(),
                nerd_fonts_enabled,
                decoration_fg,
                row_y,
                row_h,
                cursor_x,
                |s, r, glyph, fg| s.surface_draw_text_run(r, glyph, fg),
            );

            let detail_info =
                row_stack::row_content_paint::measure_row_detail(surface, item.detail.as_ref());
            let detail_reserve = row_stack::row_content_paint::row_detail_reserve(&detail_info);
            // text_right_limit is absolute; the h_scroll shift of
            // cursor_x does not affect where the detail reserve
            // boundary sits.
            let text_right_limit = row_x + row_w - detail_reserve - 4.0;

            // A span that overflows `text_right_limit` is ellipsis-
            // truncated (via the shared `fit_text_prefix`) rather than
            // hard-clipped mid-glyph — see `Tree`'s identical treatment,
            // the pattern every overflowing primitive now follows.
            for span in &item.text.spans {
                let remaining = text_right_limit - cursor_x;
                if remaining <= 0.0 {
                    break;
                }
                let span_fg = span.fg.unwrap_or(decoration_fg);
                let (sw, sh) = surface.surface_measure_text_styled(&span.text, span.bold);
                let (paint_text, paint_w, truncated) = if sw <= remaining {
                    (span.text.clone(), sw, false)
                } else {
                    let (t, w) = crate::paint_surface::fit_text_prefix(
                        surface, &span.text, span.bold, remaining,
                    );
                    (t, w, true)
                };
                if paint_w <= 0.0 {
                    break;
                }
                if let Some(sbg) = span.bg {
                    surface.surface_fill_rect(Rect::new(cursor_x, row_y, paint_w, row_h), sbg);
                }
                surface.surface_draw_text_run_styled(
                    Rect::new(cursor_x, row_y + (row_h - sh) / 2.0, paint_w, sh),
                    &paint_text,
                    span_fg,
                    span.bold,
                    false,
                    false,
                    1.0,
                );
                cursor_x += paint_w;
                if truncated {
                    break;
                }
            }

            // Detail text is pinned to the visible viewport (does not
            // scroll with h_scroll) so it stays readable regardless of
            // scroll position — render it outside the h_scroll-shifted
            // row clip.
            surface.surface_pop_clip();

            row_stack::row_content_paint::paint_row_detail(
                surface,
                &detail_info,
                Rect::new(row_x, row_y, row_w, row_h),
                4.0,
                cursor_x,
                theme.muted_fg,
            );
        }

        if items_clip_active {
            surface.surface_pop_clip();
        }

        // ── Horizontal scrollbar ──────────────────────────────────────
        // Painted after items so it overlays the bottom row's background.
        if needs_hscrollbar {
            let content_px = list.max_content_width.unwrap_or(0) as f32 * char_w;
            let track_y = if list.bordered {
                area.y + area.height - border_inset - line_height
            } else {
                area.y + area.height - line_height
            };
            let (track_x, track_w) = if list.bordered {
                (
                    area.x + border_inset,
                    (area.width - 2.0 * border_inset).max(0.0),
                )
            } else {
                (area.x, area.width)
            };
            let hsb_track = Rect::new(track_x, track_y, track_w, line_height);
            let hsb = crate::primitives::scrollbar::Scrollbar::horizontal(
                list.id,
                hsb_track,
                list.h_scroll as f32 * char_w,
                content_px,
                visible_px,
                line_height,
            );
            crate::primitives::scrollbar::native_surface_paint::paint(&hsb, surface, theme);
        }

        // ── Vertical scrollbar ─────────────────────────────────────
        // Track width is `Style::scrollbar_width`, independent of row
        // pitch — see that field's own doc. `scroll_px` wins over
        // `scroll_offset` when set, so the thumb tracks the same
        // sub-row position the items above were painted at.
        if let Some(vsb) =
            list.vscrollbar_with_scroll_px(area, line_height, style.scrollbar_width, scroll_px)
        {
            crate::primitives::scrollbar::native_surface_paint::paint(&vsb, surface, theme);
        }

        surface.surface_pop_clip();
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::primitives::list::ListItem;
        use crate::types::{Color, Icon, StyledText, WidgetId};
        use crate::Image;

        /// Records every fill + stroke + text-run call — mirrors
        /// `primitives::scrollbar`'s `RecordingSurface` test double,
        /// scoped to the verbs this primitive uses, so this test runs on
        /// any host without Cairo/Core Graphics/Direct2D.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            strokes: Vec<(Rect, Color, f32)>,
            texts: Vec<(Rect, String, Color)>,
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
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        use crate::event::Point;

        fn item(label: &str) -> ListItem {
            ListItem {
                text: StyledText::plain(label.to_string()),
                icon: None,
                detail: None,
                decoration: Decoration::Normal,
            }
        }

        fn vlist(n_items: usize) -> ListView {
            ListView {
                id: WidgetId::new("l"),
                title: None,
                items: (0..n_items).map(|i| item(&format!("row {i}"))).collect(),
                selected_idx: 0,
                scroll_offset: 0,
                has_focus: true,
                bordered: false,
                h_scroll: 0,
                max_content_width: None,
                show_v_scrollbar: true,
            }
        }

        const LINE_HEIGHT: f32 = 16.0;
        const AREA: Rect = Rect::new(0.0, 0.0, 100.0, 80.0);

        /// Verifies that the vertical scrollbar track and thumb are
        /// painted using the geometry that [`ListView::vscrollbar`]
        /// resolves.
        #[test]
        fn paints_vertical_scrollbar_track_when_enabled() {
            let list = vlist(50); // overflows AREA's 80px / 16px = 5 rows.
            let layout = list.layout(AREA.width, AREA.height, 0.0, |_| {
                super::super::ListItemMeasure::new(LINE_HEIGHT)
            });
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(
                &list,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                true,
                true,
                &mut surface,
                &theme,
                &Style::default(),
                None,
                None,
                false,
            );

            let expected = list
                .vscrollbar_with_track_width(AREA, LINE_HEIGHT, Style::default().scrollbar_width)
                .expect("50 rows in an 80px/16px viewport must need a v-scrollbar");
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

        /// `layout_with_scroll_px`'s `scroll_px` set to half a row shifts
        /// every painted row's `y` up by exactly that half-row — the
        /// "partial first row clipped" GUI look of pixel-smooth
        /// scrolling. The row is recorded with a `y` above `area.y` here
        /// (`RecordingSurface` doesn't actually clip); the real
        /// `surface_push_clip(area)` call right before painting rows is
        /// what makes a live GUI surface show only the portion still
        /// inside the viewport.
        #[test]
        fn scroll_px_shifts_rows_by_the_sub_row_remainder() {
            let list = vlist(50);
            let layout = list.layout_with_scroll_px(
                AREA.width,
                AREA.height,
                0.0,
                Some(LINE_HEIGHT / 2.0),
                |_| super::super::ListItemMeasure::new(LINE_HEIGHT),
            );
            assert_eq!(layout.resolved_scroll_offset, 0);
            assert!((layout.scroll_sub_px - LINE_HEIGHT / 2.0).abs() < 0.01);

            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(
                &list,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                true,
                true,
                &mut surface,
                &theme,
                &Style::default(),
                None,
                None,
                false,
            );

            // Item 0 is `selected_idx` with `has_focus: true`, so its
            // row-background fill is uniquely `theme.selected_bg` —
            // distinguishing it from the area background and every
            // other (unselected) row's fill.
            let selected_fill = surface
                .fills
                .iter()
                .find(|(_, c)| *c == theme.selected_bg)
                .expect("row 0's selected background should still be painted, just shifted up");
            assert!(
                selected_fill.0.y < AREA.y,
                "half a row of scroll_px should shift row 0 above the viewport top, got y={}",
                selected_fill.0.y
            );
        }

        /// The painted track's width follows `Style::scrollbar_width`,
        /// not `line_height` — a host-set non-default width must
        /// actually reach the paint surface.
        #[test]
        fn vertical_scrollbar_track_width_follows_style_token() {
            let list = vlist(50);
            let layout = list.layout(AREA.width, AREA.height, 0.0, |_| {
                super::super::ListItemMeasure::new(LINE_HEIGHT)
            });
            let style = Style::default().with_scrollbar_width(20.0);
            let mut surface = RecordingSurface::default();
            paint(
                &list,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                true,
                true,
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

        /// The "always-visible bar" acceptance item is not a painter bug:
        /// [`ListView::vscrollbar_with_track_width`] already returns `None`
        /// once every row fits the viewport (`total <= visible`, checked
        /// before any track geometry is computed), and this shared paint
        /// path is the only place any backend's vertical scrollbar fill
        /// comes from — so a list whose content fits paints no scrollbar
        /// fill at all, on every backend, with no per-backend code to fix.
        #[test]
        fn no_vertical_scrollbar_fill_when_content_fits_the_viewport() {
            // AREA is 80px tall at LINE_HEIGHT 16 → 5 rows fit; 3 items
            // is strictly fewer, so no scrollbar is needed.
            let list = vlist(3);
            let layout = list.layout(AREA.width, AREA.height, 0.0, |_| {
                super::super::ListItemMeasure::new(LINE_HEIGHT)
            });
            let mut surface = RecordingSurface::default();
            paint(
                &list,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                true,
                true,
                &mut surface,
                &Theme::default(),
                &Style::default(),
                None,
                None,
                false,
            );
            assert!(
                list.vscrollbar(AREA, LINE_HEIGHT).is_none(),
                "3 items in a 5-row viewport must not need a v-scrollbar"
            );
            for (r, _) in &surface.fills {
                assert!(
                    r.x + r.width <= AREA.x + AREA.width + 0.01,
                    "unexpected fill past the list's right edge (a scrollbar \
                     track painted when content fits): {:?}",
                    r
                );
            }
        }

        #[test]
        fn no_vertical_scrollbar_fill_when_show_v_scrollbar_false() {
            let mut list = vlist(50);
            list.show_v_scrollbar = false;
            let layout = list.layout(AREA.width, AREA.height, 0.0, |_| {
                super::super::ListItemMeasure::new(LINE_HEIGHT)
            });
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(
                &list,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                true,
                true,
                &mut surface,
                &theme,
                &Style::default(),
                None,
                None,
                false,
            );
            assert!(list.vscrollbar(AREA, LINE_HEIGHT).is_none());
            // Every recorded fill must stay within the list's own bounds
            // (nothing painted a scrollbar track past the right edge).
            for (r, _) in &surface.fills {
                assert!(
                    r.x + r.width <= AREA.x + AREA.width + 0.01,
                    "unexpected fill past the list's right edge: {:?}",
                    r
                );
            }
        }

        /// Windows-only gap preserved deliberately (see `win::list`'s
        /// module doc, "Known gap: no horizontal scrollbar", and this
        /// fn's own doc for `supports_hscrollbar`): with
        /// `supports_hscrollbar: false`, an overflowing
        /// `max_content_width` must never paint an h-scrollbar track,
        /// even though the same list *would* need one on a backend that
        /// passes `true`.
        #[test]
        fn no_hscrollbar_fill_when_supports_hscrollbar_false() {
            let mut list = vlist(3);
            list.max_content_width = Some(1000); // wildly overflows AREA.
            list.show_v_scrollbar = false;
            let layout = list.layout(AREA.width, AREA.height, 0.0, |_| {
                super::super::ListItemMeasure::new(LINE_HEIGHT)
            });
            let theme = Theme::default();

            let mut supported = RecordingSurface::default();
            paint(
                &list,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                true,
                true,
                &mut supported,
                &theme,
                &Style::default(),
                None,
                None,
                false,
            );
            assert!(
                !supported.fills.is_empty(),
                "sanity: supports_hscrollbar: true must paint something for an \
                 overflowing max_content_width"
            );

            let mut unsupported = RecordingSurface::default();
            paint(
                &list,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                true,
                false,
                &mut unsupported,
                &theme,
                &Style::default(),
                None,
                None,
                false,
            );
            // Every fill must stay within the row content area — none of
            // them may be the bottom-row-height h-scrollbar track, which
            // would sit at `AREA.y + AREA.height - LINE_HEIGHT`.
            let hscrollbar_track_y = AREA.y + AREA.height - LINE_HEIGHT;
            for (r, _) in &unsupported.fills {
                assert!(
                    (r.y - hscrollbar_track_y).abs() > 0.01,
                    "supports_hscrollbar: false must not paint a fill at the \
                     h-scrollbar track's y ({hscrollbar_track_y}), got {:?}",
                    r
                );
            }
        }

        #[test]
        fn selected_row_paints_selected_bg() {
            let mut list = vlist(3);
            list.selected_idx = 1;
            list.has_focus = true;
            let layout = list.layout(AREA.width, AREA.height, 0.0, |_| {
                super::super::ListItemMeasure::new(LINE_HEIGHT)
            });
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(
                &list,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                true,
                true,
                &mut surface,
                &theme,
                &Style::default(),
                None,
                None,
                false,
            );

            let row1 = layout
                .visible_items
                .iter()
                .find(|v| v.item_idx == 1)
                .expect("row 1 visible");
            let row1_rect = Rect::new(
                row1.bounds.x,
                row1.bounds.y,
                row1.bounds.width,
                row1.bounds.height,
            );
            assert!(
                surface
                    .fills
                    .iter()
                    .any(|(r, c)| (r.x - row1_rect.x).abs() < 0.01
                        && (r.y - row1_rect.y).abs() < 0.01
                        && *c == theme.selected_bg),
                "selected row must paint theme.selected_bg at its own bounds"
            );
        }

        /// The focused row's ring strokes exactly at the selected row's
        /// own bounds, in `Theme::focus_ring_color` at
        /// `Style::focus_ring_width` — and only there, not at any other
        /// visible row. Discriminated from `selected_row_paints_selected_bg`
        /// above by checking `surface.strokes` (a distinct call) rather
        /// than `surface.fills`, so this test can't pass on the
        /// pre-existing fill alone.
        #[test]
        fn focused_row_paints_a_ring_at_its_own_bounds() {
            let mut list = vlist(3);
            list.selected_idx = 1;
            list.has_focus = true;
            let layout = list.layout(AREA.width, AREA.height, 0.0, |_| {
                super::super::ListItemMeasure::new(LINE_HEIGHT)
            });
            let style = Style::default().with_focus_ring_width(3.0);
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(
                &list,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                true,
                true,
                &mut surface,
                &theme,
                &style,
                None,
                None,
                /* focus_ring_visible */ true,
            );

            let row1 = layout
                .visible_items
                .iter()
                .find(|v| v.item_idx == 1)
                .expect("row 1 visible");
            let row1_rect = Rect::new(
                row1.bounds.x,
                row1.bounds.y,
                row1.bounds.width,
                row1.bounds.height,
            );
            assert!(
                surface
                    .strokes
                    .iter()
                    .any(|(r, c, w)| (r.x - row1_rect.x).abs() < 0.01
                        && (r.y - row1_rect.y).abs() < 0.01
                        && (r.width - row1_rect.width).abs() < 0.01
                        && (r.height - row1_rect.height).abs() < 0.01
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

        /// `focus_ring_visible: false` paints no ring even though the
        /// row is genuinely selected-and-focused — the default a call
        /// site gets unless it explicitly composes
        /// `ring.is_focused(id) && ring.is_focus_visible()`. Paired with
        /// `no_ring_without_a_selected_row_even_when_focus_ring_visible`
        /// below: either gate alone must suppress the ring.
        #[test]
        fn no_ring_when_focus_ring_visible_is_false() {
            let mut list = vlist(3);
            list.selected_idx = 1;
            list.has_focus = true;
            let layout = list.layout(AREA.width, AREA.height, 0.0, |_| {
                super::super::ListItemMeasure::new(LINE_HEIGHT)
            });
            let mut surface = RecordingSurface::default();
            paint(
                &list,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                true,
                true,
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

        /// `focus_ring_visible: true` alone doesn't paint a ring on a
        /// list with nothing selected-and-focused (`has_focus: false`
        /// here) — the ring is still scoped to
        /// [`ListView::selected_idx`]'s row via `is_selected`.
        #[test]
        fn no_ring_without_a_selected_row_even_when_focus_ring_visible() {
            let mut list = vlist(3);
            list.selected_idx = 1;
            list.has_focus = false;
            let layout = list.layout(AREA.width, AREA.height, 0.0, |_| {
                super::super::ListItemMeasure::new(LINE_HEIGHT)
            });
            let mut surface = RecordingSurface::default();
            paint(
                &list,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                true,
                true,
                &mut surface,
                &Theme::default(),
                &Style::default(),
                None,
                None,
                /* focus_ring_visible */ true,
            );
            assert!(
                surface.strokes.is_empty(),
                "an unfocused list must paint no ring even when \
                 focus_ring_visible: true, got strokes: {:?}",
                surface.strokes
            );
        }

        /// No `▶` chevron glyph is painted for a selected row on GUI —
        /// `theme.selected_bg` (asserted above) is the only selection
        /// cue; the TUI painter keeps the chevron.
        #[test]
        fn selected_row_paints_no_chevron_glyph() {
            let mut list = vlist(3);
            list.selected_idx = 1;
            list.has_focus = true;
            let layout = list.layout(AREA.width, AREA.height, 0.0, |_| {
                super::super::ListItemMeasure::new(LINE_HEIGHT)
            });
            let mut surface = RecordingSurface::default();
            paint(
                &list,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                true,
                true,
                &mut surface,
                &Theme::default(),
                &Style::default(),
                None,
                None,
                false,
            );

            assert!(
                !surface.texts.iter().any(|(_, t, _)| t.contains('▶')),
                "no chevron glyph should be painted on GUI; texts were {:?}",
                surface.texts,
            );
        }

        #[test]
        fn icon_uses_nerd_glyph_or_ascii_fallback() {
            let mut list = vlist(1);
            list.items[0].icon = Some(Icon::new("nf-glyph", "F"));
            let layout = list.layout(AREA.width, AREA.height, 0.0, |_| {
                super::super::ListItemMeasure::new(LINE_HEIGHT)
            });
            let theme = Theme::default();

            let mut nerd_surface = RecordingSurface::default();
            paint(
                &list,
                AREA,
                &layout,
                LINE_HEIGHT,
                true,
                true,
                true,
                &mut nerd_surface,
                &theme,
                &Style::default(),
                None,
                None,
                false,
            );
            assert!(nerd_surface.texts.iter().any(|(_, t, _)| t == "nf-glyph"));

            let mut ascii_surface = RecordingSurface::default();
            paint(
                &list,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                true,
                true,
                &mut ascii_surface,
                &theme,
                &Style::default(),
                None,
                None,
                false,
            );
            assert!(ascii_surface.texts.iter().any(|(_, t, _)| t == "F"));
        }

        #[test]
        fn overlong_row_text_ellipsizes_instead_of_clipping_mid_glyph() {
            // AREA is 100px wide; this label measures 50 * 8.0 = 400px,
            // wildly overflowing the row — it must end in `…`, not stop
            // abruptly on a hard character boundary.
            let mut list = vlist(1);
            list.items[0].text = StyledText::plain("x".repeat(50));
            let layout = list.layout(AREA.width, AREA.height, 0.0, |_| {
                super::super::ListItemMeasure::new(LINE_HEIGHT)
            });
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(
                &list,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                true,
                true,
                &mut surface,
                &theme,
                &Style::default(),
                None,
                None,
                false,
            );

            let row_text = surface
                .texts
                .iter()
                .find(|(_, t, _)| t.starts_with('x'))
                .map(|(_, t, _)| t.clone())
                .expect("row text must be painted");
            assert!(
                row_text.ends_with('…'),
                "overlong row text must end in an ellipsis, got {row_text:?}"
            );
            assert!(
                row_text.len() < "x".repeat(50).len(),
                "overlong row text must be shorter than the source, got {row_text:?}"
            );
        }

        /// The issue's first acceptance bullet: a row's text starts
        /// `row_padding_x` in from its rect. Painted with a non-default
        /// [`Style::row_padding_x`] (15.0, not the 8.0 default) so
        /// reverting the token back to the old hardcoded `2.0` literal
        /// in `paint` would fail this test.
        #[test]
        fn row_text_starts_row_padding_x_in_from_the_row_rect() {
            let list = vlist(1);
            let layout = list.layout(AREA.width, AREA.height, 0.0, |_| {
                super::super::ListItemMeasure::new(LINE_HEIGHT)
            });
            let row = layout
                .visible_items
                .first()
                .expect("one visible row")
                .bounds;

            let style = Style::default().with_row_padding_x(15.0);
            let mut surface = RecordingSurface::default();
            let theme = Theme::default();
            paint(
                &list,
                AREA,
                &layout,
                LINE_HEIGHT,
                false,
                true,
                true,
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
                .find(|(_, t, _)| t.starts_with("row 0"))
                .expect("row text painted");
            assert_eq!(
                text_rect.x,
                row.x + style.row_padding_x,
                "row text must start row_padding_x in from the row's left edge"
            );
        }
    }
}

/// Events a `ListView` emits back to the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ListViewEvent {
    /// Keyboard / mouse moved selection to a different row.
    SelectionChanged { idx: usize },
    /// User pressed Enter while the list had focus and a row was selected
    /// — the keyboard equivalent of [`Self::ItemDoubleClicked`]'s mouse
    /// gesture, and the event [`crate::a11y::A11yAction::Click`] routes to
    /// for a screen reader's own "activate" action. An app wiring up "open
    /// this row" listens for both this and `ItemDoubleClicked`, since a
    /// single click never emits either — only `SelectionChanged`.
    ItemActivated { idx: usize },
    /// User double-clicked a row — distinct from `SelectionChanged` (a
    /// plain click) so an app can tell "select" apart from "open" without
    /// both folding into `ItemActivated`. See `ItemActivated`'s own doc for
    /// the keyboard/screen-reader side of the same "open" meaning.
    ItemDoubleClicked { idx: usize },
    /// A key was pressed while the list had focus and the primitive
    /// did not consume it. App may interpret it (e.g. `q` closes the
    /// quickfix panel).
    KeyPressed { key: String, modifiers: Modifiers },
    /// The full multi-selection changed (`SelectionMode::Multi`): Ctrl/Cmd
    /// click toggled a row, Shift click/extend selected a range, or a
    /// keyboard extension grew the set. `indices` is the complete,
    /// sorted, de-duplicated selection, not just the row that changed —
    /// see [`crate::types::apply_selection_gesture`].
    SelectionSet { indices: Vec<usize> },
}

#[cfg(test)]
mod multi_select_tests {
    use super::ListViewEvent;
    use crate::types::{apply_selection_gesture, Modifiers, SelectionGesture};

    #[test]
    fn ctrl_click_toggle_emits_selection_set_with_full_set() {
        let current = [0usize, 2];
        let gesture = SelectionGesture::from_modifiers(Modifiers {
            ctrl: true,
            ..Default::default()
        });
        let indices = apply_selection_gesture(&current, 2, 4, gesture);
        let event = ListViewEvent::SelectionSet {
            indices: indices.clone(),
        };
        assert_eq!(indices, vec![0, 2, 4]);
        assert_eq!(event, ListViewEvent::SelectionSet { indices });
    }

    #[test]
    fn shift_extend_emits_selection_set_covering_range() {
        let gesture = SelectionGesture::from_modifiers(Modifiers {
            shift: true,
            ..Default::default()
        });
        let indices = apply_selection_gesture(&[], 1, 3, gesture);
        assert_eq!(
            ListViewEvent::SelectionSet { indices },
            ListViewEvent::SelectionSet {
                indices: vec![1, 2, 3]
            }
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_view_roundtrip_serde() {
        let list = ListView {
            id: WidgetId::new("quickfix"),
            title: Some(StyledText::plain("QUICKFIX (3 items)")),
            items: vec![
                ListItem {
                    text: StyledText::plain("src/main.rs:12: unused variable"),
                    icon: None,
                    detail: None,
                    decoration: Decoration::Warning,
                },
                ListItem {
                    text: StyledText::plain("src/lib.rs:4: missing import"),
                    icon: None,
                    detail: Some(StyledText::plain("E0425")),
                    decoration: Decoration::Error,
                },
            ],
            selected_idx: 1,
            scroll_offset: 0,
            has_focus: true,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
        };
        let json = serde_json::to_string(&list).unwrap();
        let back: ListView = serde_json::from_str(&json).unwrap();
        assert_eq!(list, back);
    }

    // ── D6 ListView layout API tests ──────────────────────────────────

    fn make_list_item(text: &str) -> ListItem {
        ListItem {
            text: StyledText::plain(text),
            icon: None,
            detail: None,
            decoration: Decoration::Normal,
        }
    }

    fn make_list(
        title: Option<&str>,
        items: Vec<ListItem>,
        selected: usize,
        scroll: usize,
    ) -> ListView {
        ListView {
            id: WidgetId::new("l"),
            title: title.map(StyledText::plain),
            items,
            selected_idx: selected,
            scroll_offset: scroll,
            has_focus: true,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
        }
    }

    #[test]
    fn list_view_layout_empty() {
        let list = make_list(None, vec![], 0, 0);
        let layout = list.layout(40.0, 10.0, 0.0, |_| ListItemMeasure::new(1.0));
        assert_eq!(layout.visible_items.len(), 0);
        assert!(layout.title_bounds.is_none());
        assert_eq!(layout.hit_test(5.0, 5.0), ListViewHit::Empty);
    }

    #[test]
    fn list_view_layout_title_reserves_first_row() {
        let list = make_list(
            Some("QUICKFIX"),
            (0..3)
                .map(|i| make_list_item(&format!("item{i}")))
                .collect(),
            0,
            0,
        );
        let layout = list.layout(40.0, 10.0, 1.0, |_| ListItemMeasure::new(1.0));
        assert!(layout.title_bounds.is_some());
        let tb = layout.title_bounds.unwrap();
        assert_eq!(tb.y, 0.0);
        assert_eq!(tb.height, 1.0);
        // Items start at y=1 (after title).
        assert_eq!(layout.visible_items[0].bounds.y, 1.0);
        assert_eq!(layout.visible_items[0].item_idx, 0);
        // Click on title → ListViewHit::Title.
        assert_eq!(layout.hit_test(10.0, 0.5), ListViewHit::Title);
        // Click on first item row.
        assert_eq!(layout.hit_test(10.0, 1.5), ListViewHit::Item(0));
    }

    #[test]
    fn list_view_layout_no_title_starts_at_zero() {
        let list = make_list(
            None,
            (0..2).map(|i| make_list_item(&format!("i{i}"))).collect(),
            0,
            0,
        );
        let layout = list.layout(40.0, 10.0, 0.0, |_| ListItemMeasure::new(1.0));
        assert!(layout.title_bounds.is_none());
        assert_eq!(layout.visible_items[0].bounds.y, 0.0);
        assert_eq!(layout.hit_test(10.0, 0.5), ListViewHit::Item(0));
    }

    #[test]
    fn list_view_layout_scroll_offset_skips_items_not_title() {
        let list = make_list(
            Some("HEADER"),
            (0..5).map(|i| make_list_item(&format!("i{i}"))).collect(),
            0,
            2, // skip first 2 items
        );
        let layout = list.layout(40.0, 10.0, 1.0, |_| ListItemMeasure::new(1.0));
        // Title still pinned at top.
        assert_eq!(layout.title_bounds.unwrap().y, 0.0);
        // First visible item is items[2].
        assert_eq!(layout.visible_items[0].item_idx, 2);
        assert_eq!(layout.visible_items[0].bounds.y, 1.0);
    }

    #[test]
    fn list_view_layout_viewport_overflow_clips_last() {
        let list = make_list(
            None,
            (0..10).map(|i| make_list_item(&format!("i{i}"))).collect(),
            0,
            0,
        );
        // 10 items × 2.0; viewport 5.0 → 3 rows fit (last clipped to 1.0).
        let layout = list.layout(40.0, 5.0, 0.0, |_| ListItemMeasure::new(2.0));
        assert_eq!(layout.visible_items.len(), 3);
        assert_eq!(layout.visible_items[2].bounds.height, 1.0);
    }

    #[test]
    fn list_view_layout_pixel_units_with_title() {
        // GTK-style: title row 20 px, items 18.5 px each.
        let list = make_list(
            Some("DIAGNOSTICS"),
            (0..5).map(|i| make_list_item(&format!("d{i}"))).collect(),
            0,
            0,
        );
        let layout = list.layout(300.0, 100.0, 20.0, |_| ListItemMeasure::new(18.5));
        let tb = layout.title_bounds.unwrap();
        assert_eq!(tb.height, 20.0);
        // First item starts at y=20.
        assert_eq!(layout.visible_items[0].bounds.y, 20.0);
        assert_eq!(layout.visible_items[0].bounds.height, 18.5);
        // Hit-test lands on correct row with fractional coords.
        assert_eq!(layout.hit_test(100.0, 29.0), ListViewHit::Item(0));
        assert_eq!(layout.hit_test(100.0, 39.0), ListViewHit::Item(1));
    }

    #[test]
    fn list_view_layout_bordered_insets_items() {
        // Bordered: items inset by 1 cell on each side, viewport
        // height reduced by 2 (top + bottom border rows). Title (when
        // present) overlays the top border, so item area starts at y=1.
        let mut list = make_list(
            Some("Open Tabs"),
            (0..3).map(|i| make_list_item(&format!("tab{i}"))).collect(),
            0,
            0,
        );
        list.bordered = true;
        let layout = list.layout(20.0, 6.0, 1.0, |_| ListItemMeasure::new(1.0));
        // Title overlay covers the full top border row (y=0).
        let tb = layout.title_bounds.unwrap();
        assert_eq!(tb.y, 0.0);
        assert_eq!(tb.width, 20.0);
        // Items inset by 1 cell horizontally, start at y=1.
        let i0 = layout.visible_items[0].bounds;
        assert_eq!(i0.x, 1.0);
        assert_eq!(i0.y, 1.0);
        assert_eq!(i0.width, 18.0);
        // Bottom row (y=5) is reserved for the border — only 3 item
        // rows fit between y=1 and y=5 (inclusive of y=4).
        assert!(layout.visible_items.iter().all(|v| v.bounds.y < 5.0));
    }

    #[test]
    fn list_view_layout_bordered_no_title_starts_at_one() {
        let mut list = make_list(
            None,
            (0..3).map(|i| make_list_item(&format!("r{i}"))).collect(),
            0,
            0,
        );
        list.bordered = true;
        let layout = list.layout(10.0, 6.0, 0.0, |_| ListItemMeasure::new(1.0));
        assert!(layout.title_bounds.is_none());
        // Without title, items still start at y=1 (top border).
        assert_eq!(layout.visible_items[0].bounds.y, 1.0);
    }
}
