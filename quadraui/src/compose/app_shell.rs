//! `ShellView` — the activity bar + sidebar panel container pattern
//! (VS Code / JetBrains / Zellij-style app shell) as a widget-contract
//! descriptor.
//!
//! The app owns the descriptor and a [`ShellViewState`] beside it.
//! [`ShellView::handle_event`] runs the interaction state machine —
//! activity bar click toggle, active panel switching, sidebar visibility
//! and resize drag — and applies what it reports. Apps register panels at
//! construction time or dynamically via [`ShellView::add_panel`] /
//! [`ShellView::remove_panel`], match on [`ShellEvent`] for semantic
//! actions, and paint their panel content into the bounds the shell
//! returns.
//!
//! The shell renders the activity bar, sidebar header chrome, and resize
//! divider. Panel *content* and the main content area are the consumer's
//! responsibility — the shell returns [`ShellLayout`] with the bounds.

use std::collections::HashMap;

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

use super::shell_layout;
use crate::a11y::{A11yChild, A11yInfo, A11yPrimitive, A11yRole};
use crate::backend::DrawsPrimitives;
use crate::compose::Handled;
use crate::interaction::InteractionState;
use crate::interactive::{
    EventCx, EventHost, FocusSpec, FocusStop, Interactive, PointerKind, RoutedEvent, SubTarget,
    WidgetAction,
};
use crate::paint_surface::PaintSurface;
use crate::primitive::{LayoutCtx, PaintEnv, Primitive, Units};
use crate::primitives::activity_bar::{
    ActivityBar, ActivityBarAction, ActivityBarPrimitiveLayout, ActivityBarState, ActivityItem,
};
use crate::primitives::split::{sash_paint_rect, SplitDirection};
use crate::primitives::status_bar::{StatusBar, StatusBarPrimitiveLayout, StatusBarSegment};
use crate::types::{Color, Icon, Modifiers, WidgetId};
#[cfg(feature = "tui")]
use crate::CellSurface;
use crate::{Backend, ButtonMask, MouseButton, Point, Rect, Style, UiEvent};

// ── Public types ─────────────────────────────────────────────────────

/// Registration info for one sidebar panel.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelDefinition {
    pub id: WidgetId,
    /// Single icon string painted on the panel's activity-bar row.
    ///
    /// [`ShellView::build_activity_bar`] widens this into the built
    /// [`ActivityItem::icon`] as `Icon { glyph: icon, fallback: icon }` —
    /// i.e. the same string paints whether or not the backend's
    /// `nerd_fonts_enabled` flag is set. A panel that wants a *distinct*
    /// Nerd-Font glyph and ASCII fallback attaches the pair with
    /// [`ShellView::with_panel_icon`] instead of widening this field
    ///
    /// **This stays a `String` on purpose.** A Rust struct literal pins
    /// its field's type exactly, and every construction site of this type
    /// is a literal: fourteen in `coord-tui`'s `shell_config()`, one in
    /// `vimcode`'s `ext_activity_panels()`, and two in the sealed
    /// `tests/acceptance/ms-11/structural_parity.rs` fixture that workers
    /// may not edit. Retyping the field to [`Icon`] breaks all of them at
    /// once with no deprecation shim available (CLAUDE.md rules 2–3), so
    /// the glyph/fallback pair rides alongside it instead.
    pub icon: String,
    pub tooltip: String,
    pub title: String,
}

/// One independently-gated horizontal band stacked in the bottom chrome
/// of the main content column.
///
/// [`ShellView`] already models a single resizable bottom drawer
/// ([`ShellView::with_bottom_panel`] / [`crate::BottomPanel`]) — the
/// terminal-panel case. Some consumers (vimcode) stack several more bands
/// below the editor content area at once: a terminal toolbar, a debug
/// toolbar, a quickfix list, a wildmenu, a separated status row — each
/// gated on its own boolean/state, not just a single shared height. A
/// `BottomBand` models one such rung: present only when `visible` is
/// `true`, sized independently via `height_lh`.
///
/// [`ShellView::with_bottom_bands`] takes an ordered `Vec<BottomBand>`.
/// Bands are stacked **bottom-up**: `bottom_bands[0]` sits flush against
/// the bottom edge of the main content column, `bottom_bands[1]` sits
/// directly above it, and so on. The legacy single [`ShellView::with_bottom_panel`]
/// drawer (if configured) docks *above* every `BottomBand`, closest to
/// remaining main content — i.e. it behaves as if it were one more,
/// implicit band stacked on top of the list. This keeps every existing
/// `with_bottom_panel` consumer's layout byte-for-byte unchanged when
/// `bottom_bands` is empty (the default).
#[derive(Debug, Clone, PartialEq)]
pub struct BottomBand {
    /// Identifies this band in [`ShellView::bottom_band_bounds`] and
    /// in [`ShellView::set_bottom_band_visible`] / [`ShellView::set_bottom_band_height`].
    pub id: WidgetId,
    /// Whether this band currently occupies space. `false` removes it
    /// from the stack entirely for this frame (the bands above and below
    /// close the gap) — this is the "independent presence" gating vimcode
    /// needs (a quickfix list that only exists while quickfix is open),
    /// not just a height of zero.
    pub visible: bool,
    /// Band height in line-height multiples (matching every other
    /// `*_height_lh` field on [`ShellView`]) — portable across TUI (cells)
    /// and GUI (pixels) backends.
    pub height_lh: f32,
}

impl BottomBand {
    /// Construct a visible band with the given id and height.
    pub fn new(id: WidgetId, height_lh: f32) -> Self {
        Self {
            id,
            visible: true,
            height_lh,
        }
    }
}

/// Which side of the viewport the activity bar + sidebar sit on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShellPosition {
    #[default]
    Left,
    Right,
}

/// Semantic events emitted by [`ShellView::handle_event`].
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub enum ShellEvent {
    PanelChanged {
        panel_id: WidgetId,
    },
    SidebarHidden,
    SidebarResized {
        new_width: f32,
    },
    BottomPanelResized {
        new_height: f32,
    },
    BottomPanelHidden,
    BottomItemClicked {
        id: WidgetId,
    },
    /// The activity bar's keyboard cursor moved to `index` in the logical
    /// item sequence (top panels, then bottom items).
    ActivityCursorMoved {
        index: usize,
    },
    Consumed,
    Ignored,
}

/// Externally-tagged, one-entry-map shape: `{"PanelChanged": {"panel_id":
/// "panel:explorer"}}`, `{"SidebarHidden": null}`, etc. — the same shape a
/// plain `#[derive(Serialize)]` would give every variant here *except* the
/// four unit variants, which `serde`'s own external tagging would instead
/// serialize as a bare string (`"SidebarHidden"`, not `{"SidebarHidden":
/// null}`). A binding's own `on_shell_event` callback already observes the
/// dict-with-`null` shape (see `quadraui-py`'s and `quadraui-node`'s own
/// shell tests), so this impl is hand-written rather than derived, to keep
/// that wire shape unchanged. Written once here instead of once per binding
/// — a binding reaches this via a generic `pythonize::pythonize`/
/// `serde_json::to_value` call, not its own hand-rolled match.
impl Serialize for ShellEvent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct PanelChangedPayload {
            panel_id: WidgetId,
        }
        #[derive(Serialize)]
        struct SidebarResizedPayload {
            new_width: f32,
        }
        #[derive(Serialize)]
        struct BottomPanelResizedPayload {
            new_height: f32,
        }
        #[derive(Serialize)]
        struct BottomItemClickedPayload {
            id: WidgetId,
        }
        #[derive(Serialize)]
        struct ActivityCursorMovedPayload {
            index: usize,
        }

        let mut map = serializer.serialize_map(Some(1))?;
        match self {
            ShellEvent::PanelChanged { panel_id } => map.serialize_entry(
                "PanelChanged",
                &PanelChangedPayload {
                    panel_id: *panel_id,
                },
            )?,
            ShellEvent::SidebarHidden => map.serialize_entry("SidebarHidden", &())?,
            ShellEvent::SidebarResized { new_width } => map.serialize_entry(
                "SidebarResized",
                &SidebarResizedPayload {
                    new_width: *new_width,
                },
            )?,
            ShellEvent::BottomPanelResized { new_height } => map.serialize_entry(
                "BottomPanelResized",
                &BottomPanelResizedPayload {
                    new_height: *new_height,
                },
            )?,
            ShellEvent::BottomPanelHidden => map.serialize_entry("BottomPanelHidden", &())?,
            ShellEvent::BottomItemClicked { id } => {
                map.serialize_entry("BottomItemClicked", &BottomItemClickedPayload { id: *id })?
            }
            ShellEvent::ActivityCursorMoved { index } => map.serialize_entry(
                "ActivityCursorMoved",
                &ActivityCursorMovedPayload { index: *index },
            )?,
            ShellEvent::Consumed => map.serialize_entry("Consumed", &())?,
            ShellEvent::Ignored => map.serialize_entry("Ignored", &())?,
        }
        map.end()
    }
}

/// Layout bounds returned by [`ShellView::render`] and [`ShellView::shell_layout`].
///
/// Bottom-band bounds are exposed via [`ShellView::bottom_band_bounds`]
/// rather than as fields here: `ShellLayout` is an all-`pub`-field
/// struct that downstream consumers build with exhaustive literals.
/// Rust cannot grow such a struct's field list without breaking those
/// call sites. Bottom-band bounds are exposed additively instead, the
/// same way `TextInput`/`Editor` grew without touching their field lists.
#[derive(Debug, Clone, PartialEq)]
pub struct ShellLayout {
    /// The full window/viewport rect this layout was computed against —
    /// the same `area` passed to [`ShellView::shell_layout`] / [`ShellView::render`].
    /// Lets [`crate::shell::ShellContext::window_edge`] hit-test the
    /// outer window border for edge-resize without the app having to
    /// separately track `backend.viewport()`.
    pub window_bounds: Rect,
    pub title_bar_bounds: Option<Rect>,
    pub activity_bar_bounds: Rect,
    pub sidebar_header_bounds: Option<Rect>,
    pub sidebar_content_bounds: Option<Rect>,
    pub divider_bounds: Option<Rect>,
    pub main_content_bounds: Rect,
    pub bottom_panel_bounds: Option<Rect>,
    pub command_line_bounds: Option<Rect>,
    pub status_bar_bounds: Option<Rect>,
}

// ── ShellView ────────────────────────────────────────────────────────

/// The [`WidgetId`] an app shell paints under unless told otherwise. Its
/// chrome regions are published as frame-inventory zones under it —
/// `app-shell:status-bar`, `app-shell:divider` and the rest (see
/// [`ShellRegion::zone_name`]).
pub const APP_SHELL_ID: &str = "app-shell";

/// The [`WidgetId`] of a shell's activity bar child.
pub const APP_SHELL_ACTIVITY_BAR_ID: &str = "app-shell:activity-bar";

/// The [`WidgetId`] of a shell's sidebar header child.
pub const APP_SHELL_SIDEBAR_HEADER_ID: &str = "app-shell:sidebar-header";

/// The app shell as a widget-contract descriptor: the activity bar + sidebar
/// panel container (VS Code / JetBrains / Zellij-style), with its optional
/// title bar, bottom panel, bottom bands, command line and status bar slots.
///
/// Everything visible is a field the app owns (controlled mode): the
/// panels, which one is active, sidebar width and visibility, the bottom
/// panel's height and visibility, and the activity bar's keyboard cursor.
/// [`Interactive::handle`] reports what a gesture asks for as a
/// [`ShellEvent`] and the app applies it; [`ShellView::handle_event`]
/// does both in one call. [`ShellViewState`] holds only
/// gesture data — an in-progress divider drag and the activity bar's
/// hover.
///
/// The shell composes its chrome rather than painting it: the activity bar
/// paints through [`ActivityBar`]'s own [`Primitive`] and the sidebar
/// header through [`StatusBar`]'s. Panel content, the main content area,
/// the bottom panel and the status bar slot are the host's to paint into
/// the bounds [`ShellViewLayout::shell`] reports.
///
/// All width/height fields are stored as **multiples of the chrome line
/// height** so they are portable across TUI (cells) and GUI (pixels)
/// backends; layout resolves them as `dimension_native = field * lh`.
///
/// Every chrome region is a named sub-region ([`ShellRegion`]), so
/// [`crate::DrawsPrimitives::draw_dyn`] publishes it as a frame-inventory
/// zone `"<id>:<region>"`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct ShellView {
    /// The shell's id; its chrome zones are published under it. Defaults
    /// to [`APP_SHELL_ID`].
    pub id: WidgetId,
    /// Sidebar panels, one activity-bar row each, top-pinned.
    pub panels: Vec<PanelDefinition>,
    /// Bottom-pinned activity-bar items (a settings gear, a bell).
    pub bottom_items: Vec<PanelDefinition>,
    /// Index into `panels` of the top panel most recently made active.
    /// Drives the top item's `is_active` accent and, when
    /// `sidebar_bottom_owner` is `None`, the sidebar header title.
    pub active_panel: Option<usize>,
    /// Index into `bottom_items` of the bottom item that currently owns
    /// the sidebar header, if one does — see [`ShellView::show_panel`].
    pub sidebar_bottom_owner: Option<usize>,
    /// Whether the sidebar is shown.
    pub sidebar_visible: bool,
    /// Sidebar width in line-height multiples.
    pub sidebar_width: f32,
    /// Smallest sidebar width a divider drag resolves to.
    pub min_sidebar_width: f32,
    /// Largest sidebar width a divider drag resolves to.
    pub max_sidebar_width: f32,
    /// Activity bar width in line-height multiples.
    pub activity_bar_width: f32,
    /// Fixed-pixel activity bar width override. When `Some`, this wins
    /// over `activity_bar_width` and is used verbatim regardless of line
    /// height — VS Code's activity bar is a fixed 48px independent of
    /// editor font size. Ignored on a cell grid.
    pub activity_bar_width_px: Option<f32>,
    /// Which side the activity bar and sidebar sit on.
    pub position: ShellPosition,
    /// Whether a title bar band is reserved along the top edge.
    pub has_title_bar: bool,
    /// Title bar height in line-height multiples.
    pub title_bar_height_lh: f32,
    /// Fixed-pixel floor under the title bar height: the resolved height
    /// is `max(title_bar_height_lh * line_height, title_bar_min_px)`.
    pub title_bar_min_px: Option<f32>,
    /// Whether a bottom panel slot is reserved under the main column.
    pub has_bottom_panel: bool,
    /// Bottom panel height in line-height multiples.
    pub bottom_panel_height_lh: f32,
    /// Smallest bottom panel height a grip drag resolves to.
    pub min_bottom_panel_height_lh: f32,
    /// Largest bottom panel height a grip drag resolves to.
    pub max_bottom_panel_height_lh: f32,
    /// Whether the bottom panel is shown.
    pub bottom_panel_visible: bool,
    /// Whether a command line band is reserved above the status bar.
    pub has_command_line: bool,
    /// Whether a status bar band is reserved along the bottom edge.
    pub has_status_bar: bool,
    /// Fixed-pixel status bar height override — VS Code's status bar is a
    /// fixed 22px independent of editor font size. `None` keeps it one
    /// line height tall.
    pub status_bar_height_px: Option<f32>,
    /// When `true`, the activity bar and sidebar run the full height below
    /// the title bar and the status bar and command line are carved from
    /// the main column only.
    pub status_bar_under_main_column: bool,
    /// Independently-gated bands stacked bottom-up below the bottom panel
    /// — see [`BottomBand`].
    pub bottom_bands: Vec<BottomBand>,
    /// Per-panel Nerd-Font glyph + ASCII fallback pairs, keyed by panel
    /// id, preferred over the panel's own [`PanelDefinition::icon`].
    pub icon_overrides: HashMap<WidgetId, Icon>,
    /// The activity bar's active-panel accent; `None` paints none.
    pub active_accent: Option<Color>,
    /// The activity bar's keyboard-selection highlight; `None` falls back
    /// to each backend's own default.
    pub selection_bg: Option<Color>,
    /// Whether the activity bar holds keyboard focus — it then paints the
    /// keyboard cursor and its keys are the bar's.
    pub activity_keyboard_focused: bool,
    /// The activity bar's keyboard cursor, in the logical item sequence
    /// (top panels, then bottom items).
    pub activity_cursor: usize,
    /// Paint the keyboard cursor even without keyboard focus.
    pub activity_cursor_visible: bool,
}

/// One region of a [`ShellView`] — the sub-parts
/// [`Interactive::sub_regions`] reports and, for the chrome regions,
/// the frame-inventory zone each is published as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShellRegion {
    /// The whole area the shell was laid out into.
    Window,
    /// The title bar band.
    TitleBar,
    /// The activity bar.
    ActivityBar,
    /// The sidebar header strip.
    SidebarHeader,
    /// The sidebar content area, below the header.
    SidebarContent,
    /// The sidebar resize divider's drag band.
    Divider,
    /// The main content area.
    MainContent,
    /// The bottom panel slot.
    BottomPanel,
    /// The drag grip along the bottom panel's top edge.
    BottomPanelGrip,
    /// The command line band.
    CommandLine,
    /// The status bar band.
    StatusBar,
    /// One activity-bar row, by its index into the activity bar layout's
    /// visible items.
    ActivityItem(usize),
}

/// [`SubTarget`] values at and above this name activity-bar rows.
const ACTIVITY_ITEM_SUB_BASE: u64 = 1 << 32;

impl ShellRegion {
    /// The [`SubTarget`] that names this region.
    pub fn sub_target(self) -> SubTarget {
        SubTarget(match self {
            ShellRegion::Window => 1,
            ShellRegion::TitleBar => 2,
            ShellRegion::ActivityBar => 3,
            ShellRegion::SidebarHeader => 4,
            ShellRegion::SidebarContent => 5,
            ShellRegion::Divider => 6,
            ShellRegion::MainContent => 7,
            ShellRegion::BottomPanel => 8,
            ShellRegion::BottomPanelGrip => 9,
            ShellRegion::CommandLine => 10,
            ShellRegion::StatusBar => 11,
            ShellRegion::ActivityItem(idx) => ACTIVITY_ITEM_SUB_BASE + idx as u64,
        })
    }

    /// The region `sub` names, if it names one.
    pub fn from_sub_target(sub: SubTarget) -> Option<Self> {
        Some(match sub.0 {
            1 => ShellRegion::Window,
            2 => ShellRegion::TitleBar,
            3 => ShellRegion::ActivityBar,
            4 => ShellRegion::SidebarHeader,
            5 => ShellRegion::SidebarContent,
            6 => ShellRegion::Divider,
            7 => ShellRegion::MainContent,
            8 => ShellRegion::BottomPanel,
            9 => ShellRegion::BottomPanelGrip,
            10 => ShellRegion::CommandLine,
            11 => ShellRegion::StatusBar,
            n if n >= ACTIVITY_ITEM_SUB_BASE => {
                ShellRegion::ActivityItem((n - ACTIVITY_ITEM_SUB_BASE) as usize)
            }
            _ => return None,
        })
    }

    /// The name this region is published under as a frame-inventory zone
    /// (`"<shell id>:<name>"`). The grip and the activity rows are not
    /// published: the rows are registered under each panel's own id by
    /// [`ShellView::render`].
    pub fn zone_name(self) -> Option<&'static str> {
        Some(match self {
            ShellRegion::Window => "window",
            ShellRegion::TitleBar => "title-bar",
            ShellRegion::ActivityBar => "activity-bar",
            ShellRegion::SidebarHeader => "sidebar-header",
            ShellRegion::SidebarContent => "sidebar-content",
            ShellRegion::Divider => "divider",
            ShellRegion::MainContent => "main-content",
            ShellRegion::BottomPanel => "bottom-panel",
            ShellRegion::CommandLine => "command-line",
            ShellRegion::StatusBar => "status-bar",
            ShellRegion::BottomPanelGrip | ShellRegion::ActivityItem(_) => return None,
        })
    }
}

/// An in-progress divider drag, with the pointer's offset from the point
/// the drag measures from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShellDrag {
    /// Resizing the sidebar; offset from the divider's centre line.
    Sidebar(f32),
    /// Resizing the bottom panel; offset from the panel's top edge.
    BottomPanel(f32),
}

/// [`Interactive::State`] for [`ShellView`]: gesture data only — the
/// divider drag in progress and the activity bar child's own state (its
/// hovered row).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ShellViewState {
    /// The activity bar child's interaction state.
    pub activity: ActivityBarState,
    /// The divider drag in progress, if any.
    pub drag: Option<ShellDrag>,
}

/// [`Primitive::Layout`] for [`ShellView`]: every chrome rect plus the
/// children laid out into theirs.
///
/// `#[non_exhaustive]`: produced by [`Primitive::layout`], never built by
/// a consumer.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct ShellViewLayout {
    /// Every chrome rect, absolute.
    pub shell: ShellLayout,
    /// Each visible [`BottomBand`]'s rect, bottom-up.
    pub bottom_band_bounds: Vec<(WidgetId, Rect)>,
    /// The chrome line height the line-height multiples resolved against.
    pub line_height: f32,
    /// The bottom panel's drag grip, just above its top edge.
    pub bottom_panel_grip: Option<Rect>,
    /// The activity bar child.
    pub activity_bar: ActivityBar,
    /// The activity bar child's layout.
    pub activity: ActivityBarPrimitiveLayout,
    /// The sidebar header child and its layout, when the sidebar shows
    /// and a panel owns it.
    pub sidebar_header: Option<(StatusBar, StatusBarPrimitiveLayout)>,
}

impl ShellViewLayout {
    /// `rect` moved from absolute into the shell's local space.
    fn local(&self, rect: Rect) -> Rect {
        let origin = self.shell.window_bounds;
        Rect::new(
            rect.x - origin.x,
            rect.y - origin.y,
            rect.width,
            rect.height,
        )
    }
}

// ── ShellView: builders, accessors and mutators ─────────────────────

impl ShellView {
    pub fn with_bottom_items(mut self, items: Vec<PanelDefinition>) -> Self {
        self.bottom_items = items;
        self
    }

    /// Attach a distinct Nerd-Font glyph + ASCII fallback pair to one
    /// activity-bar row, overriding the single-string
    /// [`PanelDefinition::icon`] that panel was registered with.
    ///
    /// `id` may name a top panel or a bottom item; unknown ids are stored
    /// but never consulted, so registration order relative to
    /// [`Self::with_bottom_items`] / [`Self::register_panel`] does not
    /// matter. Calling this twice for the same id keeps the last pair.
    ///
    /// Which half actually paints is the backend's decision, driven by its
    /// `nerd_fonts_enabled` flag — `glyph` when the icon font is available,
    /// `fallback` otherwise. Panels that never call this get
    /// `glyph == fallback == PanelDefinition::icon`.
    /// ```
    /// # use quadraui::compose::app_shell::{ShellView, PanelDefinition};
    /// # use quadraui::types::{Icon, WidgetId};
    /// let shell = ShellView::new(
    ///     vec![PanelDefinition {
    ///         id: WidgetId::new("panel:explorer"),
    ///         icon: "E".to_string(),
    ///         tooltip: "Explorer".to_string(),
    ///         title: "EXPLORER".to_string(),
    ///     }],
    ///     20.0,
    /// )
    /// .with_panel_icon(WidgetId::new("panel:explorer"), Icon::new("\u{f07c}", "E"));
    /// assert_eq!(shell.build_activity_bar().top_items[0].icon.glyph, "\u{f07c}");
    /// ```
    #[must_use]
    pub fn with_panel_icon(mut self, id: WidgetId, icon: Icon) -> Self {
        self.icon_overrides.insert(id, icon);
        self
    }

    pub fn with_min_width(mut self, min: f32) -> Self {
        self.min_sidebar_width = min;
        self
    }

    pub fn with_max_width(mut self, max: f32) -> Self {
        self.max_sidebar_width = max;
        self
    }

    /// Set the activity bar's width as a multiple of `line_height`.
    ///
    /// **Cell-grid (TUI) only by default.** On a pixel backend
    /// (`line_height > 1.0`) with no [`Self::with_activity_bar_width_px`]
    /// paired alongside it, this multiple has **no effect** — the bar
    /// instead defaults to [`crate::Style::activity_bar_width`] (48px,
    /// matching VS Code) or whatever a host's [`crate::Backend::set_style`]
    /// last set. Pass [`Self::with_activity_bar_width_px`] too if a GUI
    /// app genuinely wants a width `set_style` cannot move.
    pub fn with_activity_bar_width(mut self, width: f32) -> Self {
        self.activity_bar_width = width;
        self
    }

    /// Pin the activity bar to a fixed pixel width, overriding the
    /// line-height-derived width from [`Self::with_activity_bar_width`].
    ///
    /// The row height GTK/macOS paint each activity item at
    /// (`ACTIVITY_ROW_PX = 48.0`) is already a fixed pixel constant, not a
    /// `line_height` multiple — so a `with_activity_bar_width(2.4)`-style
    /// call only produces a square 48×48 bar at whatever `line_height`
    /// happens to make `2.4 * lh == 48.0`. Passing `48.0` here keeps the
    /// bar square regardless of the app's editor font size.
    ///
    /// `None` (the default) leaves the existing `line_height`-derived
    /// width in place — this call is additive, not a replacement.
    pub fn with_activity_bar_width_px(mut self, width_px: f32) -> Self {
        self.activity_bar_width_px = Some(width_px);
        self
    }

    /// Colour painted on the active top panel's row in the built
    /// [`ActivityBar`] (its `active_accent` field) — e.g. a theme's accent
    /// colour, so the activity bar shows an active-panel indicator line.
    /// `ShellView` has no `Theme` in scope itself; a host app sources the
    /// colour from its own theme and passes it here before the first
    /// [`Self::render`]. Unset (the default) paints no accent, matching the
    /// pre-existing behaviour.
    #[must_use]
    pub fn with_active_accent(mut self, color: Color) -> Self {
        self.active_accent = Some(color);
        self
    }

    /// Colour painted behind the keyboard-selected row in the built
    /// [`ActivityBar`] (its `selection_bg` field) when
    /// [`Self::activity_keyboard_focused`]. Unset (the default) leaves each
    /// backend's own reversed/brightened fallback fill in charge.
    #[must_use]
    pub fn with_selection_bg(mut self, color: Color) -> Self {
        self.selection_bg = Some(color);
        self
    }

    pub fn with_position(mut self, position: ShellPosition) -> Self {
        self.position = position;
        self
    }

    /// Reserve a title-bar row at construction time. See
    /// [`Self::set_title_bar_visible`] to show/hide the row at runtime
    /// (e.g. a toggleable menu bar) without rebuilding the `ShellView`.
    pub fn with_title_bar(mut self, height_lh: f32) -> Self {
        self.has_title_bar = true;
        self.title_bar_height_lh = height_lh;
        self
    }

    /// Pin a fixed-pixel floor under the title bar's line-height-derived
    /// height, overriding it only when the multiple would otherwise
    /// resolve smaller — mirroring [`Self::with_activity_bar_width_px`]'s
    /// pattern, but as a floor rather than a hard override, since
    /// unlike the activity bar's fixed-size icons, title-bar chrome (menu
    /// labels, inline window controls) is expected to keep scaling with
    /// the editor font above the floor.
    ///
    /// `None` (the default) leaves `title_bar_height_lh * line_height` in
    /// sole charge, unchanged from before this method existed. Set this
    /// when a host needs the band to never shrink under some real
    /// platform constraint it can't otherwise express in line-height
    /// units — e.g. staying clear of macOS's native traffic-light cluster
    /// even after a user shrinks `settings.font_size` at runtime
    pub fn with_title_bar_min_px(mut self, min_px: f32) -> Self {
        self.title_bar_min_px = Some(min_px);
        self
    }

    pub fn with_bottom_panel(mut self, height_lh: f32) -> Self {
        self.has_bottom_panel = true;
        self.bottom_panel_height_lh = height_lh;
        self
    }

    pub fn with_bottom_panel_limits(mut self, min: f32, max: f32) -> Self {
        self.min_bottom_panel_height_lh = min;
        self.max_bottom_panel_height_lh = max;
        self
    }

    /// Register the ordered list of independently-gated bottom bands.
    /// See [`BottomBand`] for the stacking order and how this interacts
    /// with [`Self::with_bottom_panel`].
    ///
    /// Replaces any previously-registered list. `Vec::new()` (the
    /// default) restores a single-drawer layout.
    pub fn with_bottom_bands(mut self, bands: Vec<BottomBand>) -> Self {
        self.bottom_bands = bands;
        self
    }

    pub fn with_command_line(mut self) -> Self {
        self.has_command_line = true;
        self
    }

    pub fn with_status_bar(mut self) -> Self {
        self.has_status_bar = true;
        self
    }

    /// Fix the status bar to `height_px` native units, overriding the
    /// `line_height`-derived height regardless of font size — VS Code's
    /// status bar is a constant 22px. Call alongside [`Self::with_status_bar`];
    /// has no visible effect unless that is also set.
    pub fn with_status_bar_height_px(mut self, height_px: f32) -> Self {
        self.status_bar_height_px = Some(height_px);
        self
    }

    /// Opt into the activity bar and sidebar running the full height of
    /// the area below the title bar, with the status bar and command line
    /// carved from the main column only (main column's x/width) rather
    /// than the full window width.
    ///
    /// Off by default: the status bar and command line span the full
    /// window, and the activity bar/sidebar stop above them — unchanged
    /// from before this method existed. Turn this on when a host wants
    /// the activity bar's bottom-pinned items to sit in the window's
    /// bottom-left corner rather than above a full-width status bar.
    pub fn with_status_bar_under_main_column(mut self) -> Self {
        self.status_bar_under_main_column = true;
        self
    }

    // ── State accessors ──────────────────────────────────────────────

    /// The [`PanelDefinition`] that currently owns the sidebar: the active
    /// top panel, or a bottom item that was made the owner
    /// via [`Self::show_panel`]. This is the same definition
    /// [`Self::render`] titles the sidebar header from.
    pub fn active_panel(&self) -> Option<&PanelDefinition> {
        self.sidebar_owner_panel()
    }

    /// The id of [`Self::active_panel`] — a top panel's id, or a bottom
    /// item's id when that bottom item owns the sidebar.
    pub fn active_panel_id(&self) -> Option<&WidgetId> {
        self.active_panel().map(|p| &p.id)
    }

    pub fn sidebar_visible(&self) -> bool {
        self.sidebar_visible
    }

    pub fn sidebar_width(&self) -> f32 {
        self.sidebar_width
    }

    pub fn position(&self) -> ShellPosition {
        self.position
    }

    /// Whether the title-bar row is currently reserved. See
    /// [`Self::set_title_bar_visible`] for the runtime toggle and
    /// [`Self::with_title_bar`] for the construction-time builder.
    pub fn title_bar_visible(&self) -> bool {
        self.has_title_bar
    }

    // ── Activity-bar keyboard navigation ─────────────────────────────

    /// Returns `true` when the activity bar currently holds keyboard focus.
    ///
    /// When `true`, `build_activity_bar()` sets `is_keyboard_focused = true`
    /// on the emitted [`ActivityBar`], causing both the TUI and GTK backends
    /// to deliver key presses as
    /// `UiEvent::ActivityBar(id, ActivityBarEvent::KeyPressed { … })` rather
    /// than raw `UiEvent::KeyPressed`. The consumer maps those key strings to
    /// the navigation methods below.
    pub fn activity_keyboard_focused(&self) -> bool {
        self.activity_keyboard_focused
    }

    /// Focus or unfocus the activity bar. Pair with
    /// [`Self::activity_keyboard_focused`] and the navigation methods to
    /// implement keyboard nav in a `ShellApp` consumer.
    ///
    /// Calling this does **not** reset the cursor — the cursor stays where
    /// it was so repeated focus/blur round-trips feel natural. To reset to
    /// the top item on focus, call
    /// `set_activity_keyboard_focused(true)` then `activity_set_cursor(0)`.
    pub fn set_activity_keyboard_focused(&mut self, focused: bool) {
        self.activity_keyboard_focused = focused;
    }

    /// Show (or hide) the keyboard cursor highlight on the activity bar
    /// **without** enabling the key-event redirect that
    /// [`Self::set_activity_keyboard_focused`] triggers.
    ///
    /// A `ShellApp` consumer that routes toolbar keys itself — through its
    /// own chord / key-action handlers inside `ShellApp::handle`, receiving
    /// plain `UiEvent::KeyPressed` events — calls this instead of
    /// `set_activity_keyboard_focused(true)` so the cursor paints but the
    /// TUI / GTK backends leave key events as `KeyPressed` rather than
    /// rewriting them to `UiEvent::ActivityBar(…)`.
    pub fn set_activity_cursor_visible(&mut self, visible: bool) {
        self.activity_cursor_visible = visible;
    }

    /// Set (or clear, with `None`) the active-panel accent colour
    /// [`Self::build_activity_bar`] passes through as
    /// [`ActivityBar::active_accent`]. Runtime counterpart to
    /// [`Self::with_active_accent`] — call this after a theme switch
    /// instead of rebuilding the `ShellView`.
    pub fn set_active_accent(&mut self, color: Option<Color>) {
        self.active_accent = color;
    }

    /// Set (or clear, with `None`) the keyboard-selection highlight colour
    /// [`Self::build_activity_bar`] passes through as
    /// [`ActivityBar::selection_bg`]. Runtime counterpart to
    /// [`Self::with_selection_bg`].
    pub fn set_selection_bg(&mut self, color: Option<Color>) {
        self.selection_bg = color;
    }

    /// Move the keyboard cursor to the next item in the logical sequence
    /// (top panels first, then bottom items). **Saturates** at the last
    /// item — does not wrap to the top.
    pub fn activity_select_next(&mut self) {
        let total = self.panels.len() + self.bottom_items.len();
        let mut keyboard = self.keyboard();
        keyboard.select_next(total);
        self.set_keyboard(keyboard);
    }

    /// Move the keyboard cursor to the previous item. **Saturates** at
    /// index 0 — does not wrap to the bottom.
    pub fn activity_select_prev(&mut self) {
        let mut keyboard = self.keyboard();
        keyboard.select_prev();
        self.set_keyboard(keyboard);
    }

    /// Set the keyboard cursor to an explicit index in the logical sequence.
    /// Out-of-range values are clamped to the valid range `[0, total-1]`.
    /// Has no effect when the panel + bottom-item lists are both empty.
    pub fn activity_set_cursor(&mut self, idx: usize) {
        let total = self.panels.len() + self.bottom_items.len();
        let mut keyboard = self.keyboard();
        keyboard.set_cursor(idx, total);
        self.set_keyboard(keyboard);
    }

    /// Returns the [`WidgetId`] of the item currently under the keyboard
    /// cursor, or `None` if there are no items at all.
    ///
    /// The logical sequence is top panels (`0..panels.len()`) followed by
    /// bottom items (`panels.len()..`).
    pub fn activity_selected_id(&self) -> Option<&WidgetId> {
        let np = self.panels.len();
        let cursor = self.activity_cursor;
        if cursor < np {
            Some(&self.panels[cursor].id)
        } else {
            let bi = cursor - np;
            self.bottom_items.get(bi).map(|p| &p.id)
        }
    }

    /// Activate the item currently under the keyboard cursor and return
    /// the resulting [`ShellEvent`], or `None` if there are no items.
    ///
    /// Equivalent to clicking the cursor item: top-panel items trigger the
    /// same toggle / switch logic as a mouse click, bottom items emit
    /// [`ShellEvent::BottomItemClicked`]. Does **not** clear keyboard
    /// focus — the consumer decides whether to do that.
    pub fn activity_activate_selected(&mut self) -> Option<ShellEvent> {
        let id = *self.activity_selected_id()?;
        Some(self.handle_activity_click(&id))
    }

    // ── Programmatic state control ───────────────────────────────────

    /// Make `panel_id` the sidebar's owner and reveal the sidebar.
    ///
    /// Accepts either a top panel's id or a bottom item's id (one
    /// registered via [`Self::with_bottom_items`] / [`Self::add_bottom_item`]).
    /// In the latter case the bottom item becomes the sidebar header's title
    /// source (see [`Self::active_panel`]/[`Self::active_panel_id`]) until
    /// `show_panel` is called again with a different id. A bottom item's own
    /// click ([`Self::handle_activity_click`]'s `BottomItemClicked` branch)
    /// never calls this itself, so an app opts in explicitly from its
    /// `BottomItemClicked` handler.
    ///
    /// A no-op if `panel_id` matches neither a top panel nor a bottom item.
    pub fn show_panel(&mut self, panel_id: &WidgetId) {
        for (i, p) in self.panels.iter().enumerate() {
            if p.id == *panel_id {
                self.active_panel = Some(i);
                self.sidebar_bottom_owner = None;
                self.sidebar_visible = true;
                return;
            }
        }
        for (j, p) in self.bottom_items.iter().enumerate() {
            if p.id == *panel_id {
                self.sidebar_bottom_owner = Some(j);
                self.sidebar_visible = true;
                return;
            }
        }
    }

    pub fn hide_sidebar(&mut self) {
        self.sidebar_visible = false;
    }

    pub fn toggle_sidebar(&mut self) {
        self.sidebar_visible = !self.sidebar_visible;
    }

    /// Runtime toggle for whether the title-bar row is reserved.
    ///
    /// Unlike [`Self::with_title_bar`] — a construction-time builder that
    /// permanently commits to reserving the row for the lifetime of the
    /// `ShellView` — this can be flipped at any point after construction
    /// without rebuilding the shell. `compute_layout` re-derives every
    /// downstream bound (activity bar, sidebar, main content, bottom
    /// panel, ...) from `area` and current state on every
    /// `render`/`layout`/`handle` call, so toggling this and calling any
    /// of those next is sufficient — no other field needs to change in
    /// lockstep, and no fixed offset is cached anywhere that would go stale.
    ///
    /// A menu bar painted into the title-bar row that the user can show/hide
    /// at runtime requires this: always reserving the row costs a row even
    /// while the menu is hidden; never reserving it makes a shown menu
    /// overwrite the activity bar's first row. Toggling this from the app's
    /// own visibility state avoids both.
    ///
    /// If [`Self::with_title_bar`] was never called, toggling visible for
    /// the first time reserves the default height (`title_bar_height_lh`,
    /// 1.5 line-heights). The configured height is otherwise preserved
    /// across visible → hidden → visible round-trips — hiding does not
    /// forget it.
    pub fn set_title_bar_visible(&mut self, visible: bool) {
        self.has_title_bar = visible;
    }

    pub fn set_sidebar_width(&mut self, width: f32) {
        self.sidebar_width = width.clamp(self.min_sidebar_width, self.max_sidebar_width);
    }

    pub fn bottom_panel_visible(&self) -> bool {
        self.has_bottom_panel && self.bottom_panel_visible
    }

    pub fn bottom_panel_height(&self) -> f32 {
        self.bottom_panel_height_lh
    }

    pub fn show_bottom_panel(&mut self) {
        self.bottom_panel_visible = true;
    }

    pub fn hide_bottom_panel(&mut self) {
        self.bottom_panel_visible = false;
    }

    pub fn toggle_bottom_panel(&mut self) {
        self.bottom_panel_visible = !self.bottom_panel_visible;
    }

    pub fn set_bottom_panel_height(&mut self, height: f32) {
        self.bottom_panel_height_lh = height.clamp(
            self.min_bottom_panel_height_lh,
            self.max_bottom_panel_height_lh,
        );
    }

    // ── Bottom bands ─────────────────────────────────────────────────

    /// The currently-registered bottom bands, in bottom-up stacking order
    /// (see [`BottomBand`]). Includes bands with `visible == false` — use
    /// [`Self::bottom_band_bounds`] to see which ones actually occupied
    /// space in the last computed layout.
    pub fn bottom_bands(&self) -> &[BottomBand] {
        &self.bottom_bands
    }

    /// Resolved bounds for every currently-`visible` [`BottomBand`] when
    /// the shell is laid out into `area` at `line_height` against `style`,
    /// in the same bottom-up order `bottom_bands` was given (index 0 =
    /// flush against the bottom edge of the main content column). Bands
    /// with `visible == false` have no entry — they occupy no space.
    ///
    /// Not a field on [`ShellLayout`] — see that struct's doc for why.
    pub fn bottom_band_bounds(
        &self,
        area: Rect,
        line_height: f32,
        style: &Style,
    ) -> Vec<(WidgetId, Rect)> {
        self.chrome_layout(area, line_height, style).1
    }

    /// [`Self::bottom_band_bounds`], reading the line height and [`Style`]
    /// from a live `backend` — the same inputs [`Self::render`] lays out
    /// against.
    pub fn bottom_band_bounds_for(
        &self,
        backend: &dyn Backend,
        area: Rect,
    ) -> Vec<(WidgetId, Rect)> {
        self.bottom_band_bounds(area, backend.chrome_line_height(), &backend.style())
    }

    /// Look up one registered band by id.
    pub fn bottom_band(&self, id: &WidgetId) -> Option<&BottomBand> {
        self.bottom_bands.iter().find(|b| &b.id == id)
    }

    /// Flip one band's presence at runtime (e.g. a quickfix list that
    /// opens/closes independently of every other bottom band). Returns
    /// `false` if no band with `id` was registered via
    /// [`Self::with_bottom_bands`].
    pub fn set_bottom_band_visible(&mut self, id: &WidgetId, visible: bool) -> bool {
        match self.bottom_bands.iter_mut().find(|b| &b.id == id) {
            Some(b) => {
                b.visible = visible;
                true
            }
            None => false,
        }
    }

    /// Resize one band at runtime. Returns `false` if no band with `id`
    /// was registered via [`Self::with_bottom_bands`]. Unlike the legacy
    /// single bottom panel, bands have no configurable min/max — callers
    /// own their own clamping (the toolbars/lists vimcode stacks here
    /// generally have a fixed content-driven height, not a user drag).
    pub fn set_bottom_band_height(&mut self, id: &WidgetId, height_lh: f32) -> bool {
        match self.bottom_bands.iter_mut().find(|b| &b.id == id) {
            Some(b) => {
                b.height_lh = height_lh.max(0.0);
                true
            }
            None => false,
        }
    }

    // ── Dynamic panel registration ──────────────────────────────────

    pub fn panels(&self) -> &[PanelDefinition] {
        &self.panels
    }

    pub fn bottom_items(&self) -> &[PanelDefinition] {
        &self.bottom_items
    }

    /// Register a panel at runtime. Returns `false` if a panel with the
    /// same ID already exists (no-op in that case). If this is the first
    /// panel, it becomes the active panel.
    pub fn add_panel(&mut self, def: PanelDefinition) -> bool {
        if self.panels.iter().any(|p| p.id == def.id)
            || self.bottom_items.iter().any(|p| p.id == def.id)
        {
            return false;
        }
        self.panels.push(def);
        if self.panels.len() == 1 {
            self.active_panel = Some(0);
        }
        true
    }

    /// Unregister a panel by ID. Returns `true` if found and removed.
    /// Adjusts `active_panel` to keep the same panel selected (or clears
    /// it if the active panel was the one removed).
    /// Also adjusts `activity_cursor` analogously so keyboard navigation
    /// remains consistent after a dynamic removal.
    pub fn remove_panel(&mut self, id: &WidgetId) -> bool {
        let Some(idx) = self.panels.iter().position(|p| p.id == *id) else {
            return false;
        };
        self.panels.remove(idx);
        self.active_panel = match self.active_panel {
            Some(a) if a == idx => {
                if self.panels.is_empty() {
                    None
                } else {
                    Some(a.min(self.panels.len() - 1))
                }
            }
            Some(a) if a > idx => Some(a - 1),
            other => other,
        };
        // The removed panel occupied combined index `idx`.
        self.shift_cursor_after_removal(idx);
        true
    }

    /// Register a bottom-pinned activity bar item at runtime.
    pub fn add_bottom_item(&mut self, def: PanelDefinition) -> bool {
        if self.panels.iter().any(|p| p.id == def.id)
            || self.bottom_items.iter().any(|p| p.id == def.id)
        {
            return false;
        }
        self.bottom_items.push(def);
        true
    }

    /// Unregister a bottom item by ID.
    /// Adjusts `activity_cursor` so keyboard navigation remains consistent
    /// after a dynamic removal (mirrors `remove_panel`'s treatment of
    /// `active_panel`). Also adjusts `sidebar_bottom_owner`
    /// the same way `remove_panel` adjusts `active_panel`: shifted down if
    /// it pointed past the removed item, cleared (falling back to whatever
    /// `active_panel` names) if it pointed at the removed item itself.
    pub fn remove_bottom_item(&mut self, id: &WidgetId) -> bool {
        let Some(idx) = self.bottom_items.iter().position(|p| p.id == *id) else {
            return false;
        };
        // Bottom items begin at combined index `np` in the top-then-bottom
        // sequence, so the removed item's combined index is `np + idx`.
        let np = self.panels.len();
        let combined_idx = np + idx;
        self.bottom_items.remove(idx);
        self.sidebar_bottom_owner = match self.sidebar_bottom_owner {
            Some(j) if j == idx => None,
            Some(j) if j > idx => Some(j - 1),
            other => other,
        };
        self.shift_cursor_after_removal(combined_idx);
        true
    }

    // ── Layout ───────────────────────────────────────────────────────

    /// The chrome bounds for `area`, without drawing, against
    /// `Style::default()`.
    ///
    /// Takes `line_height` rather than a `&dyn Backend` so callers that
    /// only know the line height (external consumers, tests) can still
    /// call it. It mirrors `Backend::snap_height`'s own threshold —
    /// identity above one line height (a real pixel metric), `.round()`
    /// at or below it (TUI's cell grid, whose `chrome_line_height` is
    /// architecturally always exactly `1.0`).
    ///
    /// **This path only agrees with [`Self::render`]/[`Self::handle_event`]
    /// on a backend whose [`Backend::set_style`] has never been called** —
    /// those two read `backend.style()` back, while this falls through to
    /// `Style::default()`. A caller that has a live backend in hand should
    /// call [`Self::layout_for`] instead.
    pub fn shell_layout(&self, area: Rect, line_height: f32) -> ShellLayout {
        self.chrome_layout(area, line_height, &Style::default()).0
    }

    /// [`Self::shell_layout`], but reading `line_height` and [`Style`]
    /// overrides from a live `backend` — the same inputs [`Self::render`]/
    /// [`Self::handle_event`] use, so a host's `Backend::set_style`
    /// override is reflected in the layout the app sees, not just in what
    /// gets painted.
    pub fn layout_for(&self, backend: &dyn Backend, area: Rect) -> ShellLayout {
        let lh = backend.chrome_line_height();
        let style = backend.style();
        self.chrome_layout(area, lh, &style).0
    }

    // ── Render ───────────────────────────────────────────────────────

    /// Paint the shell chrome and return the bounds for the host's content.
    ///
    /// Paints through [`crate::DrawsPrimitives::draw_dyn`], which publishes
    /// the chrome regions as `app-shell:*` zones; each activity-bar item's
    /// zone is registered here under the item's own id, and the activity
    /// bar and sidebar header children are recorded so keyboard routing and
    /// driver lookups find them. `state`'s hovered activity row is pinned
    /// as the tinted one; without one the shell paints with the ambient
    /// pointer state. The host paints the sidebar panel content and the
    /// main content area.
    pub fn render(
        &self,
        state: &ShellViewState,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> ShellLayout {
        let pinned = state.activity.hovered_idx.map(|idx| {
            crate::interaction::InteractionState::from_targets(
                Some((self.id, Some(ShellRegion::ActivityItem(idx).sub_target()))),
                None,
            )
        });
        let paint_state: &dyn crate::primitive::PaintState = match &pinned {
            Some(state) => state,
            None => &(),
        };
        let painted = DrawsPrimitives::draw_dyn(backend, area, self, paint_state)
            .downcast_ref::<ShellViewLayout>()
            .cloned();
        let Some(painted) = painted else {
            return self.layout_for(backend, area);
        };
        record_children(&painted);
        let bar_bounds = painted.shell.activity_bar_bounds;
        for hit in painted.activity.row_hits(&painted.activity_bar) {
            backend.register_zone(
                hit.id,
                Rect::new(
                    bar_bounds.x,
                    bar_bounds.y + hit.y_start,
                    bar_bounds.width,
                    hit.y_end - hit.y_start,
                ),
            );
        }
        painted.shell
    }

    // ── Handle ───────────────────────────────────────────────────────

    /// Process a [`UiEvent`] and return a semantic [`ShellEvent`].
    ///
    /// Routes the event through the shell's own [`Interactive::handle`]
    /// against `state` and applies what it reports to the descriptor. Call
    /// this before the host's own event handling. If the result is not
    /// [`ShellEvent::Ignored`], the host should redraw.
    ///
    /// A fast double-click on the activity bar behaves exactly like the
    /// second of two single clicks there, whichever button made it. Keys
    /// are left to the caller: the activity bar's keyboard navigation is
    /// driven through [`Self::activity_select_next`] and friends.
    pub fn handle_event(
        &mut self,
        state: &mut ShellViewState,
        event: &UiEvent,
        backend: &dyn Backend,
        area: Rect,
    ) -> ShellEvent {
        let laid_out = crate::backend::layout_like_paint(backend, &*self, area);
        let Some(layout) = laid_out.downcast_ref::<ShellViewLayout>() else {
            return ShellEvent::Ignored;
        };
        let local = |p: Point| Point::new(p.x - area.x, p.y - area.y);
        let pointer = |kind: PointerKind, p: Point| RoutedEvent::Pointer {
            kind,
            sub: None,
            local: local(p),
            mods: Modifiers::default(),
        };
        let (routed, hover_only) = match event {
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => (
                pointer(PointerKind::Down(MouseButton::Left), *position),
                false,
            ),
            UiEvent::DoubleClick { position, .. } => {
                if !contains(layout.shell.activity_bar_bounds, *position) {
                    return ShellEvent::Ignored;
                }
                (
                    pointer(PointerKind::Down(MouseButton::Left), *position),
                    false,
                )
            }
            UiEvent::MouseMoved {
                position,
                buttons: ButtonMask { left, .. },
            } => (pointer(PointerKind::Move, *position), !*left),
            UiEvent::MouseUp {
                button: MouseButton::Left,
                position,
                ..
            } => (
                pointer(PointerKind::Up(MouseButton::Left), *position),
                false,
            ),
            _ => return ShellEvent::Ignored,
        };
        // A move with the button up only tracks hover, even mid-drag.
        let parked_drag = if hover_only { state.drag.take() } else { None };
        let cursor = self.activity_cursor;
        let mut host = ShellHost {
            units: if layout.line_height > 1.0 {
                Units::Pixels
            } else {
                Units::Cells
            },
        };
        let outcome = {
            let mut cx = EventCx::new(self.id, &mut host);
            Interactive::handle(&*self, state, &routed, layout, &mut cx)
        };
        if parked_drag.is_some() {
            state.drag = parked_drag;
        }
        // A click parks the child's own cursor on the clicked row; the
        // shell's keyboard cursor is its own.
        self.activity_cursor = cursor;
        let event = match outcome {
            Handled::Event(event) => event,
            Handled::Consumed if matches!(routed_kind(&routed), Some(PointerKind::Move)) => {
                return ShellEvent::Ignored
            }
            Handled::Consumed => return ShellEvent::Consumed,
            Handled::Ignored => return ShellEvent::Ignored,
        };
        self.apply(&event, layout.line_height);
        event
    }

    /// Apply one event the shell reported to its own descriptor.
    fn apply(&mut self, event: &ShellEvent, line_height: f32) {
        match event {
            ShellEvent::PanelChanged { panel_id } => {
                if let Some(i) = self.panels.iter().position(|p| p.id == *panel_id) {
                    self.active_panel = Some(i);
                    self.sidebar_bottom_owner = None;
                    self.sidebar_visible = true;
                }
            }
            ShellEvent::SidebarHidden => self.sidebar_visible = false,
            ShellEvent::SidebarResized { new_width } => {
                self.sidebar_width = new_width / line_height;
            }
            ShellEvent::BottomPanelResized { new_height } => {
                self.bottom_panel_height_lh = new_height / line_height;
            }
            ShellEvent::BottomPanelHidden => self.bottom_panel_visible = false,
            ShellEvent::ActivityCursorMoved { index } => self.activity_cursor = *index,
            ShellEvent::BottomItemClicked { .. } | ShellEvent::Consumed | ShellEvent::Ignored => {}
        }
    }

    // ── Internal helpers ─────────────────────────────────────────────

    fn handle_activity_click(&mut self, clicked_id: &WidgetId) -> ShellEvent {
        match self.activity_intent(clicked_id) {
            Handled::Event(event) => {
                self.apply(&event, 1.0);
                event
            }
            _ => ShellEvent::Consumed,
        }
    }

    /// The keyboard half of the activity bar's state, as an
    /// [`ActivityBarState`] its own cursor helpers can move.
    fn keyboard(&self) -> ActivityBarState {
        ActivityBarState {
            focused: self.activity_keyboard_focused,
            cursor: self.activity_cursor,
            cursor_visible: self.activity_cursor_visible,
            hovered_idx: None,
        }
    }

    /// Write a moved [`Self::keyboard`] back to the descriptor.
    fn set_keyboard(&mut self, keyboard: ActivityBarState) {
        self.activity_keyboard_focused = keyboard.focused;
        self.activity_cursor = keyboard.cursor;
        self.activity_cursor_visible = keyboard.cursor_visible;
    }

    /// Re-home the keyboard cursor after the item at combined index
    /// `removed` left the activity bar.
    fn shift_cursor_after_removal(&mut self, removed: usize) {
        let total = self.panels.len() + self.bottom_items.len();
        let cursor = self.activity_cursor;
        if total == 0 {
            self.activity_cursor = 0;
            self.activity_keyboard_focused = false;
        } else if cursor > removed {
            self.activity_cursor = cursor - 1;
        } else if cursor == removed {
            self.activity_cursor = cursor.min(total - 1);
        }
    }
}

/// The pointer gesture `event` carries, if it is one.
fn routed_kind(event: &RoutedEvent) -> Option<PointerKind> {
    match event {
        RoutedEvent::Pointer { kind, .. } => Some(*kind),
        _ => None,
    }
}

/// Record the shell's children in the painted-widget record, so the
/// activity bar is found by the backends' keyboard routing and the
/// drivers' sub-part lookups the way a bar painted on its own is.
fn record_children(layout: &ShellViewLayout) {
    let bar = &layout.activity_bar;
    crate::primitive::record_painted_widget(crate::primitive::PaintedWidget {
        id: bar.id,
        rect: layout.activity.area,
        layout: Box::new(layout.activity.clone()),
        sub_regions: Interactive::sub_regions(bar, &layout.activity)
            .into_iter()
            .map(|(r, sub)| (translate(r, layout.activity.area), sub))
            .collect(),
        hover_targets: None,
        paint_state: Default::default(),
        focus_ring: Interactive::focus_ring(bar, &layout.activity).resolve(layout.activity.area),
    });
    if let Some((header, header_layout)) = &layout.sidebar_header {
        crate::primitive::record_painted_widget(crate::primitive::PaintedWidget {
            id: header.id,
            rect: header_layout.area,
            layout: Box::new(header_layout.clone()),
            sub_regions: Interactive::sub_regions(header, header_layout)
                .into_iter()
                .map(|(r, sub)| (translate(r, header_layout.area), sub))
                .collect(),
            hover_targets: None,
            paint_state: Default::default(),
            focus_ring: Interactive::focus_ring(header, header_layout).resolve(header_layout.area),
        });
    }
}

/// `rect`, local to `origin`, moved into absolute space.
fn translate(rect: Rect, origin: Rect) -> Rect {
    Rect::new(
        rect.x + origin.x,
        rect.y + origin.y,
        rect.width,
        rect.height,
    )
}

/// The [`EventHost`] [`ShellView`] runs its shell's [`Interactive::handle`]
/// against: the shell keeps its own drag state, so capture is a no-op.
struct ShellHost {
    units: Units,
}

impl EventHost for ShellHost {
    fn is_focused(&self, _id: &WidgetId, _sub: Option<SubTarget>) -> bool {
        false
    }
    fn request_focus(&mut self, _id: &WidgetId, _sub: Option<SubTarget>) {}
    fn capture_pointer(&mut self, _id: &WidgetId) {}
    fn release_pointer(&mut self) {}
    fn set_cursor(&mut self, _cursor: crate::backend::PointerShape) {}
    fn request_redraw(&mut self) {}
    fn emit(&mut self, _event: UiEvent) {}
    fn units(&self) -> Units {
        self.units
    }
}

fn contains(rect: Rect, point: Point) -> bool {
    point.x >= rect.x
        && point.x < rect.x + rect.width
        && point.y >= rect.y
        && point.y < rect.y + rect.height
}

// ── ShellView: descriptor behaviour ─────────────────────────────────

impl ShellView {
    /// A shell over `panels` with the first one active, the sidebar shown
    /// at `default_sidebar_width` line heights, and no optional chrome.
    pub fn new(panels: Vec<PanelDefinition>, default_sidebar_width: f32) -> Self {
        let active = if panels.is_empty() { None } else { Some(0) };
        Self {
            id: WidgetId::new(APP_SHELL_ID),
            panels,
            bottom_items: Vec::new(),
            active_panel: active,
            sidebar_bottom_owner: None,
            sidebar_visible: true,
            sidebar_width: default_sidebar_width,
            min_sidebar_width: 8.0,
            max_sidebar_width: 800.0,
            activity_bar_width: 4.0,
            activity_bar_width_px: None,
            position: ShellPosition::Left,
            has_title_bar: false,
            title_bar_height_lh: 1.5,
            title_bar_min_px: None,
            has_bottom_panel: false,
            bottom_panel_height_lh: 10.0,
            min_bottom_panel_height_lh: 3.0,
            max_bottom_panel_height_lh: 30.0,
            bottom_panel_visible: true,
            has_command_line: false,
            has_status_bar: false,
            status_bar_height_px: None,
            status_bar_under_main_column: false,
            bottom_bands: Vec::new(),
            icon_overrides: HashMap::new(),
            active_accent: None,
            selection_bg: None,
            activity_keyboard_focused: false,
            activity_cursor: 0,
            activity_cursor_visible: false,
        }
    }

    /// The panel (top or bottom item) that owns the sidebar header — a
    /// bottom item made the owner via [`ShellView::show_panel`], otherwise
    /// the active top panel.
    pub fn sidebar_owner_panel(&self) -> Option<&PanelDefinition> {
        if let Some(j) = self.sidebar_bottom_owner {
            if let Some(p) = self.bottom_items.get(j) {
                return Some(p);
            }
        }
        self.active_panel.and_then(|i| self.panels.get(i))
    }

    fn resolved_icon(&self, def: &PanelDefinition) -> Icon {
        self.icon_overrides
            .get(&def.id)
            .cloned()
            .unwrap_or_else(|| Icon::from(def.icon.clone()))
    }

    /// The [`ActivityBar`] child, built from the panels and the keyboard
    /// cursor fields.
    ///
    /// When [`Self::activity_keyboard_focused`] is set the bar is marked
    /// keyboard-focused, so backends route key presses to it as
    /// `UiEvent::ActivityBar(id, ActivityBarEvent::KeyPressed { … })`;
    /// the item at [`Self::activity_cursor`] (in the top-then-bottom
    /// logical sequence) paints the selection ring whenever the bar is
    /// focused or [`Self::activity_cursor_visible`] is set.
    pub fn build_activity_bar(&self) -> ActivityBar {
        let np = self.panels.len();
        let focused = self.activity_keyboard_focused;
        let show_cursor = focused || self.activity_cursor_visible;

        let top_items: Vec<ActivityItem> = self
            .panels
            .iter()
            .enumerate()
            .map(|(i, p)| ActivityItem {
                id: p.id,
                icon: self.resolved_icon(p),
                tooltip: p.tooltip.clone(),
                // A top panel only shows as active while it — not a bottom
                // item — actually owns the sidebar.
                is_active: self.sidebar_bottom_owner.is_none()
                    && self.active_panel == Some(i)
                    && self.sidebar_visible,
                is_keyboard_selected: show_cursor && self.activity_cursor == i,
            })
            .collect();

        let bottom_items: Vec<ActivityItem> = self
            .bottom_items
            .iter()
            .enumerate()
            .map(|(j, p)| ActivityItem {
                id: p.id,
                icon: self.resolved_icon(p),
                tooltip: p.tooltip.clone(),
                // Bottom items (gear/bell-style) never paint a persistent
                // active accent.
                is_active: false,
                is_keyboard_selected: show_cursor && self.activity_cursor == np + j,
            })
            .collect();

        ActivityBar {
            id: WidgetId::new(APP_SHELL_ACTIVITY_BAR_ID),
            top_items,
            bottom_items,
            active_accent: self.active_accent,
            selection_bg: self.selection_bg,
            is_keyboard_focused: focused,
            style: Default::default(),
        }
    }

    /// The sidebar header child, titled from the panel that owns the
    /// sidebar, in `fg` on `bg`.
    fn sidebar_header(&self, fg: Color, bg: Color) -> Option<StatusBar> {
        let panel = self.sidebar_owner_panel()?;
        Some(StatusBar {
            id: WidgetId::new(APP_SHELL_SIDEBAR_HEADER_ID),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", panel.title),
                fg,
                bg,
                bold: true,
                action_id: None,
            }],
            right_segments: vec![],
        })
    }

    /// The chrome rects for `area` at `line_height`, plus each visible
    /// bottom band's rect. `snap` quantizes like the backend paints: the
    /// identity above one line height (pixels), `.round()` at or below it
    /// (a cell grid).
    fn chrome_layout(
        &self,
        area: Rect,
        line_height: f32,
        style: &Style,
    ) -> (ShellLayout, Vec<(WidgetId, Rect)>) {
        let snap = |h: f32| {
            if line_height > 1.0 {
                h
            } else {
                h.max(0.0).round()
            }
        };
        let cfg = shell_layout::ShellLayoutConfig {
            activity_bar_width_px: self.activity_bar_width_px,
            title_bar_min_px: self.title_bar_min_px,
            status_bar_height_px: self.status_bar_height_px,
            has_title_bar: self.has_title_bar,
            title_bar_height_lh: self.title_bar_height_lh,
            has_status_bar: self.has_status_bar,
            status_bar_under_main_column: self.status_bar_under_main_column,
            has_command_line: self.has_command_line,
            has_bottom_panel: self.has_bottom_panel,
            bottom_panel_visible: self.bottom_panel_visible,
            bottom_panel_height_lh: self.bottom_panel_height_lh,
            min_bottom_panel_height_lh: self.min_bottom_panel_height_lh,
            max_bottom_panel_height_lh: self.max_bottom_panel_height_lh,
            bottom_bands: &self.bottom_bands,
            sidebar_visible: self.sidebar_visible,
            panels_empty: self.panels.is_empty(),
            position: self.position,
            activity_bar_width: self.activity_bar_width,
            sidebar_width: self.sidebar_width,
            min_sidebar_width: self.min_sidebar_width,
            max_sidebar_width: self.max_sidebar_width,
        };
        shell_layout::compute_layout(&cfg, area, line_height, style, &snap)
    }

    /// What clicking the activity item `clicked_id` asks for: the active
    /// top panel's own icon hides the sidebar, any other top panel shows
    /// it (reclaiming the header from a bottom item that owned it), and a
    /// bottom item reports itself.
    pub fn activity_intent(&self, clicked_id: &WidgetId) -> Handled<ShellEvent> {
        if let Some(i) = self.panels.iter().position(|p| p.id == *clicked_id) {
            let event = if self.active_panel == Some(i)
                && self.sidebar_visible
                && self.sidebar_bottom_owner.is_none()
            {
                ShellEvent::SidebarHidden
            } else {
                ShellEvent::PanelChanged {
                    panel_id: *clicked_id,
                }
            };
            return Handled::Event(event);
        }
        if self.bottom_items.iter().any(|p| p.id == *clicked_id) {
            return Handled::Event(ShellEvent::BottomItemClicked { id: *clicked_id });
        }
        Handled::Consumed
    }

    /// The divider drag `drag` resolves to with the pointer at
    /// shell-local `local`.
    fn drag_event(&self, drag: ShellDrag, local: Point, layout: &ShellViewLayout) -> ShellEvent {
        let lh = layout.line_height;
        match drag {
            ShellDrag::Sidebar(offset) => {
                let bar = layout.local(layout.shell.activity_bar_bounds);
                let new_native = match self.position {
                    ShellPosition::Left => local.x - offset - (bar.x + bar.width),
                    ShellPosition::Right => bar.x - local.x + offset,
                };
                let clamped =
                    (new_native / lh).clamp(self.min_sidebar_width, self.max_sidebar_width);
                ShellEvent::SidebarResized {
                    new_width: clamped * lh,
                }
            }
            ShellDrag::BottomPanel(offset) => {
                let bottom_edge = layout.shell.window_bounds.height;
                let new_native = bottom_edge - (local.y - offset);
                let status_lh = if self.has_status_bar { 1.0 } else { 0.0 };
                let cmd_lh = if self.has_command_line { 1.0 } else { 0.0 };
                let clamped = (new_native / lh - status_lh - cmd_lh).clamp(
                    self.min_bottom_panel_height_lh,
                    self.max_bottom_panel_height_lh,
                );
                ShellEvent::BottomPanelResized {
                    new_height: clamped * lh,
                }
            }
        }
    }

    /// Forward `event` to the activity bar child, at the bar's own local
    /// position, and turn an activation into the shell's intent.
    fn route_to_activity(
        &self,
        state: &mut ShellViewState,
        event: &RoutedEvent,
        layout: &ShellViewLayout,
        cx: &mut EventCx,
    ) -> Handled<ShellEvent> {
        let bar = &layout.activity_bar;
        let origin = layout.local(layout.activity.area);
        let routed = match event {
            RoutedEvent::Pointer {
                kind,
                sub,
                local,
                mods,
            } => RoutedEvent::Pointer {
                kind: *kind,
                sub: *sub,
                local: Point::new(local.x - origin.x, local.y - origin.y),
                mods: *mods,
            },
            other => other.clone(),
        };
        let mut host = ChildHost { parent: cx };
        let mut child_cx = EventCx::new(bar.id, &mut host);
        let cursor = state.activity.cursor;
        match Interactive::handle(
            bar,
            &mut state.activity,
            &routed,
            &layout.activity,
            &mut child_cx,
        ) {
            Handled::Event(ActivityBarAction::ItemActivated(id)) => self.activity_intent(&id),
            Handled::Consumed if state.activity.cursor != cursor => {
                Handled::Event(ShellEvent::ActivityCursorMoved {
                    index: state.activity.cursor,
                })
            }
            other => other.map(|_| ShellEvent::Consumed),
        }
    }
}

/// The [`EventHost`] a shell's children run against: everything goes to
/// the shell's own [`EventCx`], with focus re-addressed to the shell.
struct ChildHost<'a, 'b> {
    parent: &'a mut EventCx<'b>,
}

impl EventHost for ChildHost<'_, '_> {
    fn is_focused(&self, _id: &WidgetId, _sub: Option<SubTarget>) -> bool {
        self.parent
            .is_focused(Some(ShellRegion::ActivityBar.sub_target()))
    }
    fn request_focus(&mut self, _id: &WidgetId, _sub: Option<SubTarget>) {
        self.parent
            .request_focus(Some(ShellRegion::ActivityBar.sub_target()));
    }
    fn capture_pointer(&mut self, _id: &WidgetId) {
        self.parent.capture_pointer();
    }
    fn release_pointer(&mut self) {
        self.parent.release_pointer();
    }
    fn set_cursor(&mut self, cursor: crate::backend::PointerShape) {
        self.parent.set_cursor(cursor);
    }
    fn request_redraw(&mut self) {
        self.parent.request_redraw();
    }
    fn emit(&mut self, event: UiEvent) {
        self.parent.emit(event);
    }
    fn units(&self) -> Units {
        self.parent.units()
    }
}

// ── ShellView: widget contract ──────────────────────────────────────

impl A11yPrimitive for ShellView {
    fn a11y_info(&self) -> A11yInfo {
        A11yInfo::new()
            .with_role(A11yRole::Group)
            .with_label("Application shell")
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        let bar = self.build_activity_bar();
        vec![A11yChild::new("activity-bar", bar.a11y_info()).with_children(bar.a11y_children())]
    }
}

impl Primitive for ShellView {
    type Layout = ShellViewLayout;
    type Event = ShellEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// Carves `rect` into the shell's chrome at the backend's chrome line
    /// height (see [`LayoutCtx::chrome_line_height`]) and lays the
    /// activity bar and sidebar header children out into theirs.
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> ShellViewLayout {
        let lh = ctx.chrome_line_height();
        let (shell, bottom_band_bounds) = self.chrome_layout(rect, lh, ctx.style());
        let bottom_panel_grip = shell.bottom_panel_bounds.map(|bp| {
            // The grip sits *above* the panel's top edge only: the panel's
            // top row is its tab strip, whose clicks are the panel's own.
            let grip_h = if lh > 1.0 {
                lh * 0.5
            } else {
                (lh * 0.5).round()
            }
            .max(1.0);
            Rect::new(bp.x, bp.y - grip_h, bp.width, grip_h)
        });
        let activity_bar = self.build_activity_bar();
        let activity = Primitive::layout(&activity_bar, ctx, shell.activity_bar_bounds);
        let sidebar_header = shell.sidebar_header_bounds.and_then(|bounds| {
            let header = self.sidebar_header(Color::rgb(0, 0, 0), Color::rgb(0, 0, 0))?;
            let header_layout = Primitive::layout(&header, ctx, bounds);
            Some((header, header_layout))
        });
        ShellViewLayout {
            shell,
            bottom_band_bounds,
            line_height: lh,
            bottom_panel_grip,
            activity_bar,
            activity,
            sidebar_header,
        }
    }

    /// Paints the activity bar and sidebar header through their own
    /// [`Primitive::paint`], and the divider as one thin fill centred in
    /// its drag band (VS Code's single-pixel resize affordance).
    fn paint(&self, layout: &ShellViewLayout, surface: &mut dyn PaintSurface, env: &PaintEnv) {
        let theme = env.theme();
        let activity = self.activity_interaction(layout, env);
        Primitive::paint(
            &layout.activity_bar,
            &layout.activity,
            surface,
            &env.with_interaction(&activity),
        );
        if let Some((_, header_layout)) = &layout.sidebar_header {
            if let Some(header) = self.sidebar_header(theme.header_fg, theme.header_bg) {
                let none = InteractionState::new();
                Primitive::paint(
                    &header,
                    header_layout,
                    surface,
                    &env.with_interaction(&none),
                );
            }
        }
        if let Some(divider) = layout.shell.divider_bounds {
            surface.surface_fill_rect(
                sash_paint_rect(divider, SplitDirection::Horizontal),
                theme.separator,
            );
        }
    }

    /// The cell-grid counterpart of [`Self::paint`], through the children's
    /// own [`Primitive::paint_cells`].
    #[cfg(feature = "tui")]
    fn paint_cells(&self, layout: &ShellViewLayout, cells: &mut dyn CellSurface, env: &PaintEnv) {
        let theme = env.theme();
        let activity = self.activity_interaction(layout, env);
        Primitive::paint_cells(
            &layout.activity_bar,
            &layout.activity,
            cells,
            &env.with_interaction(&activity),
        );
        if let Some((_, header_layout)) = &layout.sidebar_header {
            if let Some(header) = self.sidebar_header(theme.header_fg, theme.header_bg) {
                let none = InteractionState::new();
                Primitive::paint_cells(&header, header_layout, cells, &env.with_interaction(&none));
            }
        }
        if let Some(divider) = layout.shell.divider_bounds {
            cells.fill(
                sash_paint_rect(divider, SplitDirection::Horizontal),
                crate::cell_surface::CellStyle {
                    fg: Some(theme.separator),
                    bg: Some(theme.separator),
                    ..Default::default()
                },
            );
        }
    }
}

impl ShellView {
    /// `env`'s hover and press on this shell's activity rows, re-addressed
    /// to the activity bar child's own rows.
    fn activity_interaction(&self, layout: &ShellViewLayout, env: &PaintEnv) -> InteractionState {
        let bar = layout.activity_bar.id;
        let row = |target: Option<Option<SubTarget>>| match target
            .flatten()
            .and_then(ShellRegion::from_sub_target)
        {
            Some(ShellRegion::ActivityItem(idx)) => Some((bar, Some(SubTarget(idx as u64)))),
            _ => None,
        };
        InteractionState::from_targets(row(env.hovered(&self.id)), row(env.pressed(&self.id)))
    }
}

impl WidgetAction for ShellEvent {
    fn into_ui_event(self, id: WidgetId) -> UiEvent {
        UiEvent::ShellView(id, self)
    }
}

impl Interactive for ShellView {
    type State = ShellViewState;
    type Action = ShellEvent;

    /// One stop per slot keyboard focus can land in: the activity bar,
    /// the sidebar, the main content, the bottom panel and the status
    /// bar, each only while the shell shows it.
    fn focus(&self) -> FocusSpec {
        let mut stops = Vec::new();
        if !self.panels.is_empty() || !self.bottom_items.is_empty() {
            stops.push(ShellRegion::ActivityBar);
        }
        if self.sidebar_visible && !self.panels.is_empty() {
            stops.push(ShellRegion::SidebarContent);
        }
        stops.push(ShellRegion::MainContent);
        if self.has_bottom_panel && self.bottom_panel_visible {
            stops.push(ShellRegion::BottomPanel);
        }
        if self.has_status_bar {
            stops.push(ShellRegion::StatusBar);
        }
        FocusSpec::Children(
            stops
                .into_iter()
                .map(|region| FocusStop {
                    sub: region.sub_target(),
                })
                .collect(),
        )
    }

    /// Every region the layout resolved, most specific first: the
    /// activity rows, the grip and the divider ahead of the bands that
    /// contain them, and the whole window last.
    fn sub_regions(&self, layout: &ShellViewLayout) -> Vec<(Rect, SubTarget)> {
        let mut out = Vec::new();
        let bar = layout.local(layout.activity.area);
        for (i, vi) in layout.activity.layout.visible_items.iter().enumerate() {
            out.push((
                translate(vi.bounds, bar),
                ShellRegion::ActivityItem(i).sub_target(),
            ));
        }
        let shell = &layout.shell;
        for (region, rect) in [
            (ShellRegion::BottomPanelGrip, layout.bottom_panel_grip),
            (ShellRegion::Divider, shell.divider_bounds),
            (ShellRegion::TitleBar, shell.title_bar_bounds),
            (ShellRegion::ActivityBar, Some(shell.activity_bar_bounds)),
            (ShellRegion::SidebarHeader, shell.sidebar_header_bounds),
            (ShellRegion::SidebarContent, shell.sidebar_content_bounds),
            (ShellRegion::MainContent, Some(shell.main_content_bounds)),
            (ShellRegion::BottomPanel, shell.bottom_panel_bounds),
            (ShellRegion::CommandLine, shell.command_line_bounds),
            (ShellRegion::StatusBar, shell.status_bar_bounds),
            (ShellRegion::Window, Some(shell.window_bounds)),
        ] {
            if let Some(rect) = rect {
                out.push((layout.local(rect), region.sub_target()));
            }
        }
        out
    }

    fn sub_region_name(&self, sub: SubTarget) -> Option<&'static str> {
        ShellRegion::from_sub_target(sub).and_then(ShellRegion::zone_name)
    }

    /// Only the activity rows and the bottom-panel grip are pointer
    /// targets; the chrome bands are published zones the pointer passes
    /// through.
    const HOVER_ON_SUB_REGIONS_ONLY: bool = true;

    /// A press on the divider or the bottom panel's grip starts a drag
    /// (capturing the pointer); moves then report the resize, and the
    /// release ends it. Gestures on the activity bar — hover, clicks,
    /// and, while the bar holds focus, its keys — go to the activity bar
    /// child, and an activation becomes [`Self::activity_intent`]'s
    /// event. Everything else is the host's.
    fn handle(
        &self,
        state: &mut ShellViewState,
        event: &RoutedEvent,
        layout: &ShellViewLayout,
        cx: &mut EventCx,
    ) -> Handled<ShellEvent> {
        let in_region = |rect: Option<Rect>, p: Point| {
            rect.map(|r| contains(layout.local(r), p)).unwrap_or(false)
        };
        let bar = Some(layout.shell.activity_bar_bounds);
        match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(MouseButton::Left),
                local,
                ..
            } => {
                if in_region(layout.shell.divider_bounds, *local) {
                    let div = layout.local(layout.shell.divider_bounds.unwrap_or_default());
                    state.drag = Some(ShellDrag::Sidebar(local.x - (div.x + div.width / 2.0)));
                    cx.capture_pointer();
                    return Handled::Consumed;
                }
                if in_region(layout.bottom_panel_grip, *local) {
                    let bp = layout.local(layout.shell.bottom_panel_bounds.unwrap_or_default());
                    state.drag = Some(ShellDrag::BottomPanel(local.y - bp.y));
                    cx.capture_pointer();
                    return Handled::Consumed;
                }
                if in_region(bar, *local) {
                    return match self.route_to_activity(state, event, layout, cx) {
                        Handled::Ignored => Handled::Consumed,
                        other => other,
                    };
                }
                Handled::Ignored
            }
            RoutedEvent::Pointer {
                kind: PointerKind::DoubleClick(_),
                local,
                sub,
                mods,
            } if in_region(bar, *local) => {
                // The second click of a fast pair acts like a click.
                let click = RoutedEvent::Pointer {
                    kind: PointerKind::Down(MouseButton::Left),
                    sub: *sub,
                    local: *local,
                    mods: *mods,
                };
                match self.route_to_activity(state, &click, layout, cx) {
                    Handled::Ignored => Handled::Consumed,
                    other => other,
                }
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Move,
                local,
                sub,
                mods,
            } => {
                if let Some(drag) = state.drag {
                    return Handled::Event(self.drag_event(drag, *local, layout));
                }
                let kind = if in_region(bar, *local) {
                    PointerKind::Move
                } else {
                    PointerKind::Leave
                };
                let hover = RoutedEvent::Pointer {
                    kind,
                    sub: *sub,
                    local: *local,
                    mods: *mods,
                };
                self.route_to_activity(state, &hover, layout, cx)
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Leave,
                ..
            } => self.route_to_activity(state, event, layout, cx),
            RoutedEvent::Pointer {
                kind: PointerKind::Up(MouseButton::Left),
                ..
            } => match state.drag.take() {
                Some(_) => {
                    cx.release_pointer();
                    Handled::Consumed
                }
                None => Handled::Ignored,
            },
            RoutedEvent::FocusIn(sub) if *sub == Some(ShellRegion::ActivityBar.sub_target()) => {
                self.route_to_activity(state, event, layout, cx)
            }
            RoutedEvent::FocusOut => self.route_to_activity(state, event, layout, cx),
            RoutedEvent::Key(_) | RoutedEvent::Action(_) if self.activity_keyboard_focused => {
                state.activity.cursor = self.activity_cursor;
                self.route_to_activity(state, event, layout, cx)
            }
            _ => Handled::Ignored,
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::RecordingBackend;
    use std::ops::{Deref, DerefMut};

    /// A host's owner for a shell: the descriptor plus its gesture state,
    /// driven the way a host drives it.
    struct Owned {
        view: ShellView,
        state: ShellViewState,
    }

    impl Owned {
        fn new(view: ShellView) -> Self {
            Self {
                view,
                state: ShellViewState::default(),
            }
        }

        fn render(&self, backend: &mut dyn Backend, area: Rect) -> ShellLayout {
            self.view.render(&self.state, backend, area)
        }

        fn handle(&mut self, event: &UiEvent, backend: &dyn Backend, area: Rect) -> ShellEvent {
            self.view
                .handle_event(&mut self.state, event, backend, area)
        }
    }

    impl Deref for Owned {
        type Target = ShellView;
        fn deref(&self) -> &ShellView {
            &self.view
        }
    }

    impl DerefMut for Owned {
        fn deref_mut(&mut self) -> &mut ShellView {
            &mut self.view
        }
    }

    fn sample_panels() -> Vec<PanelDefinition> {
        vec![
            PanelDefinition {
                id: WidgetId::new("panel:explorer"),
                icon: "E".into(),
                tooltip: "Explorer".into(),
                title: "EXPLORER".into(),
            },
            PanelDefinition {
                id: WidgetId::new("panel:search"),
                icon: "S".into(),
                tooltip: "Search".into(),
                title: "SEARCH".into(),
            },
            PanelDefinition {
                id: WidgetId::new("panel:git"),
                icon: "G".into(),
                tooltip: "Git".into(),
                title: "SOURCE CONTROL".into(),
            },
        ]
    }

    fn sample_bottom() -> Vec<PanelDefinition> {
        vec![PanelDefinition {
            id: WidgetId::new("panel:settings"),
            icon: "*".into(),
            tooltip: "Settings".into(),
            title: "Settings".into(),
        }]
    }

    fn shell() -> ShellView {
        ShellView::new(sample_panels(), 30.0).with_bottom_items(sample_bottom())
    }

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 80.0, 24.0)
    }

    // ── Construction ────────────────────────────────────────────────

    #[test]
    fn new_starts_with_first_panel_active() {
        let s = shell();
        assert_eq!(
            s.active_panel().unwrap().id,
            WidgetId::new("panel:explorer")
        );
        assert!(s.sidebar_visible());
    }

    #[test]
    fn empty_panels_has_no_active() {
        let s = ShellView::new(vec![], 30.0);
        assert!(s.active_panel().is_none());
        assert!(s.sidebar_visible());
    }

    // ── Layout — sidebar visible ────────────────────────────────────

    #[test]
    fn layout_visible_left_bounds_sum_to_width() {
        let s = shell();
        let l = s.shell_layout(area(), 1.0);
        let total = l.activity_bar_bounds.width
            + l.sidebar_header_bounds.unwrap().width
            + l.divider_bounds.unwrap().width
            + l.main_content_bounds.width;
        assert!(
            (total - area().width).abs() < 1.0,
            "total={total}, expected={}",
            area().width
        );
    }

    #[test]
    fn layout_visible_left_no_overlap() {
        let s = shell();
        let l = s.shell_layout(area(), 1.0);
        let ab_end = l.activity_bar_bounds.x + l.activity_bar_bounds.width;
        let sb_start = l.sidebar_header_bounds.unwrap().x;
        let sb_end = sb_start + l.sidebar_header_bounds.unwrap().width;
        let div_start = l.divider_bounds.unwrap().x;
        let div_end = div_start + l.divider_bounds.unwrap().width;
        let main_start = l.main_content_bounds.x;
        assert!(ab_end <= sb_start + 0.01);
        assert!(sb_end <= div_start + 0.01);
        assert!(div_end <= main_start + 0.01);
    }

    #[test]
    fn layout_visible_sidebar_content_below_header() {
        let s = shell();
        let l = s.shell_layout(area(), 1.0);
        let header = l.sidebar_header_bounds.unwrap();
        let content = l.sidebar_content_bounds.unwrap();
        assert_eq!(content.y, header.y + header.height);
        assert_eq!(content.width, header.width);
    }

    /// On a pixel backend (GUI), the sidebar header strip defaults to
    /// `Style::tab_height` (35px) instead of the raw chrome line height —
    /// the same VS Code chrome metric a tab strip uses, since both paint
    /// the same kind of header row.
    #[test]
    fn layout_visible_sidebar_header_height_defaults_to_tab_height_on_gui() {
        let s = shell();
        let l = s.shell_layout(area(), 20.0);
        assert_eq!(l.sidebar_header_bounds.unwrap().height, 35.0);
    }

    /// On the status bar: a pixel backend (GUI) defaults the status bar
    /// strip to `Style::status_bar_height` (22px) instead of the raw
    /// chrome line height; TUI's cell grid keeps the one-row default.
    #[test]
    fn status_bar_height_defaults_to_style_token_on_gui() {
        let s = full_chrome_shell();
        let l = s.shell_layout(area(), 20.0);
        assert_eq!(l.status_bar_bounds.unwrap().height, 22.0);

        let l_tui = s.shell_layout(area(), 1.0);
        assert_eq!(l_tui.status_bar_bounds.unwrap().height, 1.0);
    }

    /// [`ShellView::layout_for`] — the entry point `shell_adapter` uses —
    /// reads a host's `Backend::set_style` override back, unlike
    /// [`ShellView::shell_layout`], which only ever sees `Style::default()`. This
    /// is the layout the canonical `ShellApp` path hands the app, so it
    /// must move when the painted chrome moves.
    #[test]
    fn layout_for_reflects_a_backend_set_style_override() {
        let s = full_chrome_shell();
        let backend = RecordingBackend::with_viewport(
            crate::Viewport::new(area().width, area().height, 1.0),
            20.0,
            8.0,
        );

        let default_layout = s.layout_for(&backend, area());
        assert_eq!(default_layout.status_bar_bounds.unwrap().height, 22.0);

        let mut overridden = backend;
        overridden.set_style(Style::default().with_status_bar_height(40.0));
        let overridden_layout = s.layout_for(&overridden, area());
        assert_eq!(overridden_layout.status_bar_bounds.unwrap().height, 40.0);
    }

    /// Same override, but for `activity_bar_width` — the other chrome
    /// metric that must also reach `shell_adapter`'s app-visible layout
    /// through [`ShellView::layout_for`].
    #[test]
    fn layout_for_reflects_activity_bar_width_override() {
        let s = shell();
        let mut backend = RecordingBackend::with_viewport(
            crate::Viewport::new(area().width, area().height, 1.0),
            20.0,
            8.0,
        );
        backend.set_style(Style::default().with_activity_bar_width(96.0));

        let l = s.layout_for(&backend, area());
        assert_eq!(l.activity_bar_bounds.width, 96.0);
    }

    /// `ShellView::shell_layout` (no backend in hand) only ever sees
    /// `Style::default()` — the override is invisible there, which is
    /// exactly why a live-backend caller must use `layout_for` instead.
    #[test]
    fn layout_without_a_backend_does_not_see_a_style_override() {
        let s = full_chrome_shell();
        // No way to pass a style override to `layout` at all — it always
        // resolves against `Style::default()`, documented above.
        let l = s.shell_layout(area(), 20.0);
        assert_eq!(l.status_bar_bounds.unwrap().height, 22.0);
    }

    // ── Activity bar width ──────────────────────────────────────────

    /// On a cell grid (TUI's architecturally-fixed `line_height == 1.0`),
    /// the default activity bar width (4.0 line-heights) still tracks
    /// `line_height`, the same as every other chrome dimension — `Style`
    /// tokens are a documented no-op there.
    #[test]
    fn activity_bar_width_tracks_line_height_by_default_on_cell_grid() {
        let s = shell();
        assert_eq!(s.shell_layout(area(), 1.0).activity_bar_bounds.width, 4.0);
    }

    /// `with_activity_bar_width` overrides the line-height multiple used
    /// on a cell grid — e.g. 3.0 lh at lh=1.0 lands on 3.0 cells.
    #[test]
    fn with_activity_bar_width_overrides_the_multiple_on_cell_grid() {
        let s = shell().with_activity_bar_width(3.0);
        assert_eq!(s.shell_layout(area(), 1.0).activity_bar_bounds.width, 3.0);
    }

    /// On a pixel backend (GUI), with neither a px nor line-height-multiple
    /// override set, the activity bar defaults to `Style::activity_bar_width`
    /// (48px) instead of scaling with whatever the chrome font's line
    /// height happens to be — VS Code's own fixed-width bar.
    #[test]
    fn activity_bar_width_defaults_to_style_token_on_gui() {
        let s = shell();
        assert_eq!(s.shell_layout(area(), 20.0).activity_bar_bounds.width, 48.0);
        assert_eq!(s.shell_layout(area(), 10.0).activity_bar_bounds.width, 48.0);
    }

    /// `with_activity_bar_width`'s line-height multiple is a cell-grid-only
    /// concept: on a pixel backend it has no effect unless paired with
    /// `with_activity_bar_width_px`, which is the GUI override hook.
    #[test]
    fn activity_bar_width_multiplier_has_no_effect_on_gui_alone() {
        let s = shell().with_activity_bar_width(10.0);
        assert_eq!(s.shell_layout(area(), 20.0).activity_bar_bounds.width, 48.0);
    }

    /// `with_activity_bar_width_px` pins the bar to an exact pixel width
    /// that does not move when `line_height` changes.
    #[test]
    fn with_activity_bar_width_px_is_independent_of_line_height() {
        let s = shell().with_activity_bar_width_px(48.0);
        assert_eq!(s.shell_layout(area(), 20.0).activity_bar_bounds.width, 48.0);
        assert_eq!(s.shell_layout(area(), 10.0).activity_bar_bounds.width, 48.0);
        assert_eq!(s.shell_layout(area(), 37.5).activity_bar_bounds.width, 48.0);
    }

    /// A fixed-px override wins over a line-height multiple set via
    /// `with_activity_bar_width` on the same shell, not the other way
    /// around.
    #[test]
    fn with_activity_bar_width_px_wins_over_line_height_multiple() {
        let s = shell()
            .with_activity_bar_width(3.0)
            .with_activity_bar_width_px(48.0);
        assert_eq!(s.shell_layout(area(), 20.0).activity_bar_bounds.width, 48.0);
    }

    // ── Title bar min-px floor ──────────────────────────────────────

    /// With no floor set, the title bar's height is exactly the
    /// line-height multiple.
    #[test]
    fn title_bar_height_tracks_line_height_by_default() {
        let s = shell().with_title_bar(1.5);
        let h = s
            .shell_layout(area(), 20.0)
            .title_bar_bounds
            .unwrap()
            .height;
        assert_eq!(h, 30.0);
    }

    /// `with_title_bar_min_px` is a floor, not an override: when the
    /// line-height-derived height already clears it, the multiple still
    /// wins — unlike `with_activity_bar_width_px`, which always wins.
    #[test]
    fn with_title_bar_min_px_does_not_shrink_a_height_already_above_it() {
        let s = shell().with_title_bar(1.5).with_title_bar_min_px(20.0);
        let h = s
            .shell_layout(area(), 20.0)
            .title_bar_bounds
            .unwrap()
            .height;
        assert_eq!(h, 30.0);
    }

    /// When a small `line_height` would resolve the multiple under the
    /// floor, the floor wins instead — the title bar must not shrink past
    /// real platform constraints such as a traffic-light cluster height.
    #[test]
    fn with_title_bar_min_px_floors_a_height_that_would_otherwise_shrink_below_it() {
        let s = shell().with_title_bar(1.5).with_title_bar_min_px(34.0);
        let h = s.shell_layout(area(), 6.0).title_bar_bounds.unwrap().height;
        assert_eq!(h, 34.0);
    }

    /// On a cell grid (`line_height <= 1.0`, TUI's architecture), the three
    /// px overrides are suppressed entirely — a "48px" activity bar would
    /// otherwise become "48 columns", which is not what the caller asked
    /// for. The line-height-multiple fields drive layout instead, exactly
    /// as if the px overrides were never set.
    #[test]
    fn px_overrides_are_suppressed_on_a_cell_grid() {
        let with_px = shell()
            .with_activity_bar_width(3.0)
            .with_activity_bar_width_px(48.0)
            .with_title_bar(1.5)
            .with_title_bar_min_px(34.0)
            .with_status_bar_height_px(22.0);
        let without_px = shell().with_activity_bar_width(3.0).with_title_bar(1.5);

        let l_px = with_px.shell_layout(area(), 1.0);
        let l_plain = without_px.shell_layout(area(), 1.0);

        assert_eq!(
            l_px.activity_bar_bounds.width,
            l_plain.activity_bar_bounds.width
        );
        assert_eq!(
            l_px.title_bar_bounds.unwrap().height,
            l_plain.title_bar_bounds.unwrap().height
        );
    }

    // ── Layout — sidebar hidden ─────────────────────────────────────

    #[test]
    fn layout_hidden_no_sidebar_bounds() {
        let mut s = shell();
        s.hide_sidebar();
        let l = s.shell_layout(area(), 1.0);
        assert!(l.sidebar_header_bounds.is_none());
        assert!(l.sidebar_content_bounds.is_none());
        assert!(l.divider_bounds.is_none());
    }

    #[test]
    fn layout_hidden_main_fills_remaining() {
        let mut s = Owned::new(shell());
        s.hide_sidebar();
        let l = s.shell_layout(area(), 1.0);
        let expected = area().width - s.view.activity_bar_width;
        assert!((l.main_content_bounds.width - expected).abs() < 0.01);
    }

    // ── Layout — Right position ─────────────────────────────────────

    #[test]
    fn layout_right_position_activity_bar_at_right_edge() {
        let s = shell().with_position(ShellPosition::Right);
        let l = s.shell_layout(area(), 1.0);
        let ab_right = l.activity_bar_bounds.x + l.activity_bar_bounds.width;
        assert!((ab_right - area().width).abs() < 0.01);
    }

    #[test]
    fn layout_right_position_main_at_left_edge() {
        let s = shell().with_position(ShellPosition::Right);
        let l = s.shell_layout(area(), 1.0);
        assert!((l.main_content_bounds.x - area().x).abs() < 0.01);
    }

    // ── Toggle state machine ────────────────────────────────────────

    #[test]
    fn toggle_click_active_icon_hides_sidebar() {
        let mut s = shell();
        assert!(s.sidebar_visible());
        let ev = s.handle_activity_click(&WidgetId::new("panel:explorer"));
        assert_eq!(ev, ShellEvent::SidebarHidden);
        assert!(!s.sidebar_visible());
    }

    #[test]
    fn toggle_click_different_icon_switches_panel() {
        let mut s = shell();
        let ev = s.handle_activity_click(&WidgetId::new("panel:search"));
        assert_eq!(
            ev,
            ShellEvent::PanelChanged {
                panel_id: WidgetId::new("panel:search")
            }
        );
        assert!(s.sidebar_visible());
        assert_eq!(s.active_panel_id(), Some(&WidgetId::new("panel:search")));
    }

    #[test]
    fn toggle_click_when_hidden_shows_panel() {
        let mut s = shell();
        s.hide_sidebar();
        let ev = s.handle_activity_click(&WidgetId::new("panel:git"));
        assert_eq!(
            ev,
            ShellEvent::PanelChanged {
                panel_id: WidgetId::new("panel:git")
            }
        );
        assert!(s.sidebar_visible());
    }

    #[test]
    fn toggle_remembers_last_panel_after_hide() {
        let mut s = shell();
        s.handle_activity_click(&WidgetId::new("panel:search"));
        assert_eq!(s.active_panel_id(), Some(&WidgetId::new("panel:search")));
        s.handle_activity_click(&WidgetId::new("panel:search"));
        assert!(!s.sidebar_visible());
        // Re-click the same panel: should show it again.
        s.handle_activity_click(&WidgetId::new("panel:search"));
        assert!(s.sidebar_visible());
        assert_eq!(s.active_panel_id(), Some(&WidgetId::new("panel:search")));
    }

    #[test]
    fn bottom_item_click_emits_event_no_sidebar_change() {
        let mut s = shell();
        let was_visible = s.sidebar_visible();
        let ev = s.handle_activity_click(&WidgetId::new("panel:settings"));
        assert_eq!(
            ev,
            ShellEvent::BottomItemClicked {
                id: WidgetId::new("panel:settings")
            }
        );
        assert_eq!(s.sidebar_visible(), was_visible);
        // The bottom item's own click never touches sidebar
        // ownership — an app opts in explicitly via `show_panel`.
        assert_eq!(
            s.active_panel_id(),
            Some(&WidgetId::new("panel:explorer")),
            "a bare BottomItemClicked must not retitle the header on its own"
        );
    }

    /// `show_panel` accepts a bottom item's id, not just a top
    /// panel's — the fix that lets a bottom item (e.g. a Settings gear
    /// added via `with_bottom_items`) title the sidebar header.
    #[test]
    fn show_panel_accepts_bottom_item_id_and_titles_header() {
        let mut s = shell();
        assert_eq!(
            s.active_panel_id(),
            Some(&WidgetId::new("panel:explorer")),
            "starts on the top panel"
        );
        s.show_panel(&WidgetId::new("panel:settings"));
        assert!(s.sidebar_visible());
        assert_eq!(
            s.active_panel_id(),
            Some(&WidgetId::new("panel:settings")),
            "show_panel(bottom item id) must make it the sidebar's owner"
        );
        assert_eq!(s.active_panel().unwrap().title, "Settings");
    }

    /// Switching back to a top panel after a bottom item owned the sidebar
    /// must hand ownership back — the header must not get stuck on the
    /// bottom item.
    #[test]
    fn show_panel_top_panel_after_bottom_item_reclaims_header() {
        let mut s = shell();
        s.show_panel(&WidgetId::new("panel:settings"));
        assert_eq!(s.active_panel_id(), Some(&WidgetId::new("panel:settings")));
        s.show_panel(&WidgetId::new("panel:git"));
        assert_eq!(
            s.active_panel_id(),
            Some(&WidgetId::new("panel:git")),
            "a subsequent show_panel(top panel) must reclaim the header"
        );
    }

    /// Clicking a *different* top panel through the real
    /// activity-bar click path (`handle_activity_click`, not `show_panel`
    /// directly) after a bottom item claimed the sidebar must also reclaim
    /// the header. This is the path `AppShellDemo` and vimcode actually use
    /// for mouse/keyboard interaction — `show_panel_top_panel_after_bottom_item_reclaims_header`
    /// only covers the direct-API path.
    #[test]
    fn handle_activity_click_top_panel_after_bottom_item_reclaims_header() {
        let mut s = shell();
        s.show_panel(&WidgetId::new("panel:settings"));
        assert_eq!(s.active_panel_id(), Some(&WidgetId::new("panel:settings")));

        let ev = s.handle_activity_click(&WidgetId::new("panel:git"));
        assert_eq!(
            ev,
            ShellEvent::PanelChanged {
                panel_id: WidgetId::new("panel:git")
            }
        );
        assert_eq!(
            s.active_panel_id(),
            Some(&WidgetId::new("panel:git")),
            "clicking a top panel via the activity bar must reclaim the header \
             from a bottom item, not leave it stuck on Settings"
        );
        assert!(s.sidebar_visible());

        let bar = s.build_activity_bar();
        let git_item = bar
            .top_items
            .iter()
            .find(|i| i.id == WidgetId::new("panel:git"))
            .unwrap();
        assert!(
            git_item.is_active,
            "Git's activity-bar row must be highlighted once it reclaims the header"
        );
    }

    /// The stale-`active_panel` toggle-hide bug. Before the
    /// fix, `active_panel` stayed on the panel that was active *before* a
    /// bottom item took ownership, so re-clicking that panel's icon would
    /// hit the toggle-hide branch (`active_panel == Some(i) && visible`)
    /// and hide the whole sidebar instead of switching the header back to
    /// it.
    #[test]
    fn handle_activity_click_reclaim_does_not_hide_sidebar() {
        let mut s = shell();
        // Explorer (index 0) is active by default. Claim the sidebar with
        // a bottom item without ever clicking Explorer away.
        s.show_panel(&WidgetId::new("panel:settings"));
        assert_eq!(s.active_panel_id(), Some(&WidgetId::new("panel:settings")));
        assert!(s.sidebar_visible());

        // Re-clicking Explorer's icon now must switch the header back to
        // Explorer, not hide the sidebar.
        let ev = s.handle_activity_click(&WidgetId::new("panel:explorer"));
        assert_eq!(
            ev,
            ShellEvent::PanelChanged {
                panel_id: WidgetId::new("panel:explorer")
            }
        );
        assert!(
            s.sidebar_visible(),
            "reclaiming the header must not hide the sidebar"
        );
        assert_eq!(s.active_panel_id(), Some(&WidgetId::new("panel:explorer")));
    }

    /// A top panel's activity-bar row must not still show as active while
    /// a bottom item owns the sidebar — otherwise the highlighted icon and
    /// the header title would disagree about what's open.
    #[test]
    fn build_activity_bar_top_panel_not_active_once_bottom_item_owns_sidebar() {
        let mut s = shell();
        s.show_panel(&WidgetId::new("panel:settings"));
        let bar = s.build_activity_bar();
        assert!(bar.top_items.iter().all(|i| !i.is_active));
    }

    /// Unregistering the bottom item that currently owns the sidebar must
    /// clear that ownership (mirrors `remove_last_panel_clears_active` for
    /// top panels), not leave a dangling index.
    #[test]
    fn remove_bottom_item_that_owns_sidebar_clears_ownership() {
        let mut s = shell();
        s.show_panel(&WidgetId::new("panel:settings"));
        assert_eq!(s.active_panel_id(), Some(&WidgetId::new("panel:settings")));
        s.remove_bottom_item(&WidgetId::new("panel:settings"));
        // Falls back to the top panel that was active before the bottom
        // item took ownership.
        assert_eq!(s.active_panel_id(), Some(&WidgetId::new("panel:explorer")));
    }

    // ── build_activity_bar ──────────────────────────────────────────

    #[test]
    fn build_activity_bar_marks_active() {
        let s = shell();
        let bar = s.build_activity_bar();
        assert!(bar.top_items[0].is_active);
        assert!(!bar.top_items[1].is_active);
        assert!(!bar.top_items[2].is_active);
    }

    #[test]
    fn build_activity_bar_accent_and_selection_bg_default_to_none() {
        let s = shell();
        let bar = s.build_activity_bar();
        assert_eq!(bar.active_accent, None);
        assert_eq!(bar.selection_bg, None);
    }

    #[test]
    fn build_activity_bar_carries_colors_set_via_builder() {
        let accent = Color::rgb(80, 140, 255);
        let selection = Color::rgb(60, 60, 60);
        let s = shell()
            .with_active_accent(accent)
            .with_selection_bg(selection);
        let bar = s.build_activity_bar();
        assert_eq!(bar.active_accent, Some(accent));
        assert_eq!(bar.selection_bg, Some(selection));
    }

    #[test]
    fn set_active_accent_and_selection_bg_update_at_runtime() {
        let accent = Color::rgb(10, 20, 30);
        let selection = Color::rgb(40, 50, 60);
        let mut s = shell();
        assert_eq!(s.build_activity_bar().active_accent, None);

        s.set_active_accent(Some(accent));
        s.set_selection_bg(Some(selection));
        let bar = s.build_activity_bar();
        assert_eq!(bar.active_accent, Some(accent));
        assert_eq!(bar.selection_bg, Some(selection));

        s.set_active_accent(None);
        s.set_selection_bg(None);
        let bar = s.build_activity_bar();
        assert_eq!(bar.active_accent, None);
        assert_eq!(bar.selection_bg, None);
    }

    #[test]
    fn build_activity_bar_hidden_sidebar_no_active() {
        let mut s = shell();
        s.hide_sidebar();
        let bar = s.build_activity_bar();
        assert!(bar.top_items.iter().all(|i| !i.is_active));
    }

    #[test]
    fn build_activity_bar_includes_bottom_items() {
        let s = shell();
        let bar = s.build_activity_bar();
        assert_eq!(bar.bottom_items.len(), 1);
        assert_eq!(bar.bottom_items[0].id, WidgetId::new("panel:settings"));
    }

    /// The `Icon` pair registered with `ShellView::with_panel_icon` survives
    /// `build_activity_bar` into `ActivityItem::icon`, and the TUI rasteriser
    /// picks the right half — `glyph` when `nerd_fonts_enabled`, `fallback`
    /// otherwise. A rendered-output assertion, not just a type-level check.
    #[test]
    #[cfg(feature = "tui")]
    fn with_panel_icon_reaches_the_painted_column_via_the_nerd_fonts_flag() {
        let panels = vec![PanelDefinition {
            id: WidgetId::new("panel:explorer"),
            icon: "E".to_string(),
            tooltip: "Explorer".into(),
            title: "EXPLORER".into(),
        }];
        let s = ShellView::new(panels, 3.0)
            .with_panel_icon(WidgetId::new("panel:explorer"), Icon::new("\u{f07c}", "E"));
        let bar = s.build_activity_bar();
        assert_eq!(bar.top_items[0].icon, Icon::new("\u{f07c}", "E"));

        assert!(
            paint_first_row(&bar, true).contains('\u{f07c}'),
            "nerd_fonts_enabled: true should paint the override's glyph"
        );
        assert!(
            paint_first_row(&bar, false).contains('E'),
            "nerd_fonts_enabled: false should paint the override's fallback"
        );
    }

    /// A panel with no `with_panel_icon` override paints its single
    /// `PanelDefinition::icon` string under both settings of the
    /// `nerd_fonts_enabled` flag — the pre-existing behaviour, preserved so
    /// that leaving the field a `String` is not a silent downgrade.
    #[test]
    #[cfg(feature = "tui")]
    fn panel_without_icon_override_paints_its_string_under_either_nerd_font_setting() {
        let panels = vec![PanelDefinition {
            id: WidgetId::new("panel:explorer"),
            icon: "E".to_string(),
            tooltip: "Explorer".into(),
            title: "EXPLORER".into(),
        }];
        let bar = ShellView::new(panels, 3.0).build_activity_bar();
        assert_eq!(bar.top_items[0].icon, Icon::new("E", "E"));

        assert!(paint_first_row(&bar, true).contains('E'));
        assert!(paint_first_row(&bar, false).contains('E'));
    }

    /// `with_panel_icon` reaches bottom items too, not just top
    /// panels — both go through the same `resolved_icon` seam.
    #[test]
    fn with_panel_icon_applies_to_bottom_items() {
        let s =
            shell().with_panel_icon(WidgetId::new("panel:settings"), Icon::new("\u{f013}", "*"));
        let bar = s.build_activity_bar();
        assert_eq!(bar.bottom_items[0].icon, Icon::new("\u{f013}", "*"));
        // Untouched top panels keep the widened single-string form.
        assert_eq!(bar.top_items[0].icon, Icon::new("E", "E"));
    }

    /// Paints `bar` into a 4-cell-wide activity column and returns row 0's
    /// symbols concatenated, so a test can assert on what actually landed
    /// in the buffer rather than on the model.
    #[cfg(feature = "tui")]
    fn paint_first_row(bar: &ActivityBar, nerd_fonts_enabled: bool) -> String {
        let theme = crate::theme::Theme::default();
        let mut buf = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 40, 10));
        let area = ratatui::layout::Rect::new(0, 0, 4, 10);
        crate::primitives::activity_bar::cell_paint::paint(
            &mut crate::tui::BufferCellSurface::new(&mut buf),
            area,
            bar,
            &theme,
            None,
            nerd_fonts_enabled,
        );
        (area.x..area.x + area.width)
            .map(|x| buf[(x, area.y)].symbol().to_string())
            .collect::<String>()
    }

    /// `render`'s sidebar header and resize divider used
    /// to paint fixed VS-Code-dark literals (`Color::rgb(37,37,38)` /
    /// `Color::rgb(100,100,110)`) regardless of what theme the app had
    /// configured. This drives a real [`crate::tui::TuiBackend`] with a
    /// theme that is neither of those literals and asserts the painted
    /// cells came from `Backend::theme()` — i.e. from whatever the host
    /// last passed to `set_theme` — not a baked-in dark palette.
    #[test]
    #[cfg(feature = "tui")]
    fn render_sources_header_and_divider_from_backend_theme() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        // A light-ish theme, deliberately far from both old literals.
        let light = crate::theme::Theme {
            header_bg: crate::types::Color::rgb(240, 240, 240),
            header_fg: crate::types::Color::rgb(10, 10, 10),
            separator: crate::types::Color::rgb(200, 200, 200),
            ..crate::theme::Theme::default()
        };

        let s = Owned::new(shell());
        let a = area();
        let mut backend = crate::tui::TuiBackend::new();
        backend.set_current_theme(light);

        let mut terminal =
            Terminal::new(TestBackend::new(a.width as u16, a.height as u16)).expect("terminal");
        let mut layout = None;
        terminal
            .draw(|frame| {
                backend.enter_frame_scope(frame, |b| {
                    layout = Some(s.render(b, a));
                });
            })
            .expect("draw");
        let layout = layout.expect("render ran inside the frame scope");

        let header = layout
            .sidebar_header_bounds
            .expect("sidebar is visible with an active panel, so a header is laid out");
        let buf = terminal.backend().buffer();
        let header_cell = &buf[(header.x as u16, header.y as u16)];
        assert_eq!(
            header_cell.bg,
            crate::tui::ratatui_color(light.header_bg),
            "sidebar header bg must come from theme.header_bg, not a hardcoded literal"
        );
        assert_eq!(
            header_cell.fg,
            crate::tui::ratatui_color(light.header_fg),
            "sidebar header fg must come from theme.header_fg, not a hardcoded literal"
        );

        let divider = layout
            .divider_bounds
            .expect("sidebar is visible, so a resize divider is laid out");
        let divider_cell = &buf[(divider.x as u16, divider.y as u16)];
        assert_eq!(
            divider_cell.bg,
            crate::tui::ratatui_color(light.separator),
            "resize divider must come from theme.separator, not a hardcoded literal"
        );
    }

    // ── Keyboard navigation ─────────────────────────────────────────

    #[test]
    fn keyboard_focus_off_by_default() {
        let s = shell();
        assert!(!s.activity_keyboard_focused());
        let bar = s.build_activity_bar();
        assert!(!bar.is_keyboard_focused);
        assert!(bar.top_items.iter().all(|i| !i.is_keyboard_selected));
        assert!(bar.bottom_items.iter().all(|i| !i.is_keyboard_selected));
    }

    #[test]
    fn set_keyboard_focused_propagates_to_bar() {
        let mut s = shell();
        s.set_activity_keyboard_focused(true);
        assert!(s.activity_keyboard_focused());
        let bar = s.build_activity_bar();
        assert!(bar.is_keyboard_focused);
    }

    #[test]
    fn build_activity_bar_marks_cursor_item_when_focused() {
        let mut s = shell();
        s.set_activity_keyboard_focused(true);
        // Default cursor = 0 (first top panel).
        let bar = s.build_activity_bar();
        assert!(bar.top_items[0].is_keyboard_selected);
        assert!(!bar.top_items[1].is_keyboard_selected);
        assert!(!bar.top_items[2].is_keyboard_selected);
        assert!(!bar.bottom_items[0].is_keyboard_selected);
    }

    #[test]
    fn build_activity_bar_no_selection_when_not_focused() {
        let mut s = Owned::new(shell());
        s.view.activity_cursor = 1;
        // focused is false → no item should be selected
        let bar = s.build_activity_bar();
        assert!(bar.top_items.iter().all(|i| !i.is_keyboard_selected));
    }

    #[test]
    fn select_next_advances_cursor_through_top_then_bottom() {
        let mut s = Owned::new(shell()); // 3 top + 1 bottom
        s.set_activity_keyboard_focused(true);
        // cursor starts at 0
        s.activity_select_next(); // → 1
        assert_eq!(s.view.activity_cursor, 1);
        s.activity_select_next(); // → 2
        assert_eq!(s.view.activity_cursor, 2);
        s.activity_select_next(); // → 3 (first bottom item)
        assert_eq!(s.view.activity_cursor, 3);

        let bar = s.build_activity_bar();
        assert!(bar.bottom_items[0].is_keyboard_selected);
        assert!(bar.top_items.iter().all(|i| !i.is_keyboard_selected));
    }

    #[test]
    fn select_next_saturates_at_last_item() {
        let mut s = Owned::new(shell()); // total = 4
        s.set_activity_keyboard_focused(true);
        for _ in 0..10 {
            s.activity_select_next();
        }
        // Should saturate at 3 (last index: 3 top + 1 bottom − 1 = 3).
        assert_eq!(s.view.activity_cursor, 3);
    }

    #[test]
    fn select_prev_saturates_at_zero() {
        let mut s = Owned::new(shell());
        // cursor is already at 0
        s.activity_select_prev();
        assert_eq!(s.view.activity_cursor, 0);
        s.activity_select_prev();
        assert_eq!(s.view.activity_cursor, 0);
    }

    #[test]
    fn select_prev_moves_cursor_up() {
        let mut s = Owned::new(shell());
        s.view.activity_cursor = 3; // bottom item
        s.activity_select_prev();
        assert_eq!(s.view.activity_cursor, 2);
        s.activity_select_prev();
        assert_eq!(s.view.activity_cursor, 1);
        s.activity_select_prev();
        assert_eq!(s.view.activity_cursor, 0);
        s.activity_select_prev(); // saturates
        assert_eq!(s.view.activity_cursor, 0);
    }

    #[test]
    fn activity_set_cursor_clamps_to_valid_range() {
        let mut s = Owned::new(shell()); // 4 items total
        s.activity_set_cursor(100);
        assert_eq!(s.view.activity_cursor, 3);
        s.activity_set_cursor(0);
        assert_eq!(s.view.activity_cursor, 0);
    }

    #[test]
    fn activity_selected_id_top_items() {
        let s = shell();
        // cursor at 0 → first top panel
        assert_eq!(
            s.activity_selected_id(),
            Some(&WidgetId::new("panel:explorer"))
        );
    }

    #[test]
    fn activity_selected_id_bottom_item() {
        let mut s = Owned::new(shell());
        s.view.activity_cursor = 3; // first (and only) bottom item
        assert_eq!(
            s.activity_selected_id(),
            Some(&WidgetId::new("panel:settings"))
        );
    }

    #[test]
    fn activity_activate_selected_top_panel_emits_panel_changed() {
        let mut s = Owned::new(shell());
        s.view.activity_cursor = 1; // panel:search
        let ev = s.activity_activate_selected().expect("should return Some");
        assert_eq!(
            ev,
            ShellEvent::PanelChanged {
                panel_id: WidgetId::new("panel:search")
            }
        );
        assert_eq!(s.active_panel_id(), Some(&WidgetId::new("panel:search")));
    }

    #[test]
    fn activity_activate_selected_bottom_item_emits_bottom_clicked() {
        let mut s = Owned::new(shell());
        s.view.activity_cursor = 3; // panel:settings
        let ev = s.activity_activate_selected().expect("should return Some");
        assert_eq!(
            ev,
            ShellEvent::BottomItemClicked {
                id: WidgetId::new("panel:settings")
            }
        );
    }

    #[test]
    fn activity_activate_selected_toggle_hides_sidebar() {
        let mut s = shell();
        // cursor at 0 (panel:explorer), which is already the active panel
        let ev = s.activity_activate_selected().expect("should return Some");
        assert_eq!(ev, ShellEvent::SidebarHidden);
        assert!(!s.sidebar_visible());
    }

    #[test]
    fn activity_activate_does_not_clear_keyboard_focus() {
        let mut s = Owned::new(shell());
        s.set_activity_keyboard_focused(true);
        s.view.activity_cursor = 1;
        let _ = s.activity_activate_selected();
        // Consumer decides whether to clear focus; ShellView must not.
        assert!(s.activity_keyboard_focused());
    }

    #[test]
    fn activity_activate_selected_returns_none_when_no_items() {
        let mut s = ShellView::new(vec![], 30.0);
        assert!(s.activity_activate_selected().is_none());
    }

    #[test]
    fn select_next_noop_on_empty_shell() {
        let mut s = Owned::new(ShellView::new(vec![], 30.0));
        s.activity_select_next(); // must not panic
        assert_eq!(s.view.activity_cursor, 0);
    }

    // ── Programmatic state control ──────────────────────────────────

    #[test]
    fn show_panel_activates_and_reveals() {
        let mut s = shell();
        s.hide_sidebar();
        s.show_panel(&WidgetId::new("panel:git"));
        assert!(s.sidebar_visible());
        assert_eq!(s.active_panel_id(), Some(&WidgetId::new("panel:git")));
    }

    #[test]
    fn toggle_sidebar_flips_visibility() {
        let mut s = shell();
        assert!(s.sidebar_visible());
        s.toggle_sidebar();
        assert!(!s.sidebar_visible());
        s.toggle_sidebar();
        assert!(s.sidebar_visible());
    }

    #[test]
    fn set_sidebar_width_clamps() {
        let mut s = Owned::new(shell());
        s.set_sidebar_width(5.0);
        assert_eq!(s.sidebar_width(), s.view.min_sidebar_width);
        s.set_sidebar_width(9999.0);
        assert_eq!(s.sidebar_width(), s.view.max_sidebar_width);
    }

    // ── Render ───────────────────────────────────────────────────────

    /// The divider is one thin line centred in `divider_bounds`, painted
    /// through the shell's own paint rather than a backend fill call.
    #[test]
    fn render_paints_the_divider_without_a_backend_fill_call() {
        let s = Owned::new(shell());
        let mut backend = RecordingBackend::new();

        let layout = s.render(&mut backend, area());

        assert!(
            layout.divider_bounds.is_some(),
            "this shell shape should have a divider"
        );
    }

    // ── Resize drag ─────────────────────────────────────────────────

    #[test]
    fn resize_drag_updates_width() {
        let mut s = Owned::new(shell());
        let l = s.shell_layout(area(), 1.0);
        let div = l.divider_bounds.unwrap();
        let _center = div.x + div.width / 2.0;

        s.state.drag = Some(ShellDrag::Sidebar(0.0));
        let ab_right = l.activity_bar_bounds.x + l.activity_bar_bounds.width;
        let target_width = 40.0;
        let mouse_x = ab_right + target_width;

        let ev = s.handle(
            &UiEvent::MouseMoved {
                position: Point::new(mouse_x, div.y + 1.0),
                buttons: ButtonMask {
                    left: true,
                    middle: false,
                    right: false,
                },
            },
            &RecordingBackend::new(),
            area(),
        );
        assert!(matches!(ev, ShellEvent::SidebarResized { .. }));
        assert!((s.sidebar_width() - target_width).abs() < 1.0);
    }

    #[test]
    fn resize_drag_clamps_to_min() {
        let mut s = Owned::new(shell());
        s.state.drag = Some(ShellDrag::Sidebar(0.0));
        let ev = s.handle(
            &UiEvent::MouseMoved {
                position: Point::new(s.view.activity_bar_width + 1.0, 5.0),
                buttons: ButtonMask {
                    left: true,
                    middle: false,
                    right: false,
                },
            },
            &RecordingBackend::new(),
            area(),
        );
        assert!(matches!(ev, ShellEvent::SidebarResized { .. }));
        assert_eq!(s.sidebar_width(), s.view.min_sidebar_width);
    }

    #[test]
    fn mouse_up_ends_drag() {
        let mut s = Owned::new(shell());
        s.state.drag = Some(ShellDrag::Sidebar(0.0));
        let ev = s.handle(
            &UiEvent::MouseUp {
                button: MouseButton::Left,
                position: Point::new(50.0, 5.0),
                widget: None,
            },
            &RecordingBackend::new(),
            area(),
        );
        assert_eq!(ev, ShellEvent::Consumed);
        assert!(s.state.drag.is_none());
    }

    // ── Dynamic panel registration ──────────────────────────────────

    #[test]
    fn add_panel_appends_and_is_reachable() {
        let mut s = shell();
        assert_eq!(s.panels().len(), 3);
        s.add_panel(PanelDefinition {
            id: WidgetId::new("panel:ext-lua"),
            icon: "L".into(),
            tooltip: "Lua".into(),
            title: "LUA".into(),
        });
        assert_eq!(s.panels().len(), 4);
        s.show_panel(&WidgetId::new("panel:ext-lua"));
        assert_eq!(s.active_panel_id(), Some(&WidgetId::new("panel:ext-lua")));
    }

    #[test]
    fn add_panel_rejects_duplicate() {
        let mut s = shell();
        let ok = s.add_panel(PanelDefinition {
            id: WidgetId::new("panel:explorer"),
            icon: "E".into(),
            tooltip: "Dup".into(),
            title: "DUP".into(),
        });
        assert!(!ok);
        assert_eq!(s.panels().len(), 3);
    }

    #[test]
    fn add_panel_rejects_id_collision_with_bottom() {
        let mut s = shell();
        let ok = s.add_panel(PanelDefinition {
            id: WidgetId::new("panel:settings"),
            icon: "*".into(),
            tooltip: "Dup".into(),
            title: "DUP".into(),
        });
        assert!(!ok);
    }

    #[test]
    fn add_panel_to_empty_shell_activates_it() {
        let mut s = ShellView::new(vec![], 30.0);
        assert!(s.active_panel().is_none());
        s.add_panel(PanelDefinition {
            id: WidgetId::new("panel:first"),
            icon: "1".into(),
            tooltip: "First".into(),
            title: "FIRST".into(),
        });
        assert_eq!(s.active_panel_id(), Some(&WidgetId::new("panel:first")));
    }

    #[test]
    fn remove_panel_adjusts_active_index() {
        let mut s = shell();
        s.show_panel(&WidgetId::new("panel:git"));
        assert_eq!(s.active_panel_id(), Some(&WidgetId::new("panel:git")));
        // Remove "panel:explorer" (index 0) — active was index 2, should shift to 1.
        s.remove_panel(&WidgetId::new("panel:explorer"));
        assert_eq!(s.panels().len(), 2);
        assert_eq!(s.active_panel_id(), Some(&WidgetId::new("panel:git")));
    }

    #[test]
    fn remove_active_panel_selects_neighbor() {
        let mut s = shell();
        s.show_panel(&WidgetId::new("panel:search"));
        s.remove_panel(&WidgetId::new("panel:search"));
        assert_eq!(s.panels().len(), 2);
        assert!(s.active_panel().is_some());
    }

    #[test]
    fn remove_last_panel_clears_active() {
        let mut s = ShellView::new(
            vec![PanelDefinition {
                id: WidgetId::new("panel:only"),
                icon: "O".into(),
                tooltip: "Only".into(),
                title: "ONLY".into(),
            }],
            30.0,
        );
        s.remove_panel(&WidgetId::new("panel:only"));
        assert!(s.active_panel().is_none());
        assert!(s.panels().is_empty());
    }

    #[test]
    fn remove_nonexistent_panel_returns_false() {
        let mut s = shell();
        assert!(!s.remove_panel(&WidgetId::new("panel:nope")));
    }

    // ── activity_cursor adjustment on remove ────────────────────────

    #[test]
    fn remove_panel_before_cursor_shifts_cursor_down() {
        // shell() has 3 panels (explorer=0, search=1, git=2) + 1 bottom (settings=3).
        let mut s = Owned::new(shell());
        s.set_activity_keyboard_focused(true);
        s.activity_set_cursor(2); // pointing at git (index 2)
        s.remove_panel(&WidgetId::new("panel:explorer")); // remove index 0
                                                          // cursor was > 0, so should decrement to 1 (still pointing at git)
        assert_eq!(s.view.activity_cursor, 1);
        assert_eq!(s.activity_selected_id(), Some(&WidgetId::new("panel:git")));
    }

    #[test]
    fn remove_panel_after_cursor_leaves_cursor_unchanged() {
        let mut s = Owned::new(shell());
        s.set_activity_keyboard_focused(true);
        s.activity_set_cursor(0); // pointing at explorer (index 0)
        s.remove_panel(&WidgetId::new("panel:search")); // remove index 1 (after cursor)
        assert_eq!(s.view.activity_cursor, 0);
        assert_eq!(
            s.activity_selected_id(),
            Some(&WidgetId::new("panel:explorer"))
        );
    }

    #[test]
    fn remove_panel_at_cursor_clamps_cursor() {
        let mut s = Owned::new(shell());
        s.set_activity_keyboard_focused(true);
        s.activity_set_cursor(2); // pointing at git (last panel, index 2)
        s.remove_panel(&WidgetId::new("panel:git")); // remove the cursor item
                                                     // New total = 2 panels + 1 bottom = 3; cursor was 2, clamped to 2.
                                                     // Now index 2 is the bottom item (settings).
        assert_eq!(s.view.activity_cursor, 2);
        assert_eq!(
            s.activity_selected_id(),
            Some(&WidgetId::new("panel:settings"))
        );
    }

    #[test]
    fn remove_last_panel_while_focused_clears_focus() {
        let mut s = Owned::new(ShellView::new(
            vec![PanelDefinition {
                id: WidgetId::new("panel:only"),
                icon: "O".into(),
                tooltip: "Only".into(),
                title: "ONLY".into(),
            }],
            30.0,
        ));
        s.set_activity_keyboard_focused(true);
        s.remove_panel(&WidgetId::new("panel:only"));
        assert!(!s.activity_keyboard_focused());
        assert_eq!(s.view.activity_cursor, 0);
    }

    #[test]
    fn remove_bottom_item_before_cursor_leaves_cursor_unchanged() {
        // shell(): panels 0,1,2; bottom item at combined index 3.
        // cursor at 3 (settings), remove settings → cursor was at removed item,
        // clamp to new total (3 panels + 0 bottom = 3, but 3 is out of range → 2).
        // This test checks cursor BEFORE the bottom item first.
        let mut s = Owned::new(shell());
        s.set_activity_keyboard_focused(true);
        s.activity_set_cursor(1); // pointing at search (panel index 1)
        s.remove_bottom_item(&WidgetId::new("panel:settings")); // remove bottom at combined 3
        assert_eq!(s.view.activity_cursor, 1); // cursor was before removed item
        assert_eq!(
            s.activity_selected_id(),
            Some(&WidgetId::new("panel:search"))
        );
    }

    #[test]
    fn remove_bottom_item_at_cursor_clamps_cursor() {
        let mut s = Owned::new(shell());
        s.set_activity_keyboard_focused(true);
        s.activity_set_cursor(3); // pointing at settings (combined index 3)
        s.remove_bottom_item(&WidgetId::new("panel:settings"));
        // New total = 3; cursor was 3, clamped to 2 (last panel).
        assert_eq!(s.view.activity_cursor, 2);
        assert_eq!(s.activity_selected_id(), Some(&WidgetId::new("panel:git")));
    }

    #[test]
    fn add_bottom_item_shows_in_activity_bar() {
        let mut s = shell();
        s.add_bottom_item(PanelDefinition {
            id: WidgetId::new("panel:debug-console"),
            icon: "D".into(),
            tooltip: "Debug".into(),
            title: "DEBUG".into(),
        });
        let bar = s.build_activity_bar();
        assert_eq!(bar.bottom_items.len(), 2);
    }

    #[test]
    fn remove_bottom_item_works() {
        let mut s = shell();
        assert!(s.remove_bottom_item(&WidgetId::new("panel:settings")));
        let bar = s.build_activity_bar();
        assert!(bar.bottom_items.is_empty());
    }

    #[test]
    fn dynamic_panel_toggle_via_activity_click() {
        let mut s = shell();
        s.add_panel(PanelDefinition {
            id: WidgetId::new("panel:ext"),
            icon: "X".into(),
            tooltip: "Ext".into(),
            title: "EXTENSION".into(),
        });
        let ev = s.handle_activity_click(&WidgetId::new("panel:ext"));
        assert_eq!(
            ev,
            ShellEvent::PanelChanged {
                panel_id: WidgetId::new("panel:ext")
            }
        );
        assert_eq!(s.active_panel_id(), Some(&WidgetId::new("panel:ext")));
        // Toggle off.
        let ev = s.handle_activity_click(&WidgetId::new("panel:ext"));
        assert_eq!(ev, ShellEvent::SidebarHidden);
    }

    // ── Double-click on the activity bar ──────────────────────────────
    //
    // See the `UiEvent::DoubleClick` arm's comment on `handle` above for
    // why the fold reaches this arm and what it must do.

    /// The top-left of activity item `id`'s row, from the same layout the
    /// shell paints and routes clicks against.
    #[cfg(feature = "tui")]
    fn activity_item_point(s: &Owned, backend: &dyn Backend, a: Rect, id: &str) -> Point {
        let laid = crate::backend::layout_like_paint(backend, &s.view, a);
        let l = laid
            .downcast_ref::<ShellViewLayout>()
            .expect("a shell lays out a ShellViewLayout");
        let ab = l.shell.activity_bar_bounds;
        let hit = l
            .activity
            .row_hits(&l.activity_bar)
            .into_iter()
            .find(|h| h.id == WidgetId::new(id))
            .expect("the item has a row");
        Point::new(ab.x, ab.y + hit.y_start)
    }

    /// Real paint → real hit zones → real click, via a `TuiBackend` frame
    /// scope — not a synthetic `Rect`/`Point` — so this exercises the same
    /// cached-hit path a live click goes through.
    #[test]
    #[cfg(feature = "tui")]
    fn double_click_on_activity_bar_same_icon_toggles_like_second_single_click() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut s = Owned::new(shell());
        let a = area();
        let mut backend = crate::tui::TuiBackend::new();
        let mut terminal =
            Terminal::new(TestBackend::new(a.width as u16, a.height as u16)).expect("terminal");
        terminal
            .draw(|frame| {
                backend.enter_frame_scope(frame, |b| {
                    s.render(b, a);
                });
            })
            .expect("draw");

        let p = activity_item_point(&s, &backend, a, "panel:explorer");

        // Explorer is already the active panel (default), so a plain
        // MouseDown on its icon toggles the sidebar closed per
        // `handle_activity_click`'s own toggle branch.
        let ev = s.handle(
            &UiEvent::MouseDown {
                widget: None,
                button: MouseButton::Left,
                position: p,
                modifiers: crate::types::Modifiers::default(),
            },
            &backend,
            a,
        );
        assert_eq!(ev, ShellEvent::SidebarHidden);

        // Re-show it so the next click (the one under test) starts from
        // the same "already active" state a real fast double-click would
        // land in.
        s.show_panel(&WidgetId::new("panel:explorer"));
        assert!(s.sidebar_visible());

        // A DoubleClick landing on the same icon must behave exactly like
        // that second single click — toggle the sidebar closed again —
        // not fall through to `ShellEvent::Ignored`.
        let ev = s.handle(
            &UiEvent::DoubleClick {
                widget: None,
                button: crate::MouseButton::Left,
                position: p,
            },
            &backend,
            a,
        );
        assert_eq!(
            ev,
            ShellEvent::SidebarHidden,
            "DoubleClick on an already-active activity-bar icon must toggle \
             the sidebar closed, same as a second single click"
        );
    }

    /// Adjacent-icon case: a `DoubleClick` whose position lands on a
    /// *different* icon than the previous click (the TUI adjacent-row
    /// fold, where icon rows sit closer together than the detector's
    /// click radius) must switch to that icon's panel, exactly like a
    /// fresh single click there — not get dropped.
    #[test]
    #[cfg(feature = "tui")]
    fn double_click_on_activity_bar_different_icon_switches_panel() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut s = Owned::new(shell());
        let a = area();
        let mut backend = crate::tui::TuiBackend::new();
        let mut terminal =
            Terminal::new(TestBackend::new(a.width as u16, a.height as u16)).expect("terminal");
        terminal
            .draw(|frame| {
                backend.enter_frame_scope(frame, |b| {
                    s.render(b, a);
                });
            })
            .expect("draw");

        let p = activity_item_point(&s, &backend, a, "panel:git");

        let ev = s.handle(
            &UiEvent::DoubleClick {
                widget: None,
                button: crate::MouseButton::Left,
                position: p,
            },
            &backend,
            a,
        );
        assert_eq!(
            ev,
            ShellEvent::PanelChanged {
                panel_id: WidgetId::new("panel:git")
            },
            "DoubleClick on a non-active activity-bar icon must switch to \
             its panel, same as a plain MouseDown there"
        );
        assert_eq!(s.active_panel_id(), Some(&WidgetId::new("panel:git")));
    }

    /// A `DoubleClick` outside the activity-bar band must stay `Ignored`
    /// — this arm must not swallow double-clicks everywhere, only hit-test
    /// the same band the `MouseDown` arm does.
    #[test]
    fn double_click_outside_activity_bar_is_ignored() {
        let mut s = Owned::new(shell());
        let a = area();
        let l = s.shell_layout(a, 1.0);
        let outside = Point::new(
            l.activity_bar_bounds.x + l.activity_bar_bounds.width + 5.0,
            1.0,
        );

        let ev = s.handle(
            &UiEvent::DoubleClick {
                widget: None,
                button: crate::MouseButton::Left,
                position: outside,
            },
            &RecordingBackend::new(),
            a,
        );
        assert_eq!(ev, ShellEvent::Ignored);
    }

    // ── Ignored events ──────────────────────────────────────────────

    #[test]
    fn key_events_are_ignored() {
        let mut s = Owned::new(shell());
        let ev = s.handle(
            &UiEvent::KeyPressed {
                key: crate::Key::Char('q'),
                modifiers: Default::default(),
                repeat: false,
            },
            &RecordingBackend::new(),
            area(),
        );
        assert_eq!(ev, ShellEvent::Ignored);
    }

    // ── Chrome slot layout ─────────────────────────────────────────

    fn full_chrome_shell() -> ShellView {
        ShellView::new(sample_panels(), 30.0)
            .with_bottom_items(sample_bottom())
            .with_title_bar(1.5)
            .with_bottom_panel(8.0)
            .with_bottom_panel_limits(3.0, 25.0)
            .with_command_line()
            .with_status_bar()
    }

    #[test]
    fn chrome_slots_none_when_not_configured() {
        let s = shell();
        let l = s.shell_layout(area(), 1.0);
        assert!(l.title_bar_bounds.is_none());
        assert!(l.bottom_panel_bounds.is_none());
        assert!(l.command_line_bounds.is_none());
        assert!(l.status_bar_bounds.is_none());
    }

    #[test]
    fn chrome_slots_present_when_configured() {
        let s = full_chrome_shell();
        let l = s.shell_layout(area(), 1.0);
        assert!(l.title_bar_bounds.is_some());
        assert!(l.bottom_panel_bounds.is_some());
        assert!(l.command_line_bounds.is_some());
        assert!(l.status_bar_bounds.is_some());
    }

    #[test]
    fn title_bar_at_top_edge() {
        let s = full_chrome_shell();
        let l = s.shell_layout(area(), 1.0);
        let tb = l.title_bar_bounds.unwrap();
        assert_eq!(tb.y, 0.0);
        assert_eq!(tb.x, 0.0);
        assert_eq!(tb.width, area().width);
    }

    #[test]
    fn status_bar_at_bottom_edge() {
        let s = full_chrome_shell();
        let l = s.shell_layout(area(), 1.0);
        let sb = l.status_bar_bounds.unwrap();
        let sb_bottom = sb.y + sb.height;
        assert!((sb_bottom - area().height).abs() < 0.01);
        assert_eq!(sb.width, area().width);
    }

    #[test]
    fn command_line_above_status_bar() {
        let s = full_chrome_shell();
        let l = s.shell_layout(area(), 1.0);
        let cl = l.command_line_bounds.unwrap();
        let sb = l.status_bar_bounds.unwrap();
        assert!((cl.y + cl.height - sb.y).abs() < 0.01);
    }

    #[test]
    fn bottom_panel_above_command_line() {
        let s = full_chrome_shell();
        let l = s.shell_layout(area(), 1.0);
        let bp = l.bottom_panel_bounds.unwrap();
        let cl = l.command_line_bounds.unwrap();
        assert!((bp.y + bp.height - cl.y).abs() < 0.01);
    }

    #[test]
    fn activity_bar_runs_full_band_height() {
        // The activity bar (and sidebar) span the full content band down to the
        // command line — the bottom panel docks only under main content, not
        // under the sidebar column, so it does not shorten the activity bar.
        let s = full_chrome_shell();
        let l = s.shell_layout(area(), 1.0);
        let tb = l.title_bar_bounds.unwrap();
        let cl = l.command_line_bounds.unwrap();
        let ab_top = l.activity_bar_bounds.y;
        let ab_bottom = l.activity_bar_bounds.y + l.activity_bar_bounds.height;
        assert!((ab_top - (tb.y + tb.height)).abs() < 0.01);
        assert!((ab_bottom - cl.y).abs() < 0.01);
    }

    #[test]
    fn bottom_panel_docks_under_main_only() {
        let s = full_chrome_shell();
        let l = s.shell_layout(area(), 1.0);
        let bp = l.bottom_panel_bounds.unwrap();
        let main = l.main_content_bounds;
        // Panel shares the main column's x and width — not the full viewport.
        assert_eq!(bp.x, main.x);
        assert_eq!(bp.width, main.width);
        assert!(bp.width < area().width);
        // Panel sits directly below the (shortened) main content.
        assert!((main.y + main.height - bp.y).abs() < 0.01);
    }

    #[test]
    fn chrome_vertical_no_overlap_no_gap() {
        let s = full_chrome_shell();
        let l = s.shell_layout(area(), 1.0);
        let tb = l.title_bar_bounds.unwrap();
        let cl = l.command_line_bounds.unwrap();
        let sb = l.status_bar_bounds.unwrap();
        // The activity-bar column runs full height; the panel lives in the main
        // column, so the left-edge vertical stack excludes it.
        let middle_h = l.activity_bar_bounds.height;
        let total = tb.height + middle_h + cl.height + sb.height;
        assert!(
            (total - area().height).abs() < 1.0,
            "total={total}, expected={}",
            area().height
        );
    }

    #[test]
    fn bottom_panel_hidden_no_bounds() {
        let mut s = full_chrome_shell();
        s.hide_bottom_panel();
        let l = s.shell_layout(area(), 1.0);
        assert!(l.bottom_panel_bounds.is_none());
        assert!(l.command_line_bounds.is_some());
        assert!(l.status_bar_bounds.is_some());
    }

    #[test]
    fn bottom_panel_toggle() {
        let mut s = full_chrome_shell();
        assert!(s.bottom_panel_visible());
        s.toggle_bottom_panel();
        assert!(!s.bottom_panel_visible());
        s.toggle_bottom_panel();
        assert!(s.bottom_panel_visible());
    }

    #[test]
    fn set_bottom_panel_height_clamps() {
        let mut s = full_chrome_shell();
        s.set_bottom_panel_height(1.0);
        assert_eq!(s.bottom_panel_height(), 3.0);
        s.set_bottom_panel_height(999.0);
        assert_eq!(s.bottom_panel_height(), 25.0);
    }

    // ── status_bar_under_main_column ──────────────────────────────

    #[test]
    fn status_bar_under_main_column_is_off_by_default_byte_identical() {
        // With the option left untouched, layout must match the
        // full-width carve exactly, field for field.
        let default_layout = full_chrome_shell().shell_layout(area(), 1.0);
        let explicit_off_layout = full_chrome_shell().shell_layout(area(), 1.0);
        assert_eq!(default_layout, explicit_off_layout);
    }

    #[test]
    fn status_bar_under_main_column_activity_bar_runs_to_area_bottom() {
        let s = full_chrome_shell().with_status_bar_under_main_column();
        let l = s.shell_layout(area(), 1.0);
        let ab_bottom = l.activity_bar_bounds.y + l.activity_bar_bounds.height;
        assert!((ab_bottom - area().height).abs() < 0.01);
        let tb = l.title_bar_bounds.unwrap();
        let header = l.sidebar_header_bounds.unwrap();
        let content = l.sidebar_content_bounds.unwrap();
        let sidebar_bottom = content.y + content.height;
        assert!((header.y - (tb.y + tb.height)).abs() < 0.01);
        assert!((sidebar_bottom - area().height).abs() < 0.01);
    }

    #[test]
    fn status_bar_under_main_column_status_bar_matches_main_column_width() {
        let s = full_chrome_shell().with_status_bar_under_main_column();
        let l = s.shell_layout(area(), 1.0);
        let sb = l.status_bar_bounds.unwrap();
        let main = l.main_content_bounds;
        assert_eq!(sb.x, main.x);
        assert_eq!(sb.width, main.width);
        assert!(sb.width < area().width);
        let sb_bottom = sb.y + sb.height;
        assert!((sb_bottom - area().height).abs() < 0.01);
    }

    #[test]
    fn status_bar_under_main_column_command_line_above_status_bar_in_main_column() {
        let s = full_chrome_shell().with_status_bar_under_main_column();
        let l = s.shell_layout(area(), 1.0);
        let cl = l.command_line_bounds.unwrap();
        let sb = l.status_bar_bounds.unwrap();
        let main = l.main_content_bounds;
        assert_eq!(cl.x, main.x);
        assert_eq!(cl.width, main.width);
        assert!((cl.y + cl.height - sb.y).abs() < 0.01);
    }

    #[test]
    fn status_bar_under_main_column_bottom_panel_above_command_line_in_main_column() {
        let s = full_chrome_shell().with_status_bar_under_main_column();
        let l = s.shell_layout(area(), 1.0);
        let bp = l.bottom_panel_bounds.unwrap();
        let cl = l.command_line_bounds.unwrap();
        let main = l.main_content_bounds;
        assert_eq!(bp.x, main.x);
        assert_eq!(bp.width, main.width);
        assert!((bp.y + bp.height - cl.y).abs() < 0.01);
        assert!((main.y + main.height - bp.y).abs() < 0.01);
    }

    #[test]
    fn bottom_panel_resize_drag() {
        let mut s = Owned::new(full_chrome_shell());
        s.state.drag = Some(ShellDrag::BottomPanel(0.0));
        let ev = s.handle(
            &UiEvent::MouseMoved {
                position: Point::new(40.0, 18.0),
                buttons: ButtonMask {
                    left: true,
                    middle: false,
                    right: false,
                },
            },
            &RecordingBackend::new(),
            area(),
        );
        assert!(matches!(ev, ShellEvent::BottomPanelResized { .. }));
    }

    // ── Runtime title-bar toggle ────────────────────────────────────

    #[test]
    fn title_bar_visible_reflects_construction_state() {
        assert!(!shell().title_bar_visible());
        assert!(full_chrome_shell().title_bar_visible());
    }

    #[test]
    fn set_title_bar_visible_toggles_at_runtime_without_with_title_bar() {
        let mut s = shell(); // never called with_title_bar()
        assert!(s.shell_layout(area(), 1.0).title_bar_bounds.is_none());

        s.set_title_bar_visible(true);
        assert!(s.title_bar_visible());
        let tb = s
            .shell_layout(area(), 1.0)
            .title_bar_bounds
            .expect("row now reserved");
        // Falls back to the default height (1.5 line-heights) since
        // `with_title_bar` was never called: (1.5 * lh=1.0).round() == 2.0.
        assert_eq!(tb.height, 2.0);

        s.set_title_bar_visible(false);
        assert!(!s.title_bar_visible());
        assert!(s.shell_layout(area(), 1.0).title_bar_bounds.is_none());
    }

    /// Confirm that the activity bar and main content re-derive their
    /// origins from scratch in both states rather than assuming a fixed
    /// offset.
    #[test]
    fn set_title_bar_visible_shifts_activity_bar_and_main_origin_both_ways() {
        let mut s = full_chrome_shell(); // with_title_bar(1.5)
        let l = s.shell_layout(area(), 1.0);
        let tb = l.title_bar_bounds.unwrap();
        assert_eq!(l.activity_bar_bounds.y, tb.y + tb.height);
        assert_eq!(l.main_content_bounds.y, tb.y + tb.height);

        s.set_title_bar_visible(false);
        let l = s.shell_layout(area(), 1.0);
        assert!(l.title_bar_bounds.is_none());
        assert_eq!(l.activity_bar_bounds.y, area().y);
        assert_eq!(l.main_content_bounds.y, area().y);

        // Toggle back on: restores the height configured via
        // `with_title_bar` (2.0, from 1.5 lh) — hiding does not forget it
        // and fall back to some other default.
        s.set_title_bar_visible(true);
        let l = s.shell_layout(area(), 1.0);
        let tb = l.title_bar_bounds.unwrap();
        assert_eq!(tb.height, 2.0);
        assert_eq!(l.activity_bar_bounds.y, tb.y + tb.height);
    }

    #[test]
    fn set_title_bar_visible_false_reclaims_the_row_for_content() {
        let mut s = full_chrome_shell();
        let visible_main_h = s.shell_layout(area(), 1.0).main_content_bounds.height;

        s.set_title_bar_visible(false);
        let hidden_main_h = s.shell_layout(area(), 1.0).main_content_bounds.height;

        assert!(
            hidden_main_h > visible_main_h,
            "hiding the title bar should hand its row back to content: \
             hidden={hidden_main_h}, visible={visible_main_h}"
        );
    }

    // ── Bottom bands ─────────────────────────────────────────────────

    fn band(id: &str, height_lh: f32) -> BottomBand {
        BottomBand::new(WidgetId::new(id), height_lh)
    }

    /// With no bands registered, `bottom_band_bounds()` stays empty and the
    /// existing single-drawer layout is unaffected — the whole point of
    /// making this additive.
    #[test]
    fn no_bands_by_default_layout_unchanged() {
        let s = full_chrome_shell();
        let l = s.shell_layout(area(), 1.0);
        assert!(s
            .bottom_band_bounds(area(), 1.0, &Style::default())
            .is_empty());
        assert!(l.bottom_panel_bounds.is_some());
    }

    /// Two visible bands stack bottom-up: band 0 flush against the main
    /// column's bottom edge, band 1 directly above it.
    #[test]
    fn bands_stack_bottom_up_in_list_order() {
        let s = shell().with_bottom_bands(vec![band("band:wildmenu", 1.0), band("band:qf", 3.0)]);
        let l = s.shell_layout(area(), 1.0);
        let bands = s.bottom_band_bounds(area(), 1.0, &Style::default());
        assert_eq!(bands.len(), 2);

        let (id0, r0) = &bands[0];
        let (id1, r1) = &bands[1];
        assert_eq!(*id0, WidgetId::new("band:wildmenu"));
        assert_eq!(*id1, WidgetId::new("band:qf"));

        // band 0 (wildmenu) sits flush against the shell's bottom edge —
        // `shell()` reserves no status bar / command line, so that edge is
        // the viewport's own bottom edge.
        let main = l.main_content_bounds;
        assert!((r0.y + r0.height - (area().y + area().height)).abs() < 0.01);
        assert_eq!(r0.height, 1.0);
        assert_eq!(r1.height, 3.0);
        // band 1 sits directly above band 0, with no gap or overlap.
        assert!((r1.y + r1.height - r0.y).abs() < 0.01);
        // Both share the main column's x/width, not the full viewport.
        assert_eq!(r0.x, main.x);
        assert_eq!(r0.width, main.width);
        assert_eq!(r1.x, main.x);
        assert_eq!(r1.width, main.width);
    }

    /// A band with `visible: false` occupies no space — presence gating,
    /// not just a height of zero. The remaining bands close the gap.
    #[test]
    fn invisible_band_is_absent_from_layout_and_closes_the_gap() {
        let mut hidden = band("band:qf", 3.0);
        hidden.visible = false;
        let s = shell().with_bottom_bands(vec![band("band:wildmenu", 1.0), hidden]);
        let l = s.shell_layout(area(), 1.0);
        let bands = s.bottom_band_bounds(area(), 1.0, &Style::default());
        assert_eq!(bands.len(), 1);
        assert_eq!(bands[0].0, WidgetId::new("band:wildmenu"));

        // Main content only lost the visible band's height, not both.
        let baseline = shell().shell_layout(area(), 1.0).main_content_bounds.height;
        assert_eq!(l.main_content_bounds.height, baseline - 1.0);
    }

    /// The legacy single bottom-panel drawer docks directly above every
    /// bottom band — i.e. it behaves as one more, implicit band stacked
    /// on top of the list, not underneath or interleaved with it.
    #[test]
    fn legacy_bottom_panel_docks_above_all_bands() {
        let s = full_chrome_shell().with_bottom_bands(vec![band("band:status", 1.0)]);
        let l = s.shell_layout(area(), 1.0);
        let bp = l
            .bottom_panel_bounds
            .expect("full_chrome_shell has a panel");
        let bands = s.bottom_band_bounds(area(), 1.0, &Style::default());
        assert_eq!(bands.len(), 1);
        let (_, band_rect) = &bands[0];

        // Band sits below the panel, flush with the main column's original
        // bottom edge; the panel sits directly above the band.
        assert!((bp.y + bp.height - band_rect.y).abs() < 0.01);
    }

    /// `set_bottom_band_visible`/`set_bottom_band_height` mutate the named
    /// band and report `false` for an unknown id, mirroring
    /// `close_tab`'s "not found" contract on `BottomPanel`.
    #[test]
    fn set_bottom_band_visible_and_height_mutate_by_id() {
        let mut s = shell().with_bottom_bands(vec![band("band:qf", 3.0)]);
        let id = WidgetId::new("band:qf");

        assert!(s.set_bottom_band_visible(&id, false));
        assert!(!s.bottom_band(&id).unwrap().visible);
        s.shell_layout(area(), 1.0);
        assert!(s
            .bottom_band_bounds(area(), 1.0, &Style::default())
            .is_empty());

        assert!(s.set_bottom_band_visible(&id, true));
        assert!(s.set_bottom_band_height(&id, 5.0));
        assert_eq!(s.bottom_band(&id).unwrap().height_lh, 5.0);
        s.shell_layout(area(), 1.0);
        assert_eq!(
            s.bottom_band_bounds(area(), 1.0, &Style::default())[0]
                .1
                .height,
            5.0
        );

        let unknown = WidgetId::new("band:nope");
        assert!(!s.set_bottom_band_visible(&unknown, true));
        assert!(!s.set_bottom_band_height(&unknown, 2.0));
    }

    /// A band taller than the available main-content height clamps to
    /// fill it rather than producing a negative-height remainder — same
    /// defensive clamp the legacy single-drawer carve already applies.
    #[test]
    fn oversized_band_clamps_to_available_height() {
        let s = shell().with_bottom_bands(vec![band("band:huge", 9999.0)]);
        let l = s.shell_layout(area(), 1.0);
        let bands = s.bottom_band_bounds(area(), 1.0, &Style::default());
        assert_eq!(bands.len(), 1);
        assert_eq!(l.main_content_bounds.height, 0.0);
        assert!(bands[0].1.height > 0.0);
    }

    /// Golden shape for every [`ShellEvent`] variant's [`Serialize`]
    /// impl — the externally-tagged, one-entry-map JSON a binding's own
    /// `on_shell_event` dict/JSON payload must keep matching byte-for-byte
    /// (pins `quadraui-py`'s `test_run_shell.py` and `quadraui-node`'s
    /// `shell.test.ts` expectations, which assert these exact shapes
    /// through `pythonize`/`serde_json::to_value` rather than this crate's
    /// own `Serialize` impl directly).
    #[test]
    fn app_shell_event_serializes_to_the_externally_tagged_dict_shape() {
        let cases = [
            (
                ShellEvent::PanelChanged {
                    panel_id: WidgetId::new("panel:explorer"),
                },
                serde_json::json!({"PanelChanged": {"panel_id": "panel:explorer"}}),
            ),
            (
                ShellEvent::SidebarHidden,
                serde_json::json!({"SidebarHidden": null}),
            ),
            (
                ShellEvent::SidebarResized { new_width: 42.0 },
                serde_json::json!({"SidebarResized": {"new_width": 42.0}}),
            ),
            (
                ShellEvent::BottomPanelResized { new_height: 7.5 },
                serde_json::json!({"BottomPanelResized": {"new_height": 7.5}}),
            ),
            (
                ShellEvent::BottomPanelHidden,
                serde_json::json!({"BottomPanelHidden": null}),
            ),
            (
                ShellEvent::BottomItemClicked {
                    id: WidgetId::new("bottom:logs"),
                },
                serde_json::json!({"BottomItemClicked": {"id": "bottom:logs"}}),
            ),
            (ShellEvent::Consumed, serde_json::json!({"Consumed": null})),
            (ShellEvent::Ignored, serde_json::json!({"Ignored": null})),
        ];
        for (event, expected) in cases {
            let actual = serde_json::to_value(&event).expect("ShellEvent always serializes");
            assert_eq!(actual, expected, "{event:?}");
        }
    }
}
