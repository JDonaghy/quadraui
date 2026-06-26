//! `AppShell` — a composed controller for the activity bar + sidebar
//! panel container pattern (VS Code / JetBrains / Zellij-style app shell).
//!
//! Owns the full interaction state machine: activity bar click toggle,
//! active panel switching, sidebar visibility, and resize drag. Apps
//! register panels at construction time or dynamically via
//! [`AppShell::add_panel`] / [`AppShell::remove_panel`], match on
//! [`AppShellEvent`] for semantic actions, and paint their panel content
//! into the bounds the shell returns.
//!
//! The shell renders the activity bar, sidebar header chrome, and resize
//! divider. Panel *content* and the main content area are the consumer's
//! responsibility — the shell returns [`AppShellLayout`] with the bounds.

use std::cell::RefCell;

use crate::primitives::activity_bar::{ActivityBar, ActivityBarRowHit, ActivityItem};
use crate::primitives::status_bar::{StatusBar, StatusBarSegment};
use crate::types::{Color, WidgetId};
use crate::{Backend, ButtonMask, MouseButton, Point, Rect, UiEvent};

// ── Public types ─────────────────────────────────────────────────────

/// Registration info for one sidebar panel.
#[derive(Debug, Clone)]
pub struct PanelDefinition {
    pub id: WidgetId,
    pub icon: String,
    pub tooltip: String,
    pub title: String,
}

/// Which side of the viewport the activity bar + sidebar sit on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShellPosition {
    #[default]
    Left,
    Right,
}

/// Semantic events emitted by [`AppShell::handle`].
#[derive(Debug, Clone, PartialEq)]
pub enum AppShellEvent {
    PanelChanged { panel_id: WidgetId },
    SidebarHidden,
    SidebarResized { new_width: f32 },
    BottomPanelResized { new_height: f32 },
    BottomPanelHidden,
    BottomItemClicked { id: WidgetId },
    Consumed,
    Ignored,
}

/// Layout bounds returned by [`AppShell::render`] and [`AppShell::layout`].
#[derive(Debug, Clone, PartialEq)]
pub struct AppShellLayout {
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

// ── AppShell ─────────────────────────────────────────────────────────

/// All width/height fields are stored as **multiples of `line_height`**
/// so they are portable across TUI (cells) and GUI (pixels) backends.
/// `compute_layout()` resolves them: `dimension_native = field * lh`.
pub struct AppShell {
    panels: Vec<PanelDefinition>,
    bottom_items: Vec<PanelDefinition>,
    active_panel: Option<usize>,
    sidebar_visible: bool,
    /// Sidebar width in line_height multiples.
    sidebar_width: f32,
    min_sidebar_width: f32,
    max_sidebar_width: f32,
    /// Activity bar width in line_height multiples.
    activity_bar_width: f32,
    position: ShellPosition,
    drag_offset: Option<f32>,
    hovered_activity_idx: Option<usize>,
    // ── Chrome slot config ───────────────────────────────────────
    has_title_bar: bool,
    title_bar_height_lh: f32,
    has_bottom_panel: bool,
    bottom_panel_height_lh: f32,
    min_bottom_panel_height_lh: f32,
    max_bottom_panel_height_lh: f32,
    bottom_panel_visible: bool,
    has_command_line: bool,
    has_status_bar: bool,
    bottom_panel_drag_offset: Option<f32>,
    /// Cached hit regions from the last `render()` call. `handle()`
    /// dispatches clicks against these so paint and click agree on
    /// row positions — the structural fix for the GTK ACTIVITY_ROW_PX
    /// vs line_height mismatch (LESSONS.md: one derivation, two consumers).
    /// `RefCell` because `render(&self)` is called from `AppLogic::render`
    /// which takes `&self`.
    cached_activity_hits: RefCell<Vec<ActivityBarRowHit>>,
    cached_activity_bar_bounds: RefCell<Option<Rect>>,
    // ── Keyboard navigation state ─────────────────────────────────
    /// Whether the activity bar currently holds keyboard focus. When
    /// `true`, `build_activity_bar()` sets `is_keyboard_focused = true`
    /// on the emitted [`ActivityBar`], causing the TUI and GTK backends
    /// to redirect [`crate::UiEvent::KeyPressed`] events to
    /// [`crate::UiEvent::ActivityBar`] events — so navigation logic
    /// stays entirely in the consumer.
    activity_keyboard_focused: bool,
    /// Cursor position in the **logical item sequence**: top panels
    /// (indices `0..panels.len()`) followed by bottom items
    /// (indices `panels.len()..panels.len()+bottom_items.len()`).
    /// Saturates at the ends; does not wrap.
    activity_cursor: usize,
}

impl AppShell {
    pub fn new(panels: Vec<PanelDefinition>, default_sidebar_width: f32) -> Self {
        let active = if panels.is_empty() { None } else { Some(0) };
        Self {
            panels,
            bottom_items: Vec::new(),
            active_panel: active,
            sidebar_visible: true,
            sidebar_width: default_sidebar_width,
            min_sidebar_width: 8.0,
            max_sidebar_width: 800.0,
            activity_bar_width: 3.0,
            position: ShellPosition::Left,
            drag_offset: None,
            hovered_activity_idx: None,
            has_title_bar: false,
            title_bar_height_lh: 1.5,
            has_bottom_panel: false,
            bottom_panel_height_lh: 10.0,
            min_bottom_panel_height_lh: 3.0,
            max_bottom_panel_height_lh: 30.0,
            bottom_panel_visible: true,
            has_command_line: false,
            has_status_bar: false,
            bottom_panel_drag_offset: None,
            cached_activity_hits: RefCell::new(Vec::new()),
            cached_activity_bar_bounds: RefCell::new(None),
            activity_keyboard_focused: false,
            activity_cursor: 0,
        }
    }

    pub fn with_bottom_items(mut self, items: Vec<PanelDefinition>) -> Self {
        self.bottom_items = items;
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

    pub fn with_activity_bar_width(mut self, width: f32) -> Self {
        self.activity_bar_width = width;
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

    // ── State accessors ──────────────────────────────────────────────

    pub fn active_panel(&self) -> Option<&PanelDefinition> {
        self.active_panel.and_then(|i| self.panels.get(i))
    }

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

    pub fn hovered_activity_idx(&self) -> Option<usize> {
        self.hovered_activity_idx
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

    /// Move the keyboard cursor to the next item in the logical sequence
    /// (top panels first, then bottom items). **Saturates** at the last
    /// item — does not wrap to the top.
    pub fn activity_select_next(&mut self) {
        let total = self.panels.len() + self.bottom_items.len();
        if total > 0 {
            self.activity_cursor = (self.activity_cursor + 1).min(total - 1);
        }
    }

    /// Move the keyboard cursor to the previous item. **Saturates** at
    /// index 0 — does not wrap to the bottom.
    pub fn activity_select_prev(&mut self) {
        self.activity_cursor = self.activity_cursor.saturating_sub(1);
    }

    /// Set the keyboard cursor to an explicit index in the logical sequence.
    /// Out-of-range values are clamped to the valid range `[0, total-1]`.
    /// Has no effect when the panel + bottom-item lists are both empty.
    pub fn activity_set_cursor(&mut self, idx: usize) {
        let total = self.panels.len() + self.bottom_items.len();
        if total > 0 {
            self.activity_cursor = idx.min(total - 1);
        }
    }

    /// Returns the [`WidgetId`] of the item currently under the keyboard
    /// cursor, or `None` if there are no items at all.
    ///
    /// The logical sequence is top panels (`0..panels.len()`) followed by
    /// bottom items (`panels.len()..`).
    pub fn activity_selected_id(&self) -> Option<&WidgetId> {
        let np = self.panels.len();
        if self.activity_cursor < np {
            Some(&self.panels[self.activity_cursor].id)
        } else {
            let bi = self.activity_cursor - np;
            self.bottom_items.get(bi).map(|p| &p.id)
        }
    }

    /// Activate the item currently under the keyboard cursor and return
    /// the resulting [`AppShellEvent`], or `None` if there are no items.
    ///
    /// Equivalent to clicking the cursor item: top-panel items trigger the
    /// same toggle / switch logic as a mouse click, bottom items emit
    /// [`AppShellEvent::BottomItemClicked`]. Does **not** clear keyboard
    /// focus — the consumer decides whether to do that.
    pub fn activity_activate_selected(&mut self) -> Option<AppShellEvent> {
        let id = self.activity_selected_id()?.clone();
        Some(self.handle_activity_click(&id))
    }

    // ── Programmatic state control ───────────────────────────────────

    pub fn show_panel(&mut self, panel_id: &WidgetId) {
        for (i, p) in self.panels.iter().enumerate() {
            if p.id == *panel_id {
                self.active_panel = Some(i);
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
    pub fn remove_bottom_item(&mut self, id: &WidgetId) -> bool {
        let Some(idx) = self.bottom_items.iter().position(|p| p.id == *id) else {
            return false;
        };
        self.bottom_items.remove(idx);
        true
    }

    // ── Layout ───────────────────────────────────────────────────────

    /// Compute layout bounds without drawing. Use when only hit-testing
    /// is needed (e.g. in `handle`).
    pub fn layout(&self, area: Rect, line_height: f32) -> AppShellLayout {
        self.compute_layout(area, line_height)
    }

    /// Build the [`ActivityBar`] primitive from current panel state.
    ///
    /// When [`Self::activity_keyboard_focused`] is `true`:
    /// - `ActivityBar::is_keyboard_focused` is set to `true`, causing both
    ///   the TUI and GTK backends to route key events as
    ///   `UiEvent::ActivityBar(id, ActivityBarEvent::KeyPressed { … })`.
    /// - The item at [`Self::activity_cursor`] (in the top-then-bottom
    ///   logical sequence) has `is_keyboard_selected = true`, painting the
    ///   selection ring on that icon.
    pub fn build_activity_bar(&self) -> ActivityBar {
        let np = self.panels.len();
        let focused = self.activity_keyboard_focused;

        let top_items: Vec<ActivityItem> = self
            .panels
            .iter()
            .enumerate()
            .map(|(i, p)| ActivityItem {
                id: p.id.clone(),
                icon: p.icon.clone(),
                tooltip: p.tooltip.clone(),
                is_active: self.active_panel == Some(i) && self.sidebar_visible,
                is_keyboard_selected: focused && self.activity_cursor == i,
            })
            .collect();

        let bottom_items: Vec<ActivityItem> = self
            .bottom_items
            .iter()
            .enumerate()
            .map(|(j, p)| ActivityItem {
                id: p.id.clone(),
                icon: p.icon.clone(),
                tooltip: p.tooltip.clone(),
                is_active: false,
                is_keyboard_selected: focused && self.activity_cursor == np + j,
            })
            .collect();

        ActivityBar {
            id: WidgetId::new("app-shell:activity-bar"),
            top_items,
            bottom_items,
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: focused,
        }
    }

    // ── Render ───────────────────────────────────────────────────────

    /// Render shell chrome and return layout bounds for consumer content.
    ///
    /// Draws: activity bar, sidebar header, resize divider.
    /// Consumer draws: sidebar panel content, main content area.
    pub fn render(&self, backend: &mut dyn Backend, area: Rect) -> AppShellLayout {
        let lh = backend.line_height();
        let layout = self.compute_layout(area, lh);

        let bar = self.build_activity_bar();
        let hits =
            backend.draw_activity_bar(layout.activity_bar_bounds, &bar, self.hovered_activity_idx);
        *self.cached_activity_hits.borrow_mut() = hits;
        *self.cached_activity_bar_bounds.borrow_mut() = Some(layout.activity_bar_bounds);

        if let Some(header_bounds) = layout.sidebar_header_bounds {
            if let Some(panel) = self.active_panel.and_then(|i| self.panels.get(i)) {
                let header_bar = StatusBar {
                    id: WidgetId::new("app-shell:sidebar-header"),
                    left_segments: vec![StatusBarSegment {
                        text: format!(" {} ", panel.title),
                        fg: Color::rgb(220, 220, 220),
                        bg: Color::rgb(37, 37, 38),
                        bold: true,
                        action_id: None,
                    }],
                    right_segments: vec![],
                };
                let _ = backend.draw_status_bar(header_bounds, &header_bar, None, None);
            }
        }

        if let Some(divider_bounds) = layout.divider_bounds {
            let row_text = " ".repeat(divider_bounds.width.ceil() as usize);
            let rows = (divider_bounds.height / lh).ceil() as usize;
            for row in 0..rows {
                let row_y = divider_bounds.y + row as f32 * lh;
                let row_rect = Rect::new(divider_bounds.x, row_y, divider_bounds.width, lh);
                let divider_bar = StatusBar {
                    id: WidgetId::new("app-shell:divider"),
                    left_segments: vec![StatusBarSegment {
                        text: row_text.clone(),
                        fg: Color::rgb(100, 100, 110),
                        bg: Color::rgb(100, 100, 110),
                        bold: false,
                        action_id: None,
                    }],
                    right_segments: vec![],
                };
                let _ = backend.draw_status_bar(row_rect, &divider_bar, None, None);
            }
        }

        layout
    }

    // ── Handle ───────────────────────────────────────────────────────

    /// Process a [`UiEvent`] and return a semantic [`AppShellEvent`].
    ///
    /// Call this before the consumer's own event handling. If the result
    /// is not [`AppShellEvent::Ignored`], the consumer should redraw.
    pub fn handle(&mut self, event: &UiEvent, backend: &dyn Backend, area: Rect) -> AppShellEvent {
        let lh = backend.line_height();
        let layout = self.compute_layout(area, lh);

        match event {
            UiEvent::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => {
                let p = *position;

                if let Some(div) = layout.divider_bounds {
                    if contains(div, p) {
                        let center_x = div.x + div.width / 2.0;
                        self.drag_offset = Some(p.x - center_x);
                        return AppShellEvent::Consumed;
                    }
                }

                if let Some(bp) = layout.bottom_panel_bounds {
                    // Grip sits *above* the panel's top edge only. The panel's
                    // top row is the tab strip, whose clicks must reach the
                    // bottom-panel controller — so the grip must not overlap it.
                    let grip_h = (lh * 0.5).max(1.0).round();
                    let grip = Rect::new(bp.x, bp.y - grip_h, bp.width, grip_h);
                    if contains(grip, p) {
                        self.bottom_panel_drag_offset = Some(p.y - bp.y);
                        return AppShellEvent::Consumed;
                    }
                }

                if contains(layout.activity_bar_bounds, p) {
                    if let Some(hit) = self.cached_activity_hit(p) {
                        return self.handle_activity_click(&hit);
                    }
                    return AppShellEvent::Consumed;
                }

                AppShellEvent::Ignored
            }

            UiEvent::MouseMoved {
                position,
                buttons:
                    ButtonMask {
                        left: true,
                        middle: _,
                        right: _,
                    },
            } => {
                if let Some(offset) = self.drag_offset {
                    let ab_edge = match self.position {
                        ShellPosition::Left => {
                            layout.activity_bar_bounds.x + layout.activity_bar_bounds.width
                        }
                        ShellPosition::Right => layout.activity_bar_bounds.x,
                    };
                    let new_native = match self.position {
                        ShellPosition::Left => position.x - offset - ab_edge,
                        ShellPosition::Right => ab_edge - position.x + offset,
                    };
                    let new_lh = new_native / lh;
                    let clamped = new_lh.clamp(self.min_sidebar_width, self.max_sidebar_width);
                    self.sidebar_width = clamped;
                    return AppShellEvent::SidebarResized {
                        new_width: clamped * lh,
                    };
                }
                if let Some(offset) = self.bottom_panel_drag_offset {
                    let bottom_edge = area.y + area.height;
                    let new_native = bottom_edge - (position.y - offset);
                    let status_lh = if self.has_status_bar { 1.0 } else { 0.0 };
                    let cmd_lh = if self.has_command_line { 1.0 } else { 0.0 };
                    let new_lh = new_native / lh - status_lh - cmd_lh;
                    let clamped = new_lh.clamp(
                        self.min_bottom_panel_height_lh,
                        self.max_bottom_panel_height_lh,
                    );
                    self.bottom_panel_height_lh = clamped;
                    return AppShellEvent::BottomPanelResized {
                        new_height: clamped * lh,
                    };
                }
                self.update_hover(position, &layout, lh);
                AppShellEvent::Ignored
            }

            UiEvent::MouseMoved { position, .. } => {
                self.update_hover(position, &layout, lh);
                AppShellEvent::Ignored
            }

            UiEvent::MouseUp {
                button: MouseButton::Left,
                ..
            } => {
                if self.drag_offset.take().is_some() {
                    return AppShellEvent::Consumed;
                }
                if self.bottom_panel_drag_offset.take().is_some() {
                    return AppShellEvent::Consumed;
                }
                AppShellEvent::Ignored
            }

            _ => AppShellEvent::Ignored,
        }
    }

    // ── Internal helpers ─────────────────────────────────────────────

    fn compute_layout(&self, area: Rect, line_height: f32) -> AppShellLayout {
        let lh = line_height.max(1.0);

        // ── Vertical carve: title bar (top), then status/cmd/bottom (bottom) ──

        let mut band_y = area.y;
        let mut band_h = area.height;

        let title_bar_bounds = if self.has_title_bar {
            let h = (self.title_bar_height_lh * lh).round();
            let r = Rect::new(area.x, band_y, area.width, h);
            band_y += h;
            band_h -= h;
            Some(r)
        } else {
            None
        };

        let status_bar_bounds = if self.has_status_bar {
            let h = lh.round();
            band_h -= h;
            Some(Rect::new(area.x, band_y + band_h, area.width, h))
        } else {
            None
        };

        let command_line_bounds = if self.has_command_line {
            let h = lh.round();
            band_h -= h;
            Some(Rect::new(area.x, band_y + band_h, area.width, h))
        } else {
            None
        };

        // Bottom panel height — computed here but *not* subtracted from the
        // band. The panel docks only under main content (full width minus the
        // sidebar), so it is carved from the main column after the horizontal
        // split below. The sidebar and activity bar run the full band height.
        let bottom_panel_h = if self.has_bottom_panel && self.bottom_panel_visible {
            let h = (self.bottom_panel_height_lh.clamp(
                self.min_bottom_panel_height_lh,
                self.max_bottom_panel_height_lh,
            ) * lh)
                .round();
            Some(h.min(band_h * 0.6))
        } else {
            None
        };

        let band_h = band_h.max(0.0);

        // Split a main-column rect into (content_above, bottom_panel).
        let carve_bottom_panel = |main: Rect| -> (Rect, Option<Rect>) {
            match bottom_panel_h {
                Some(h) => {
                    let h = h.min(main.height);
                    let above = Rect::new(main.x, main.y, main.width, (main.height - h).max(0.0));
                    let panel = Rect::new(main.x, main.y + main.height - h, main.width, h);
                    (above, Some(panel))
                }
                None => (main, None),
            }
        };

        // ── Horizontal carve: activity bar + sidebar + divider + main ──

        let ab_w = (self.activity_bar_width * lh).round();
        let divider_w = (lh * 0.25).max(1.0).round().min(4.0);

        if !self.sidebar_visible || self.panels.is_empty() {
            let (ab_bounds, main_bounds) = match self.position {
                ShellPosition::Left => (
                    Rect::new(area.x, band_y, ab_w, band_h),
                    Rect::new(area.x + ab_w, band_y, (area.width - ab_w).max(0.0), band_h),
                ),
                ShellPosition::Right => {
                    let ab_x = area.x + area.width - ab_w;
                    (
                        Rect::new(ab_x.max(area.x), band_y, ab_w, band_h),
                        Rect::new(area.x, band_y, (area.width - ab_w).max(0.0), band_h),
                    )
                }
            };
            let (main_bounds, bottom_panel_bounds) = carve_bottom_panel(main_bounds);
            return AppShellLayout {
                title_bar_bounds,
                activity_bar_bounds: ab_bounds,
                sidebar_header_bounds: None,
                sidebar_content_bounds: None,
                divider_bounds: None,
                main_content_bounds: main_bounds,
                bottom_panel_bounds,
                command_line_bounds,
                status_bar_bounds,
            };
        }

        let sidebar_w = (self
            .sidebar_width
            .clamp(self.min_sidebar_width, self.max_sidebar_width)
            * lh)
            .round();
        let remaining = (area.width - ab_w - divider_w).max(0.0);
        let sidebar_w = sidebar_w.min(remaining * 0.8);
        let header_h = lh;

        match self.position {
            ShellPosition::Left => {
                let ab_bounds = Rect::new(area.x, band_y, ab_w, band_h);
                let sidebar_x = area.x + ab_w;
                let header_bounds = Rect::new(sidebar_x, band_y, sidebar_w, header_h);
                let content_y = band_y + header_h;
                let content_h = (band_h - header_h).max(0.0);
                let content_bounds = Rect::new(sidebar_x, content_y, sidebar_w, content_h);
                let div_x = sidebar_x + sidebar_w;
                let div_bounds = Rect::new(div_x, band_y, divider_w, band_h);
                let main_x = div_x + divider_w;
                let main_w = (area.x + area.width - main_x).max(0.0);
                let main_bounds = Rect::new(main_x, band_y, main_w, band_h);
                let (main_bounds, bottom_panel_bounds) = carve_bottom_panel(main_bounds);

                AppShellLayout {
                    title_bar_bounds,
                    activity_bar_bounds: ab_bounds,
                    sidebar_header_bounds: Some(header_bounds),
                    sidebar_content_bounds: Some(content_bounds),
                    divider_bounds: Some(div_bounds),
                    main_content_bounds: main_bounds,
                    bottom_panel_bounds,
                    command_line_bounds,
                    status_bar_bounds,
                }
            }
            ShellPosition::Right => {
                let ab_x = area.x + area.width - ab_w;
                let ab_bounds = Rect::new(ab_x.max(area.x), band_y, ab_w, band_h);
                let sidebar_x = ab_x - sidebar_w;
                let header_bounds = Rect::new(sidebar_x.max(area.x), band_y, sidebar_w, header_h);
                let content_y = band_y + header_h;
                let content_h = (band_h - header_h).max(0.0);
                let content_bounds =
                    Rect::new(sidebar_x.max(area.x), content_y, sidebar_w, content_h);
                let div_x = sidebar_x - divider_w;
                let div_bounds = Rect::new(div_x.max(area.x), band_y, divider_w, band_h);
                let main_x = area.x;
                let main_w = (div_x - area.x).max(0.0);
                let main_bounds = Rect::new(main_x, band_y, main_w, band_h);
                let (main_bounds, bottom_panel_bounds) = carve_bottom_panel(main_bounds);

                AppShellLayout {
                    title_bar_bounds,
                    activity_bar_bounds: ab_bounds,
                    sidebar_header_bounds: Some(header_bounds),
                    sidebar_content_bounds: Some(content_bounds),
                    divider_bounds: Some(div_bounds),
                    main_content_bounds: main_bounds,
                    bottom_panel_bounds,
                    command_line_bounds,
                    status_bar_bounds,
                }
            }
        }
    }

    fn handle_activity_click(&mut self, clicked_id: &WidgetId) -> AppShellEvent {
        for (i, panel) in self.panels.iter().enumerate() {
            if panel.id == *clicked_id {
                if self.active_panel == Some(i) && self.sidebar_visible {
                    self.sidebar_visible = false;
                    return AppShellEvent::SidebarHidden;
                } else {
                    self.active_panel = Some(i);
                    self.sidebar_visible = true;
                    return AppShellEvent::PanelChanged {
                        panel_id: panel.id.clone(),
                    };
                }
            }
        }

        for item in &self.bottom_items {
            if item.id == *clicked_id {
                return AppShellEvent::BottomItemClicked {
                    id: item.id.clone(),
                };
            }
        }

        AppShellEvent::Consumed
    }

    fn cached_activity_hit(&self, position: Point) -> Option<WidgetId> {
        let ab = (*self.cached_activity_bar_bounds.borrow())?;
        let hits = self.cached_activity_hits.borrow();
        for hit in hits.iter() {
            if position.y >= hit.y_start as f32 + ab.y
                && position.y < hit.y_end as f32 + ab.y
                && position.x >= ab.x
                && position.x < ab.x + ab.width
            {
                return Some(hit.id.clone());
            }
        }
        None
    }

    fn update_hover(&mut self, position: &Point, layout: &AppShellLayout, _line_height: f32) {
        if contains(layout.activity_bar_bounds, *position) {
            let ab = layout.activity_bar_bounds;
            let hits = self.cached_activity_hits.borrow();
            self.hovered_activity_idx = hits.iter().position(|hit| {
                position.y >= hit.y_start as f32 + ab.y && position.y < hit.y_end as f32 + ab.y
            });
        } else {
            self.hovered_activity_idx = None;
        }
    }
}

fn contains(rect: Rect, point: Point) -> bool {
    point.x >= rect.x
        && point.x < rect.x + rect.width
        && point.y >= rect.y
        && point.y < rect.y + rect.height
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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

    fn shell() -> AppShell {
        AppShell::new(sample_panels(), 30.0).with_bottom_items(sample_bottom())
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
        let s = AppShell::new(vec![], 30.0);
        assert!(s.active_panel().is_none());
        assert!(s.sidebar_visible());
    }

    // ── Layout — sidebar visible ────────────────────────────────────

    #[test]
    fn layout_visible_left_bounds_sum_to_width() {
        let s = shell();
        let l = s.layout(area(), 1.0);
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
        let l = s.layout(area(), 1.0);
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
        let l = s.layout(area(), 1.0);
        let header = l.sidebar_header_bounds.unwrap();
        let content = l.sidebar_content_bounds.unwrap();
        assert_eq!(content.y, header.y + header.height);
        assert_eq!(content.width, header.width);
    }

    // ── Layout — sidebar hidden ─────────────────────────────────────

    #[test]
    fn layout_hidden_no_sidebar_bounds() {
        let mut s = shell();
        s.hide_sidebar();
        let l = s.layout(area(), 1.0);
        assert!(l.sidebar_header_bounds.is_none());
        assert!(l.sidebar_content_bounds.is_none());
        assert!(l.divider_bounds.is_none());
    }

    #[test]
    fn layout_hidden_main_fills_remaining() {
        let mut s = shell();
        s.hide_sidebar();
        let l = s.layout(area(), 1.0);
        let expected = area().width - s.activity_bar_width;
        assert!((l.main_content_bounds.width - expected).abs() < 0.01);
    }

    // ── Layout — Right position ─────────────────────────────────────

    #[test]
    fn layout_right_position_activity_bar_at_right_edge() {
        let s = shell().with_position(ShellPosition::Right);
        let l = s.layout(area(), 1.0);
        let ab_right = l.activity_bar_bounds.x + l.activity_bar_bounds.width;
        assert!((ab_right - area().width).abs() < 0.01);
    }

    #[test]
    fn layout_right_position_main_at_left_edge() {
        let s = shell().with_position(ShellPosition::Right);
        let l = s.layout(area(), 1.0);
        assert!((l.main_content_bounds.x - area().x).abs() < 0.01);
    }

    // ── Toggle state machine ────────────────────────────────────────

    #[test]
    fn toggle_click_active_icon_hides_sidebar() {
        let mut s = shell();
        assert!(s.sidebar_visible());
        let ev = s.handle_activity_click(&WidgetId::new("panel:explorer"));
        assert_eq!(ev, AppShellEvent::SidebarHidden);
        assert!(!s.sidebar_visible());
    }

    #[test]
    fn toggle_click_different_icon_switches_panel() {
        let mut s = shell();
        let ev = s.handle_activity_click(&WidgetId::new("panel:search"));
        assert_eq!(
            ev,
            AppShellEvent::PanelChanged {
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
            AppShellEvent::PanelChanged {
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
            AppShellEvent::BottomItemClicked {
                id: WidgetId::new("panel:settings")
            }
        );
        assert_eq!(s.sidebar_visible(), was_visible);
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
        let mut s = shell();
        s.activity_cursor = 1;
        // focused is false → no item should be selected
        let bar = s.build_activity_bar();
        assert!(bar.top_items.iter().all(|i| !i.is_keyboard_selected));
    }

    #[test]
    fn select_next_advances_cursor_through_top_then_bottom() {
        let mut s = shell(); // 3 top + 1 bottom
        s.set_activity_keyboard_focused(true);
        // cursor starts at 0
        s.activity_select_next(); // → 1
        assert_eq!(s.activity_cursor, 1);
        s.activity_select_next(); // → 2
        assert_eq!(s.activity_cursor, 2);
        s.activity_select_next(); // → 3 (first bottom item)
        assert_eq!(s.activity_cursor, 3);

        let bar = s.build_activity_bar();
        assert!(bar.bottom_items[0].is_keyboard_selected);
        assert!(bar.top_items.iter().all(|i| !i.is_keyboard_selected));
    }

    #[test]
    fn select_next_saturates_at_last_item() {
        let mut s = shell(); // total = 4
        s.set_activity_keyboard_focused(true);
        for _ in 0..10 {
            s.activity_select_next();
        }
        // Should saturate at 3 (last index: 3 top + 1 bottom − 1 = 3).
        assert_eq!(s.activity_cursor, 3);
    }

    #[test]
    fn select_prev_saturates_at_zero() {
        let mut s = shell();
        // cursor is already at 0
        s.activity_select_prev();
        assert_eq!(s.activity_cursor, 0);
        s.activity_select_prev();
        assert_eq!(s.activity_cursor, 0);
    }

    #[test]
    fn select_prev_moves_cursor_up() {
        let mut s = shell();
        s.activity_cursor = 3; // bottom item
        s.activity_select_prev();
        assert_eq!(s.activity_cursor, 2);
        s.activity_select_prev();
        assert_eq!(s.activity_cursor, 1);
        s.activity_select_prev();
        assert_eq!(s.activity_cursor, 0);
        s.activity_select_prev(); // saturates
        assert_eq!(s.activity_cursor, 0);
    }

    #[test]
    fn activity_set_cursor_clamps_to_valid_range() {
        let mut s = shell(); // 4 items total
        s.activity_set_cursor(100);
        assert_eq!(s.activity_cursor, 3);
        s.activity_set_cursor(0);
        assert_eq!(s.activity_cursor, 0);
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
        let mut s = shell();
        s.activity_cursor = 3; // first (and only) bottom item
        assert_eq!(
            s.activity_selected_id(),
            Some(&WidgetId::new("panel:settings"))
        );
    }

    #[test]
    fn activity_activate_selected_top_panel_emits_panel_changed() {
        let mut s = shell();
        s.activity_cursor = 1; // panel:search
        let ev = s.activity_activate_selected().expect("should return Some");
        assert_eq!(
            ev,
            AppShellEvent::PanelChanged {
                panel_id: WidgetId::new("panel:search")
            }
        );
        assert_eq!(s.active_panel_id(), Some(&WidgetId::new("panel:search")));
    }

    #[test]
    fn activity_activate_selected_bottom_item_emits_bottom_clicked() {
        let mut s = shell();
        s.activity_cursor = 3; // panel:settings
        let ev = s.activity_activate_selected().expect("should return Some");
        assert_eq!(
            ev,
            AppShellEvent::BottomItemClicked {
                id: WidgetId::new("panel:settings")
            }
        );
    }

    #[test]
    fn activity_activate_selected_toggle_hides_sidebar() {
        let mut s = shell();
        // cursor at 0 (panel:explorer), which is already the active panel
        let ev = s.activity_activate_selected().expect("should return Some");
        assert_eq!(ev, AppShellEvent::SidebarHidden);
        assert!(!s.sidebar_visible());
    }

    #[test]
    fn activity_activate_does_not_clear_keyboard_focus() {
        let mut s = shell();
        s.set_activity_keyboard_focused(true);
        s.activity_cursor = 1;
        let _ = s.activity_activate_selected();
        // Consumer decides whether to clear focus; AppShell must not.
        assert!(s.activity_keyboard_focused());
    }

    #[test]
    fn activity_activate_selected_returns_none_when_no_items() {
        let mut s = AppShell::new(vec![], 30.0);
        assert!(s.activity_activate_selected().is_none());
    }

    #[test]
    fn select_next_noop_on_empty_shell() {
        let mut s = AppShell::new(vec![], 30.0);
        s.activity_select_next(); // must not panic
        assert_eq!(s.activity_cursor, 0);
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
        let mut s = shell();
        s.set_sidebar_width(5.0);
        assert_eq!(s.sidebar_width(), s.min_sidebar_width);
        s.set_sidebar_width(9999.0);
        assert_eq!(s.sidebar_width(), s.max_sidebar_width);
    }

    // ── Resize drag ─────────────────────────────────────────────────

    #[test]
    fn resize_drag_updates_width() {
        let mut s = shell();
        let l = s.compute_layout(area(), 1.0);
        let div = l.divider_bounds.unwrap();
        let _center = div.x + div.width / 2.0;

        s.drag_offset = Some(0.0);
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
            &MockBackend,
            area(),
        );
        assert!(matches!(ev, AppShellEvent::SidebarResized { .. }));
        assert!((s.sidebar_width() - target_width).abs() < 1.0);
    }

    #[test]
    fn resize_drag_clamps_to_min() {
        let mut s = shell();
        s.drag_offset = Some(0.0);
        let ev = s.handle(
            &UiEvent::MouseMoved {
                position: Point::new(s.activity_bar_width + 1.0, 5.0),
                buttons: ButtonMask {
                    left: true,
                    middle: false,
                    right: false,
                },
            },
            &MockBackend,
            area(),
        );
        assert!(matches!(ev, AppShellEvent::SidebarResized { .. }));
        assert_eq!(s.sidebar_width(), s.min_sidebar_width);
    }

    #[test]
    fn mouse_up_ends_drag() {
        let mut s = shell();
        s.drag_offset = Some(0.0);
        let ev = s.handle(
            &UiEvent::MouseUp {
                button: MouseButton::Left,
                position: Point::new(50.0, 5.0),
                widget: None,
            },
            &MockBackend,
            area(),
        );
        assert_eq!(ev, AppShellEvent::Consumed);
        assert!(s.drag_offset.is_none());
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
        let mut s = AppShell::new(vec![], 30.0);
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
        let mut s = AppShell::new(
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
            AppShellEvent::PanelChanged {
                panel_id: WidgetId::new("panel:ext")
            }
        );
        assert_eq!(s.active_panel_id(), Some(&WidgetId::new("panel:ext")));
        // Toggle off.
        let ev = s.handle_activity_click(&WidgetId::new("panel:ext"));
        assert_eq!(ev, AppShellEvent::SidebarHidden);
    }

    // ── Ignored events ──────────────────────────────────────────────

    #[test]
    fn key_events_are_ignored() {
        let mut s = shell();
        let ev = s.handle(
            &UiEvent::KeyPressed {
                key: crate::Key::Char('q'),
                modifiers: Default::default(),
                repeat: false,
            },
            &MockBackend,
            area(),
        );
        assert_eq!(ev, AppShellEvent::Ignored);
    }

    // ── Chrome slot layout ─────────────────────────────────────────

    fn full_chrome_shell() -> AppShell {
        AppShell::new(sample_panels(), 30.0)
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
        let l = s.layout(area(), 1.0);
        assert!(l.title_bar_bounds.is_none());
        assert!(l.bottom_panel_bounds.is_none());
        assert!(l.command_line_bounds.is_none());
        assert!(l.status_bar_bounds.is_none());
    }

    #[test]
    fn chrome_slots_present_when_configured() {
        let s = full_chrome_shell();
        let l = s.layout(area(), 1.0);
        assert!(l.title_bar_bounds.is_some());
        assert!(l.bottom_panel_bounds.is_some());
        assert!(l.command_line_bounds.is_some());
        assert!(l.status_bar_bounds.is_some());
    }

    #[test]
    fn title_bar_at_top_edge() {
        let s = full_chrome_shell();
        let l = s.layout(area(), 1.0);
        let tb = l.title_bar_bounds.unwrap();
        assert_eq!(tb.y, 0.0);
        assert_eq!(tb.x, 0.0);
        assert_eq!(tb.width, area().width);
    }

    #[test]
    fn status_bar_at_bottom_edge() {
        let s = full_chrome_shell();
        let l = s.layout(area(), 1.0);
        let sb = l.status_bar_bounds.unwrap();
        let sb_bottom = sb.y + sb.height;
        assert!((sb_bottom - area().height).abs() < 0.01);
        assert_eq!(sb.width, area().width);
    }

    #[test]
    fn command_line_above_status_bar() {
        let s = full_chrome_shell();
        let l = s.layout(area(), 1.0);
        let cl = l.command_line_bounds.unwrap();
        let sb = l.status_bar_bounds.unwrap();
        assert!((cl.y + cl.height - sb.y).abs() < 0.01);
    }

    #[test]
    fn bottom_panel_above_command_line() {
        let s = full_chrome_shell();
        let l = s.layout(area(), 1.0);
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
        let l = s.layout(area(), 1.0);
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
        let l = s.layout(area(), 1.0);
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
        let l = s.layout(area(), 1.0);
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
        let l = s.layout(area(), 1.0);
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

    #[test]
    fn bottom_panel_resize_drag() {
        let mut s = full_chrome_shell();
        s.bottom_panel_drag_offset = Some(0.0);
        let ev = s.handle(
            &UiEvent::MouseMoved {
                position: Point::new(40.0, 18.0),
                buttons: ButtonMask {
                    left: true,
                    middle: false,
                    right: false,
                },
            },
            &MockBackend,
            area(),
        );
        assert!(matches!(ev, AppShellEvent::BottomPanelResized { .. }));
    }

    // ── Mock backend for handle() tests ─────────────────────────────

    struct MockBackend;

    impl Backend for MockBackend {
        fn viewport(&self) -> crate::Viewport {
            crate::Viewport {
                width: 80.0,
                height: 24.0,
                scale: 1.0,
            }
        }
        fn begin_frame(&mut self, _v: crate::Viewport) {}
        fn end_frame(&mut self) {}
        fn poll_events(&mut self) -> Vec<UiEvent> {
            Vec::new()
        }
        fn wait_events(&mut self, _t: std::time::Duration) -> Vec<UiEvent> {
            Vec::new()
        }
        fn register_accelerator(&mut self, _a: &crate::Accelerator) {}
        fn unregister_accelerator(&mut self, _id: &crate::AcceleratorId) {}
        fn modal_stack_mut(&mut self) -> &mut crate::ModalStack {
            unimplemented!()
        }
        fn services(&self) -> &dyn crate::backend::PlatformServices {
            unimplemented!()
        }
        fn line_height(&self) -> f32 {
            1.0
        }
        fn char_width(&self) -> f32 {
            1.0
        }
        fn draw_tree(&mut self, _r: Rect, _t: &crate::TreeView) {}
        fn draw_list(&mut self, _r: Rect, _l: &crate::ListView) {}
        fn draw_data_table(
            &mut self,
            _r: Rect,
            _t: &crate::DataTable,
            _h: Option<usize>,
        ) -> crate::DataTableLayout {
            unimplemented!()
        }
        fn data_table_layout(&self, _r: Rect, _t: &crate::DataTable) -> crate::DataTableLayout {
            unimplemented!()
        }
        fn list_hscrollbar(&self, _r: Rect, _l: &crate::ListView) -> Option<crate::Scrollbar> {
            None
        }
        fn list_vscrollbar(&self, _r: Rect, _l: &crate::ListView) -> Option<crate::Scrollbar> {
            None
        }
        fn draw_form(&mut self, _r: Rect, _f: &crate::Form) {}
        fn draw_palette(&mut self, _r: Rect, _p: &crate::Palette) {}
        fn draw_status_bar(
            &mut self,
            _r: Rect,
            _b: &StatusBar,
            _hovered_id: Option<&WidgetId>,
            _pressed_id: Option<&WidgetId>,
        ) -> crate::StatusBarLayout {
            crate::StatusBarLayout {
                bar_width: 0.0,
                bar_height: 0.0,
                visible_segments: Vec::new(),
                hit_regions: Vec::new(),
                resolved_right_start: 0,
            }
        }
        fn draw_tab_bar(
            &mut self,
            _r: Rect,
            _b: &crate::TabBar,
            _h: Option<usize>,
        ) -> crate::TabBarHits {
            crate::TabBarHits::default()
        }
        fn draw_activity_bar(
            &mut self,
            _r: Rect,
            _b: &ActivityBar,
            _h: Option<usize>,
        ) -> Vec<crate::ActivityBarRowHit> {
            Vec::new()
        }
        fn draw_terminal(&mut self, _r: Rect, _t: &crate::Terminal) {}
        fn draw_text_display(&mut self, _r: Rect, _t: &crate::TextDisplay) {}
        fn draw_command_line(&mut self, _r: Rect, _c: &crate::CommandLine) {}
        fn status_bar_layout(&self, _r: Rect, _b: &crate::StatusBar) -> crate::StatusBarLayout {
            crate::StatusBarLayout {
                bar_width: 0.0,
                bar_height: 0.0,
                visible_segments: Vec::new(),
                hit_regions: Vec::new(),
                resolved_right_start: 0,
            }
        }
        fn tab_bar_layout(&self, _r: Rect, _b: &crate::TabBar) -> crate::TabBarHits {
            crate::TabBarHits::default()
        }
        fn activity_bar_layout(
            &self,
            _r: Rect,
            _b: &crate::primitives::activity_bar::ActivityBar,
        ) -> Vec<crate::ActivityBarRowHit> {
            Vec::new()
        }
        fn text_display_layout(
            &self,
            _r: Rect,
            _t: &crate::TextDisplay,
        ) -> crate::TextDisplayLayout {
            unimplemented!()
        }
        fn draw_text_input(&mut self, _r: Rect, _t: &crate::TextInput) -> crate::TextInputLayout {
            unimplemented!()
        }
        fn text_input_layout(&self, _r: Rect, _t: &crate::TextInput) -> crate::TextInputLayout {
            unimplemented!()
        }
        fn draw_tooltip(&mut self, _t: &crate::Tooltip, _l: &crate::TooltipLayout) {}
        fn draw_context_menu(
            &mut self,
            _m: &crate::ContextMenu,
            _l: &crate::ContextMenuLayout,
        ) -> Vec<(Rect, WidgetId)> {
            Vec::new()
        }
        fn draw_dialog(&mut self, _d: &crate::Dialog, _l: &crate::DialogLayout) -> Vec<Rect> {
            Vec::new()
        }
        fn draw_multi_section_view(&mut self, _r: Rect, _v: &crate::MultiSectionView) {}
        fn msv_layout(
            &self,
            _r: Rect,
            _v: &crate::MultiSectionView,
        ) -> crate::MultiSectionViewLayout {
            unimplemented!()
        }
        fn msv_metrics(&self) -> crate::primitives::multi_section_view::LayoutMetrics {
            unimplemented!()
        }
        fn tree_layout(
            &self,
            _r: Rect,
            _t: &crate::TreeView,
        ) -> crate::primitives::tree::TreeViewLayout {
            unimplemented!()
        }
        fn form_layout(&self, _r: Rect, _f: &crate::Form) -> crate::primitives::form::FormLayout {
            unimplemented!()
        }
        fn draw_editor(
            &mut self,
            _r: Rect,
            _e: &crate::primitives::editor::Editor,
        ) -> crate::backend::EditorPaintResult {
            Default::default()
        }
        fn draw_message_list(
            &mut self,
            _r: Rect,
            _l: &crate::primitives::message_list::MessageList,
        ) {
        }
        fn draw_rich_text_popup(
            &mut self,
            _p: &crate::RichTextPopup,
            _l: &crate::primitives::rich_text_popup::RichTextPopupLayout,
        ) {
        }
        fn draw_find_replace(
            &mut self,
            _r: Rect,
            _p: &crate::primitives::find_replace::FindReplacePanel,
        ) {
        }
        fn draw_completions(
            &mut self,
            _c: &crate::Completions,
            _l: &crate::primitives::completions::CompletionsLayout,
        ) {
        }
        fn draw_scrollbar(&mut self, _r: Rect, _s: &crate::Scrollbar) {}
        fn draw_drop_overlay(&mut self, _o: &crate::primitives::drop_zone::DropOverlay) {}
        fn draw_menu_bar(&mut self, _r: Rect, _b: &crate::MenuBar) -> crate::MenuBarLayout {
            unimplemented!()
        }
        fn menu_bar_layout(&self, _r: Rect, _b: &crate::MenuBar) -> crate::MenuBarLayout {
            unimplemented!()
        }
        fn draw_split(&mut self, _r: Rect, _s: &crate::Split) -> crate::SplitLayout {
            unimplemented!()
        }
        fn split_layout(&self, _r: Rect, _s: &crate::Split) -> crate::SplitLayout {
            unimplemented!()
        }
        fn draw_panel(&mut self, _r: Rect, _p: &crate::Panel) -> crate::PanelLayout {
            unimplemented!()
        }
        fn panel_layout(&self, _r: Rect, _p: &crate::Panel) -> crate::PanelLayout {
            unimplemented!()
        }
        fn draw_toast_stack(
            &mut self,
            _r: Rect,
            _s: &crate::ToastStack,
        ) -> crate::ToastStackLayout {
            unimplemented!()
        }
        fn toast_stack_layout(&self, _r: Rect, _s: &crate::ToastStack) -> crate::ToastStackLayout {
            unimplemented!()
        }
        fn draw_pipeline_view(
            &mut self,
            _r: Rect,
            _v: &crate::PipelineView,
        ) -> crate::PipelineViewLayout {
            unimplemented!()
        }
        fn pipeline_view_layout(
            &self,
            _r: Rect,
            _v: &crate::PipelineView,
        ) -> crate::PipelineViewLayout {
            unimplemented!()
        }
        fn draw_progress(&mut self, _r: Rect, _b: &crate::ProgressBar) -> crate::ProgressBarLayout {
            unimplemented!()
        }
        fn progress_layout(&self, _r: Rect, _b: &crate::ProgressBar) -> crate::ProgressBarLayout {
            unimplemented!()
        }
        fn draw_spinner(&mut self, _r: Rect, _s: &crate::Spinner) -> crate::SpinnerLayout {
            unimplemented!()
        }
        fn spinner_layout(&self, _r: Rect, _s: &crate::Spinner) -> crate::SpinnerLayout {
            unimplemented!()
        }
        fn draw_command_center(
            &mut self,
            _r: Rect,
            _c: &crate::CommandCenter,
        ) -> crate::CommandCenterLayout {
            unimplemented!()
        }
        fn command_center_layout(
            &self,
            _r: Rect,
            _c: &crate::CommandCenter,
        ) -> crate::CommandCenterLayout {
            unimplemented!()
        }
        fn draw_chart(
            &mut self,
            _r: Rect,
            _c: &crate::primitives::chart::Chart,
            _h: Option<(usize, usize)>,
            _x: Option<f64>,
        ) -> crate::primitives::chart::ChartLayout {
            unimplemented!()
        }
        fn chart_layout(
            &self,
            _r: Rect,
            _c: &crate::primitives::chart::Chart,
        ) -> crate::primitives::chart::ChartLayout {
            unimplemented!()
        }
        fn draw_toolbar(
            &mut self,
            _r: Rect,
            _b: &crate::primitives::toolbar::Toolbar,
            _h: Option<&crate::types::WidgetId>,
            _p: Option<&crate::types::WidgetId>,
        ) -> crate::primitives::toolbar::ToolbarLayout {
            unimplemented!()
        }
        fn toolbar_layout(
            &self,
            _r: Rect,
            _b: &crate::primitives::toolbar::Toolbar,
        ) -> crate::primitives::toolbar::ToolbarLayout {
            unimplemented!()
        }
        fn draw_sidebar_panel(
            &mut self,
            _r: Rect,
            _p: &crate::primitives::sidebar_panel::SidebarPanel,
            _h: Option<&crate::types::WidgetId>,
            _pr: Option<&crate::types::WidgetId>,
        ) -> crate::primitives::sidebar_panel::SidebarPanelLayout {
            unimplemented!()
        }
        fn sidebar_panel_layout(
            &self,
            _r: Rect,
            _p: &crate::primitives::sidebar_panel::SidebarPanel,
        ) -> crate::primitives::sidebar_panel::SidebarPanelLayout {
            unimplemented!()
        }

        fn draw_diff_view(
            &mut self,
            _r: Rect,
            view: &crate::primitives::diff_view::DiffView,
        ) -> crate::primitives::diff_view::DiffViewLayout {
            crate::primitives::diff_view::DiffViewLayout {
                visible_rows: 0,
                total_rows: view.total_rows(),
            }
        }

        fn draw_board(
            &mut self,
            _r: Rect,
            _m: &crate::primitives::board::BoardModel,
        ) -> crate::primitives::board::BoardLayout {
            crate::primitives::board::BoardLayout {
                bounds: crate::event::Rect::new(_r.x, _r.y, _r.width, _r.height),
                columns: vec![],
            }
        }
    }
}
