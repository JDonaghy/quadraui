//! `AppShell` — a composed controller for the activity bar + sidebar
//! panel container pattern (VS Code / JetBrains / Zellij-style app shell).
//!
//! Owns the full interaction state machine: activity bar click toggle,
//! active panel switching, sidebar visibility, and resize drag. Apps
//! register panels at construction time, match on [`AppShellEvent`]
//! for semantic actions, and paint their panel content into the bounds
//! the shell returns.
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
    BottomItemClicked { id: WidgetId },
    Consumed,
    Ignored,
}

/// Layout bounds returned by [`AppShell::render`] and [`AppShell::layout`].
#[derive(Debug, Clone, PartialEq)]
pub struct AppShellLayout {
    pub activity_bar_bounds: Rect,
    pub sidebar_header_bounds: Option<Rect>,
    pub sidebar_content_bounds: Option<Rect>,
    pub divider_bounds: Option<Rect>,
    pub main_content_bounds: Rect,
}

// ── AppShell ─────────────────────────────────────────────────────────

/// All width fields are stored as **multiples of `line_height`** so
/// they are portable across TUI (cells) and GUI (pixels) backends.
/// `compute_layout()` resolves them: `width_native = field * lh`.
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
    /// Cached hit regions from the last `render()` call. `handle()`
    /// dispatches clicks against these so paint and click agree on
    /// row positions — the structural fix for the GTK ACTIVITY_ROW_PX
    /// vs line_height mismatch (LESSONS.md: one derivation, two consumers).
    /// `RefCell` because `render(&self)` is called from `AppLogic::render`
    /// which takes `&self`.
    cached_activity_hits: RefCell<Vec<ActivityBarRowHit>>,
    cached_activity_bar_bounds: RefCell<Option<Rect>>,
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
            cached_activity_hits: RefCell::new(Vec::new()),
            cached_activity_bar_bounds: RefCell::new(None),
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

    // ── Layout ───────────────────────────────────────────────────────

    /// Compute layout bounds without drawing. Use when only hit-testing
    /// is needed (e.g. in `handle`).
    pub fn layout(&self, area: Rect, line_height: f32) -> AppShellLayout {
        self.compute_layout(area, line_height)
    }

    /// Build the [`ActivityBar`] primitive from current panel state.
    pub fn build_activity_bar(&self) -> ActivityBar {
        let top_items: Vec<ActivityItem> = self
            .panels
            .iter()
            .enumerate()
            .map(|(i, p)| ActivityItem {
                id: p.id.clone(),
                icon: p.icon.clone(),
                tooltip: p.tooltip.clone(),
                is_active: self.active_panel == Some(i) && self.sidebar_visible,
                is_keyboard_selected: false,
            })
            .collect();

        let bottom_items: Vec<ActivityItem> = self
            .bottom_items
            .iter()
            .map(|p| ActivityItem {
                id: p.id.clone(),
                icon: p.icon.clone(),
                tooltip: p.tooltip.clone(),
                is_active: false,
                is_keyboard_selected: false,
            })
            .collect();

        ActivityBar {
            id: WidgetId::new("app-shell:activity-bar"),
            top_items,
            bottom_items,
            active_accent: None,
            selection_bg: None,
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
            let divider_bar = StatusBar {
                id: WidgetId::new("app-shell:divider"),
                left_segments: vec![StatusBarSegment {
                    text: " ".repeat(divider_bounds.width.ceil() as usize),
                    fg: Color::rgb(60, 60, 60),
                    bg: Color::rgb(60, 60, 60),
                    bold: false,
                    action_id: None,
                }],
                right_segments: vec![],
            };
            let _ = backend.draw_status_bar(divider_bounds, &divider_bar, None, None);
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
                AppShellEvent::Ignored
            }

            _ => AppShellEvent::Ignored,
        }
    }

    // ── Internal helpers ─────────────────────────────────────────────

    fn compute_layout(&self, area: Rect, line_height: f32) -> AppShellLayout {
        let lh = line_height.max(1.0);
        let ab_w = (self.activity_bar_width * lh).round();
        let divider_w = (lh * 0.25).max(1.0).round().min(4.0);

        if !self.sidebar_visible || self.panels.is_empty() {
            let (ab_bounds, main_bounds) = match self.position {
                ShellPosition::Left => (
                    Rect::new(area.x, area.y, ab_w, area.height),
                    Rect::new(
                        area.x + ab_w,
                        area.y,
                        (area.width - ab_w).max(0.0),
                        area.height,
                    ),
                ),
                ShellPosition::Right => {
                    let ab_x = area.x + area.width - ab_w;
                    (
                        Rect::new(ab_x.max(area.x), area.y, ab_w, area.height),
                        Rect::new(area.x, area.y, (area.width - ab_w).max(0.0), area.height),
                    )
                }
            };
            return AppShellLayout {
                activity_bar_bounds: ab_bounds,
                sidebar_header_bounds: None,
                sidebar_content_bounds: None,
                divider_bounds: None,
                main_content_bounds: main_bounds,
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
                let ab_bounds = Rect::new(area.x, area.y, ab_w, area.height);
                let sidebar_x = area.x + ab_w;
                let header_bounds = Rect::new(sidebar_x, area.y, sidebar_w, header_h);
                let content_y = area.y + header_h;
                let content_h = (area.height - header_h).max(0.0);
                let content_bounds = Rect::new(sidebar_x, content_y, sidebar_w, content_h);
                let div_x = sidebar_x + sidebar_w;
                let div_bounds = Rect::new(div_x, area.y, divider_w, area.height);
                let main_x = div_x + divider_w;
                let main_w = (area.x + area.width - main_x).max(0.0);
                let main_bounds = Rect::new(main_x, area.y, main_w, area.height);

                AppShellLayout {
                    activity_bar_bounds: ab_bounds,
                    sidebar_header_bounds: Some(header_bounds),
                    sidebar_content_bounds: Some(content_bounds),
                    divider_bounds: Some(div_bounds),
                    main_content_bounds: main_bounds,
                }
            }
            ShellPosition::Right => {
                let ab_x = area.x + area.width - ab_w;
                let ab_bounds = Rect::new(ab_x.max(area.x), area.y, ab_w, area.height);
                let sidebar_x = ab_x - sidebar_w;
                let header_bounds = Rect::new(sidebar_x.max(area.x), area.y, sidebar_w, header_h);
                let content_y = area.y + header_h;
                let content_h = (area.height - header_h).max(0.0);
                let content_bounds =
                    Rect::new(sidebar_x.max(area.x), content_y, sidebar_w, content_h);
                let div_x = sidebar_x - divider_w;
                let div_bounds = Rect::new(div_x.max(area.x), area.y, divider_w, area.height);
                let main_x = area.x;
                let main_w = (div_x - area.x).max(0.0);
                let main_bounds = Rect::new(main_x, area.y, main_w, area.height);

                AppShellLayout {
                    activity_bar_bounds: ab_bounds,
                    sidebar_header_bounds: Some(header_bounds),
                    sidebar_content_bounds: Some(content_bounds),
                    divider_bounds: Some(div_bounds),
                    main_content_bounds: main_bounds,
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
        fn text_display_layout(
            &self,
            _r: Rect,
            _t: &crate::TextDisplay,
        ) -> crate::TextDisplayLayout {
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
    }
}
