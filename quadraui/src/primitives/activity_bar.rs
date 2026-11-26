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
//! **Declarative.** Render the `top_items` from the top of the strip,
//! then the `bottom_items` pinned to the bottom. Click on item → emit
//! `ActivityBarEvent::ItemClicked { id }`.
//!
//! The bar paints through the widget contract
//! ([`crate::backend::DrawsPrimitives::draw_dyn`]). The hovered row and
//! the paint-time [`ActivityBarStyle`] ride on the descriptor itself
//! ([`ActivityBar::hovered`], [`ActivityBar::style`]), so every painter
//! reads them from the one value it is handed.

use crate::behaviour::nav::{FlatNav, NavKey};
use crate::compose::Handled;
use crate::event::{Key, Rect};
use crate::primitives::toolbar::{
    Toolbar, ToolbarButton, ToolbarItemKind, ToolbarItemMeasure, ToolbarLayout,
    ToolbarLayoutOptions, ToolbarOrientation, ToolbarRailStyle,
};
use crate::theme::Theme;
use crate::types::{Color, Icon, Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

/// Declarative description of an activity bar.
///
/// # Examples
///
/// ```
/// use quadraui::{ActivityBar, ActivityBarHit, ActivityItem, Icon, WidgetId};
///
/// let bar = ActivityBar {
///     id: WidgetId::new("activity_bar:main"),
///     top_items: vec![ActivityItem {
///         id: WidgetId::new("activity:explorer"),
///         icon: Icon::new("\u{f114}", "E"),
///         tooltip: "Explorer".to_string(),
///         is_active: true,
///         is_keyboard_selected: false,
///     }],
///     bottom_items: vec![],
///     active_accent: None,
///     selection_bg: None,
///     is_keyboard_focused: false,
///     style: Default::default(),
///     hovered: None,
/// };
///
/// let layout = bar.layout(2.0, 10.0, 1.0);
/// assert_eq!(
///     layout.hit_test(0.0, 0.0),
///     ActivityBarHit::Item(WidgetId::new("activity:explorer"))
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityBar {
    pub id: WidgetId,
    /// Top items rendered starting at the top edge, one row per item.
    pub top_items: Vec<ActivityItem>,
    /// Bottom-pinned items rendered from the bottom edge upward.
    /// Rendered only if there's room after `top_items`.
    #[serde(default)]
    pub bottom_items: Vec<ActivityItem>,
    /// Colour of the left-edge accent bar on active items.
    /// `None` = no accent rendering. Every rasteriser honours this
    /// literally — there is no theme fallback, so `None`
    /// genuinely paints zero accent pixels. See [`ActivityBarStyle`] for
    /// the independent VS-Code-style row-fill knob.
    #[serde(default)]
    pub active_accent: Option<Color>,
    /// Background colour for keyboard-selected items (arrow-nav highlight).
    /// `None` = backends fall back to their own default.
    #[serde(default)]
    pub selection_bg: Option<Color>,
    /// When `true` the bar is accepting keyboard input. The TUI, GTK and
    /// macOS backends intercept key events and emit
    /// [`ActivityBarEvent::KeyPressed`] (wrapped in
    /// [`crate::UiEvent::ActivityBar`]) rather than forwarding them as raw
    /// [`crate::UiEvent::KeyPressed`] events.
    ///
    /// **GTK note**: the runner already attaches its `EventControllerKey` to
    /// the window (not a sibling `DrawingArea`), so setting this flag does
    /// *not* call `grab_focus()` on any widget and does not interfere with
    /// other key controllers.
    ///
    /// Navigation logic (`j`/`k` move cursor, `l`/`Enter` activate, `Esc`/`h`
    /// dismiss focus) stays in the consumer. The bar delivers every key as
    /// [`ActivityBarEvent::KeyPressed`] so the app can map keys to its own
    /// engine calls (`activity_bar_move_up`, `focus_out`, etc.).
    #[serde(default)]
    pub is_keyboard_focused: bool,
    /// Paint-time style request: the active row's fill colour and the
    /// icon glyph size. [`ActivityBarStyle::default`] paints no fill and
    /// the default icon size.
    #[serde(default)]
    pub style: ActivityBarStyle,
    /// The [`ActivityBarLayout::visible_items`] row the pointer is over,
    /// tinted by every painter. A host fills it from its own hover
    /// tracking (for example [`ActivityBarState::hovered_idx`]).
    #[serde(default)]
    pub hovered: Option<usize>,
}

/// One icon entry in an `ActivityBar`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityItem {
    /// Opaque widget id for click routing. The adapter picks a meaningful
    /// namespaced string (e.g. `"activity:explorer"`, `"activity:ext:foo"`).
    pub id: WidgetId,
    /// Icon to render — `glyph` for a Nerd Font / icon-font variant,
    /// `fallback` for ASCII/basic-Unicode. Which one paints is decided by
    /// the backend's `nerd_fonts_enabled` flag, not by this
    /// primitive. TUI paints a single cell (`glyph.chars().next()` /
    /// `fallback.chars().next()`); GTK can render wider strings when the
    /// font supports them.
    pub icon: Icon,
    /// Hover tooltip text. TUI ignores (no hover UI); GTK uses as a native
    /// `set_tooltip_text`.
    #[serde(default)]
    pub tooltip: String,
    #[serde(default)]
    pub is_active: bool,
    /// Keyboard-focused selection highlight. Both the TUI and GTK rasterizers
    /// honour this flag: TUI applies `Modifier::REVERSED` (or paints
    /// `ActivityBar::selection_bg` when set); GTK fills the row with a
    /// brightened background tint (or `ActivityBar::selection_bg` when set).
    /// Set this on the item whose index matches the app's keyboard cursor while
    /// `ActivityBar::is_keyboard_focused` is `true`.
    #[serde(default)]
    pub is_keyboard_selected: bool,
}

/// Rebuild this activity bar as the vertical [`Toolbar`] of icon buttons
/// it's shaped like: `top_items`, then a [`ToolbarButton::Spacer`], then
/// `bottom_items` — the exact `buttons` order
/// [`ActivityBar::layout`] packs via [`Toolbar::layout_with_options`]
/// with [`ToolbarOrientation::Vertical`], reusing the same
/// trailing-group-wins collision rule this type's own `layout` doc
/// already specified for its bottom-pinned items.
///
/// - `id` carries straight through.
/// - Each [`ActivityItem`] becomes a [`ToolbarButton::Action`]: `icon`
///   is the item's `fallback` half — which glyph half paints
///   (`Icon::glyph` vs `Icon::fallback`) is a paint-time
///   `nerd_fonts_enabled` decision this conversion has no way to see,
///   so a caller that needs the glyph half re-reads `ActivityItem::icon`
///   directly rather than this `Toolbar`'s `icon` field. `is_active`
///   carries through verbatim — a `Toolbar`-path renderer shows it as
///   the same pressed/toggled tint a toggled `ToolbarButton::Action`
///   gets. `enabled` is always `true` (`ActivityItem` has no disabled
///   state). `tooltip` and `id` carry through; `spec` stays `None`.
/// - `focused_index` is the flat `buttons` index of whichever item has
///   `is_keyboard_selected` set (top items first, then the spacer, then
///   bottom items — the same order `buttons` itself is built in), or
///   `None` if no item does.
/// - `bg` stays `None` — picking `theme.tab_bar_bg` instead of a plain
///   `Toolbar`'s own `theme.header_bg` default is a rasteriser-level
///   choice, not a property of this conversion.
///
/// A host that wants `ActivityItem`'s own `A11yRole::Tab` semantics from
/// the resulting `Toolbar` calls
/// [`Toolbar::a11y_children_with_role`]`(`[`ToolbarItemRole::Tab`]`)`.
///
/// [`ToolbarItemRole::Tab`]: crate::primitives::toolbar::ToolbarItemRole::Tab
impl From<&ActivityBar> for Toolbar {
    fn from(bar: &ActivityBar) -> Toolbar {
        let mut buttons = Vec::with_capacity(bar.top_items.len() + 1 + bar.bottom_items.len());
        let mut focused_index = None;
        for item in &bar.top_items {
            if item.is_keyboard_selected {
                focused_index = Some(buttons.len());
            }
            buttons.push(activity_item_to_toolbar_button(item));
        }
        buttons.push(ToolbarButton::Spacer);
        for item in &bar.bottom_items {
            if item.is_keyboard_selected {
                focused_index = Some(buttons.len());
            }
            buttons.push(activity_item_to_toolbar_button(item));
        }
        Toolbar::new(bar.id)
            .with_buttons(buttons)
            .with_focused_index(focused_index)
    }
}

fn activity_item_to_toolbar_button(item: &ActivityItem) -> ToolbarButton {
    activity_item_to_toolbar_button_with_icon(item, item.icon.fallback.clone())
}

fn activity_item_to_toolbar_button_with_icon(item: &ActivityItem, icon: String) -> ToolbarButton {
    ToolbarButton::Action {
        id: item.id,
        label: String::new(),
        icon: Some(icon),
        key_hint: None,
        enabled: true,
        is_active: item.is_active,
        tooltip: item.tooltip.clone(),
        spec: None,
    }
}

/// Paint-state-preserving twin of `impl From<&ActivityBar> for Toolbar`:
/// the same `top_items`, `Spacer`, `bottom_items` button order and
/// `focused_index` derivation, plus the paint state that conversion drops
/// — returned as a [`ToolbarRailStyle`] sidecar rather than new fields on
/// `Toolbar`/`ActivityBar*` (see that type's own doc for why).
///
/// Unlike `From`, which always bakes in `item.icon.fallback`, each item's
/// icon here is chosen by `nerd_fonts_enabled` — `item.icon.glyph` when
/// `true`, `item.icon.fallback` when `false` — the same choice
/// `native_surface_paint::paint` makes per item today.
///
/// Mapping:
/// - `style.active_bg` → [`ToolbarRailStyle::active_bg`].
/// - `self.active_accent` → [`ToolbarRailStyle::active_accent`].
/// - `self.selection_bg` → [`ToolbarRailStyle::selection_bg`].
/// - `theme.separator` → [`ToolbarRailStyle::trailing_separator`]
///   (unconditional — every activity-bar rasteriser paints this column
///   today, so there is no "unset" case to preserve).
/// - the returned `Toolbar`'s `bg` is `theme.tab_bar_bg` (the activity
///   bar's own chrome background), not `None` like the plain `From`
///   conversion — a Toolbar-rail painter that reads `bg` directly gets
///   the right colour without also needing `theme` passed in separately.
///
/// `From<&ActivityBar> for Toolbar` is unchanged and keeps its own
/// `bg: None` / fallback-icon-only behaviour, because
/// [`ActivityBar::layout`] depends on it for geometry only — layout never
/// reads `bg` or any icon string.
///
/// No painter reads the returned `ToolbarRailStyle` yet; this method only
/// carries the state across the conversion boundary.
impl ActivityBar {
    pub fn to_toolbar_parts(
        &self,
        style: &ActivityBarStyle,
        theme: &Theme,
        nerd_fonts_enabled: bool,
    ) -> (Toolbar, ToolbarRailStyle) {
        let mut buttons = Vec::with_capacity(self.top_items.len() + 1 + self.bottom_items.len());
        let mut focused_index = None;
        let icon_of = |item: &ActivityItem| -> String {
            if nerd_fonts_enabled {
                item.icon.glyph.clone()
            } else {
                item.icon.fallback.clone()
            }
        };
        for item in &self.top_items {
            if item.is_keyboard_selected {
                focused_index = Some(buttons.len());
            }
            buttons.push(activity_item_to_toolbar_button_with_icon(
                item,
                icon_of(item),
            ));
        }
        buttons.push(ToolbarButton::Spacer);
        for item in &self.bottom_items {
            if item.is_keyboard_selected {
                focused_index = Some(buttons.len());
            }
            buttons.push(activity_item_to_toolbar_button_with_icon(
                item,
                icon_of(item),
            ));
        }

        let toolbar = Toolbar::new(self.id)
            .with_buttons(buttons)
            .with_focused_index(focused_index)
            .with_bg(theme.tab_bar_bg);

        let mut rail_style = ToolbarRailStyle::new().with_trailing_separator(theme.separator);
        if let Some(active_bg) = style.active_bg {
            rail_style = rail_style.with_active_bg(active_bg);
        }
        if let Some(accent) = self.active_accent {
            rail_style = rail_style.with_active_accent(accent);
        }
        if let Some(selection_bg) = self.selection_bg {
            rail_style = rail_style.with_selection_bg(selection_bg);
        }

        (toolbar, rail_style)
    }

    /// Turn a vertical [`ToolbarLayout`] — built from this bar's own
    /// [`Self::to_toolbar_parts`] `Toolbar`, via
    /// [`Toolbar::layout_with_options`] with
    /// [`ToolbarOrientation::Vertical`] and
    /// [`ToolbarLayoutOptions::with_pin_trailing_overflow_to_origin`] set,
    /// at origin `(0.0, 0.0)` — into the row-hit list a
    /// rail painter returns.
    ///
    /// Order and coordinate space match
    /// [`native_surface_paint::paint`]'s own return value exactly:
    /// **bottom-pinned items first (in order), then top-pinned (in
    /// order)** — the same order [`Self::layout`]'s own
    /// `ActivityBarLayout::visible_items` uses — and `y_start`/`y_end`
    /// bar-relative (first row at `0.0`), which only holds when the
    /// caller's `ToolbarLayout` was itself built at origin `(0.0, 0.0)`.
    /// Skips non-`Action` items and zero-height clipped items, same as
    /// [`Self::layout`].
    pub fn row_hits_from_toolbar_layout(
        &self,
        toolbar_layout: &ToolbarLayout,
    ) -> Vec<ActivityBarRowHit> {
        let top_len = self.top_items.len();
        let mut top = Vec::new();
        let mut bottom = Vec::new();

        for vis in &toolbar_layout.visible_items {
            if vis.kind != ToolbarItemKind::Action || vis.bounds.height <= 0.0 {
                continue;
            }
            let item = if vis.item_idx < top_len {
                &self.top_items[vis.item_idx]
            } else {
                &self.bottom_items[vis.item_idx - top_len - 1]
            };
            let hit = ActivityBarRowHit {
                y_start: vis.bounds.y,
                y_end: vis.bounds.y + vis.bounds.height,
                id: item.id,
                tooltip: item.tooltip.clone(),
            };
            if vis.item_idx < top_len {
                top.push(hit);
            } else {
                bottom.push(hit);
            }
        }

        let mut out = Vec::with_capacity(top.len() + bottom.len());
        out.extend(bottom);
        out.extend(top);
        out
    }
}

/// Paint-time style for an [`ActivityBar`], carried on the descriptor as
/// [`ActivityBar::style`]: the active-item row-fill colour, VS Code style
/// (no line, a soft chip on the row itself), and the icon glyph size.
///
/// [`ActivityBarStyle::default`] paints no fill and the default icon
/// size.
///
/// `#[non_exhaustive]`: means a future style knob (e.g. a corner radius
/// for the fill) is additive rather than breaking. Construct with
/// [`ActivityBarStyle::new`] / [`ActivityBarStyle::default`] and the
/// `with_*` builders; the field stays `pub` for reading.
// `Eq` dropped from the derive (was `PartialEq, Eq`): `icon_size_px`
// is an `Option<f32>`, and `f32` has no `Eq` impl (NaN isn't
// reflexive) — the same reason no other float-carrying `Copy` chrome
// style type in this crate (e.g. `TabChrome`) derives `Eq` either.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct ActivityBarStyle {
    /// Background fill colour for the active item's row. `None` (the
    /// default) = no fill, i.e. today's behaviour. Independent of
    /// [`ActivityBar::active_accent`]: set this alone for a VS-Code-style
    /// fill, `active_accent` alone for a JetBrains-style line, both, or
    /// neither.
    #[serde(default)]
    pub active_bg: Option<Color>,

    /// Icon glyph size, in device-independent pixels. `None`
    /// (the default) resolves to [`DEFAULT_ACTIVITY_ICON_SIZE_PX`] via
    /// [`Self::resolved_icon_size_px`] — VS-Code parity (~24px) regardless
    /// of whatever size the app's editor or chrome font happens to be set
    /// to.
    ///
    /// Every pixel backend paints the icon at this size rather than at
    /// the editor or chrome font's size, which was never meant to govern
    /// it.
    #[serde(default)]
    pub icon_size_px: Option<f32>,
}

impl ActivityBarStyle {
    /// Style with no overrides — identical to [`Self::default`], provided
    /// for symmetry with [`crate::TooltipChrome::new`] / [`crate::TabChrome::new`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the active-item row-fill colour.
    pub fn with_active_bg(mut self, color: Color) -> Self {
        self.active_bg = Some(color);
        self
    }

    /// Set the icon glyph size, in device-independent pixels. See
    /// [`Self::icon_size_px`]'s doc for how each pixel backend interprets
    /// this value.
    pub fn with_icon_size_px(mut self, size_px: f32) -> Self {
        self.icon_size_px = Some(size_px);
        self
    }

    /// [`Self::icon_size_px`], or [`DEFAULT_ACTIVITY_ICON_SIZE_PX`] if unset.
    pub fn resolved_icon_size_px(&self) -> f32 {
        self.icon_size_px.unwrap_or(DEFAULT_ACTIVITY_ICON_SIZE_PX)
    }
}

/// VS Code renders its activity-bar icons (24×24 codicon SVGs) at a fixed
/// size regardless of the app's editor or chrome font size. This is the
/// default an [`ActivityBarStyle`] with no explicit
/// [`ActivityBarStyle::icon_size_px`] resolves to via
/// [`ActivityBarStyle::resolved_icon_size_px`].
///
/// The unit is device-independent pixels, not any one backend's native
/// font-size unit — GTK's Pango and Win-GUI's DirectWrite both size fonts
/// in *points* under the legacy 96/72 dpi convention (`pt = px * 72.0 /
/// 96.0`), while Core Text's *points* on macOS are already
/// device-independent pixels 1:1 (no 96/72 rescale) — each backend's
/// `activity_bar` module doc spells out its own conversion at the call
/// site. TUI is unaffected: it always paints one glyph per cell,
/// independent of any font size.
pub const DEFAULT_ACTIVITY_ICON_SIZE_PX: f32 = 24.0;

/// Fixed row height, in device-independent pixels, of a single
/// activity-bar item on every pixel backend — VS Code's square 48px rail
/// button. Deliberately independent of any font size: the row is chrome
/// geometry, while the glyph inside it is sized by
/// [`ActivityBarStyle::icon_size_px`].
///
/// [`PIXEL_ROW_HEIGHT_CELLS`] is the cell-grid counterpart.
pub const PIXEL_ROW_HEIGHT: f32 = 48.0;

/// Row height, in character cells, of a single activity-bar item on a
/// cell grid: one row per item, the convention every cell-grid caller
/// already paints with.
pub const PIXEL_ROW_HEIGHT_CELLS: f32 = 1.0;

/// Width, in character cells, a cell-grid activity bar reports as its
/// natural size: one accent column, two icon columns and one separator
/// column — what [`crate::ShellLayout::activity_bar_width`] defaults to.
pub const CELL_BAR_WIDTH: f32 = 4.0;

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
///
/// # Coordinate space — **relative to the bar**
///
/// `y_start` / `y_end` are measured from the **top edge of the `rect`
/// the bar was laid out in**, i.e. the first row always
/// starts at `0.0` regardless of where the bar sits on screen. They are
/// **not** target-surface (absolute) coordinates — this is the opposite
/// convention from [`TabBarHits`], whose `x` spans *are* absolute.
///
/// Callers therefore add the bar's origin themselves before comparing
/// against a raw click position:
///
/// ```ignore
/// let hit_top = hit.y_start as f32 + activity_bar_bounds.y;
/// ```
///
/// Coordinates here are bar-relative offsets, not absolute paint `y`
/// values. Callers add the bar origin before hit-testing.
#[derive(Debug, Clone)]
pub struct ActivityBarRowHit {
    /// Top edge of the row, **relative to the bar's `rect.y`** (first
    /// row is `0.0`). Add the bar origin before hit-testing a click.
    pub y_start: f32,
    /// Bottom edge (exclusive) of the row, **relative to the bar's
    /// `rect.y`**. Add the bar origin before hit-testing a click.
    pub y_end: f32,
    pub id: WidgetId,
    pub tooltip: String,
}

/// Fully-resolved activity-bar layout. Backends iterate `visible_items`
/// for painting and call [`Self::hit_test`] for clicks.
#[derive(Debug, Clone, PartialEq)]
pub struct ActivityBarLayout {
    pub viewport_width: f32,
    pub viewport_height: f32,
    /// All visible items — **bottom-pinned first** (in order), then
    /// top-pinned (in order). That is the order [`ActivityBar::layout`]
    /// appends them in, because bottom items are placed first (they win
    /// on collision). Backends `enumerate()` this list to derive the flat
    /// index they compare against `hovered_idx`, so callers must preserve
    /// this order to agree with hit-testing.
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
    /// **bottom items win** and top items are clipped.
    ///
    /// Delegates to [`Toolbar::layout_with_options`] (via
    /// `Toolbar::from(self)`, [`ToolbarOrientation::Vertical`], a fixed
    /// `item_height`-per-item measurer, and
    /// [`ToolbarLayoutOptions::pin_trailing_overflow_to_origin`] set):
    /// that method's own Spacer collision rule — the trailing group
    /// always paints in full, the leading group clips against the
    /// trailing group's start — is the exact rule this method's own doc
    /// describes, by construction (the `Toolbar` built from `self`
    /// places `bottom_items` as the trailing group). The pinned-overflow
    /// flag additionally makes a `bottom_items` group that alone is
    /// taller than `viewport_height` pack from `y=0` downward and drop
    /// its last item, instead of hanging off the bottom edge and
    /// dropping its first — see that option's own doc for the worked
    /// example. Every pixel/cell backend shares this one geometry engine
    /// with `Toolbar`.
    ///
    /// The returned [`ActivityBarLayout::visible_items`] keeps this
    /// type's own **bottom-pinned-items-first, then top-pinned** order
    /// (not `Toolbar`'s document order) — see that field's own doc for
    /// why: hosts derive `hovered_idx` from this exact enumeration.
    pub fn layout(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        item_height: f32,
    ) -> ActivityBarLayout {
        if item_height <= 0.0 || viewport_height <= 0.0 {
            return ActivityBarLayout {
                viewport_width,
                viewport_height,
                visible_items: Vec::new(),
                hit_regions: Vec::new(),
            };
        }

        let top_len = self.top_items.len();
        let toolbar: Toolbar = self.into();
        let toolbar_layout = toolbar.layout_with_options(
            0.0,
            0.0,
            viewport_width,
            viewport_height,
            ToolbarLayoutOptions::new()
                .with_orientation(ToolbarOrientation::Vertical)
                .with_pin_trailing_overflow_to_origin(true),
            |_btn| ToolbarItemMeasure::new(item_height),
        );

        let mut top: Vec<VisibleActivityItem> = Vec::new();
        let mut bottom: Vec<VisibleActivityItem> = Vec::new();
        for vis in &toolbar_layout.visible_items {
            if vis.kind != ToolbarItemKind::Action || vis.bounds.height <= 0.0 {
                continue;
            }
            if vis.item_idx < top_len {
                top.push(VisibleActivityItem {
                    side: ActivitySide::Top,
                    item_idx: vis.item_idx,
                    bounds: vis.bounds,
                });
            } else {
                bottom.push(VisibleActivityItem {
                    side: ActivitySide::Bottom,
                    item_idx: vis.item_idx - top_len - 1,
                    bounds: vis.bounds,
                });
            }
        }

        let mut visible_items = Vec::with_capacity(top.len() + bottom.len());
        visible_items.extend(bottom);
        visible_items.extend(top);

        let hit_regions = visible_items
            .iter()
            .map(|vi| {
                let item = match vi.side {
                    ActivitySide::Top => &self.top_items[vi.item_idx],
                    ActivitySide::Bottom => &self.bottom_items[vi.item_idx],
                };
                (vi.bounds, ActivityBarHit::Item(item.id))
            })
            .collect();

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

/// Convert a [`crate::Key`] to the string form carried in
/// [`ActivityBarEvent::KeyPressed`].
///
/// - Printable characters map to their single-character string (`"j"`, `"k"`, …).
/// - Named keys use TitleCase names (`"Escape"`, `"Enter"`, `"Up"`, …).
///
/// The TUI, GTK and macOS backends all use this helper so the emitted key
/// strings are identical regardless of which backend is active.
///
/// This is `pub(crate)` because external consumers receive the already-normalised
/// string from [`ActivityBarEvent::KeyPressed::key`] — they have no reason to call
/// the conversion helper directly.
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    all(feature = "macos", target_os = "macos"),
    feature = "win"
))]
pub(crate) fn key_to_activity_bar_string(key: &crate::event::Key) -> String {
    use crate::event::Key;
    match key {
        Key::Char(c) => c.to_string(),
        Key::Named(named) => named.name().into_owned(),
    }
}

// `paint` below delegates to `crate::primitives::toolbar::native_surface_paint::paint_rail`,
// the one shared rail painter every pixel backend paints through — see that
// function's own doc for the fill order and per-row foreground rule. This
// module's job is purely the seam: build the `Toolbar` + `ToolbarRailStyle`
// pair via [`ActivityBar::to_toolbar_parts`], lay it out with the same
// options [`ActivityBar::layout`] itself uses, paint, and translate the
// result back into `Vec<ActivityBarRowHit>` via
// [`ActivityBar::row_hits_from_toolbar_layout`].
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{
        ActivityBar, ActivityBarLayout, ActivityBarRowHit, ActivityBarStyle, ActivitySide,
    };
    use crate::paint_surface::PaintSurface;
    use crate::primitives::toolbar::{
        self, ToolbarItemMeasure, ToolbarLayoutOptions, ToolbarOrientation,
    };
    use crate::theme::Theme;

    /// Paint an [`ActivityBar`] at its caller-resolved, bar-relative
    /// `layout` onto `surface`, with the rail's top-left corner at
    /// `origin`. Returns per-row hit spans, **bar-relative** (see
    /// [`ActivityBarRowHit`]'s own doc) — callers add the bar's origin
    /// before hit-testing a click.
    ///
    /// The icon glyphs paint at [`ActivityBarStyle::resolved_icon_size_px`]
    /// through [`PaintSurface::surface_with_icon_font`], independent of
    /// the surface's chrome font size; a surface with no icon-font twin
    /// paints them in its own font.
    ///
    /// `hovered_idx` indexes into `layout.visible_items` (bottom-pinned
    /// items first, then top — see [`ActivityBar::layout`]'s doc); it's
    /// mapped to the matching item's [`crate::WidgetId`] and passed to
    /// `toolbar::native_surface_paint::paint_rail` as its own `hovered_id`.
    /// `nerd_fonts_enabled` selects `item.icon.glyph` vs
    /// `item.icon.fallback` — via [`ActivityBar::to_toolbar_parts`], before
    /// the painter ever sees either `ActivityBar` or `ActivityItem`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint(
        bar: &ActivityBar,
        layout: &ActivityBarLayout,
        origin: crate::event::Point,
        style: &ActivityBarStyle,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        hovered_idx: Option<usize>,
        nerd_fonts_enabled: bool,
    ) -> Vec<ActivityBarRowHit> {
        let (toolbar, rail_style) = bar.to_toolbar_parts(style, theme, nerd_fonts_enabled);

        // Every visible row shares one uniform height (the `item_height`
        // `ActivityBar::layout` was called with); recover it from any row
        // rather than threading a redundant parameter through this
        // function's fixed signature.
        let item_height = layout
            .visible_items
            .iter()
            .map(|vi| vi.bounds.height)
            .fold(0.0_f32, f32::max);

        let rail_layout_at = |x: f32, y: f32| {
            toolbar.layout_with_options(
                x,
                y,
                layout.viewport_width,
                layout.viewport_height,
                ToolbarLayoutOptions::new()
                    .with_orientation(ToolbarOrientation::Vertical)
                    .with_pin_trailing_overflow_to_origin(true),
                |_btn| ToolbarItemMeasure::new(item_height),
            )
        };
        let painted_layout = rail_layout_at(origin.x, origin.y);

        let hovered_id = hovered_idx
            .and_then(|idx| layout.visible_items.get(idx))
            .map(|vi| match vi.side {
                ActivitySide::Top => bar.top_items[vi.item_idx].id,
                ActivitySide::Bottom => bar.bottom_items[vi.item_idx].id,
            });

        let mut paint_rail = |surface: &mut dyn PaintSurface| {
            toolbar::native_surface_paint::paint_rail(
                &toolbar,
                &rail_style,
                &painted_layout,
                surface,
                theme,
                hovered_id.as_ref(),
            );
        };
        if !surface.surface_with_icon_font(style.resolved_icon_size_px(), &mut paint_rail) {
            paint_rail(surface);
        }

        // `row_hits_from_toolbar_layout` reads a layout built at the
        // origin, which keeps the hits bar-relative.
        bar.row_hits_from_toolbar_layout(&rail_layout_at(0.0, 0.0))
    }
}

// ── Widget contract ─────────────────────────────────────────────────────────

/// [`ActivityBar`]'s fully-resolved widget-contract layout: the absolute
/// area the rail was laid out into, plus the [`ActivityBarLayout`]
/// resolved inside it.
///
/// `layout`'s bounds are **bar-relative** (first row at `y = 0.0`), the
/// frame [`ActivityBarLayout::hit_test`] and [`ActivityBarRowHit`] have
/// always used, so [`Self::hit_test`] takes bar-local coordinates too —
/// already the space [`crate::interactive::RoutedEvent::Pointer`]'s
/// `local` field and [`crate::interactive::Interactive::sub_regions`]
/// answer in.
///
/// `#[non_exhaustive]`: produced by
/// [`Primitive::layout`](crate::primitive::Primitive::layout), never built
/// by a consumer via struct literal.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ActivityBarPrimitiveLayout {
    /// The rect [`Primitive::layout`](crate::primitive::Primitive::layout)
    /// received — the rail's own absolute area.
    pub area: Rect,
    /// The rail's resolved layout, in bar-relative coordinates.
    pub layout: ActivityBarLayout,
    /// Whether the laying-out backend had a Nerd Font icon face
    /// registered — see [`crate::primitive::LayoutCtx::nerd_fonts_enabled`].
    ///
    /// Baked into the layout because
    /// [`crate::primitive::Primitive::paint_cells`] takes no
    /// [`crate::primitive::LayoutCtx`] of its own, and an
    /// [`ActivityItem`]'s [`crate::Icon`] resolves a different glyph with
    /// and without one.
    pub nerd_fonts: bool,
    /// The descriptor's [`ActivityBar::is_keyboard_focused`], kept on the
    /// painted layout so a backend reads which rail holds keyboard focus
    /// from this frame's painted-widget record
    /// ([`painted_keyboard_focused_bar`]).
    pub keyboard_focused: bool,
}

/// The id of the [`ActivityBar`] painted this frame with
/// [`ActivityBar::is_keyboard_focused`] set, if any, read from the
/// painted-widget record [`crate::backend::DrawsPrimitives::draw_dyn`]
/// fills. A backend redirects key presses to that bar as
/// [`ActivityBarEvent::KeyPressed`].
#[cfg(any(
    feature = "tui",
    feature = "gtk",
    all(feature = "macos", target_os = "macos"),
    feature = "win"
))]
pub(crate) fn painted_keyboard_focused_bar() -> Option<WidgetId> {
    crate::primitive::with_painted_widgets(|painted| {
        painted.iter().find_map(|w| {
            w.layout
                .downcast_ref::<ActivityBarPrimitiveLayout>()
                .filter(|layout| layout.keyboard_focused)
                .map(|_| w.id)
        })
    })
}

impl ActivityBarPrimitiveLayout {
    /// Classify a bar-local point against this rail — see
    /// [`ActivityBarLayout::hit_test`].
    pub fn hit_test(&self, x: f32, y: f32) -> ActivityBarHit {
        self.layout.hit_test(x, y)
    }

    /// Index into [`ActivityBarLayout::visible_items`] of the row at
    /// bar-local `(x, y)` — the same enumeration a host's `hovered_idx`
    /// indexes into (bottom-pinned items first, then top-pinned).
    pub fn visible_index_at(&self, x: f32, y: f32) -> Option<usize> {
        self.layout.visible_items.iter().position(|vi| {
            x >= vi.bounds.x
                && x < vi.bounds.x + vi.bounds.width
                && y >= vi.bounds.y
                && y < vi.bounds.y + vi.bounds.height
        })
    }

    /// One bar-relative [`ActivityBarRowHit`] per visible row, in
    /// [`ActivityBarLayout::visible_items`] order (bottom-pinned items
    /// first, then top-pinned), carrying each item's id and tooltip.
    /// `bar` is the descriptor this layout was resolved for.
    pub fn row_hits(&self, bar: &ActivityBar) -> Vec<ActivityBarRowHit> {
        self.layout
            .visible_items
            .iter()
            .filter_map(|vi| {
                let item = match vi.side {
                    ActivitySide::Top => bar.top_items.get(vi.item_idx)?,
                    ActivitySide::Bottom => bar.bottom_items.get(vi.item_idx)?,
                };
                Some(ActivityBarRowHit {
                    y_start: vi.bounds.y,
                    y_end: vi.bounds.y + vi.bounds.height,
                    id: item.id,
                    tooltip: item.tooltip.clone(),
                })
            })
            .collect()
    }
}

/// [`crate::interactive::Interactive::State`] for [`ActivityBar`]:
/// keyboard focus, the keyboard cursor's position in the bar's logical
/// item sequence, and which row the pointer is over.
///
/// The **logical item sequence** is `top_items` first, then
/// `bottom_items` — the order [`ActivityItem::is_keyboard_selected`] and
/// [`Toolbar::focused_index`] already follow. That is *not*
/// [`ActivityBarLayout::visible_items`]' order (bottom-pinned first),
/// which [`Self::hovered_idx`] uses instead; the two are deliberately
/// different spaces, named by their own field docs.
///
/// `#[non_exhaustive]`: a future field add here is additive.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActivityBarState {
    /// Whether the rail currently holds keyboard focus.
    pub focused: bool,
    /// Cursor position in the **logical** item sequence. Saturates at the
    /// ends; does not wrap. Meaningful even when `!focused`, so a
    /// `cursor_visible`-only host still has somewhere to paint the
    /// highlight.
    pub cursor: usize,
    /// Show the keyboard cursor highlight without taking keyboard focus —
    /// for a host that paints the rail's cursor while the keyboard is
    /// driving something else.
    pub cursor_visible: bool,
    /// Index into [`ActivityBarLayout::visible_items`] of the row under
    /// the pointer, if any — the value every rasteriser's `hovered_idx`
    /// parameter takes.
    pub hovered_idx: Option<usize>,
}

impl ActivityBarState {
    /// Move the cursor to the next item. Saturates at `total - 1` — does
    /// not wrap. No-op when `total == 0`.
    pub fn select_next(&mut self, total: usize) {
        // A cursor past `total` (reachable via `set_cursor_raw` when the
        // item count shrinks afterwards) is clamped into range first, so a
        // forward move lands on the last item rather than seeding at the
        // first one — `FlatNav` treats an out-of-range `current` as "no
        // selection yet".
        let current = self.cursor.min(total.saturating_sub(1));
        self.cursor =
            FlatNav::mv(total, Some(current), NavKey::Down, false, |_| true).unwrap_or(self.cursor);
    }

    /// Move the cursor to the previous item. Saturates at index 0 — does
    /// not wrap.
    pub fn select_prev(&mut self) {
        // No `total` bound is needed for an upward move: `self.cursor + 1`
        // is always a valid length for the current cursor position, so
        // `FlatNav` clamps at 0 exactly like a plain `saturating_sub(1)`.
        self.cursor = FlatNav::mv(
            self.cursor + 1,
            Some(self.cursor),
            NavKey::Up,
            false,
            |_| true,
        )
        .unwrap_or(self.cursor);
    }

    /// Set the cursor to `idx`, clamped to `[0, total - 1]`. No-op when
    /// `total == 0`.
    pub fn set_cursor(&mut self, idx: usize, total: usize) {
        if total > 0 {
            self.cursor = idx.min(total - 1);
        }
    }

    /// Set the cursor to an exact index with no clamping against a item
    /// count. For a caller that has already computed a valid index itself
    /// (a host's own panel add/remove bookkeeping, which knows the new
    /// item count) — [`Self::set_cursor`] is the clamping entry point.
    pub fn set_cursor_raw(&mut self, idx: usize) {
        self.cursor = idx;
    }
}

impl crate::primitive::PaintState for ActivityBarState {
    /// Index into [`ActivityBarLayout::visible_items`] — see
    /// [`Self::hovered_idx`].
    fn hovered_idx(&self) -> Option<usize> {
        self.hovered_idx
    }
}

/// What an [`ActivityBar`]'s own
/// [`crate::interactive::Interactive::handle`] resolves one gesture to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivityBarAction {
    /// An item was activated — by click, by Enter on the keyboard cursor,
    /// or by an assistive-technology `Click`. Carries that
    /// [`ActivityItem::id`].
    ItemActivated(WidgetId),
}

impl crate::interactive::WidgetAction for ActivityBarAction {
    /// Rides [`crate::UiEvent::ActivityBar`] under the rail's own id,
    /// carrying [`ActivityBarEvent::ItemClicked`] — the one channel every
    /// existing activity-bar host already routes an item activation
    /// through, whether the gesture was a click or Enter.
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        match self {
            ActivityBarAction::ItemActivated(item) => {
                crate::UiEvent::ActivityBar(id, ActivityBarEvent::ItemClicked { id: item })
            }
        }
    }
}

impl ActivityBar {
    /// Number of items in the bar's **logical** sequence — see
    /// [`ActivityBarState::cursor`].
    pub fn item_count(&self) -> usize {
        self.top_items.len() + self.bottom_items.len()
    }

    /// The item at logical index `idx`, or `None` past the end.
    pub fn item_at(&self, idx: usize) -> Option<&ActivityItem> {
        let top_len = self.top_items.len();
        if idx < top_len {
            self.top_items.get(idx)
        } else {
            self.bottom_items.get(idx - top_len)
        }
    }

    /// Logical index of the item a [`VisibleActivityItem`] names — the
    /// translation between [`ActivityBarLayout::visible_items`]' own
    /// bottom-pinned-first order and the logical sequence
    /// [`ActivityBarState::cursor`] walks.
    pub fn logical_index_of(&self, visible: &VisibleActivityItem) -> usize {
        match visible.side {
            ActivitySide::Top => visible.item_idx,
            ActivitySide::Bottom => self.top_items.len() + visible.item_idx,
        }
    }

    /// Cell-units layout: one row per item, [`CELL_BAR_WIDTH`]-ish rails
    /// are the caller's own choice of `rect` width.
    fn cell_layout(&self, rect: Rect) -> ActivityBarLayout {
        self.layout(rect.width, rect.height, PIXEL_ROW_HEIGHT_CELLS)
    }

    /// Pixel-units layout: the fixed [`PIXEL_ROW_HEIGHT`] row every pixel
    /// backend's own rail uses, so one derivation serves paint and
    /// hit-test on all of them.
    pub fn pixel_layout(&self, rect: Rect) -> ActivityBarLayout {
        self.layout(rect.width, rect.height, PIXEL_ROW_HEIGHT)
    }
}

impl crate::primitive::Primitive for ActivityBar {
    type Layout = ActivityBarPrimitiveLayout;
    type Event = ActivityBarHit;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::ActivityBar(self.clone())
    }

    /// A rail stretches to its container's height and is a fixed width
    /// across it: [`CELL_BAR_WIDTH`] columns on a cell grid,
    /// [`crate::Style::activity_bar_width`] on a pixel surface.
    fn measure(
        &self,
        ctx: &crate::primitive::LayoutCtx,
        _avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        let width = match ctx.units() {
            crate::primitive::Units::Cells => CELL_BAR_WIDTH,
            _ => ctx.style().activity_bar_width,
        };
        crate::NaturalSize::new(Some(width), None)
    }

    /// One row per item on a cell grid; a fixed [`PIXEL_ROW_HEIGHT`] row
    /// on a pixel surface. Both go through [`ActivityBar::layout`], whose
    /// own doc carries the bottom-pinned collision rule.
    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> ActivityBarPrimitiveLayout {
        let layout = match ctx.units() {
            crate::primitive::Units::Cells => self.cell_layout(rect),
            _ => self.pixel_layout(rect),
        };
        ActivityBarPrimitiveLayout {
            area: rect,
            layout,
            nerd_fonts: ctx.nerd_fonts_enabled,
            keyboard_focused: self.is_keyboard_focused,
        }
    }

    /// The shared rail painter, through [`native_surface_paint::paint`],
    /// with the descriptor's own [`ActivityBar::style`] and
    /// [`ActivityBar::hovered`] row.
    fn paint(
        &self,
        layout: &ActivityBarPrimitiveLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        let style = env.style();
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            let _ = style;
            native_surface_paint::paint(
                self,
                &layout.layout,
                crate::event::Point::new(layout.area.x, layout.area.y),
                &self.style,
                surface,
                theme,
                self.hovered,
                layout.nerd_fonts,
            );
        }
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        {
            let _ = (layout, surface, theme, style);
        }
    }

    /// The cell-grid rail painter, [`cell_paint::paint`].
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &ActivityBarPrimitiveLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        // CSR: ratatui_buffer escape hatch
        let buf = cells
            .ratatui_buffer()
            .expect(crate::primitive::CELL_BUFFER_REQUIRED);
        let area = ratatui::layout::Rect::new(
            layout.area.x.max(0.0).round() as u16,
            layout.area.y.max(0.0).round() as u16,
            layout.area.width.max(0.0).round() as u16,
            layout.area.height.max(0.0).round() as u16,
        );
        let _ = cell_paint::paint(buf, area, self, theme, layout.nerd_fonts);
    }
}

/// The cell-grid rail painter. An activity bar paints as a vertical-rail
/// [`Toolbar`] ([`ActivityBar::to_toolbar_parts`]) through Toolbar's own
/// rail rasteriser, so the two rails share one cell painter; the same
/// layout then becomes this bar's [`ActivityBarRowHit`] list through
/// [`ActivityBar::row_hits_from_toolbar_layout`].
#[cfg(feature = "tui")]
pub(crate) mod cell_paint {
    use super::{ActivityBar, ActivityBarRowHit};
    use crate::primitives::toolbar::cell_layout::{
        rail_layout_options, tui_toolbar_layout_with_options,
    };
    use crate::primitives::toolbar::cell_paint::draw_toolbar_with_rail_style;
    use crate::theme::Theme;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    /// Paint `bar` into `area` on `buf` with its own
    /// [`ActivityBar::style`] and [`ActivityBar::hovered`] row, returning
    /// the bar-relative row hits (bottom-pinned items first, then
    /// top-pinned, the order [`ActivityBar::hovered`] indexes).
    ///
    /// `nerd_fonts_enabled` picks which half of each item's
    /// [`crate::Icon`] paints: `glyph` when `true`, `fallback` when
    /// `false`.
    pub(crate) fn paint(
        buf: &mut Buffer,
        area: Rect,
        bar: &ActivityBar,
        theme: &Theme,
        nerd_fonts_enabled: bool,
    ) -> Vec<ActivityBarRowHit> {
        if area.width == 0 || area.height == 0 {
            return Vec::new();
        }

        let (toolbar, rail_style) = bar.to_toolbar_parts(&bar.style, theme, nerd_fonts_enabled);

        // Row hits are built at origin (0, 0), the precondition of
        // `row_hits_from_toolbar_layout`, which also makes them the
        // lookup table `bar.hovered` indexes into.
        let zero_origin_layout = tui_toolbar_layout_with_options(
            &toolbar,
            Rect::new(0, 0, area.width, area.height),
            rail_layout_options(),
        );
        let row_hits = bar.row_hits_from_toolbar_layout(&zero_origin_layout);

        let hovered_id = bar.hovered.and_then(|i| row_hits.get(i)).map(|h| h.id);

        let _ = draw_toolbar_with_rail_style(
            buf,
            area,
            &toolbar,
            &rail_style,
            theme,
            hovered_id.as_ref(),
        );

        row_hits
    }
}

impl crate::interactive::Interactive for ActivityBar {
    type State = ActivityBarState;
    type Action = ActivityBarAction;

    /// The whole rail is one keyboard focus stop, VS Code-style: Tab
    /// reaches the bar, and Up/Down then walk its own items via the
    /// cursor in [`ActivityBarState`]. The items are not individual tab
    /// stops.
    ///
    /// [`crate::interactive::FocusSpec::None`] for an empty rail — there
    /// is nothing for keyboard focus to land on.
    fn focus(&self) -> crate::interactive::FocusSpec {
        if self.item_count() == 0 {
            crate::interactive::FocusSpec::None
        } else {
            crate::interactive::FocusSpec::Stop
        }
    }

    /// Every visible row's own rect, in bar-local coordinates, keyed by
    /// its index into [`ActivityBarLayout::visible_items`] — the rows
    /// never overlap, so there is no most-specific-first ordering to
    /// impose.
    fn sub_regions(
        &self,
        layout: &ActivityBarPrimitiveLayout,
    ) -> Vec<(Rect, crate::interactive::SubTarget)> {
        layout
            .layout
            .visible_items
            .iter()
            .enumerate()
            .map(|(i, vi)| (vi.bounds, crate::interactive::SubTarget(i as u64)))
            .collect()
    }

    /// `Click` activates whatever the keyboard cursor is on, the same
    /// item Enter would.
    fn a11y_actions(&self, state: &ActivityBarState) -> Vec<crate::a11y::A11yAction> {
        if self.item_at(state.cursor).is_some() {
            vec![crate::a11y::A11yAction::Click]
        } else {
            Vec::new()
        }
    }

    /// A left click activates the row it lands on (and parks the cursor
    /// there); Up/Down walk the logical item sequence through
    /// [`FlatNav`], saturating at both ends rather than wrapping; Enter
    /// activates the cursor's item.
    ///
    /// Hover tracks which [`ActivityBarLayout::visible_items`] row the
    /// pointer is over, for a host that paints a hover tint.
    ///
    /// Everything else is [`Handled::Ignored`] on purpose, so the keys a
    /// host maps to its own engine (vim-style `j`/`k`/`l`, `Esc`/`h` to
    /// leave the rail) still reach it, and a right click still bubbles as
    /// a plain [`crate::UiEvent::MouseDown`] for the host's own
    /// context-menu policy.
    fn handle(
        &self,
        state: &mut ActivityBarState,
        event: &crate::interactive::RoutedEvent,
        layout: &ActivityBarPrimitiveLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> Handled<ActivityBarAction> {
        use crate::interactive::{KeyEvent, PointerKind, RoutedEvent};

        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Enter | PointerKind::Move,
                local,
                ..
            } => {
                let hit = layout.visible_index_at(local.x, local.y);
                if state.hovered_idx == hit {
                    Handled::Ignored
                } else {
                    state.hovered_idx = hit;
                    cx.request_redraw();
                    Handled::Consumed
                }
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Leave,
                ..
            } => {
                if state.hovered_idx.take().is_some() {
                    cx.request_redraw();
                    Handled::Consumed
                } else {
                    Handled::Ignored
                }
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Down(crate::event::MouseButton::Left),
                local,
                ..
            } => {
                let Some(idx) = layout.visible_index_at(local.x, local.y) else {
                    return Handled::Ignored;
                };
                let visible = layout.layout.visible_items[idx];
                let logical = self.logical_index_of(&visible);
                let Some(item) = self.item_at(logical) else {
                    return Handled::Ignored;
                };
                state.hovered_idx = Some(idx);
                state.set_cursor(logical, self.item_count());
                Handled::Event(ActivityBarAction::ItemActivated(item.id))
            }
            RoutedEvent::Key(KeyEvent { key, modifiers, .. })
                if !modifiers.ctrl && !modifiers.alt && !modifiers.cmd =>
            {
                match key {
                    Key::Named(crate::event::NamedKey::Down) => {
                        state.select_next(self.item_count());
                        cx.request_redraw();
                        Handled::Consumed
                    }
                    Key::Named(crate::event::NamedKey::Up) => {
                        state.select_prev();
                        cx.request_redraw();
                        Handled::Consumed
                    }
                    Key::Named(crate::event::NamedKey::Enter) => self.activate_cursor(state),
                    _ => Handled::Ignored,
                }
            }
            RoutedEvent::Action(crate::a11y::A11yAction::Click) => self.activate_cursor(state),
            RoutedEvent::FocusIn(_) => {
                state.focused = true;
                Handled::Ignored
            }
            RoutedEvent::FocusOut => {
                state.focused = false;
                Handled::Ignored
            }
            _ => Handled::Ignored,
        }
    }
}

impl ActivityBar {
    /// Activate the item the keyboard cursor sits on, or
    /// [`Handled::Ignored`] when the cursor names no item (an empty rail,
    /// or a cursor left past the end by a shrunk item list).
    fn activate_cursor(&self, state: &ActivityBarState) -> Handled<ActivityBarAction> {
        match self.item_at(state.cursor) {
            Some(item) => Handled::Event(ActivityBarAction::ItemActivated(item.id)),
            None => Handled::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_bar_roundtrip_serde() {
        let bar = ActivityBar {
            id: WidgetId::new("main-activity-bar"),
            top_items: vec![
                ActivityItem {
                    id: WidgetId::new("activity:explorer"),
                    icon: "\u{f07c}".into(),
                    tooltip: "Explorer".to_string(),
                    is_active: true,
                    is_keyboard_selected: false,
                },
                ActivityItem {
                    id: WidgetId::new("activity:search"),
                    icon: "\u{f422}".into(),
                    tooltip: "Search".to_string(),
                    is_active: false,
                    is_keyboard_selected: true,
                },
            ],
            bottom_items: vec![ActivityItem {
                id: WidgetId::new("activity:settings"),
                icon: "\u{f013}".into(),
                tooltip: "Settings".to_string(),
                is_active: false,
                is_keyboard_selected: false,
            }],
            active_accent: Some(Color::rgb(120, 180, 255)),
            selection_bg: Some(Color::rgb(80, 80, 80)),
            is_keyboard_focused: false,
            style: Default::default(),
            hovered: None,
        };
        let json = serde_json::to_string(&bar).unwrap();
        let back: ActivityBar = serde_json::from_str(&json).unwrap();
        assert_eq!(bar, back);
    }

    /// `ActivityBarStyle` round-trips, and a payload with no
    /// `active_bg` key at all (as if serialized before the type existed)
    /// still deserializes, defaulting the field to `None`. It's a sidecar
    /// rather than a field on `ActivityBar` itself specifically so that no
    /// existing `ActivityBar { .. }` literal — in-tree or downstream — ever
    /// needs to change; see `ActivityBarStyle`'s doc for the full reasoning.
    #[test]
    fn activity_bar_style_roundtrip_and_defaults_to_none_for_pre_658_payloads() {
        let style = ActivityBarStyle::new().with_active_bg(Color::rgb(49, 50, 51));
        let json = serde_json::to_string(&style).unwrap();
        let back: ActivityBarStyle = serde_json::from_str(&json).unwrap();
        assert_eq!(style, back);

        let old_json = "{}";
        let defaulted: ActivityBarStyle = serde_json::from_str(old_json).unwrap();
        assert_eq!(defaulted, ActivityBarStyle::default());
        assert_eq!(defaulted.active_bg, None);
    }

    /// `icon_size_px` round-trips, defaults to `None` for
    /// pre-existing payloads, and `resolved_icon_size_px` falls back to
    /// [`DEFAULT_ACTIVITY_ICON_SIZE_PX`] when unset.
    #[test]
    fn activity_bar_style_icon_size_roundtrips_and_defaults() {
        let style = ActivityBarStyle::new().with_icon_size_px(18.0);
        let json = serde_json::to_string(&style).unwrap();
        let back: ActivityBarStyle = serde_json::from_str(&json).unwrap();
        assert_eq!(style, back);
        assert_eq!(back.resolved_icon_size_px(), 18.0);

        let old_json = "{}";
        let defaulted: ActivityBarStyle = serde_json::from_str(old_json).unwrap();
        assert_eq!(defaulted.icon_size_px, None);
        assert_eq!(
            defaulted.resolved_icon_size_px(),
            DEFAULT_ACTIVITY_ICON_SIZE_PX
        );
    }

    #[test]
    fn activity_bar_event_roundtrip_serde() {
        let events = vec![
            ActivityBarEvent::ItemClicked {
                id: WidgetId::new("activity:git"),
            },
            ActivityBarEvent::KeyPressed {
                key: "Escape".to_string(),
                modifiers: Modifiers::default(),
            },
        ];
        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let back: ActivityBarEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(event, &back);
        }
    }

    // ── D6 ActivityBar layout API tests ───────────────────────────────

    fn make_activity_item(id: &str, icon: char) -> ActivityItem {
        ActivityItem {
            id: WidgetId::new(id),
            icon: icon.to_string().into(),
            tooltip: String::new(),
            is_active: false,
            is_keyboard_selected: false,
        }
    }

    #[test]
    fn activity_bar_layout_empty() {
        let bar = ActivityBar {
            id: WidgetId::new("a"),
            top_items: vec![],
            bottom_items: vec![],
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
            style: Default::default(),
            hovered: None,
        };
        let layout = bar.layout(3.0, 20.0, 1.0);
        assert_eq!(layout.visible_items.len(), 0);
        assert_eq!(layout.hit_test(1.0, 5.0), ActivityBarHit::Empty);
    }

    // ── `From<&ActivityBar> for Toolbar` ────────────────────────────────

    #[test]
    fn from_activity_bar_orders_buttons_top_spacer_bottom() {
        let mut explorer = make_activity_item("activity:explorer", 'E');
        explorer.is_active = true;
        let mut search = make_activity_item("activity:search", 'S');
        search.is_keyboard_selected = true;
        let settings = make_activity_item("activity:settings", 'G');

        let bar = ActivityBar {
            id: WidgetId::new("activity"),
            top_items: vec![explorer, search],
            bottom_items: vec![settings],
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: true,
            style: Default::default(),
            hovered: None,
        };

        let toolbar: Toolbar = (&bar).into();
        assert_eq!(toolbar.id, bar.id);
        assert_eq!(toolbar.buttons.len(), 4, "top(2) + Spacer + bottom(1)");
        assert!(matches!(toolbar.buttons[2], ToolbarButton::Spacer));

        match &toolbar.buttons[0] {
            ToolbarButton::Action {
                id,
                is_active,
                enabled,
                ..
            } => {
                assert_eq!(id.as_str(), "activity:explorer");
                assert!(*is_active, "is_active carries through verbatim");
                assert!(*enabled, "ActivityItem has no disabled state");
            }
            other => panic!("expected Action, got {other:?}"),
        }
        match &toolbar.buttons[3] {
            ToolbarButton::Action { id, .. } => assert_eq!(id.as_str(), "activity:settings"),
            other => panic!("expected Action, got {other:?}"),
        }

        // `search` (top_items[1], flat index 1) is the keyboard-selected
        // item, so `focused_index` must point at it.
        assert_eq!(toolbar.focused_index, Some(1));
    }

    #[test]
    fn from_activity_bar_focused_index_none_when_nothing_selected() {
        let bar = ActivityBar {
            id: WidgetId::new("activity"),
            top_items: vec![make_activity_item("activity:explorer", 'E')],
            bottom_items: vec![make_activity_item("activity:settings", 'G')],
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
            style: Default::default(),
            hovered: None,
        };
        let toolbar: Toolbar = (&bar).into();
        assert_eq!(toolbar.focused_index, None);
    }

    // ── `ActivityBar::to_toolbar_parts` ─────────────────────────────────

    fn make_activity_item_with_icons(id: &str, glyph: char, fallback: char) -> ActivityItem {
        ActivityItem {
            id: WidgetId::new(id),
            icon: Icon::new(glyph.to_string(), fallback.to_string()),
            tooltip: String::new(),
            is_active: false,
            is_keyboard_selected: false,
        }
    }

    #[test]
    fn to_toolbar_parts_maps_active_bg_set() {
        let bar = ActivityBar {
            id: WidgetId::new("activity"),
            top_items: vec![make_activity_item("activity:explorer", 'E')],
            bottom_items: vec![],
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
            style: Default::default(),
            hovered: None,
        };
        let style = ActivityBarStyle::new().with_active_bg(Color::rgb(1, 2, 3));
        let theme = Theme::default();
        let (_toolbar, rail_style) = bar.to_toolbar_parts(&style, &theme, false);
        assert_eq!(rail_style.active_bg, Some(Color::rgb(1, 2, 3)));
    }

    #[test]
    fn to_toolbar_parts_maps_active_bg_unset() {
        let bar = ActivityBar {
            id: WidgetId::new("activity"),
            top_items: vec![make_activity_item("activity:explorer", 'E')],
            bottom_items: vec![],
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
            style: Default::default(),
            hovered: None,
        };
        let style = ActivityBarStyle::new();
        let theme = Theme::default();
        let (_toolbar, rail_style) = bar.to_toolbar_parts(&style, &theme, false);
        assert_eq!(rail_style.active_bg, None);
    }

    #[test]
    fn to_toolbar_parts_maps_active_accent_set() {
        let bar = ActivityBar {
            id: WidgetId::new("activity"),
            top_items: vec![make_activity_item("activity:explorer", 'E')],
            bottom_items: vec![],
            active_accent: Some(Color::rgb(4, 5, 6)),
            selection_bg: None,
            is_keyboard_focused: false,
            style: Default::default(),
            hovered: None,
        };
        let style = ActivityBarStyle::new();
        let theme = Theme::default();
        let (_toolbar, rail_style) = bar.to_toolbar_parts(&style, &theme, false);
        assert_eq!(rail_style.active_accent, Some(Color::rgb(4, 5, 6)));
    }

    #[test]
    fn to_toolbar_parts_maps_active_accent_unset() {
        let bar = ActivityBar {
            id: WidgetId::new("activity"),
            top_items: vec![make_activity_item("activity:explorer", 'E')],
            bottom_items: vec![],
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
            style: Default::default(),
            hovered: None,
        };
        let style = ActivityBarStyle::new();
        let theme = Theme::default();
        let (_toolbar, rail_style) = bar.to_toolbar_parts(&style, &theme, false);
        assert_eq!(rail_style.active_accent, None);
    }

    #[test]
    fn to_toolbar_parts_maps_selection_bg_set() {
        let bar = ActivityBar {
            id: WidgetId::new("activity"),
            top_items: vec![make_activity_item("activity:explorer", 'E')],
            bottom_items: vec![],
            active_accent: None,
            selection_bg: Some(Color::rgb(7, 8, 9)),
            is_keyboard_focused: false,
            style: Default::default(),
            hovered: None,
        };
        let style = ActivityBarStyle::new();
        let theme = Theme::default();
        let (_toolbar, rail_style) = bar.to_toolbar_parts(&style, &theme, false);
        assert_eq!(rail_style.selection_bg, Some(Color::rgb(7, 8, 9)));
    }

    #[test]
    fn to_toolbar_parts_maps_selection_bg_unset() {
        let bar = ActivityBar {
            id: WidgetId::new("activity"),
            top_items: vec![make_activity_item("activity:explorer", 'E')],
            bottom_items: vec![],
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
            style: Default::default(),
            hovered: None,
        };
        let style = ActivityBarStyle::new();
        let theme = Theme::default();
        let (_toolbar, rail_style) = bar.to_toolbar_parts(&style, &theme, false);
        assert_eq!(rail_style.selection_bg, None);
    }

    #[test]
    fn to_toolbar_parts_bg_is_theme_tab_bar_bg_and_separator_is_trailing() {
        let bar = ActivityBar {
            id: WidgetId::new("activity"),
            top_items: vec![make_activity_item("activity:explorer", 'E')],
            bottom_items: vec![],
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
            style: Default::default(),
            hovered: None,
        };
        let style = ActivityBarStyle::new();
        let theme = Theme::default();
        let (toolbar, rail_style) = bar.to_toolbar_parts(&style, &theme, false);
        assert_eq!(toolbar.bg, Some(theme.tab_bar_bg));
        assert_eq!(rail_style.trailing_separator, Some(theme.separator));
    }

    #[test]
    fn to_toolbar_parts_icon_is_glyph_when_nerd_fonts_enabled() {
        let bar = ActivityBar {
            id: WidgetId::new("activity"),
            top_items: vec![make_activity_item_with_icons("activity:explorer", 'G', 'F')],
            bottom_items: vec![],
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
            style: Default::default(),
            hovered: None,
        };
        let style = ActivityBarStyle::new();
        let theme = Theme::default();
        let (toolbar, _rail_style) = bar.to_toolbar_parts(&style, &theme, true);
        match &toolbar.buttons[0] {
            ToolbarButton::Action { icon, .. } => {
                assert_eq!(icon.as_deref(), Some("G"));
            }
            other => panic!("expected Action, got {other:?}"),
        }
    }

    #[test]
    fn to_toolbar_parts_icon_is_fallback_when_nerd_fonts_disabled() {
        let bar = ActivityBar {
            id: WidgetId::new("activity"),
            top_items: vec![make_activity_item_with_icons("activity:explorer", 'G', 'F')],
            bottom_items: vec![],
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
            style: Default::default(),
            hovered: None,
        };
        let style = ActivityBarStyle::new();
        let theme = Theme::default();
        let (toolbar, _rail_style) = bar.to_toolbar_parts(&style, &theme, false);
        match &toolbar.buttons[0] {
            ToolbarButton::Action { icon, .. } => {
                assert_eq!(icon.as_deref(), Some("F"));
            }
            other => panic!("expected Action, got {other:?}"),
        }
    }

    // ── `ActivityBar::row_hits_from_toolbar_layout` ─────────────────────

    /// Builds the expected `ActivityBarRowHit` list the same way
    /// `native_surface_paint::paint` would, directly off
    /// [`ActivityBar::layout`]'s own `visible_items` (already
    /// bottom-then-top ordered), so this test doesn't depend on any
    /// pixel/native-surface feature being enabled.
    fn expected_row_hits(
        bar: &ActivityBar,
        layout: &ActivityBarLayout,
    ) -> Vec<(f32, f32, WidgetId, String)> {
        layout
            .visible_items
            .iter()
            .map(|vi| {
                let item = match vi.side {
                    ActivitySide::Top => &bar.top_items[vi.item_idx],
                    ActivitySide::Bottom => &bar.bottom_items[vi.item_idx],
                };
                (
                    vi.bounds.y,
                    vi.bounds.y + vi.bounds.height,
                    item.id,
                    item.tooltip.clone(),
                )
            })
            .collect()
    }

    fn actual_row_hits(
        bar: &ActivityBar,
        width: f32,
        height: f32,
        item_height: f32,
    ) -> Vec<(f32, f32, WidgetId, String)> {
        let style = ActivityBarStyle::new();
        let theme = Theme::default();
        let (toolbar, _rail_style) = bar.to_toolbar_parts(&style, &theme, false);
        let toolbar_layout = toolbar.layout_with_options(
            0.0,
            0.0,
            width,
            height,
            ToolbarLayoutOptions::new()
                .with_orientation(ToolbarOrientation::Vertical)
                .with_pin_trailing_overflow_to_origin(true),
            |_btn| ToolbarItemMeasure::new(item_height),
        );
        bar.row_hits_from_toolbar_layout(&toolbar_layout)
            .into_iter()
            .map(|hit| (hit.y_start, hit.y_end, hit.id, hit.tooltip))
            .collect()
    }

    #[test]
    fn row_hits_from_toolbar_layout_matches_layout_for_top_and_bottom_bar() {
        let bar = ActivityBar {
            id: WidgetId::new("a"),
            top_items: vec![
                make_activity_item("activity:explorer", 'E'),
                make_activity_item("activity:search", 'S'),
            ],
            bottom_items: vec![make_activity_item("activity:settings", 'G')],
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
            style: Default::default(),
            hovered: None,
        };
        let layout = bar.layout(3.0, 10.0, 1.0);
        assert_eq!(
            expected_row_hits(&bar, &layout),
            actual_row_hits(&bar, 3.0, 10.0, 1.0)
        );
    }

    /// Same bottom-overflow case as
    /// `activity_bar_layout_bottom_overflow_drops_last_item_not_first`:
    /// the row-hit helper must drop the same (last) item `ActivityBar::layout`
    /// drops, not re-derive a different overflow policy of its own.
    #[test]
    fn row_hits_from_toolbar_layout_matches_layout_on_bottom_overflow() {
        let bar = ActivityBar {
            id: WidgetId::new("a"),
            top_items: vec![],
            bottom_items: vec![
                make_activity_item("bot:0", '0'),
                make_activity_item("bot:1", '1'),
                make_activity_item("bot:2", '2'),
            ],
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
            style: Default::default(),
            hovered: None,
        };
        let layout = bar.layout(3.0, 2.0, 1.0);
        let expected = expected_row_hits(&bar, &layout);
        assert_eq!(expected.len(), 2, "only 2 of 3 items fit");
        assert_eq!(expected, actual_row_hits(&bar, 3.0, 2.0, 1.0));
    }

    /// [`ActivityBar::layout`] delegates its geometry to
    /// [`Toolbar::layout_with_options`] under the hood; this pins that
    /// the two stay bit-for-bit in agreement — same bounds, same flat
    /// `Toolbar::buttons` index — for a bar whose top and bottom groups
    /// collide (bottom wins), the case most likely to drift if the two
    /// engines ever disagree.
    #[test]
    fn layout_matches_toolbar_layout_with_options_on_collision() {
        let bar = ActivityBar {
            id: WidgetId::new("a"),
            top_items: (0..5)
                .map(|i| make_activity_item(&format!("top:{i}"), 'T'))
                .collect(),
            bottom_items: (0..3)
                .map(|i| make_activity_item(&format!("bot:{i}"), 'B'))
                .collect(),
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
            style: Default::default(),
            hovered: None,
        };
        let layout = bar.layout(3.0, 6.0, 1.0);

        let toolbar: Toolbar = (&bar).into();
        let toolbar_layout = toolbar.layout_with_options(
            0.0,
            0.0,
            3.0,
            6.0,
            ToolbarLayoutOptions::new()
                .with_orientation(ToolbarOrientation::Vertical)
                .with_pin_trailing_overflow_to_origin(true),
            |_btn| ToolbarItemMeasure::new(1.0),
        );

        // Every visible `ActivityBar::layout` item has a matching
        // `Toolbar`-layout entry with the same bounds — catches an
        // index-mapping slip.
        for vi in &layout.visible_items {
            let flat_idx = match vi.side {
                ActivitySide::Top => vi.item_idx,
                ActivitySide::Bottom => vi.item_idx + bar.top_items.len() + 1,
            };
            let matching = toolbar_layout
                .visible_items
                .iter()
                .find(|t| t.item_idx == flat_idx)
                .unwrap_or_else(|| panic!("no Toolbar-layout entry for flat index {flat_idx}"));
            assert_eq!(
                matching.bounds, vi.bounds,
                "ActivityBar::layout and Toolbar::layout_with_options must agree \
                 on bounds for flat index {flat_idx}"
            );
        }

        // The visible-item *counts* also agree — catches a dropped item
        // that the per-entry loop above (which only walks what
        // `ActivityBar::layout` kept) can't see on its own.
        let toolbar_visible_actions = toolbar_layout
            .visible_items
            .iter()
            .filter(|t| t.kind == ToolbarItemKind::Action && t.bounds.height > 0.0)
            .count();
        assert_eq!(
            layout.visible_items.len(),
            toolbar_visible_actions,
            "ActivityBar::layout must not drop or add items relative to the Toolbar layout"
        );
    }

    /// When the bottom-pinned group alone is taller than the viewport
    /// (no top items in the way), it packs from `y=0` downward and drops
    /// its *last* item, via
    /// [`ToolbarLayoutOptions::pin_trailing_overflow_to_origin`] —
    /// rather than `Toolbar`'s own default of anchoring the trailing
    /// group to the far edge and dropping its *first* item.
    #[test]
    fn activity_bar_layout_bottom_overflow_drops_last_item_not_first() {
        let bar = ActivityBar {
            id: WidgetId::new("a"),
            top_items: vec![],
            bottom_items: vec![
                make_activity_item("bot:0", '0'),
                make_activity_item("bot:1", '1'),
                make_activity_item("bot:2", '2'),
            ],
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
            style: Default::default(),
            hovered: None,
        };
        let layout = bar.layout(3.0, 2.0, 1.0);

        assert_eq!(layout.visible_items.len(), 2, "only 2 of 3 items fit");
        let item0 = layout.hit_test(1.0, 0.5);
        let item1 = layout.hit_test(1.0, 1.5);
        assert_eq!(
            item0,
            ActivityBarHit::Item(WidgetId::new("bot:0")),
            "first bottom item still occupies row 0"
        );
        assert_eq!(
            item1,
            ActivityBarHit::Item(WidgetId::new("bot:1")),
            "second bottom item still occupies row 1"
        );
        for vi in &layout.visible_items {
            assert_ne!(
                vi.item_idx, 2,
                "the third (last) bottom item is the one that drops, not the first"
            );
        }
    }

    #[test]
    fn activity_bar_layout_top_only() {
        let bar = ActivityBar {
            id: WidgetId::new("a"),
            top_items: vec![
                make_activity_item("activity:explorer", 'E'),
                make_activity_item("activity:search", 'S'),
            ],
            bottom_items: vec![],
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
            style: Default::default(),
            hovered: None,
        };
        let layout = bar.layout(3.0, 10.0, 1.0);
        assert_eq!(layout.visible_items.len(), 2);
        assert_eq!(layout.visible_items[0].side, ActivitySide::Top);
        assert_eq!(layout.visible_items[0].bounds.y, 0.0);
        assert_eq!(layout.visible_items[1].bounds.y, 1.0);
        match layout.hit_test(1.0, 0.5) {
            ActivityBarHit::Item(id) => assert_eq!(id.as_str(), "activity:explorer"),
            _ => panic!("expected explorer hit"),
        }
    }

    #[test]
    fn activity_bar_layout_bottom_pinned() {
        let bar = ActivityBar {
            id: WidgetId::new("a"),
            top_items: vec![make_activity_item("activity:explorer", 'E')],
            bottom_items: vec![make_activity_item("activity:settings", 'G')],
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
            style: Default::default(),
            hovered: None,
        };
        // Viewport 10, items 1 each. Top at y=0, bottom at y=9.
        let layout = bar.layout(3.0, 10.0, 1.0);
        assert_eq!(layout.visible_items.len(), 2);
        let top = layout
            .visible_items
            .iter()
            .find(|v| v.side == ActivitySide::Top)
            .unwrap();
        let bot = layout
            .visible_items
            .iter()
            .find(|v| v.side == ActivitySide::Bottom)
            .unwrap();
        assert_eq!(top.bounds.y, 0.0);
        assert_eq!(bot.bounds.y, 9.0);
        // Click near top → explorer. Click near bottom → settings.
        match layout.hit_test(1.0, 0.5) {
            ActivityBarHit::Item(id) => assert_eq!(id.as_str(), "activity:explorer"),
            _ => panic!(),
        }
        match layout.hit_test(1.0, 9.5) {
            ActivityBarHit::Item(id) => assert_eq!(id.as_str(), "activity:settings"),
            _ => panic!(),
        }
    }

    #[test]
    fn activity_bar_layout_bottom_wins_on_collision() {
        // 5 top items + 3 bottom items, item_height=1, viewport=6.
        // Bottom reserves [3, 6). Top stops at y=3 → only 3 top items fit.
        let bar = ActivityBar {
            id: WidgetId::new("a"),
            top_items: (0..5)
                .map(|i| make_activity_item(&format!("top:{i}"), 'T'))
                .collect(),
            bottom_items: (0..3)
                .map(|i| make_activity_item(&format!("bot:{i}"), 'B'))
                .collect(),
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
            style: Default::default(),
            hovered: None,
        };
        let layout = bar.layout(3.0, 6.0, 1.0);
        let top_count = layout
            .visible_items
            .iter()
            .filter(|v| v.side == ActivitySide::Top)
            .count();
        let bot_count = layout
            .visible_items
            .iter()
            .filter(|v| v.side == ActivitySide::Bottom)
            .count();
        assert_eq!(bot_count, 3, "all bottom items visible");
        assert_eq!(
            top_count, 3,
            "top truncated to fit above bottom reserved area"
        );
    }

    /// Same overflow rule as
    /// `activity_bar_layout_bottom_overflow_drops_last_item_not_first`,
    /// at pixel-backend scale (48px rows, 100px strip): rects are
    /// `y=0 h=48`, `y=48 h=48`, `y=96 h=4` — the last item clips to the
    /// 4px left over rather than the group re-anchoring so an earlier
    /// item clips instead.
    #[test]
    fn activity_bar_layout_pixel_bottom_overflow_matches_legacy_rects() {
        let bar = ActivityBar {
            id: WidgetId::new("a"),
            top_items: vec![],
            bottom_items: vec![
                make_activity_item("bot:0", '0'),
                make_activity_item("bot:1", '1'),
                make_activity_item("bot:2", '2'),
            ],
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
            style: Default::default(),
            hovered: None,
        };
        let layout = bar.layout(48.0, 100.0, 48.0);

        assert_eq!(layout.visible_items.len(), 3, "all 3 rows paint, clipped");
        let item0 = layout
            .visible_items
            .iter()
            .find(|v| v.item_idx == 0)
            .unwrap();
        let item1 = layout
            .visible_items
            .iter()
            .find(|v| v.item_idx == 1)
            .unwrap();
        let item2 = layout
            .visible_items
            .iter()
            .find(|v| v.item_idx == 2)
            .unwrap();
        assert_eq!(item0.bounds.y, 0.0);
        assert_eq!(item0.bounds.height, 48.0);
        assert_eq!(item1.bounds.y, 48.0);
        assert_eq!(item1.bounds.height, 48.0);
        assert_eq!(item2.bounds.y, 96.0);
        assert_eq!(item2.bounds.height, 4.0, "clipped to the 4px left over");
    }

    #[test]
    fn activity_bar_layout_pixel_units() {
        // GTK-style: 48 px item height, 200 px strip.
        let bar = ActivityBar {
            id: WidgetId::new("a"),
            top_items: (0..3)
                .map(|i| make_activity_item(&format!("top:{i}"), 'T'))
                .collect(),
            bottom_items: vec![make_activity_item("activity:settings", 'G')],
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
            style: Default::default(),
            hovered: None,
        };
        let layout = bar.layout(48.0, 200.0, 48.0);
        // Top items at y = 0, 48, 96. Settings at y = 200 - 48 = 152.
        let top0 = layout
            .visible_items
            .iter()
            .find(|v| v.side == ActivitySide::Top && v.item_idx == 0)
            .unwrap();
        assert_eq!(top0.bounds.y, 0.0);
        assert_eq!(top0.bounds.height, 48.0);
        let bot0 = layout
            .visible_items
            .iter()
            .find(|v| v.side == ActivitySide::Bottom)
            .unwrap();
        assert_eq!(bot0.bounds.y, 152.0);
    }
}

/// `native_surface_paint::paint`'s own tests — only compiled alongside
/// that module (gated the same way).
#[cfg(all(
    test,
    any(
        feature = "gtk",
        feature = "win",
        all(feature = "macos", target_os = "macos")
    )
))]
mod native_surface_paint_tests {
    use super::{native_surface_paint, ActivityBar, ActivityBarStyle, ActivityItem};
    use crate::event::{Point, Rect, Viewport};
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;
    use crate::types::{Color, Icon, WidgetId};
    use crate::{backend::ImagePaintResult, Image};

    /// Records every fill/text call this module's `paint` makes — enough
    /// to pin the exact row-hit output and the relative paint order of
    /// `active_bg` / hover / `selection_bg` / `active_accent`, without a
    /// Cairo/Core Graphics/Direct2D host.
    #[derive(Default)]
    struct RecordingSurface {
        fills: Vec<(Rect, Color)>,
    }

    impl PaintSurface for RecordingSurface {
        fn surface_begin_frame(&mut self, _viewport: Viewport) {}
        fn surface_end_frame(&mut self) {}
        fn surface_viewport(&self) -> Viewport {
            Viewport::new(48.0, 144.0, 1.0)
        }
        fn surface_line_height(&self) -> f32 {
            14.0
        }
        fn surface_char_width(&self) -> f32 {
            7.0
        }
        fn surface_measure_text(&self, text: &str) -> (f32, f32) {
            (text.chars().count() as f32 * 7.0, 12.0)
        }
        fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
            self.fills.push((rect, color));
        }
        fn surface_fill_rounded_rect(&mut self, rect: Rect, _radius: f32, color: Color) {
            self.fills.push((rect, color));
        }
        fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
        fn surface_draw_text_run(&mut self, _rect: Rect, _text: &str, _color: Color) {}
        fn surface_draw_line(&mut self, _from: Point, _to: Point, _color: Color, _sw: f32) {}
        fn surface_push_clip(&mut self, _rect: Rect) {}
        fn surface_pop_clip(&mut self) {}
        fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
            ImagePaintResult::Unsupported
        }
    }

    /// Two top items (the first active, keyboard-selected and hovered;
    /// the second plain) plus one bottom-pinned item — enough rows to
    /// pin both the seam helper's bottom-first row-hit order and the
    /// single active row's fill priority.
    #[test]
    fn paint_pins_row_hit_order_and_fill_priority() {
        let active_bg = Color::rgb(10, 20, 30);
        let selection_bg = Color::rgb(60, 70, 80);
        let accent = Color::rgb(200, 100, 50);

        let bar = ActivityBar {
            id: WidgetId::new("bar"),
            top_items: vec![
                ActivityItem {
                    id: WidgetId::new("activity:explorer"),
                    icon: Icon::new("E", "e"),
                    tooltip: "Explorer".to_string(),
                    is_active: true,
                    is_keyboard_selected: true,
                },
                ActivityItem {
                    id: WidgetId::new("activity:search"),
                    icon: Icon::new("S", "s"),
                    tooltip: "Search".to_string(),
                    is_active: false,
                    is_keyboard_selected: false,
                },
            ],
            bottom_items: vec![ActivityItem {
                id: WidgetId::new("activity:settings"),
                icon: Icon::new("G", "g"),
                tooltip: "Settings".to_string(),
                is_active: false,
                is_keyboard_selected: false,
            }],
            active_accent: Some(accent),
            selection_bg: Some(selection_bg),
            is_keyboard_focused: false,
            style: Default::default(),
            hovered: None,
        };
        let style = ActivityBarStyle::new().with_active_bg(active_bg);
        let theme = Theme::default();
        let layout = bar.layout(48.0, 144.0, 48.0);
        let mut surface = RecordingSurface::default();

        // `layout.visible_items` is bottom-first: settings (0), explorer
        // (1), search (2) — hover the explorer row.
        let regions = native_surface_paint::paint(
            &bar,
            &layout,
            Point::new(0.0, 0.0),
            &style,
            &mut surface,
            &theme,
            Some(1),
            false,
        );

        assert_eq!(
            regions.len(),
            3,
            "row hits were {regions:?}, expected 3 visible rows"
        );
        assert_eq!(regions[0].id, WidgetId::new("activity:settings"));
        assert_eq!(regions[0].y_start, 96.0);
        assert_eq!(regions[0].y_end, 144.0);
        assert_eq!(regions[1].id, WidgetId::new("activity:explorer"));
        assert_eq!(regions[1].tooltip, "Explorer");
        assert_eq!(regions[1].y_start, 0.0);
        assert_eq!(regions[1].y_end, 48.0);
        assert_eq!(regions[2].id, WidgetId::new("activity:search"));
        assert_eq!(regions[2].y_start, 48.0);
        assert_eq!(regions[2].y_end, 96.0);

        let hover_bg = theme.tab_bar_bg.lighten(0.10);
        let pos = |color: Color| {
            surface
                .fills
                .iter()
                .position(|(_, c)| *c == color)
                .unwrap_or_else(|| {
                    panic!("no fill matched {color:?}; fills were {:?}", surface.fills)
                })
        };
        let active_pos = pos(active_bg);
        let hover_pos = pos(hover_bg);
        let selection_pos = pos(selection_bg);
        let accent_pos = pos(accent);
        assert!(
            active_pos < hover_pos && hover_pos < selection_pos && selection_pos < accent_pos,
            "fill order must be active < hover < selection < accent; positions were \
             active={active_pos} hover={hover_pos} selection={selection_pos} accent={accent_pos}, \
             fills were {:?}",
            surface.fills,
        );
    }
}

/// The cell-grid rail's paint/hit-region coverage, driven through
/// [`cell_paint::paint`], the painter
/// [`crate::primitive::Primitive::paint_cells`] reaches.
#[cfg(all(test, feature = "tui"))]
mod cell_paint_tests {
    use super::cell_paint::paint as draw_activity_bar;
    use super::{ActivityBar, ActivityBarStyle, ActivityItem};
    use crate::theme::Theme;
    use crate::tui::ratatui_color;
    use crate::types::{Color, Icon, WidgetId};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    fn item(id: &str, icon: &str) -> ActivityItem {
        ActivityItem {
            id: WidgetId::new(id),
            icon: icon.into(),
            tooltip: format!("{id} tooltip"),
            is_active: false,
            is_keyboard_selected: false,
        }
    }

    fn bar() -> ActivityBar {
        ActivityBar {
            id: WidgetId::new("activity"),
            top_items: vec![item("explorer", "E"), item("search", "S"), item("git", "G")],
            bottom_items: vec![item("settings", "*")],
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
            style: Default::default(),
            hovered: None,
        }
    }

    /// Hit regions are **bar-relative**, not absolute, so the top row
    /// starts at `0.0` no matter where the bar is painted.
    ///
    /// Looks rows up **by id** rather than by position: `visible_items`
    /// (and therefore this list) is bottom-pinned-first, not visual
    /// top-to-bottom order.
    #[test]
    fn hit_regions_are_bar_relative_not_absolute() {
        let b = bar();
        let theme = Theme::default();

        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 20));
        let area = Rect::new(0, 3, 3, 10);
        let hits = draw_activity_bar(&mut buf, area, &b, &theme, false);

        let span = |id: &str| {
            let h = hits
                .iter()
                .find(|h| h.id.as_str() == id)
                .unwrap_or_else(|| panic!("no hit region for {id:?}"));
            (h.y_start, h.y_end)
        };

        assert_eq!(
            span("explorer"),
            (0.0, 1.0),
            "the top row must start at 0.0 relative to the bar, not at \
             area.y ({}) — folding the origin in here double-counts it \
             against AppShell's own `+ activity_bar_bounds.y`",
            area.y
        );
        assert_eq!(span("search"), (1.0, 2.0), "second row is one row down");
        assert_eq!(span("git"), (2.0, 3.0));
        // Bottom-pinned: `area.height` (10) - 1 row, still measured from
        // the bar's own top edge, not the screen's.
        assert_eq!(span("settings"), (9.0, 10.0));
    }

    /// The same regions must come back for *any* `area.y`. Pinning
    /// invariance rather than a single value is what stops the bug from
    /// reappearing: output is correct at `area.y == 0` (title bar hidden)
    /// and would be wrong everywhere else if the origin leaked in, which
    /// is exactly why a static single-origin test passes either way.
    #[test]
    fn hit_regions_do_not_move_when_the_bar_does() {
        let b = bar();
        let theme = Theme::default();

        let spans = |y: u16| {
            let mut buf = Buffer::empty(Rect::new(0, 0, 40, 30));
            let hits = draw_activity_bar(&mut buf, Rect::new(0, y, 3, 10), &b, &theme, false);
            hits.iter()
                .map(|h| (h.y_start, h.y_end))
                .collect::<Vec<_>>()
        };

        let at_zero = spans(0);
        assert!(!at_zero.is_empty(), "sanity: some rows were painted");
        for y in [1u16, 2, 5] {
            assert_eq!(
                spans(y),
                at_zero,
                "hit regions must be independent of the bar's screen origin; \
                 they differed at area.y = {y}"
            );
        }
    }

    /// Painting uses the absolute row; hit regions are bar-relative.
    #[test]
    fn icons_still_paint_at_the_absolute_row() {
        let b = bar();
        let theme = Theme::default();
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 20));
        let area = Rect::new(0, 3, 4, 10);
        draw_activity_bar(&mut buf, area, &b, &theme, false);

        let row_text = |y: u16| {
            (area.x..area.x + area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        };
        assert!(
            row_text(3).contains('E'),
            "first icon should paint on the bar's first *screen* row (3), \
             got {:?}",
            row_text(3)
        );
        assert!(
            row_text(4).contains('S'),
            "second icon on screen row 4, got {:?}",
            row_text(4)
        );
    }

    /// `nerd_fonts_enabled` picks `Icon::glyph` when `true` and
    /// `Icon::fallback` when `false`.
    #[test]
    fn nerd_fonts_flag_selects_glyph_or_fallback() {
        let mut b = bar();
        b.top_items[0].icon = Icon::new("\u{f07c}", "E");
        let theme = Theme::default();

        let row_text = |nerd_fonts_enabled: bool| {
            let mut buf = Buffer::empty(Rect::new(0, 0, 40, 20));
            let area = Rect::new(0, 0, 4, 10);
            draw_activity_bar(&mut buf, area, &b, &theme, nerd_fonts_enabled);
            (area.x..area.x + area.width)
                .map(|x| buf[(x, area.y)].symbol().to_string())
                .collect::<String>()
        };

        assert!(
            row_text(true).contains('\u{f07c}'),
            "nerd_fonts_enabled: true should paint the glyph half of the Icon"
        );
        assert!(
            row_text(false).contains('E'),
            "nerd_fonts_enabled: false should paint the fallback half of the Icon"
        );
    }

    /// `style.active_bg: Some(..)` with `active_accent: None` fills the
    /// active row's cells with the requested colour and paints **zero**
    /// accent-line pixels — the accent column (`area.x`) must not carry
    /// the `'▎'` glyph, only the plain fill background.
    #[test]
    fn active_bg_fills_row_cells_with_zero_accent_glyph_when_accent_is_none() {
        let mut b = bar();
        b.top_items[0].is_active = true; // "explorer"
        let theme = Theme::default();
        let fill_color = Color::rgb(49, 50, 51);
        let style = ActivityBarStyle::new().with_active_bg(fill_color);

        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 20));
        let area = Rect::new(0, 0, 3, 10);
        b.style = style;
        draw_activity_bar(&mut buf, area, &b, &theme, false);

        let fill = ratatui_color(fill_color);
        assert_ne!(
            buf[(area.x, area.y)].symbol(),
            "▎",
            "active_accent is None: the accent column must not carry the \
             accent glyph (zero accent-line pixels)"
        );
        assert_eq!(
            buf[(area.x, area.y)].bg,
            fill,
            "accent column should carry the active_bg fill, not the plain bar bg"
        );
        let icon_col = area.x + 1;
        assert_eq!(
            buf[(icon_col, area.y)].bg,
            fill,
            "icon column should also carry the active_bg fill"
        );
    }

    /// The flip side: `active_accent: Some(..)` with no `style.active_bg`
    /// still paints the traditional accent glyph, and the row keeps the
    /// plain bar background elsewhere.
    #[test]
    fn active_accent_paints_glyph_when_set_with_no_active_bg() {
        let mut b = bar();
        b.top_items[0].is_active = true; // "explorer"
        b.active_accent = Some(Color::rgb(80, 140, 255));
        let theme = Theme::default();

        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 20));
        let area = Rect::new(0, 0, 3, 10);
        b.style = ActivityBarStyle::default();
        draw_activity_bar(&mut buf, area, &b, &theme, false);

        assert_eq!(
            buf[(area.x, area.y)].symbol(),
            "▎",
            "active_accent is set: the accent column should carry the accent glyph"
        );
        let icon_col = area.x + 1;
        assert_eq!(
            buf[(icon_col, area.y)].bg,
            ratatui_color(theme.tab_bar_bg),
            "with no active_bg, the icon column keeps the plain bar background"
        );
    }
}

/// [`ActivityBar`]'s own `Primitive`/`Interactive` impls, exercised
/// directly: layout in both unit regimes, and every gesture the widget
/// resolves.
#[cfg(test)]
mod widget_contract_tests {
    use super::{
        ActivityBar, ActivityBarAction, ActivityBarHit, ActivityBarState, ActivityItem,
        ActivitySide, CELL_BAR_WIDTH, PIXEL_ROW_HEIGHT,
    };
    use crate::compose::Handled;
    use crate::event::{Key, MouseButton, NamedKey, Point};
    use crate::interactive::{
        EventCx, EventHost, FocusSpec, Interactive, KeyEvent, PointerKind, RoutedEvent, SubTarget,
    };
    use crate::primitive::{LayoutCtx, PaintState, Primitive, Units};
    use crate::types::WidgetId;
    use crate::Rect;

    /// Records the [`EventHost`] intents a `handle` call makes.
    #[derive(Default)]
    struct RecordingHost {
        redraws: usize,
    }

    impl EventHost for RecordingHost {
        fn is_focused(&self, _id: &WidgetId, _sub: Option<SubTarget>) -> bool {
            false
        }
        fn request_focus(&mut self, _id: &WidgetId, _sub: Option<SubTarget>) {}
        fn capture_pointer(&mut self, _id: &WidgetId) {}
        fn release_pointer(&mut self) {}
        fn set_cursor(&mut self, _cursor: crate::PointerShape) {}
        fn request_redraw(&mut self) {
            self.redraws += 1;
        }
        fn emit(&mut self, _event: crate::UiEvent) {}
        fn units(&self) -> Units {
            Units::Cells
        }
    }

    fn item(id: &str, icon: &str) -> ActivityItem {
        ActivityItem {
            id: WidgetId::new(id),
            icon: icon.into(),
            tooltip: String::new(),
            is_active: false,
            is_keyboard_selected: false,
        }
    }

    fn bar() -> ActivityBar {
        ActivityBar {
            id: WidgetId::new("activity"),
            top_items: vec![item("explorer", "E"), item("search", "S")],
            bottom_items: vec![item("settings", "*")],
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
            style: Default::default(),
            hovered: None,
        }
    }

    /// 4 columns wide, 10 rows tall: enough room for both groups without
    /// a collision.
    const AREA: Rect = Rect::new(0.0, 0.0, 4.0, 10.0);

    fn cell_ctx() -> LayoutCtx<'static> {
        LayoutCtx::new(1.0, 1.0, false)
    }

    fn pointer(kind: PointerKind, x: f32, y: f32) -> RoutedEvent {
        RoutedEvent::Pointer {
            kind,
            sub: None,
            local: Point::new(x, y),
            mods: Default::default(),
        }
    }

    fn key(named: NamedKey) -> RoutedEvent {
        RoutedEvent::Key(KeyEvent {
            key: Key::Named(named),
            modifiers: Default::default(),
            repeat: false,
        })
    }

    #[test]
    fn the_logical_sequence_is_top_items_then_bottom_items() {
        let bar = bar();
        assert_eq!(bar.item_count(), 3);
        assert_eq!(
            bar.item_at(0).map(|i| i.id),
            Some(WidgetId::new("explorer"))
        );
        assert_eq!(
            bar.item_at(2).map(|i| i.id),
            Some(WidgetId::new("settings"))
        );
        assert_eq!(bar.item_at(3), None);
    }

    #[test]
    fn cell_layout_puts_one_row_per_item_and_pins_the_bottom_group() {
        let bar = bar();
        let layout = Primitive::layout(&bar, &cell_ctx(), Rect::new(2.0, 7.0, 4.0, 10.0));

        assert_eq!(layout.area, Rect::new(2.0, 7.0, 4.0, 10.0));
        // `visible_items` is bottom-pinned-first; the bottom group sits on
        // the bar's own last row, bar-relative.
        let settings = layout.layout.visible_items[0];
        assert_eq!(settings.side, ActivitySide::Bottom);
        assert_eq!(settings.bounds.y, 9.0);
        assert_eq!(settings.bounds.height, 1.0);
        assert_eq!(
            layout.hit_test(1.0, 0.5),
            ActivityBarHit::Item(WidgetId::new("explorer"))
        );
    }

    #[test]
    fn pixel_layout_uses_the_shared_fixed_row_height() {
        let bar = bar();
        let ctx = LayoutCtx::new(16.0, 8.0, false);
        assert_eq!(ctx.units(), Units::Pixels);
        let layout = Primitive::layout(&bar, &ctx, Rect::new(0.0, 0.0, 48.0, 480.0));

        for vi in &layout.layout.visible_items {
            assert_eq!(vi.bounds.height, PIXEL_ROW_HEIGHT);
        }
    }

    #[test]
    fn measure_reports_a_fixed_rail_width_and_stretches_vertically() {
        let bar = bar();
        let natural =
            Primitive::measure(&bar, &cell_ctx(), crate::primitive::Size::new(40.0, 20.0));
        assert_eq!(natural.width, Some(CELL_BAR_WIDTH));
        assert_eq!(natural.height, None);
    }

    #[test]
    fn the_rail_is_one_tab_stop_and_an_empty_rail_is_none() {
        assert_eq!(Interactive::focus(&bar()), FocusSpec::Stop);

        let empty = ActivityBar {
            top_items: vec![],
            bottom_items: vec![],
            ..bar()
        };
        assert_eq!(Interactive::focus(&empty), FocusSpec::None);
    }

    #[test]
    fn sub_regions_name_every_visible_row_in_bar_local_space() {
        let bar = bar();
        let layout = Primitive::layout(&bar, &cell_ctx(), AREA);
        let regions = Interactive::sub_regions(&bar, &layout);

        assert_eq!(regions.len(), layout.layout.visible_items.len());
        assert_eq!(regions[0].0, layout.layout.visible_items[0].bounds);
        assert_eq!(regions[0].1, SubTarget(0));
    }

    #[test]
    fn clicking_a_row_activates_that_item_and_parks_the_cursor_on_it() {
        let bar = bar();
        let layout = Primitive::layout(&bar, &cell_ctx(), AREA);
        let mut state = ActivityBarState::default();
        let mut host = RecordingHost::default();
        let mut cx = EventCx::new(bar.id, &mut host);

        // Row 1 (bar-relative) is the second top item, "search".
        let handled = Interactive::handle(
            &bar,
            &mut state,
            &pointer(PointerKind::Down(MouseButton::Left), 1.0, 1.5),
            &layout,
            &mut cx,
        );
        assert_eq!(
            handled,
            Handled::Event(ActivityBarAction::ItemActivated(WidgetId::new("search")))
        );
        assert_eq!(state.cursor, 1, "the cursor follows the click");
    }

    /// The bottom-pinned group's own click must resolve to its own item,
    /// which only holds if `visible_items`' bottom-first order is
    /// translated back into the logical sequence.
    #[test]
    fn clicking_the_bottom_group_activates_the_bottom_item() {
        let bar = bar();
        let layout = Primitive::layout(&bar, &cell_ctx(), AREA);
        let mut state = ActivityBarState::default();
        let mut host = RecordingHost::default();
        let mut cx = EventCx::new(bar.id, &mut host);

        let handled = Interactive::handle(
            &bar,
            &mut state,
            &pointer(PointerKind::Down(MouseButton::Left), 1.0, 9.5),
            &layout,
            &mut cx,
        );
        assert_eq!(
            handled,
            Handled::Event(ActivityBarAction::ItemActivated(WidgetId::new("settings")))
        );
        assert_eq!(state.cursor, 2);
    }

    #[test]
    fn a_click_on_a_blank_row_activates_nothing() {
        let bar = bar();
        let layout = Primitive::layout(&bar, &cell_ctx(), AREA);
        let mut state = ActivityBarState::default();
        let mut host = RecordingHost::default();
        let mut cx = EventCx::new(bar.id, &mut host);

        let handled = Interactive::handle(
            &bar,
            &mut state,
            &pointer(PointerKind::Down(MouseButton::Left), 1.0, 5.5),
            &layout,
            &mut cx,
        );
        assert_eq!(handled, Handled::Ignored);
    }

    /// A right click is deliberately not claimed, so the host's own
    /// context-menu policy still sees the raw mouse event.
    #[test]
    fn a_right_click_is_left_for_the_host() {
        let bar = bar();
        let layout = Primitive::layout(&bar, &cell_ctx(), AREA);
        let mut state = ActivityBarState::default();
        let mut host = RecordingHost::default();
        let mut cx = EventCx::new(bar.id, &mut host);

        let handled = Interactive::handle(
            &bar,
            &mut state,
            &pointer(PointerKind::Down(MouseButton::Right), 1.0, 0.5),
            &layout,
            &mut cx,
        );
        assert_eq!(handled, Handled::Ignored);
    }

    #[test]
    fn hover_tracks_the_visible_row_under_the_pointer() {
        let bar = bar();
        let layout = Primitive::layout(&bar, &cell_ctx(), AREA);
        let mut state = ActivityBarState::default();
        let mut host = RecordingHost::default();
        let mut cx = EventCx::new(bar.id, &mut host);

        let handled = Interactive::handle(
            &bar,
            &mut state,
            &pointer(PointerKind::Enter, 1.0, 0.5),
            &layout,
            &mut cx,
        );
        assert_eq!(handled, Handled::Consumed);
        // `visible_items` is bottom-pinned-first, so the top group's first
        // row is index 1 — and that is the index a rasteriser's
        // `hovered_idx` parameter wants.
        let expected = layout
            .visible_index_at(1.0, 0.5)
            .expect("the top row is visible");
        assert_eq!(state.hovered_idx, Some(expected));
        assert_eq!(PaintState::hovered_idx(&state), Some(expected));

        let handled = Interactive::handle(
            &bar,
            &mut state,
            &pointer(PointerKind::Leave, 1.0, 0.5),
            &layout,
            &mut cx,
        );
        assert_eq!(handled, Handled::Consumed);
        assert_eq!(state.hovered_idx, None);
    }

    #[test]
    fn down_and_up_walk_the_logical_sequence_without_wrapping() {
        let bar = bar();
        let layout = Primitive::layout(&bar, &cell_ctx(), AREA);
        let mut state = ActivityBarState::default();
        let mut host = RecordingHost::default();
        let mut cx = EventCx::new(bar.id, &mut host);

        for expected in [1usize, 2, 2] {
            let handled =
                Interactive::handle(&bar, &mut state, &key(NamedKey::Down), &layout, &mut cx);
            assert_eq!(handled, Handled::Consumed);
            assert_eq!(
                state.cursor, expected,
                "Down must saturate at the last item"
            );
        }
        for expected in [1usize, 0, 0] {
            let handled =
                Interactive::handle(&bar, &mut state, &key(NamedKey::Up), &layout, &mut cx);
            assert_eq!(handled, Handled::Consumed);
            assert_eq!(state.cursor, expected, "Up must saturate at the first item");
        }
    }

    #[test]
    fn enter_activates_the_cursors_item() {
        let bar = bar();
        let layout = Primitive::layout(&bar, &cell_ctx(), AREA);
        let mut state = ActivityBarState::default();
        let mut host = RecordingHost::default();
        let mut cx = EventCx::new(bar.id, &mut host);

        let _ = Interactive::handle(&bar, &mut state, &key(NamedKey::Down), &layout, &mut cx);
        let handled =
            Interactive::handle(&bar, &mut state, &key(NamedKey::Enter), &layout, &mut cx);
        assert_eq!(
            handled,
            Handled::Event(ActivityBarAction::ItemActivated(WidgetId::new("search")))
        );
    }

    /// The vim-style keys a host maps to its own engine must keep
    /// reaching it.
    #[test]
    fn an_unmapped_key_is_left_for_the_host() {
        let bar = bar();
        let layout = Primitive::layout(&bar, &cell_ctx(), AREA);
        let mut state = ActivityBarState::default();
        let mut host = RecordingHost::default();
        let mut cx = EventCx::new(bar.id, &mut host);

        let handled = Interactive::handle(
            &bar,
            &mut state,
            &RoutedEvent::Key(KeyEvent {
                key: Key::Char('j'),
                modifiers: Default::default(),
                repeat: false,
            }),
            &layout,
            &mut cx,
        );
        assert_eq!(handled, Handled::Ignored);
    }

    #[test]
    fn focus_in_and_out_track_keyboard_focus() {
        let bar = bar();
        let layout = Primitive::layout(&bar, &cell_ctx(), AREA);
        let mut state = ActivityBarState::default();
        let mut host = RecordingHost::default();
        let mut cx = EventCx::new(bar.id, &mut host);

        let _ = Interactive::handle(
            &bar,
            &mut state,
            &RoutedEvent::FocusIn(None),
            &layout,
            &mut cx,
        );
        assert!(state.focused);
        let _ = Interactive::handle(&bar, &mut state, &RoutedEvent::FocusOut, &layout, &mut cx);
        assert!(!state.focused);
    }

    #[test]
    fn the_item_action_rides_the_activity_bar_ui_event() {
        use crate::interactive::WidgetAction;
        let event = ActivityBarAction::ItemActivated(WidgetId::new("explorer"))
            .into_ui_event(WidgetId::new("activity"));
        assert_eq!(
            event,
            crate::UiEvent::ActivityBar(
                WidgetId::new("activity"),
                super::ActivityBarEvent::ItemClicked {
                    id: WidgetId::new("explorer")
                }
            )
        );
    }

    /// Three-way event parity's in-crate leg: an [`crate::a11y::A11yAction::Click`]
    /// routed through [`crate::a11y::route_widget_action`] on a fresh rail
    /// (cursor on the first item) produces the same [`crate::UiEvent`] as
    /// a mouse click on the first row. The `TuiDriver` leg lives in
    /// `tests/widget_contract_activity_bar.rs`, the binding-host leg in
    /// `quadraui-binding-core`'s engine tests.
    #[test]
    fn route_widget_action_click_matches_a_mouse_click_on_the_first_row() {
        use crate::interactive::WidgetAction;

        let bar = bar();
        let mut tree = crate::frame_tree::FrameTree::new();
        tree.push(bar.id, AREA, Box::new(bar.clone()));
        let mut states = crate::widget_state::WidgetStateStore::new();
        let via_a11y = crate::a11y::route_widget_action(
            &tree,
            &mut states,
            &cell_ctx(),
            bar.id,
            crate::a11y::A11yAction::Click,
        );

        let layout = Primitive::layout(&bar, &cell_ctx(), AREA);
        let explorer = layout
            .layout
            .visible_items
            .iter()
            .find(|vi| vi.side == ActivitySide::Top && vi.item_idx == 0)
            .expect("the first top item is visible")
            .bounds;
        let mut host = RecordingHost::default();
        let mut cx = EventCx::new(bar.id, &mut host);
        let via_mouse = Interactive::handle(
            &bar,
            &mut ActivityBarState::default(),
            &pointer(
                PointerKind::Down(MouseButton::Left),
                explorer.x + 0.5,
                explorer.y + 0.5,
            ),
            &layout,
            &mut cx,
        );

        let expected =
            ActivityBarAction::ItemActivated(WidgetId::new("explorer")).into_ui_event(bar.id);
        assert_eq!(via_a11y, vec![expected.clone()]);
        assert_eq!(
            via_mouse.map(|a| a.into_ui_event(bar.id)),
            Handled::Event(expected)
        );
    }
}

/// The macOS pixel coverage for the rail, painted through the widget
/// contract ([`crate::DrawsPrimitives::draw_dyn`]) onto a headless
/// Core Graphics bitmap.
#[cfg(all(test, feature = "macos", target_os = "macos"))]
mod mac_paint_tests {
    use super::{ActivityBar, ActivityBarPrimitiveLayout, ActivityBarRowHit, ActivityItem};
    use crate::event::{Rect as QRect, Viewport};
    use crate::macos::headless::BitmapSurface;
    use crate::macos::text::{make_font, system_ui_font};
    use crate::macos::MacBackend;
    use crate::theme::Theme;
    use crate::types::{Color, WidgetId};
    use crate::{Backend, DrawsPrimitives};
    use core_text::font::CTFont;

    const ACTIVITY_ROW_PX: f64 = super::PIXEL_ROW_HEIGHT as f64;

    const W: u32 = 48;
    const H: u32 = 240; // 5 rows × 48px

    fn font() -> CTFont {
        make_font("Menlo", 14.0).expect("Menlo installed")
    }

    fn sample_bar() -> ActivityBar {
        ActivityBar {
            id: WidgetId::new("activity"),
            top_items: vec![
                ActivityItem {
                    id: WidgetId::new("activity:explorer"),
                    icon: "E".into(),
                    tooltip: "Explorer".into(),
                    is_active: true,
                    is_keyboard_selected: false,
                },
                ActivityItem {
                    id: WidgetId::new("activity:search"),
                    icon: "S".into(),
                    tooltip: "Search".into(),
                    is_active: false,
                    is_keyboard_selected: false,
                },
            ],
            bottom_items: vec![ActivityItem {
                id: WidgetId::new("activity:settings"),
                icon: "G".into(),
                tooltip: "Settings".into(),
                is_active: false,
                is_keyboard_selected: false,
            }],
            active_accent: Some(Color::rgb(80, 140, 255)),
            selection_bg: None,
            is_keyboard_focused: false,
            style: Default::default(),
            hovered: None,
        }
    }

    /// Paint `bar` (with `hovered` set on it) through the widget contract
    /// at `(x, y)` on a `canvas_w` × `canvas_h` bitmap, returning the
    /// bitmap and the painted rail's bar-relative row hits.
    fn paint_bar_at(
        bar: &ActivityBar,
        hovered: Option<usize>,
        x: f32,
        y: f32,
        canvas_w: u32,
        canvas_h: u32,
    ) -> (BitmapSurface, Vec<ActivityBarRowHit>) {
        let surface = BitmapSurface::new(canvas_w, canvas_h);
        surface.fill(0.0, 0.0, 0.0, 0.0);

        let mut bar = bar.clone();
        bar.hovered = hovered;
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        backend.begin_frame(Viewport::new(canvas_w as f32, canvas_h as f32, 1.0));
        let regions = backend.enter_frame_scope(surface.context_ptr(), |b| {
            b.draw_dyn(QRect::new(x, y, W as f32, H as f32), &bar, &())
                .downcast_ref::<ActivityBarPrimitiveLayout>()
                .expect("an ActivityBar lays out as ActivityBarPrimitiveLayout")
                .row_hits(&bar)
        });
        backend.end_frame();
        (surface, regions)
    }

    fn paint_via_backend(
        bar: &ActivityBar,
        hovered: Option<usize>,
    ) -> (BitmapSurface, Vec<ActivityBarRowHit>) {
        paint_bar_at(bar, hovered, 0.0, 0.0, W, H)
    }

    fn paint_via_backend_at(
        bar: &ActivityBar,
        hovered: Option<usize>,
        x: f32,
        y: f32,
        canvas_w: u32,
        canvas_h: u32,
    ) -> (BitmapSurface, Vec<ActivityBarRowHit>) {
        paint_bar_at(bar, hovered, x, y, canvas_w, canvas_h)
    }

    fn paint_via_backend_with_style(
        bar: &ActivityBar,
        style: &crate::ActivityBarStyle,
        hovered: Option<usize>,
    ) -> (BitmapSurface, Vec<ActivityBarRowHit>) {
        let mut bar = bar.clone();
        bar.style = *style;
        paint_bar_at(&bar, hovered, 0.0, 0.0, W, H)
    }

    /// Bounding-box height (device px) of every non-background pixel
    /// within row `[y0, y1)` — used by icon-size tests.
    /// Excludes the bar's own 1pt right-edge separator column
    /// (`x == W - 1`), which is unconditional and would otherwise swamp
    /// a glyph-only ink measurement.
    fn ink_height_in_row(surface: &BitmapSurface, y0: u32, y1: u32) -> u32 {
        let theme = Theme::default();
        let bg = (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b);
        let mut found: Option<(u32, u32)> = None;
        for y in y0..y1 {
            for x in 0..(W - 1) {
                let (r, g, b, _) = surface.pixel(x, y);
                if (r, g, b) != bg {
                    found = Some(match found {
                        Some((min_y, max_y)) => (min_y.min(y), max_y.max(y)),
                        None => (y, y),
                    });
                }
            }
        }
        found.map_or(0, |(min_y, max_y)| max_y - min_y + 1)
    }

    /// The icon glyph's ink size follows
    /// `ActivityBarStyle::icon_size_px`, not the backend's `chrome_font`
    /// size.
    #[test]
    fn icon_ink_height_follows_style_not_chrome_font_size() {
        use crate::primitives::activity_bar::DEFAULT_ACTIVITY_ICON_SIZE_PX;

        // Settings (bottom-pinned, not active): no accent strip to
        // isolate the glyph ink from.
        let mut bar = sample_bar();
        bar.active_accent = None;
        let settings_y0 = H - ACTIVITY_ROW_PX as u32;
        let settings_y1 = H;

        let paint_with = |style: &crate::ActivityBarStyle, chrome_size_pt: f64| -> u32 {
            let surface = BitmapSurface::new(W, H);
            surface.fill(0.0, 0.0, 0.0, 0.0);
            let mut backend = MacBackend::new();
            backend.set_chrome_font(system_ui_font(chrome_size_pt));
            backend.begin_frame(Viewport::new(W as f32, H as f32, 1.0));
            let mut bar = bar.clone();
            bar.style = *style;
            backend.enter_frame_scope(surface.context_ptr(), |b| {
                b.draw_dyn(QRect::new(0.0, 0.0, W as f32, H as f32), &bar, &());
            });
            backend.end_frame();
            ink_height_in_row(&surface, settings_y0, settings_y1)
        };

        let default_style = crate::ActivityBarStyle::default();
        let default_small_chrome = paint_with(&default_style, 6.0);
        let default_huge_chrome = paint_with(&default_style, 40.0);
        assert!(
            default_small_chrome > 0,
            "sanity: the glyph should paint some ink"
        );
        assert_eq!(
            default_small_chrome, default_huge_chrome,
            "default icon ink height must not track chrome_font's own size \
             (6pt vs 40pt chrome font should paint identically)"
        );

        let small_style = crate::ActivityBarStyle::new().with_icon_size_px(10.0);
        let small_icon = paint_with(&small_style, 11.0);
        assert!(
            small_icon < default_huge_chrome,
            "icon_size_px(10.0) ({small_icon}px ink) should paint smaller than \
             the {DEFAULT_ACTIVITY_ICON_SIZE_PX}px default ({default_huge_chrome}px ink)"
        );
    }

    #[test]
    fn background_is_tab_bar_bg() {
        let bar = sample_bar();
        let (surface, _) = paint_via_backend(&bar, None);
        let theme = Theme::default();
        // Probe deep inside the second (non-active) row, well away
        // from any glyph centre.
        let (r, g, b, _) = surface.pixel(W - 6, (ACTIVITY_ROW_PX as u32) + 4);
        assert_eq!(
            (r, g, b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
        );
    }

    #[test]
    fn active_item_paints_accent_strip_on_left_edge() {
        // 2-px accent strip at x ∈ [0, 2). Probe column 0 inside the
        // first row (active) and assert accent colour.
        let bar = sample_bar();
        let (surface, _) = paint_via_backend(&bar, None);
        let accent = bar.active_accent.unwrap();
        // y = ACTIVITY_ROW_PX/2 sits mid-row inside the first
        // (active) item.
        let probe_y = (ACTIVITY_ROW_PX as u32) / 2;
        let (r, g, b, _) = surface.pixel(0, probe_y);
        assert_eq!((r, g, b), (accent.r, accent.g, accent.b));
        let (r2, g2, b2, _) = surface.pixel(1, probe_y);
        assert_eq!((r2, g2, b2), (accent.r, accent.g, accent.b));

        // x = 2 should already be off the accent strip — proves the
        // strip is exactly 2 points wide. Probe at row top to dodge
        // any wide-glyph render.
        let (r3, g3, b3, _) = surface.pixel(2, 2);
        let theme = Theme::default();
        assert_eq!(
            (r3, g3, b3),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
            "x=2 should be tab_bar_bg, not accent",
        );
    }

    /// `style.active_bg: Some(..)` + `active_accent: None`
    /// paints a filled active row with **zero** accent-line pixels — the
    /// legacy 2-pt left-edge strip (x ∈ [0, 2)) must show the fill colour,
    /// not any accent tint.
    #[test]
    fn active_bg_fills_row_with_zero_accent_pixels_when_accent_is_none() {
        let mut bar = sample_bar();
        bar.active_accent = None;
        let fill = Color::rgb(49, 50, 51);
        let style = crate::ActivityBarStyle::new().with_active_bg(fill);
        let (surface, _) = paint_via_backend_with_style(&bar, &style, None);

        let probe_y = (ACTIVITY_ROW_PX as u32) / 2;
        for x in [0, 1] {
            let (r, g, b, _) = surface.pixel(x, probe_y);
            assert_eq!(
                (r, g, b),
                (fill.r, fill.g, fill.b),
                "x={x} is inside the legacy accent strip; with active_accent \
                 None it must show the active_bg fill, not any accent tint"
            );
        }
        // Deep in the row, away from the glyph, should also be filled.
        let (r, g, b, _) = surface.pixel(W - 6, probe_y);
        assert_eq!((r, g, b), (fill.r, fill.g, fill.b));
    }

    #[test]
    fn row_hits_cover_painted_rows() {
        // Round-trip: returned regions must point at where the icon
        // was painted. Verify each region's y-span matches the
        // ACTIVITY_ROW_PX grid AND its id matches the item.
        //
        // `visible_items` order is now bottom-pinned-first (the
        // shared `ActivityBar::layout`'s order — matching
        // `gtk::activity_bar` / `win::activity_bar`), not this module's
        // old top-first `row_plan` order. Only the *order* of these
        // assertions changed; the y-spans themselves are unchanged.
        let bar = sample_bar();
        let (_surface, regions) = paint_via_backend(&bar, None);
        // 2 top + 1 bottom = 3 visible rows.
        assert_eq!(regions.len(), 3);

        // Bottom-pinned item comes first.
        assert_eq!(regions[0].y_start, H as f32 - ACTIVITY_ROW_PX as f32);
        assert_eq!(regions[0].y_end, H as f32);
        assert_eq!(regions[0].id, WidgetId::new("activity:settings"));

        assert_eq!(regions[1].y_start, 0.0);
        assert_eq!(regions[1].y_end, ACTIVITY_ROW_PX as f32);
        assert_eq!(regions[1].id, WidgetId::new("activity:explorer"));

        assert_eq!(regions[2].y_start, ACTIVITY_ROW_PX as f32);
        assert_eq!(regions[2].y_end, 2.0 * ACTIVITY_ROW_PX as f32);
        assert_eq!(regions[2].id, WidgetId::new("activity:search"));
    }

    #[test]
    fn right_edge_has_separator_pixel() {
        let bar = sample_bar();
        let (surface, _) = paint_via_backend(&bar, None);
        let theme = Theme::default();
        let (r, g, b, _) = surface.pixel(W - 1, H / 2);
        assert_eq!(
            (r, g, b),
            (theme.separator.r, theme.separator.g, theme.separator.b),
        );
    }

    /// The shared `native_surface_paint::paint` fills `bar.selection_bg`
    /// (or the `lighten(0.20)` default) for a keyboard-selected row.
    #[test]
    fn keyboard_selected_row_paints_selection_bg() {
        let mut bar = sample_bar();
        // Search (top_items[1], not active) is the keyboard-selected row.
        bar.top_items[1].is_keyboard_selected = true;
        let (surface, _) = paint_via_backend(&bar, None);
        let theme = Theme::default();
        let expected = theme.tab_bar_bg.lighten(0.20);

        // Search paints at y ∈ [48, 96) — probe deep in the row, away
        // from the glyph and the (unrelated) accent strip.
        let probe_y = ACTIVITY_ROW_PX as u32 + ACTIVITY_ROW_PX as u32 / 2;
        let (r, g, b, _) = surface.pixel(W - 6, probe_y);
        assert_eq!(
            (r, g, b),
            (expected.r, expected.g, expected.b),
            "keyboard-selected row should paint the selection background",
        );
    }

    /// `cargo test -p quadraui --features macos -- --ignored --nocapture primitives::activity_bar::mac_paint_tests::dump_smoke_ppm`
    ///
    /// Paints the sample bar (2 top items + 1 bottom-pinned) into a
    /// 48 × 240 surface and writes `/tmp/quadraui_activity_bar.ppm`.
    /// Hover row 1 so the lightened bg is visible. Open in Preview to
    /// confirm:
    /// - Top item (active) has a 2-pt accent strip on the left edge.
    /// - Middle item shows the hover-lightened bg.
    /// - Bottom item is pinned to the bottom edge.
    /// - Right-edge has a 1-pt separator column.
    #[test]
    #[ignore = "writes /tmp/quadraui_activity_bar.ppm — opt in with --ignored"]
    fn dump_smoke_ppm() {
        let bar = sample_bar();
        let (surface, _) = paint_via_backend(&bar, Some(1));
        surface.write_ppm_and_open("/tmp/quadraui_activity_bar.ppm");
    }

    #[test]
    fn hover_lightens_row_background() {
        let bar = sample_bar();
        // Hover `visible_items[1]` — the explorer row (`ActivityBar::layout`
        // orders bottom-pinned items first, so index 1 is the first top item).
        // Hover paints unconditionally regardless of `is_active`, and the
        // default style has no `active_bg`, so the probed pixel is still the
        // plain hover tint either way.
        let (surface, _) = paint_via_backend(&bar, Some(1));
        let theme = Theme::default();
        let expected = theme.tab_bar_bg.lighten(0.10);
        // Explorer paints at y ∈ [0, 48) — probe deep into that row,
        // away from the glyph centre.
        let (r, g, b, _) = surface.pixel(W - 6, 4);
        assert_eq!(
            (r, g, b),
            (expected.r, expected.g, expected.b),
            "hovered row should paint the lightened bg",
        );
    }

    /// [`crate::DrawsPrimitives::layout_primitive`] must return exactly
    /// the rows the widget painted — both resolve the same
    /// `ActivityBar::layout`, and this pins that.
    #[test]
    fn layout_twin_matches_the_painted_rows() {
        let bar = sample_bar();
        let (_surface, painted) = paint_via_backend(&bar, None);

        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        let computed = backend
            .layout_primitive(QRect::new(0.0, 0.0, W as f32, H as f32), &bar)
            .row_hits(&bar);

        assert_eq!(painted.len(), computed.len());
        for (p, c) in painted.iter().zip(computed.iter()) {
            assert_eq!(p.id, c.id);
            assert_eq!(p.tooltip, c.tooltip);
            assert!((p.y_start - c.y_start).abs() < 0.001);
            assert!((p.y_end - c.y_end).abs() < 0.001);
        }
    }

    /// Spans are bar-relative: every row's `y_start` is unchanged
    /// regardless of where the bar sits.
    #[test]
    fn layout_twin_spans_are_bar_relative_at_a_nonzero_origin() {
        let bar = sample_bar();
        let mut backend = MacBackend::new();
        backend.set_current_font(font());
        let at_origin = backend
            .layout_primitive(QRect::new(0.0, 0.0, W as f32, H as f32), &bar)
            .row_hits(&bar);
        let moved = backend
            .layout_primitive(QRect::new(23.0, 41.0, W as f32, H as f32), &bar)
            .row_hits(&bar);

        assert_eq!(at_origin.len(), moved.len());
        // `at_origin[0]` is the bottom-pinned `settings` item (`ActivityBar::layout`
        // orders bottom-pinned items first), so its bar-relative `y_start` is
        // `H - ACTIVITY_ROW_PX`, not `0.0`.
        assert!((at_origin[0].y_start - (H as f32 - ACTIVITY_ROW_PX as f32)).abs() < 0.001);
        for (a, m) in at_origin.iter().zip(moved.iter()) {
            assert!(
                (a.y_start - m.y_start).abs() < 0.001,
                "row {:?} moved with the bar origin: {} vs {}",
                a.id,
                a.y_start,
                m.y_start,
            );
        }
    }

    /// Painting at a non-zero origin must leave `(0, 0)` untouched and
    /// land the strip's background + right-edge separator exactly at
    /// `(rect.x, rect.y)`.
    #[test]
    fn background_paints_at_rect_origin_not_at_window_origin() {
        const ORIGIN_X: f32 = 30.0;
        const ORIGIN_Y: f32 = 28.0; // e.g. a title bar's height
        let canvas_w = ORIGIN_X as u32 + W;
        let canvas_h = ORIGIN_Y as u32 + H;

        let bar = sample_bar();
        let (surface, regions) =
            paint_via_backend_at(&bar, None, ORIGIN_X, ORIGIN_Y, canvas_w, canvas_h);
        let theme = Theme::default();

        // The window's literal top-left corner must stay exactly as the
        // surface was initialised — fully transparent — never the
        // activity bar's background fill.
        let (_, _, _, a) = surface.pixel(0, 0);
        assert_eq!(
            a, 0,
            "window origin (0,0) must stay untouched when the bar sits at ({ORIGIN_X}, {ORIGIN_Y})",
        );

        // Deep inside the (non-active) second row, offset by the bar's
        // real origin: activity-bar background.
        let probe_x = ORIGIN_X as u32 + W - 6;
        let probe_y = ORIGIN_Y as u32 + ACTIVITY_ROW_PX as u32 + 4;
        let (r, g, b, _) = surface.pixel(probe_x, probe_y);
        assert_eq!(
            (r, g, b),
            (theme.tab_bar_bg.r, theme.tab_bar_bg.g, theme.tab_bar_bg.b),
            "activity bar background should paint at rect's origin, not the window origin",
        );

        // Right-edge separator (1 pt) shifts with the origin too.
        let (sr, sg, sb, _) = surface.pixel(ORIGIN_X as u32 + W - 1, ORIGIN_Y as u32 + H / 2);
        assert_eq!(
            (sr, sg, sb),
            (theme.separator.r, theme.separator.g, theme.separator.b),
            "right-edge separator should shift with the bar's origin",
        );

        // Hit regions stay bar-relative — the paint origin must not leak
        // into the returned spans. `regions[0]` is the bottom-pinned `settings`
        // item (`ActivityBar::layout` orders bottom-pinned items first), so its
        // bar-relative `y_start` is `H - ACTIVITY_ROW_PX`, not `0.0`.
        assert_eq!(regions[0].y_start, H as f32 - ACTIVITY_ROW_PX as f32);
    }
}
