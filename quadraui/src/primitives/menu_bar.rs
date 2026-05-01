//! `MenuBar` primitive: a horizontal strip of top-level menu labels
//! (File / Edit / View / ...). Each top-level item opens a dropdown
//! menu — represented in vimcode's rendering path by a
//! `ContextMenu`-style popup. The menu bar itself is just the
//! navigation strip; the dropdown is a separate concern the app
//! composes when a menu is open.
//!
//! Used for the top-of-window menu on Linux / Windows (macOS uses the
//! global menu bar, which this primitive maps to identically — the
//! backend decides whether to actually draw the strip or defer to
//! NSMenu).
//!
//! # Backend contract
//!
//! **Declarative.** Render the menu-bar row with each top-level item
//! as a clickable label. Click / keyboard-navigation emits
//! `ItemActivated { idx }`; the app opens a dropdown next to the
//! item using the returned `hit_regions` position. Keyboard Alt+key
//! activates the item whose label starts with that character.

use crate::event::Rect;
use crate::types::{Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of a menu bar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuBar {
    pub id: WidgetId,
    pub items: Vec<MenuBarItem>,
    /// Index of the currently-open menu (if any). Backends use this to
    /// render the "pressed" visual on the active item.
    #[serde(default)]
    pub open_item: Option<usize>,
    /// Keyboard-focused item (for Alt+navigation) — may differ from
    /// `open_item` during arrow-key traversal.
    #[serde(default)]
    pub focused_item: Option<usize>,
}

/// One top-level menu entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuBarItem {
    pub id: WidgetId,
    /// Display label, e.g. `"&File"` (with the `&` marking the
    /// Alt-activation character — backends render the following char
    /// underlined and map Alt+that-char to this item). If no `&` is
    /// present, Alt activation uses the first character.
    pub label: String,
    /// When true, the item is rendered dimmed and clicks are ignored.
    #[serde(default)]
    pub disabled: bool,
}

/// Events a `MenuBar` emits back to the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MenuBarEvent {
    /// User clicked or Alt-activated a top-level item. App opens the
    /// corresponding dropdown.
    ItemActivated { idx: usize },
    /// User pressed a navigation key while the bar was focused.
    KeyPressed { key: String, modifiers: Modifiers },
    /// Menu-bar focus released (Escape or click outside).
    Cancelled,
}

// ── D6 Layout API ───────────────────────────────────────────────────────────

/// Per-item measurement (width in the backend's unit).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MenuBarItemMeasure {
    pub width: f32,
}

impl MenuBarItemMeasure {
    pub fn new(width: f32) -> Self {
        Self { width }
    }
}

/// Resolved position of one visible menu-bar item.
#[derive(Debug, Clone, PartialEq)]
pub struct VisibleMenuBarItem {
    pub item_idx: usize,
    pub id: WidgetId,
    pub bounds: Rect,
    /// `true` iff the item is clickable (not disabled).
    pub clickable: bool,
}

/// Classification of a hit-test result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuBarHit {
    /// Click landed on a top-level item.
    Item(usize),
    /// Click landed on the bar (not on any item) — apps may swallow.
    Bar,
    /// Click landed outside the bar — apps may dismiss the open menu.
    Outside,
}

/// Fully-resolved menu-bar layout.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuBarLayout {
    /// Full bar bounds.
    pub bounds: Rect,
    pub visible_items: Vec<VisibleMenuBarItem>,
    pub hit_regions: Vec<(Rect, MenuBarHit)>,
}

impl MenuBarLayout {
    pub fn hit_test(&self, x: f32, y: f32) -> MenuBarHit {
        let inside = x >= self.bounds.x
            && x < self.bounds.x + self.bounds.width
            && y >= self.bounds.y
            && y < self.bounds.y + self.bounds.height;
        if !inside {
            return MenuBarHit::Outside;
        }
        for (rect, hit) in &self.hit_regions {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return hit.clone();
            }
        }
        MenuBarHit::Bar
    }
}

impl MenuBar {
    /// Compute item positions along the bar.
    ///
    /// # Arguments
    ///
    /// - `bounds` — menu-bar row.
    /// - `measure_item(i)` — width of item `i`.
    pub fn layout<F>(&self, bounds: Rect, measure_item: F) -> MenuBarLayout
    where
        F: Fn(usize) -> MenuBarItemMeasure,
    {
        let mut visible_items: Vec<VisibleMenuBarItem> = Vec::new();
        let mut hit_regions: Vec<(Rect, MenuBarHit)> = Vec::new();

        let mut cursor_x = bounds.x;
        for (i, item) in self.items.iter().enumerate() {
            let w = measure_item(i).width;
            if cursor_x + w > bounds.x + bounds.width {
                break;
            }
            let item_bounds = Rect::new(cursor_x, bounds.y, w, bounds.height);
            let clickable = !item.disabled;
            visible_items.push(VisibleMenuBarItem {
                item_idx: i,
                id: item.id.clone(),
                bounds: item_bounds,
                clickable,
            });
            if clickable {
                hit_regions.push((item_bounds, MenuBarHit::Item(i)));
            }
            cursor_x += w;
        }

        MenuBarLayout {
            bounds,
            visible_items,
            hit_regions,
        }
    }

    /// Find the index of the item whose label contains the Alt-key
    /// character `ch` (case-insensitive). The label's `&` prefix marks
    /// the activation character; if no `&`, the first character is
    /// used.
    pub fn find_alt_target(&self, ch: char) -> Option<usize> {
        let target = ch.to_ascii_lowercase();
        for (i, item) in self.items.iter().enumerate() {
            if item.disabled {
                continue;
            }
            let marker = item.label.find('&').map(|p| p + 1);
            let trigger = match marker {
                Some(idx) => item.label.chars().nth(idx),
                None => item.label.chars().next(),
            };
            if let Some(c) = trigger {
                if c.to_ascii_lowercase() == target {
                    return Some(i);
                }
            }
        }
        None
    }
}
