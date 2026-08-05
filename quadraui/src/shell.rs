//! Shell runner infrastructure: [`ShellApp`] trait + [`ShellConfig`].
//!
//! Apps that want an AppShell (activity bar + sidebar + main content)
//! implement [`ShellApp`] instead of [`AppLogic`](crate::AppLogic).
//! Per-backend `run_with_shell()` functions handle the full lifecycle:
//! window creation, event wiring, AppShell chrome rendering, and event
//! routing — the consumer renders only its own content.

use std::cell::{Cell, Ref, RefCell, RefMut};

use crate::compose::app_shell::{
    AppShell, AppShellEvent, AppShellLayout, PanelDefinition, ShellPosition,
};
use crate::compose::bottom_panel::{BottomPanelConfig, BottomPanelEvent};
use crate::event::Rect;
use crate::types::WidgetId;
use crate::{Backend, Reaction, ResizeEdge, UiEvent};

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
    /// Editor font override: `(family, size_pt)`. `None` (the default)
    /// leaves each backend's built-in default in place (GTK:
    /// `"Monospace 11"`; TUI: fixed-cell, no font concept — the value is
    /// ignored). Set via [`Self::with_editor_font`]; the shell runner
    /// applies it once via [`crate::Backend::set_editor_font`] during
    /// [`crate::shell_adapter::ShellAdapter`]'s one-time `setup()`, before
    /// the app's own `setup()` runs, so painted glyphs and click-column
    /// math derive from the same font from the very first frame (#422).
    pub editor_font: Option<(String, f32)>,
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
            editor_font: None,
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

    /// Override the font painted for editor content (family name + size
    /// in points).
    ///
    /// `family` should name a monospace font — primitives that map
    /// columns to pixels assume uniform glyph width. Applied once via
    /// [`crate::Backend::set_editor_font`] during the shell runner's
    /// one-time `setup()` call (see [`Self::editor_font`]); call
    /// [`crate::Backend::set_editor_font`] directly at runtime (e.g. from
    /// [`ShellApp::handle`]) to change it again after startup, such as a
    /// zoom-in/out keybinding.
    pub fn with_editor_font(mut self, family: impl Into<String>, size_pt: f32) -> Self {
        self.editor_font = Some((family.into(), size_pt));
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
    /// Set via [`Self::request_activity_keyboard_focus`]; consumed by
    /// [`crate::shell_adapter::ShellAdapter::handle`] after
    /// `ShellApp::handle` returns. `Cell` because `ShellContext` is passed
    /// by shared reference — the consumer can request focus without a
    /// `&mut` hook back into the shell.
    activity_focus_requested: Cell<bool>,
    /// A scoped mutable borrow of the [`AppShell`] instance
    /// [`crate::shell_adapter::ShellAdapter`] actually renders, lent out for
    /// the duration of one `ShellApp::handle` dispatch (#454).
    ///
    /// Before this existed, a `ShellApp` consumer had no way to reach the
    /// rendered shell at all — `ShellAdapter::shell` is `pub(crate)` — so
    /// driving shell state programmatically (e.g. a `Ctrl+B` toggle-sidebar
    /// binding) required keeping a second, shadow `AppShell` that silently
    /// drifted from the one actually painted (vimcode `Ctrl+B` was dead on
    /// GTK because of exactly this: the shadow toggled, the rendered
    /// instance never learned about it).
    ///
    /// `RefCell` because `ShellContext` is passed by shared reference (see
    /// `activity_focus_requested` above) — access via [`Self::shell`] /
    /// [`Self::shell_mut`]. `AppShell` remains solely owned and constructed
    /// by `ShellAdapter`; this only lends a scoped borrow for one dispatch,
    /// so the single-owner invariant that makes drift impossible is
    /// preserved.
    shell: RefCell<&'a mut AppShell>,
}

impl<'a> ShellContext<'a> {
    /// Construct a [`ShellContext`] for one `ShellApp::handle` dispatch.
    /// Only called by [`crate::shell_adapter::ShellAdapter`]; downstream
    /// consumers receive an already-built context, they don't build one.
    pub(crate) fn new(
        active_panel_id: Option<&'a WidgetId>,
        sidebar_visible: bool,
        layout: &'a AppShellLayout,
        shell: &'a mut AppShell,
    ) -> Self {
        Self {
            active_panel_id,
            sidebar_visible,
            layout,
            activity_focus_requested: Cell::new(false),
            shell: RefCell::new(shell),
        }
    }

    /// Borrow the real, rendered [`AppShell`] read-only.
    ///
    /// Prefer the narrow accessors above (`in_sidebar`, `sidebar_bounds`,
    /// ...) for layout queries — this exists for the rest of `AppShell`'s
    /// API (e.g. `bottom_panel_visible()`, `activity_selected_id()`) that
    /// `ShellContext` doesn't otherwise mirror.
    ///
    /// # Panics
    /// Panics if [`Self::shell_mut`] is borrowed at the same time (standard
    /// `RefCell` rules). Each borrow is meant to be short-lived — grab it,
    /// read, drop.
    pub fn shell(&self) -> Ref<'_, &'a mut AppShell> {
        self.shell.borrow()
    }

    /// Borrow the real, rendered [`AppShell`] mutably.
    ///
    /// This is the fix for #454: call any `AppShell` mutator directly on
    /// the instance [`crate::shell_adapter::ShellAdapter`] actually
    /// renders — `ctx.shell_mut().toggle_sidebar()`,
    /// `ctx.shell_mut().show_bottom_panel()`,
    /// `ctx.shell_mut().set_sidebar_width(w)`, etc. — instead of tracking a
    /// shadow copy that can silently drift from what's on screen.
    ///
    /// # Panics
    /// Panics if [`Self::shell`] / [`Self::shell_mut`] is borrowed at the
    /// same time (standard `RefCell` rules). Each borrow is meant to be
    /// short-lived — grab it, mutate, drop.
    pub fn shell_mut(&self) -> RefMut<'_, &'a mut AppShell> {
        self.shell.borrow_mut()
    }

    /// Request that the activity bar take keyboard focus, with its cursor
    /// reset to the top item, once the current `ShellApp::handle` call
    /// returns.
    ///
    /// Call this from your own `handle()` in response to whatever key you
    /// want to bind as the "focus the activity bar" trigger (`Tab`,
    /// `Ctrl+W`, a command-palette action, ...) — quadraui does not
    /// reserve a key for this itself, so different consumers can pick
    /// different triggers. Once focused, [`crate::shell_adapter::ShellAdapter`]
    /// owns the navigation keys (`j`/`k`/arrows to move, `Enter`/`Space` to
    /// activate, `Esc` to cancel) by driving the same
    /// [`crate::compose::app_shell::AppShell`] methods the raw `AppLogic`
    /// pattern uses directly — see `examples/common/shell_app.rs`.
    ///
    /// A no-op if called more than once per `handle()` call (the flag is
    /// simply set, not counted).
    pub fn request_activity_keyboard_focus(&self) {
        self.activity_focus_requested.set(true);
    }

    /// Consume the pending focus request, if any. `pub(crate)` — only
    /// [`crate::shell_adapter::ShellAdapter::handle`] calls this, after
    /// `ShellApp::handle` returns.
    pub(crate) fn take_activity_focus_requested(&self) -> bool {
        self.activity_focus_requested.replace(false)
    }

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

    /// The full window/viewport bounds this layout was computed against.
    pub fn window_bounds(&self) -> Rect {
        self.layout.window_bounds
    }

    /// Which edge or corner of the window `(x, y)` is within `margin` of,
    /// or `None` if the position isn't near any outer border (#406).
    ///
    /// Mirrors [`Self::in_title_bar`]'s pattern: the app hit-tests the
    /// pointer against a stored bounds field every frame rather than the
    /// backend owning edge detection. Callers should derive `margin` from a
    /// portable backend unit (e.g. `backend.line_height()`) rather than a
    /// hardcoded pixel/cell constant — see `quadraui/docs/LESSONS.md`
    /// "Shared AppLogic code must not hardcode backend-native units".
    ///
    /// Call from a `MouseDown` handler (pass the result to
    /// [`Backend::begin_window_resize`]) and from a `MouseMoved` handler
    /// (pass `PointerShape::Resize(edge)` / `PointerShape::Default` to
    /// [`Backend::set_cursor`]).
    pub fn window_edge(&self, x: f32, y: f32, margin: f32) -> Option<ResizeEdge> {
        let r = self.layout.window_bounds;
        // Points outside the window entirely (shouldn't normally happen —
        // mouse events are clipped to the window — but defend anyway) never
        // resolve to an edge.
        if x < r.x || x > r.x + r.width || y < r.y || y > r.y + r.height {
            return None;
        }
        // `near_right`/`near_bottom` use `>=` (not `>`) so the last valid
        // cell index is included on TUI, mirroring `rect_contains`'s own
        // `x < r.x + r.width` upper bound (which treats column
        // `r.width - 1` as inside). With continuous GTK pixel coordinates
        // the `=` case is a single point of measure zero and doesn't
        // change behaviour; on TUI's discrete cell grid it's the
        // difference between the bottom-right corner being reachable at
        // all and a permanently-dead corner (margin == 1 cell exactly
        // covers the last row/column, so a strict `>` would never fire).
        let margin = margin.max(0.0);
        let near_left = x < r.x + margin;
        let near_right = x >= r.x + r.width - margin;
        let near_top = y < r.y + margin;
        let near_bottom = y >= r.y + r.height - margin;

        // Corners first, then single edges. An if-chain (rather than an
        // exhaustive match on all four bools) reads clearer and handles the
        // degenerate case of a window smaller than `2 * margin` in one
        // dimension (both `near_top` and `near_bottom` true, no left/right)
        // by falling through to a deterministic single-edge pick instead of
        // requiring a meaningless tie-break arm.
        if near_top && near_left {
            Some(ResizeEdge::NorthWest)
        } else if near_top && near_right {
            Some(ResizeEdge::NorthEast)
        } else if near_bottom && near_left {
            Some(ResizeEdge::SouthWest)
        } else if near_bottom && near_right {
            Some(ResizeEdge::SouthEast)
        } else if near_top {
            Some(ResizeEdge::North)
        } else if near_bottom {
            Some(ResizeEdge::South)
        } else if near_left {
            Some(ResizeEdge::West)
        } else if near_right {
            Some(ResizeEdge::East)
        } else {
            None
        }
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

    /// Poll for a pending **app-initiated** panel switch — e.g. an action
    /// handler that jumps straight to a different panel (a "launch
    /// interactive session" command landing in the Terminal panel) without
    /// the user clicking the ActivityBar.
    ///
    /// [`ShellAdapter`](crate::shell_adapter::ShellAdapter) calls this once
    /// immediately after every [`Self::handle`] dispatch. Returning
    /// `Some(panel_id)` applies the switch to the underlying `AppShell`
    /// (updating the ActivityBar highlight and sidebar panel header, which
    /// are otherwise owned entirely by `AppShell` and invisible to
    /// `ShellApp` implementors) and re-notifies via
    /// [`Self::on_shell_event`] with [`AppShellEvent::PanelChanged`], the
    /// same notification a mouse click produces — so app code that already
    /// syncs its own view state from `on_shell_event` doesn't need a
    /// second code path.
    ///
    /// Before this hook existed, consumers had no way to keep the chrome in
    /// sync with a raw internal view-state write: the ActivityBar highlight
    /// and panel header would silently keep pointing at the
    /// previously-active panel until the user clicked the ActivityBar
    /// themselves (a bug class seen in consumer apps).
    ///
    /// Default: never requests a switch — existing consumers are
    /// unaffected. `&mut self` (not `&self`) so implementors can `.take()`
    /// a stored `Option` field rather than needing interior mutability.
    fn take_requested_panel(&mut self) -> Option<WidgetId> {
        None
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a bare (no panels, no chrome) `AppShellLayout` sized `w x h`
    /// starting at the origin — just enough to exercise `window_bounds`.
    fn layout_for(w: f32, h: f32) -> AppShellLayout {
        AppShell::new(Vec::new(), 20.0).layout(Rect::new(0.0, 0.0, w, h), 1.0)
    }

    fn ctx<'a>(layout: &'a AppShellLayout, shell: &'a mut AppShell) -> ShellContext<'a> {
        ShellContext::new(None, false, layout, shell)
    }

    /// #422: a fresh `ShellConfig` has no editor-font override — backends
    /// keep their own default (GTK: "Monospace 11").
    #[test]
    fn shell_config_editor_font_defaults_to_none() {
        let config = ShellConfig::new("test", Vec::new());
        assert_eq!(config.editor_font, None);
    }

    /// #422: `with_editor_font` stores the family/size pair verbatim for
    /// the shell runner to apply via `Backend::set_editor_font`.
    #[test]
    fn shell_config_with_editor_font_sets_family_and_size() {
        let config = ShellConfig::new("test", Vec::new()).with_editor_font("Fira Code", 14.0);
        assert_eq!(config.editor_font, Some(("Fira Code".to_string(), 14.0)));
    }

    #[test]
    fn window_edge_none_in_the_middle() {
        let layout = layout_for(100.0, 40.0);
        let mut shell = AppShell::new(Vec::new(), 20.0);
        assert_eq!(ctx(&layout, &mut shell).window_edge(50.0, 20.0, 4.0), None);
    }

    #[test]
    fn window_edge_detects_each_side() {
        let layout = layout_for(100.0, 40.0);
        let mut shell = AppShell::new(Vec::new(), 20.0);
        let c = ctx(&layout, &mut shell);
        assert_eq!(c.window_edge(50.0, 0.0, 4.0), Some(ResizeEdge::North));
        assert_eq!(c.window_edge(50.0, 40.0, 4.0), Some(ResizeEdge::South));
        assert_eq!(c.window_edge(0.0, 20.0, 4.0), Some(ResizeEdge::West));
        assert_eq!(c.window_edge(100.0, 20.0, 4.0), Some(ResizeEdge::East));
    }

    #[test]
    fn window_edge_detects_each_corner() {
        let layout = layout_for(100.0, 40.0);
        let mut shell = AppShell::new(Vec::new(), 20.0);
        let c = ctx(&layout, &mut shell);
        assert_eq!(c.window_edge(0.0, 0.0, 4.0), Some(ResizeEdge::NorthWest));
        assert_eq!(c.window_edge(100.0, 0.0, 4.0), Some(ResizeEdge::NorthEast));
        assert_eq!(c.window_edge(0.0, 40.0, 4.0), Some(ResizeEdge::SouthWest));
        assert_eq!(c.window_edge(100.0, 40.0, 4.0), Some(ResizeEdge::SouthEast));
    }

    /// A point outside the window bounds entirely never resolves to an
    /// edge, even if it would be "within margin" of one in absolute terms.
    #[test]
    fn window_edge_none_outside_window() {
        let layout = layout_for(100.0, 40.0);
        let mut shell = AppShell::new(Vec::new(), 20.0);
        assert_eq!(ctx(&layout, &mut shell).window_edge(-2.0, 20.0, 4.0), None);
        assert_eq!(ctx(&layout, &mut shell).window_edge(50.0, 45.0, 4.0), None);
    }

    /// A window narrower/shorter than `2 * margin` must still resolve
    /// deterministically (no panic) rather than requiring an ambiguous
    /// top-vs-bottom / left-vs-right tie-break.
    #[test]
    fn window_edge_degenerate_tiny_window_is_deterministic() {
        let layout = layout_for(3.0, 3.0);
        let mut shell = AppShell::new(Vec::new(), 20.0);
        let c = ctx(&layout, &mut shell);
        // Every point in a 3x3 window is within margin=4.0 of every edge;
        // just assert this doesn't panic and returns a corner (checked
        // first in the priority chain).
        assert_eq!(c.window_edge(1.5, 1.5, 4.0), Some(ResizeEdge::NorthWest));
    }

    /// #454: `ctx.shell_mut()` reaches the real `AppShell` — a `ShellApp`
    /// can drive shell state (e.g. a `Ctrl+B` toggle-sidebar binding)
    /// directly on the instance that's actually rendered, with no shadow
    /// copy required.
    #[test]
    fn shell_mut_toggles_the_real_app_shell() {
        let layout = layout_for(100.0, 40.0);
        let mut shell = AppShell::new(Vec::new(), 20.0);
        assert!(shell.sidebar_visible());
        {
            let c = ctx(&layout, &mut shell);
            c.shell_mut().toggle_sidebar();
        }
        assert!(!shell.sidebar_visible());
    }

    /// #454: `ctx.shell()` gives read access without requiring a mutable
    /// borrow, mirroring the read-only convenience accessors above.
    #[test]
    fn shell_read_reflects_current_state() {
        let layout = layout_for(100.0, 40.0);
        let mut shell = AppShell::new(Vec::new(), 20.0);
        shell.hide_sidebar();
        let c = ctx(&layout, &mut shell);
        assert!(!c.shell().sidebar_visible());
    }
}
