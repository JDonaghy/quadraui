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
    /// The caller (e.g. coord-tui) increments / decrements this in response
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

/// Per-item measurement supplied by the backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ListItemMeasure {
    pub height: f32,
}

impl ListItemMeasure {
    pub fn new(height: f32) -> Self {
        Self { height }
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
        let resolved_scroll_offset = if self.items.is_empty() {
            0
        } else {
            self.scroll_offset.min(self.items.len() - 1)
        };

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
        // row so it stays inside the box, above the bottom border.
        let (track_x, track_w, track_y) = if self.bordered {
            (
                area.x + 1.0,
                (area.width - 2.0).max(0.0),
                area.y + (area.height - 2.0 * row_height).max(0.0),
            )
        } else {
            (
                area.x,
                area.width,
                area.y + (area.height - row_height).max(0.0),
            )
        };
        let track = Rect::new(track_x, track_y, track_w, row_height);
        Some(Scrollbar::horizontal(
            self.id.clone(),
            track,
            self.h_scroll as f32,
            total,
            visible_w,
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
