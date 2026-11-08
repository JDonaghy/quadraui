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
use crate::primitives::toolbar::{
    Toolbar, ToolbarButton, ToolbarItemKind, ToolbarItemMeasure, ToolbarLayoutOptions,
    ToolbarOrientation,
};
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
/// };
///
/// let layout = bar.layout(2.0, 10.0, 1.0);
/// assert_eq!(
///     layout.hit_test(0.0, 0.0),
///     ActivityBarHit::Item(WidgetId::new("activity:explorer"))
/// );
/// ```
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
    ToolbarButton::Action {
        id: item.id,
        label: String::new(),
        icon: Some(item.icon.fallback.clone()),
        key_hint: None,
        enabled: true,
        is_active: item.is_active,
        tooltip: item.tooltip.clone(),
        spec: None,
    }
}

/// Per-frame style overrides for
/// [`crate::Backend::draw_activity_bar_with_style`] — currently just
/// the active-item row-fill colour, VS Code style (no line, a soft chip on
/// the row itself).
///
/// This is a **sidecar** passed alongside an [`ActivityBar`] rather than a
/// field on it. `ActivityBar` is a plain, non-`#[non_exhaustive]`,
/// all-`pub`-field struct constructed via *exhaustive* literals both
/// in-tree and downstream, so adding a required field to it would break
/// every exhaustive literal call site. Threading the row-fill colour
/// through this separate value sidesteps the break entirely — mirrors
/// [`crate::TooltipChrome`] and [`crate::TabChrome`], which solve the
/// identical problem for `Tooltip` and `TabBar`.
///
/// [`crate::Backend::draw_activity_bar`] keeps its exact signature and
/// behaviour (delegates to the new method with
/// `ActivityBarStyle::default()`, i.e. no fill), so existing `Backend`
/// implementors and callers are untouched.
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
    /// Before this field existed, macOS painted the icon glyph at
    /// whatever font the backend's `draw_activity_bar` passed in — first
    /// the *editor* font, then the *chrome* font — so icon
    /// size tracked a font size never meant to govern it; GTK independently
    /// hardcoded a fixed 18pt (≈24px @ 96dpi) size for the same reason.
    /// This field generalises GTK's already-correct fixed size into a
    /// configurable, cross-backend knob every pixel backend now honours,
    /// rather than leaving a third independently-tuned constant on
    /// Win-GUI.
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
/// passed to [`Backend::draw_activity_bar`]**, i.e. the first row always
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
/// values. Each backend's `activity_bar` module doc spells out its
/// conversion. Callers add the bar origin before hit-testing.
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

// ── PaintSurface Phase 4 slice 5/8 ─────────────────────────────────────────
//
// `paint` below is the one shared paint implementation, written against
// [`crate::paint_surface::PaintSurface`] instead of any one backend's
// API — see `crate::primitives::toolbar::native_surface_paint` for the
// same pattern applied one primitive earlier in this issue.
//
// Pre-migration, `gtk::activity_bar`, `macos::activity_bar` and
// `win::activity_bar` agreed on background, right-edge separator (except
// where noted), active-row fill / accent-line, hover tint, and
// per-state foreground colouring — but diverged in ways deeper than a
// cosmetic corner radius:
//
// - **Row ordering / the layout twin.** GTK and Windows both derive
//   `visible_items` from the shared
//   [`ActivityBar::layout`] (bottom-pinned items first, then top —
//   see that method's doc), and their own no-paint layout functions
//   (`gtk_toolbar_layout`'s twin `bar.layout(...)` call, `win_activity_bar_layout`)
//   call the exact same method, so paint and hit-test can't drift
//   apart. **macOS instead had a private `row_plan` helper** that
//   walked top-then-bottom — geometrically identical `y` positions, but
//   a different `visible_items` order, and thus a different meaning for
//   the flat index `hovered_idx` is compared against (macOS's own
//   module doc called this out explicitly as a "known divergence,
//   deliberately left alone"). Unifying onto one `paint` forces one
//   `visible_items` order for all three; adopting the GTK/Windows order
//   is what finally resolves that flagged-but-parked divergence rather
//   than parking it again. `macos::activity_bar::mac_activity_bar_layout`
//   now calls [`ActivityBar::layout`] directly too, so paint and layout
//   stay in lock-step exactly like the other two backends.
// - **Keyboard-selection highlight.** [`ActivityItem::is_keyboard_selected`]'s
//   own doc says "Both the TUI and GTK rasterizers honour this flag" —
//   an admission, not a design choice, that macOS never did: pre-migration
//   `macos::activity_bar::draw_row` had no `is_keyboard_selected` branch
//   at all, so keyboard-arrow navigation was invisible on macOS, and its
//   foreground-colour decision didn't brighten for a selected-but-not-
//   active/hovered row either (`item.is_active || is_hovered` — GTK/Win's
//   equivalent OR's in `is_keyboard_selected` too). `paint` adds both,
//   closing the gap rather than leaving a third of the three backends
//   without the feature at all.
// - **Right-edge separator.** GTK and macOS both painted a 1px
//   `theme.separator` column at the bar's right edge; **Windows did
//   not** — no doc comment explains why, so this reads as an
//   oversight rather than a deliberate omission. Adopted for all three.
//
// Every other pixel (background fill, active-row fill/accent-line
// priority order, hover tint, icon glyph selection via
// `nerd_fonts_enabled`) was already identical in intent across the
// three, differing only in which native API painted it.
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
    use crate::event::Rect;
    use crate::paint_surface::PaintSurface;
    use crate::theme::Theme;

    /// Paint an [`ActivityBar`] at its caller-resolved `layout` onto
    /// `surface`. Returns per-row hit spans, **bar-relative** (see
    /// [`ActivityBarRowHit`]'s own doc) — callers add the bar's origin
    /// before hit-testing a click.
    ///
    /// `hovered_idx` indexes into `layout.visible_items` (bottom-pinned
    /// items first, then top — see [`ActivityBar::layout`]'s doc).
    /// `nerd_fonts_enabled` selects `item.icon.glyph` vs
    /// `item.icon.fallback`.
    pub(crate) fn paint(
        bar: &ActivityBar,
        layout: &ActivityBarLayout,
        style: &ActivityBarStyle,
        surface: &mut dyn PaintSurface,
        theme: &Theme,
        hovered_idx: Option<usize>,
        nerd_fonts_enabled: bool,
    ) -> Vec<ActivityBarRowHit> {
        let width = layout.viewport_width;
        let height = layout.viewport_height;

        surface.surface_fill_rect(Rect::new(0.0, 0.0, width, height), theme.tab_bar_bg);
        surface.surface_fill_rect(Rect::new(width - 1.0, 0.0, 1.0, height), theme.separator);

        let hover_bg = theme.tab_bar_bg.lighten(0.10);

        let mut regions: Vec<ActivityBarRowHit> = Vec::new();

        for (flat_idx, vi) in layout.visible_items.iter().enumerate() {
            let y = vi.bounds.y;
            let row_h = vi.bounds.height;
            let item = match vi.side {
                ActivitySide::Top => &bar.top_items[vi.item_idx],
                ActivitySide::Bottom => &bar.bottom_items[vi.item_idx],
            };
            let is_hovered = hovered_idx == Some(flat_idx);

            // Active-row fill (VS Code style) — lowest priority,
            // painted first so hover/selection tints below still win.
            if item.is_active {
                if let Some(bg) = style.active_bg {
                    surface.surface_fill_rect(Rect::new(0.0, y, width, row_h), bg);
                }
            }

            // Hover tint — painted before selection so the brighter
            // selection tint always wins when a row is both hovered and
            // keyboard-selected.
            if is_hovered {
                surface.surface_fill_rect(Rect::new(0.0, y, width, row_h), hover_bg);
            }

            // Keyboard-selection highlight.
            if item.is_keyboard_selected {
                let sel_bg = bar
                    .selection_bg
                    .unwrap_or_else(|| theme.tab_bar_bg.lighten(0.20));
                surface.surface_fill_rect(Rect::new(0.0, y, width, row_h), sel_bg);
            }

            // Left-edge accent line — only when the bar opts in via
            // `active_accent`; `None` paints zero accent pixels.
            if item.is_active {
                if let Some(accent) = bar.active_accent {
                    surface.surface_fill_rect(Rect::new(0.0, y, 2.0, row_h), accent);
                }
            }

            let icon_str = if nerd_fonts_enabled {
                item.icon.glyph.as_str()
            } else {
                item.icon.fallback.as_str()
            };
            let (iw, ih) = surface.surface_measure_text(icon_str);
            let fg = if item.is_active || is_hovered || item.is_keyboard_selected {
                theme.foreground
            } else {
                theme.inactive_fg
            };
            surface.surface_draw_text_run(
                Rect::new(
                    (width - iw) / 2.0,
                    y + (row_h - ih) / 2.0,
                    iw.max(0.0),
                    ih.max(0.0),
                ),
                icon_str,
                fg,
            );

            regions.push(ActivityBarRowHit {
                y_start: y,
                y_end: y + row_h,
                id: item.id,
                tooltip: item.tooltip.clone(),
            });
        }

        regions
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
        };
        let toolbar: Toolbar = (&bar).into();
        assert_eq!(toolbar.focused_index, None);
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
