//! `BottomPanel` — a tabbed dockable panel for the bottom of an [`AppShell`].
//!
//! Modelled on VS Code's terminal panel. Attach a [`BottomPanelConfig`] to
//! [`crate::ShellConfig::bottom_panel`] to add a tab strip + switchable
//! content region below the main content area.
//!
//! # Usage pattern
//!
//! ```ignore
//! use quadraui::{Backend, Rect, ShellConfig};
//! use quadraui::compose::bottom_panel::{
//!     BackendWidget, BottomPanelConfig, BottomPanelTab,
//! };
//!
//! struct MyContent;
//! impl BackendWidget for MyContent {
//!     fn render(&self, backend: &mut dyn Backend, rect: Rect) {
//!         // draw something into rect
//!     }
//! }
//!
//! let config = ShellConfig::new("App", panels)
//!     .with_bottom_panel_config(BottomPanelConfig {
//!         tabs: vec![
//!             BottomPanelTab {
//!                 id: "bp:terminal".into(),
//!                 label: "TERMINAL".into(),
//!                 closable: false,
//!                 badge: None,
//!                 content: Box::new(MyContent),
//!             },
//!         ],
//!         active_tab_id: "bp:terminal".into(),
//!         maximised: false,
//!         height_fraction: 0.3,
//!     });
//! ```
//!
//! # Adoption status
//!
//! Folded into [`AppShell`]: `ShellConfig::bottom_panel` /
//! `ShellConfig::with_bottom_panel` (`shell.rs`) is what constructs a
//! `BottomPanelController` (`shell_adapter.rs`), so a host app opts in
//! by setting that config field rather than constructing this type
//! directly. Demoed via `examples/tui_bottom_panel.rs`,
//! `examples/gtk_bottom_panel.rs`, and
//! `examples/common/full_chrome_demo.rs`.
//!
//! # Event routing
//!
//! The shell runner calls [`ShellApp::on_bottom_panel_event`] whenever
//! the user interacts with the tab strip. Tab activation and close are
//! handled automatically (the controller mutates its own state); the
//! app is notified for re-render and any higher-level bookkeeping.
//!
//! # Maximised mode
//!
//! When [`BottomPanelController::maximised`] is `true`, the panel
//! expands to fill the full content area. The tab strip remains visible
//! at the top; main content is hidden. Click the `^`/`v` button or
//! emit [`BottomPanelEvent::MaximiseToggled`] to toggle.
//!
//! # Resize
//!
//! The resize grip (top edge of the panel) is managed by [`AppShell`]
//! — dragging it emits `AppShellEvent::BottomPanelResized`, which the
//! shell runner maps to [`BottomPanelEvent::Resized`] for the app.

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

use crate::a11y::{A11yChild, A11yInfo, A11yPrimitive};
pub use crate::backend::BackendWidget;
use crate::backend::DrawsPrimitives;
use crate::compose::Handled;
use crate::event::{MouseButton, Point};
use crate::interactive::{
    EventCx, EventHost, FocusSpec, Interactive, PointerKind, RoutedEvent, SubTarget, WidgetAction,
};
use crate::paint_surface::PaintSurface;
use crate::primitive::{LayoutCtx, PaintEnv, Primitive, Units};
use crate::primitives::tab_bar::{
    tab_bar_sub_regions, TabBar, TabBarEvent, TabBarPrimitiveLayout, TabBarSegment, TabBarState,
    TabItem,
};
use crate::types::{Modifiers, WidgetId};
#[cfg(feature = "tui")]
use crate::CellSurface;
use crate::{Backend, Rect};

// ── Public types ──────────────────────────────────────────────────────────────

/// One tab in a [`BottomPanelConfig`].
pub struct BottomPanelTab {
    /// Unique identifier referenced by [`BottomPanelConfig::active_tab_id`]
    /// and emitted in events.
    pub id: String,
    /// Display label shown in the tab strip.
    pub label: String,
    /// When `true`, the tab renders a `×` close button; clicking it emits
    /// [`BottomPanelEvent::TabClosed`] and removes the tab from the panel.
    pub closable: bool,
    /// Optional badge text rendered after the label (e.g. `"3"` for a
    /// problem count). Displayed inside parentheses: `"PROBLEMS (3)"`.
    pub badge: Option<String>,
    /// Content rendered into the panel body when this tab is active.
    pub content: Box<dyn BackendWidget>,
}

/// Configuration for a bottom-panel tab strip.
///
/// Pass as [`crate::ShellConfig::bottom_panel`] to add a tabbed panel
/// below the main content area. `None` (the default) preserves the
/// existing no-panel layout.
pub struct BottomPanelConfig {
    /// Ordered list of panel tabs.
    pub tabs: Vec<BottomPanelTab>,
    /// `id` of the initially-active tab. Falls back to the first tab when
    /// the named tab is not found.
    pub active_tab_id: String,
    /// When `true` the panel starts maximised (takes the full content area,
    /// hiding main content).
    pub maximised: bool,
    /// Initial panel height as a fraction of total viewport height.
    /// Defaults to `0.3` (30 %). The fraction is only used on first setup
    /// (and on viewport resize); subsequent heights come from the resize
    /// drag managed by [`AppShell`].
    pub height_fraction: f32,
}

impl Default for BottomPanelConfig {
    fn default() -> Self {
        Self {
            tabs: Vec::new(),
            active_tab_id: String::new(),
            maximised: false,
            height_fraction: 0.3,
        }
    }
}

/// Semantic events emitted by the bottom panel controller.
///
/// The shell runner delivers these to [`crate::ShellApp::on_bottom_panel_event`].
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub enum BottomPanelEvent {
    /// User switched to a different tab. Payload is the new active tab `id`.
    ///
    /// The controller updates [`BottomPanelController::active_tab_id`]
    /// automatically before emitting this event.
    TabActivated(String),
    /// User clicked the `×` close button on a closable tab. Payload is the
    /// closed tab's `id`. The controller removes the tab automatically.
    TabClosed(String),
    /// User clicked the `^`/`v` maximise toggle. The controller flips
    /// [`BottomPanelController::maximised`] automatically.
    MaximiseToggled,
    /// User dragged the resize grip (forwarded from
    /// `AppShellEvent::BottomPanelResized`). Payload is the new panel
    /// height in the backend's native units (cells for TUI, pixels for GTK).
    Resized(f32),
}

/// Externally-tagged, one-entry-map shape: `{"TabActivated": "bp:terminal"}`,
/// `{"MaximiseToggled": null}`, etc. Hand-written rather than derived for
/// the same reason [`crate::compose::app_shell::AppShellEvent`]'s own impl
/// is: `serde`'s default external tagging would serialize the unit variant
/// [`BottomPanelEvent::MaximiseToggled`] as a bare string, not the
/// dict-with-`null` shape a binding's own `on_bottom_panel_event` callback
/// already observes. Written once here rather than once per binding — a
/// binding reaches this via a generic `pythonize::pythonize`/
/// `serde_json::to_value` call, not its own hand-rolled match.
impl Serialize for BottomPanelEvent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(1))?;
        match self {
            BottomPanelEvent::TabActivated(id) => map.serialize_entry("TabActivated", id)?,
            BottomPanelEvent::TabClosed(id) => map.serialize_entry("TabClosed", id)?,
            BottomPanelEvent::MaximiseToggled => map.serialize_entry("MaximiseToggled", &())?,
            BottomPanelEvent::Resized(height) => map.serialize_entry("Resized", height)?,
        }
        map.end()
    }
}

/// Resolved panel regions for one rendered frame.
#[derive(Debug, Clone, PartialEq)]
pub struct BottomPanelLayout {
    /// Strip at the top of the panel containing tab labels + controls —
    /// `Style::tab_height` tall on a pixel backend, one row tall on a
    /// cell grid (TUI).
    pub tab_strip_bounds: Rect,
    /// Content area below the strip where the active tab's widget renders.
    pub content_bounds: Rect,
}

// ── BottomPanel: the widget-contract descriptor ─────────────────────────────

/// One tab of a [`BottomPanel`]: what its strip shows. The tab's content
/// is the host's; [`BottomPanelController`] pairs each of these with a
/// [`BackendWidget`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BottomPanelItem {
    /// Identifies the tab in [`BottomPanel::active_tab_id`] and in events.
    pub id: String,
    /// The tab's label.
    pub label: String,
    /// Whether the tab shows a close button.
    pub closable: bool,
    /// Optional badge shown after the label in parentheses.
    pub badge: Option<String>,
}

impl BottomPanelItem {
    /// A tab with no close button and no badge.
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            closable: false,
            badge: None,
        }
    }

    /// Set whether the tab shows a close button.
    #[must_use]
    pub fn with_closable(mut self, closable: bool) -> Self {
        self.closable = closable;
        self
    }

    /// Set the tab's badge.
    #[must_use]
    pub fn with_badge(mut self, badge: Option<String>) -> Self {
        self.badge = badge;
        self
    }
}

/// The [`WidgetId`] a bottom panel's tab strip paints under, which the
/// bindings and drivers address it by.
pub const BOTTOM_PANEL_TABS_ID: &str = "bottom-panel:tabs";

/// The [`WidgetId`] of a bottom panel's maximise toggle — the strip's one
/// right segment.
pub const BOTTOM_PANEL_MAXIMISE_ID: &str = "bp:maximise";

/// A tabbed bottom panel as a widget-contract descriptor: the tabs, the
/// active one and whether the panel is maximised. The app owns all of it
/// (controlled mode) and applies the [`BottomPanelEvent`]s
/// [`Interactive::handle`] reports; [`BottomPanelController`] is the
/// ready-made owner that does exactly that.
///
/// The strip paints and routes through a [`TabBar`] child that shares
/// this panel's [`Self::id`]; the content area below it is the host's.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct BottomPanel {
    /// The panel's id — also its tab strip's id. Defaults to
    /// [`BOTTOM_PANEL_TABS_ID`].
    pub id: WidgetId,
    /// The tabs, in strip order.
    pub tabs: Vec<BottomPanelItem>,
    /// The active tab's id.
    pub active_tab_id: String,
    /// Whether the panel is maximised; flips the toggle's glyph.
    pub maximised: bool,
}

impl BottomPanel {
    /// A docked panel with `tabs`, `active_tab_id` active.
    pub fn new(tabs: Vec<BottomPanelItem>, active_tab_id: impl Into<String>) -> Self {
        Self {
            id: WidgetId::new(BOTTOM_PANEL_TABS_ID),
            tabs,
            active_tab_id: active_tab_id.into(),
            maximised: false,
        }
    }

    /// Set the panel's id.
    #[must_use]
    pub fn with_id(mut self, id: WidgetId) -> Self {
        self.id = id;
        self
    }

    /// Set whether the panel is maximised.
    #[must_use]
    pub fn with_maximised(mut self, maximised: bool) -> Self {
        self.maximised = maximised;
        self
    }

    /// The [`TabBar`] child this panel paints, scrolled to `scroll_offset`.
    pub fn tab_bar(&self, scroll_offset: usize) -> TabBar {
        let tabs: Vec<TabItem> = self
            .tabs
            .iter()
            .map(|t| {
                let label = match &t.badge {
                    Some(b) => format!("{} ({})", t.label, b),
                    None => t.label.clone(),
                };
                TabItem {
                    label,
                    is_active: t.id == self.active_tab_id,
                    is_dirty: false,
                    is_preview: false,
                    // Per-tab closability suppresses the × glyph and its
                    // hit region even when `show_tab_close` is set.
                    is_closable: t.closable,
                }
            })
            .collect();

        // Maximise segment: " v " when maximised, " ^ " when docked.
        let max_text = if self.maximised { " v " } else { " ^ " };
        let right_segments = vec![TabBarSegment {
            text: max_text.to_string(),
            width_cells: 3,
            id: Some(WidgetId::new(BOTTOM_PANEL_MAXIMISE_ID)),
            is_active: self.maximised,
        }];

        TabBar {
            id: self.id,
            tabs,
            scroll_offset,
            right_segments,
            active_accent: None,
            // `show_tab_close` drives close-button rendering for every tab;
            // each tab's own `is_closable` then narrows it.
            show_tab_close: self.tabs.iter().any(|t| t.closable),
            compact: true,
            ..Default::default()
        }
    }

    /// The intent a tab-bar event on this panel's strip stands for.
    fn map_tab_event(&self, event: TabBarEvent) -> Handled<BottomPanelEvent> {
        match event {
            TabBarEvent::TabActivated { index } => match self.tabs.get(index) {
                Some(tab) if tab.id != self.active_tab_id => {
                    Handled::Event(BottomPanelEvent::TabActivated(tab.id.clone()))
                }
                _ => Handled::Consumed,
            },
            TabBarEvent::TabClosed { index } => match self.tabs.get(index) {
                Some(tab) if tab.closable => {
                    Handled::Event(BottomPanelEvent::TabClosed(tab.id.clone()))
                }
                _ => Handled::Consumed,
            },
            TabBarEvent::ButtonClicked { id } if id.as_str() == BOTTOM_PANEL_MAXIMISE_ID => {
                Handled::Event(BottomPanelEvent::MaximiseToggled)
            }
            _ => Handled::Consumed,
        }
    }
}

/// [`Primitive::Layout`] for [`BottomPanel`]: the strip and content split
/// and the [`TabBar`] child laid out on the strip.
///
/// `#[non_exhaustive]`: produced by [`Primitive::layout`], never built by
/// a consumer.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct BottomPanelPrimitiveLayout {
    /// The rect the panel was laid out into.
    pub area: Rect,
    /// The tab strip, flush with the top of [`Self::area`].
    pub tab_strip_bounds: Rect,
    /// Everything below the strip, where the active tab's content goes.
    pub content_bounds: Rect,
    /// The offset that keeps the active tab visible under this frame's
    /// measurements.
    pub scroll_offset: usize,
    /// The child strip, scrolled to [`Self::scroll_offset`].
    pub bar: TabBar,
    /// The child strip's own layout.
    pub tabs: TabBarPrimitiveLayout,
}

impl A11yPrimitive for BottomPanel {
    fn a11y_info(&self) -> A11yInfo {
        self.tab_bar(0).a11y_info()
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        self.tab_bar(0).a11y_children()
    }
}

impl Primitive for BottomPanel {
    type Layout = BottomPanelPrimitiveLayout;
    type Event = BottomPanelEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// Splits the strip off the top of `rect` — [`crate::Style::tab_height`]
    /// tall (or the chrome line height, when taller) on a pixel backend,
    /// one row on a cell grid — and lays the [`TabBar`] child out on it,
    /// a second time at the corrected offset when the active tab would
    /// otherwise be scrolled out of view.
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> Self::Layout {
        let lh = ctx.chrome_line_height();
        let strip_h = if lh > 1.0 {
            lh.max(ctx.style().tab_height)
        } else {
            lh
        };
        let tab_strip_bounds = Rect::new(rect.x, rect.y, rect.width, strip_h);
        let content_bounds = Rect::new(
            rect.x,
            rect.y + strip_h,
            rect.width,
            (rect.height - strip_h).max(0.0),
        );
        let mut scroll_offset = 0;
        let mut bar = self.tab_bar(scroll_offset);
        let mut tabs = Primitive::layout(&bar, ctx, tab_strip_bounds);
        if tabs.corrected_scroll_offset != scroll_offset {
            scroll_offset = tabs.corrected_scroll_offset;
            bar = self.tab_bar(scroll_offset);
            tabs = Primitive::layout(&bar, ctx, tab_strip_bounds);
        }
        BottomPanelPrimitiveLayout {
            area: rect,
            tab_strip_bounds,
            content_bounds,
            scroll_offset,
            bar,
            tabs,
        }
    }

    /// The strip, through the [`TabBar`] child's own paint.
    fn paint(&self, layout: &Self::Layout, surface: &mut dyn PaintSurface, env: &PaintEnv) {
        Primitive::paint(&layout.bar, &layout.tabs, surface, env);
    }

    /// The strip, through the [`TabBar`] child's own cell paint.
    #[cfg(feature = "tui")]
    fn paint_cells(&self, layout: &Self::Layout, cells: &mut dyn CellSurface, env: &PaintEnv) {
        Primitive::paint_cells(&layout.bar, &layout.tabs, cells, env);
    }
}

/// [`Interactive::State`] for [`BottomPanel`]: the [`TabBar`] child's own
/// state (its keyboard-focused tab).
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BottomPanelState {
    /// The tab strip's interaction state.
    pub tab_bar: TabBarState,
}

impl WidgetAction for BottomPanelEvent {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        crate::UiEvent::BottomPanel(id, self)
    }
}

impl Interactive for BottomPanel {
    type State = BottomPanelState;
    type Action = BottomPanelEvent;

    /// The [`TabBar`] child's own focus.
    fn focus(&self) -> FocusSpec {
        Interactive::focus(&self.tab_bar(0))
    }

    /// The strip's tabs, close buttons and maximise toggle; the strip
    /// shares the panel's origin.
    fn sub_regions(&self, layout: &Self::Layout) -> Vec<(Rect, SubTarget)> {
        tab_bar_sub_regions(&layout.tabs.layout)
    }

    /// Routes strip gestures and tab keys through the [`TabBar`] child:
    /// activating another tab, closing a closable one, or toggling
    /// maximised. A press on the strip's dead space is consumed; one on
    /// the content area is the host's.
    fn handle(
        &self,
        state: &mut BottomPanelState,
        event: &RoutedEvent,
        layout: &Self::Layout,
        cx: &mut EventCx,
    ) -> Handled<BottomPanelEvent> {
        let strip_press = match event {
            RoutedEvent::Pointer {
                kind: PointerKind::Down(_) | PointerKind::DoubleClick(_),
                local,
                ..
            } => {
                let strip = layout.tab_strip_bounds;
                if local.x < 0.0
                    || local.y < 0.0
                    || local.x >= strip.width
                    || local.y >= strip.height
                {
                    return Handled::Ignored;
                }
                true
            }
            _ => false,
        };
        // The child shares this panel's id, so it runs against the same
        // `cx`.
        match Interactive::handle(&layout.bar, &mut state.tab_bar, event, &layout.tabs, cx) {
            Handled::Event(tab_event) => self.map_tab_event(tab_event),
            Handled::Consumed => Handled::Consumed,
            Handled::Ignored if strip_press => Handled::Consumed,
            Handled::Ignored => Handled::Ignored,
        }
    }
}

// ── Controller ────────────────────────────────────────────────────────────────

/// Stateful controller that renders a bottom panel tab strip and handles
/// user interactions (tab activation, close, maximise).
///
/// Created by the shell runner from a [`BottomPanelConfig`]; apps interact
/// through [`BottomPanelEvent`]s and by querying accessors. Paints a
/// [`BottomPanel`] descriptor built from its own fields, routes clicks
/// through that descriptor's [`Interactive::handle`] and applies what it
/// reports.
///
/// # Interior-mutability note
///
/// The controller must be mutated during `render()`, which the runner
/// calls from `AppLogic::render(&self, …)`. Wrap in a `RefCell` when
/// storing in a `&self`-render context (the runner does this for you).
pub struct BottomPanelController {
    // ── Live mutable state ──────────────────────────────────────────
    /// The currently-active tab's `id`.
    pub active_tab_id: String,
    /// Whether the panel is currently maximised (takes the full content area).
    pub maximised: bool,
    /// The ordered tab list. Closed tabs are removed here.
    tabs: Vec<BottomPanelTab>,
    /// Panel height fraction stored for viewport-resize recalculation.
    pub height_fraction: f32,
    /// The scroll offset the last `render()` resolved.
    tab_scroll_offset: usize,
    /// The panel's interaction state.
    state: BottomPanelState,
    /// The panel's resolved layout from the last `render()`, which clicks
    /// are routed against so paint and click agree on tab, close-button
    /// and maximise positions.
    last_layout: Option<BottomPanelPrimitiveLayout>,
}

/// The [`EventHost`] [`BottomPanelController`] runs its panel's
/// [`Interactive::handle`] against: no focus, capture or outbox of its
/// own.
struct ControllerHost {
    units: Units,
}

impl EventHost for ControllerHost {
    fn is_focused(&self, _id: &WidgetId, _sub: Option<SubTarget>) -> bool {
        false
    }
    fn request_focus(&mut self, _id: &WidgetId, _sub: Option<SubTarget>) {}
    fn capture_pointer(&mut self, _id: &WidgetId) {}
    fn release_pointer(&mut self) {}
    fn set_cursor(&mut self, _cursor: crate::backend::PointerShape) {}
    fn request_redraw(&mut self) {}
    fn emit(&mut self, _event: crate::UiEvent) {}
    fn units(&self) -> Units {
        self.units
    }
}

impl BottomPanelController {
    /// Create from an initial configuration. The config is consumed; its
    /// mutable parts (active tab, maximised flag, tabs) live in the
    /// controller from here on.
    ///
    /// If `config.active_tab_id` does not match any tab in `config.tabs`,
    /// the first tab's `id` is used instead (or an empty string when there
    /// are no tabs). This implements the documented fallback.
    pub fn new(config: BottomPanelConfig) -> Self {
        let active_tab_id = if config.tabs.iter().any(|t| t.id == config.active_tab_id) {
            config.active_tab_id
        } else {
            config
                .tabs
                .first()
                .map(|t| t.id.clone())
                .unwrap_or_default()
        };
        Self {
            active_tab_id,
            maximised: config.maximised,
            tabs: config.tabs,
            height_fraction: config.height_fraction,
            tab_scroll_offset: 0,
            state: BottomPanelState::default(),
            last_layout: None,
        }
    }

    // ── Accessors ──────────────────────────────────────────────────

    /// Ordered slice of all tabs currently in the panel.
    pub fn tabs(&self) -> &[BottomPanelTab] {
        &self.tabs
    }

    /// The [`BottomPanel`] descriptor this controller paints right now.
    pub fn view(&self) -> BottomPanel {
        BottomPanel::new(
            self.tabs
                .iter()
                .map(|t| {
                    BottomPanelItem::new(t.id.clone(), t.label.clone())
                        .with_closable(t.closable)
                        .with_badge(t.badge.clone())
                })
                .collect(),
            self.active_tab_id.clone(),
        )
        .with_maximised(self.maximised)
    }

    // ── Mutations (called automatically from handle_click) ─────────

    /// Toggle between docked and maximised mode.
    pub fn toggle_maximised(&mut self) {
        self.maximised = !self.maximised;
    }

    /// Make `id` the active tab and reset the strip scroll offset.
    pub fn activate_tab(&mut self, id: String) {
        self.active_tab_id = id;
        self.tab_scroll_offset = 0;
    }

    /// Remove the tab with the given `id`. If it was the active tab,
    /// the first remaining tab becomes active; returns `false` when
    /// no tab with that `id` exists.
    pub fn close_tab(&mut self, id: impl Into<WidgetId>) -> bool {
        let id = id.into();
        let Some(idx) = self.tabs.iter().position(|t| t.id == id.as_str()) else {
            return false;
        };
        self.tabs.remove(idx);
        if self.active_tab_id == id.as_str() {
            self.active_tab_id = self.tabs.first().map(|t| t.id.clone()).unwrap_or_default();
        }
        true
    }

    // ── Render ─────────────────────────────────────────────────────

    /// Paint the panel's tab strip through the widget contract, then call
    /// the active tab's [`BackendWidget::render`] for the content area.
    ///
    /// Returns the resolved [`BottomPanelLayout`] so the caller can use
    /// the bounds for hit-region documentation or overlays.
    ///
    /// # Coordinate units
    ///
    /// `panel_bounds` must be in the backend's native units (cells for
    /// TUI, pixels for GTK). All returned rects use the same unit.
    pub fn render(&mut self, backend: &mut dyn Backend, panel_bounds: Rect) -> BottomPanelLayout {
        let view = self.view();
        let painted = DrawsPrimitives::draw_dyn(backend, panel_bounds, &view, &())
            .downcast_ref::<BottomPanelPrimitiveLayout>()
            .cloned()
            .unwrap_or_else(|| {
                let ctx = LayoutCtx::new(backend.chrome_line_height(), backend.char_width(), false);
                Primitive::layout(&view, &ctx, panel_bounds)
            });
        self.tab_scroll_offset = painted.scroll_offset;
        let layout = BottomPanelLayout {
            tab_strip_bounds: painted.tab_strip_bounds,
            content_bounds: painted.content_bounds,
        };
        self.last_layout = Some(painted);

        let content = layout.content_bounds;
        if content.width > 0.0 && content.height > 0.0 {
            if let Some(idx) = self.tabs.iter().position(|t| t.id == self.active_tab_id) {
                self.tabs[idx].content.render(backend, content);
            }
        }
        layout
    }

    // ── Click dispatch ─────────────────────────────────────────────

    /// Resolve a mouse click at `(x, y)` in viewport coordinates.
    ///
    /// Applies any resulting state change (tab activation, close, maximise
    /// toggle) to the controller and returns [`Handled::Event`] with the
    /// event for the app. Returns [`Handled::Consumed`] when the click
    /// landed inside the tab strip but on dead space (a non-closable tab's
    /// × region, or the already-active tab's body) — nothing to route, but
    /// the click was this controller's to handle. Returns
    /// [`Handled::Ignored`] when the click was outside the tab strip
    /// entirely.
    ///
    /// Call this from the shell runner's mouse-down handler after
    /// [`AppShell::handle`] returns [`AppShellEvent::Ignored`].
    pub fn handle_click_event(&mut self, x: f32, y: f32) -> Handled<BottomPanelEvent> {
        let Some(layout) = self.last_layout.as_ref() else {
            return Handled::Ignored;
        };
        // The strip spans only the panel's own column (`AppShell` docks
        // the bottom panel under the main column, not the full viewport
        // width), so a click at the strip's `y` but outside its `x` range
        // is not this controller's to claim.
        let strip = layout.tab_strip_bounds;
        if y < strip.y || y >= strip.y + strip.height || x < strip.x || x >= strip.x + strip.width {
            return Handled::Ignored;
        }
        let routed = RoutedEvent::Pointer {
            kind: PointerKind::Down(MouseButton::Left),
            sub: None,
            local: Point::new(x - layout.area.x, y - layout.area.y),
            mods: Modifiers::default(),
        };
        let view = self.view();
        let mut host = ControllerHost {
            units: if strip.height > 1.0 {
                Units::Pixels
            } else {
                Units::Cells
            },
        };
        let outcome = {
            let mut cx = EventCx::new(view.id, &mut host);
            Interactive::handle(&view, &mut self.state, &routed, layout, &mut cx)
        };
        if let Handled::Event(event) = &outcome {
            match event {
                BottomPanelEvent::TabActivated(id) => self.activate_tab(id.clone()),
                BottomPanelEvent::TabClosed(id) => {
                    self.close_tab(id.as_str());
                }
                BottomPanelEvent::MaximiseToggled => self.toggle_maximised(),
                BottomPanelEvent::Resized(_) => {}
            }
        }
        outcome
    }

    /// Resolve a mouse click at `(x, y)` in viewport coordinates.
    ///
    /// Applies any resulting state change (tab activation, close, maximise
    /// toggle) to the controller and returns the event for the app. Returns
    /// `None` when the click is outside the tab strip or on dead space.
    #[deprecated(
        since = "0.9.0",
        note = "use `handle_click_event`, which returns `Handled<BottomPanelEvent>` and distinguishes dead-space clicks (`Handled::Consumed`) from clicks outside the panel (`Handled::Ignored`)"
    )]
    pub fn handle_click(&mut self, x: f32, y: f32) -> Option<BottomPanelEvent> {
        self.handle_click_event(x, y).into_event()
    }

    // ── Helpers ────────────────────────────────────────────────────

    /// The [`TabBar`] this controller paints, at its last scroll offset.
    #[cfg(test)]
    fn build_tab_bar(&self) -> TabBar {
        self.view().tab_bar(self.tab_scroll_offset)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Test helper: no-op BackendWidget ──────────────────────────────────────

    struct NoOpWidget;

    impl BackendWidget for NoOpWidget {
        fn render(&self, _backend: &mut dyn Backend, _rect: Rect) {}
    }

    // ── Config helpers ────────────────────────────────────────────────────────

    fn tab(id: &str, label: &str, closable: bool) -> BottomPanelTab {
        BottomPanelTab {
            id: id.to_string(),
            label: label.to_string(),
            closable,
            badge: None,
            content: Box::new(NoOpWidget),
        }
    }

    fn make_config(tabs: Vec<BottomPanelTab>, active: &str) -> BottomPanelConfig {
        BottomPanelConfig {
            tabs,
            active_tab_id: active.to_string(),
            maximised: false,
            height_fraction: 0.3,
        }
    }

    // ── Layout pre-loading helper ─────────────────────────────────────────────

    /// Prime a controller's strip layout + bounds so handle_click works
    /// without a real backend render. Measurement: each tab = 6 cells total
    /// (4 label + 2 padding/close), maximise segment = 3 cells — the same
    /// strip-relative `TabBarLayout` a painted `TabBar` primitive resolves.
    fn prime_layout(ctrl: &mut BottomPanelController, strip_x: f32, strip_y: f32, bar_w: f32) {
        use crate::primitives::tab_bar::{SegmentMeasure, TabMeasure};

        let tab_bar = ctrl.build_tab_bar();
        let n = ctrl.tabs.len();
        let layout = tab_bar.layout(
            bar_w,
            1.0,
            0.0,
            |i| {
                if i < n && ctrl.tabs[i].closable {
                    TabMeasure::new(6.0, 2.0)
                } else {
                    TabMeasure::new(6.0, 0.0)
                }
            },
            |_| SegmentMeasure::new(3.0),
        );
        let strip = Rect::new(strip_x, strip_y, bar_w, 1.0);
        ctrl.last_layout = Some(BottomPanelPrimitiveLayout {
            area: strip,
            tab_strip_bounds: strip,
            content_bounds: Rect::new(strip_x, strip_y + 1.0, bar_w, 0.0),
            scroll_offset: 0,
            bar: tab_bar,
            tabs: TabBarPrimitiveLayout {
                area: strip,
                nerd_fonts_enabled: false,
                layout,
                corrected_scroll_offset: 0,
            },
        });
    }

    // ── Construction ─────────────────────────────────────────────────────────

    #[test]
    fn new_controller_reflects_initial_state() {
        let cfg = make_config(vec![tab("t0", "TERM", false)], "t0");
        let ctrl = BottomPanelController::new(cfg);
        assert_eq!(ctrl.active_tab_id, "t0");
        assert!(!ctrl.maximised);
        assert_eq!(ctrl.tabs().len(), 1);
    }

    // ── Toggle maximised ─────────────────────────────────────────────────────

    #[test]
    fn toggle_maximised_flips_state_twice() {
        let cfg = make_config(vec![], "");
        let mut ctrl = BottomPanelController::new(cfg);
        ctrl.toggle_maximised();
        assert!(ctrl.maximised);
        ctrl.toggle_maximised();
        assert!(!ctrl.maximised);
    }

    // ── Activate tab ─────────────────────────────────────────────────────────

    #[test]
    fn activate_tab_changes_active_and_resets_scroll() {
        let cfg = make_config(vec![tab("t0", "A", false), tab("t1", "B", false)], "t0");
        let mut ctrl = BottomPanelController::new(cfg);
        ctrl.tab_scroll_offset = 2; // dirty state
        ctrl.activate_tab("t1".to_string());
        assert_eq!(ctrl.active_tab_id, "t1");
        assert_eq!(ctrl.tab_scroll_offset, 0);
    }

    // ── Close tab ─────────────────────────────────────────────────────────────

    #[test]
    fn close_tab_removes_it_and_falls_back_to_first() {
        let cfg = make_config(
            vec![
                tab("t0", "A", true),
                tab("t1", "B", false),
                tab("t2", "C", false),
            ],
            "t0",
        );
        let mut ctrl = BottomPanelController::new(cfg);
        let removed = ctrl.close_tab("t0");
        assert!(removed);
        assert_eq!(ctrl.tabs().len(), 2);
        assert_eq!(ctrl.active_tab_id, "t1"); // fallback to first
    }

    #[test]
    fn close_inactive_tab_preserves_active() {
        let cfg = make_config(vec![tab("t0", "A", false), tab("t1", "B", true)], "t0");
        let mut ctrl = BottomPanelController::new(cfg);
        ctrl.close_tab("t1");
        assert_eq!(ctrl.active_tab_id, "t0"); // not changed
        assert_eq!(ctrl.tabs().len(), 1);
    }

    #[test]
    fn close_unknown_tab_returns_false() {
        let cfg = make_config(vec![tab("t0", "A", false)], "t0");
        let mut ctrl = BottomPanelController::new(cfg);
        assert!(!ctrl.close_tab("no-such-id"));
    }

    #[test]
    fn close_tab_also_accepts_a_widget_id_directly() {
        let cfg = make_config(vec![tab("t0", "A", true), tab("t1", "B", false)], "t0");
        let mut ctrl = BottomPanelController::new(cfg);
        assert!(ctrl.close_tab(WidgetId::new("t0")));
        assert_eq!(ctrl.tabs().len(), 1);
    }

    // ── handle_click: no layout primed ────────────────────────────────────────

    #[test]
    #[allow(deprecated)] // exercises the `handle_click` shim directly.
    fn handle_click_without_render_returns_none() {
        let cfg = make_config(vec![tab("t0", "T", false)], "t0");
        let mut ctrl = BottomPanelController::new(cfg);
        assert!(ctrl.handle_click(0.0, 0.0).is_none());
    }

    // ── handle_click: outside strip ──────────────────────────────────────────

    #[test]
    #[allow(deprecated)] // exercises the `handle_click` shim directly.
    fn handle_click_above_strip_is_none() {
        let cfg = make_config(vec![tab("t0", "T", false)], "t0");
        let mut ctrl = BottomPanelController::new(cfg);
        prime_layout(&mut ctrl, 0.0, 10.0, 80.0);
        assert!(ctrl.handle_click(5.0, 5.0).is_none()); // y=5 < strip.y=10
    }

    #[test]
    #[allow(deprecated)] // exercises the `handle_click` shim directly.
    fn handle_click_below_strip_is_none() {
        let cfg = make_config(vec![tab("t0", "T", false)], "t0");
        let mut ctrl = BottomPanelController::new(cfg);
        prime_layout(&mut ctrl, 0.0, 10.0, 80.0);
        assert!(ctrl.handle_click(5.0, 12.0).is_none()); // y=12 >= strip.y+h=11
    }

    // ── handle_click: tab body ────────────────────────────────────────────────

    #[test]
    #[allow(deprecated)] // exercises the `handle_click` shim directly.
    fn click_on_inactive_tab_emits_tab_activated() {
        // t0 = [0..6], t1 = [6..12], maximise = [77..80]
        let cfg = make_config(
            vec![tab("t0", "AAAA", false), tab("t1", "BBBB", false)],
            "t0",
        );
        let mut ctrl = BottomPanelController::new(cfg);
        prime_layout(&mut ctrl, 0.0, 0.0, 80.0);
        let ev = ctrl.handle_click(9.0, 0.5); // x=9 → local=9 → tab1
        assert_eq!(ev, Some(BottomPanelEvent::TabActivated("t1".to_string())));
        assert_eq!(ctrl.active_tab_id, "t1"); // state updated
    }

    #[test]
    #[allow(deprecated)] // exercises the `handle_click` shim directly.
    fn click_on_already_active_tab_returns_none() {
        let cfg = make_config(vec![tab("t0", "AAAA", false)], "t0");
        let mut ctrl = BottomPanelController::new(cfg);
        prime_layout(&mut ctrl, 0.0, 0.0, 80.0);
        let ev = ctrl.handle_click(2.0, 0.5); // x=2 → tab0 (already active)
        assert_eq!(ev, None);
    }

    // ── handle_click: close button ────────────────────────────────────────────

    #[test]
    #[allow(deprecated)] // exercises the `handle_click` shim directly.
    fn click_on_close_button_emits_tab_closed_and_removes_tab() {
        // tab0 closable: total=6, close=2 → close region [4..6]
        let cfg = make_config(vec![tab("t0", "AAAA", true)], "t0");
        let mut ctrl = BottomPanelController::new(cfg);
        prime_layout(&mut ctrl, 0.0, 0.0, 80.0);
        let ev = ctrl.handle_click(5.0, 0.5); // x=5 → close region of tab0
        assert_eq!(ev, Some(BottomPanelEvent::TabClosed("t0".to_string())));
        assert_eq!(ctrl.tabs().len(), 0);
    }

    #[test]
    #[allow(deprecated)] // exercises the `handle_click` shim directly.
    fn click_on_non_closable_tab_close_area_returns_none() {
        // tab0 NOT closable but measurement still gives it 6 cells total/0 close
        let cfg = make_config(vec![tab("t0", "AAAA", false)], "t0");
        let mut ctrl = BottomPanelController::new(cfg);
        prime_layout(&mut ctrl, 0.0, 0.0, 80.0);
        // With close_width=0, there is no TabBarHit::TabClose for tab0
        // so clicking anywhere in [0..6] → Tab(0) → already active → None
        let ev = ctrl.handle_click(5.0, 0.5);
        assert_eq!(ev, None);
    }

    // ── handle_click: non-zero strip origin (panel docked in main column) ─────

    #[test]
    #[allow(deprecated)] // exercises the `handle_click` shim directly.
    fn click_switches_and_closes_when_strip_offset_from_origin() {
        // Regression: the bottom panel docks in the main column, so the strip's
        // x origin is non-zero. Hits are in absolute coords, so clicks must be
        // compared against the raw x — not re-offset by the strip origin.
        // strip_x=20 → t0=[20..26] (close [24..26]), t1=[26..32] (absolute).
        let cfg = make_config(
            vec![tab("t0", "AAAA", true), tab("t1", "BBBB", false)],
            "t0",
        );
        let mut ctrl = BottomPanelController::new(cfg);
        prime_layout(&mut ctrl, 20.0, 0.0, 100.0);

        // Click body of the inactive tab t1 → activates it.
        let ev = ctrl.handle_click(28.0, 0.5);
        assert_eq!(ev, Some(BottomPanelEvent::TabActivated("t1".to_string())));
        assert_eq!(ctrl.active_tab_id, "t1");

        // Click the × of closable t0 (close region [24..26]) → closes it.
        prime_layout(&mut ctrl, 20.0, 0.0, 100.0);
        let ev = ctrl.handle_click(25.0, 0.5);
        assert_eq!(ev, Some(BottomPanelEvent::TabClosed("t0".to_string())));
        assert_eq!(ctrl.tabs().len(), 1);
    }

    // ── handle_click: maximise button ────────────────────────────────────────

    #[test]
    #[allow(deprecated)] // exercises the `handle_click` shim directly.
    fn click_on_maximise_toggles_and_emits_event() {
        // Maximise segment at [77..80] in 80-cell bar
        let cfg = make_config(vec![tab("t0", "AAAA", false)], "t0");
        let mut ctrl = BottomPanelController::new(cfg);
        prime_layout(&mut ctrl, 0.0, 0.0, 80.0);
        let ev = ctrl.handle_click(78.0, 0.5);
        assert_eq!(ev, Some(BottomPanelEvent::MaximiseToggled));
        assert!(ctrl.maximised);
        // Second click toggles back.
        prime_layout(&mut ctrl, 0.0, 0.0, 80.0); // re-prime after state change
        let ev2 = ctrl.handle_click(78.0, 0.5);
        assert_eq!(ev2, Some(BottomPanelEvent::MaximiseToggled));
        assert!(!ctrl.maximised);
    }

    // ── Docked vs maximised panel layout ─────────────────────────────────────

    #[test]
    fn build_tab_bar_shows_caret_when_docked() {
        let cfg = make_config(vec![tab("t0", "T", false)], "t0");
        let ctrl = BottomPanelController::new(cfg);
        let bar = ctrl.build_tab_bar();
        assert_eq!(bar.right_segments[0].text, " ^ ");
    }

    #[test]
    fn build_tab_bar_shows_down_arrow_when_maximised() {
        let cfg = make_config(vec![tab("t0", "T", false)], "t0");
        let mut ctrl = BottomPanelController::new(cfg);
        ctrl.maximised = true;
        let bar = ctrl.build_tab_bar();
        assert_eq!(bar.right_segments[0].text, " v ");
    }

    // ── Per-tab closability ───────────────────────────────────────────────────

    /// When a mix of closable and non-closable tabs is present, only the
    /// closable tab should have `is_closable = true` in the built `TabBar`.
    /// This ensures rasterisers can suppress the × glyph for non-closable tabs.
    #[test]
    fn build_tab_bar_propagates_per_tab_closability() {
        let cfg = make_config(
            vec![tab("t0", "TERM", false), tab("t1", "LOGS", true)],
            "t0",
        );
        let ctrl = BottomPanelController::new(cfg);
        let bar = ctrl.build_tab_bar();
        assert!(
            !bar.tabs[0].is_closable,
            "non-closable tab should have is_closable=false"
        );
        assert!(
            bar.tabs[1].is_closable,
            "closable tab should have is_closable=true"
        );
    }

    /// In a mixed configuration, clicking the close region of a non-closable tab
    /// (which has no close bounds) falls through to tab-body click → returns None
    /// (already active). A closable tab's close region still emits TabClosed.
    #[test]
    #[allow(deprecated)] // exercises the `handle_click` shim directly.
    fn mixed_closable_tabs_only_closable_tab_emits_close_event() {
        // t0: non-closable (close_w=0 → no close bounds)
        // t1: closable (close_w=2 → close region [10..12] in strip [0..80])
        // Layout: t0=[0..6] no close, t1=[6..12] with close [10..12]
        let cfg = make_config(
            vec![tab("t0", "AAAA", false), tab("t1", "BBBB", true)],
            "t0",
        );
        let mut ctrl = BottomPanelController::new(cfg);
        prime_layout(&mut ctrl, 0.0, 0.0, 80.0);

        // Click anywhere in t0's region — body click on already-active tab → None.
        let ev = ctrl.handle_click(3.0, 0.5);
        assert_eq!(ev, None, "click on non-closable active tab body → None");

        // Activate t1 first so t0 is inactive, then re-prime.
        ctrl.activate_tab("t1".to_string());
        prime_layout(&mut ctrl, 0.0, 0.0, 80.0);

        // Click t0's body → activates it (no close event even though t0 is non-closable).
        let ev = ctrl.handle_click(3.0, 0.5);
        assert_eq!(ev, Some(BottomPanelEvent::TabActivated("t0".to_string())));

        // Re-prime and click t1's close region.
        prime_layout(&mut ctrl, 0.0, 0.0, 80.0);
        let ev = ctrl.handle_click(11.0, 0.5); // x=11 → close region of t1 [10..12]
        assert_eq!(ev, Some(BottomPanelEvent::TabClosed("t1".to_string())));
        assert_eq!(ctrl.tabs().len(), 1, "t1 removed; only t0 remains");
    }

    // ── active_tab_id fallback ────────────────────────────────────────────────

    /// When `active_tab_id` does not match any tab, the constructor falls
    /// back to the first tab's id — implementing the documented behaviour.
    #[test]
    fn new_falls_back_to_first_tab_when_active_id_unknown() {
        let cfg = make_config(
            vec![tab("t0", "A", false), tab("t1", "B", false)],
            "no-such-id",
        );
        let ctrl = BottomPanelController::new(cfg);
        assert_eq!(ctrl.active_tab_id, "t0", "should fall back to first tab");
    }

    /// When the tab list is empty and active_tab_id is unknown, the constructor
    /// uses an empty string (the only sensible sentinel for "no active tab").
    #[test]
    fn new_uses_empty_string_when_no_tabs_and_unknown_id() {
        let cfg = make_config(vec![], "no-such-id");
        let ctrl = BottomPanelController::new(cfg);
        assert_eq!(ctrl.active_tab_id, "", "no tabs → empty active_tab_id");
    }

    // ── Tab strip height ──────────────────────────────────────────────

    fn gui_backend() -> crate::testing::RecordingBackend {
        crate::testing::RecordingBackend {
            line_height: 20.0,
            chrome_line_height: Some(18.0),
            viewport: crate::Viewport {
                width: 800.0,
                height: 300.0,
                scale: 1.0,
            },
            char_width: 8.0,
            ..Default::default()
        }
    }

    /// On a pixel backend (GUI), the tab row is sized off
    /// `Style::tab_height` (35, the default) rather than the raw chrome
    /// line height.
    #[test]
    fn render_tab_strip_height_uses_style_tab_height_on_gui() {
        let mut ctrl = BottomPanelController::new(make_config(vec![tab("t0", "A", false)], "t0"));
        let mut backend = gui_backend();
        let layout = ctrl.render(&mut backend, Rect::new(0.0, 0.0, 800.0, 300.0));
        assert_eq!(layout.tab_strip_bounds.height, 35.0);
    }

    /// `Backend::set_style` overriding `tab_height` changes the painted
    /// row height.
    #[test]
    fn render_tab_strip_height_follows_a_set_style_override() {
        let mut ctrl = BottomPanelController::new(make_config(vec![tab("t0", "A", false)], "t0"));
        let mut backend = gui_backend();
        backend.set_style(crate::Style::default().with_tab_height(50.0));
        let layout = ctrl.render(&mut backend, Rect::new(0.0, 0.0, 800.0, 300.0));
        assert_eq!(layout.tab_strip_bounds.height, 50.0);
    }

    /// On a cell grid (TUI), the row stays exactly one line tall.
    #[test]
    fn render_tab_strip_height_ignores_style_tab_height_on_a_cell_grid() {
        let mut ctrl = BottomPanelController::new(make_config(vec![tab("t0", "A", false)], "t0"));
        let mut backend = crate::testing::RecordingBackend::new();
        backend.set_style(crate::Style::default().with_tab_height(50.0));
        let layout = ctrl.render(&mut backend, Rect::new(0.0, 0.0, 80.0, 24.0));
        assert_eq!(layout.tab_strip_bounds.height, 1.0);
    }

    /// A real `render` caches the strip-relative layout the painted
    /// `TabBar` primitive resolved, so a click on tab 1's own close glyph
    /// closes *that* tab even when the panel is docked away from the
    /// viewport origin.
    #[test]
    fn a_close_glyph_click_after_a_real_render_closes_that_tab() {
        let mut ctrl = BottomPanelController::new(make_config(
            vec![
                tab("t0", "TERMINAL", true),
                tab("t1", "OUTPUT", true),
                tab("t2", "PROBLEMS", true),
            ],
            "t0",
        ));
        let mut backend = crate::testing::RecordingBackend::new();
        let strip = ctrl
            .render(&mut backend, Rect::new(5.0, 18.0, 60.0, 6.0))
            .tab_strip_bounds;

        let (cx, cy) = ctrl
            .last_layout
            .as_ref()
            .expect("render cached a layout")
            .tabs
            .layout
            .tab_close_center(1)
            .expect("tab 1 painted a close glyph");

        assert_eq!(
            ctrl.handle_click_event(strip.x + cx, strip.y + cy),
            Handled::Event(BottomPanelEvent::TabClosed("t1".to_string()))
        );
        assert_eq!(
            ctrl.tabs()
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            vec!["t0", "t2"]
        );
    }

    /// Same real-paint path on a tab body: the click activates that tab.
    #[test]
    fn a_tab_body_click_after_a_real_render_activates_that_tab() {
        let mut ctrl = BottomPanelController::new(make_config(
            vec![
                tab("t0", "TERMINAL", true),
                tab("t1", "OUTPUT", true),
                tab("t2", "PROBLEMS", true),
            ],
            "t0",
        ));
        let mut backend = crate::testing::RecordingBackend::new();
        let strip = ctrl
            .render(&mut backend, Rect::new(5.0, 18.0, 60.0, 6.0))
            .tab_strip_bounds;

        let (body_x, body_y) = {
            let layout = &ctrl
                .last_layout
                .as_ref()
                .expect("render cached a layout")
                .tabs
                .layout;
            let bounds = layout
                .visible_tabs
                .iter()
                .find(|vt| vt.tab_idx == 2)
                .expect("tab 2 is visible")
                .bounds;
            (bounds.x + 0.5, bounds.y + bounds.height / 2.0)
        };

        assert_eq!(
            ctrl.handle_click_event(strip.x + body_x, strip.y + body_y),
            Handled::Event(BottomPanelEvent::TabActivated("t2".to_string()))
        );
        assert_eq!(ctrl.active_tab_id, "t2");
    }

    /// Golden shape for every [`BottomPanelEvent`] variant's [`Serialize`]
    /// impl — pins the externally-tagged, one-entry-map JSON a binding's
    /// own `on_bottom_panel_event` dict/JSON payload must keep matching
    /// byte-for-byte, the same pinning `app_shell`'s own golden test gives
    /// [`crate::compose::app_shell::AppShellEvent`].
    #[test]
    fn bottom_panel_event_serializes_to_the_externally_tagged_dict_shape() {
        let cases = [
            (
                BottomPanelEvent::TabActivated("bp:terminal".to_string()),
                serde_json::json!({"TabActivated": "bp:terminal"}),
            ),
            (
                BottomPanelEvent::TabClosed("bp:logs".to_string()),
                serde_json::json!({"TabClosed": "bp:logs"}),
            ),
            (
                BottomPanelEvent::MaximiseToggled,
                serde_json::json!({"MaximiseToggled": null}),
            ),
            (
                BottomPanelEvent::Resized(12.5),
                serde_json::json!({"Resized": 12.5}),
            ),
        ];
        for (event, expected) in cases {
            let actual = serde_json::to_value(&event).expect("BottomPanelEvent always serializes");
            assert_eq!(actual, expected, "{event:?}");
        }
    }

    // ── handle_click_event: Handled<BottomPanelEvent> ─────────────────────────

    #[test]
    fn handle_click_event_without_render_is_ignored() {
        let cfg = make_config(vec![tab("t0", "T", false)], "t0");
        let mut ctrl = BottomPanelController::new(cfg);
        assert_eq!(ctrl.handle_click_event(0.0, 0.0), Handled::Ignored);
    }

    #[test]
    fn handle_click_event_outside_strip_is_ignored() {
        let cfg = make_config(vec![tab("t0", "T", false)], "t0");
        let mut ctrl = BottomPanelController::new(cfg);
        prime_layout(&mut ctrl, 0.0, 10.0, 80.0);
        assert_eq!(ctrl.handle_click_event(5.0, 5.0), Handled::Ignored); // above strip
        assert_eq!(ctrl.handle_click_event(5.0, 12.0), Handled::Ignored); // below strip
    }

    #[test]
    fn handle_click_event_outside_strip_horizontally_is_ignored() {
        // Strip starts at x=20 (docked under the main column, as `AppShell`
        // does), so a click at the strip's `y` but left or right of its `x`
        // range (the activity bar / sidebar columns) must not be claimed.
        let cfg = make_config(vec![tab("t0", "T", false)], "t0");
        let mut ctrl = BottomPanelController::new(cfg);
        prime_layout(&mut ctrl, 20.0, 0.0, 40.0);
        assert_eq!(ctrl.handle_click_event(5.0, 0.5), Handled::Ignored); // left of strip
        assert_eq!(ctrl.handle_click_event(70.0, 0.5), Handled::Ignored); // right of strip
    }

    #[test]
    fn handle_click_event_on_inactive_tab_is_event() {
        let cfg = make_config(
            vec![tab("t0", "AAAA", false), tab("t1", "BBBB", false)],
            "t0",
        );
        let mut ctrl = BottomPanelController::new(cfg);
        prime_layout(&mut ctrl, 0.0, 0.0, 80.0);
        let outcome = ctrl.handle_click_event(9.0, 0.5); // x=9 → tab1
        assert_eq!(
            outcome,
            Handled::Event(BottomPanelEvent::TabActivated("t1".to_string()))
        );
    }

    #[test]
    fn handle_click_event_on_already_active_tab_is_consumed() {
        let cfg = make_config(vec![tab("t0", "AAAA", false)], "t0");
        let mut ctrl = BottomPanelController::new(cfg);
        prime_layout(&mut ctrl, 0.0, 0.0, 80.0);
        // Inside the panel's tab strip, but dead space — the already-active
        // tab's own body — so this is `Consumed`, not `Ignored`.
        assert_eq!(ctrl.handle_click_event(2.0, 0.5), Handled::Consumed);
    }

    #[test]
    fn handle_click_event_on_non_closable_close_area_is_consumed() {
        let cfg = make_config(vec![tab("t0", "AAAA", false)], "t0");
        let mut ctrl = BottomPanelController::new(cfg);
        prime_layout(&mut ctrl, 0.0, 0.0, 80.0);
        assert_eq!(ctrl.handle_click_event(5.0, 0.5), Handled::Consumed);
    }

    #[test]
    fn handle_click_event_on_close_button_is_event() {
        let cfg = make_config(vec![tab("t0", "AAAA", true)], "t0");
        let mut ctrl = BottomPanelController::new(cfg);
        prime_layout(&mut ctrl, 0.0, 0.0, 80.0);
        let outcome = ctrl.handle_click_event(5.0, 0.5); // close region of tab0
        assert_eq!(
            outcome,
            Handled::Event(BottomPanelEvent::TabClosed("t0".to_string()))
        );
        assert_eq!(ctrl.tabs().len(), 0);
    }

    #[test]
    #[allow(deprecated)] // exercises the `handle_click` shim directly.
    fn deprecated_handle_click_still_adapts_to_option() {
        let cfg = make_config(
            vec![tab("t0", "AAAA", false), tab("t1", "BBBB", false)],
            "t0",
        );
        let mut ctrl = BottomPanelController::new(cfg);
        prime_layout(&mut ctrl, 0.0, 0.0, 80.0);
        assert_eq!(
            ctrl.handle_click(9.0, 0.5),
            Some(BottomPanelEvent::TabActivated("t1".to_string()))
        );
        assert_eq!(ctrl.handle_click(9.0, 0.5), None); // already active now — consumed, no event
        assert_eq!(ctrl.handle_click(5.0, 5.0), None); // ignored, outside strip
    }
}
