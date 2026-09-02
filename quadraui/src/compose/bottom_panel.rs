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

pub use crate::backend::BackendWidget;
use crate::primitives::tab_bar::{TabBar, TabBarHits, TabBarSegment, TabItem};
use crate::types::WidgetId;
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
#[derive(Debug, Clone, PartialEq)]
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

/// Resolved panel regions for one rendered frame.
#[derive(Debug, Clone, PartialEq)]
pub struct BottomPanelLayout {
    /// One-row strip at the top of the panel containing tab labels + controls.
    pub tab_strip_bounds: Rect,
    /// Content area below the strip where the active tab's widget renders.
    pub content_bounds: Rect,
}

// ── Controller ────────────────────────────────────────────────────────────────

/// Stateful controller that renders a bottom panel tab strip and handles
/// user interactions (tab activation, close, maximise).
///
/// Created by the shell runner from a [`BottomPanelConfig`]; apps interact
/// through [`BottomPanelEvent`]s and by querying accessors.
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
    // ── Internal hit-test cache ─────────────────────────────────────
    tab_scroll_offset: usize,
    /// Hit map from the last `render()` — the authoritative regions the
    /// backend's `draw_tab_bar` actually painted against, so paint and click
    /// agree on tab / close-button / maximise positions. Positions are in
    /// target-surface (viewport-absolute) coordinates, matching the click.
    last_hits: Option<TabBarHits>,
    last_strip_bounds: Option<Rect>,
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
        // Implement the active_tab_id fallback: if the provided id doesn't
        // exist in the tab list, fall back to the first tab.
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
            last_hits: None,
            last_strip_bounds: None,
        }
    }

    // ── Accessors ──────────────────────────────────────────────────

    /// Ordered slice of all tabs currently in the panel.
    pub fn tabs(&self) -> &[BottomPanelTab] {
        &self.tabs
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
    pub fn close_tab(&mut self, id: &str) -> bool {
        let Some(idx) = self.tabs.iter().position(|t| t.id == id) else {
            return false;
        };
        self.tabs.remove(idx);
        if self.active_tab_id == id {
            self.active_tab_id = self.tabs.first().map(|t| t.id.clone()).unwrap_or_default();
        }
        true
    }

    // ── Render ─────────────────────────────────────────────────────

    /// Render the tab strip into the top row of `panel_bounds`, then call
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
        let lh = backend.line_height();

        let strip_h = lh;
        let strip = Rect::new(panel_bounds.x, panel_bounds.y, panel_bounds.width, strip_h);
        let content_h = (panel_bounds.height - strip_h).max(0.0);
        let content = Rect::new(
            panel_bounds.x,
            panel_bounds.y + strip_h,
            panel_bounds.width,
            content_h,
        );

        // Build the TabBar primitive for this frame.
        let tab_bar = self.build_tab_bar();

        // Paint the tab strip and capture the authoritative hit map the
        // rasteriser actually used. Hit-testing against this (rather than a
        // re-derived layout with a guessed measurer) guarantees the close-button
        // and maximise regions line up with the painted glyphs on every backend.
        let hits = backend.draw_tab_bar(strip, &tab_bar, None);
        self.tab_scroll_offset = hits.correct_scroll_offset;
        self.last_hits = Some(hits);
        self.last_strip_bounds = Some(strip);

        // Render active tab content.
        if content.width > 0.0 && content.height > 0.0 {
            if let Some(idx) = self.tabs.iter().position(|t| t.id == self.active_tab_id) {
                self.tabs[idx].content.render(backend, content);
            }
        }

        BottomPanelLayout {
            tab_strip_bounds: strip,
            content_bounds: content,
        }
    }

    // ── Click dispatch ─────────────────────────────────────────────

    /// Resolve a mouse click at `(x, y)` in viewport coordinates.
    ///
    /// Applies any resulting state change (tab activation, close, maximise
    /// toggle) to the controller and returns the event for the app. Returns
    /// `None` when the click is outside the tab strip or on dead space.
    ///
    /// Call this from the shell runner's mouse-down handler after
    /// [`AppShell::handle`] returns [`AppShellEvent::Ignored`].
    pub fn handle_click(&mut self, x: f32, y: f32) -> Option<BottomPanelEvent> {
        let strip = self.last_strip_bounds?;
        // Check vertical bounds.
        if y < strip.y || y >= strip.y + strip.height {
            return None;
        }
        let hits = self.last_hits.as_ref()?;

        // `TabBarHits` positions are in target-surface (viewport-absolute)
        // coordinates — the same space as the incoming click — so compare
        // the click `x` directly without re-subtracting the strip origin.
        let click_x = x as f64;
        let in_range = |range: (f64, f64)| click_x >= range.0 && click_x < range.1;

        // Close buttons take precedence over tab bodies — the × sits at the
        // tab's right edge, inside the body region.
        for (idx, cb) in hits.close_bounds.iter().enumerate() {
            if let Some(range) = cb {
                if in_range(*range) {
                    if let Some(tab) = self.tabs.get(idx) {
                        if tab.closable {
                            let id = tab.id.clone();
                            self.close_tab(&id);
                            return Some(BottomPanelEvent::TabClosed(id));
                        }
                    }
                    return None;
                }
            }
        }

        // Right segments — the only one we emit is the maximise toggle.
        if hits.right_segment_bounds.iter().copied().any(in_range) {
            self.toggle_maximised();
            return Some(BottomPanelEvent::MaximiseToggled);
        }

        // Tab bodies.
        for (idx, range) in hits.slot_positions.iter().enumerate() {
            if in_range(*range) {
                if let Some(tab) = self.tabs.get(idx) {
                    let id = tab.id.clone();
                    if id != self.active_tab_id {
                        self.activate_tab(id.clone());
                        return Some(BottomPanelEvent::TabActivated(id));
                    }
                }
                return None;
            }
        }
        None
    }

    // ── Helpers ────────────────────────────────────────────────────

    /// Build the [`TabBar`] primitive from current state.
    fn build_tab_bar(&self) -> TabBar {
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
                    // Propagate per-tab closability so rasterisers can suppress
                    // the × glyph and close-button hit region for non-closable
                    // tabs even when `show_tab_close` is set on the bar.
                    is_closable: t.closable,
                }
            })
            .collect();

        // Maximise segment: " v " when maximised, " ^ " when docked.
        let max_text = if self.maximised { " v " } else { " ^ " };
        let right_segments = vec![TabBarSegment {
            text: max_text.to_string(),
            width_cells: 3,
            id: Some(WidgetId::new("bp:maximise")),
            is_active: self.maximised,
        }];

        TabBar {
            id: WidgetId::new("bottom-panel:tabs"),
            tabs,
            scroll_offset: self.tab_scroll_offset,
            right_segments,
            active_accent: None,
            // show_tab_close drives close-button rendering for ALL tabs;
            // the per-tab `closable` flag gates the close_width in our
            // measurement function so non-closable tabs get no close region.
            show_tab_close: self.tabs.iter().any(|t| t.closable),
            compact: true,
        }
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

    /// Prime a controller's hit map + strip bounds so handle_click works
    /// without a real backend render. Measurement: each tab = 6 cells total
    /// (4 label + 2 padding/close), maximise segment = 3 cells. Converted to
    /// `TabBarHits` exactly as a backend's `draw_tab_bar` would return them.
    fn prime_layout(ctrl: &mut BottomPanelController, strip_x: f32, strip_y: f32, bar_w: f32) {
        use crate::backend::tab_bar_hits_from_layout;
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
        // Mirror the backends: `TabBarHits` are returned in target-surface
        // (absolute) coordinates, i.e. offset by the strip's x origin.
        let mut hits = tab_bar_hits_from_layout(&layout, &tab_bar);
        let ox = strip_x as f64;
        for sp in &mut hits.slot_positions {
            if *sp != (0.0, 0.0) {
                sp.0 += ox;
                sp.1 += ox;
            }
        }
        for cb in hits.close_bounds.iter_mut().flatten() {
            cb.0 += ox;
            cb.1 += ox;
        }
        for rb in &mut hits.right_segment_bounds {
            rb.0 += ox;
            rb.1 += ox;
        }
        ctrl.last_hits = Some(hits);
        ctrl.last_strip_bounds = Some(Rect::new(strip_x, strip_y, bar_w, 1.0));
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

    // ── handle_click: no layout primed ────────────────────────────────────────

    #[test]
    fn handle_click_without_render_returns_none() {
        let cfg = make_config(vec![tab("t0", "T", false)], "t0");
        let mut ctrl = BottomPanelController::new(cfg);
        assert!(ctrl.handle_click(0.0, 0.0).is_none());
    }

    // ── handle_click: outside strip ──────────────────────────────────────────

    #[test]
    fn handle_click_above_strip_is_none() {
        let cfg = make_config(vec![tab("t0", "T", false)], "t0");
        let mut ctrl = BottomPanelController::new(cfg);
        prime_layout(&mut ctrl, 0.0, 10.0, 80.0);
        assert!(ctrl.handle_click(5.0, 5.0).is_none()); // y=5 < strip.y=10
    }

    #[test]
    fn handle_click_below_strip_is_none() {
        let cfg = make_config(vec![tab("t0", "T", false)], "t0");
        let mut ctrl = BottomPanelController::new(cfg);
        prime_layout(&mut ctrl, 0.0, 10.0, 80.0);
        assert!(ctrl.handle_click(5.0, 12.0).is_none()); // y=12 >= strip.y+h=11
    }

    // ── handle_click: tab body ────────────────────────────────────────────────

    #[test]
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
    fn click_on_already_active_tab_returns_none() {
        let cfg = make_config(vec![tab("t0", "AAAA", false)], "t0");
        let mut ctrl = BottomPanelController::new(cfg);
        prime_layout(&mut ctrl, 0.0, 0.0, 80.0);
        let ev = ctrl.handle_click(2.0, 0.5); // x=2 → tab0 (already active)
        assert_eq!(ev, None);
    }

    // ── handle_click: close button ────────────────────────────────────────────

    #[test]
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
}
