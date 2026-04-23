//! `ContextMenu` primitive: a keyboard/mouse-navigable popup of actions
//! triggered at a specific screen location (right-click, keyboard
//! shortcut, explicit open-menu command). Each item is either an
//! action (clickable, emits an id) or a separator (visual only).
//!
//! Submenus are out of scope for v1 — flat menus only. Adding submenus
//! would require an additional `children` field on items and nested
//! layout state; revisit once a consumer needs them.
//!
//! # Backend contract
//!
//! **Modal overlay.** Render as a popup at the computed position;
//! intercept clicks so they don't fall through to the underlying UI.
//! Click on action item → `ContextMenuEvent::ItemActivated { id }`;
//! click outside → `Cancelled`. Keyboard up/down moves `selected_idx`
//! (skipping separators); Enter activates the selected item; Escape
//! emits `Cancelled`.

use crate::event::Rect;
use crate::types::{Color, Modifiers, StyledText, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a context menu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextMenu {
    pub id: WidgetId,
    pub items: Vec<ContextMenuItem>,
    /// Index of the keyboard-selected item. Apps are responsible for
    /// skipping separators when navigating.
    pub selected_idx: usize,
    /// Background colour override. `None` = theme default.
    #[serde(default)]
    pub bg: Option<Color>,
}

/// One entry in a `ContextMenu`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextMenuItem {
    /// `None` = separator (non-interactive); `Some(id)` = action.
    #[serde(default)]
    pub id: Option<WidgetId>,
    /// Label for the item. Ignored for separators.
    pub label: StyledText,
    /// Optional right-aligned detail (e.g. keyboard shortcut "Ctrl+C").
    #[serde(default)]
    pub detail: Option<StyledText>,
    /// When true, the item is rendered dimmed and click emits no event.
    #[serde(default)]
    pub disabled: bool,
}

impl ContextMenuItem {
    /// Convenience: is this item a separator (non-clickable)?
    pub fn is_separator(&self) -> bool {
        self.id.is_none()
    }
}

/// Events a `ContextMenu` emits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextMenuEvent {
    ItemActivated {
        id: WidgetId,
    },
    SelectionChanged {
        idx: usize,
    },
    /// Menu dismissed by click-outside, Escape, or window change.
    Cancelled,
    /// Key pressed while the menu had focus and the primitive didn't
    /// consume it.
    KeyPressed {
        key: String,
        modifiers: Modifiers,
    },
}

// ── D6 Layout API ───────────────────────────────────────────────────────────

/// Per-item measurement supplied by the backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextMenuItemMeasure {
    pub height: f32,
}

impl ContextMenuItemMeasure {
    pub fn new(height: f32) -> Self {
        Self { height }
    }
}

/// Resolved position of one visible context-menu item.
#[derive(Debug, Clone, PartialEq)]
pub struct VisibleContextMenuItem {
    pub item_idx: usize,
    pub bounds: Rect,
    /// `true` iff this item is a separator (no hit region, renders as
    /// a horizontal divider).
    pub is_separator: bool,
    /// `true` iff this item is clickable (has an `id` and isn't
    /// disabled). Separators and disabled items are `false`.
    pub clickable: bool,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextMenuHit {
    /// Click landed on an actionable item.
    Item(WidgetId),
    /// Click landed on a non-interactive item (separator or disabled).
    Inert,
    /// Click landed outside the menu — apps typically dismiss.
    Empty,
}

/// Fully-resolved context-menu layout.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextMenuLayout {
    /// Full bounds of the menu box.
    pub bounds: Rect,
    pub visible_items: Vec<VisibleContextMenuItem>,
    pub hit_regions: Vec<(Rect, ContextMenuHit)>,
}

impl ContextMenuLayout {
    pub fn hit_test(&self, x: f32, y: f32) -> ContextMenuHit {
        // Inside menu bounds?
        let inside = x >= self.bounds.x
            && x < self.bounds.x + self.bounds.width
            && y >= self.bounds.y
            && y < self.bounds.y + self.bounds.height;
        if !inside {
            return ContextMenuHit::Empty;
        }
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        // Inside menu, but not on any item (e.g. border padding) → treat as Inert.
        ContextMenuHit::Inert
    }
}

impl ContextMenu {
    /// Compute menu placement and per-item bounds.
    ///
    /// # Arguments
    ///
    /// - `anchor_x`, `anchor_y` — preferred top-left origin (typically
    ///   the click position). The menu shifts left / up if placing it
    ///   here would overflow the viewport.
    /// - `viewport` — parent surface bounds; menu is clamped inside.
    /// - `menu_width` — width of the menu box.
    /// - `measure_item(i)` — height for item `i`.
    ///
    /// # Overflow handling
    ///
    /// If `anchor_x + menu_width > viewport.right`, the menu shifts
    /// left so its right edge aligns with the viewport right edge.
    /// Same for the bottom edge. If the menu is taller than the
    /// viewport, items beyond the bottom edge are not emitted as
    /// visible (no scrolling in v1 — if this is a real problem,
    /// consumers should use `Palette` instead).
    pub fn layout<F>(
        &self,
        anchor_x: f32,
        anchor_y: f32,
        viewport: Rect,
        menu_width: f32,
        measure_item: F,
    ) -> ContextMenuLayout
    where
        F: Fn(usize) -> ContextMenuItemMeasure,
    {
        let measures: Vec<ContextMenuItemMeasure> =
            (0..self.items.len()).map(&measure_item).collect();
        let total_height: f32 = measures.iter().map(|m| m.height).sum();

        // Position the menu.
        let x = if anchor_x + menu_width > viewport.x + viewport.width {
            (viewport.x + viewport.width - menu_width).max(viewport.x)
        } else {
            anchor_x.max(viewport.x)
        };
        let y = if anchor_y + total_height > viewport.y + viewport.height {
            (viewport.y + viewport.height - total_height).max(viewport.y)
        } else {
            anchor_y.max(viewport.y)
        };

        let clipped_height = total_height.min(viewport.y + viewport.height - y);
        let bounds = Rect::new(x, y, menu_width, clipped_height);

        let mut visible_items: Vec<VisibleContextMenuItem> = Vec::new();
        let mut hit_regions: Vec<(Rect, ContextMenuHit)> = Vec::new();

        let mut cursor_y = y;
        for (i, item) in self.items.iter().enumerate() {
            if cursor_y >= y + clipped_height {
                break;
            }
            let h = measures[i].height;
            let remaining = y + clipped_height - cursor_y;
            let draw_h = h.min(remaining).max(0.0);
            if draw_h <= 0.0 {
                break;
            }
            let item_bounds = Rect::new(x, cursor_y, menu_width, draw_h);
            let is_sep = item.is_separator();
            let clickable = !is_sep && !item.disabled;
            visible_items.push(VisibleContextMenuItem {
                item_idx: i,
                bounds: item_bounds,
                is_separator: is_sep,
                clickable,
            });
            if clickable {
                if let Some(id) = &item.id {
                    hit_regions.push((item_bounds, ContextMenuHit::Item(id.clone())));
                }
            } else {
                hit_regions.push((item_bounds, ContextMenuHit::Inert));
            }
            cursor_y += h;
        }

        ContextMenuLayout {
            bounds,
            visible_items,
            hit_regions,
        }
    }
}
