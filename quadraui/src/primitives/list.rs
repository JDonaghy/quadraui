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
//!
//! On a feature set that compiles `impl Interactive for ListView` (see
//! that section below), a `ListView` reached through
//! [`crate::frame::Surface::Widget`] drives all of the above itself —
//! keyboard nav, click/double-click hit-testing, and multi-select — via
//! [`crate::interactive::Interactive::handle`], reporting the same
//! `ListViewEvent`s described above plus `SelectionSet` for a
//! multi-selection gesture. [`ListViewState`] is the one piece of
//! per-widget state that round-trip needs that the declarative
//! descriptor has no field for.

use crate::event::Rect;
use crate::primitives::row_stack::{self, RowPaintState};
use crate::primitives::scrollbar::Scrollbar;
use crate::types::{Decoration, Icon, Modifiers, StyledText, WidgetId};
use serde::{Deserialize, Serialize};

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
use crate::a11y::A11yAction;
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
use crate::event::{Key, MouseButton, NamedKey};
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
use crate::interactive::{
    EventCx, FocusSpec, Interactive, KeyEvent, PointerKind, RoutedEvent, WidgetAction,
};
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
use crate::types::{apply_selection_gesture, SelectionGesture};
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
use crate::Handled;

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

#[cfg(test)]
mod paint_state_tests {
    use super::ListPaintState;
    use crate::primitive::PaintState;

    /// Every [`ListPaintState`] field is reachable through the
    /// [`PaintState`] trait, with a non-default value on each so a
    /// method that silently read a different field (or a stale
    /// default) would fail this.
    #[test]
    fn list_paint_state_methods_delegate_to_the_matching_field() {
        let state = ListPaintState::default()
            .with_hovered_idx(Some(1))
            .with_pressed_idx(Some(2))
            .with_scroll_px(Some(3.0))
            .with_striped(true);

        assert_eq!(PaintState::hovered_idx(&state), Some(1));
        assert_eq!(PaintState::pressed_idx(&state), Some(2));
        assert_eq!(PaintState::scroll_px(&state), Some(3.0));
        assert!(PaintState::striped(&state));
    }

    /// The default `ListPaintState` reports "no such state" through
    /// every `PaintState` method, matching [`PaintState`]'s own
    /// trait-default behaviour for a type that tracks nothing.
    #[test]
    fn default_list_paint_state_reports_no_state_through_paint_state() {
        let state = ListPaintState::default();

        assert_eq!(PaintState::hovered_idx(&state), None);
        assert_eq!(PaintState::pressed_idx(&state), None);
        assert_eq!(PaintState::scroll_px(&state), None);
        assert!(!PaintState::striped(&state));
        assert!(!PaintState::focus_ring_visible(&state));
    }
}

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
    ///   caller passes `tree.is_focused(&list.id, None) &&
    ///   tree.is_focus_visible()` here, not `list.has_focus`. `false`
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
        paint_inner(
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
            scroll_px,
            false,
        )
    }

    /// [`paint_with_scroll_px`], plus `striped` — tint every other
    /// non-selected, non-header row with [`Theme::row_alt_bg`] ("zebra
    /// striping"). `striped: false` (what [`paint_with_scroll_px`]
    /// passes) paints pixel-identically to it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint_with_state(
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
        striped: bool,
    ) {
        paint_inner(
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
            scroll_px,
            striped,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_inner(
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
        striped: bool,
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
            // Zebra stripe on the absolute item index (not the visible
            // on-screen position), so a given row's shading survives
            // scrolling, the same convention Finder/Explorer use — a
            // header row is never striped, and selection takes priority
            // over the stripe.
            let is_striped_row = striped
                && !is_selected
                && !matches!(item.decoration, Decoration::Header)
                && vis_item.item_idx % 2 == 1;
            let row_bg = if is_selected {
                theme.selected_bg
            } else if matches!(item.decoration, Decoration::Header) {
                theme.header_bg
            } else if is_striped_row {
                theme.row_alt_bg(base_bg)
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

        /// A headless, end-to-end proof that `focus_ring_visible` reflects
        /// [`crate::focus::FocusTree`]'s real Tab-vs-click `:focus-visible`
        /// convention — composed as `tree.is_focused(id, None) &&
        /// tree.is_focus_visible()`, the bool a host passes here — and
        /// that the painted stroke actually tracks it, not just a literal
        /// `true`/`false` this test assigns itself.
        #[test]
        fn focused_list_row_paints_a_ring_after_tab_but_not_after_click() {
            use crate::focus::FocusTree;

            let id = WidgetId::new("sidebar:list");
            let mut tree = FocusTree::new();
            tree.sync_tab_order(&[(id, Rect::new(0.0, 0.0, 0.0, 0.0))]);
            let theme = Theme::default();
            let style = Style::default();
            let list = ListView { id, ..vlist(1) };

            let paint_and_count_strokes = |focus_ring_visible: bool| -> usize {
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
                    &theme,
                    &style,
                    None,
                    None,
                    focus_ring_visible,
                );
                surface.strokes.len()
            };

            // Tab: `focus_next()` is the real keyboard-move path.
            tree.focus_next();
            let focus_ring_visible = tree.is_focused(&id, None) && tree.is_focus_visible();
            assert!(
                focus_ring_visible,
                "a Tab-reached list must report focus-visible"
            );
            assert_eq!(
                paint_and_count_strokes(focus_ring_visible),
                1,
                "a Tab-reached list's selected row must paint a ring"
            );

            // Click: `set_focus` re-focuses the same widget via the pointer path.
            tree.set_focus(Some((id, None)));
            let focus_ring_visible = tree.is_focused(&id, None) && tree.is_focus_visible();
            assert!(
                tree.is_focused(&id, None),
                "the click did move focus to this list"
            );
            assert!(
                !focus_ring_visible,
                "a mouse click must suppress the list's focus-visible ring"
            );
            assert_eq!(
                paint_and_count_strokes(focus_ring_visible),
                0,
                "a clicked list's selected row must paint no ring"
            );

            // Shift+Tab afterwards restores the ring.
            tree.focus_prev();
            let focus_ring_visible = tree.is_focused(&id, None) && tree.is_focus_visible();
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

// ── `crate::primitive::Primitive` pilot ─────────────────────────────
//
// `paint`'s pixel-backend branch below needs `native_surface_paint`;
// `paint_cells` needs `crate::tui::draw_list`/`ratatui` — gated to
// whichever of those a build actually has, same as those modules
// themselves.

/// [`crate::primitive::Primitive::Layout`] for [`ListView`] — the
/// list-local [`ListViewLayout`] paired with the absolute area it was
/// laid out into and the resolved row-paint metrics
/// [`crate::primitive::Primitive::paint`]'s fixed signature has no
/// other way to carry (see that trait's module doc, "Design").
///
/// `#[non_exhaustive]`: backend-returned, never constructed by a
/// consumer via struct literal, same convention as [`ListViewLayout`]
/// itself.
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ListViewPrimitiveLayout {
    /// Absolute area [`ListView::layout`] was called with — `layout`'s
    /// own `title_bounds`/`visible_items`/`hit_regions` are relative to
    /// this rect's origin.
    pub area: Rect,
    /// Row height resolved from the [`crate::primitive::LayoutCtx`]
    /// [`Primitive::layout`](crate::primitive::Primitive::layout) was
    /// called with.
    pub line_height: f32,
    /// Nerd-Font toggle resolved the same way.
    pub nerd_fonts_enabled: bool,
    /// The list-local layout itself.
    pub layout: ListViewLayout,
}

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
impl ListViewPrimitiveLayout {
    /// Hit-test at an absolute `(x, y)` — subtracts [`Self::area`]'s
    /// origin before delegating to [`ListViewLayout::hit_test`], whose
    /// `hit_regions` are list-local.
    pub fn hit_test(&self, x: f32, y: f32) -> ListViewHit {
        self.layout.hit_test(x - self.area.x, y - self.area.y)
    }
}

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
impl crate::primitive::Primitive for ListView {
    type Layout = ListViewPrimitiveLayout;
    type Event = ListViewEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// Round-trips through [`crate::frame::OwnedContent::ListWidget`] —
    /// distinct from [`crate::frame::OwnedContent::List`] (which
    /// [`crate::frame::capture_surface`] produces for the legacy
    /// [`crate::frame::Surface::List`] instead) so a `ListView` reached
    /// through [`crate::frame::Surface::Widget`] replays through
    /// [`crate::backend::DrawsPrimitives::draw_primitive`] and
    /// hit-tests as [`crate::frame::FrameZone::Widget`] after capture,
    /// exactly as it does live — see that variant's own doc.
    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::ListWidget(self.clone())
    }

    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> Self::Layout {
        // `ctx.units()` is a reliable *runtime* discriminator between a
        // TUI caller and a pixel-backend one even when both features are
        // compiled into the same binary (`cargo build --features
        // gtk,tui`), where a compile-time `cfg` alone could not tell
        // them apart.
        #[cfg(feature = "tui")]
        {
            if matches!(ctx.units(), crate::primitive::Units::Cells) {
                // Route through the exact resolver `crate::tui::draw_list`
                // itself calls (`tui_list_layout`) so this layout can
                // never drift from what TUI actually paints —
                // `Self::paint_cells` below makes the matching
                // paint-side guarantee.
                let area = crate::tui::backend::q_rect_to_ratatui(rect);
                let layout = crate::tui::tui_list_layout(area, self);
                return ListViewPrimitiveLayout {
                    area: rect,
                    line_height: ctx.line_height,
                    nerd_fonts_enabled: ctx.nerd_fonts_enabled,
                    layout,
                };
            }
        }
        // `Self::paint`'s shared `native_surface_paint::paint_with_state`
        // call strokes no frame of its own (see that module's doc, "Note:
        // `bordered`'s frame stroke remains backend-specific") — each
        // backend's thin wrapper strokes its own frame around this call
        // instead. `border_inset` therefore stays `0.0` here regardless
        // of `self.bordered`, matching `supports_border: false` below.
        let border_inset: f64 = 0.0;
        let layout = crate::primitives::layout_metrics::list_layout(
            self,
            rect.width as f64,
            rect.height as f64,
            ctx.line_height as f64,
            ctx.char_width as f64,
            border_inset,
        );
        ListViewPrimitiveLayout {
            area: rect,
            line_height: ctx.line_height,
            nerd_fonts_enabled: ctx.nerd_fonts_enabled,
            layout,
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
            use crate::primitive::PaintState;

            // No per-call interaction-state channel reaches this far —
            // `Primitive::paint`'s fixed signature carries none (see
            // that trait's own doc) — so this reads the "no such state"
            // defaults every `PaintState` method already falls back to,
            // naming the gap through the shared trait rather than a
            // bare `None`/`None`/`false`/`false` literal tuple.
            //
            // `supports_border: false` below: this shared paint strokes
            // no frame of its own (see `native_surface_paint`'s module
            // doc, "Note: `bordered`'s frame stroke remains
            // backend-specific") — only a backend wrapper that strokes
            // its own frame around this call should pass `true`, and
            // none of them do yet through this `Primitive` seam.
            let paint_state = ListPaintState::default();
            native_surface_paint::paint_with_state(
                self,
                layout.area,
                &layout.layout,
                layout.line_height,
                layout.nerd_fonts_enabled,
                /* supports_border */ false,
                /* supports_hscrollbar */ true,
                surface,
                theme,
                style,
                paint_state.hovered_idx(),
                paint_state.pressed_idx(),
                paint_state.focus_ring_visible(),
                paint_state.scroll_px(),
                paint_state.striped(),
            );
        }
        // A `tui`-only build has no `PaintSurface` to paint into at all
        // (`PaintSurface`'s module doc, "Why TUI stays out") — `self`
        // reaching the screen on TUI goes through `Self::paint_cells`
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

    /// Delegates straight to [`crate::tui::draw_list`] — the exact
    /// function [`crate::tui::backend::TuiBackend::draw_list`] itself
    /// calls — so painting `ListView` through [`crate::primitive::Primitive`]
    /// on TUI is byte-for-byte identical to painting it through
    /// `Backend::draw_list` by construction, not by separately
    /// maintained parity. See [`crate::primitive::Primitive::paint_cells`]'s
    /// own doc for why this method exists instead of a cell-quantised
    /// `PaintSurface` impl.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &Self::Layout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        // CSR: ratatui_buffer escape hatch
        let buf = cells
            .ratatui_buffer()
            .expect(crate::primitive::CELL_BUFFER_REQUIRED);
        let area = crate::tui::backend::q_rect_to_ratatui(layout.area);
        // `ListPaintState::default()` is the "no such state" value
        // [`crate::primitive::PaintState`]'s own defaults describe —
        // routed through [`crate::tui::draw_list_with_state`] (which
        // takes a real [`ListPaintState`]), so this path is visibly
        // state-shaped even though `Primitive::paint_cells`'s fixed
        // signature has no live interaction state to put in it.
        let paint_state = ListPaintState::default();
        crate::tui::draw_list_with_state(
            buf,
            area,
            self,
            theme,
            layout.nerd_fonts_enabled,
            &paint_state,
        );
    }
}

// ── Interactive: keyboard nav + mouse click ─────────────────────────
//
// `ListView` stays purely declarative (see this module's own doc): every
// navigation/selection method below computes the *new* `selected_idx`
// from `self`'s current one and reports it via `ListViewEvent` rather
// than mutating anything in place — the app applies the event onto its
// own `ListView` before the next frame, the same contract
// `ListViewEvent`'s own doc already promises.
//
// `ListViewState` holds only the one piece of interaction state
// `ListView`'s descriptor has no field for: the multi-selection set
// beyond `selected_idx`, plus the anchor a Shift range-extension grows
// from — see [`SelectionGesture`]'s own doc for why a row-oriented
// widget needs a host-tracked anchor instead of deriving one per event.

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
impl WidgetAction for ListViewEvent {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        crate::UiEvent::List(id, self)
    }
}

/// [`Interactive::State`] for [`ListView`] — see this section's module
/// doc for why it carries only the multi-selection set, its anchor and
/// its keyboard cursor.
///
/// `#[non_exhaustive]`: a future field add here (e.g. an in-progress
/// drag anchor) is additive, same convention as [`ListViewLayout`].
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListViewState {
    /// The full multi-selection set, sorted/deduped by
    /// [`apply_selection_gesture`]. Empty until a Ctrl/Cmd or Shift
    /// gesture first grows it beyond the single `selected_idx` every
    /// plain click/arrow press collapses it back to.
    pub selected: Vec<usize>,
    /// The index a Shift range-extension grows from — the last
    /// `Replace`/`Toggle` target, per [`SelectionGesture::Range`]'s own
    /// doc. `None` until the first extending gesture, at which point it
    /// seeds from `cursor` (or `ListView::selected_idx` if even that is
    /// unset yet) rather than defaulting to row `0`.
    pub anchor: Option<usize>,
    /// The row the last keyboard/mouse move actually landed on,
    /// independent of whatever `ListView::selected_idx` the app echoes
    /// back next frame (which, for a `SelectionSet`, may only carry
    /// `indices.first()`). `None` until the first move, at which point
    /// it falls back to `ListView::selected_idx`.
    pub cursor: Option<usize>,
    /// The `selected_idx` [`ListView::current_idx`] expects the app to
    /// echo back after the move that produced [`Self::cursor`] — for a
    /// `SelectionSet`, that is `indices.first()`, since that is all a
    /// plain `selected_idx` field can carry. As long as the next frame's
    /// `ListView::selected_idx` still matches this, `cursor` reflects a
    /// move the app accepted and stays authoritative; a mismatch means
    /// the app set `selected_idx` itself (a programmatic jump), so the
    /// descriptor wins per the controlled-mode contract and `cursor`
    /// resyncs to it. `None` until the first move.
    pub cursor_echo: Option<usize>,
}

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
impl ListView {
    /// The row the next navigation/extension computes relative to:
    /// `state.cursor` when `self.selected_idx` still matches
    /// [`ListViewState::cursor_echo`] (the app accepted the widget's
    /// last move), falling back to `self.selected_idx` (clamped into
    /// `self.items`) otherwise — either because no move has happened
    /// yet, or because the app set `selected_idx` itself since, which
    /// the controlled-mode contract ("if the descriptor's selection
    /// field is set, it wins") requires this to trust over a stale
    /// `cursor`.
    fn current_idx(&self, state: &ListViewState) -> usize {
        let last = self.items.len().saturating_sub(1);
        match state.cursor {
            Some(cursor) if state.cursor_echo == Some(self.selected_idx) => cursor.min(last),
            _ => self.selected_idx.min(last),
        }
    }

    /// The multi-selection set [`Self::handle_mouse_down`]'s `Toggle`/`Range`
    /// gestures grow from: `state.selected` once any gesture has grown it
    /// beyond a single row, falling back to `[self.selected_idx]` (or
    /// empty, for an empty list) before the first one — the same
    /// lazy-seed convention [`Self::current_idx`] applies to the cursor,
    /// so a first Ctrl/Cmd-click never discards the descriptor's own
    /// `selected_idx` the way an unseeded empty `Vec` would.
    fn current_selected(&self, state: &ListViewState) -> Vec<usize> {
        if !state.selected.is_empty() {
            state.selected.clone()
        } else if self.items.is_empty() {
            Vec::new()
        } else {
            vec![self.selected_idx.min(self.items.len() - 1)]
        }
    }

    /// Build the `Handled::Event` that moves the selection to `target`
    /// (already clamped into `self.items`), honouring `extend` (Shift
    /// held) the same way a click's own [`SelectionGesture`] does: a
    /// plain move collapses to a single-row `SelectionChanged` and
    /// re-anchors there; an extending move grows `state.selected` via
    /// [`apply_selection_gesture`]'s `Range` gesture from `state.anchor`
    /// (seeded from [`Self::current_idx`] the first time an extension
    /// starts, not reset on every subsequent one) and reports the full
    /// set via `SelectionSet`. Either way, `state.cursor` is updated to
    /// `target` so the *next* call keeps extending from where this one
    /// left off, even though the app's own echoed `selected_idx` may not
    /// track that.
    fn navigate(
        &self,
        state: &mut ListViewState,
        target: usize,
        extend: bool,
    ) -> Handled<ListViewEvent> {
        if self.items.is_empty() {
            return Handled::Ignored;
        }
        let current = self.current_idx(state);
        state.cursor = Some(target);
        if extend {
            let anchor = *state.anchor.get_or_insert(current);
            let indices =
                apply_selection_gesture(&state.selected, anchor, target, SelectionGesture::Range);
            state.selected = indices.clone();
            state.cursor_echo = indices.first().copied();
            Handled::Event(ListViewEvent::SelectionSet { indices })
        } else {
            state.anchor = Some(target);
            state.selected = vec![target];
            state.cursor_echo = Some(target);
            Handled::Event(ListViewEvent::SelectionChanged { idx: target })
        }
    }

    /// Clamp `delta` applied to [`Self::current_idx`] into `[0,
    /// items.len())`, saturating at either edge rather than wrapping —
    /// the keyboard-navigation counterpart of
    /// [`ListViewLayout::hit_test`]'s mouse-side row resolution.
    fn clamped_target(&self, state: &ListViewState, delta: isize) -> usize {
        let last = self.items.len().saturating_sub(1);
        self.current_idx(state)
            .saturating_add_signed(delta)
            .min(last)
    }

    /// Dispatch one idle (no active gesture) keystroke: `j`/Down and
    /// `k`/Up move the selection by one row, Home/End jump to either
    /// edge, Page Up/Down move by one viewport page (`layout.visible_items`'s
    /// length, floored at one row), Enter activates the current
    /// selection, and every other key is reported to the app via
    /// [`EventCx::emit`] as the raw [`ListViewEvent::KeyPressed`] an app
    /// may bind its own hotkeys to, then declared [`Handled::Ignored`]
    /// rather than claimed — see that variant's own doc, and
    /// [`Interactive::handle`]'s own doc for why an unclaimed key must
    /// still bubble (Tab/Shift+Tab focus-cycling, Escape, and anything
    /// else a later dispatch step owns).
    fn handle_key(
        &self,
        state: &mut ListViewState,
        key: &Key,
        modifiers: &Modifiers,
        layout: &ListViewPrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<ListViewEvent> {
        let extend = modifiers.shift;
        let page = layout.layout.visible_items.len().max(1) as isize;
        let no_accel = !(modifiers.ctrl || modifiers.alt || modifiers.cmd);
        match key {
            Key::Named(NamedKey::Down) => {
                let target = self.clamped_target(state, 1);
                self.navigate(state, target, extend)
            }
            Key::Char('j') if no_accel => {
                let target = self.clamped_target(state, 1);
                self.navigate(state, target, extend)
            }
            Key::Named(NamedKey::Up) => {
                let target = self.clamped_target(state, -1);
                self.navigate(state, target, extend)
            }
            Key::Char('k') if no_accel => {
                let target = self.clamped_target(state, -1);
                self.navigate(state, target, extend)
            }
            Key::Named(NamedKey::Home) => self.navigate(state, 0, extend),
            Key::Named(NamedKey::End) => {
                let target = self.items.len().saturating_sub(1);
                self.navigate(state, target, extend)
            }
            Key::Named(NamedKey::PageDown) => {
                let target = self.clamped_target(state, page);
                self.navigate(state, target, extend)
            }
            Key::Named(NamedKey::PageUp) => {
                let target = self.clamped_target(state, -page);
                self.navigate(state, target, extend)
            }
            Key::Named(NamedKey::Enter) | Key::Char(' ') => {
                if self.items.is_empty() {
                    Handled::Ignored
                } else {
                    Handled::Event(ListViewEvent::ItemActivated {
                        idx: self.current_idx(state),
                    })
                }
            }
            Key::Char('a') if modifiers.ctrl || modifiers.cmd => {
                if self.items.is_empty() {
                    Handled::Ignored
                } else {
                    let indices: Vec<usize> = (0..self.items.len()).collect();
                    let last = self.items.len() - 1;
                    state.selected = indices.clone();
                    state.anchor = Some(last);
                    state.cursor = Some(last);
                    Handled::Event(ListViewEvent::SelectionSet { indices })
                }
            }
            _ => {
                cx.emit(
                    ListViewEvent::KeyPressed {
                        key: key_to_event_string(key),
                        modifiers: *modifiers,
                    }
                    .into_ui_event(cx.id),
                );
                Handled::Ignored
            }
        }
    }

    /// `PointerKind::Down(MouseButton::Left)` — hit-tests `local` and
    /// selects the row it landed on, honouring `mods`' own
    /// [`SelectionGesture`] the same way a keyboard range-extension does
    /// (see [`Self::navigate`]). A hit on the title row requests focus
    /// (so clicking a bordered list's header still focuses it) but is
    /// otherwise non-actionable, per [`ListViewHit`]'s own doc; a hit on
    /// the empty tail below the last item is ignored outright.
    fn handle_mouse_down(
        &self,
        state: &mut ListViewState,
        hit: ListViewHit,
        mods: &Modifiers,
        cx: &mut EventCx,
    ) -> Handled<ListViewEvent> {
        let idx = match hit {
            ListViewHit::Item(idx) => idx,
            ListViewHit::Title => {
                cx.request_focus(None);
                return Handled::Ignored;
            }
            ListViewHit::Empty => return Handled::Ignored,
        };
        cx.request_focus(None);
        let current = self.current_idx(state);
        match SelectionGesture::from_modifiers(*mods) {
            SelectionGesture::Replace => {
                state.anchor = Some(idx);
                state.cursor = Some(idx);
                state.selected = vec![idx];
                state.cursor_echo = Some(idx);
                Handled::Event(ListViewEvent::SelectionChanged { idx })
            }
            gesture @ SelectionGesture::Toggle => {
                let anchor = state.anchor.unwrap_or(current);
                let selected = self.current_selected(state);
                let indices = apply_selection_gesture(&selected, anchor, idx, gesture);
                state.selected = indices.clone();
                state.anchor = Some(idx);
                state.cursor = Some(idx);
                state.cursor_echo = indices.first().copied();
                Handled::Event(ListViewEvent::SelectionSet { indices })
            }
            gesture @ SelectionGesture::Range => {
                let anchor = *state.anchor.get_or_insert(current);
                let selected = self.current_selected(state);
                let indices = apply_selection_gesture(&selected, anchor, idx, gesture);
                state.selected = indices.clone();
                state.cursor = Some(idx);
                state.cursor_echo = indices.first().copied();
                Handled::Event(ListViewEvent::SelectionSet { indices })
            }
        }
    }
}

/// Canonical key string for [`ListViewEvent::KeyPressed`] — the same
/// spelling [`NamedKey::name`] already promises as the single source of
/// truth, so this fallback agrees with every other backend's own
/// `KeyPressed` event instead of carrying a `Debug`-derived string no
/// other producer uses.
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
fn key_to_event_string(key: &Key) -> String {
    match key {
        Key::Char(c) => c.to_string(),
        Key::Named(named) => named.name().into_owned(),
    }
}

#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
impl Interactive for ListView {
    type State = ListViewState;
    type Action = ListViewEvent;

    /// One focus stop as a whole — a flat list navigates internally via
    /// arrow/`j`/`k` keys inside [`Self::handle`]'s own dispatch rather
    /// than declaring a [`FocusSpec::Children`] stop per row (which,
    /// per that variant's own doc, collapses to a single Tab stop
    /// anyway once a host walks it). `FocusSpec::None` for an empty
    /// list — there is no row for a selection to land on.
    fn focus(&self) -> FocusSpec {
        if self.items.is_empty() {
            FocusSpec::None
        } else {
            FocusSpec::Stop
        }
    }

    fn handle(
        &self,
        state: &mut ListViewState,
        event: &RoutedEvent,
        layout: &Self::Layout,
        cx: &mut EventCx,
    ) -> Handled<ListViewEvent> {
        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                local,
                mods,
                ..
            } => {
                let hit = layout.layout.hit_test(local.x, local.y);
                self.handle_mouse_down(state, hit, mods, cx)
            }
            RoutedEvent::Pointer {
                kind: PointerKind::DoubleClick(MouseButton::Left),
                local,
                ..
            } => match layout.layout.hit_test(local.x, local.y) {
                ListViewHit::Item(idx) => Handled::Event(ListViewEvent::ItemDoubleClicked { idx }),
                _ => Handled::Ignored,
            },
            RoutedEvent::Key(KeyEvent { key, modifiers, .. }) => {
                self.handle_key(state, key, modifiers, layout, cx)
            }
            // `A11yAction` carries no row index — unlike `route_list_action`,
            // which recovers one from the `A11yTree` owner key's
            // `"item.{idx}"` encoding, there is no sub-target in
            // `RoutedEvent::Action` to address a specific row through this
            // generic `Surface::Widget` seam. `Click` is the one action
            // meaningful without one: activate whichever row
            // `Self::current_idx` already tracks, the same row Enter
            // activates — the assistive-technology default-action
            // convention and the keyboard convention land on the same
            // `UiEvent` by construction. `ScrollIntoView` stays unrouted for
            // the same reason `a11y::action::route_action`'s own arm
            // documents: no generic "scroll to bounds" `UiEvent` exists yet.
            RoutedEvent::Action(A11yAction::Click) => {
                if self.items.is_empty() {
                    Handled::Ignored
                } else {
                    Handled::Event(ListViewEvent::ItemActivated {
                        idx: self.current_idx(state),
                    })
                }
            }
            _ => Handled::Ignored,
        }
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
    use crate::event::{Key, NamedKey, Point};
    use crate::interactive::EventHost;
    use crate::primitive::{LayoutCtx, Primitive, Units};
    use crate::PointerShape;

    /// Minimal [`EventHost`] mock: records every call so a test can
    /// assert on what [`Interactive::handle`] asked for, the same
    /// shape `interactive`'s own module tests use.
    #[derive(Debug, Default)]
    struct MockHost {
        focus_requests: Vec<Option<crate::interactive::SubTarget>>,
        emitted: Vec<crate::UiEvent>,
    }

    impl EventHost for MockHost {
        fn is_focused(&self, _id: &WidgetId, _sub: Option<crate::interactive::SubTarget>) -> bool {
            false
        }
        fn request_focus(&mut self, _id: &WidgetId, sub: Option<crate::interactive::SubTarget>) {
            self.focus_requests.push(sub);
        }
        fn capture_pointer(&mut self, _id: &WidgetId) {}
        fn release_pointer(&mut self) {}
        fn set_cursor(&mut self, _cursor: PointerShape) {}
        fn request_redraw(&mut self) {}
        fn emit(&mut self, event: crate::UiEvent) {
            self.emitted.push(event);
        }
        fn units(&self) -> Units {
            Units::Cells
        }
    }

    fn list(items: usize, selected_idx: usize) -> ListView {
        ListView {
            id: WidgetId::new("list:test"),
            title: None,
            items: (0..items)
                .map(|i| ListItem {
                    text: StyledText::plain(format!("item {i}")),
                    icon: None,
                    detail: None,
                    decoration: Decoration::Normal,
                })
                .collect(),
            selected_idx,
            scroll_offset: 0,
            has_focus: true,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
        }
    }

    fn layout_for(list: &ListView) -> ListViewPrimitiveLayout {
        let ctx = LayoutCtx::new(1.0, 1.0, false);
        Primitive::layout(list, &ctx, Rect::new(0.0, 0.0, 20.0, 10.0))
    }

    fn key(named: NamedKey) -> RoutedEvent {
        RoutedEvent::Key(KeyEvent {
            key: Key::Named(named),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn key_with_mods(named: NamedKey, modifiers: Modifiers) -> RoutedEvent {
        RoutedEvent::Key(KeyEvent {
            key: Key::Named(named),
            modifiers,
            repeat: false,
        })
    }

    #[test]
    fn empty_list_has_no_focus_stop() {
        assert_eq!(list(0, 0).focus(), FocusSpec::None);
    }

    #[test]
    fn non_empty_list_is_one_focus_stop() {
        assert_eq!(list(3, 0).focus(), FocusSpec::Stop);
    }

    #[test]
    fn down_arrow_moves_the_selection_by_one() {
        let l = list(3, 0);
        let layout = layout_for(&l);
        let mut state = ListViewState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(l.id, &mut host);

        let outcome = l.handle(&mut state, &key(NamedKey::Down), &layout, &mut cx);
        assert_eq!(
            outcome,
            Handled::Event(ListViewEvent::SelectionChanged { idx: 1 })
        );
    }

    #[test]
    fn up_arrow_at_the_top_row_stays_put() {
        let l = list(3, 0);
        let layout = layout_for(&l);
        let mut state = ListViewState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(l.id, &mut host);

        let outcome = l.handle(&mut state, &key(NamedKey::Up), &layout, &mut cx);
        assert_eq!(
            outcome,
            Handled::Event(ListViewEvent::SelectionChanged { idx: 0 })
        );
    }

    #[test]
    fn end_jumps_to_the_last_row_and_home_back_to_the_first() {
        let l = list(5, 1);
        let layout = layout_for(&l);
        let mut state = ListViewState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(l.id, &mut host);

        let outcome = l.handle(&mut state, &key(NamedKey::End), &layout, &mut cx);
        assert_eq!(
            outcome,
            Handled::Event(ListViewEvent::SelectionChanged { idx: 4 })
        );

        let outcome = l.handle(&mut state, &key(NamedKey::Home), &layout, &mut cx);
        assert_eq!(
            outcome,
            Handled::Event(ListViewEvent::SelectionChanged { idx: 0 })
        );
    }

    #[test]
    fn enter_activates_the_current_selection() {
        let l = list(3, 2);
        let layout = layout_for(&l);
        let mut state = ListViewState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(l.id, &mut host);

        let outcome = l.handle(&mut state, &key(NamedKey::Enter), &layout, &mut cx);
        assert_eq!(
            outcome,
            Handled::Event(ListViewEvent::ItemActivated { idx: 2 })
        );
    }

    #[test]
    fn enter_on_an_empty_list_is_ignored() {
        let l = list(0, 0);
        let layout = layout_for(&l);
        let mut state = ListViewState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(l.id, &mut host);

        let outcome = l.handle(&mut state, &key(NamedKey::Enter), &layout, &mut cx);
        assert_eq!(outcome, Handled::Ignored);
    }

    #[test]
    fn shift_down_extends_the_selection_into_a_selection_set() {
        let l = list(5, 0);
        let layout = layout_for(&l);
        let mut state = ListViewState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(l.id, &mut host);

        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        let outcome = l.handle(
            &mut state,
            &key_with_mods(NamedKey::Down, shift),
            &layout,
            &mut cx,
        );
        assert_eq!(
            outcome,
            Handled::Event(ListViewEvent::SelectionSet {
                indices: vec![0, 1]
            })
        );
    }

    /// Regression: a second consecutive Shift+Down must keep growing the
    /// range from where the first one left off (`state.cursor`), not
    /// re-derive the target from `ListView::selected_idx` — which, after
    /// a `SelectionSet`, an app may only echo back as
    /// `indices.first()` (as this driver app does), collapsing every
    /// extension back to the same two rows forever.
    #[test]
    fn consecutive_shift_downs_keep_growing_the_range() {
        let mut l = list(5, 0);
        let layout = layout_for(&l);
        let mut state = ListViewState::default();
        let mut host = MockHost::default();
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };

        {
            let mut cx = EventCx::new(l.id, &mut host);
            let outcome = l.handle(
                &mut state,
                &key_with_mods(NamedKey::Down, shift),
                &layout,
                &mut cx,
            );
            assert_eq!(
                outcome,
                Handled::Event(ListViewEvent::SelectionSet {
                    indices: vec![0, 1]
                })
            );
        }
        // The app echoes back only `indices.first()`, same as
        // `tests/list_interactive_driver.rs`'s own `SelectionSet` arm.
        l.selected_idx = 0;

        let mut cx = EventCx::new(l.id, &mut host);
        let outcome = l.handle(
            &mut state,
            &key_with_mods(NamedKey::Down, shift),
            &layout,
            &mut cx,
        );
        assert_eq!(
            outcome,
            Handled::Event(ListViewEvent::SelectionSet {
                indices: vec![0, 1, 2]
            })
        );
    }

    /// Regression: the very first Shift+Down on a list whose selection
    /// didn't start at row 0 must anchor at the actual current row, not
    /// row 0 — `ListViewState::anchor`'s own default (`None`) seeds from
    /// `ListView::selected_idx` on first use rather than defaulting to
    /// `0`.
    #[test]
    fn shift_down_from_a_non_zero_selection_anchors_there_not_at_row_zero() {
        let l = list(5, 3);
        let layout = layout_for(&l);
        let mut state = ListViewState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(l.id, &mut host);

        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        let outcome = l.handle(
            &mut state,
            &key_with_mods(NamedKey::Down, shift),
            &layout,
            &mut cx,
        );
        assert_eq!(
            outcome,
            Handled::Event(ListViewEvent::SelectionSet {
                indices: vec![3, 4]
            })
        );
    }

    #[test]
    fn clicking_the_title_requests_focus_but_is_ignored() {
        let mut l = list(3, 0);
        l.bordered = true;
        l.title = Some(StyledText::plain("Picker"));
        let layout = layout_for(&l);
        let mut state = ListViewState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(l.id, &mut host);

        let click = RoutedEvent::Pointer {
            kind: PointerKind::Down(MouseButton::Left),
            sub: None,
            local: Point::new(0.0, 0.0),
            mods: Modifiers::default(),
        };
        let outcome = l.handle(&mut state, &click, &layout, &mut cx);
        assert_eq!(outcome, Handled::Ignored);
        assert_eq!(host.focus_requests, vec![None]);
    }

    #[test]
    fn ctrl_a_selects_every_row() {
        let l = list(4, 0);
        let layout = layout_for(&l);
        let mut state = ListViewState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(l.id, &mut host);

        let ctrl_a = RoutedEvent::Key(KeyEvent {
            key: Key::Char('a'),
            modifiers: Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
            repeat: false,
        });
        let outcome = l.handle(&mut state, &ctrl_a, &layout, &mut cx);
        assert_eq!(
            outcome,
            Handled::Event(ListViewEvent::SelectionSet {
                indices: vec![0, 1, 2, 3]
            })
        );
    }

    #[test]
    fn an_unbound_key_falls_through_to_the_raw_key_pressed_event_but_bubbles() {
        let l = list(3, 0);
        let layout = layout_for(&l);
        let mut state = ListViewState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(l.id, &mut host);

        let q = RoutedEvent::Key(KeyEvent {
            key: Key::Char('q'),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        let outcome = l.handle(&mut state, &q, &layout, &mut cx);
        // `Handled::Ignored`, not `Handled::Event` — an unbound key must
        // still bubble past this widget (Tab/Shift+Tab focus-cycling,
        // Escape, …), even though the app still sees the raw key via the
        // emitted `UiEvent`.
        assert_eq!(outcome, Handled::Ignored);
        assert_eq!(
            host.emitted,
            vec![crate::UiEvent::List(
                l.id,
                ListViewEvent::KeyPressed {
                    key: "q".to_string(),
                    modifiers: Modifiers::default(),
                }
            )]
        );
    }

    #[test]
    fn an_unbound_named_key_uses_named_keys_canonical_spelling() {
        let l = list(3, 0);
        let layout = layout_for(&l);
        let mut state = ListViewState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(l.id, &mut host);

        let tab = RoutedEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Tab),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        let outcome = l.handle(&mut state, &tab, &layout, &mut cx);
        assert_eq!(outcome, Handled::Ignored);
        assert_eq!(
            host.emitted,
            vec![crate::UiEvent::List(
                l.id,
                ListViewEvent::KeyPressed {
                    key: "Tab".to_string(),
                    modifiers: Modifiers::default(),
                }
            )]
        );
    }

    #[test]
    fn clicking_a_row_selects_it_and_requests_focus() {
        let l = list(3, 0);
        let layout = layout_for(&l);
        let mut state = ListViewState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(l.id, &mut host);

        // Row 1's local point: title is absent, so row 1 starts one row
        // height (`1.0`) down from the top.
        let click = RoutedEvent::Pointer {
            kind: PointerKind::Down(MouseButton::Left),
            sub: None,
            local: Point::new(0.0, 1.0),
            mods: Modifiers::default(),
        };
        let outcome = l.handle(&mut state, &click, &layout, &mut cx);
        assert_eq!(
            outcome,
            Handled::Event(ListViewEvent::SelectionChanged { idx: 1 })
        );
        assert_eq!(host.focus_requests, vec![None]);
    }

    #[test]
    fn double_clicking_a_row_activates_it() {
        let l = list(3, 0);
        let layout = layout_for(&l);
        let mut state = ListViewState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(l.id, &mut host);

        let double_click = RoutedEvent::Pointer {
            kind: PointerKind::DoubleClick(MouseButton::Left),
            sub: None,
            local: Point::new(0.0, 1.0),
            mods: Modifiers::default(),
        };
        let outcome = l.handle(&mut state, &double_click, &layout, &mut cx);
        assert_eq!(
            outcome,
            Handled::Event(ListViewEvent::ItemDoubleClicked { idx: 1 })
        );
    }

    #[test]
    fn clicking_below_the_last_row_is_ignored() {
        let l = list(2, 0);
        let layout = layout_for(&l);
        let mut state = ListViewState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(l.id, &mut host);

        let click = RoutedEvent::Pointer {
            kind: PointerKind::Down(MouseButton::Left),
            sub: None,
            local: Point::new(0.0, 9.0),
            mods: Modifiers::default(),
        };
        let outcome = l.handle(&mut state, &click, &layout, &mut cx);
        assert_eq!(outcome, Handled::Ignored);
    }

    #[test]
    fn ctrl_click_toggles_a_row_into_the_selection() {
        let l = list(3, 0);
        let layout = layout_for(&l);
        let mut state = ListViewState {
            selected: vec![0],
            anchor: Some(0),
            cursor: Some(0),
            cursor_echo: Some(0),
        };
        let mut host = MockHost::default();
        let mut cx = EventCx::new(l.id, &mut host);

        let ctrl_click = RoutedEvent::Pointer {
            kind: PointerKind::Down(MouseButton::Left),
            sub: None,
            local: Point::new(0.0, 2.0),
            mods: Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
        };
        let outcome = l.handle(&mut state, &ctrl_click, &layout, &mut cx);
        assert_eq!(
            outcome,
            Handled::Event(ListViewEvent::SelectionSet {
                indices: vec![0, 2]
            })
        );
        assert_eq!(state.selected, vec![0, 2]);
    }

    /// Regression for the bug the second review round flagged: a list
    /// with `selected_idx: 1` whose very *first* interaction is a
    /// Ctrl-click on row 2 must grow the selection to `[1, 2]`, not
    /// replace it with `[2]` — `ListViewState::default()`'s empty
    /// `selected` must not be read as "nothing was ever selected" when
    /// `ListView::selected_idx` says otherwise.
    #[test]
    fn first_ctrl_click_keeps_the_descriptors_own_selection() {
        let l = list(3, 1);
        let layout = layout_for(&l);
        let mut state = ListViewState::default();
        let mut host = MockHost::default();
        let mut cx = EventCx::new(l.id, &mut host);

        let ctrl_click = RoutedEvent::Pointer {
            kind: PointerKind::Down(MouseButton::Left),
            sub: None,
            local: Point::new(0.0, 2.0),
            mods: Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
        };
        let outcome = l.handle(&mut state, &ctrl_click, &layout, &mut cx);
        assert_eq!(
            outcome,
            Handled::Event(ListViewEvent::SelectionSet {
                indices: vec![1, 2]
            })
        );
        assert_eq!(state.selected, vec![1, 2]);
    }

    #[test]
    fn widget_action_turns_itself_into_a_ui_event_carrying_the_widget_id() {
        let id = WidgetId::new("list:action");
        let event = ListViewEvent::ItemActivated { idx: 1 }.into_ui_event(id);
        match event {
            crate::UiEvent::List(event_id, ListViewEvent::ItemActivated { idx }) => {
                assert_eq!(event_id, id);
                assert_eq!(idx, 1);
            }
            other => panic!("expected UiEvent::List(.., ItemActivated), got {other:?}"),
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
    /// evidence they painted the same thing.
    #[derive(Default)]
    struct LogSurface {
        calls: Vec<String>,
    }

    impl PaintSurface for LogSurface {
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

    fn sample_list() -> ListView {
        ListView {
            id: WidgetId::new("quickfix"),
            title: Some(StyledText::plain("QUICKFIX")),
            items: (0..5)
                .map(|i| ListItem {
                    text: StyledText::plain(format!("row {i}")),
                    icon: None,
                    detail: Some(StyledText::plain("detail")),
                    decoration: Decoration::default(),
                })
                .collect(),
            selected_idx: 2,
            scroll_offset: 0,
            has_focus: true,
            bordered: false,
            h_scroll: 0,
            max_content_width: Some(60),
            show_v_scrollbar: false,
        }
    }

    /// The acceptance criterion: `ListView`'s [`Primitive::layout`] +
    /// [`Primitive::paint`] must produce the exact same verb log as
    /// calling [`native_surface_paint::paint`] directly with the
    /// equivalent arguments — the pilot changes *how* a caller reaches
    /// the paint, not *what* gets painted.
    #[test]
    fn list_primitive_paint_matches_native_surface_paint() {
        let list = sample_list();
        let rect = Rect::new(10.0, 20.0, 200.0, 80.0);
        let ctx = LayoutCtx::new(16.0, 8.0, true);
        let theme = Theme::default();
        let style = Style::default();

        let via_primitive_layout = Primitive::layout(&list, &ctx, rect);
        let mut via_primitive_surface = LogSurface::default();
        Primitive::paint(
            &list,
            &via_primitive_layout,
            &mut via_primitive_surface,
            &crate::PaintEnv::new(&theme, &style),
        );

        let direct_layout = crate::primitives::layout_metrics::list_layout(
            &list,
            rect.width as f64,
            rect.height as f64,
            ctx.line_height as f64,
            ctx.char_width as f64,
            0.0,
        );
        let mut direct_surface = LogSurface::default();
        native_surface_paint::paint(
            &list,
            rect,
            &direct_layout,
            ctx.line_height,
            ctx.nerd_fonts_enabled,
            false,
            true,
            &mut direct_surface,
            &theme,
            &style,
            None,
            None,
            false,
        );

        assert!(
            !via_primitive_surface.calls.is_empty(),
            "the list must paint something for this comparison to be meaningful"
        );
        assert_eq!(via_primitive_surface.calls, direct_surface.calls);
    }

    /// A bordered `ListView`'s [`Primitive::layout`] + [`Primitive::paint`]
    /// strokes no frame of its own — [`native_surface_paint::paint`]
    /// contains no frame-drawing verb, so `Primitive::layout` must pass
    /// `border_inset: 0.0` and `Primitive::paint` must pass
    /// `supports_border: false` to [`crate::primitives::layout_metrics::list_layout`]
    /// / [`native_surface_paint::paint_with_state`] regardless of
    /// [`ListView::bordered`], matching [`crate::macos::list`]'s
    /// pre-migration `draw_list`, which never strokes a frame either.
    #[test]
    fn bordered_list_primitive_seam_strokes_no_frame_of_its_own() {
        let list = ListView {
            bordered: true,
            // No h-scrollbar thumb to confound the "no frame stroke"
            // assertion below — `sample_list`'s `max_content_width:
            // Some(60)` would otherwise paint one via
            // `fill_rounded_rect`, unrelated to `bordered`.
            max_content_width: None,
            ..sample_list()
        };
        let rect = Rect::new(10.0, 20.0, 200.0, 80.0);
        let ctx = LayoutCtx::new(16.0, 8.0, true);
        let theme = Theme::default();
        let style = Style::default();

        let via_primitive_layout = Primitive::layout(&list, &ctx, rect);
        let mut via_primitive_surface = LogSurface::default();
        Primitive::paint(
            &list,
            &via_primitive_layout,
            &mut via_primitive_surface,
            &crate::PaintEnv::new(&theme, &style),
        );

        let direct_layout = crate::primitives::layout_metrics::list_layout(
            &list,
            rect.width as f64,
            rect.height as f64,
            ctx.line_height as f64,
            ctx.char_width as f64,
            /* border_inset */ 0.0,
        );
        let mut direct_surface = LogSurface::default();
        native_surface_paint::paint_with_state(
            &list,
            rect,
            &direct_layout,
            ctx.line_height,
            ctx.nerd_fonts_enabled,
            /* supports_border */ false,
            true,
            &mut direct_surface,
            &theme,
            &style,
            None,
            None,
            false,
            None,
            false,
        );

        assert!(
            !direct_surface
                .calls
                .iter()
                .any(|call| call.starts_with("fill_rounded_rect") || call.starts_with("draw_line")),
            "a bordered list's own paint must not stroke a frame through this seam — \
             each backend's thin wrapper strokes its own frame around this call instead, \
             got {:?}",
            direct_surface.calls
        );
        assert_eq!(via_primitive_surface.calls, direct_surface.calls);
    }

    /// [`ListViewPrimitiveLayout::hit_test`] subtracts the area's
    /// origin before delegating — a click at the layout's absolute
    /// origin must resolve to the same hit as `(0, 0)` on the
    /// underlying list-local [`ListViewLayout`].
    #[test]
    fn primitive_layout_hit_test_is_relative_to_area_origin() {
        let list = sample_list();
        let rect = Rect::new(10.0, 20.0, 200.0, 80.0);
        let ctx = LayoutCtx::new(16.0, 8.0, false);

        let layout = Primitive::layout(&list, &ctx, rect);
        assert_eq!(
            layout.hit_test(rect.x, rect.y + layout.line_height / 2.0),
            layout.layout.hit_test(0.0, layout.line_height / 2.0),
        );
    }
}
