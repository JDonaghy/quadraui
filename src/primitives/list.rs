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

        // Title row (if present and reserved a height).
        let title_bounds = if self.title.is_some() && title_height > 0.0 {
            let title_h = title_height.min(viewport_height);
            let bounds = Rect::new(0.0, 0.0, viewport_width, title_h);
            hit_regions.push((bounds, ListViewHit::Title));
            Some(bounds)
        } else {
            None
        };
        let items_y_start = title_bounds.map(|b| b.y + b.height).unwrap_or(0.0);

        // Clamp scroll_offset.
        let resolved_scroll_offset = if self.items.is_empty() {
            0
        } else {
            self.scroll_offset.min(self.items.len() - 1)
        };

        let mut y = items_y_start;
        for i in resolved_scroll_offset..self.items.len() {
            if y >= viewport_height {
                break;
            }
            let m = measure_item(i);
            let remaining = viewport_height - y;
            let height = m.height.min(remaining).max(0.0);
            if height <= 0.0 {
                break;
            }
            let bounds = Rect::new(0.0, y, viewport_width, height);
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
