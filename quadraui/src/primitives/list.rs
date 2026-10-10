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
//! `items[scroll_offset..]` until the viewport fills. Click on row →
//! emit `ListViewEvent::ItemActivated { idx }`. Keyboard `j`/`k`/`Enter`
//! emit the corresponding events. The app updates `selected_idx` and
//! `scroll_offset` for the next frame.

use crate::event::Rect;
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
    /// Horizontal scroll offset in chars (number of content columns to skip
    /// from the left before rendering). Default `0` = no scroll.
    /// The caller increments / decrements this in response
    /// to Left / Right key events.
    #[serde(default)]
    pub h_scroll: usize,
    /// Total width (in chars) of the widest item row, including the 2-char
    /// selection prefix, any icon, and the main text. When
    /// `max_content_width > visible_area_width` the TUI rasteriser reserves
    /// the bottom row of the list area for a horizontal scrollbar.
    /// `None` = caller hasn't measured content; no scrollbar shown.
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
/// `list_hscrollbar` rasteriser so `bordered` handling can't drift
/// between them (#790 — the macOS copy had silently dropped it while
/// GTK and Windows stayed in sync as 0.98-similarity twins).
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
    /// (quadraui#817).
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
#[derive(Debug, Clone, PartialEq)]
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

        // Clamp scroll_offset.
        let resolved_scroll_offset =
            crate::primitives::scrollbar::clamp_scroll_offset(self.scroll_offset, self.items.len());

        let items_y_end = if self.bordered {
            inset_y + items_h_max
        } else {
            viewport_height
        };

        let mut y = items_y_start;
        for i in resolved_scroll_offset..self.items.len() {
            if y >= items_y_end {
                break;
            }
            let m = measure_item(i);
            let remaining = items_y_end - y;
            let height = m.height.min(remaining).max(0.0);
            if height <= 0.0 {
                break;
            }
            let bounds = Rect::new(inset_x, y, item_w, height);
            visible_items.push(VisibleListItem {
                item_idx: i,
                bounds,
            });
            hit_regions.push((bounds, ListViewHit::Item(i)));
            y += m.height;
        }

        ListViewLayout {
            viewport_width,
            viewport_height,
            title_bounds,
            visible_items,
            hit_regions,
            resolved_scroll_offset,
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
            self.id.clone(),
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
        if !self.show_v_scrollbar {
            return None;
        }
        let total = self.items.len() as f32;
        if total == 0.0 {
            return None;
        }
        // Track occupies the rightmost content column; its height spans the
        // content rows (below the title in flat mode, between borders in
        // bordered mode).
        let (track_x, track_y, track_h) = if self.bordered {
            // In bordered mode: right border at area.x + area.width - 1,
            // so the track sits at area.x + area.width - 2*row_height (last
            // content column), spanning from top-border to bottom-border.
            let x = area.x + area.width - 2.0 * row_height;
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
            let x = area.x + area.width - row_height;
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
        let track = Rect::new(track_x, track_y, row_height, track_h);
        Some(Scrollbar::vertical(
            self.id.clone(),
            track,
            self.scroll_offset as f32,
            total,
            visible,
            row_height,
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

    /// Pixel-unit regression for #790: every pixel backend
    /// (GTK/Windows/macOS) calls `hscrollbar_track` with `char_w`/`row_h`
    /// in real pixels, not the `1.0`/`1.0` TUI-cell units the
    /// `ListView::hscrollbar` wrapper above exercises. Assert the inset
    /// math holds at those units too, since a bug that only shows up when
    /// `char_w != 1.0` would slip past `hscrollbar_tests` entirely — which
    /// is exactly how the macOS `list_hscrollbar` bug (dropping `bordered`
    /// altogether) went unnoticed: nothing exercised the pixel-unit path.
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
        // #790: "thumb bounds sit inside the border on a bordered list."
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
}

// ── PaintSurface paint (#1075, PaintSurface Phase 4 slice 2/8) ───────────
//
// Before this, `gtk::draw_list` (Cairo), `macos::list::draw_list` (Core
// Graphics) and `win::list::draw_list` (Direct2D) each independently
// painted the same title/row/decoration/scrollbar content with their own
// drawing API. `paint` below is the one shared implementation, written
// against [`crate::paint_surface::PaintSurface`] instead of any one
// backend's API — see `crate::primitives::split_tree::native_surface_paint`
// and `crate::primitives::scrollbar::native_surface_paint` for the same
// pattern applied to earlier primitives.
//
// What this migration fixes (issue #1075's named drift):
//
// - **Missing vertical scrollbar.** `Backend::list_vscrollbar` already
//   returns real track/thumb geometry on every pixel backend (used for
//   hit-testing/dragging), but none of the three `draw_list`s ever
//   painted it — `paint` below does, via
//   [`crate::primitives::scrollbar::native_surface_paint::paint`], the
//   same helper the horizontal scrollbar already used on GTK/macOS.
//
// What this migration does NOT change: `bordered`'s frame stroke — GTK
// clips to (and later strokes) a *rounded* rect, Windows fills a plain
// square 1-DIP frame, and macOS renders no frame at all yet (see
// `macos::list`'s module doc, "Scope omissions"). None of the three
// backends has a `PaintSurface` verb for a rounded stroke, and
// unifying "does this backend draw a frame at all" is a real,
// documented per-backend capability difference rather than paint/click
// drift — so each backend's thin wrapper still paints its own frame
// immediately before/after calling this shared `paint` for the content.
// `supports_border` below only toggles the *content*-side effects of
// `bordered` (the inset content area + overlay title vs. the flat
// header+rows layout) that a backend without frame support should skip
// — matching what `macos::list`'s pre-migration `draw_list` already did
// (it never took the overlay-title branch, so `title_overlay` is always
// `false` there, same as passing `supports_border: false`).
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{ListView, ListViewLayout};
    use crate::paint_surface::PaintSurface;
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

            let row_rect = Rect::new(row_x, row_y, row_w, row_h);
            surface.surface_fill_rect(row_rect, row_bg);

            // Per-row clip: with h_scroll the cursor starts to the left
            // of `row_x`, so scrolled-off glyphs would paint outside the
            // row band without an explicit clip.
            surface.surface_push_clip(row_rect);
            let mut cursor_x = row_x + 2.0 - h_off_px;

            let prefix = if is_selected { "▶ " } else { "  " };
            let (pw, ph) = surface.surface_measure_text(prefix);
            surface.surface_draw_text_run(
                Rect::new(cursor_x, row_y + (row_h - ph) / 2.0, pw, ph),
                prefix,
                decoration_fg,
            );
            cursor_x += pw;

            if let Some(ref icon) = item.icon {
                let glyph = if nerd_fonts_enabled {
                    icon.glyph.as_str()
                } else {
                    icon.fallback.as_str()
                };
                let (iw, ih) = surface.surface_measure_text(glyph);
                surface.surface_draw_text_run(
                    Rect::new(cursor_x, row_y + (row_h - ih) / 2.0, iw, ih),
                    glyph,
                    decoration_fg,
                );
                cursor_x += iw + 6.0;
            }

            let detail_info = item.detail.as_ref().map(|detail| {
                let detail_text: String = detail.spans.iter().map(|s| s.text.as_str()).collect();
                let (dw, _) = surface.surface_measure_text(&detail_text);
                (detail_text, dw)
            });
            let detail_reserve = detail_info.as_ref().map(|(_, dw)| *dw + 8.0).unwrap_or(0.0);
            // text_right_limit is absolute; the h_scroll shift of
            // cursor_x does not affect where the detail reserve
            // boundary sits.
            let text_right_limit = row_x + row_w - detail_reserve - 4.0;

            for span in &item.text.spans {
                if cursor_x >= text_right_limit {
                    break;
                }
                let span_fg = span.fg.unwrap_or(decoration_fg);
                if let Some(sbg) = span.bg {
                    let (sw, _) = surface.surface_measure_text_styled(&span.text, span.bold);
                    surface.surface_fill_rect(
                        Rect::new(cursor_x, row_y, sw.min(text_right_limit - cursor_x), row_h),
                        sbg,
                    );
                }
                let (sw, sh) = surface.surface_measure_text_styled(&span.text, span.bold);
                surface.surface_draw_text_run_styled(
                    Rect::new(cursor_x, row_y + (row_h - sh) / 2.0, sw, sh),
                    &span.text,
                    span_fg,
                    span.bold,
                    false,
                    false,
                    1.0,
                );
                cursor_x += sw;
            }

            // Detail text is pinned to the visible viewport (does not
            // scroll with h_scroll) so it stays readable regardless of
            // scroll position — render it outside the h_scroll-shifted
            // row clip.
            surface.surface_pop_clip();

            if let Some((detail_text, dw)) = detail_info {
                let dx = row_x + row_w - dw - 4.0;
                if dx > cursor_x {
                    let (_, dh) = surface.surface_measure_text(&detail_text);
                    surface.surface_draw_text_run(
                        Rect::new(dx, row_y + (row_h - dh) / 2.0, dw, dh),
                        &detail_text,
                        theme.muted_fg,
                    );
                }
            }
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
                list.id.clone(),
                hsb_track,
                list.h_scroll as f32 * char_w,
                content_px,
                visible_px,
                line_height,
            );
            crate::primitives::scrollbar::native_surface_paint::paint(&hsb, surface, theme);
        }

        // ── Vertical scrollbar (#1075 fix: previously never painted) ───
        if let Some(vsb) = list.vscrollbar(area, line_height) {
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

        /// Records every fill + text-run call — mirrors
        /// `primitives::scrollbar`'s `RecordingSurface` test double,
        /// scoped to the verbs this primitive uses, so this test runs on
        /// any host without Cairo/Core Graphics/Direct2D.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
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
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_stroke_rounded_rect(
                &mut self,
                _rect: Rect,
                _radius: f32,
                _color: Color,
                _stroke_width: f32,
            ) {
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

        /// #1075 regression: before this migration, none of the three
        /// backends' `draw_list` painted `ListView::show_v_scrollbar`'s
        /// track/thumb even though `Backend::list_vscrollbar` already
        /// exposed real geometry for hit-testing. This test paints
        /// through the shared fn and asserts a fill lands at the exact
        /// track rect `ListView::vscrollbar` resolves — this would have
        /// failed (no such fill recorded) against any of the pre-#1075
        /// per-backend `draw_list` bodies.
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
            );

            let expected = list
                .vscrollbar(AREA, LINE_HEIGHT)
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
            );
            assert!(ascii_surface.texts.iter().any(|(_, t, _)| t == "F"));
        }
    }
}

/// Events a `ListView` emits back to the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ListViewEvent {
    /// Keyboard / mouse moved selection to a different row.
    SelectionChanged { idx: usize },
    /// User confirmed a row (Enter or double-click).
    ItemActivated { idx: usize },
    /// A key was pressed while the list had focus and the primitive
    /// did not consume it. App may interpret it (e.g. `q` closes the
    /// quickfix panel).
    KeyPressed { key: String, modifiers: Modifiers },
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
