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
    /// How to position the menu relative to the anchor point. Default
    /// `AnchorPoint` (right-click conventions: anchor IS the click
    /// position; menu shifts up/left to fit but doesn't flip
    /// directionality). `Below` / `Above` enable dropdown-style
    /// auto-flip placement.
    #[serde(default)]
    pub placement: ContextMenuPlacement,
}

/// Preferred placement of a `ContextMenu` relative to its anchor.
///
/// `AnchorPoint` is the right-click default: the anchor IS the cursor
/// position. The menu's top-left corner aligns with the anchor; the
/// menu shifts up/left to keep the box inside the viewport but never
/// flips directionality.
///
/// `Below` and `Above` enable dropdown-style placement: the anchor is
/// the trigger element (e.g. a button), and the menu opens above or
/// below it. The layout auto-flips to the opposite side if the
/// preferred direction would overflow the viewport — same behaviour as
/// [`super::tooltip::TooltipPlacement`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ContextMenuPlacement {
    /// Right-click default: anchor is the cursor position. No flipping.
    #[default]
    AnchorPoint,
    /// Open below the anchor (e.g. dropdown attached to a top-row
    /// button). Auto-flips to above if it would overflow the bottom.
    Below,
    /// Open above the anchor (e.g. dropdown attached to a bottom-row
    /// button — kubeui's namespace picker). Auto-flips to below if it
    /// would overflow the top.
    Above,
}

/// Resolved placement after the layout decided whether to flip the
/// preferred [`ContextMenuPlacement`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedContextMenuPlacement {
    AnchorPoint,
    Below,
    Above,
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
    /// Placement actually used. For `AnchorPoint` always
    /// `ResolvedContextMenuPlacement::AnchorPoint`; for `Below` / `Above`
    /// reports whether the layout flipped to the opposite direction
    /// when the preferred side would have overflowed.
    pub resolved_placement: ResolvedContextMenuPlacement,
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
        self.layout_at(
            Rect::new(anchor_x, anchor_y, 0.0, 0.0),
            viewport,
            menu_width,
            measure_item,
        )
    }

    /// Anchor-rect variant of [`Self::layout`]. The anchor is a
    /// rectangle (typically the trigger button's bounds) instead of a
    /// single point — required for `Below` / `Above` placement so the
    /// menu can sit flush against the trigger's bottom or top edge.
    /// For `AnchorPoint` placement only `anchor.x` and `anchor.y` are
    /// used (top-left of the rect).
    pub fn layout_at<F>(
        &self,
        anchor: Rect,
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

        // Horizontal positioning (same for all placement modes): align
        // the menu's left edge with the anchor's left edge, but shift
        // left if the menu would overflow the viewport's right side.
        let x = if anchor.x + menu_width > viewport.x + viewport.width {
            (viewport.x + viewport.width - menu_width).max(viewport.x)
        } else {
            anchor.x.max(viewport.x)
        };

        // Vertical positioning depends on placement mode.
        let viewport_top = viewport.y;
        let viewport_bottom = viewport.y + viewport.height;
        let (y, resolved_placement) = match self.placement {
            ContextMenuPlacement::AnchorPoint => {
                // Right-click default: anchor IS the click point;
                // menu top-left aligns with anchor; shift up to fit if
                // the menu would overflow the viewport bottom.
                let y_pref = anchor.y;
                let y = if y_pref + total_height > viewport_bottom {
                    (viewport_bottom - total_height).max(viewport_top)
                } else {
                    y_pref.max(viewport_top)
                };
                (y, ResolvedContextMenuPlacement::AnchorPoint)
            }
            ContextMenuPlacement::Below => {
                // Dropdown opens below the trigger. Menu's top is at
                // the trigger's bottom edge. Auto-flip to Above if it
                // would overflow the viewport bottom AND there's more
                // room above than below.
                let space_below = viewport_bottom - (anchor.y + anchor.height);
                let space_above = anchor.y - viewport_top;
                if total_height > space_below && space_above > space_below {
                    // Flip to Above.
                    let y_pref = anchor.y - total_height;
                    let y = y_pref.max(viewport_top);
                    (y, ResolvedContextMenuPlacement::Above)
                } else {
                    let y_pref = anchor.y + anchor.height;
                    let y = if y_pref + total_height > viewport_bottom {
                        (viewport_bottom - total_height).max(viewport_top)
                    } else {
                        y_pref.max(viewport_top)
                    };
                    (y, ResolvedContextMenuPlacement::Below)
                }
            }
            ContextMenuPlacement::Above => {
                // Dropdown opens above the trigger (kubeui's status-bar
                // segment). Menu's bottom is at the trigger's top edge.
                // Auto-flip to Below if it would overflow the viewport
                // top AND there's more room below than above.
                let space_below = viewport_bottom - (anchor.y + anchor.height);
                let space_above = anchor.y - viewport_top;
                if total_height > space_above && space_below > space_above {
                    // Flip to Below.
                    let y_pref = anchor.y + anchor.height;
                    let y = if y_pref + total_height > viewport_bottom {
                        (viewport_bottom - total_height).max(viewport_top)
                    } else {
                        y_pref.max(viewport_top)
                    };
                    (y, ResolvedContextMenuPlacement::Below)
                } else {
                    let y_pref = anchor.y - total_height;
                    let y = y_pref.max(viewport_top);
                    (y, ResolvedContextMenuPlacement::Above)
                }
            }
        };

        let clipped_height = total_height.min(viewport_bottom - y);
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
            resolved_placement,
        }
    }
}
