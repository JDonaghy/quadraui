//! `ContextMenu` primitive: a keyboard/mouse-navigable popup of actions
//! triggered at a specific screen location (right-click, keyboard
//! shortcut, explicit open-menu command). Each item is either an
//! action (clickable, emits an id), a separator (visual only), or a
//! submenu parent (opens a nested pull-right popup).
//!
//! Items carry `submenu: Option<Vec<ContextMenuItem>>` for nested menus.
//! The TUI rasteriser renders the `▶` affordance and the pull-right child
//! popup; see [`crate::tui::draw_context_menu`] and
//! [`crate::tui::draw_context_menu_with_submenus`]. The macOS backend
//! wires this into native `NSMenu` nesting.
//!
//! # Backend contract
//!
//! **Modal overlay.** Render as a popup at the computed position;
//! intercept clicks so they don't fall through to the underlying UI.
//! Click on action item activates it (see
//! [`crate::UiEvent::ContextMenuItemActivated`] for the native-menu
//! path via [`crate::Backend::show_context_menu`]); click outside
//! dismisses. Keyboard up/down moves `selected_idx` (skipping
//! separators); Enter activates the selected item; Escape dismisses
//! (see [`crate::UiEvent::ContextMenuDismissed`]).

use crate::accelerator::Accelerator;
use crate::event::Rect;
use crate::types::{Color, StyledText, WidgetId};
use serde::{Deserialize, Serialize};

/// Shared menu-row type, used by both [`ContextMenu`] and (via
/// [`crate::MenuBarItem::submenu`]) the [`crate::MenuBar`] dropdown.
///
/// Currently a type alias to [`ContextMenuItem`]. New code should
/// prefer this name — it reflects that the same shape backs every
/// menu (right-click context menus, menu-bar dropdowns, NSMenu /
/// future Win32 menu installers). Existing `ContextMenuItem` usages
/// continue to compile unchanged.
pub type MenuItem = ContextMenuItem;

/// Declarative description of a context menu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextMenu {
    pub id: WidgetId,
    pub items: Vec<ContextMenuItem>,
    /// Index of the keyboard-selected item. Use [`Self::move_selection`]
    /// and [`Self::first_selectable`] to navigate — they skip separators
    /// and disabled items automatically.
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

/// One entry in a `ContextMenu`. Also re-exported as
/// [`MenuItem`] for use as the shared shape across menu-bar
/// dropdowns and right-click menus.
///
/// Construct items via the struct literal with `..Default::default()`
/// — only the fields you care about need to be set, the rest
/// (`detail`, `key_equivalent`, `checked`, `submenu`) default to
/// `None`, `disabled` to `false`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextMenuItem {
    /// `None` = separator (non-interactive); `Some(id)` = action.
    #[serde(default)]
    pub id: Option<WidgetId>,
    /// Label for the item. Ignored for separators.
    pub label: StyledText,
    /// Optional right-aligned detail (e.g. keyboard shortcut "Ctrl+C").
    /// Prefer [`Self::key_equivalent`] for new code — rasterisers
    /// will render it via `render_accelerator` so the format matches
    /// the host platform (`⌘S` on macOS, `Ctrl+S` elsewhere). `detail`
    /// continues to win when both are set, for back-compat.
    #[serde(default)]
    pub detail: Option<StyledText>,
    /// When true, the item is rendered dimmed and click emits no event.
    #[serde(default)]
    pub disabled: bool,
    /// Structured keyboard shortcut for the item. When `Some` and
    /// [`Self::detail`] is `None`, rasterisers render
    /// `render_accelerator(acc, platform)` as the right-aligned
    /// shortcut hint. The macOS NSMenu installer (#184 PR 2) wires
    /// this directly to `NSMenuItem.keyEquivalent` + the modifier
    /// mask so OS-level shortcut dispatch works.
    #[serde(default)]
    pub key_equivalent: Option<Accelerator>,
    /// Checkbox / radio state. `None` = item has no state; `Some(true)`
    /// renders a leading `✓` and (on macOS) sets `NSMenuItem.state =
    /// .on`; `Some(false)` reserves the prefix space without filling it
    /// so a column of items aligns visually.
    #[serde(default)]
    pub checked: Option<bool>,
    /// Nested submenu. When `Some`, activating the item opens the child
    /// menu instead of firing an action. The TUI rasteriser renders a `▶`
    /// affordance and a pull-right popup (see
    /// [`crate::tui::draw_context_menu_with_submenus`]). The macOS NSMenu
    /// installer wires this as a real nested `NSMenu`. GTK is a follow-up.
    #[serde(default)]
    pub submenu: Option<Vec<ContextMenuItem>>,
}

impl ContextMenuItem {
    /// Convenience: is this item a separator (non-clickable)?
    pub fn is_separator(&self) -> bool {
        self.id.is_none()
    }
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

    /// Index of the first non-separator, non-disabled item.
    /// Returns 0 if no selectable items exist.
    pub fn first_selectable(&self) -> usize {
        self.items
            .iter()
            .position(|item| !item.is_separator() && !item.disabled)
            .unwrap_or(0)
    }

    /// Navigate selection by `delta` steps (positive = down, negative = up),
    /// skipping separators and disabled items, wrapping at boundaries.
    /// Returns the new selected index, or `current` if no selectable item
    /// exists.
    pub fn move_selection(&self, current: usize, delta: i32) -> usize {
        if self.items.is_empty() {
            return current;
        }
        let n = self.items.len() as i32;
        let mut idx = current as i32;
        for _ in 0..self.items.len() {
            idx = (idx + delta).rem_euclid(n);
            let item = &self.items[idx as usize];
            if !item.is_separator() && !item.disabled {
                return idx as usize;
            }
        }
        current
    }

    /// Derive a menu width (in the same units as `char_width` — TUI
    /// char cells, or pixels on a pixel backend) from each item's label
    /// length — no real text measurement available (quadraui#419). Same
    /// char-cell-approximation rationale as
    /// `crate::primitives::dialog::Dialog::measure_generic`.
    ///
    /// Per item, counts: a 2-char prefix slot when `checked` is
    /// `Some(_)` (mirrors the `"✓ "` / `"  "` prefix both
    /// `tui::context_menu::draw_context_menu` and this module's
    /// `native_surface_paint::paint` reserve); the label text; and,
    /// when present, a 1-char gap plus either a 1-char `▶` submenu
    /// arrow or [`ContextMenuItem::detail`]'s length — whichever
    /// trailing content the item carries. [`ContextMenuItem::key_equivalent`]-rendered
    /// shortcuts need a `crate::accelerator::Platform` this helper
    /// doesn't take, so they're not counted; callers that rely on
    /// `key_equivalent` alone should populate `detail` too, or pad the
    /// result. Two char-widths of interior left/right margin are added
    /// on top of the widest item.
    ///
    /// `border_chrome_inset` optionally shrinks the result to leave
    /// room for a border the rasteriser draws *outside* the bounds this
    /// width feeds into [`Self::layout`]/[`Self::layout_at`] — see
    /// `Dialog::measure_generic`'s doc for the same parameter.
    /// `tui::context_menu::draw_context_menu` is exactly this case: its
    /// box-drawing border lives one char cell outside
    /// [`ContextMenuLayout::bounds`] on every side, so a TUI caller
    /// should pass one `char_width` here.
    pub fn measure_generic_width(&self, char_width: f32, border_chrome_inset: f32) -> f32 {
        let max_chars = self
            .items
            .iter()
            .map(Self::item_content_chars)
            .max()
            .unwrap_or(0);
        let content_w = (max_chars as f32 + 2.0) * char_width;
        (content_w - border_chrome_inset * 2.0).max(char_width)
    }

    /// Char-cell width of one item's content: optional `checked` prefix,
    /// label, and optional trailing submenu arrow / detail text. See
    /// [`Self::measure_generic_width`]'s doc for the exact accounting.
    fn item_content_chars(item: &ContextMenuItem) -> usize {
        let prefix_len = if item.checked.is_some() { 2 } else { 0 };
        let label_len: usize = item
            .label
            .spans
            .iter()
            .map(|s| s.text.chars().count())
            .sum();
        let trailing_len = if item.submenu.is_some() {
            1
        } else if let Some(det) = &item.detail {
            det.spans.iter().map(|s| s.text.chars().count()).sum()
        } else {
            0
        };
        let gap = if trailing_len > 0 { 1 } else { 0 };
        prefix_len + label_len + gap + trailing_len
    }
}

// ── PaintSurface Phase 4 slice 4/8 (#1077) ─────────────────────────────────
//
// `paint` below is the one shared paint implementation, written against
// [`crate::paint_surface::PaintSurface`] instead of any one backend's
// API — see `crate::primitives::palette::native_surface_paint` for the
// same pattern applied one primitive earlier (#1076, slice 3/8).
//
// Pre-migration, `gtk::context_menu`, `macos::context_menu` and
// `win::context_menu` were near-identical (all three: bg fill, pass 1
// separators + selection highlight, pass 2 label + right-aligned
// shortcut text, border stroke on top) with two small drifts, resolved
// here rather than silently picked:
//
// - **Box corner rounding.** GTK alone drew a 3px-radius rounded
//   rectangle (both the bg fill and the border stroke) via
//   `rounded_rect_path`; macOS/Windows both used a plain square rect.
//   `paint` adopts the majority (square) — [`PaintSurface`] has a
//   [`crate::paint_surface::PaintSurface::surface_fill_rounded_rect`]
//   verb but no rounded-stroke equivalent, so keeping GTK's rounding
//   would mean a rounded fill under a square border stroke, a visible
//   seam at the corners worse than just picking one. Low real-world
//   impact (3px radius on a small chrome popup).
// - **Separator stroke weight.** GTK stroked at `0.5` width; macOS
//   filled a `1.0`-tall rect; Windows drew a `1.0`-width line. `paint`
//   uses [`PaintSurface::surface_draw_line`] at `1.0` — the 2-of-3
//   majority.
//
// Every other pixel (bg colour, selection highlight inset, text
// baseline centring, `checked` prefix, shortcut right-alignment) was
// already byte-identical across all three backends, so nothing else
// changes.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{ContextMenu, ContextMenuItem, ContextMenuLayout};
    use crate::accelerator::{render_accelerator, Platform};
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;
    use crate::types::WidgetId;
    use crate::{Point, Rect};

    /// Right-aligned shortcut text — sourced from `item.detail`
    /// (preferred, back-compat) or rendered from `item.key_equivalent`
    /// for `platform` (`⌘S` on macOS, `Ctrl+S` elsewhere). Returns
    /// `None` if neither is set.
    pub(super) fn shortcut_text(item: &ContextMenuItem, platform: Platform) -> Option<String> {
        if let Some(ref det) = item.detail {
            return Some(det.spans.iter().map(|sp| sp.text.as_str()).collect());
        }
        item.key_equivalent
            .as_ref()
            .map(|acc| render_accelerator(acc, platform))
    }

    /// Paint a [`ContextMenu`] popup at its caller-resolved
    /// `menu_layout` onto `surface`. Returns the per-clickable-item hit
    /// rectangles + their [`WidgetId`]s so the caller's click handler
    /// can resolve mouse events without re-running layout — mirrors
    /// [`crate::Backend::draw_context_menu`]'s own return contract.
    ///
    /// `platform` selects the key-equivalent render form (`⌘S` vs.
    /// `Ctrl+S`) — see [`shortcut_text`].
    pub(crate) fn paint(
        menu: &ContextMenu,
        menu_layout: &ContextMenuLayout,
        platform: Platform,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
    ) -> Vec<(Rect, WidgetId)> {
        let bounds = menu_layout.bounds;
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return Vec::new();
        }

        let bg = menu.bg.unwrap_or(theme.hover_bg);
        surface.surface_fill_rect(bounds, bg);

        let mut rects: Vec<(Rect, WidgetId)> = Vec::new();

        // Pass 1: backgrounds (separators + selection highlights).
        // Drawn first so no highlight can overwrite previously
        // rendered text.
        for vis in &menu_layout.visible_items {
            let row = vis.bounds;

            if vis.is_separator {
                let sep_y = row.y + row.height * 0.5;
                surface.surface_draw_line(
                    Point::new(row.x + 4.0, sep_y),
                    Point::new(row.x + row.width - 4.0, sep_y),
                    theme.muted_fg,
                    1.0,
                );
                continue;
            }

            let is_selected = vis.item_idx == menu.selected_idx && vis.clickable;
            if is_selected {
                let sel_rect =
                    Rect::new(row.x + 1.0, row.y, (row.width - 2.0).max(0.0), row.height);
                surface.surface_fill_rect(sel_rect, theme.selected_bg);
            }

            if vis.clickable {
                if let Some(ref id) = menu.items[vis.item_idx].id {
                    rects.push((row, id.clone()));
                }
            }
        }

        // Pass 2: text (labels + detail/shortcut). Rendered on top of
        // all backgrounds so descenders are never clipped.
        for vis in &menu_layout.visible_items {
            if vis.is_separator {
                continue;
            }

            let item = &menu.items[vis.item_idx];
            let row = vis.bounds;

            // Prefix the label with a check glyph when `checked` is
            // set. `Some(false)` reserves the slot with spaces so a
            // column of mixed checked/unchecked items aligns.
            let prefix = match item.checked {
                Some(true) => "\u{2713} ",
                Some(false) => "  ",
                None => "",
            };
            let label_text: String = std::iter::once(prefix.to_string())
                .chain(item.label.spans.iter().map(|s| s.text.clone()))
                .collect();
            let label_fg = if vis.clickable {
                theme.foreground
            } else {
                theme.muted_fg
            };
            let (_, lh) = surface.surface_measure_text(&label_text);
            let text_y = row.y + (row.height - lh) / 2.0;
            surface.surface_draw_text_run(
                Rect::new(row.x + 8.0, text_y, (row.width - 8.0).max(0.0), lh),
                &label_text,
                label_fg,
            );

            if item.submenu.is_some() {
                // Submenu-parent: show a `▶` pull-right affordance at the
                // far-right column instead of a keyboard-shortcut hint —
                // mirrors `crate::tui::draw_context_menu` (#370). Submenu
                // parents open a child menu rather than dispatching an
                // action, so a shortcut hint would never fire anyway.
                const SUBMENU_ARROW: &str = "\u{25b6}";
                let (aw, ah) = surface.surface_measure_text(SUBMENU_ARROW);
                surface.surface_draw_text_run(
                    Rect::new(row.x + row.width - aw - 8.0, text_y, aw.max(1.0), ah),
                    SUBMENU_ARROW,
                    label_fg,
                );
            } else if let Some(shortcut) = shortcut_text(item, platform) {
                if !shortcut.is_empty() {
                    let (sw, sh) = surface.surface_measure_text(&shortcut);
                    surface.surface_draw_text_run(
                        Rect::new(row.x + row.width - sw - 8.0, text_y, sw.max(1.0), sh),
                        &shortcut,
                        theme.muted_fg,
                    );
                }
            }
        }

        // Pass 3: border on top so selection bg never obscures edges.
        surface.surface_stroke_rect(bounds, theme.hover_border, 1.0);

        rects
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{StyledText, WidgetId};

    fn action(id: &str) -> ContextMenuItem {
        ContextMenuItem {
            id: Some(WidgetId::new(id)),
            label: StyledText::plain(id),
            ..Default::default()
        }
    }

    fn separator() -> ContextMenuItem {
        ContextMenuItem::default()
    }

    fn disabled(id: &str) -> ContextMenuItem {
        ContextMenuItem {
            id: Some(WidgetId::new(id)),
            label: StyledText::plain(id),
            disabled: true,
            ..Default::default()
        }
    }

    fn menu(items: Vec<ContextMenuItem>) -> ContextMenu {
        ContextMenu {
            id: WidgetId::new("test-menu"),
            items,
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::Below,
        }
    }

    #[test]
    fn first_selectable_skips_separator_and_disabled() {
        let m = menu(vec![separator(), disabled("x"), action("a"), action("b")]);
        assert_eq!(m.first_selectable(), 2);
    }

    #[test]
    fn first_selectable_returns_zero_when_all_inert() {
        let m = menu(vec![separator(), disabled("x")]);
        assert_eq!(m.first_selectable(), 0);
    }

    #[test]
    fn first_selectable_first_item() {
        let m = menu(vec![action("a"), separator(), action("b")]);
        assert_eq!(m.first_selectable(), 0);
    }

    #[test]
    fn move_selection_skips_separators() {
        let m = menu(vec![action("a"), separator(), action("b")]);
        assert_eq!(m.move_selection(0, 1), 2);
        assert_eq!(m.move_selection(2, -1), 0);
    }

    #[test]
    fn move_selection_skips_disabled() {
        let m = menu(vec![action("a"), disabled("x"), action("b")]);
        assert_eq!(m.move_selection(0, 1), 2);
    }

    #[test]
    fn move_selection_wraps_forward() {
        let m = menu(vec![action("a"), separator(), action("b")]);
        assert_eq!(m.move_selection(2, 1), 0);
    }

    #[test]
    fn move_selection_wraps_backward() {
        let m = menu(vec![action("a"), separator(), action("b")]);
        assert_eq!(m.move_selection(0, -1), 2);
    }

    #[test]
    fn move_selection_returns_current_when_no_selectable() {
        let m = menu(vec![separator(), disabled("x")]);
        assert_eq!(m.move_selection(0, 1), 0);
    }

    #[test]
    fn move_selection_empty_menu() {
        let m = menu(vec![]);
        assert_eq!(m.move_selection(0, 1), 0);
    }

    // ── #184 PR 1: shape upgrade ─────────────────────────────────────────

    #[test]
    fn deserialises_old_shape_without_new_fields() {
        // Existing on-disk JSON (e.g. plugin manifests) predates the
        // key_equivalent / checked / submenu fields. Verify it still
        // parses, with the new fields defaulting to None.
        let json = r#"{
            "id": "copy",
            "label": {"spans":[{"text":"Copy","fg":null,"bg":null,"bold":false,"italic":false,"underline":false}]},
            "detail": null,
            "disabled": false
        }"#;
        let item: ContextMenuItem = serde_json::from_str(json).expect("old shape parses");
        assert_eq!(item.id, Some(WidgetId::new("copy")));
        assert!(item.key_equivalent.is_none());
        assert!(item.checked.is_none());
        assert!(item.submenu.is_none());
    }

    #[test]
    fn menu_item_type_alias_compiles() {
        // `MenuItem` is a type alias for `ContextMenuItem`. Any
        // ContextMenuItem assigns straight to a MenuItem binding —
        // this test mostly exists to lock in the alias as a
        // compile-time contract.
        let item: super::MenuItem = action("save");
        assert_eq!(item.id, Some(WidgetId::new("save")));
    }

    #[test]
    fn nested_submenu_round_trips_through_serde() {
        let json = r#"{
            "id": "view",
            "label": {"spans":[{"text":"View","fg":null,"bg":null,"bold":false,"italic":false,"underline":false}]},
            "submenu": [
                {
                    "id": "toggle_sidebar",
                    "label": {"spans":[{"text":"Toggle Sidebar","fg":null,"bg":null,"bold":false,"italic":false,"underline":false}]},
                    "checked": true
                }
            ]
        }"#;
        let item: ContextMenuItem = serde_json::from_str(json).expect("nested shape parses");
        let nested = item.submenu.as_ref().expect("submenu present");
        assert_eq!(nested.len(), 1);
        assert_eq!(nested[0].checked, Some(true));
    }

    // ── ContextMenu primitive tests (D6) ──────────────────────────────

    fn cm_action(id: &str, label: &str) -> ContextMenuItem {
        ContextMenuItem {
            id: Some(WidgetId::new(id)),
            label: StyledText::plain(label),
            ..Default::default()
        }
    }

    fn cm_separator() -> ContextMenuItem {
        ContextMenuItem::default()
    }

    #[test]
    fn context_menu_layout_flat() {
        let menu = ContextMenu {
            id: WidgetId::new("m"),
            items: vec![
                cm_action("cut", "Cut"),
                cm_action("copy", "Copy"),
                cm_separator(),
                cm_action("paste", "Paste"),
            ],
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::default(),
        };
        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
        let layout = menu.layout(100.0, 100.0, viewport, 160.0, |_| {
            ContextMenuItemMeasure::new(20.0)
        });
        assert_eq!(layout.bounds.x, 100.0);
        assert_eq!(layout.bounds.y, 100.0);
        assert_eq!(layout.bounds.width, 160.0);
        assert_eq!(layout.bounds.height, 80.0); // 4 × 20
        assert_eq!(layout.visible_items.len(), 4);
        // Separator at index 2 is visually present, non-clickable.
        assert!(layout.visible_items[2].is_separator);
        assert!(!layout.visible_items[2].clickable);
        // Hit-test on Copy (2nd item, y=120..140).
        match layout.hit_test(120.0, 125.0) {
            ContextMenuHit::Item(id) => assert_eq!(id.as_str(), "copy"),
            _ => panic!("expected Item(copy)"),
        }
        // Hit-test on separator (y=140..160) → Inert.
        assert_eq!(layout.hit_test(120.0, 150.0), ContextMenuHit::Inert);
        // Hit-test far outside → Empty.
        assert_eq!(layout.hit_test(500.0, 500.0), ContextMenuHit::Empty);
    }

    #[test]
    fn context_menu_layout_shifts_left_when_overflow() {
        let menu = ContextMenu {
            id: WidgetId::new("m"),
            items: vec![cm_action("a", "A")],
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::default(),
        };
        let viewport = Rect::new(0.0, 0.0, 200.0, 200.0);
        // Anchor at x=180, menu_width=100 → right edge would be 280 > 200.
        let layout = menu.layout(180.0, 50.0, viewport, 100.0, |_| {
            ContextMenuItemMeasure::new(20.0)
        });
        assert_eq!(layout.bounds.x, 100.0); // 200 - 100 = 100
    }

    #[test]
    fn context_menu_layout_shifts_up_when_overflow() {
        let menu = ContextMenu {
            id: WidgetId::new("m"),
            items: vec![cm_action("a", "A"), cm_action("b", "B")],
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::default(),
        };
        let viewport = Rect::new(0.0, 0.0, 200.0, 100.0);
        // Anchor at y=80, 2 items × 20 = 40, bottom would be 120 > 100.
        let layout = menu.layout(10.0, 80.0, viewport, 100.0, |_| {
            ContextMenuItemMeasure::new(20.0)
        });
        assert_eq!(layout.bounds.y, 60.0); // 100 - 40
    }

    #[test]
    fn context_menu_layout_below_places_at_anchor_bottom() {
        // Trigger button is anchor (10, 10, 80, 20); menu opens below.
        let menu = ContextMenu {
            id: WidgetId::new("m"),
            items: vec![cm_action("a", "A"), cm_action("b", "B")],
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::Below,
        };
        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
        let layout = menu.layout_at(Rect::new(10.0, 10.0, 80.0, 20.0), viewport, 100.0, |_| {
            ContextMenuItemMeasure::new(20.0)
        });
        // Menu's y starts at anchor.bottom = 30.
        assert_eq!(layout.bounds.y, 30.0);
        assert_eq!(
            layout.resolved_placement,
            ResolvedContextMenuPlacement::Below
        );
    }

    #[test]
    fn context_menu_layout_below_flips_above_when_no_room() {
        // Trigger near bottom edge; below would overflow; more room above.
        let menu = ContextMenu {
            id: WidgetId::new("m"),
            items: (0..6).map(|i| cm_action(&format!("i{i}"), "X")).collect(),
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::Below,
        };
        let viewport = Rect::new(0.0, 0.0, 200.0, 200.0);
        // Trigger at y=180, height=20 → bottom=200. Menu height = 6×20 = 120.
        // Space below = 0, space above = 180 → flip.
        let layout = menu.layout_at(Rect::new(10.0, 180.0, 80.0, 20.0), viewport, 100.0, |_| {
            ContextMenuItemMeasure::new(20.0)
        });
        assert_eq!(
            layout.resolved_placement,
            ResolvedContextMenuPlacement::Above
        );
        // Menu's bottom edge sits at the trigger's top edge → y = 180 - 120 = 60.
        assert_eq!(layout.bounds.y, 60.0);
    }

    #[test]
    fn context_menu_layout_above_places_at_anchor_top() {
        // kubeui's status-bar segment use case: trigger at bottom row,
        // menu opens upward.
        let menu = ContextMenu {
            id: WidgetId::new("m"),
            items: vec![
                cm_action("a", "A"),
                cm_action("b", "B"),
                cm_action("c", "C"),
            ],
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::Above,
        };
        let viewport = Rect::new(0.0, 0.0, 200.0, 100.0);
        // Trigger at y=99 (last row, status bar). Menu height = 60.
        // Space above = 99 (room for menu); resolves to Above.
        let layout = menu.layout_at(Rect::new(10.0, 99.0, 80.0, 1.0), viewport, 100.0, |_| {
            ContextMenuItemMeasure::new(20.0)
        });
        assert_eq!(
            layout.resolved_placement,
            ResolvedContextMenuPlacement::Above
        );
        // Menu y = 99 - 60 = 39.
        assert_eq!(layout.bounds.y, 39.0);
    }

    #[test]
    fn context_menu_layout_disabled_items_inert() {
        let mut menu = ContextMenu {
            id: WidgetId::new("m"),
            items: vec![cm_action("delete", "Delete")],
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::default(),
        };
        menu.items[0].disabled = true;
        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
        let layout = menu.layout(10.0, 10.0, viewport, 100.0, |_| {
            ContextMenuItemMeasure::new(20.0)
        });
        assert!(!layout.visible_items[0].clickable);
        // Click on disabled item → Inert, not Item.
        assert_eq!(layout.hit_test(50.0, 15.0), ContextMenuHit::Inert);
    }

    // ── native_surface_paint::shortcut_text (#1077) ───────────────────────

    #[cfg(any(
        feature = "gtk",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    #[test]
    fn shortcut_text_detail_wins_over_key_equivalent() {
        use crate::accelerator::{Accelerator, AcceleratorId, AcceleratorScope, KeyBinding};
        let item = ContextMenuItem {
            id: Some(WidgetId::new("save")),
            label: StyledText::plain("Save"),
            detail: Some(StyledText::plain("legacy-string")),
            key_equivalent: Some(Accelerator {
                id: AcceleratorId::new("editor.save"),
                binding: KeyBinding::Save,
                scope: AcceleratorScope::Global,
                label: None,
            }),
            ..Default::default()
        };
        let shortcut =
            super::native_surface_paint::shortcut_text(&item, crate::accelerator::Platform::Linux)
                .expect("detail wins");
        assert_eq!(shortcut, "legacy-string");
    }

    #[cfg(any(
        feature = "gtk",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    #[test]
    fn shortcut_text_falls_back_to_key_equivalent_per_platform() {
        use crate::accelerator::{Accelerator, AcceleratorId, AcceleratorScope, KeyBinding};
        let item = ContextMenuItem {
            id: Some(WidgetId::new("save")),
            label: StyledText::plain("Save"),
            key_equivalent: Some(Accelerator {
                id: AcceleratorId::new("editor.save"),
                binding: KeyBinding::Save,
                scope: AcceleratorScope::Global,
                label: None,
            }),
            ..Default::default()
        };
        let shortcut =
            super::native_surface_paint::shortcut_text(&item, crate::accelerator::Platform::Macos)
                .expect("key_equivalent produces a string");
        assert!(
            shortcut.contains('⌘'),
            "macOS shortcut should contain ⌘, got {shortcut:?}",
        );
    }

    // ── ContextMenu::measure_generic_width (quadraui#419) ─────────────────

    #[test]
    fn measure_generic_width_widens_for_longer_label() {
        let short = menu(vec![cm_action("a", "Cut")]);
        let long = menu(vec![cm_action("a", "Toggle Sidebar Visibility")]);
        let short_w = short.measure_generic_width(1.0, 0.0);
        let long_w = long.measure_generic_width(1.0, 0.0);
        assert!(
            long_w > short_w,
            "longer label should produce a wider menu: {long_w} vs {short_w}"
        );
    }

    #[test]
    fn measure_generic_width_scales_with_char_width() {
        let m = menu(vec![cm_action("a", "Copy")]);
        let w1 = m.measure_generic_width(1.0, 0.0);
        let w2 = m.measure_generic_width(2.0, 0.0);
        assert_eq!(w2, w1 * 2.0);
    }

    #[test]
    fn measure_generic_width_checked_item_reserves_prefix_space() {
        let unchecked = menu(vec![cm_action("a", "Wrap")]);
        let mut checked_items = vec![cm_action("a", "Wrap")];
        checked_items[0].checked = Some(true);
        let checked = menu(checked_items);
        assert!(
            checked.measure_generic_width(1.0, 0.0) > unchecked.measure_generic_width(1.0, 0.0),
            "checked item reserves extra prefix width"
        );
    }

    #[test]
    fn measure_generic_width_submenu_arrow_reserves_trailing_space() {
        let plain = menu(vec![cm_action("a", "View")]);
        let mut submenu_items = vec![cm_action("a", "View")];
        submenu_items[0].submenu = Some(vec![cm_action("b", "Toggle Sidebar")]);
        let with_submenu = menu(submenu_items);
        assert!(
            with_submenu.measure_generic_width(1.0, 0.0) > plain.measure_generic_width(1.0, 0.0),
            "submenu arrow reserves extra trailing width"
        );
    }

    #[test]
    fn measure_generic_width_detail_reserves_trailing_space() {
        let plain = menu(vec![cm_action("a", "Save")]);
        let mut detail_items = vec![cm_action("a", "Save")];
        detail_items[0].detail = Some(StyledText::plain("Ctrl+S"));
        let with_detail = menu(detail_items);
        assert!(
            with_detail.measure_generic_width(1.0, 0.0) > plain.measure_generic_width(1.0, 0.0),
            "detail text reserves extra trailing width"
        );
    }

    #[test]
    fn measure_generic_width_border_chrome_inset_shrinks_result() {
        let m = menu(vec![cm_action("a", "Copy")]);
        let no_inset = m.measure_generic_width(1.0, 0.0);
        let with_inset = m.measure_generic_width(1.0, 1.0);
        assert_eq!(with_inset, no_inset - 2.0);
    }

    #[test]
    fn measure_generic_width_never_shrinks_below_one_char() {
        let m = menu(vec![cm_action("a", "X")]);
        let w = m.measure_generic_width(1.0, 1000.0);
        assert_eq!(w, 1.0, "clamped to at least one char_width");
    }

    #[test]
    fn measure_generic_width_empty_menu() {
        let m = menu(vec![]);
        // Only the 2-char interior margin, no item content.
        assert_eq!(m.measure_generic_width(1.0, 0.0), 2.0);
    }
}
