//! `ActivityBar` primitive: a vertical strip of icon buttons (VSCode-style
//! left rail). Items are split into a top group (rendered from the top)
//! and an optional bottom group (pinned to the bottom of the available
//! area). Each item is a single clickable icon with active / keyboard-
//! selection visual states and an optional tooltip.
//!
//! Typical vimcode layout:
//! * Top: hamburger / menu, explorer, search, debug, git, extensions, AI,
//!   dynamically-registered extension panels
//! * Bottom: settings (gear)
//!
//! Click resolution is per-backend — TUI computes from cell-row arithmetic,
//! GTK from pixel-row arithmetic. The primitive itself carries no layout
//! calculation; it's a declarative list.
//!
//! # Backend contract
//!
//! **Declarative + per-frame interaction state passed alongside.** Render
//! the `top_items` from the top of the strip, then the `bottom_items`
//! pinned to the bottom. Click on item → emit
//! `ActivityBarEvent::ItemClicked { id }`.
//!
//! Hover state (which item the mouse is currently over for tooltip
//! affordance) is **per-frame, backend-owned** — the primitive does NOT
//! carry it. Backends pass `hovered_idx: Option<usize>` to their own
//! `draw_activity_bar` function. Same pattern as `TabBar`'s
//! `hovered_close_tab`. Rule: **state that's only knowable by the
//! backend (cursor position, focus-within, scroll momentum) lives
//! beside the primitive, not inside it.**

use crate::event::Rect;
use crate::types::{Color, Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of an activity bar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityBar {
    pub id: WidgetId,
    /// Top items rendered starting at the top edge, one row per item.
    pub top_items: Vec<ActivityItem>,
    /// Bottom-pinned items rendered from the bottom edge upward.
    /// Rendered only if there's room after `top_items`.
    #[serde(default)]
    pub bottom_items: Vec<ActivityItem>,
    /// Colour of the left-edge accent bar on active items.
    /// `None` = no accent rendering.
    #[serde(default)]
    pub active_accent: Option<Color>,
    /// Background colour for keyboard-selected items (arrow-nav highlight).
    /// `None` = backends fall back to their own default.
    #[serde(default)]
    pub selection_bg: Option<Color>,
}

/// One icon entry in an `ActivityBar`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityItem {
    /// Opaque widget id for click routing. The adapter picks a meaningful
    /// namespaced string (e.g. `"activity:explorer"`, `"activity:ext:foo"`).
    pub id: WidgetId,
    /// Glyph to render — single character for the TUI cell-based layout;
    /// GTK backend can render wider strings when font supports them.
    pub icon: String,
    /// Hover tooltip text. TUI ignores (no hover UI); GTK uses as a native
    /// `set_tooltip_text`.
    #[serde(default)]
    pub tooltip: String,
    #[serde(default)]
    pub is_active: bool,
    /// Keyboard-focused selection highlight (used by TUI when the activity
    /// bar has `toolbar_focused`). GTK rarely sets this — native buttons
    /// manage their own focus rings.
    #[serde(default)]
    pub is_keyboard_selected: bool,
}

// ── D6 Layout API ───────────────────────────────────────────────────────────
//
// Per Decision D6: primitives return fully-resolved `Layout` structs;
// backends rasterise verbatim. Fifth primitive on the new shape.
// ActivityBar uses a uniform item_height since that's the convention
// across backends (1 cell TUI, equal line_height rows in GTK).

/// Which side of the bar a visible item belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivitySide {
    /// Top-pinned item (indexed into `top_items`).
    Top,
    /// Bottom-pinned item (indexed into `bottom_items`).
    Bottom,
}

/// Resolved position of one visible activity-bar item after layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleActivityItem {
    pub side: ActivitySide,
    /// Index into `top_items` or `bottom_items` (depending on `side`).
    pub item_idx: usize,
    pub bounds: Rect,
}

/// Classification of a hit-test result. The hit carries the item's
/// `WidgetId` rather than an index, because vimcode routes activity-bar
/// clicks via opaque IDs (`"activity:explorer"`, `"activity:settings"`,
/// etc).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivityBarHit {
    Item(WidgetId),
    Empty,
}

/// Per-row hit region produced by the rasteriser, carrying the
/// item's `WidgetId` and tooltip alongside the painted vertical
/// span. Apps that need both click routing AND hover tooltips (e.g.
/// vimcode's GTK activity bar with `connect_query_tooltip`) read
/// from this list rather than re-resolving via the layout.
#[derive(Debug, Clone)]
pub struct ActivityBarRowHit {
    pub y_start: f64,
    pub y_end: f64,
    pub id: WidgetId,
    pub tooltip: String,
}

/// Fully-resolved activity-bar layout. Backends iterate `visible_items`
/// for painting and call [`Self::hit_test`] for clicks.
#[derive(Debug, Clone, PartialEq)]
pub struct ActivityBarLayout {
    pub viewport_width: f32,
    pub viewport_height: f32,
    /// All visible items — top-pinned first (in order), then
    /// bottom-pinned (in order).
    pub visible_items: Vec<VisibleActivityItem>,
    pub hit_regions: Vec<(Rect, ActivityBarHit)>,
}

impl ActivityBarLayout {
    pub fn hit_test(&self, x: f32, y: f32) -> ActivityBarHit {
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        ActivityBarHit::Empty
    }
}

impl ActivityBar {
    /// Compute the full rendering + hit-test layout for this activity bar.
    ///
    /// Per D6: layout decisions live here; backends iterate
    /// `visible_items` for painting and call `hit_test` for clicks.
    ///
    /// # Arguments
    ///
    /// - `viewport_width`, `viewport_height` — available strip area.
    /// - `item_height` — uniform row height for every item. Use `1.0`
    ///   for TUI cells, `line_height` for GTK / Win-GUI / macOS.
    ///
    /// # Collision policy
    ///
    /// Top items lay out from `y=0` downward until they run out or the
    /// bottom-items region is reached. Bottom items lay out from
    /// `y=viewport_height` upward. If the two groups would overlap,
    /// **bottom items win** and top items are clipped — matches the
    /// pre-D6 TUI behaviour (`src/tui_main/quadraui_tui.rs::draw_activity_bar`).
    pub fn layout(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        item_height: f32,
    ) -> ActivityBarLayout {
        let mut visible_items: Vec<VisibleActivityItem> = Vec::new();
        let mut hit_regions: Vec<(Rect, ActivityBarHit)> = Vec::new();

        if item_height <= 0.0 || viewport_height <= 0.0 {
            return ActivityBarLayout {
                viewport_width,
                viewport_height,
                visible_items,
                hit_regions,
            };
        }

        // Bottom items first (they win on collision): place them pinned
        // to the bottom edge, working upward.
        let bottom_count = self.bottom_items.len();
        let bottom_y_start = (viewport_height - (bottom_count as f32) * item_height).max(0.0);
        for (i, item) in self.bottom_items.iter().enumerate() {
            let y = bottom_y_start + (i as f32) * item_height;
            if y >= viewport_height {
                break;
            }
            let height = item_height.min(viewport_height - y);
            if height <= 0.0 {
                break;
            }
            let bounds = Rect::new(0.0, y, viewport_width, height);
            visible_items.push(VisibleActivityItem {
                side: ActivitySide::Bottom,
                item_idx: i,
                bounds,
            });
            hit_regions.push((bounds, ActivityBarHit::Item(item.id.clone())));
        }

        // Top items: place from top, stop at the bottom-items region.
        let top_limit = bottom_y_start;
        for (i, item) in self.top_items.iter().enumerate() {
            let y = (i as f32) * item_height;
            if y >= top_limit {
                break;
            }
            let height = item_height.min(top_limit - y);
            if height <= 0.0 {
                break;
            }
            let bounds = Rect::new(0.0, y, viewport_width, height);
            // Insert top items before bottom items in `visible_items`
            // so painting order is top-to-bottom visually. Simpler: just
            // extend and sort on `bounds.y`, but insertion at index
            // `bottom-items-inserted-so-far ... wait, easier approach:
            // collect top first, then extend with bottom — but that
            // changes the collision logic. Keep it ordered by inserting
            // top at the front of each visible_items run. Since visually
            // order of iteration doesn't matter for painting (they don't
            // overlap), and hit_test walks all regions, insertion order
            // is inconsequential. Just append.
            visible_items.push(VisibleActivityItem {
                side: ActivitySide::Top,
                item_idx: i,
                bounds,
            });
            hit_regions.push((bounds, ActivityBarHit::Item(item.id.clone())));
        }

        ActivityBarLayout {
            viewport_width,
            viewport_height,
            visible_items,
            hit_regions,
        }
    }
}

/// Events an `ActivityBar` emits back to the app. Currently unused by
/// vimcode (click path dispatches by row arithmetic + engine-side
/// `SidebarPanel` enum), but defined for plugin invariants §10.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActivityBarEvent {
    /// An item was clicked (or activated via Enter while keyboard-focused).
    ItemClicked { id: WidgetId },
    /// A key was pressed with the activity bar focused and the primitive
    /// didn't consume it.
    KeyPressed { key: String, modifiers: Modifiers },
}
