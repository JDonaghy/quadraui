//! Shell runner infrastructure: [`ShellApp`] trait + [`ShellConfig`].
//!
//! Apps that want an AppShell (activity bar + sidebar + main content)
//! implement [`ShellApp`] instead of [`AppLogic`](crate::AppLogic).
//! Per-backend `run_with_shell()` functions handle the full lifecycle:
//! window creation, event wiring, AppShell chrome rendering, and event
//! routing — the consumer renders only its own content.

use crate::compose::app_shell::{AppShellEvent, AppShellLayout, PanelDefinition, ShellPosition};
use crate::compose::bottom_panel::{BottomPanelConfig, BottomPanelEvent};
use crate::event::Rect;
use crate::types::WidgetId;
use crate::{Backend, Reaction, UiEvent};

/// Configuration for creating an AppShell.
pub struct ShellConfig {
    pub panels: Vec<PanelDefinition>,
    pub bottom_items: Vec<PanelDefinition>,
    pub title: String,
    pub default_sidebar_width: f32,
    pub min_sidebar_width: f32,
    pub max_sidebar_width: f32,
    pub position: ShellPosition,
    pub has_title_bar: bool,
    pub title_bar_height_lh: f32,
    pub has_bottom_panel: bool,
    pub bottom_panel_height_lh: f32,
    pub min_bottom_panel_height_lh: f32,
    pub max_bottom_panel_height_lh: f32,
    pub has_command_line: bool,
    pub has_status_bar: bool,
    /// Optional tabbed bottom panel. `None` (the default) preserves the
    /// existing no-panel layout. When `Some`, the shell runner creates a
    /// [`crate::compose::bottom_panel::BottomPanelController`] that renders
    /// the tab strip and delegates content to each tab's `BackendWidget`.
    ///
    /// Setting this field automatically enables an AppShell bottom panel
    /// region — no need to also call [`Self::with_bottom_panel`] unless you
    /// want to tune the height via the old API as well.
    pub bottom_panel: Option<BottomPanelConfig>,
}

impl ShellConfig {
    pub fn new(title: impl Into<String>, panels: Vec<PanelDefinition>) -> Self {
        Self {
            panels,
            bottom_items: Vec::new(),
            title: title.into(),
            default_sidebar_width: 20.0,
            min_sidebar_width: 8.0,
            max_sidebar_width: 50.0,
            position: ShellPosition::Left,
            has_title_bar: false,
            title_bar_height_lh: 1.5,
            has_bottom_panel: false,
            bottom_panel_height_lh: 10.0,
            min_bottom_panel_height_lh: 3.0,
            max_bottom_panel_height_lh: 30.0,
            has_command_line: false,
            has_status_bar: false,
            bottom_panel: None,
        }
    }

    pub fn with_bottom_items(mut self, items: Vec<PanelDefinition>) -> Self {
        self.bottom_items = items;
        self
    }

    pub fn with_position(mut self, position: ShellPosition) -> Self {
        self.position = position;
        self
    }

    pub fn with_title_bar(mut self, height_lh: f32) -> Self {
        self.has_title_bar = true;
        self.title_bar_height_lh = height_lh;
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

    pub fn with_command_line(mut self) -> Self {
        self.has_command_line = true;
        self
    }

    pub fn with_status_bar(mut self) -> Self {
        self.has_status_bar = true;
        self
    }

    /// Attach a tabbed bottom panel.
    ///
    /// This is the preferred way to add a bottom panel when you want tab
    /// switching, close buttons, and per-tab [`crate::compose::bottom_panel::BackendWidget`]
    /// content. It automatically enables the AppShell bottom panel region;
    /// the initial height comes from [`BottomPanelConfig::height_fraction`].
    ///
    /// Use [`Self::with_bottom_panel`] + [`Self::with_bottom_panel_limits`]
    /// instead when you only need a bare unstyled region below main content.
    pub fn with_bottom_panel_config(mut self, config: BottomPanelConfig) -> Self {
        self.bottom_panel = Some(config);
        self
    }
}

/// Context passed to [`ShellApp::handle`] so the consumer can route
/// events by panel without tracking shell state themselves.
pub struct ShellContext<'a> {
    /// Currently active sidebar panel, if any.
    pub active_panel_id: Option<&'a WidgetId>,
    /// Whether the sidebar is visible.
    pub sidebar_visible: bool,
    /// Layout bounds from the last render.
    pub layout: &'a AppShellLayout,
}

impl<'a> ShellContext<'a> {
    /// Check if a mouse position lands inside the sidebar content area.
    pub fn in_sidebar(&self, x: f32, y: f32) -> bool {
        rect_contains_opt(self.layout.sidebar_content_bounds, x, y)
    }

    /// Check if a mouse position lands inside the main content area.
    pub fn in_main(&self, x: f32, y: f32) -> bool {
        rect_contains(self.layout.main_content_bounds, x, y)
    }

    /// Check if a mouse position lands inside the bottom panel.
    pub fn in_bottom_panel(&self, x: f32, y: f32) -> bool {
        rect_contains_opt(self.layout.bottom_panel_bounds, x, y)
    }

    /// Check if a mouse position lands inside the title bar.
    pub fn in_title_bar(&self, x: f32, y: f32) -> bool {
        rect_contains_opt(self.layout.title_bar_bounds, x, y)
    }

    /// Check if a mouse position lands inside the status bar.
    pub fn in_status_bar(&self, x: f32, y: f32) -> bool {
        rect_contains_opt(self.layout.status_bar_bounds, x, y)
    }

    /// Check if a mouse position lands inside the command line.
    pub fn in_command_line(&self, x: f32, y: f32) -> bool {
        rect_contains_opt(self.layout.command_line_bounds, x, y)
    }

    /// Sidebar content bounds (convenience for coordinate translation).
    pub fn sidebar_bounds(&self) -> Option<Rect> {
        self.layout.sidebar_content_bounds
    }

    /// Main content bounds.
    pub fn main_bounds(&self) -> Rect {
        self.layout.main_content_bounds
    }

    /// Bottom panel bounds.
    pub fn bottom_panel_bounds(&self) -> Option<Rect> {
        self.layout.bottom_panel_bounds
    }

    /// Title bar bounds.
    pub fn title_bar_bounds(&self) -> Option<Rect> {
        self.layout.title_bar_bounds
    }

    /// Status bar bounds.
    pub fn status_bar_bounds(&self) -> Option<Rect> {
        self.layout.status_bar_bounds
    }

    /// Command line bounds.
    pub fn command_line_bounds(&self) -> Option<Rect> {
        self.layout.command_line_bounds
    }
}

fn rect_contains(r: Rect, x: f32, y: f32) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

fn rect_contains_opt(r: Option<Rect>, x: f32, y: f32) -> bool {
    r.is_some_and(|r| rect_contains(r, x, y))
}

/// Application trait for apps that use the AppShell chrome.
///
/// The shell handles: activity bar rendering + clicks, sidebar
/// header + divider, panel switching, and resize drag. The consumer
/// renders panel content and main-area content into the bounds the
/// shell provides via [`AppShellLayout`].
pub trait ShellApp {
    /// Render content into the shell's content zones. The shell has
    /// already drawn its chrome (activity bar, sidebar header, divider);
    /// the consumer draws sidebar panel content + main content here.
    fn render_content(&self, backend: &mut dyn Backend, layout: &AppShellLayout);

    /// Handle events the shell didn't consume. The [`ShellContext`]
    /// provides the active panel ID and layout bounds so the consumer
    /// can route per-panel without tracking shell state.
    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend, ctx: &ShellContext)
        -> Reaction;

    /// Called once after the shell is built. Optional.
    fn setup(&mut self, _backend: &mut dyn Backend) {}

    /// Notified when a panel switch occurs (activity bar click or
    /// programmatic). Optional.
    fn on_shell_event(&mut self, _event: &AppShellEvent) {}

    /// Notified when the bottom panel tab strip emits an event
    /// (tab activation, close, maximise toggle, or resize).
    ///
    /// Only called when `ShellConfig.bottom_panel` is `Some`. The
    /// [`crate::compose::bottom_panel::BottomPanelController`] has already
    /// applied the state change (switched active tab, removed a closed tab,
    /// toggled maximised) before this method is called. Use it for any
    /// higher-level bookkeeping — re-rendering is triggered automatically.
    ///
    /// Default: no-op.
    fn on_bottom_panel_event(&mut self, _event: &BottomPanelEvent) {}

    /// Periodic callback invoked by the runner on every frame (≈60Hz on
    /// the TUI backend, GTK timeout on GTK). Use for timer-driven work —
    /// polling background tasks, expiring caches, draining channels —
    /// that must happen without a user input event. Default is no-op.
    fn tick(&mut self, _backend: &mut dyn Backend) -> Reaction {
        Reaction::Continue
    }
}
