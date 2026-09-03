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
//! as a clickable label. Click / keyboard-navigation resolves to
//! [`MenuBarHit::Item`]; the app opens a dropdown next to the item
//! using the returned `hit_regions` position. Keyboard Alt+key
//! activates the item whose label starts with that character.

use crate::event::Rect;
use crate::types::WidgetId;
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
    /// present, the label is never underlined (quadraui#625 — no
    /// implicit "underline the first char" fallback), though
    /// [`MenuBar::find_alt_target`]'s keyboard-activation lookup still
    /// falls back to the first character, unrelated to the visual
    /// underline.
    pub label: String,
    /// When true, the item is rendered dimmed and clicks are ignored.
    #[serde(default)]
    pub disabled: bool,
    /// Declarative dropdown items. When `Some`, native menu installers
    /// (macOS `NSMenu` via #184 PR 2; future Win32 / GTK installers)
    /// build the dropdown directly from this list. In-window
    /// rasterisers (TUI / GTK `draw_menu_bar`) ignore the field today;
    /// apps that draw their own dropdown via the `MenuSystem` compose
    /// helper continue to wire that path independently.
    #[serde(default)]
    pub submenu: Option<Vec<crate::primitives::context_menu::ContextMenuItem>>,
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

    /// Like [`Self::layout`], but reserves `leading_width` device units
    /// at the start of `bounds` for a fixed-size leading element — an
    /// app-logo [`crate::Image`] left of the first menu item, VS-Code
    /// style (#662) — before laying out items.
    ///
    /// This exists because the offset math is the actual regression
    /// risk of a leading slot, not the paint: a consumer that narrows
    /// the rect it hands to a paint call but not the rect it hands to a
    /// click-routing call (or vice versa) gets a menu bar whose visible
    /// items and clickable items silently disagree — the same bug class
    /// #552 found in `TabBar`'s hit-x-offset. Centralizing the shift
    /// here means both call sites can pass the *same* `bounds` +
    /// `leading_width` pair instead of each independently computing a
    /// narrowed rect.
    ///
    /// `leading_width <= 0.0` behaves exactly like [`Self::layout`]
    /// called with `bounds` unchanged. [`MenuBarLayout::bounds`] on the
    /// result still covers the *full* `bounds` (including the reserved
    /// leading region) so [`MenuBarLayout::hit_test`]'s "inside the bar"
    /// check keeps treating a click over the icon as `MenuBarHit::Bar`
    /// rather than `MenuBarHit::Outside` — callers that want to
    /// special-case a click on the icon itself compare the click x
    /// against `bounds.x + leading_width` themselves, since the icon's
    /// own geometry isn't a `MenuBar` concern.
    pub fn layout_with_leading<F>(
        &self,
        bounds: Rect,
        leading_width: f32,
        measure_item: F,
    ) -> MenuBarLayout
    where
        F: Fn(usize) -> MenuBarItemMeasure,
    {
        let leading_width = leading_width.max(0.0).min(bounds.width);
        let items_bounds = Rect::new(
            bounds.x + leading_width,
            bounds.y,
            (bounds.width - leading_width).max(0.0),
            bounds.height,
        );
        let mut layout = self.layout(items_bounds, measure_item);
        layout.bounds = bounds;
        layout
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

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(n: usize) -> MenuBar {
        MenuBar {
            id: WidgetId("bar".into()),
            items: (0..n)
                .map(|i| MenuBarItem {
                    id: WidgetId(format!("item{i}")),
                    label: format!("Item{i}"),
                    disabled: false,
                    submenu: None,
                })
                .collect(),
            open_item: None,
            focused_item: None,
        }
    }

    // #662: a leading icon slot (app logo left of the first menu item)
    // must shift every item's x-offset by exactly `leading_width`, and a
    // click that used to land on item 0 pre-shift must land on item 0
    // again post-shift, at its new (shifted) x. This is the regression
    // the module doc on `layout_with_leading` calls out — the paint
    // itself is trivial, keeping paint and hit-test in agreement is not.
    #[test]
    fn leading_icon_shifts_item_x_offsets_by_its_width() {
        let bounds = Rect::new(0.0, 0.0, 200.0, 1.0);
        let measure = |_: usize| MenuBarItemMeasure::new(20.0);
        let leading_width = 32.0;

        let plain = bar(3).layout(bounds, measure);
        let with_icon = bar(3).layout_with_leading(bounds, leading_width, measure);

        assert_eq!(plain.visible_items.len(), with_icon.visible_items.len());
        for (p, w) in plain.visible_items.iter().zip(&with_icon.visible_items) {
            assert_eq!(w.bounds.x, p.bounds.x + leading_width);
            assert_eq!(w.bounds.width, p.bounds.width);
        }
    }

    #[test]
    fn leading_icon_click_on_first_item_still_hits_it_at_its_shifted_x() {
        let bounds = Rect::new(0.0, 0.0, 200.0, 1.0);
        let measure = |_: usize| MenuBarItemMeasure::new(20.0);
        let leading_width = 32.0;

        let layout = bar(3).layout_with_leading(bounds, leading_width, measure);
        let item0 = &layout.visible_items[0];
        assert_eq!(item0.bounds.x, leading_width);

        // Click in the middle of item 0's shifted bounds.
        let hit = layout.hit_test(item0.bounds.x + 1.0, 0.0);
        assert_eq!(hit, MenuBarHit::Item(0));

        // Click over the reserved icon region (left of the shift) is
        // still "inside the bar" — `bounds` covers the full width —
        // but doesn't land on any item.
        let icon_hit = layout.hit_test(leading_width / 2.0, 0.0);
        assert_eq!(icon_hit, MenuBarHit::Bar);
    }

    #[test]
    fn zero_leading_width_matches_plain_layout() {
        let bounds = Rect::new(0.0, 0.0, 200.0, 1.0);
        let measure = |_: usize| MenuBarItemMeasure::new(20.0);

        let plain = bar(3).layout(bounds, measure);
        let zero_leading = bar(3).layout_with_leading(bounds, 0.0, measure);

        assert_eq!(plain, zero_leading);
    }

    #[test]
    fn leading_width_is_clamped_to_bounds_width() {
        let bounds = Rect::new(0.0, 0.0, 50.0, 1.0);
        let measure = |_: usize| MenuBarItemMeasure::new(20.0);

        // Absurdly large leading_width must not push items_bounds.width
        // negative (which would panic or wrap in a naive `width -
        // leading_width` subtraction).
        let layout = bar(2).layout_with_leading(bounds, 10_000.0, measure);
        assert!(layout.visible_items.is_empty());
    }

    // ── MenuBar primitive tests ───────────────────────────────────────

    fn mk_menu_item(id: &str, label: &str) -> MenuBarItem {
        MenuBarItem {
            id: WidgetId::new(id),
            label: label.to_string(),
            disabled: false,
            submenu: None,
        }
    }

    #[test]
    fn menu_bar_layout_flat_items() {
        let bar = MenuBar {
            id: WidgetId::new("mb"),
            items: vec![
                mk_menu_item("file", "&File"),
                mk_menu_item("edit", "&Edit"),
                mk_menu_item("view", "&View"),
            ],
            open_item: None,
            focused_item: None,
        };
        let bounds = Rect::new(0.0, 0.0, 800.0, 20.0);
        let layout = bar.layout(bounds, |_| MenuBarItemMeasure::new(60.0));
        assert_eq!(layout.visible_items.len(), 3);
        assert_eq!(layout.visible_items[0].bounds.x, 0.0);
        assert_eq!(layout.visible_items[1].bounds.x, 60.0);
        assert_eq!(layout.visible_items[2].bounds.x, 120.0);
        // Click on Edit.
        match layout.hit_test(70.0, 10.0) {
            MenuBarHit::Item(1) => {}
            other => panic!("expected Item(1), got {other:?}"),
        }
    }

    #[test]
    fn menu_bar_alt_target_resolution() {
        let bar = MenuBar {
            id: WidgetId::new("mb"),
            items: vec![
                mk_menu_item("file", "&File"),
                mk_menu_item("edit", "&Edit"),
                mk_menu_item("view", "&View"),
            ],
            open_item: None,
            focused_item: None,
        };
        assert_eq!(bar.find_alt_target('f'), Some(0));
        assert_eq!(bar.find_alt_target('E'), Some(1));
        assert_eq!(bar.find_alt_target('v'), Some(2));
        assert_eq!(bar.find_alt_target('x'), None);
    }

    #[test]
    fn menu_bar_disabled_items_not_clickable() {
        let bar = MenuBar {
            id: WidgetId::new("mb"),
            items: vec![
                mk_menu_item("file", "&File"),
                MenuBarItem {
                    id: WidgetId::new("tools"),
                    label: "&Tools".to_string(),
                    disabled: true,
                    submenu: None,
                },
            ],
            open_item: None,
            focused_item: None,
        };
        let bounds = Rect::new(0.0, 0.0, 800.0, 20.0);
        let layout = bar.layout(bounds, |_| MenuBarItemMeasure::new(60.0));
        assert!(!layout.visible_items[1].clickable);
        // Click on the disabled Tools item → Bar (not Item).
        assert_eq!(layout.hit_test(70.0, 10.0), MenuBarHit::Bar);
        // Alt+t skips disabled.
        assert_eq!(bar.find_alt_target('t'), None);
    }

    #[test]
    fn menu_bar_click_outside() {
        let bar = MenuBar {
            id: WidgetId::new("mb"),
            items: vec![mk_menu_item("file", "File")],
            open_item: None,
            focused_item: None,
        };
        let bounds = Rect::new(0.0, 0.0, 200.0, 20.0);
        let layout = bar.layout(bounds, |_| MenuBarItemMeasure::new(50.0));
        assert_eq!(layout.hit_test(100.0, 50.0), MenuBarHit::Outside);
    }
}
