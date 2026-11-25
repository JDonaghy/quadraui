//! `ContextMenu` primitive: a keyboard/mouse-navigable popup of actions
//! triggered at a specific screen location (right-click, keyboard
//! shortcut, explicit open-menu command). Each item is either an
//! action (clickable, emits an id), a separator (visual only), or a
//! submenu parent (opens a nested pull-right popup).
//!
//! Items carry `submenu: Option<Vec<ContextMenuItem>>` for nested menus.
//! Both painters render the `▶` affordance on a submenu parent; a host
//! (`compose::ContextMenuController`, `compose::MenuSystem`) places and
//! paints each open child popup beside its parent through
//! [`ContextMenuLayout::submenu_anchor`]. The macOS backend wires this
//! into native `NSMenu` nesting.
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

use crate::a11y::A11yAction;
use crate::accelerator::{Accelerator, Platform};
use crate::behaviour::nav::{FlatNav, NavKey};
use crate::behaviour::typeahead::Typeahead;
use crate::compose::Handled;
use crate::event::{Key, MouseButton, NamedKey, Point, Rect};
use crate::interactive::{
    EventCx, FocusSpec, Interactive, KeyEvent, PointerKind, RoutedEvent, SubTarget, WidgetAction,
};
use crate::layout::{Anchor, ResolvedSide, Side};
use crate::primitive::{LayoutCtx, Primitive, Size, Units};
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
///     glyphs: Default::default(),
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
    /// Check-mark / submenu-arrow glyph overrides for this popup.
    /// [`ContextMenuGlyphs::default`] keeps the crate's own marks, so a
    /// menu that names no glyphs renders exactly as it does without the
    /// field.
    #[serde(default)]
    pub glyphs: ContextMenuGlyphs,
}

/// Optional app-supplied override for the glyphs a context menu paints
/// in place of the default codicon check mark and submenu pull-right
/// arrow.
///
/// Lives on the descriptor, as [`ContextMenu::glyphs`], so every paint
/// path reaches it from the menu alone — the widget-contract
/// [`Primitive::paint`]/[`Primitive::paint_cells`] entry points take no
/// glyph argument, and a popup painted as a `Surface::Widget` renders
/// the same marks as one painted through a backend wrapper.
/// [`Self::default`] means "no overrides", so today's codicon glyphs
/// render byte-for-byte unchanged until an app opts in.
///
/// `#[non_exhaustive]`, so a future glyph slot is an additive change
/// here rather than a required field on the exhaustive [`ContextMenu`]
/// descriptor. On the wire both entries are optional: `{}`
/// deserialises to [`Self::new`] and `{"checked": {"glyph": "V",
/// "fallback": "v"}}` overrides only the check mark.
///
/// Each slot is an [`Icon`] — `glyph` for when
/// [`crate::Backend::nerd_fonts_enabled`] is set, `fallback` otherwise
/// — rather than a bare string. A bare `&str`/`String` still works via
/// `Icon`'s blanket `From` impls. Which half is taken comes from
/// [`crate::primitive::LayoutCtx::nerd_fonts_enabled`], carried onto
/// [`ContextMenuPrimitiveLayout::nerd_fonts_enabled`] at layout time.
///
/// The TUI rasteriser paints a single cell per glyph: it resolves the
/// `Icon` to a `&str` and takes only its first `char`, so a
/// multi-character override silently degrades to its leading character.
/// `Icon::color` is resolved but not painted on TUI — the glyph cell
/// always uses the surrounding theme foreground, never the `Icon`'s own
/// colour.
///
/// Both entries are boxed: an override is rare, while this registry is
/// a by-value field of the [`ContextMenu`] descriptor every frame
/// carries, so the empty case costs two pointers rather than two whole
/// [`Icon`]s.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ContextMenuGlyphs {
    #[serde(default)]
    checked: Option<Box<Icon>>,
    #[serde(default)]
    submenu_arrow: Option<Box<Icon>>,
}

impl ContextMenuGlyphs {
    /// An empty override set — a menu whose [`ContextMenu::glyphs`] is
    /// `Self::new()` paints the crate's own default marks.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the checked-item check-mark glyph (default: the codicon
    /// check glyph on GUI).
    #[must_use]
    pub fn with_checked_glyph(mut self, glyph: impl Into<Icon>) -> Self {
        self.checked = Some(Box::new(glyph.into()));
        self
    }

    /// Override the submenu-parent pull-right arrow glyph (default: the
    /// codicon chevron-right glyph on GUI).
    #[must_use]
    pub fn with_submenu_arrow_glyph(mut self, glyph: impl Into<Icon>) -> Self {
        self.submenu_arrow = Some(Box::new(glyph.into()));
        self
    }

    /// The overridden check-mark glyph, if set.
    #[must_use]
    pub fn checked_glyph(&self) -> Option<&Icon> {
        self.checked.as_deref()
    }

    /// The overridden submenu-arrow glyph, if set.
    #[must_use]
    pub fn submenu_arrow_glyph(&self) -> Option<&Icon> {
        self.submenu_arrow.as_deref()
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
    /// Unlike `checked`, `icon` has no `Some(false)`-style per-item
    /// reservation — the gutter column itself is decided menu-wide:
    /// when *any* item in the menu has `checked.is_some()` or
    /// `icon.is_some()`, every item's label (icon-bearing or not)
    /// indents by the same fixed gutter width, so the column stays
    /// aligned even when only some items fill it. Both
    /// [`native_surface_paint::paint_with_glyphs`] (pixel backends) and
    /// the TUI rasteriser apply this rule the same way.
    #[serde(default)]
    pub icon: Option<Icon>,
    /// Nested submenu. When `Some`, activating the item opens the child
    /// menu instead of firing an action. The painters render a `▶`
    /// affordance, and the host opens a pull-right popup (see
    /// [`ContextMenuLayout::submenu_anchor`]). The macOS NSMenu installer
    /// wires this as a real nested `NSMenu`.
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
    /// `"✓ "` / icon-glyph / `"  "` prefix both this module's
    /// `cell_paint::paint` and `native_surface_paint::paint` reserve — the two share one slot,
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
    /// The cell painter is exactly this case: its box-drawing border
    /// lives one char cell outside
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

// ── Widget contract: `Primitive` + `Interactive` ───────────────────────────

/// Cells a cell-grid rasteriser paints the popup's box border on,
/// *outside* the items region, on each side — [`cell_paint::paint`]'s own
/// chrome reservation, and what [`Primitive::layout`] reports as
/// [`ContextMenuPrimitiveLayout::border_inset`] on a cell grid.
///
/// A pixel surface reserves nothing: its border is stroked on the box's
/// own edge ([`native_surface_paint`]'s `surface_stroke_rounded_rect`),
/// inside the bounds, so there the inset is `0.0`.
pub const CELL_BORDER_INSET: f32 = 1.0;

/// The right-aligned shortcut hint one row shows — sourced from
/// [`ContextMenuItem::detail`] (preferred, back-compat) or rendered from
/// [`ContextMenuItem::key_equivalent`] for `platform` (`⌘S` on macOS,
/// `Ctrl+S` elsewhere). `None` when the item carries neither.
///
/// The one hint derivation every painter reads, so a pixel surface and a
/// cell grid never disagree about which string a row's trailing column
/// shows.
pub fn shortcut_text(item: &ContextMenuItem, platform: Platform) -> Option<String> {
    if let Some(ref det) = item.detail {
        return Some(det.spans.iter().map(|sp| sp.text.as_str()).collect());
    }
    item.key_equivalent
        .as_ref()
        .map(|acc| crate::accelerator::render_accelerator(acc, platform))
}

/// The item / separator row heights a menu lays its rows out at, from a
/// host's line height.
///
/// One rule for both menu styles — a free-floating right-click popup and
/// a bar-anchored dropdown — so the two stay geometrically identical
/// without either controller depending on the other's state. On a cell
/// grid (`line_height == 1.0`) both collapse to the single row a
/// terminal has to spend.
pub fn item_heights(line_height: f32) -> (f32, f32) {
    let item_h = (line_height * 1.4).round().max(line_height);
    let sep_h = (line_height * 0.5).round().max(1.0);
    (item_h, sep_h)
}

/// The platform a shortcut hint renders for when the popup paints
/// through [`Primitive::paint`], which carries no backend identity of its
/// own: the host OS this build runs on.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
pub(crate) fn host_platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::Macos
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else {
        Platform::Linux
    }
}

/// What a pointer position resolves to against one menu level — the
/// shared classification both [`Interactive::handle`] and
/// [`crate::compose::ContextMenuController`]'s deepest-level-first walk
/// resolve a click and a hover through, so the two cannot disagree about
/// which row a point belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextMenuClick {
    /// An actionable leaf row: activating it fires `id`.
    Item { idx: usize, id: WidgetId },
    /// A submenu-parent row: activating it opens that child menu.
    Submenu { idx: usize },
    /// A separator, a disabled row, or the box's own interior padding —
    /// inside the popup, but nothing to activate.
    Inert,
    /// Outside the popup's box entirely.
    Outside,
}

/// The semantic outcomes a [`ContextMenu`] popup reports — its
/// [`Interactive::Action`].
///
/// [`ContextMenu`] stays purely declarative: no variant below mutates the
/// popup. Each names the state the host writes back onto its own
/// `ContextMenu` (or, for the two closing variants, that it should drop
/// this level) before the next frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextMenuEvent {
    /// Keyboard navigation, typeahead, or the pointer moved the highlight
    /// to `idx`. The host writes it onto [`ContextMenu::selected_idx`].
    SelectionChanged { idx: usize },
    /// Item `idx` is a submenu parent and asked for its child popup —
    /// Enter, Right, or a click on that row. The host opens the child
    /// beside this menu, positioned through
    /// [`ContextMenuLayout::submenu_anchor`].
    SubmenuOpened { idx: usize },
    /// Left asked to close one level: the deepest open child popup, or
    /// this popup itself when it is the deepest.
    SubmenuClosed,
    /// An actionable item fired — Enter, a click, or an
    /// assistive-technology activation. Carries that item's own
    /// [`ContextMenuItem::id`], the identity a native menu reports too.
    Activated { item: WidgetId },
    /// Escape dismissed the popup without activating anything.
    Dismissed,
}

impl WidgetAction for ContextMenuEvent {
    /// [`Self::Activated`] and the two closing variants are the outcomes
    /// the crate's [`crate::UiEvent`] vocabulary names directly
    /// ([`crate::UiEvent::ContextMenuItemActivated`] /
    /// [`crate::UiEvent::ContextMenuDismissed`]) — the same two events the
    /// native pop-up path delivers through the event queue, so an app
    /// routes both paths through one match arm.
    /// [`Self::SubmenuClosed`] maps onto the dismissal too: for a single
    /// popup, closing the deepest level *is* dismissing this one. A
    /// multi-level composer reads [`ContextMenuEvent`] itself and keeps
    /// the distinction.
    ///
    /// The other two ride [`crate::UiEvent::List`] under the popup's own
    /// [`ContextMenu::id`], the channel [`crate::Completions`] already
    /// uses for the same shapes: a moved highlight is exactly
    /// [`crate::ListViewEvent::SelectionChanged`]'s `{ idx }`, and asking
    /// for a submenu is that row's activation,
    /// [`crate::ListViewEvent::ItemActivated`] — loss-free under one id,
    /// rather than an unrelated event an existing handler could act on.
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        match self {
            ContextMenuEvent::Activated { item } => crate::UiEvent::ContextMenuItemActivated(item),
            ContextMenuEvent::Dismissed | ContextMenuEvent::SubmenuClosed => {
                crate::UiEvent::ContextMenuDismissed
            }
            ContextMenuEvent::SelectionChanged { idx } => {
                crate::UiEvent::List(id, crate::ListViewEvent::SelectionChanged { idx })
            }
            ContextMenuEvent::SubmenuOpened { idx } => {
                crate::UiEvent::List(id, crate::ListViewEvent::ItemActivated { idx })
            }
        }
    }
}

/// [`ContextMenu`]'s own interaction state: the debounced typeahead
/// buffer that lets a few quick keystrokes jump the highlight to the
/// best prefix match, plus the monotonic origin its 500ms debounce window
/// is measured from.
///
/// Everything else a popup renders — its items, the highlight, the
/// background override — lives on the descriptor, which the host updates
/// from [`ContextMenuEvent`]; only the keystroke *timing* can't, since it
/// is not something a frame paints.
#[derive(Debug, Default)]
pub struct ContextMenuState {
    typeahead: Typeahead,
    origin: Option<std::time::Instant>,
}

impl ContextMenuState {
    /// Milliseconds since this state's first keystroke — the clock
    /// [`Typeahead::push`]'s debounce window reads. Seeded on first use,
    /// so a state that never sees a keystroke never reads a clock.
    fn now_ms(&mut self) -> u64 {
        let origin = *self.origin.get_or_insert_with(std::time::Instant::now);
        origin.elapsed().as_millis() as u64
    }
}

/// [`ContextMenu`]'s fully-resolved widget-contract layout: the absolute
/// `area` the popup was laid out in, the chrome reservation and inner
/// viewport that produced its box, the row heights the items were
/// resolved at, and the popup-local [`ContextMenuLayout`] itself.
///
/// `#[non_exhaustive]`: produced by [`Primitive::layout`], never built by
/// a consumer via struct literal — the same convention
/// [`ContextMenuLayout`] itself follows.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ContextMenuPrimitiveLayout {
    /// The rect [`Primitive::layout`] was called with: the popup's whole
    /// box, including the ring of chrome a cell grid paints its border
    /// on.
    pub area: Rect,
    /// Units of [`Self::area`] reserved for that border on each side —
    /// [`CELL_BORDER_INSET`] on a cell grid, `0.0` on a pixel surface.
    /// A caller reads the reservation off the resolved layout rather than
    /// asking a backend for it.
    pub border_inset: f32,
    /// [`Self::area`] inset by [`Self::border_inset`] on all four sides:
    /// the space the items were placed in, and the one the popup's own
    /// `bounds` is clamped inside.
    pub viewport: Rect,
    /// Height one actionable row was resolved at — [`item_heights`]'s
    /// first value for the host's line height.
    pub item_height: f32,
    /// Height one separator row was resolved at — [`item_heights`]'s
    /// second value.
    pub separator_height: f32,
    /// [`crate::primitive::LayoutCtx::nerd_fonts_enabled`] as the host
    /// reported it at layout time, carried through to paint so both
    /// [`ContextMenu::glyphs`] and an item's own
    /// [`ContextMenuItem::icon`] resolve to the same half of their
    /// [`Icon`] the surrounding chrome does.
    pub nerd_fonts_enabled: bool,
    /// The resolved popup layout. Its `bounds`/`visible_items`/
    /// `hit_regions` are in the same absolute space as [`Self::area`].
    pub layout: ContextMenuLayout,
}

impl ContextMenuPrimitiveLayout {
    /// Hit-test at a position relative to [`Self::area`]'s own origin —
    /// the space [`crate::interactive::RoutedEvent::Pointer`]'s `local`
    /// field uses.
    pub fn hit_test_local(&self, x: f32, y: f32) -> ContextMenuHit {
        self.layout.hit_test(self.area.x + x, self.area.y + y)
    }

    /// How many rows the popup shows at once — the page size
    /// `PageUp`/`PageDown` move by. At least `1`, so a box too short for
    /// a single row still pages one item at a time rather than standing
    /// still.
    pub fn page(&self) -> usize {
        self.layout.visible_items.len().max(1)
    }
}

impl ContextMenu {
    /// Whether item `i` can take the highlight: not a separator, not
    /// disabled.
    fn is_selectable(&self, i: usize) -> bool {
        self.items
            .get(i)
            .is_some_and(|item| !item.is_separator() && !item.disabled)
    }

    /// Item `i`'s leading label text — the string typeahead matches a
    /// prefix against, empty for a row no keystroke may land on.
    fn typeahead_text(&self, i: usize) -> &str {
        if !self.is_selectable(i) {
            return "";
        }
        self.items[i]
            .label
            .spans
            .first()
            .map(|s| s.text.as_str())
            .unwrap_or("")
    }

    /// Where a [`NavKey`] moves the highlight from `current`, skipping
    /// separators and disabled items and wrapping at both ends.
    ///
    /// `page` is how many rows the popup shows at once, for the two Page
    /// keys. `current` itself counts as navigable for a stepping move, so
    /// an app-supplied [`Self::selected_idx`] that points at a separator
    /// still steps to the next selectable item in list order instead of
    /// seeding at either end; `Home`/`End` ignore it and land on the
    /// first/last selectable row.
    pub fn navigate(&self, current: usize, key: NavKey, page: usize) -> Option<usize> {
        let key = match key {
            NavKey::PageUp(_) => NavKey::PageUp(page.max(1)),
            NavKey::PageDown(_) => NavKey::PageDown(page.max(1)),
            other => other,
        };
        match key {
            NavKey::Home | NavKey::End => {
                FlatNav::mv(self.items.len(), Some(current), key, true, |i| {
                    self.is_selectable(i)
                })
            }
            _ => FlatNav::mv(self.items.len(), Some(current), key, true, |i| {
                i == current || self.is_selectable(i)
            }),
        }
    }

    /// Resolve a pointer position (in the same absolute space `layout`
    /// reports) against this one menu level.
    pub fn resolve_click(&self, layout: &ContextMenuLayout, x: f32, y: f32) -> ContextMenuClick {
        match layout.hit_test(x, y) {
            ContextMenuHit::Empty => ContextMenuClick::Outside,
            ContextMenuHit::Inert => ContextMenuClick::Inert,
            ContextMenuHit::Item(id) => {
                let idx = layout
                    .visible_items
                    .iter()
                    .find(|vis| vis.clickable && self.items[vis.item_idx].id == Some(id))
                    .map(|vis| vis.item_idx);
                match idx {
                    Some(idx) if self.items[idx].submenu.is_some() => {
                        ContextMenuClick::Submenu { idx }
                    }
                    Some(idx) => ContextMenuClick::Item { idx, id },
                    None => ContextMenuClick::Inert,
                }
            }
        }
    }

    /// Activate item `idx`: open its child popup when it is a submenu
    /// parent, fire its id when it is an actionable leaf, and swallow the
    /// gesture otherwise (a separator or disabled row owns the keystroke
    /// but has nothing to report).
    fn activate(&self, idx: usize) -> Handled<ContextMenuEvent> {
        match self.items.get(idx) {
            Some(item) if item.submenu.is_some() => {
                Handled::Event(ContextMenuEvent::SubmenuOpened { idx })
            }
            Some(item) if !item.disabled => match item.id {
                Some(item) => Handled::Event(ContextMenuEvent::Activated { item }),
                None => Handled::Consumed,
            },
            _ => Handled::Consumed,
        }
    }

    /// Resolve one keystroke against the popup — the single key→meaning
    /// rule both [`Interactive::handle`] and
    /// [`crate::compose::ContextMenuController`] dispatch through, so a
    /// popup driven by either cannot disagree about what a key does.
    ///
    /// `page` is how many rows the popup shows at once (see
    /// [`ContextMenuPrimitiveLayout::page`]); `state` carries the
    /// debounced typeahead buffer a typed character extends.
    pub fn handle_key(
        &self,
        key: &KeyEvent,
        page: usize,
        state: &mut ContextMenuState,
    ) -> Handled<ContextMenuEvent> {
        let current = self.selected_idx;
        match key.key {
            Key::Named(NamedKey::Escape) => Handled::Event(ContextMenuEvent::Dismissed),
            Key::Named(NamedKey::Left) => Handled::Event(ContextMenuEvent::SubmenuClosed),
            // Right only opens a submenu — on a leaf it belongs to
            // whatever the popup is floating over, not to the popup.
            Key::Named(NamedKey::Right) => {
                if self
                    .items
                    .get(current)
                    .is_some_and(|item| item.submenu.is_some())
                {
                    Handled::Event(ContextMenuEvent::SubmenuOpened { idx: current })
                } else {
                    Handled::Ignored
                }
            }
            Key::Named(NamedKey::Enter) => self.activate(current),
            // A typed character jumps the highlight to the best prefix
            // match through the debounced buffer. Modified keystrokes
            // (Ctrl/Alt/Super) are accelerators the host owns, not
            // typeahead.
            Key::Char(ch) if !key.modifiers.ctrl && !key.modifiers.alt && !key.modifiers.cmd => {
                let now_ms = state.now_ms();
                let len = self.items.len();
                match state
                    .typeahead
                    .push(ch, now_ms, |i| self.typeahead_text(i), len, current)
                {
                    Some(idx) if idx != current => {
                        Handled::Event(ContextMenuEvent::SelectionChanged { idx })
                    }
                    // Already on the best match: the popup owns the
                    // keystroke (it must not reach whatever is behind
                    // the menu) but has nothing to report.
                    Some(_) => Handled::Consumed,
                    None => Handled::Consumed,
                }
            }
            _ => {
                let Some(nav) = NavKey::from_key(key) else {
                    return Handled::Ignored;
                };
                match self.navigate(current, nav, page) {
                    Some(idx) if idx != current => {
                        Handled::Event(ContextMenuEvent::SelectionChanged { idx })
                    }
                    Some(_) => Handled::Consumed,
                    None => Handled::Ignored,
                }
            }
        }
    }
}

impl Primitive for ContextMenu {
    type Layout = ContextMenuPrimitiveLayout;
    type Event = ContextMenuEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::ContextMenu(self.clone())
    }

    /// The widest row's own content ([`Self::measure_generic_width`]) and
    /// one [`item_heights`] row per item — plus, on a cell grid, the
    /// border ring both axes spend a cell on at each edge.
    fn measure(&self, ctx: &LayoutCtx, _avail: Size) -> crate::NaturalSize {
        let inset = border_inset_for(ctx);
        let (item_h, sep_h) = item_heights(row_pitch(ctx));
        let height: f32 = self
            .items
            .iter()
            .map(|item| if item.is_separator() { sep_h } else { item_h })
            .sum();
        crate::NaturalSize::new(
            Some(self.measure_generic_width(ctx.char_width, 0.0) + inset * 2.0),
            Some(height + inset * 2.0),
        )
    }

    /// `rect` is the popup's whole box — a host has already resolved
    /// where the popup goes (through [`ContextMenu::layout`]'s own
    /// anchor-relative placement, or by handing this widget a rect) — so
    /// the items region is `rect` inset by
    /// [`ContextMenuPrimitiveLayout::border_inset`] on all four sides,
    /// which is both the space the rows are placed in and the chrome
    /// reservation a cell-grid border needs. Rows are [`item_heights`]
    /// tall for the host's own line height.
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> ContextMenuPrimitiveLayout {
        let border_inset = border_inset_for(ctx);
        let viewport = Rect::new(
            rect.x + border_inset,
            rect.y + border_inset,
            (rect.width - border_inset * 2.0).max(0.0),
            (rect.height - border_inset * 2.0).max(0.0),
        );
        let (item_height, separator_height) = item_heights(row_pitch(ctx));
        // The box fills `viewport`: every `ContextMenuPlacement` resolves
        // to the viewport's own origin for a zero-height anchor sitting
        // there (`Anchor::resolve` clamps the box inside the viewport
        // whichever side it prefers), so an already-placed popup paints
        // where its host put it rather than re-running placement.
        let layout = self.layout(viewport.x, viewport.y, viewport, viewport.width, |i| {
            if self.items[i].is_separator() {
                ContextMenuItemMeasure::new(separator_height)
            } else {
                ContextMenuItemMeasure::new(item_height)
            }
        });
        ContextMenuPrimitiveLayout {
            area: rect,
            border_inset,
            viewport,
            item_height,
            separator_height,
            nerd_fonts_enabled: ctx.nerd_fonts_enabled,
            layout,
        }
    }

    /// The shared pixel painter,
    /// `native_surface_paint::paint_with_glyphs`, rendering shortcut hints
    /// for the host platform, [`Self::glyphs`]'s marks and each item's own
    /// icon resolved at [`ContextMenuPrimitiveLayout::nerd_fonts_enabled`].
    fn paint(
        &self,
        layout: &ContextMenuPrimitiveLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        theme: &crate::theme::Theme,
        style: &crate::style::Style,
    ) {
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            native_surface_paint::paint_with_glyphs(
                self,
                &layout.layout,
                host_platform(),
                surface,
                theme,
                style,
                Some(&self.glyphs),
                layout.nerd_fonts_enabled,
            );
        }
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        let _ = (layout, surface, theme, style);
    }

    /// [`cell_paint::paint`] over `buf`. [`Self::glyphs`] and
    /// [`ContextMenuPrimitiveLayout::nerd_fonts_enabled`] are the two
    /// glyph inputs, so the cell path resolves the same marks the pixel
    /// path does.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &ContextMenuPrimitiveLayout,
        buf: &mut ratatui::buffer::Buffer,
        theme: &crate::theme::Theme,
    ) {
        // The painter never moves the cursor, so the sink is never read.
        let mut cursor_sink = None;
        let mut surface = crate::tui::cell_surface::TuiCellSurface::new(
            buf,
            crate::backend::ColorDepth::TrueColor,
            &mut cursor_sink,
        );
        cell_paint::paint(
            self,
            &layout.layout,
            &mut surface,
            theme,
            Some(&self.glyphs),
            layout.nerd_fonts_enabled,
        );
    }
}

/// The border reservation a `ctx`'s units imply — see
/// [`CELL_BORDER_INSET`].
fn border_inset_for(ctx: &LayoutCtx) -> f32 {
    match ctx.units() {
        Units::Cells => CELL_BORDER_INSET,
        _ => 0.0,
    }
}

/// The line height [`item_heights`] derives row pitch from, floored at
/// one unit so a ctx carrying no line height still lays out one row per
/// item instead of collapsing every row to zero.
fn row_pitch(ctx: &LayoutCtx) -> f32 {
    if ctx.line_height > 0.0 {
        ctx.line_height
    } else {
        1.0
    }
}

impl Interactive for ContextMenu {
    type State = ContextMenuState;
    type Action = ContextMenuEvent;

    /// One focus stop as a whole while the popup is live — it navigates
    /// internally through [`Self::handle`]'s own key dispatch rather than
    /// declaring a stop per row. [`FocusSpec::None`] for an empty popup.
    fn focus(&self) -> FocusSpec {
        if self.items.is_empty() {
            FocusSpec::None
        } else {
            FocusSpec::Stop
        }
    }

    fn handle(
        &self,
        state: &mut ContextMenuState,
        event: &RoutedEvent,
        layout: &ContextMenuPrimitiveLayout,
        cx: &mut EventCx,
    ) -> Handled<ContextMenuEvent> {
        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                local,
                ..
            } => {
                let click = self.resolve_click(
                    &layout.layout,
                    layout.area.x + local.x,
                    layout.area.y + local.y,
                );
                match click {
                    ContextMenuClick::Item { id, .. } => {
                        cx.request_focus(None);
                        Handled::Event(ContextMenuEvent::Activated { item: id })
                    }
                    ContextMenuClick::Submenu { idx } => {
                        cx.request_focus(None);
                        Handled::Event(ContextMenuEvent::SubmenuOpened { idx })
                    }
                    // A separator, a disabled row, or the box's own
                    // chrome: the popup swallows the click rather than
                    // letting it fall through to whatever it floats over.
                    ContextMenuClick::Inert => {
                        cx.request_focus(None);
                        Handled::Consumed
                    }
                    ContextMenuClick::Outside => Handled::Event(ContextMenuEvent::Dismissed),
                }
            }
            // Hover moves the highlight, and opens a submenu parent's
            // child popup immediately — no timer, the same "open on
            // hover" shape every menu in this crate uses.
            RoutedEvent::Pointer {
                kind: PointerKind::Move,
                local,
                ..
            } => {
                let click = self.resolve_click(
                    &layout.layout,
                    layout.area.x + local.x,
                    layout.area.y + local.y,
                );
                match click {
                    ContextMenuClick::Submenu { idx } => {
                        Handled::Event(ContextMenuEvent::SubmenuOpened { idx })
                    }
                    ContextMenuClick::Item { idx, .. } if idx != self.selected_idx => {
                        Handled::Event(ContextMenuEvent::SelectionChanged { idx })
                    }
                    ContextMenuClick::Item { .. } => Handled::Consumed,
                    ContextMenuClick::Inert | ContextMenuClick::Outside => Handled::Ignored,
                }
            }
            RoutedEvent::Key(key) => self.handle_key(key, layout.page(), state),
            // `A11yAction` carries no row index through this generic
            // seam, so `Click` activates whichever row is highlighted —
            // the same one Enter activates, matching the
            // assistive-technology default-action convention.
            RoutedEvent::Action(A11yAction::Click) => self.activate(self.selected_idx),
            _ => Handled::Ignored,
        }
    }

    fn a11y_actions(&self, _state: &ContextMenuState) -> Vec<A11yAction> {
        if self.items.is_empty() {
            Vec::new()
        } else {
            vec![A11yAction::Click]
        }
    }

    /// One sub-part per visible row, addressed by its index into
    /// [`ContextMenu::items`], in [`Self::handle`]'s own local space.
    fn sub_regions(&self, layout: &ContextMenuPrimitiveLayout) -> Vec<(Rect, SubTarget)> {
        layout
            .layout
            .visible_items
            .iter()
            .map(|vis| {
                (
                    Rect::new(
                        vis.bounds.x - layout.area.x,
                        vis.bounds.y - layout.area.y,
                        vis.bounds.width,
                        vis.bounds.height,
                    ),
                    SubTarget(vis.item_idx as u64),
                )
            })
            .collect()
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
    use super::{shortcut_text, ContextMenu, ContextMenuGlyphs, ContextMenuLayout};
    use crate::accelerator::Platform;
    use crate::interaction::InteractionState;
    use crate::paint_surface::PaintSurface;
    use crate::style::Style;
    use crate::theme::Theme;
    use crate::types::{Color, WidgetId};
    use crate::{Point, Rect};

    /// Pixel width of the leading check-mark/icon glyph itself, when the
    /// menu-wide gutter column (see `has_icon_gutter` in
    /// `paint_with_glyphs_impl`) is reserved.
    const ICON_COLUMN_WIDTH: f32 = 16.0;
    /// Gap between the glyph column and the label text that follows it.
    const ICON_COLUMN_GAP: f32 = 4.0;

    /// Paint a [`ContextMenu`] popup at its caller-resolved
    /// `menu_layout` onto `surface`. Returns the per-clickable-item hit
    /// rectangles + their [`WidgetId`]s so the caller's click handler
    /// can resolve mouse events without re-running layout.
    ///
    /// `platform` selects the key-equivalent render form (`⌘S` vs.
    /// `Ctrl+S`) — see [`shortcut_text`]. `style` supplies
    /// the corner radius/border width/shadow elevation the popup box
    /// paints with — callers pass `&self.style()`. The menu's own
    /// [`ContextMenu::glyphs`] supplies the check-mark / submenu-arrow
    /// marks; [`paint_with_glyphs`] additionally takes a caller-supplied
    /// override set and the `nerd_fonts_enabled` flag that resolves it.
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
    /// instead of its plain selection/resting fill.
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

    /// Identical to [`paint`], plus an explicit [`ContextMenuGlyphs`]
    /// override for the check-mark / submenu-arrow glyphs and the
    /// `nerd_fonts_enabled` flag (see [`crate::Backend::nerd_fonts_enabled`])
    /// that selects each override [`Icon`]'s `glyph` vs. `fallback` form.
    /// `glyphs: None` (what [`paint`] passes) falls back to the menu's
    /// own [`ContextMenu::glyphs`], so an unset descriptor field renders
    /// byte-for-byte the same as today's codicon defaults.
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

        // An explicit argument wins; with none, the menu's own
        // descriptor field is the override set, so every paint path
        // honours it without threading an extra parameter.
        let glyphs = Some(glyphs.unwrap_or(&menu.glyphs));

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

        // Whether *any* item in the menu carries a check state or an
        // icon decides whether the whole menu reserves the leading
        // gutter column — menu-wide, not per-item, so every label in
        // the menu indents by the same amount and stays aligned even
        // when only some items actually fill the slot.
        let has_icon_gutter = menu
            .items
            .iter()
            .any(|item| item.checked.is_some() || item.icon.is_some());
        let gutter_width = if has_icon_gutter {
            ICON_COLUMN_WIDTH + ICON_COLUMN_GAP
        } else {
            0.0
        };

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

            // The leading gutter column (check glyph or item icon) is
            // reserved menu-wide via `gutter_width`, not per item — every
            // label in the menu indents by the same fixed amount, so a
            // mix of icon-bearing and plain items still lines up. The
            // glyph itself paints separately via `surface_draw_icon_glyph`
            // (below) rather than being embedded in this plain
            // `surface_draw_text_run` call: a PUA codepoint baked into an
            // ordinary text run resolves through whatever font that run's
            // own call site already carries, which is not guaranteed to
            // be the fallback-wrapped description `surface_draw_icon_glyph`
            // resolves through — the same reason the submenu pull-right
            // arrow below gets its own draw call instead of living in
            // this label string. A checkable item's state mark always
            // wins that one slot over a decorative icon — see
            // [`ContextMenuItem::icon`]'s doc.
            let label_text: String = item.label.spans.iter().map(|s| s.text.clone()).collect();
            let label_fg = if vis.clickable {
                theme.foreground
            } else {
                theme.muted_fg
            };
            let (_, lh) = surface.surface_measure_text(&label_text);
            let text_y = row.y + (row.height - lh) / 2.0;
            let gutter_x = row.x + style.row_padding_x;
            let label_x = gutter_x + gutter_width;
            surface.surface_draw_text_run(
                Rect::new(
                    label_x,
                    text_y,
                    (row.width - style.row_padding_x - gutter_width).max(0.0),
                    lh,
                ),
                &label_text,
                label_fg,
            );

            if item.checked == Some(true) {
                let (cw, ch) = surface.surface_measure_text(&check_glyph);
                surface.surface_draw_icon_glyph(
                    Rect::new(gutter_x, text_y, cw.max(1.0), ch.max(1.0)),
                    &check_glyph,
                    label_fg,
                );
            } else if item.checked.is_none() {
                if let Some(icon) = &item.icon {
                    let glyph = icon.resolve(nerd_fonts_enabled);
                    let (iw, ih) = surface.surface_measure_text(glyph);
                    surface.surface_draw_icon_glyph(
                        Rect::new(gutter_x, text_y, iw.max(1.0), ih.max(1.0)),
                        glyph,
                        label_fg,
                    );
                }
            }

            if item.submenu.is_some() {
                // Submenu-parent: show a codicon chevron pull-right
                // affordance at the far-right column instead of a
                // keyboard-shortcut hint — mirrors the shape the cell
                // painter paints with a plain `▶` glyph instead. Submenu parents open a child
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

/// The cell-grid painter for [`ContextMenu`], written against
/// [`crate::CellSurface`].
///
/// A box-bordered popup with one row per item: the border ring paints on
/// the cells immediately *outside* [`ContextMenuLayout::bounds`] (the
/// [`CELL_BORDER_INSET`] a caller reserves up front), and every row
/// paints inside it.
///
/// - A **separator** row draws a horizontal `─` rule in
///   [`crate::Theme::muted_fg`].
/// - The **selected** clickable row renders inverted (fg/bg swapped);
///   a disabled row renders dimmed.
/// - An item with `checked == Some(true)` shows a leading `✓` in the
///   row's left gutter column; an item with no `checked` state but an
///   [`ContextMenuItem::icon`] shows that icon's resolved glyph in the
///   same column instead (`checked` always wins the shared slot). When
///   *any* item in the menu carries a checked state or an icon, every row
///   reserves a two-column gutter (glyph plus one blank gap) ahead of its
///   label, so a glyph never runs into the label; a menu with neither
///   keeps the single-column left margin.
/// - A **submenu parent** shows `▶` in the far-right column instead of a
///   shortcut hint (it opens a child menu rather than firing an action);
///   every other row's trailing columns show [`shortcut_text`]'s hint,
///   right-aligned.
///
/// `glyphs` optionally overrides the check-mark and submenu-arrow marks
/// the menu's own [`ContextMenu::glyphs`] otherwise supplies; each slot
/// is a single cell, so only an override's first `char` renders, and
/// `nerd_fonts_enabled` picks which half of each override
/// [`Icon`] is used. The same content
/// [`native_surface_paint::paint`] puts on a pixel surface, degraded onto
/// the cell grid (see [`crate::paint_surface`]'s module doc, "Why TUI
/// stays out").
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::{shortcut_text, ContextMenu, ContextMenuGlyphs, ContextMenuLayout};
    use crate::accelerator::Platform;
    use crate::cell_surface::{CellStyle, CellSurface};
    use crate::theme::Theme;
    use crate::types::Color;

    /// An opaque single-cell style — colours only, no modifier.
    fn solid(fg: Color, bg: Color) -> CellStyle {
        CellStyle {
            fg: Some(fg),
            bg: Some(bg),
            ..CellStyle::default()
        }
    }

    /// Paint `menu` at its caller-resolved `layout`.
    ///
    /// `layout.bounds` is the **inner** items region; the box border
    /// paints on a whole cell *outside* it on every side. For that border
    /// to have a cell to occupy when the menu is anchored flush against a
    /// viewport edge, the caller reserves it up front — the inset
    /// [`crate::primitive::Primitive::layout`] reports as
    /// [`super::ContextMenuPrimitiveLayout::border_inset`]. Reserving it
    /// in the layout, rather than shifting only where this function
    /// paints, keeps painted rows and the rows
    /// [`ContextMenuLayout::hit_test`] reports in one coordinate space.
    ///
    /// That inset is a *recommendation*, not a precondition: a layout
    /// built from a non-inset viewport (so `bounds.x` or `bounds.y`
    /// rounds to `0`) still renders, and still paints every item on the
    /// row its own `bounds` reports. The consequence is cosmetic and
    /// confined to the near edge — the border column/row has nowhere to
    /// go outside the surface, so it falls on the first item
    /// column/row, which is painted after it and wins. Nothing here
    /// panics or wraps on such a layout.
    pub(crate) fn paint(
        menu: &ContextMenu,
        layout: &ContextMenuLayout,
        surface: &mut dyn CellSurface,
        theme: &Theme,
        glyphs: Option<&ContextMenuGlyphs>,
        nerd_fonts_enabled: bool,
    ) {
        // An explicit argument wins; with none, the menu's own
        // descriptor field is the override set, so every paint path
        // honours it without threading an extra parameter.
        let glyphs = Some(glyphs.unwrap_or(&menu.glyphs));

        let bg = theme.tab_bar_bg;
        let fg = theme.foreground;
        let sep_fg = theme.muted_fg;
        let dim_fg = theme.muted_fg;

        // Menu-wide decision, not per-item: when any item carries a
        // checked state or an icon, every row's label indents by two
        // columns (glyph + gap) instead of one, so a glyph never runs
        // straight into the label text that follows it.
        let has_icon_gutter = menu
            .items
            .iter()
            .any(|item| item.checked.is_some() || item.icon.is_some());
        let label_col_offset: u16 = if has_icon_gutter { 2 } else { 1 };

        let inner_x = layout.bounds.x.round() as u16;
        let inner_y = layout.bounds.y.round() as u16;
        let inner_w = layout.bounds.width.round() as u16;
        let inner_h = layout.bounds.height.round() as u16;
        if inner_w == 0 || inner_h == 0 {
            return;
        }
        // Border box: the ring of cells immediately outside the items
        // region. `bounds` comes from caller-supplied `f32` geometry, so
        // a viewport that reserved no inset can put the items region at
        // column/row 0, leaving the near border no cell inside the
        // surface to occupy. Saturate instead of underflowing: the near
        // border then shares the first item column/row, which is painted
        // afterwards and wins, so the only consequence is a missing
        // edge. Items always paint at the coordinates `bounds` reports,
        // keeping paint and `hit_test` in agreement whichever way this
        // lands.
        let left = inner_x.saturating_sub(1);
        let top = inner_y.saturating_sub(1);
        let right = inner_x.saturating_add(inner_w);
        let bottom = inner_y.saturating_add(inner_h);

        let chrome = solid(fg, bg);
        for cy in top..=bottom {
            for cx in left..=right {
                let ch = if cy == top {
                    if cx == left {
                        '┌'
                    } else if cx == right {
                        '┐'
                    } else {
                        '─'
                    }
                } else if cy == bottom {
                    if cx == left {
                        '└'
                    } else if cx == right {
                        '┘'
                    } else {
                        '─'
                    }
                } else if cx == left || cx == right {
                    '│'
                } else {
                    ' '
                };
                surface.put_char(cx, cy, ch, chrome);
            }
        }

        for vis in &layout.visible_items {
            let item = &menu.items[vis.item_idx];
            let row_y = vis.bounds.y.round() as u16;
            if vis.is_separator {
                for dx in 0..inner_w {
                    surface.put_char(inner_x.saturating_add(dx), row_y, '─', solid(sep_fg, bg));
                }
                continue;
            }
            let is_selected = vis.item_idx == menu.selected_idx;
            let (item_fg, item_bg) = if is_selected && vis.clickable {
                (bg, fg) // inverted
            } else if !vis.clickable {
                (dim_fg, bg)
            } else {
                (fg, bg)
            };
            let row_style = solid(item_fg, item_bg);
            for dx in 0..inner_w {
                surface.put_char(inner_x.saturating_add(dx), row_y, ' ', row_style);
            }
            // Check prefix — sits in the left gutter column at inner_x.
            // `Some(false)` reserves the slot without filling it (so a
            // column of mixed checked/unchecked items aligns visually).
            if matches!(item.checked, Some(true)) {
                let ch = glyphs
                    .and_then(ContextMenuGlyphs::checked_glyph)
                    .map(|icon| icon.resolve(nerd_fonts_enabled))
                    .and_then(|s| s.chars().next())
                    .unwrap_or('✓');
                surface.put_char(inner_x, row_y, ch, row_style);
            } else if item.checked.is_none() {
                // No checkbox state claims the shared leading slot —
                // show the item's own icon there instead, if it has one.
                // A single cell, same degrade every TUI chrome-glyph slot
                // takes: only the resolved glyph's first `char` renders.
                if let Some(ch) = item
                    .icon
                    .as_ref()
                    .and_then(|icon| icon.resolve(nerd_fonts_enabled).chars().next())
                {
                    surface.put_char(inner_x, row_y, ch, row_style);
                }
            }
            let label = item
                .label
                .spans
                .first()
                .map(|s| s.text.as_str())
                .unwrap_or("");
            for (i, ch) in label.chars().enumerate() {
                let col = inner_x
                    .saturating_add(label_col_offset)
                    .saturating_add(i as u16);
                if col >= right {
                    break;
                }
                surface.put_char(col, row_y, ch, row_style);
            }
            if item.submenu.is_some() {
                // Submenu parent: `▶` at the far-right column. Keyboard
                // shortcuts are not rendered for submenu parents (they
                // open a child menu rather than dispatching an action).
                let arrow_fg = if is_selected && vis.clickable {
                    item_fg
                } else {
                    dim_fg
                };
                let arrow_ch = glyphs
                    .and_then(ContextMenuGlyphs::submenu_arrow_glyph)
                    .map(|icon| icon.resolve(nerd_fonts_enabled))
                    .and_then(|s| s.chars().next())
                    .unwrap_or('▶');
                surface.put_char(
                    right.saturating_sub(1),
                    row_y,
                    arrow_ch,
                    solid(arrow_fg, item_bg),
                );
            } else if let Some(shortcut) = shortcut_text(item, Platform::Tui) {
                let sc_w = shortcut.chars().count().min(u16::MAX as usize) as u16;
                let sc_start =
                    inner_x.saturating_add(inner_w.saturating_sub(sc_w.saturating_add(1)));
                let sc_fg = if is_selected && vis.clickable {
                    item_fg
                } else {
                    dim_fg
                };
                let sc_style = solid(sc_fg, item_bg);
                for (i, ch) in shortcut.chars().enumerate() {
                    let col = sc_start.saturating_add(i as u16);
                    if col >= right {
                        break;
                    }
                    surface.put_char(col, row_y, ch, sc_style);
                }
            }
        }
    }
}

/// The cell painter's own coverage, run against a real ratatui buffer
/// through the test-local [`draw_context_menu`] and its glyph-override
/// and submenu-stack siblings, which adapt a `Buffer` into the
/// [`crate::CellSurface`] [`cell_paint::paint`] takes.
#[cfg(all(test, feature = "tui"))]
mod cell_paint_tests {
    use super::*;
    use crate::theme::Theme;
    use crate::types::{StyledSpan, StyledText, WidgetId};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    /// Paints `menu` at `layout` with its own glyphs at the ASCII half of
    /// each icon.
    fn draw_context_menu(
        buf: &mut Buffer,
        menu: &ContextMenu,
        layout: &ContextMenuLayout,
        theme: &Theme,
    ) {
        draw_context_menu_with_glyphs(buf, menu, layout, theme, None, false);
    }

    /// [`draw_context_menu`] with an override glyph set and the Nerd Font
    /// flag; `glyphs: None` falls back to the menu's own glyphs.
    fn draw_context_menu_with_glyphs(
        buf: &mut Buffer,
        menu: &ContextMenu,
        layout: &ContextMenuLayout,
        theme: &Theme,
        glyphs: Option<&ContextMenuGlyphs>,
        nerd_fonts_enabled: bool,
    ) {
        let mut cursor_sink = None;
        let mut surface = crate::tui::cell_surface::TuiCellSurface::new(
            buf,
            crate::backend::ColorDepth::TrueColor,
            &mut cursor_sink,
        );
        cell_paint::paint(
            menu,
            layout,
            &mut surface,
            theme,
            glyphs,
            nerd_fonts_enabled,
        );
    }

    /// Paints `root_menu` and every submenu `submenu_path` opens inside
    /// it, each child anchored through
    /// [`ContextMenuLayout::submenu_anchor`] in `viewport` inset by the
    /// border ring, and carrying the root's own glyphs.
    /// `selected_at_depth[d]` is the selected row inside the submenu at
    /// depth `d`.
    fn draw_context_menu_with_submenus(
        buf: &mut Buffer,
        root_menu: &ContextMenu,
        root_layout: &ContextMenuLayout,
        submenu_path: &[usize],
        selected_at_depth: &[usize],
        viewport: crate::event::Rect,
        theme: &Theme,
    ) {
        draw_context_menu(buf, root_menu, root_layout, theme);
        let vp = crate::event::Rect::new(
            viewport.x + CELL_BORDER_INSET,
            viewport.y + CELL_BORDER_INSET,
            (viewport.width - CELL_BORDER_INSET * 2.0).max(0.0),
            (viewport.height - CELL_BORDER_INSET * 2.0).max(0.0),
        );
        let mut parent_items: Vec<ContextMenuItem> = root_menu.items.clone();
        let mut parent_layout = root_layout.clone();
        const MENU_WIDTH: f32 = 20.0;
        for (depth, &path_idx) in submenu_path.iter().enumerate() {
            let Some(sub_items) = parent_items
                .get(path_idx)
                .and_then(|item| item.submenu.clone())
            else {
                break;
            };
            let anchor = parent_layout.submenu_anchor(path_idx, MENU_WIDTH, vp);
            let sub_menu = ContextMenu {
                id: WidgetId::new("tui-submenu"),
                items: sub_items.clone(),
                selected_idx: selected_at_depth.get(depth).copied().unwrap_or(0),
                bg: None,
                placement: ContextMenuPlacement::AnchorPoint,
                glyphs: root_menu.glyphs.clone(),
            };
            let sub_layout = sub_menu.layout(anchor.x, anchor.y, vp, MENU_WIDTH, |_| {
                ContextMenuItemMeasure::new(1.0)
            });
            draw_context_menu(buf, &sub_menu, &sub_layout, theme);
            parent_items = sub_items;
            parent_layout = sub_layout;
        }
    }

    fn item(label: &str, clickable: bool) -> ContextMenuItem {
        ContextMenuItem {
            id: if clickable {
                Some(WidgetId::new(label))
            } else {
                None
            },
            label: StyledText {
                spans: vec![StyledSpan::plain(label)],
            },
            disabled: !clickable,
            ..Default::default()
        }
    }

    fn make_menu() -> ContextMenu {
        ContextMenu {
            id: WidgetId::new("menu"),
            items: vec![
                item("Open", true),
                item("Open to Side", true),
                // Separator: id = None.
                ContextMenuItem::default(),
                item("Delete", true),
            ],
            selected_idx: 0,
            bg: None,
            placement: crate::primitives::context_menu::ContextMenuPlacement::default(),
            glyphs: Default::default(),
        }
    }

    fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
        buf[(x, y)].symbol().chars().next().unwrap_or(' ')
    }

    #[test]
    fn paints_corner_glyphs_and_items() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let menu = make_menu();
        let layout = menu.layout(
            2.0,
            1.0,
            crate::event::Rect::new(0.0, 0.0, 30.0, 10.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());

        // Border corners around the inner items region (inset by 1).
        let bx = layout.bounds.x.round() as u16 - 1;
        let by = layout.bounds.y.round() as u16 - 1;
        let bw = layout.bounds.width.round() as u16 + 2;
        let bh = layout.bounds.height.round() as u16 + 2;
        assert_eq!(cell_char(&buf, bx, by), '┌');
        assert_eq!(cell_char(&buf, bx + bw - 1, by), '┐');
        assert_eq!(cell_char(&buf, bx, by + bh - 1), '└');
        assert_eq!(cell_char(&buf, bx + bw - 1, by + bh - 1), '┘');
    }

    /// An app-supplied [`ContextMenuGlyphs`] override swaps the
    /// checked-item check-mark and submenu pull-right arrow; with no
    /// override, [`draw_context_menu_with_glyphs`] (what
    /// [`draw_context_menu`] always calls) paints the same `✓`/`▶`
    /// glyphs as today's default.
    #[test]
    fn context_menu_glyphs_override_swaps_checked_and_arrow_default_keeps_defaults() {
        let mut checked_item = item("Word Wrap", true);
        checked_item.checked = Some(true);
        let mut submenu_item = item("Share", true);
        submenu_item.submenu = Some(vec![item("Email", true)]);
        let menu = ContextMenu {
            id: WidgetId::new("menu"),
            items: vec![checked_item, submenu_item],
            selected_idx: 2, // neither row selected
            bg: None,
            placement: crate::primitives::context_menu::ContextMenuPlacement::default(),
            glyphs: Default::default(),
        };
        let layout = menu.layout(
            2.0,
            1.0,
            crate::event::Rect::new(0.0, 0.0, 30.0, 10.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );

        // Default (no override): '✓' and '▶'.
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        draw_context_menu_with_glyphs(&mut buf, &menu, &layout, &Theme::default(), None, false);
        let mut found_check = false;
        let mut found_arrow = false;
        for y in 0..10 {
            for x in 0..30 {
                match cell_char(&buf, x, y) {
                    '✓' => found_check = true,
                    '▶' => found_arrow = true,
                    _ => {}
                }
            }
        }
        assert!(found_check, "expected default '✓' check glyph");
        assert!(found_arrow, "expected default '▶' submenu arrow");

        // Override: the app's own glyphs replace the defaults, which
        // then appear nowhere in the buffer.
        let glyphs = ContextMenuGlyphs::new()
            .with_checked_glyph("+")
            .with_submenu_arrow_glyph(">");
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        draw_context_menu_with_glyphs(
            &mut buf,
            &menu,
            &layout,
            &Theme::default(),
            Some(&glyphs),
            true,
        );
        let mut found_plus = false;
        let mut found_gt = false;
        for y in 0..10 {
            for x in 0..30 {
                let ch = cell_char(&buf, x, y);
                assert_ne!(ch, '✓', "default check glyph leaked through override");
                assert_ne!(ch, '▶', "default submenu arrow leaked through override");
                match ch {
                    '+' => found_plus = true,
                    '>' => found_gt = true,
                    _ => {}
                }
            }
        }
        assert!(found_plus, "expected overridden '+' check glyph");
        assert!(found_gt, "expected overridden '>' submenu arrow");
    }

    #[test]
    fn separator_paints_horizontal_dashes() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let menu = make_menu();
        let layout = menu.layout(
            2.0,
            1.0,
            crate::event::Rect::new(0.0, 0.0, 30.0, 10.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());

        // The third visible item is a separator — find a row that's all '─'.
        let mut found_sep_row = false;
        for vis in &layout.visible_items {
            if vis.is_separator {
                let row_y = vis.bounds.y.round() as u16;
                let inner_x = layout.bounds.x.round() as u16;
                let inner_w = layout.bounds.width.round() as u16;
                let row: String = (inner_x..inner_x + inner_w)
                    .map(|x| cell_char(&buf, x, row_y))
                    .collect();
                assert!(row.chars().all(|c| c == '─'), "separator row: {:?}", row);
                found_sep_row = true;
                break;
            }
        }
        assert!(found_sep_row, "expected at least one separator row");
    }

    #[test]
    fn selected_clickable_inverted() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let menu = make_menu(); // selected_idx = 0 → "Open"
        let layout = menu.layout(
            2.0,
            1.0,
            crate::event::Rect::new(0.0, 0.0, 30.0, 10.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        let theme = Theme {
            tab_bar_bg: crate::types::Color::rgb(0, 0, 0),
            foreground: crate::types::Color::rgb(255, 255, 255),
            ..Theme::default()
        };
        draw_context_menu(&mut buf, &menu, &layout, &theme);

        // Find the "Open" row's first cell (inner_x). The selected row has
        // inverted bg = foreground colour.
        let inner_x = layout.bounds.x.round() as u16;
        let row_y = layout.visible_items[0].bounds.y.round() as u16;
        let bg = buf[(inner_x, row_y)].bg;
        assert_eq!(bg, ratatui::style::Color::Rgb(255, 255, 255));
    }

    #[test]
    fn checked_item_paints_check_glyph_in_left_padding() {
        use crate::primitives::context_menu::ContextMenuItem;
        let menu = ContextMenu {
            id: WidgetId::new("menu"),
            items: vec![
                ContextMenuItem {
                    id: Some(WidgetId::new("toggle-sidebar")),
                    label: StyledText {
                        spans: vec![StyledSpan::plain("Toggle Sidebar")],
                    },
                    checked: Some(true),
                    ..Default::default()
                },
                ContextMenuItem {
                    id: Some(WidgetId::new("toggle-panel")),
                    label: StyledText {
                        spans: vec![StyledSpan::plain("Toggle Panel")],
                    },
                    checked: Some(false),
                    ..Default::default()
                },
            ],
            selected_idx: 0,
            bg: None,
            placement: crate::primitives::context_menu::ContextMenuPlacement::default(),
            glyphs: Default::default(),
        };
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let layout = menu.layout(
            2.0,
            1.0,
            crate::event::Rect::new(0.0, 0.0, 30.0, 10.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());
        let inner_x = layout.bounds.x.round() as u16;
        let checked_row = layout.visible_items[0].bounds.y.round() as u16;
        assert_eq!(
            cell_char(&buf, inner_x, checked_row),
            '✓',
            "checked=Some(true) item should paint ✓ at inner_x",
        );
        let unchecked_row = layout.visible_items[1].bounds.y.round() as u16;
        assert_eq!(
            cell_char(&buf, inner_x, unchecked_row),
            ' ',
            "checked=Some(false) item should leave the slot blank",
        );
    }

    #[test]
    fn item_icon_paints_resolved_glyph_in_left_padding() {
        use crate::primitives::context_menu::ContextMenuItem;
        use crate::types::Icon;
        let menu = ContextMenu {
            id: WidgetId::new("menu"),
            items: vec![ContextMenuItem {
                id: Some(WidgetId::new("new-file")),
                label: StyledText {
                    spans: vec![StyledSpan::plain("New File")],
                },
                icon: Some(Icon::new("G", "F")),
                ..Default::default()
            }],
            selected_idx: 0,
            bg: None,
            placement: crate::primitives::context_menu::ContextMenuPlacement::default(),
            glyphs: Default::default(),
        };
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let layout = menu.layout(
            2.0,
            1.0,
            crate::event::Rect::new(0.0, 0.0, 30.0, 10.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());
        let inner_x = layout.bounds.x.round() as u16;
        let row_y = layout.visible_items[0].bounds.y.round() as u16;
        // `draw_context_menu` resolves with `nerd_fonts_enabled = false`,
        // so the icon resolves to its ASCII fallback "F", not the glyph
        // "G". `item_icon_paints_nerd_font_glyph_when_enabled` below pins
        // the `nerd_fonts_enabled = true` path through
        // `draw_context_menu_with_glyphs`, and
        // `widget_contract_paint_resolves_nerd_font_item_icon` pins it
        // through `Primitive::paint_cells`, where the flag comes from the
        // `LayoutCtx`.
        assert_eq!(
            cell_char(&buf, inner_x, row_y),
            'F',
            "item icon should paint its resolved fallback glyph at inner_x",
        );
    }

    #[test]
    fn menu_with_icon_reserves_two_cell_gutter_with_blank_gap() {
        // A menu where any item carries an icon reserves a 2-cell gutter
        // (icon cell + one blank gap cell) for every row, so the icon
        // never runs straight into the label — the "XCut"/"CCopy"
        // overprint the icon column exists to prevent.
        use crate::primitives::context_menu::ContextMenuItem;
        use crate::types::Icon;
        let menu = ContextMenu {
            id: WidgetId::new("menu"),
            items: vec![ContextMenuItem {
                id: Some(WidgetId::new("cut")),
                label: StyledText {
                    spans: vec![StyledSpan::plain("Cut")],
                },
                icon: Some(Icon::new("G", "X")),
                ..Default::default()
            }],
            selected_idx: 0,
            bg: None,
            placement: crate::primitives::context_menu::ContextMenuPlacement::default(),
            glyphs: Default::default(),
        };
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let layout = menu.layout(
            2.0,
            1.0,
            crate::event::Rect::new(0.0, 0.0, 30.0, 10.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());
        let inner_x = layout.bounds.x.round() as u16;
        let row_y = layout.visible_items[0].bounds.y.round() as u16;
        assert_eq!(
            cell_char(&buf, inner_x, row_y),
            'X',
            "icon glyph at inner_x"
        );
        assert_eq!(
            cell_char(&buf, inner_x + 1, row_y),
            ' ',
            "gap cell between icon and label must be blank, not the label's first char"
        );
        assert_eq!(
            cell_char(&buf, inner_x + 2, row_y),
            'C',
            "label starts two cells after inner_x, past the icon + gap"
        );
    }

    #[test]
    fn menu_without_icon_or_checked_keeps_single_cell_left_margin() {
        // A plain menu (no item carries `checked`/`icon`) keeps the
        // original compact 1-cell left margin — the 2-cell gutter is
        // reserved only when the menu actually needs it.
        let menu = ContextMenu {
            id: WidgetId::new("menu"),
            items: vec![ContextMenuItem {
                id: Some(WidgetId::new("about")),
                label: StyledText {
                    spans: vec![StyledSpan::plain("About")],
                },
                ..Default::default()
            }],
            selected_idx: 0,
            bg: None,
            placement: crate::primitives::context_menu::ContextMenuPlacement::default(),
            glyphs: Default::default(),
        };
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let layout = menu.layout(
            2.0,
            1.0,
            crate::event::Rect::new(0.0, 0.0, 30.0, 10.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());
        let inner_x = layout.bounds.x.round() as u16;
        let row_y = layout.visible_items[0].bounds.y.round() as u16;
        assert_eq!(
            cell_char(&buf, inner_x + 1, row_y),
            'A',
            "no icon/checked anywhere in the menu: label keeps the 1-cell margin"
        );
    }

    #[test]
    fn item_icon_paints_nerd_font_glyph_when_enabled() {
        use crate::primitives::context_menu::ContextMenuItem;
        use crate::types::Icon;
        let menu = ContextMenu {
            id: WidgetId::new("menu"),
            items: vec![ContextMenuItem {
                id: Some(WidgetId::new("new-file")),
                label: StyledText {
                    spans: vec![StyledSpan::plain("New File")],
                },
                icon: Some(Icon::new("G", "F")),
                ..Default::default()
            }],
            selected_idx: 0,
            bg: None,
            placement: crate::primitives::context_menu::ContextMenuPlacement::default(),
            glyphs: Default::default(),
        };
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let layout = menu.layout(
            2.0,
            1.0,
            crate::event::Rect::new(0.0, 0.0, 30.0, 10.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        draw_context_menu_with_glyphs(&mut buf, &menu, &layout, &Theme::default(), None, true);
        let inner_x = layout.bounds.x.round() as u16;
        let row_y = layout.visible_items[0].bounds.y.round() as u16;
        assert_eq!(
            cell_char(&buf, inner_x, row_y),
            'G',
            "item icon should paint its Nerd Font glyph when nerd_fonts_enabled is true",
        );
    }

    #[test]
    fn checked_state_wins_shared_slot_over_item_icon() {
        use crate::primitives::context_menu::ContextMenuItem;
        use crate::types::Icon;
        let menu = ContextMenu {
            id: WidgetId::new("menu"),
            items: vec![ContextMenuItem {
                id: Some(WidgetId::new("word-wrap")),
                label: StyledText {
                    spans: vec![StyledSpan::plain("Word Wrap")],
                },
                checked: Some(true),
                icon: Some(Icon::new("G", "F")),
                ..Default::default()
            }],
            selected_idx: 0,
            bg: None,
            placement: crate::primitives::context_menu::ContextMenuPlacement::default(),
            glyphs: Default::default(),
        };
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let layout = menu.layout(
            2.0,
            1.0,
            crate::event::Rect::new(0.0, 0.0, 30.0, 10.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());
        let inner_x = layout.bounds.x.round() as u16;
        let row_y = layout.visible_items[0].bounds.y.round() as u16;
        assert_eq!(
            cell_char(&buf, inner_x, row_y),
            '✓',
            "checked=Some(true) should win the shared slot over the item's own icon",
        );
    }

    #[test]
    fn key_equivalent_renders_as_right_aligned_shortcut() {
        // key_equivalent set but detail unset → shortcut text comes
        // from render_accelerator(Platform::Tui).
        use crate::accelerator::{Accelerator, AcceleratorId, AcceleratorScope, KeyBinding};
        use crate::primitives::context_menu::ContextMenuItem;
        let menu = ContextMenu {
            id: WidgetId::new("menu"),
            items: vec![ContextMenuItem {
                id: Some(WidgetId::new("save")),
                label: StyledText {
                    spans: vec![StyledSpan::plain("Save")],
                },
                key_equivalent: Some(Accelerator {
                    id: AcceleratorId::new("editor.save"),
                    binding: KeyBinding::Save,
                    scope: AcceleratorScope::Global,
                    label: None,
                }),
                ..Default::default()
            }],
            selected_idx: 0,
            bg: None,
            placement: crate::primitives::context_menu::ContextMenuPlacement::default(),
            glyphs: Default::default(),
        };
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let layout = menu.layout(
            2.0,
            1.0,
            crate::event::Rect::new(0.0, 0.0, 30.0, 10.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());
        // Platform::Tui renders Save as "Ctrl+S" — look for that suffix
        // near the right edge of the row.
        let inner_x = layout.bounds.x.round() as u16;
        let inner_w = layout.bounds.width.round() as u16;
        let row_y = layout.visible_items[0].bounds.y.round() as u16;
        let row: String = (inner_x..inner_x + inner_w)
            .map(|x| cell_char(&buf, x, row_y))
            .collect();
        assert!(
            row.contains("Ctrl+S"),
            "row should contain rendered key_equivalent: row was {row:?}",
        );
    }

    #[test]
    fn detail_wins_over_key_equivalent_for_back_compat() {
        // Pre-existing apps that set `detail` directly should keep
        // seeing that text — even when key_equivalent is also set.
        use crate::accelerator::{Accelerator, AcceleratorId, AcceleratorScope, KeyBinding};
        use crate::primitives::context_menu::ContextMenuItem;
        let menu = ContextMenu {
            id: WidgetId::new("menu"),
            items: vec![ContextMenuItem {
                id: Some(WidgetId::new("save")),
                label: StyledText {
                    spans: vec![StyledSpan::plain("Save")],
                },
                detail: Some(StyledText {
                    spans: vec![StyledSpan::plain("⌘S-legacy")],
                }),
                key_equivalent: Some(Accelerator {
                    id: AcceleratorId::new("editor.save"),
                    binding: KeyBinding::Save,
                    scope: AcceleratorScope::Global,
                    label: None,
                }),
                ..Default::default()
            }],
            selected_idx: 0,
            bg: None,
            placement: crate::primitives::context_menu::ContextMenuPlacement::default(),
            glyphs: Default::default(),
        };
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let layout = menu.layout(
            2.0,
            1.0,
            crate::event::Rect::new(0.0, 0.0, 30.0, 10.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());
        let inner_x = layout.bounds.x.round() as u16;
        let inner_w = layout.bounds.width.round() as u16;
        let row_y = layout.visible_items[0].bounds.y.round() as u16;
        let row: String = (inner_x..inner_x + inner_w)
            .map(|x| cell_char(&buf, x, row_y))
            .collect();
        assert!(
            row.contains("legacy"),
            "detail string should win over key_equivalent: row was {row:?}",
        );
    }

    #[test]
    fn zero_size_is_a_no_op() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 10));
        let menu = make_menu();
        let layout = menu.layout(
            0.0,
            0.0,
            crate::event::Rect::new(0.0, 0.0, 0.0, 0.0),
            0.0,
            |_| ContextMenuItemMeasure::new(0.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());
        assert_eq!(cell_char(&buf, 0, 0), ' ');
    }

    // ── Submenu (▶ affordance + pull-right popup) ────────────────────────

    fn submenu_parent_menu() -> ContextMenu {
        use crate::types::{StyledSpan, StyledText, WidgetId};
        let sub_item = ContextMenuItem {
            id: Some(WidgetId::new("sub-a")),
            label: StyledText {
                spans: vec![StyledSpan::plain("SubItem")],
            },
            ..Default::default()
        };
        ContextMenu {
            id: WidgetId::new("menu"),
            items: vec![
                ContextMenuItem {
                    id: Some(WidgetId::new("parent")),
                    label: StyledText {
                        spans: vec![StyledSpan::plain("Parent")],
                    },
                    submenu: Some(vec![sub_item]),
                    ..Default::default()
                },
                ContextMenuItem {
                    id: Some(WidgetId::new("leaf")),
                    label: StyledText {
                        spans: vec![StyledSpan::plain("Leaf")],
                    },
                    ..Default::default()
                },
            ],
            selected_idx: 0,
            bg: None,
            placement: crate::primitives::context_menu::ContextMenuPlacement::default(),
            glyphs: Default::default(),
        }
    }

    #[test]
    fn submenu_parent_paints_arrow_affordance() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 10));
        let menu = submenu_parent_menu();
        let layout = menu.layout(
            2.0,
            1.0,
            crate::event::Rect::new(0.0, 0.0, 40.0, 10.0),
            16.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());

        // The ▶ appears at the rightmost inner column of the parent item's row.
        let inner_x = layout.bounds.x.round() as u16;
        let inner_w = layout.bounds.width.round() as u16;
        let row_y = layout.visible_items[0].bounds.y.round() as u16;
        assert_eq!(
            cell_char(&buf, inner_x + inner_w - 1, row_y),
            '▶',
            "submenu-parent item should show ▶ at rightmost inner column",
        );

        // A plain leaf item (no submenu) must not show ▶.
        let leaf_row_y = layout.visible_items[1].bounds.y.round() as u16;
        assert_ne!(
            cell_char(&buf, inner_x + inner_w - 1, leaf_row_y),
            '▶',
            "plain leaf item must not show ▶",
        );
    }

    #[test]
    fn submenu_popup_anchored_at_right_edge() {
        // Root menu inner at x=2, width=16.
        // Viewport 50 wide — plenty of room to the right.
        // Expected: child inner left = 2 + 16 + 1 = 19.
        //           child left border (┌) at (18, anchor_y − 1).
        let mut buf = Buffer::empty(Rect::new(0, 0, 50, 10));
        let menu = submenu_parent_menu();
        let viewport = crate::event::Rect::new(0.0, 0.0, 50.0, 10.0);
        let layout = menu.layout(2.0, 1.0, viewport, 16.0, |_| {
            ContextMenuItemMeasure::new(1.0)
        });

        // submenu_path = [0] means item 0 (the submenu-parent) is open.
        // selected_at_depth = [0] → first child selected.
        draw_context_menu_with_submenus(
            &mut buf,
            &menu,
            &layout,
            &[0],
            &[0],
            viewport,
            &Theme::default(),
        );

        // Root menu ┌ is at (inner_x − 1, inner_y − 1) = (1, 0).
        assert_eq!(cell_char(&buf, 1, 0), '┌', "root menu top-left corner");

        // Child menu: preferred_x = 2 + 16 + 1 = 19 (inner left).
        // Child left border (┌) at inner_x − 1 = 18.
        // anchor_y = bounds.y of visible_items[0] = 1.0
        // by = anchor_y − 1 = 0.
        assert_eq!(
            cell_char(&buf, 18, 0),
            '┌',
            "child popup top-left corner should be at x=18 (right-edge anchor)",
        );
    }

    #[test]
    fn submenu_overflow_flips_left() {
        // Viewport 50 wide.  Root menu inner at x=25, width=10.
        // preferred_x = 25 + 10 + 1 = 36; 36 + 20 = 56 > 50 → overflow.
        // flipped_x   = 25 − 20 − 1 = 4 ≥ 0 → use 4.
        // Child inner left = 4 → child ┌ at (3, anchor_y − 1).
        let mut buf = Buffer::empty(Rect::new(0, 0, 50, 10));
        let menu = submenu_parent_menu();
        let viewport = crate::event::Rect::new(0.0, 0.0, 50.0, 10.0);
        let layout = menu.layout(25.0, 1.0, viewport, 10.0, |_| {
            ContextMenuItemMeasure::new(1.0)
        });

        draw_context_menu_with_submenus(
            &mut buf,
            &menu,
            &layout,
            &[0],
            &[0],
            viewport,
            &Theme::default(),
        );

        // anchor_y = 1.0, child border-top at y = 0.
        assert_eq!(
            cell_char(&buf, 3, 0),
            '┌',
            "flipped child popup top-left corner should be at x=3 (left-flip)",
        );
    }

    #[test]
    fn top_border_survives_when_menu_anchored_at_viewport_top_row() {
        // Anchored at y=0 (e.g. a right-click on a tab bar rendered at the
        // very top row of the viewport). Callers lay a menu out inside
        // the widget contract's `ContextMenuPrimitiveLayout::viewport` —
        // the surface inset by one cell on every side on TUI, where the
        // box border sits outside the item region. This test passes that inset surface directly, so
        // `layout_at`'s own clamping puts `bounds.y` on row 1 and leaves
        // row 0 free for the border.
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let menu = make_menu();
        let layout = menu.layout(
            2.0,
            0.0,
            crate::event::Rect::new(1.0, 1.0, 29.0, 9.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());

        let inner_x = layout.bounds.x.round() as u16;
        let bx = inner_x - 1;
        // Top border must still be drawn, and on a different row than the
        // first item's label.
        assert_eq!(
            cell_char(&buf, bx, 0),
            '┌',
            "top-left corner must be visible when anchored at y=0"
        );
        assert_eq!(
            cell_char(&buf, inner_x + 1, 1),
            'O',
            "first item (\"Open\") must render one row below the top border"
        );
    }

    #[test]
    fn left_border_survives_when_menu_anchored_at_viewport_left_edge() {
        // Symmetric case: anchored at x=0 inside the same inset surface
        // (see the top-row test above), so `layout_at` puts `bounds.x` on
        // column 1, leaving column 0 free for the border.
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let menu = make_menu();
        let layout = menu.layout(
            0.0,
            1.0,
            crate::event::Rect::new(1.0, 1.0, 29.0, 9.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());

        let row_y = layout.visible_items[0].bounds.y.round() as u16;
        assert_eq!(
            cell_char(&buf, 0, row_y),
            '│',
            "left border must be visible when anchored at x=0"
        );
        assert_eq!(
            cell_char(&buf, 2, row_y),
            'O',
            "first item (\"Open\") label must still start after the left border"
        );
    }

    #[test]
    fn clicks_land_on_painted_row_when_menu_anchored_at_viewport_corner() {
        // Paint↔click round-trip: a menu anchored flush against the
        // top-left corner of the border-inset viewport must paint
        // its first item on the exact row/column `hit_test` reports for
        // that item's `WidgetId`. The border occupies the row and column
        // outside `bounds`, so neither moves.
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let menu = make_menu();
        let layout = menu.layout(
            0.0,
            0.0,
            crate::event::Rect::new(1.0, 1.0, 29.0, 9.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());

        let first = &layout.visible_items[0];
        let item_x = first.bounds.x;
        let row_y = first.bounds.y;

        // Sanity: the painted label starts one column right of the item's
        // left edge (the check-mark/padding column), on the item's row.
        assert_eq!(
            cell_char(&buf, item_x.round() as u16 + 1, row_y.round() as u16),
            'O',
            "\"Open\" must be painted at the row/column its own bounds report"
        );

        // The border occupies the row/column directly outside `bounds` —
        // hit-testing that cell must miss the menu, not resolve to the item.
        assert_eq!(
            layout.hit_test(item_x - 1.0, row_y - 1.0),
            ContextMenuHit::Empty,
            "the border cell must not be classified as the first item"
        );

        // Hit-testing the exact cell the item was painted at must resolve
        // to that item's id: reserving the border in the viewport the
        // layout is built from is what keeps paint and hit-test in one
        // coordinate space.
        assert_eq!(
            layout.hit_test(item_x, row_y),
            ContextMenuHit::Item(WidgetId::new("Open")),
            "clicking the painted row of the first item must hit \"Open\""
        );
    }

    /// Row `y` of `buf`, as a string, for substring assertions.
    fn buf_row(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| cell_char(buf, x, y)).collect()
    }

    #[test]
    fn zero_origin_layout_paints_without_overflow() {
        // A caller that lays the menu out in the raw viewport instead of
        // the border-inset one gets an item region on column
        // and row 0, so the near border has no cell inside the buffer to
        // occupy. That must stay a cosmetic degrade: the `u16` border
        // arithmetic may not underflow (a panic under `overflow-checks`,
        // a wrap to 65535 without), and every item must still paint on
        // the row and column its own bounds report.
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        let menu = make_menu();
        let layout = menu.layout(
            0.0,
            0.0,
            crate::event::Rect::new(0.0, 0.0, 30.0, 10.0),
            20.0,
            |_| ContextMenuItemMeasure::new(1.0),
        );
        assert_eq!(
            (
                layout.bounds.x.round() as u16,
                layout.bounds.y.round() as u16
            ),
            (0, 0),
            "test precondition: a non-inset viewport puts the item region at the origin"
        );

        draw_context_menu(&mut buf, &menu, &layout, &Theme::default());

        let first = &layout.visible_items[0];
        assert_eq!(
            cell_char(
                &buf,
                first.bounds.x.round() as u16 + 1,
                first.bounds.y.round() as u16
            ),
            'O',
            "\"Open\" must paint at the row/column its own bounds report:\n{}",
            buf_row(&buf, first.bounds.y.round() as u16)
        );
        assert_eq!(
            layout.hit_test(first.bounds.x + 1.0, first.bounds.y),
            ContextMenuHit::Item(WidgetId::new("Open")),
            "paint and hit-test must agree even when the border has no cell"
        );

        // The edges that do have room are unaffected: the border still
        // closes on the right and the bottom.
        let right = layout.bounds.width.round() as u16;
        let bottom = layout.bounds.height.round() as u16;
        assert_eq!(
            cell_char(&buf, right, bottom),
            '┘',
            "bottom-right corner must still be painted"
        );
    }

    #[test]
    fn submenu_child_keeps_its_border_at_zero_origin_viewport() {
        // Same non-inset viewport, but through the submenu rasteriser:
        // the parent sits at the origin, so the child's own `layout`
        // would clamp to row 0 too. `draw_context_menu_with_submenus`
        // insets the viewport it lays children out in, which keeps the
        // child's chrome on-screen; either way the `u16` arithmetic must
        // not overflow and the child's items must still render.
        let mut buf = Buffer::empty(Rect::new(0, 0, 50, 10));
        let menu = submenu_parent_menu();
        let viewport = crate::event::Rect::new(0.0, 0.0, 50.0, 10.0);
        let layout = menu.layout(0.0, 0.0, viewport, 16.0, |_| {
            ContextMenuItemMeasure::new(1.0)
        });
        assert_eq!(
            layout.bounds.y.round() as u16,
            0,
            "test precondition: the parent menu's item region starts on row 0"
        );

        draw_context_menu_with_submenus(
            &mut buf,
            &menu,
            &layout,
            &[0],
            &[0],
            viewport,
            &Theme::default(),
        );

        let screen: String = (0..buf.area.height)
            .map(|y| buf_row(&buf, y))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            screen.contains("SubItem"),
            "the child menu's item must render:\n{screen}"
        );
        assert!(
            screen.lines().any(|line| {
                matches!((line.find('┌'), line.rfind('┐')), (Some(l), Some(r)) if r > l)
            }),
            "the child popup's top border must be fully on-screen:\n{screen}"
        );
    }

    #[test]
    fn submenu_left_flip_against_left_edge_does_not_overflow() {
        // The left-flip branch subtracts the child width from the
        // parent's left edge, which goes negative when the parent is
        // anchored near column 0 in a narrow viewport. The child is
        // clamped back inside instead of wrapping, and still paints.
        let mut buf = Buffer::empty(Rect::new(0, 0, 24, 8));
        let menu = submenu_parent_menu();
        let viewport = crate::event::Rect::new(0.0, 0.0, 24.0, 8.0);
        let layout = menu.layout(1.0, 0.0, viewport, 16.0, |_| {
            ContextMenuItemMeasure::new(1.0)
        });

        draw_context_menu_with_submenus(
            &mut buf,
            &menu,
            &layout,
            &[0],
            &[0],
            viewport,
            &Theme::default(),
        );

        let screen: String = (0..buf.area.height)
            .map(|y| buf_row(&buf, y))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            screen.contains("SubItem"),
            "the flipped child menu's item must render inside the buffer:\n{screen}"
        );
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
            glyphs: Default::default(),
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
            glyphs: Default::default(),
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
    fn context_menu_layout_anchor_point_near_bottom_fits_fully_in_viewport() {
        // A right-click 10px above a 600px-tall window's bottom edge, on
        // a 7-item menu that would overflow it by 130px if the box
        // simply opened downward from the click. `AnchorPoint` has no
        // directional flip of its own, but `Anchor::resolve`'s vertical
        // clamp pulls the top edge up until the box's bottom edge lands
        // flush with the viewport — functionally the same "stay fully
        // on screen" outcome a `Below`→`Above` flip gives dropdown-style
        // placements, just reached by clamping instead of flipping.
        let menu = ContextMenu {
            id: WidgetId::new("m"),
            items: (0..7).map(|i| cm_action(&format!("i{i}"), "X")).collect(),
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::AnchorPoint,
            glyphs: Default::default(),
        };
        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
        let layout = menu.layout(100.0, 590.0, viewport, 160.0, |_| {
            ContextMenuItemMeasure::new(20.0)
        });
        assert_eq!(
            layout.bounds.height, 140.0,
            "all 7 rows should fit, none clipped"
        );
        assert_eq!(
            layout.bounds.y, 460.0,
            "top pulled up so bounds.y + height == 600"
        );
        assert!(
            layout.bounds.y + layout.bounds.height <= viewport.y + viewport.height,
            "menu bottom edge must stay inside the viewport: {:?}",
            layout.bounds
        );
    }

    #[test]
    fn context_menu_layout_anchor_point_near_bottom_right_corner_clamps_both_axes() {
        // Right-click in the bottom-right corner: both the horizontal
        // and vertical clamps must apply together, independently of
        // each other — neither one masks the other.
        let menu = ContextMenu {
            id: WidgetId::new("m"),
            items: (0..7).map(|i| cm_action(&format!("i{i}"), "X")).collect(),
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::AnchorPoint,
            glyphs: Default::default(),
        };
        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
        let layout = menu.layout(780.0, 590.0, viewport, 160.0, |_| {
            ContextMenuItemMeasure::new(20.0)
        });
        assert_eq!(layout.bounds.x, 640.0, "right edge clamped: 800 - 160");
        assert_eq!(layout.bounds.y, 460.0, "bottom edge clamped: 600 - 140");
        assert!(layout.bounds.x >= viewport.x);
        assert!(layout.bounds.x + layout.bounds.width <= viewport.x + viewport.width);
        assert!(layout.bounds.y >= viewport.y);
        assert!(layout.bounds.y + layout.bounds.height <= viewport.y + viewport.height);
    }

    #[test]
    fn context_menu_layout_shifts_left_when_overflow() {
        let menu = ContextMenu {
            id: WidgetId::new("m"),
            items: vec![cm_action("a", "A")],
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::default(),
            glyphs: Default::default(),
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
            glyphs: Default::default(),
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
            glyphs: Default::default(),
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
            glyphs: Default::default(),
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
            glyphs: Default::default(),
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
            glyphs: Default::default(),
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
            glyphs: Default::default(),
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
            glyphs: Default::default(),
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
            glyphs: Default::default(),
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
            glyphs: Default::default(),
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

    // ── shortcut_text ─────────────────────────────────────────────────────

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
            super::shortcut_text(&item, crate::accelerator::Platform::Linux).expect("detail wins");
        assert_eq!(shortcut, "legacy-string");
    }

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
        let shortcut = super::shortcut_text(&item, crate::accelerator::Platform::Macos)
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

        /// [`Primitive::paint`] takes no glyph argument, so a pixel
        /// surface can only reach an app's own marks through
        /// [`ContextMenu::glyphs`] — and resolves them at the Nerd Font
        /// half the [`LayoutCtx`] reports.
        #[test]
        fn descriptor_glyphs_reach_the_pixel_widget_contract_paint() {
            let mut item = cm_action("a", "Save");
            item.checked = Some(true);
            let (mut m, _) = popup_layout(item);
            m.glyphs =
                ContextMenuGlyphs::new().with_checked_glyph(crate::types::Icon::new("V", "v"));

            let paint = |nerd: bool| {
                let ctx = LayoutCtx::new(20.0, 8.0, nerd);
                let layout = Primitive::layout(&m, &ctx, Rect::new(0.0, 0.0, 200.0, 100.0));
                let mut surface = RecordingSurface::default();
                Primitive::paint(
                    &m,
                    &layout,
                    &mut surface,
                    &Theme::default(),
                    &Style::default(),
                );
                surface.runs
            };
            assert!(
                paint(true).iter().any(|r| r == "V"),
                "the descriptor's Nerd Font check glyph must reach the pixel painter"
            );
            assert!(
                paint(false).iter().any(|r| r == "v"),
                "with no Nerd Font face the descriptor's fallback must paint instead"
            );
            let default_codicon = crate::codicon::CHECK.to_string();
            assert!(!paint(true).iter().any(|r| r == &default_codicon));
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

        /// The issue's icon-column acceptance bullet: when a menu has an
        /// icon item, every label (including a plain sibling with no
        /// icon of its own) indents past the reserved icon column
        /// instead of starting at the same `row_padding_x` offset the
        /// icon glyph itself paints at — the overlap that produced
        /// `✂Cut`/`⎘Copy` in the capture this test pins against.
        #[test]
        fn icon_item_reserves_column_so_label_does_not_overlap_it() {
            let mut cut = cm_action("cut", "Cut");
            cut.icon = Some(crate::types::Icon::new("G", "X"));
            let plain = cm_action("about", "About");
            let m = menu(vec![cut, plain]);
            let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);
            let layout = m.layout(10.0, 10.0, viewport, 160.0, |_| {
                ContextMenuItemMeasure::new(20.0)
            });
            let row = layout.visible_items[0].bounds;

            let style = Style::default();
            let mut surface = RecordingSurface::default();
            native_surface_paint::paint(
                &m,
                &layout,
                Platform::Linux,
                &mut surface,
                &Theme::default(),
                &style,
            );

            let (icon_rect, _) = surface
                .texts
                .iter()
                .find(|(_, t)| t == "X")
                .expect("icon glyph painted");
            assert_eq!(
                icon_rect.x,
                row.x + style.row_padding_x,
                "icon glyph paints at the row's own padding offset"
            );

            let (cut_label_rect, _) = surface
                .texts
                .iter()
                .find(|(_, t)| t == "Cut")
                .expect("Cut label painted");
            let icon_right = icon_rect.x + icon_rect.width;
            assert!(
                cut_label_rect.x >= icon_right,
                "label must start no earlier than the icon's right edge: \
                 label.x={}, icon_right={}",
                cut_label_rect.x,
                icon_right
            );
            assert!(
                cut_label_rect.x > row.x + style.row_padding_x,
                "label must indent past the plain row_padding_x offset when an icon \
                 column is reserved"
            );

            // The plain sibling (no icon of its own) still indents by the
            // same reserved column — the menu reserves one shared gutter,
            // not a per-item one.
            let (about_label_rect, _) = surface
                .texts
                .iter()
                .find(|(_, t)| t == "About")
                .expect("About label painted");
            assert_eq!(
                about_label_rect.x, cut_label_rect.x,
                "every label in the menu indents by the same reserved gutter width"
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

/// The widget contract's own unit coverage: [`Primitive::layout`]'s
/// border reservation and row pitch, [`Primitive::measure`], and every
/// key/pointer gesture [`Interactive::handle`] resolves.
#[cfg(test)]
mod widget_contract_tests {
    use super::*;
    use crate::interactive::EventHost;
    use crate::types::Modifiers;
    use crate::{PointerShape, UiEvent};

    /// Records what `handle` asked the host for, so a test can assert on
    /// the focus request a click makes.
    #[derive(Default)]
    struct RecordingHost {
        focused: Vec<Option<SubTarget>>,
    }

    impl EventHost for RecordingHost {
        fn is_focused(&self, _id: &WidgetId, _sub: Option<SubTarget>) -> bool {
            false
        }
        fn request_focus(&mut self, _id: &WidgetId, sub: Option<SubTarget>) {
            self.focused.push(sub);
        }
        fn capture_pointer(&mut self, _id: &WidgetId) {}
        fn release_pointer(&mut self) {}
        fn set_cursor(&mut self, _cursor: PointerShape) {}
        fn request_redraw(&mut self) {}
        fn emit(&mut self, _event: UiEvent) {}
        fn units(&self) -> Units {
            Units::Cells
        }
    }

    fn leaf(id: &str, label: &str) -> ContextMenuItem {
        ContextMenuItem {
            id: Some(WidgetId::new(id)),
            label: StyledText::plain(label),
            ..Default::default()
        }
    }

    /// Copy / Paste / ─── / Delete (disabled) / Share ▶.
    fn menu() -> ContextMenu {
        ContextMenu {
            id: WidgetId::new("context_menu:editor"),
            items: vec![
                leaf("copy", "Copy"),
                leaf("paste", "Paste"),
                ContextMenuItem::default(),
                ContextMenuItem {
                    disabled: true,
                    ..leaf("delete", "Delete")
                },
                ContextMenuItem {
                    submenu: Some(vec![leaf("email", "Email")]),
                    ..leaf("share", "Share")
                },
            ],
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::AnchorPoint,
            glyphs: Default::default(),
        }
    }

    const AREA: Rect = Rect::new(2.0, 1.0, 22.0, 8.0);

    fn cell_ctx() -> LayoutCtx<'static> {
        LayoutCtx::new(1.0, 1.0, false)
    }

    fn resolved(menu: &ContextMenu) -> ContextMenuPrimitiveLayout {
        Primitive::layout(menu, &cell_ctx(), AREA)
    }

    fn key(key: Key) -> KeyEvent {
        KeyEvent {
            key,
            modifiers: Modifiers::default(),
            repeat: false,
        }
    }

    fn press(menu: &ContextMenu, event: KeyEvent) -> Handled<ContextMenuEvent> {
        let layout = resolved(menu);
        let mut state = ContextMenuState::default();
        let mut host = RecordingHost::default();
        let mut cx = EventCx::new(menu.id, &mut host);
        Interactive::handle(menu, &mut state, &RoutedEvent::Key(event), &layout, &mut cx)
    }

    fn pointer(menu: &ContextMenu, kind: PointerKind, x: f32, y: f32) -> Handled<ContextMenuEvent> {
        let layout = resolved(menu);
        let mut state = ContextMenuState::default();
        let mut host = RecordingHost::default();
        let mut cx = EventCx::new(menu.id, &mut host);
        Interactive::handle(
            menu,
            &mut state,
            &RoutedEvent::Pointer {
                kind,
                sub: None,
                local: Point::new(x, y),
                mods: Modifiers::default(),
            },
            &layout,
            &mut cx,
        )
    }

    #[test]
    fn a_cell_layout_reserves_one_cell_of_border_on_all_four_sides() {
        let layout = resolved(&menu());
        assert_eq!(layout.border_inset, CELL_BORDER_INSET);
        assert_eq!(layout.viewport, Rect::new(3.0, 2.0, 20.0, 6.0));
        assert_eq!(layout.layout.bounds.x, 3.0);
        assert_eq!(layout.layout.bounds.y, 2.0);
        assert_eq!(layout.layout.bounds.width, 20.0);
        // Five rows, one cell each, all inside the inner region.
        assert_eq!(layout.item_height, 1.0);
        assert_eq!(layout.separator_height, 1.0);
        assert_eq!(layout.layout.visible_items.len(), 5);
        assert_eq!(layout.page(), 5);
    }

    #[test]
    fn a_pixel_layout_reserves_no_border_and_uses_the_taller_row_pitch() {
        let ctx = LayoutCtx::new(20.0, 8.0, false);
        let layout = Primitive::layout(&menu(), &ctx, Rect::new(0.0, 0.0, 240.0, 200.0));
        assert_eq!(layout.border_inset, 0.0);
        assert_eq!(layout.viewport, Rect::new(0.0, 0.0, 240.0, 200.0));
        let (item_h, sep_h) = item_heights(20.0);
        assert_eq!(layout.item_height, item_h);
        assert_eq!(layout.separator_height, sep_h);
        assert_eq!(layout.layout.visible_items[0].bounds.height, item_h);
        assert_eq!(layout.layout.visible_items[2].bounds.height, sep_h);
    }

    #[test]
    fn item_heights_collapse_to_one_row_on_a_cell_grid() {
        assert_eq!(item_heights(1.0), (1.0, 1.0));
    }

    #[test]
    fn measure_reports_the_widest_row_plus_the_border_ring() {
        let menu = menu();
        let natural = Primitive::measure(&menu, &cell_ctx(), Size::new(80.0, 24.0));
        assert_eq!(
            natural.width,
            Some(menu.measure_generic_width(1.0, 0.0) + CELL_BORDER_INSET * 2.0)
        );
        // Five one-cell rows plus a border cell top and bottom.
        assert_eq!(natural.height, Some(7.0));
    }

    #[test]
    fn hit_test_local_resolves_against_the_areas_own_origin() {
        let layout = resolved(&menu());
        assert_eq!(
            layout.hit_test_local(1.0, 1.0),
            ContextMenuHit::Item(WidgetId::new("copy"))
        );
        assert_eq!(
            layout.hit_test_local(1.0, 2.0),
            ContextMenuHit::Item(WidgetId::new("paste"))
        );
        // The separator row and the disabled row are both inert.
        assert_eq!(layout.hit_test_local(1.0, 3.0), ContextMenuHit::Inert);
        assert_eq!(layout.hit_test_local(1.0, 4.0), ContextMenuHit::Inert);
    }

    #[test]
    fn resolve_click_classifies_every_row_shape() {
        let menu = menu();
        let layout = resolved(&menu);
        let at = |row: f32| menu.resolve_click(&layout.layout, 4.0, 2.0 + row);
        assert_eq!(
            at(0.0),
            ContextMenuClick::Item {
                idx: 0,
                id: WidgetId::new("copy")
            }
        );
        assert_eq!(at(2.0), ContextMenuClick::Inert);
        assert_eq!(at(3.0), ContextMenuClick::Inert);
        assert_eq!(at(4.0), ContextMenuClick::Submenu { idx: 4 });
        assert_eq!(at(-2.0), ContextMenuClick::Outside);
    }

    #[test]
    fn a_click_on_a_leaf_activates_it_and_claims_focus() {
        let menu = menu();
        let layout = resolved(&menu);
        let mut state = ContextMenuState::default();
        let mut host = RecordingHost::default();
        let mut cx = EventCx::new(menu.id, &mut host);
        let handled = Interactive::handle(
            &menu,
            &mut state,
            &RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                sub: None,
                local: Point::new(2.0, 2.0),
                mods: Modifiers::default(),
            },
            &layout,
            &mut cx,
        );
        assert_eq!(
            handled,
            Handled::Event(ContextMenuEvent::Activated {
                item: WidgetId::new("paste")
            })
        );
        assert_eq!(host.focused, vec![None]);
    }

    #[test]
    fn a_click_on_a_submenu_parent_asks_for_its_child_popup() {
        assert_eq!(
            pointer(&menu(), PointerKind::Down(MouseButton::Left), 2.0, 5.0),
            Handled::Event(ContextMenuEvent::SubmenuOpened { idx: 4 })
        );
    }

    #[test]
    fn a_click_on_an_inert_row_is_swallowed_not_ignored() {
        assert_eq!(
            pointer(&menu(), PointerKind::Down(MouseButton::Left), 2.0, 3.0),
            Handled::Consumed
        );
    }

    #[test]
    fn hover_moves_the_highlight_and_opens_a_submenu_parent() {
        let menu = menu();
        assert_eq!(
            pointer(&menu, PointerKind::Move, 2.0, 2.0),
            Handled::Event(ContextMenuEvent::SelectionChanged { idx: 1 })
        );
        assert_eq!(
            pointer(&menu, PointerKind::Move, 2.0, 5.0),
            Handled::Event(ContextMenuEvent::SubmenuOpened { idx: 4 })
        );
        // Already on the highlighted row: owned, but nothing to report.
        assert_eq!(
            pointer(&menu, PointerKind::Move, 2.0, 1.0),
            Handled::Consumed
        );
    }

    #[test]
    fn down_and_up_skip_the_separator_and_the_disabled_row() {
        let mut menu = menu();
        menu.selected_idx = 1;
        assert_eq!(
            press(&menu, key(Key::Named(NamedKey::Down))),
            Handled::Event(ContextMenuEvent::SelectionChanged { idx: 4 })
        );
        menu.selected_idx = 4;
        assert_eq!(
            press(&menu, key(Key::Named(NamedKey::Up))),
            Handled::Event(ContextMenuEvent::SelectionChanged { idx: 1 })
        );
    }

    #[test]
    fn navigation_wraps_at_both_ends() {
        let mut menu = menu();
        menu.selected_idx = 4;
        assert_eq!(
            press(&menu, key(Key::Named(NamedKey::Down))),
            Handled::Event(ContextMenuEvent::SelectionChanged { idx: 0 })
        );
        menu.selected_idx = 0;
        assert_eq!(
            press(&menu, key(Key::Named(NamedKey::Up))),
            Handled::Event(ContextMenuEvent::SelectionChanged { idx: 4 })
        );
    }

    #[test]
    fn home_and_end_land_on_the_first_and_last_selectable_rows() {
        let mut menu = menu();
        menu.selected_idx = 1;
        assert_eq!(
            press(&menu, key(Key::Named(NamedKey::End))),
            Handled::Event(ContextMenuEvent::SelectionChanged { idx: 4 })
        );
        menu.selected_idx = 4;
        assert_eq!(
            press(&menu, key(Key::Named(NamedKey::Home))),
            Handled::Event(ContextMenuEvent::SelectionChanged { idx: 0 })
        );
    }

    #[test]
    fn a_page_key_moves_by_the_rows_on_screen() {
        let menu = menu();
        // Five rows fit the box, so one page forward from the first
        // selectable row runs to the last one.
        assert_eq!(
            press(&menu, key(Key::Named(NamedKey::PageDown))),
            Handled::Event(ContextMenuEvent::SelectionChanged { idx: 4 })
        );
    }

    #[test]
    fn navigating_from_a_separator_selection_steps_to_a_neighbouring_row() {
        let mut menu = menu();
        menu.selected_idx = 2; // the separator
        assert_eq!(
            press(&menu, key(Key::Named(NamedKey::Down))),
            Handled::Event(ContextMenuEvent::SelectionChanged { idx: 4 })
        );
        assert_eq!(
            press(&menu, key(Key::Named(NamedKey::Up))),
            Handled::Event(ContextMenuEvent::SelectionChanged { idx: 1 })
        );
    }

    #[test]
    fn typing_jumps_the_highlight_to_the_best_prefix_match() {
        let menu = menu();
        assert_eq!(
            press(&menu, key(Key::Char('s'))),
            Handled::Event(ContextMenuEvent::SelectionChanged { idx: 4 })
        );
        // Typeahead never lands on a separator or a disabled row: "d"
        // only matches the disabled "Delete".
        assert_eq!(press(&menu, key(Key::Char('d'))), Handled::Consumed);
    }

    #[test]
    fn a_typeahead_buffer_accumulates_within_its_debounce_window() {
        let menu = menu();
        let layout = resolved(&menu);
        let mut state = ContextMenuState::default();
        // "p" matches Paste; "pa" still does, so the second keystroke
        // reports nothing new rather than jumping elsewhere.
        assert_eq!(
            menu.handle_key(&key(Key::Char('p')), layout.page(), &mut state),
            Handled::Event(ContextMenuEvent::SelectionChanged { idx: 1 })
        );
        let mut on_paste = menu.clone();
        on_paste.selected_idx = 1;
        assert_eq!(
            on_paste.handle_key(&key(Key::Char('a')), layout.page(), &mut state),
            Handled::Consumed
        );
    }

    #[test]
    fn a_modified_keystroke_is_an_accelerator_not_typeahead() {
        let menu = menu();
        let event = KeyEvent {
            key: Key::Char('s'),
            modifiers: Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
            repeat: false,
        };
        assert_eq!(press(&menu, event), Handled::Ignored);
    }

    #[test]
    fn enter_activates_a_leaf_and_opens_a_submenu_parent() {
        let mut menu = menu();
        assert_eq!(
            press(&menu, key(Key::Named(NamedKey::Enter))),
            Handled::Event(ContextMenuEvent::Activated {
                item: WidgetId::new("copy")
            })
        );
        menu.selected_idx = 4;
        assert_eq!(
            press(&menu, key(Key::Named(NamedKey::Enter))),
            Handled::Event(ContextMenuEvent::SubmenuOpened { idx: 4 })
        );
    }

    #[test]
    fn right_only_opens_a_submenu_parent() {
        let mut menu = menu();
        assert_eq!(
            press(&menu, key(Key::Named(NamedKey::Right))),
            Handled::Ignored
        );
        menu.selected_idx = 4;
        assert_eq!(
            press(&menu, key(Key::Named(NamedKey::Right))),
            Handled::Event(ContextMenuEvent::SubmenuOpened { idx: 4 })
        );
    }

    #[test]
    fn left_closes_a_level_and_escape_dismisses() {
        let menu = menu();
        assert_eq!(
            press(&menu, key(Key::Named(NamedKey::Left))),
            Handled::Event(ContextMenuEvent::SubmenuClosed)
        );
        assert_eq!(
            press(&menu, key(Key::Named(NamedKey::Escape))),
            Handled::Event(ContextMenuEvent::Dismissed)
        );
    }

    #[test]
    fn enter_on_an_inert_row_is_swallowed() {
        let mut menu = menu();
        menu.selected_idx = 2;
        assert_eq!(
            press(&menu, key(Key::Named(NamedKey::Enter))),
            Handled::Consumed
        );
        menu.selected_idx = 3;
        assert_eq!(
            press(&menu, key(Key::Named(NamedKey::Enter))),
            Handled::Consumed
        );
    }

    #[test]
    fn an_a11y_click_activates_the_highlighted_row() {
        let menu = menu();
        let layout = resolved(&menu);
        let mut state = ContextMenuState::default();
        let mut host = RecordingHost::default();
        let mut cx = EventCx::new(menu.id, &mut host);
        assert_eq!(
            Interactive::handle(
                &menu,
                &mut state,
                &RoutedEvent::Action(A11yAction::Click),
                &layout,
                &mut cx,
            ),
            Handled::Event(ContextMenuEvent::Activated {
                item: WidgetId::new("copy")
            })
        );
        assert_eq!(
            Interactive::a11y_actions(&menu, &state),
            vec![A11yAction::Click]
        );
    }

    #[test]
    fn an_empty_popup_is_no_focus_stop_and_offers_no_a11y_action() {
        let empty = ContextMenu {
            id: WidgetId::new("context_menu:empty"),
            items: Vec::new(),
            selected_idx: 0,
            bg: None,
            placement: ContextMenuPlacement::AnchorPoint,
            glyphs: Default::default(),
        };
        assert_eq!(Interactive::focus(&empty), FocusSpec::None);
        assert!(Interactive::a11y_actions(&empty, &ContextMenuState::default()).is_empty());
        assert_eq!(Interactive::focus(&menu()), FocusSpec::Stop);
    }

    #[test]
    fn sub_regions_name_every_visible_row_in_local_space() {
        let menu = menu();
        let layout = resolved(&menu);
        let regions = Interactive::sub_regions(&menu, &layout);
        assert_eq!(regions.len(), 5);
        // Row 0 sits one cell in from the area's own origin, inside the
        // border ring.
        assert_eq!(regions[0].0.x, 1.0);
        assert_eq!(regions[0].0.y, 1.0);
        assert_eq!(regions[0].1, SubTarget(0));
        assert_eq!(regions[4].1, SubTarget(4));
    }

    #[test]
    fn each_event_rides_a_ui_event_under_the_right_id() {
        let id = WidgetId::new("context_menu:editor");
        assert_eq!(
            ContextMenuEvent::Activated {
                item: WidgetId::new("copy")
            }
            .into_ui_event(id),
            UiEvent::ContextMenuItemActivated(WidgetId::new("copy"))
        );
        assert_eq!(
            ContextMenuEvent::Dismissed.into_ui_event(id),
            UiEvent::ContextMenuDismissed
        );
        assert_eq!(
            ContextMenuEvent::SubmenuClosed.into_ui_event(id),
            UiEvent::ContextMenuDismissed
        );
        assert_eq!(
            ContextMenuEvent::SelectionChanged { idx: 3 }.into_ui_event(id),
            UiEvent::List(id, crate::ListViewEvent::SelectionChanged { idx: 3 })
        );
        assert_eq!(
            ContextMenuEvent::SubmenuOpened { idx: 4 }.into_ui_event(id),
            UiEvent::List(id, crate::ListViewEvent::ItemActivated { idx: 4 })
        );
    }

    #[test]
    fn capture_round_trips_the_popup_as_its_own_owned_content() {
        let menu = menu();
        match Primitive::capture(&menu) {
            crate::frame::OwnedContent::ContextMenu(captured) => assert_eq!(captured, menu),
            other => panic!("expected OwnedContent::ContextMenu, got {other:?}"),
        }
    }

    /// The host's live Nerd Font flag rides the resolved layout, so both
    /// `paint` entry points resolve a glyph the same way the surrounding
    /// chrome does.
    #[test]
    fn layout_carries_the_ctxs_nerd_font_flag() {
        assert!(!resolved(&menu()).nerd_fonts_enabled);
        let on = LayoutCtx::new(1.0, 1.0, true);
        assert!(Primitive::layout(&menu(), &on, AREA).nerd_fonts_enabled);
    }

    /// [`ContextMenu::glyphs`] round-trips through the descriptor's own
    /// wire shape: an absent key means "no override", and an override
    /// deserialises back to the same registry.
    #[test]
    fn glyphs_default_when_absent_from_the_wire_shape() {
        let bare: ContextMenu = serde_json::from_value(
            serde_json::json!({ "id": "m", "items": [], "selected_idx": 0 }),
        )
        .unwrap();
        assert_eq!(bare.glyphs, ContextMenuGlyphs::new());

        let overridden: ContextMenu = serde_json::from_value(serde_json::json!({
            "id": "m",
            "items": [],
            "selected_idx": 0,
            "glyphs": { "checked": { "glyph": "V", "fallback": "v" } },
        }))
        .unwrap();
        assert_eq!(
            overridden.glyphs,
            ContextMenuGlyphs::new().with_checked_glyph(Icon::new("V", "v"))
        );
    }

    /// The whole point of the descriptor field: a menu carrying its own
    /// [`ContextMenu::glyphs`] paints those marks through
    /// [`Primitive::paint_cells`], which takes no glyph argument.
    #[cfg(feature = "tui")]
    #[test]
    fn paint_cells_paints_the_descriptors_glyph_overrides() {
        let checked_row = ContextMenuItem {
            checked: Some(true),
            ..leaf("copy", "Copy")
        };
        let mut menu = menu();
        menu.items[0] = checked_row;
        menu.glyphs = ContextMenuGlyphs::new()
            .with_checked_glyph("V")
            .with_submenu_arrow_glyph(">");

        let layout = resolved(&menu);
        let mut buf = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 30, 10));
        Primitive::paint_cells(&menu, &layout, &mut buf, &crate::theme::Theme::default());

        let inner_x = layout.layout.bounds.x.round() as u16;
        let right = inner_x + layout.layout.bounds.width.round() as u16;
        let checked_y = layout.layout.visible_items[0].bounds.y.round() as u16;
        let share_y = layout.layout.visible_items[4].bounds.y.round() as u16;
        assert_eq!(
            buf[(inner_x, checked_y)].symbol(),
            "V",
            "the descriptor's checked glyph must reach the cell painter"
        );
        assert_eq!(
            buf[(right - 1, share_y)].symbol(),
            ">",
            "the descriptor's submenu-arrow glyph must reach the cell painter"
        );
    }

    /// An item's own icon resolves to its Nerd Font half when the host's
    /// [`LayoutCtx`] says the face is registered — the input the
    /// widget-contract path takes from the layout rather than hardcoding.
    #[cfg(feature = "tui")]
    #[test]
    fn widget_contract_paint_resolves_nerd_font_item_icon() {
        let mut menu = menu();
        menu.items[0] = ContextMenuItem {
            icon: Some(Icon::new("G", "F")),
            ..leaf("copy", "Copy")
        };
        let theme = crate::theme::Theme::default();

        let glyph_at = |nerd: bool| {
            let ctx = LayoutCtx::new(1.0, 1.0, nerd);
            let layout = Primitive::layout(&menu, &ctx, AREA);
            let mut buf = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 30, 10));
            Primitive::paint_cells(&menu, &layout, &mut buf, &theme);
            let x = layout.layout.bounds.x.round() as u16;
            let y = layout.layout.visible_items[0].bounds.y.round() as u16;
            buf[(x, y)].symbol().to_string()
        };
        assert_eq!(glyph_at(false), "F", "no Nerd Font face: ASCII fallback");
        assert_eq!(glyph_at(true), "G", "Nerd Font face registered: the glyph");
    }

    /// Event parity's in-crate leg: an [`A11yAction::Click`] routed
    /// through [`crate::a11y::route_widget_action`] activates the
    /// highlighted row — the same `UiEvent::ContextMenuItemActivated` a
    /// pointer click on that row produces. The pointer legs (TuiDriver and
    /// the binding host) assert the same event in
    /// `tests/widget_contract_menus_event_parity.rs` and
    /// `quadraui-binding-core`'s engine tests.
    #[test]
    fn route_widget_action_click_matches_the_parity_event() {
        let menu = menu();
        let id = menu.id;
        let mut tree = crate::frame_tree::FrameTree::new();
        tree.push(id, AREA, Box::new(menu));
        let mut states = crate::widget_state::WidgetStateStore::new();
        let events = crate::a11y::route_widget_action(
            &tree,
            &mut states,
            &cell_ctx(),
            id,
            A11yAction::Click,
        );
        assert_eq!(
            events,
            vec![UiEvent::ContextMenuItemActivated(WidgetId::new("copy"))]
        );
    }
}
