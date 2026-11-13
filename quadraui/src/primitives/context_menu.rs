//! `ContextMenu` primitive: a keyboard/mouse-navigable popup of actions
//! triggered at a specific screen location (right-click, keyboard
//! shortcut, explicit open-menu command). Each item is either an
//! action (clickable, emits an id), a separator (visual only), or a
//! submenu parent (opens a nested pull-right popup).
//!
//! Items carry `submenu: Option<Vec<ContextMenuItem>>` for nested menus.
//! The TUI rasteriser renders the `▶` affordance and the pull-right child
//! popup; see `crate::tui::draw_context_menu` and
//! `crate::tui::draw_context_menu_with_submenus` (both `#[doc(hidden)]`,
//! so not linked here). The macOS backend wires this into native
//! `NSMenu` nesting.
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
use crate::event::{Point, Rect};
use crate::layout::{Anchor, ResolvedSide, Side};
use crate::types::{Color, Icon, StyledText, WidgetId};
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
///
/// # Examples
///
/// ```
/// use quadraui::{ContextMenu, ContextMenuHit, ContextMenuItem, ContextMenuItemMeasure, Rect, StyledText, WidgetId};
///
/// let menu = ContextMenu {
///     id: WidgetId::new("context_menu:editor"),
///     items: vec![ContextMenuItem {
///         id: Some(WidgetId::new("menu:copy")),
///         label: StyledText::plain("Copy"),
///         ..Default::default()
///     }],
///     selected_idx: 0,
///     bg: None,
///     placement: Default::default(),
/// };
///
/// let viewport = Rect::new(0.0, 0.0, 80.0, 24.0);
/// let layout = menu.layout(5.0, 5.0, viewport, 20.0, |_| ContextMenuItemMeasure::new(1.0));
/// assert_eq!(
///     layout.hit_test(6.0, 5.0),
///     ContextMenuHit::Item(WidgetId::new("menu:copy"))
/// );
/// ```
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

/// Optional app-supplied override for the glyphs [`native_surface_paint`]
/// paints in place of the default codicon check mark and submenu
/// pull-right arrow.
///
/// A separate, `#[non_exhaustive]` type rather than fields on
/// [`ContextMenu`]/[`ContextMenuItem`] — those two stay exhaustive for
/// downstream struct literals, so a glyph slot can only ever be added
/// here, where a new field can never break an existing construction
/// site. Passed as an extra argument to
/// [`native_surface_paint::paint_with_glyphs`]; the plain
/// [`native_surface_paint::paint`] every existing backend call site
/// already uses keeps calling it with `None`, so today's codicon
/// glyphs render byte-for-byte unchanged until an app opts in.
///
/// Each slot is an [`Icon`] — `glyph` for when
/// [`crate::Backend::nerd_fonts_enabled`] is set, `fallback` otherwise
/// — rather than a bare string. A bare `&str`/`String` still works via
/// `Icon`'s blanket `From` impls.
///
/// The TUI rasteriser paints a single cell per glyph: it resolves the
/// `Icon` to a `&str` and takes only its first `char`, so a
/// multi-character override silently degrades to its leading character.
/// `Icon::color` is resolved but not painted on TUI — the glyph cell
/// always uses the surrounding theme foreground, never the `Icon`'s own
/// colour.
///
/// `crate::tui::draw_context_menu_with_submenus` (`#[doc(hidden)]`, so not
/// linked here) does not take a `glyphs` argument and always paints the
/// default submenu-arrow glyph; only
/// [`native_surface_paint::paint_with_glyphs`] honours this override.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ContextMenuGlyphs {
    checked: Option<Icon>,
    submenu_arrow: Option<Icon>,
}

impl ContextMenuGlyphs {
    /// An empty override set — [`native_surface_paint::paint_with_glyphs`]
    /// painted with `Some(&Self::new())` is byte-identical to
    /// [`native_surface_paint::paint`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the checked-item check-mark glyph (default: the codicon
    /// check glyph on GUI).
    #[must_use]
    pub fn with_checked_glyph(mut self, glyph: impl Into<Icon>) -> Self {
        self.checked = Some(glyph.into());
        self
    }

    /// Override the submenu-parent pull-right arrow glyph (default: the
    /// codicon chevron-right glyph on GUI).
    #[must_use]
    pub fn with_submenu_arrow_glyph(mut self, glyph: impl Into<Icon>) -> Self {
        self.submenu_arrow = Some(glyph.into());
        self
    }

    /// The overridden check-mark glyph, if set.
    #[must_use]
    pub fn checked_glyph(&self) -> Option<&Icon> {
        self.checked.as_ref()
    }

    /// The overridden submenu-arrow glyph, if set.
    #[must_use]
    pub fn submenu_arrow_glyph(&self) -> Option<&Icon> {
        self.submenu_arrow.as_ref()
    }
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
///
/// When the menu's height fits on neither `Below` nor `Above`, this
/// reports the *preferred* side rather than whichever side the box was
/// actually pinned against — the box is still clamped fully inside the
/// viewport, just not necessarily on the side this value names.
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
/// (`detail`, `key_equivalent`, `checked`, `icon`, `submenu`) default to
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
    /// shortcut hint.
    #[serde(default)]
    pub key_equivalent: Option<Accelerator>,
    /// Checkbox / radio state. `None` = item has no state; `Some(true)`
    /// renders a leading `✓` and (on macOS) sets `NSMenuItem.state =
    /// .on`; `Some(false)` reserves the prefix space without filling it
    /// so a column of items aligns visually.
    #[serde(default)]
    pub checked: Option<bool>,
    /// Optional leading icon, painted in the same reserved gutter column
    /// [`Self::checked`]'s `✓` uses. Only shown when `checked` is `None`
    /// — a checkable item's state mark always takes priority over a
    /// decorative icon in that one shared slot, the same way a tree row
    /// never shows both a chevron and an icon in the same cell. Build one
    /// with [`crate::types::Icon::codicon`] for a bundled codicon, or
    /// [`crate::types::Icon::new`] for an app-supplied glyph/fallback
    /// pair.
    ///
    /// Unlike `checked`, `icon` has no `Some(false)`-style reservation
    /// without a fill: the gutter is per-item, not per-menu, so in a menu
    /// where only some items carry an icon, those items' labels shift
    /// right relative to the rest. Give every item in the menu an icon
    /// (or none) to keep labels aligned to one gutter.
    #[serde(default)]
    pub icon: Option<Icon>,
    /// Nested submenu. When `Some`, activating the item opens the child
    /// menu instead of firing an action. The TUI rasteriser renders a `▶`
    /// affordance and a pull-right popup (see
    /// `crate::tui::draw_context_menu_with_submenus`, `#[doc(hidden)]` so
    /// not linked here). The macOS NSMenu installer wires this as a real
    /// nested `NSMenu`. GTK is a follow-up.
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
    /// Shared positioning rule for a nested submenu opened beside this
    /// (already-resolved) menu's item `item_idx`: pulls right of this
    /// menu's own right edge by one unit, flips to the left of this
    /// menu's own left edge on right-edge overflow, and — when neither
    /// side fits — clamps fully inside `viewport` as a last resort.
    /// `y` aligns with `item_idx`'s own row (or this menu's top edge if
    /// that item isn't in [`Self::visible_items`] — e.g. clipped past
    /// the bottom edge).
    ///
    /// The one shared implementation both [`crate::compose::MenuSystem`]'s
    /// menu-bar submenus and [`crate::compose::ContextMenuController`]'s
    /// right-click submenus position through — a consumer that paints a
    /// nested popup beside a parent item anchors it here instead of
    /// re-deriving the pull-right/flip-left math itself.
    pub fn submenu_anchor(&self, item_idx: usize, submenu_width: f32, viewport: Rect) -> Point {
        let anchor_y = self
            .visible_items
            .iter()
            .find(|v| v.item_idx == item_idx)
            .map(|v| v.bounds.y)
            .unwrap_or(self.bounds.y);
        let preferred_x = self.bounds.x + self.bounds.width + 1.0;
        let flipped_x = self.bounds.x - submenu_width - 1.0;
        let anchor_x = if preferred_x + submenu_width <= viewport.x + viewport.width {
            preferred_x
        } else if flipped_x >= viewport.x {
            flipped_x
        } else {
            (viewport.x + viewport.width - submenu_width).max(viewport.x)
        };
        Point::new(anchor_x, anchor_y)
    }

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

        // Horizontal position is clamped into the viewport *before*
        // `Anchor::resolve` runs, independently of the vertical flip
        // decision below. `Anchor::resolve`'s `fits` predicate tests
        // both axes together, so an anchor `x` that is already outside
        // the viewport would make the horizontal overflow fail the fit
        // test on whichever vertical side is tried, masking the
        // `Below`/`Above` flip. This clamp is the same left-align-then-
        // shift for every placement mode, `AnchorPoint` included —
        // pre-clamping is a no-op for `AnchorPoint` since
        // [`Side::AtPoint`] clamps `rect.x` again on its own.
        let ax = anchor.x.clamp(
            viewport.x,
            (viewport.x + viewport.width - menu_width).max(viewport.x),
        );

        // Placement, via the shared `Anchor` flip/clamp resolver. The
        // anchor rect's width is set to `menu_width` so `Anchor::resolve`'s
        // cross-axis centering (`(rect.width - w) * 0.5`) collapses to
        // zero and `x` stays left-aligned with `ax` rather than
        // centered. `rect.height` carries `anchor.height` through for
        // the `Below`/`Above` candidates' `r.y + r.height` / `r.y - h`
        // formulas; `AnchorPoint` maps to [`Side::AtPoint`], which
        // ignores both and only clamps `rect.x`/`rect.y` directly,
        // matching this mode's "menu top-left IS the anchor point"
        // contract.
        let side = match self.placement {
            ContextMenuPlacement::AnchorPoint => Side::AtPoint,
            ContextMenuPlacement::Below => Side::Bottom,
            ContextMenuPlacement::Above => Side::Top,
        };
        let anchor_rect = Rect::new(ax, anchor.y, menu_width, anchor.height);
        let (origin, resolved_side) =
            Anchor::new(anchor_rect, side).resolve((menu_width, total_height), viewport);
        let (x, y) = (origin.x, origin.y);
        let resolved_placement = match resolved_side {
            ResolvedSide::AtPoint => ResolvedContextMenuPlacement::AnchorPoint,
            ResolvedSide::Bottom => ResolvedContextMenuPlacement::Below,
            ResolvedSide::Top => ResolvedContextMenuPlacement::Above,
            ResolvedSide::Left | ResolvedSide::Right => {
                unreachable!("ContextMenuPlacement never maps to Side::Left/Right")
            }
        };

        let viewport_bottom = viewport.y + viewport.height;
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
                    hit_regions.push((item_bounds, ContextMenuHit::Item(*id)));
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
    /// length — no real text measurement available. Same
    /// char-cell-approximation rationale as
    /// `crate::primitives::dialog::Dialog::measure_generic`.
    ///
    /// Per item, counts: a 2-char prefix slot when `checked` is
    /// `Some(_)` or [`ContextMenuItem::icon`] is `Some(_)` (mirrors the
    /// `"✓ "` / icon-glyph / `"  "` prefix both
    /// `tui::context_menu::draw_context_menu` and this module's
    /// `native_surface_paint::paint` reserve — the two share one slot,
    /// so the reservation is never doubled when both are set); the label
    /// text; and,
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
        let prefix_len = if item.checked.is_some() || item.icon.is_some() {
            2
        } else {
            0
        };
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

// `paint` below is the one shared paint implementation, written against
// [`crate::paint_surface::PaintSurface`] instead of any one backend's
// API — see `crate::primitives::palette::native_surface_paint` for the
// same pattern applied one primitive earlier.
//
// When migrating `gtk::context_menu`, `macos::context_menu` and
// `win::context_menu`, two small differences were resolved:
//
// - **Box corner rounding.** GTK drew 3px-radius rounded rectangles;
//   macOS and Windows used plain square rects. `paint` adopts the
//   majority (square) for consistency.
// - **Separator stroke weight.** GTK stroked at `0.5` width; macOS
//   filled a `1.0`-tall rect; Windows drew a `1.0`-width line. `paint`
//   uses [`PaintSurface::surface_draw_line`] at `1.0` — the majority.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{ContextMenu, ContextMenuGlyphs, ContextMenuItem, ContextMenuLayout};
    use crate::accelerator::{render_accelerator, Platform};
    use crate::interaction::InteractionState;
    use crate::paint_surface::PaintSurface;
    use crate::style::Style;
    use crate::theme::Theme;
    use crate::types::{Color, WidgetId};
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
    /// `Ctrl+S`) — see [`shortcut_text`]. `style` supplies
    /// the corner radius/border width/shadow elevation the popup box
    /// paints with — callers pass `&self.style()`.
    pub(crate) fn paint(
        menu: &ContextMenu,
        menu_layout: &ContextMenuLayout,
        platform: Platform,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        style: &Style,
    ) -> Vec<(Rect, WidgetId)> {
        paint_with_glyphs(
            menu,
            menu_layout,
            platform,
            surface,
            theme,
            style,
            None,
            false,
        )
    }

    /// [`paint`], reading hover/pressed state from a single
    /// [`InteractionState`] keyed by [`WidgetId`]: the item whose
    /// [`ContextMenuItem::id`] matches `interaction.hovered()` /
    /// `interaction.pressed()` paints with
    /// [`Theme::menu_item_hover_bg`] / [`Theme::menu_item_pressed_bg`]
    /// instead of its plain selection/resting fill — see
    /// [`crate::Backend::draw_context_menu_with_state`]'s own doc.
    pub(crate) fn paint_interactive(
        menu: &ContextMenu,
        menu_layout: &ContextMenuLayout,
        platform: Platform,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        style: &Style,
        interaction: &InteractionState,
    ) -> Vec<(Rect, WidgetId)> {
        paint_with_glyphs_interactive(
            menu,
            menu_layout,
            platform,
            surface,
            theme,
            style,
            None,
            false,
            interaction,
        )
    }

    /// Identical to [`paint`], plus an optional [`ContextMenuGlyphs`]
    /// override for the check-mark / submenu-arrow glyphs and the
    /// `nerd_fonts_enabled` flag (see [`crate::Backend::nerd_fonts_enabled`])
    /// that selects each override [`Icon`]'s `glyph` vs. `fallback` form.
    /// `glyphs: None` (what [`paint`] passes) renders byte-for-byte the
    /// same as today's codicon defaults.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint_with_glyphs(
        menu: &ContextMenu,
        menu_layout: &ContextMenuLayout,
        platform: Platform,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        style: &Style,
        glyphs: Option<&ContextMenuGlyphs>,
        nerd_fonts_enabled: bool,
    ) -> Vec<(Rect, WidgetId)> {
        paint_with_glyphs_impl(
            menu,
            menu_layout,
            platform,
            surface,
            theme,
            style,
            glyphs,
            nerd_fonts_enabled,
            None,
        )
    }

    /// [`paint_with_glyphs`] + [`paint_interactive`]'s hover/pressed
    /// treatment combined — the method a backend that wants both glyph
    /// overrides and hover feedback calls.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint_with_glyphs_interactive(
        menu: &ContextMenu,
        menu_layout: &ContextMenuLayout,
        platform: Platform,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        style: &Style,
        glyphs: Option<&ContextMenuGlyphs>,
        nerd_fonts_enabled: bool,
        interaction: &InteractionState,
    ) -> Vec<(Rect, WidgetId)> {
        paint_with_glyphs_impl(
            menu,
            menu_layout,
            platform,
            surface,
            theme,
            style,
            glyphs,
            nerd_fonts_enabled,
            Some(interaction),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_with_glyphs_impl(
        menu: &ContextMenu,
        menu_layout: &ContextMenuLayout,
        platform: Platform,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        style: &Style,
        glyphs: Option<&ContextMenuGlyphs>,
        nerd_fonts_enabled: bool,
        interaction: Option<&InteractionState>,
    ) -> Vec<(Rect, WidgetId)> {
        let bounds = menu_layout.bounds;
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return Vec::new();
        }

        // Resolved once, outside the per-item loop below — these two
        // values are constant across the whole popup, not per-item.
        let check_glyph: std::borrow::Cow<'_, str> = glyphs
            .and_then(ContextMenuGlyphs::checked_glyph)
            .map(|icon| std::borrow::Cow::Borrowed(icon.resolve(nerd_fonts_enabled)))
            .unwrap_or(std::borrow::Cow::Owned(crate::codicon::CHECK.to_string()));
        let submenu_arrow_glyph: std::borrow::Cow<'_, str> = glyphs
            .and_then(ContextMenuGlyphs::submenu_arrow_glyph)
            .map(|icon| std::borrow::Cow::Borrowed(icon.resolve(nerd_fonts_enabled)))
            .unwrap_or(std::borrow::Cow::Owned(
                crate::codicon::CHEVRON_RIGHT.to_string(),
            ));

        let bg = menu.bg.unwrap_or(theme.hover_bg);
        // VS-Code-style elevated card — a shadow behind, a
        // rounded fill, and a border this popup never had before.
        surface.surface_draw_shadow(
            bounds,
            style.corner_radius,
            style.shadow_elevation,
            Color::rgba(0, 0, 0, 100),
        );
        surface.surface_fill_rounded_rect(bounds, style.corner_radius, bg);
        surface.surface_stroke_rounded_rect(
            bounds,
            style.corner_radius,
            theme.border_fg,
            style.border_width,
        );

        let mut rects: Vec<(Rect, WidgetId)> = Vec::new();

        // Pass 1: backgrounds (separators + selection highlights).
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
            let item_id = menu.items[vis.item_idx].id;
            let is_pressed = vis.clickable
                && item_id.is_some_and(|id| interaction.is_some_and(|i| i.is_pressed(&id)));
            let is_hovered = vis.clickable
                && item_id.is_some_and(|id| interaction.is_some_and(|i| i.is_hovered(&id)));
            // `is_selected` (the keyboard-moved cursor) outranks
            // `is_hovered` here: a mouse can hover the very row the
            // keyboard last selected, and `theme.selected_bg` is meant
            // to read as the stronger of the two cues. Were hover
            // checked first, hovering the selected row would downgrade
            // it to the fainter `menu_item_hover_bg`. Pressed still wins
            // over both.
            let row_bg = if is_pressed {
                Some(theme.menu_item_pressed_bg(bg))
            } else if is_selected {
                Some(theme.selected_bg)
            } else if is_hovered {
                Some(theme.menu_item_hover_bg(bg))
            } else {
                None
            };
            if let Some(row_bg) = row_bg {
                let sel_rect =
                    Rect::new(row.x + 1.0, row.y, (row.width - 2.0).max(0.0), row.height);
                surface.surface_fill_rect(sel_rect, row_bg);
            }

            if vis.clickable {
                if let Some(ref id) = menu.items[vis.item_idx].id {
                    rects.push((row, *id));
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

            // Reserve a fixed-width leading slot for the check glyph /
            // item icon so a column of mixed items aligns — `checked ==
            // Some(false)` or an item with neither reserves it with two
            // plain spaces, `checked == Some(true)` or `icon.is_some()`
            // fills it with the codicon check glyph or the item's own
            // icon, painted *separately* via `surface_draw_icon_glyph`
            // (below) rather than embedded in this plain
            // `surface_draw_text_run` call: a PUA codepoint baked into
            // an ordinary text run resolves through whatever font that
            // run's own call site already carries, which is not
            // guaranteed to be the fallback-wrapped description
            // `surface_draw_icon_glyph` resolves through — the same
            // reason the submenu pull-right arrow below gets its own
            // draw call instead of living in this label string. A
            // checkable item's state mark always wins that one slot over
            // a decorative icon — see [`ContextMenuItem::icon`]'s doc.
            let has_check_slot = item.checked.is_some();
            let has_icon_slot = !has_check_slot && item.icon.is_some();
            let label_text: String = if has_check_slot || has_icon_slot {
                std::iter::once("  ".to_string())
                    .chain(item.label.spans.iter().map(|s| s.text.clone()))
                    .collect()
            } else {
                item.label.spans.iter().map(|s| s.text.clone()).collect()
            };
            let label_fg = if vis.clickable {
                theme.foreground
            } else {
                theme.muted_fg
            };
            let (_, lh) = surface.surface_measure_text(&label_text);
            let text_y = row.y + (row.height - lh) / 2.0;
            surface.surface_draw_text_run(
                Rect::new(
                    row.x + style.row_padding_x,
                    text_y,
                    (row.width - style.row_padding_x).max(0.0),
                    lh,
                ),
                &label_text,
                label_fg,
            );

            if item.checked == Some(true) {
                let (cw, ch) = surface.surface_measure_text(&check_glyph);
                surface.surface_draw_icon_glyph(
                    Rect::new(
                        row.x + style.row_padding_x,
                        text_y,
                        cw.max(1.0),
                        ch.max(1.0),
                    ),
                    &check_glyph,
                    label_fg,
                );
            } else if has_icon_slot {
                if let Some(icon) = &item.icon {
                    let glyph = icon.resolve(nerd_fonts_enabled);
                    let (iw, ih) = surface.surface_measure_text(glyph);
                    surface.surface_draw_icon_glyph(
                        Rect::new(row.x + 8.0, text_y, iw.max(1.0), ih.max(1.0)),
                        glyph,
                        label_fg,
                    );
                }
            }

            if item.submenu.is_some() {
                // Submenu-parent: show a codicon chevron pull-right
                // affordance at the far-right column instead of a
                // keyboard-shortcut hint — mirrors the shape
                // `crate::tui::draw_context_menu` paints with a
                // plain `▶` glyph instead. Submenu parents open a child
                // menu rather than dispatching an action, so a shortcut
                // hint would never fire anyway.
                let (aw, ah) = surface.surface_measure_text(&submenu_arrow_glyph);
                surface.surface_draw_icon_glyph(
                    Rect::new(
                        row.x + row.width - aw - style.row_padding_x,
                        text_y,
                        aw.max(1.0),
                        ah,
                    ),
                    &submenu_arrow_glyph,
                    label_fg,
                );
            } else if let Some(shortcut) = shortcut_text(item, platform) {
                if !shortcut.is_empty() {
                    let (sw, sh) = surface.surface_measure_text(&shortcut);
                    surface.surface_draw_text_run(
                        Rect::new(
                            row.x + row.width - sw - style.row_padding_x,
                            text_y,
                            sw.max(1.0),
                            sh,
                        ),
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
    fn context_menu_layout_above_flips_below_when_no_room() {
        // Trigger near the top edge; `Above` has no room, flips to `Below`.
        let menu = ContextMenu {
            id: WidgetId::new("m"),
            items: (0..3).map(|i| cm_action(&format!("i{i}"), "X")).collect(),
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::Above,
        };
        let viewport = Rect::new(0.0, 0.0, 200.0, 200.0);
        let layout = menu.layout_at(Rect::new(10.0, 5.0, 80.0, 1.0), viewport, 100.0, |_| {
            ContextMenuItemMeasure::new(20.0)
        });
        assert_eq!(
            layout.resolved_placement,
            ResolvedContextMenuPlacement::Below
        );
        // Menu's top sits at the trigger's bottom edge → y = 5 + 1 = 6.
        assert_eq!(layout.bounds.y, 6.0);
    }

    #[test]
    fn context_menu_layout_below_pins_to_preferred_when_neither_side_fits() {
        // Trigger near the middle of a short viewport: a 60-tall menu
        // fits neither fully below (40px room) nor fully above (55px
        // room), even though above has more room than below.
        //
        // When neither side fits, the menu pins to the preferred side
        // (`Below`, matching `Tooltip`/`Float`/`Completions`) and
        // clamps — landing pinned to the viewport's bottom edge (y=40,
        // i.e. `bounds.y + bounds.height == 100`).
        let menu = ContextMenu {
            id: WidgetId::new("m"),
            items: (0..3).map(|i| cm_action(&format!("i{i}"), "X")).collect(),
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::Below,
        };
        let viewport = Rect::new(0.0, 0.0, 200.0, 100.0);
        let layout = menu.layout_at(Rect::new(10.0, 55.0, 80.0, 5.0), viewport, 100.0, |_| {
            ContextMenuItemMeasure::new(20.0)
        });
        assert_eq!(
            layout.resolved_placement,
            ResolvedContextMenuPlacement::Below
        );
        assert_eq!(layout.bounds.y, 40.0);
        assert_eq!(layout.bounds.height, 60.0);
    }

    #[test]
    fn context_menu_layout_below_flips_above_when_trigger_overflows_both_axes() {
        // Trigger in the bottom-right corner of the viewport: overflows
        // horizontally (menu_width 100 past anchor.x 180 in a 200-wide
        // viewport) AND the preferred `Below` candidate overflows
        // vertically (trigger near the bottom). The vertical flip to
        // `Above` must still happen — horizontal overflow is clamped
        // independently and must not suppress it.
        let menu = ContextMenu {
            id: WidgetId::new("m"),
            items: vec![cm_action("a", "A"), cm_action("b", "B")],
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::Below,
        };
        let viewport = Rect::new(0.0, 0.0, 200.0, 200.0);
        let layout = menu.layout_at(Rect::new(180.0, 180.0, 20.0, 10.0), viewport, 100.0, |_| {
            ContextMenuItemMeasure::new(20.0)
        });
        assert_eq!(
            layout.resolved_placement,
            ResolvedContextMenuPlacement::Above
        );
        // x clamps to the right edge: 200 - 100 = 100.
        assert_eq!(layout.bounds.x, 100.0);
        // Flips above: y = anchor.y - menu_height = 180 - 40 = 140.
        assert_eq!(layout.bounds.y, 140.0);
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

    // ── ContextMenuLayout::submenu_anchor ─────────────────────────────

    fn anchor_point_menu(items: Vec<ContextMenuItem>) -> ContextMenu {
        ContextMenu {
            id: WidgetId::new("m"),
            items,
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::AnchorPoint,
        }
    }

    #[test]
    fn submenu_anchor_pulls_right_of_parent_by_default() {
        let menu = anchor_point_menu(vec![cm_action("a", "A"), cm_action("b", "B")]);
        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
        let layout = menu.layout(10.0, 10.0, viewport, 100.0, |_| {
            ContextMenuItemMeasure::new(20.0)
        });
        // Submenu for item 1 ("b") pulls right of the parent's right edge
        // (10 + 100 + 1 = 111) and aligns y with item 1's own row
        // (10 + 20 = 30).
        let anchor = layout.submenu_anchor(1, 80.0, viewport);
        assert_eq!(anchor.x, 111.0);
        assert_eq!(anchor.y, 30.0);
    }

    #[test]
    fn submenu_anchor_flips_left_near_right_edge() {
        let menu = anchor_point_menu(vec![cm_action("a", "A")]);
        let viewport = Rect::new(0.0, 0.0, 200.0, 600.0);
        // Parent menu's right edge (150 + 50 = 200) is flush with the
        // viewport's right edge, so a 80-wide submenu pulled right
        // (201 + 80 = 281 > 200) overflows and must flip left instead
        // (150 - 80 - 1 = 69).
        let layout = menu.layout(150.0, 10.0, viewport, 50.0, |_| {
            ContextMenuItemMeasure::new(20.0)
        });
        let anchor = layout.submenu_anchor(0, 80.0, viewport);
        assert_eq!(anchor.x, 69.0);
    }

    #[test]
    fn submenu_anchor_clamps_inside_viewport_when_neither_side_fits() {
        let menu = anchor_point_menu(vec![cm_action("a", "A")]);
        // A viewport too narrow for the 80-wide submenu to fit on either
        // side of a parent flush against the right edge.
        let viewport = Rect::new(0.0, 0.0, 100.0, 600.0);
        let layout = menu.layout(50.0, 10.0, viewport, 50.0, |_| {
            ContextMenuItemMeasure::new(20.0)
        });
        let anchor = layout.submenu_anchor(0, 80.0, viewport);
        // Clamped flush with the viewport's right edge: 100 - 80 = 20.
        assert_eq!(anchor.x, 20.0);
    }

    #[test]
    fn submenu_anchor_falls_back_to_menu_top_when_item_not_visible() {
        // A viewport too short to show item 1 at all (clipped past the
        // bottom edge) — `submenu_anchor` falls back to the menu's own
        // top edge for `y` instead of panicking or guessing.
        let menu = anchor_point_menu(vec![cm_action("a", "A"), cm_action("b", "B")]);
        let viewport = Rect::new(0.0, 0.0, 800.0, 20.0);
        let layout = menu.layout(0.0, 0.0, viewport, 100.0, |_| {
            ContextMenuItemMeasure::new(20.0)
        });
        assert_eq!(layout.visible_items.len(), 1); // item 1 clipped out
        let anchor = layout.submenu_anchor(1, 80.0, viewport);
        assert_eq!(anchor.y, layout.bounds.y);
    }

    // ── native_surface_paint::shortcut_text ───────────────────────────────

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

    // ── ContextMenu::measure_generic_width ────────────────────────────────

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
    fn measure_generic_width_icon_reserves_leading_space() {
        let plain = menu(vec![cm_action("a", "Reveal")]);
        let mut icon_items = vec![cm_action("a", "Reveal")];
        icon_items[0].icon = Some(crate::types::Icon::new("G", "F"));
        let with_icon = menu(icon_items);
        assert!(
            with_icon.measure_generic_width(1.0, 0.0) > plain.measure_generic_width(1.0, 0.0),
            "icon reserves extra leading width"
        );
    }

    #[test]
    fn measure_generic_width_icon_and_checked_do_not_double_reserve() {
        // The icon and the check mark share one leading slot (the icon
        // is only shown when `checked` is `None`) — an item carrying
        // both must not reserve width twice.
        let mut checked_only = vec![cm_action("a", "Reveal")];
        checked_only[0].checked = Some(true);
        let checked_only_menu = menu(checked_only);

        let mut both = vec![cm_action("a", "Reveal")];
        both[0].checked = Some(true);
        both[0].icon = Some(crate::types::Icon::new("G", "F"));
        let both_menu = menu(both);

        assert_eq!(
            both_menu.measure_generic_width(1.0, 0.0),
            checked_only_menu.measure_generic_width(1.0, 0.0),
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

    #[cfg(any(
        feature = "gtk",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    mod glyph_override {
        use super::*;
        use crate::accelerator::Platform;
        use crate::paint_surface::PaintSurface;
        use crate::style::Style;
        use crate::theme::Theme;
        use crate::types::Color;
        use crate::{Point, Rect, Viewport};

        /// Records every painted glyph/text run so a test can assert on
        /// what the rasteriser actually drew, not an internal helper.
        #[derive(Default)]
        struct RecordingSurface {
            runs: Vec<String>,
        }

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(0.0, 0.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                16.0
            }
            fn surface_char_width(&self) -> f32 {
                8.0
            }
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 8.0, 16.0)
            }
            fn surface_fill_rect(&mut self, _rect: Rect, _color: Color) {}
            fn surface_fill_rounded_rect(&mut self, _rect: Rect, _radius: f32, _color: Color) {}
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_stroke_rounded_rect(
                &mut self,
                _rect: Rect,
                _radius: f32,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_draw_text_run(&mut self, _rect: Rect, text: &str, _color: Color) {
                self.runs.push(text.to_string());
            }
            fn surface_draw_line(
                &mut self,
                _from: Point,
                _to: Point,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(
                &mut self,
                _rect: Rect,
                _image: &crate::Image,
            ) -> crate::backend::ImagePaintResult {
                crate::backend::ImagePaintResult::Unsupported
            }
        }

        fn popup_layout(item: ContextMenuItem) -> (ContextMenu, ContextMenuLayout) {
            let m = menu(vec![item]);
            let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
            let layout = m.layout(10.0, 10.0, viewport, 100.0, |_| {
                ContextMenuItemMeasure::new(20.0)
            });
            (m, layout)
        }

        /// With no [`ContextMenuGlyphs`] override, `paint_with_glyphs`
        /// (what `paint` always calls) paints the default codicon
        /// check mark — byte-for-byte today's behaviour.
        #[test]
        fn no_override_paints_default_codicon_check() {
            let mut item = cm_action("a", "Save");
            item.checked = Some(true);
            let (m, layout) = popup_layout(item);
            let mut surface = RecordingSurface::default();
            native_surface_paint::paint_with_glyphs(
                &m,
                &layout,
                Platform::Linux,
                &mut surface,
                &Theme::default(),
                &Style::default(),
                None,
                false,
            );
            let expected = crate::codicon::CHECK.to_string();
            assert!(
                surface.runs.iter().any(|r| r == &expected),
                "expected default codicon check among {:?}",
                surface.runs
            );
        }

        /// An app-supplied [`ContextMenuGlyphs`] override replaces the
        /// check mark with the app's own glyph; the codicon default no
        /// longer appears.
        #[test]
        fn override_replaces_check_glyph() {
            let mut item = cm_action("a", "Save");
            item.checked = Some(true);
            let (m, layout) = popup_layout(item);
            let glyphs = ContextMenuGlyphs::new().with_checked_glyph("V");
            let mut surface = RecordingSurface::default();
            native_surface_paint::paint_with_glyphs(
                &m,
                &layout,
                Platform::Linux,
                &mut surface,
                &Theme::default(),
                &Style::default(),
                Some(&glyphs),
                true,
            );
            assert!(surface.runs.iter().any(|r| r == "V"));
            let default_codicon = crate::codicon::CHECK.to_string();
            assert!(!surface.runs.iter().any(|r| r == &default_codicon));
        }

        /// Same override/default contract for the submenu pull-right arrow.
        #[test]
        fn override_replaces_submenu_arrow_glyph() {
            let mut item = cm_action("a", "More");
            item.submenu = Some(vec![cm_action("b", "Nested")]);
            let (m, layout) = popup_layout(item);
            let glyphs = ContextMenuGlyphs::new().with_submenu_arrow_glyph(">");
            let mut surface = RecordingSurface::default();
            native_surface_paint::paint_with_glyphs(
                &m,
                &layout,
                Platform::Linux,
                &mut surface,
                &Theme::default(),
                &Style::default(),
                Some(&glyphs),
                true,
            );
            assert!(surface.runs.iter().any(|r| r == ">"));
            let default_codicon = crate::codicon::CHEVRON_RIGHT.to_string();
            assert!(!surface.runs.iter().any(|r| r == &default_codicon));
        }

        /// An item's own [`ContextMenuItem::icon`] paints its resolved
        /// glyph in the leading slot when `checked` is `None`.
        /// `nerd_fonts_enabled` selects `Icon::glyph` vs. `Icon::fallback`,
        /// same as every other icon-bearing row primitive.
        #[test]
        fn item_icon_paints_resolved_glyph() {
            let mut item = cm_action("a", "New File");
            item.icon = Some(crate::types::Icon::new("G", "F"));
            let (m, layout) = popup_layout(item);

            let mut surface = RecordingSurface::default();
            native_surface_paint::paint_with_glyphs(
                &m,
                &layout,
                Platform::Linux,
                &mut surface,
                &Theme::default(),
                &Style::default(),
                None,
                true,
            );
            assert!(
                surface.runs.iter().any(|r| r == "G"),
                "expected the icon's nerd-font glyph among {:?}",
                surface.runs
            );

            let mut fallback_surface = RecordingSurface::default();
            native_surface_paint::paint_with_glyphs(
                &m,
                &layout,
                Platform::Linux,
                &mut fallback_surface,
                &Theme::default(),
                &Style::default(),
                None,
                false,
            );
            assert!(
                fallback_surface.runs.iter().any(|r| r == "F"),
                "expected the icon's plain-text fallback among {:?}",
                fallback_surface.runs
            );
        }

        /// A checked item's check mark wins the shared leading slot over
        /// its own icon — the icon never paints when `checked` is
        /// `Some(_)`.
        #[test]
        fn checked_state_wins_shared_slot_over_icon() {
            let mut item = cm_action("a", "Word Wrap");
            item.checked = Some(true);
            item.icon = Some(crate::types::Icon::new("G", "F"));
            let (m, layout) = popup_layout(item);

            let mut surface = RecordingSurface::default();
            native_surface_paint::paint_with_glyphs(
                &m,
                &layout,
                Platform::Linux,
                &mut surface,
                &Theme::default(),
                &Style::default(),
                None,
                true,
            );
            let expected_check = crate::codicon::CHECK.to_string();
            assert!(surface.runs.iter().any(|r| r == &expected_check));
            assert!(
                !surface.runs.iter().any(|r| r == "G"),
                "icon must not paint when checked wins the shared slot: {:?}",
                surface.runs
            );
        }
    }

    #[cfg(any(
        feature = "gtk",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    mod row_padding_x_paint_surface {
        use super::*;
        use crate::accelerator::Platform;
        use crate::paint_surface::PaintSurface;
        use crate::style::Style;
        use crate::theme::Theme;
        use crate::types::{Color, StyledText};
        use crate::{Point, Rect, Viewport};

        /// Records every painted text run's rect (not just the glyph
        /// string `glyph_override::RecordingSurface` needs) — the
        /// geometry assertions below need to compare against the row's
        /// own bounds.
        #[derive(Default)]
        struct RecordingSurface {
            texts: Vec<(Rect, String)>,
        }

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(0.0, 0.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                16.0
            }
            fn surface_char_width(&self) -> f32 {
                8.0
            }
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 8.0, 16.0)
            }
            fn surface_fill_rect(&mut self, _rect: Rect, _color: Color) {}
            fn surface_fill_rounded_rect(&mut self, _rect: Rect, _radius: f32, _color: Color) {}
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_stroke_rounded_rect(
                &mut self,
                _rect: Rect,
                _radius: f32,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, _color: Color) {
                self.texts.push((rect, text.to_string()));
            }
            fn surface_draw_line(
                &mut self,
                _from: Point,
                _to: Point,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(
                &mut self,
                _rect: Rect,
                _image: &crate::Image,
            ) -> crate::backend::ImagePaintResult {
                crate::backend::ImagePaintResult::Unsupported
            }
        }

        /// The issue's first acceptance bullet: a row's text starts
        /// `row_padding_x` in from its rect. Painted with a non-default
        /// [`Style::row_padding_x`] (15.0, not the 8.0 default) so
        /// reverting the token back to a hardcoded literal in `paint`
        /// would fail this test — both the leading label and the
        /// trailing shortcut move with it.
        #[test]
        fn label_and_shortcut_sit_row_padding_x_in_from_the_row_edges() {
            let mut item = cm_action("a", "Save");
            item.detail = Some(StyledText::plain("Ctrl+S"));
            let m = menu(vec![item]);
            let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
            let layout = m.layout(10.0, 10.0, viewport, 100.0, |_| {
                ContextMenuItemMeasure::new(20.0)
            });
            let row = layout.visible_items[0].bounds;

            let style = Style::default().with_row_padding_x(15.0);
            let mut surface = RecordingSurface::default();
            native_surface_paint::paint(
                &m,
                &layout,
                Platform::Linux,
                &mut surface,
                &Theme::default(),
                &style,
            );

            let (label_rect, _) = surface
                .texts
                .iter()
                .find(|(_, t)| t == "Save")
                .expect("label painted");
            assert_eq!(
                label_rect.x,
                row.x + style.row_padding_x,
                "label must start row_padding_x in from the row's left edge"
            );

            let (shortcut_rect, _) = surface
                .texts
                .iter()
                .find(|(_, t)| t == "Ctrl+S")
                .expect("shortcut painted");
            let shortcut_right = shortcut_rect.x + shortcut_rect.width;
            let expected_right = row.x + row.width - style.row_padding_x;
            assert!(
                (shortcut_right - expected_right).abs() < 0.01,
                "shortcut's right edge ({shortcut_right}) must sit row_padding_x in from \
                 the row's right edge ({expected_right})"
            );
        }
    }

    #[cfg(any(
        feature = "gtk",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    ))]
    mod interactive_hover {
        use super::*;
        use crate::accelerator::Platform;
        use crate::interaction::InteractionState;
        use crate::paint_surface::PaintSurface;
        use crate::style::Style;
        use crate::theme::Theme;
        use crate::types::Color;
        use crate::{Point, Rect, Viewport};

        /// Records every filled rect's colour — what the hover/pressed
        /// assertions below need, unlike the other `RecordingSurface`
        /// test doubles in this module which only record text runs.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
        }

        impl PaintSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(0.0, 0.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                16.0
            }
            fn surface_char_width(&self) -> f32 {
                8.0
            }
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 8.0, 16.0)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_fill_rounded_rect(&mut self, _rect: Rect, _radius: f32, _color: Color) {}
            fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
            fn surface_stroke_rounded_rect(
                &mut self,
                _rect: Rect,
                _radius: f32,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_draw_text_run(&mut self, _rect: Rect, _text: &str, _color: Color) {}
            fn surface_draw_line(
                &mut self,
                _from: Point,
                _to: Point,
                _color: Color,
                _stroke_width: f32,
            ) {
            }
            fn surface_push_clip(&mut self, _rect: Rect) {}
            fn surface_pop_clip(&mut self) {}
            fn surface_draw_image(
                &mut self,
                _rect: Rect,
                _image: &crate::Image,
            ) -> crate::backend::ImagePaintResult {
                crate::backend::ImagePaintResult::Unsupported
            }
        }

        fn popup(id: &str) -> (ContextMenu, ContextMenuLayout) {
            let m = menu(vec![cm_action(id, "Copy"), cm_action("other", "Paste")]);
            let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
            let layout = m.layout(10.0, 10.0, viewport, 100.0, |_| {
                ContextMenuItemMeasure::new(20.0)
            });
            (m, layout)
        }

        #[test]
        fn hovered_item_paints_menu_item_hover_bg() {
            // Hovers "other" (index 1), not "copy" (index 0, the
            // `menu()` helper's default `selected_idx`) — hovering the
            // keyboard-selected row is covered separately below, and
            // paints `theme.selected_bg` instead; see
            // `hovering_the_selected_row_keeps_selected_bg_not_hover_bg`.
            let (m, layout) = popup("copy");
            let theme = Theme::default();
            let mut interaction = InteractionState::new();
            interaction.set_hovered(Some(WidgetId::new("other")));
            let mut surface = RecordingSurface::default();
            native_surface_paint::paint_interactive(
                &m,
                &layout,
                Platform::Linux,
                &mut surface,
                &theme,
                &Style::default(),
                &interaction,
            );
            let expected = theme.menu_item_hover_bg(theme.hover_bg);
            assert!(
                surface.fills.iter().any(|(_, c)| *c == expected),
                "hovered item must paint Theme::menu_item_hover_bg: {:?}",
                surface.fills
            );
        }

        #[test]
        fn hovering_the_selected_row_keeps_selected_bg_not_hover_bg() {
            // "copy" is index 0, the `menu()` helper's default
            // `selected_idx` — hovering it with the mouse must not
            // downgrade its stronger `theme.selected_bg` highlight to
            // the fainter hover tint.
            let (m, layout) = popup("copy");
            let theme = Theme::default();
            let mut interaction = InteractionState::new();
            interaction.set_hovered(Some(WidgetId::new("copy")));
            let mut surface = RecordingSurface::default();
            native_surface_paint::paint_interactive(
                &m,
                &layout,
                Platform::Linux,
                &mut surface,
                &theme,
                &Style::default(),
                &interaction,
            );
            let hover_bg = theme.menu_item_hover_bg(theme.hover_bg);
            assert!(
                !surface.fills.iter().any(|(_, c)| *c == hover_bg),
                "the keyboard-selected row must not downgrade to the hover tint when hovered: {:?}",
                surface.fills
            );
            assert!(
                surface.fills.iter().any(|(_, c)| *c == theme.selected_bg),
                "the keyboard-selected row must keep theme.selected_bg while hovered: {:?}",
                surface.fills
            );
        }

        #[test]
        fn leaving_the_item_clears_the_hover_fill() {
            let (m, layout) = popup("copy");
            let theme = Theme::default();
            let interaction = InteractionState::new(); // nothing hovered
            let mut surface = RecordingSurface::default();
            native_surface_paint::paint_interactive(
                &m,
                &layout,
                Platform::Linux,
                &mut surface,
                &theme,
                &Style::default(),
                &interaction,
            );
            let hover_bg = theme.menu_item_hover_bg(theme.hover_bg);
            assert!(
                !surface.fills.iter().any(|(_, c)| *c == hover_bg),
                "no item is hovered, so no row should paint the hover tint: {:?}",
                surface.fills
            );
        }

        #[test]
        fn pressed_item_paints_menu_item_pressed_bg_not_hover_bg() {
            let (m, layout) = popup("copy");
            let theme = Theme::default();
            let mut interaction = InteractionState::new();
            interaction.set_hovered(Some(WidgetId::new("copy")));
            interaction.set_pressed(Some(WidgetId::new("copy")));
            let mut surface = RecordingSurface::default();
            native_surface_paint::paint_interactive(
                &m,
                &layout,
                Platform::Linux,
                &mut surface,
                &theme,
                &Style::default(),
                &interaction,
            );
            let pressed = theme.menu_item_pressed_bg(theme.hover_bg);
            let hover = theme.menu_item_hover_bg(theme.hover_bg);
            assert!(
                surface.fills.iter().any(|(_, c)| *c == pressed),
                "pressed item must paint Theme::menu_item_pressed_bg: {:?}",
                surface.fills
            );
            assert!(
                !surface.fills.iter().any(|(_, c)| *c == hover),
                "a pressed item must not also paint the plain hover tint: {:?}",
                surface.fills
            );
        }
    }
}
