//! Backend-agnostic app code for the richer "demo" example
//! ([`tui_demo`] / [`gtk_demo`]).
//!
//! Holds [`AppState`] (tabs + status focus + last message), the
//! state→primitive builders for [`crate::common::demo::build_tab_bar`]
//! and [`crate::common::demo::build_status_bar`], the focused-segment
//! and action-dispatch helpers, and the [`AppLogic`] impl that ties
//! them together. Identical app code runs on both backends through
//! the [`quadraui::tui::run`] / [`quadraui::gtk::run`] runners.

use std::cell::Cell;

use quadraui::{
    AppLogic, Backend, Color, Key, NamedKey, Reaction, Rect, StatusBar, StatusBarSegment, TabBar,
    TabBarSegment, TabItem, UiEvent, WidgetId,
};

// ─── App state ───────────────────────────────────────────────────────────────

/// All persistent state lives here. Each frame:
/// 1. We build quadraui primitives FROM this state.
/// 2. We render the primitives.
/// 3. Events MUTATE this state.
pub struct AppState {
    pub tabs: Vec<String>,
    pub active_tab: usize,
    /// Authoritative tab scroll offset. The `TabBar` primitive's
    /// `scroll_offset` field is the *input* per frame — this field is
    /// where we store the value the backend computed and wrote back via
    /// the `TabBar` contract. Wrapped in `Cell` so we can update it
    /// from inside `AppLogic::render(&self, ...)` after consuming the
    /// trait method's `TabBarHits::correct_scroll_offset`.
    pub tab_scroll_offset: Cell<usize>,
    /// Which clickable status-bar segment currently has keyboard focus
    /// (highlighted). Tab cycles through the visible right segments.
    pub focused_status_idx: usize,
    /// Last action triggered, shown in the status message.
    pub last_message: String,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            tabs: vec![
                "main.rs".into(),
                "lib.rs".into(),
                "Cargo.toml".into(),
                "README.md".into(),
                "tests.rs".into(),
            ],
            active_tab: 0,
            tab_scroll_offset: Cell::new(0),
            focused_status_idx: 0,
            last_message: "ready".into(),
        }
    }

    pub fn next_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active_tab = (self.active_tab + 1) % self.tabs.len();
        }
    }
    pub fn prev_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active_tab = (self.active_tab + self.tabs.len() - 1) % self.tabs.len();
        }
    }
    pub fn open_tab(&mut self) {
        let n = self.tabs.len() + 1;
        self.tabs.push(format!("scratch-{n}.txt"));
        self.active_tab = self.tabs.len() - 1;
    }
    pub fn close_active(&mut self) {
        if self.tabs.len() > 1 {
            self.tabs.remove(self.active_tab);
            if self.active_tab >= self.tabs.len() {
                self.active_tab = self.tabs.len() - 1;
            }
        }
    }
    /// Number of clickable right-side status segments at the current
    /// state. Used to bound `focused_status_idx` cycling.
    pub fn interactive_status_count(&self) -> usize {
        build_status_bar(self, None)
            .right_segments
            .iter()
            .filter(|s| s.action_id.is_some())
            .count()
    }
    /// Move status-segment focus by `delta` (`+1` Tab, `-1` Shift-Tab),
    /// wrapping around.
    pub fn cycle_status_focus(&mut self, delta: isize) {
        let count = self.interactive_status_count();
        if count == 0 {
            return;
        }
        let i = self.focused_status_idx as isize + delta;
        let len = count as isize;
        self.focused_status_idx = (((i % len) + len) % len) as usize;
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Primitive builders (state → primitive) ──────────────────────────────────

pub fn build_tab_bar(state: &AppState) -> TabBar {
    let tabs = state
        .tabs
        .iter()
        .enumerate()
        .map(|(i, name)| TabItem {
            label: format!(" {}: {} ", i + 1, name),
            is_active: i == state.active_tab,
            is_dirty: false,
            is_preview: false,
        })
        .collect();
    TabBar {
        id: WidgetId::new("tabs:editor"),
        tabs,
        scroll_offset: state.tab_scroll_offset.get(),
        right_segments: vec![TabBarSegment {
            text: " + ".into(),
            width_cells: 3,
            id: Some(WidgetId::new("tab:new")),
            is_active: false,
        }],
        active_accent: Some(Color::rgb(80, 160, 240)),
    }
}

pub fn build_status_bar(state: &AppState, focused_id: Option<&str>) -> StatusBar {
    let bar_fg = Color::rgb(220, 220, 220);
    let bar_bg = Color::rgb(40, 40, 60);
    let mode_fg = Color::rgb(80, 200, 120);

    let make = |text: String, action: Option<&str>| StatusBarSegment {
        text,
        fg: bar_fg,
        bg: bar_bg,
        bold: false,
        action_id: action.map(WidgetId::new),
    };

    let left = vec![
        StatusBarSegment {
            text: " DEMO ".into(),
            fg: mode_fg,
            bg: bar_bg,
            bold: true,
            action_id: None,
        },
        make(
            format!(
                " {} ",
                state
                    .tabs
                    .get(state.active_tab)
                    .cloned()
                    .unwrap_or_default()
            ),
            None,
        ),
    ];

    // Right segments — built least-important first, most-important last.
    // `fit_right_start` drops from the front when the bar is narrow,
    // so cursor position (the rightmost segment) always stays visible.
    // See `StatusBar`'s "Backend contract" rustdoc.
    let mut right = vec![
        make(format!(" {} ", state.last_message), Some("status:dismiss")),
        make(" UTF-8 ".into(), Some("status:encoding")),
        make(" LF ".into(), Some("status:line_ending")),
        make(
            format!(" Tab {} ", state.active_tab + 1),
            Some("status:goto"),
        ),
        make(" rust ".into(), Some("status:language")),
    ];

    // Per-frame interaction state passed alongside the primitive — the
    // focused segment gets a bold style. Backend-owned, not stored on
    // the primitive struct (see `BACKEND.md` §6, "ActivityBar contract"
    // for the same pattern with hover state).
    if let Some(fid) = focused_id {
        for seg in &mut right {
            if let Some(ref id) = seg.action_id {
                if id.as_str() == fid {
                    seg.bold = true;
                }
            }
        }
    }

    StatusBar {
        id: WidgetId::new("status:editor"),
        left_segments: left,
        right_segments: right,
    }
}

// ─── Event dispatch (event → state mutation) ─────────────────────────────────

/// Resolve which status segment currently has keyboard focus. Returns
/// `None` if none of the visible segments are interactive (shouldn't
/// happen for this demo, defensive).
pub fn focused_segment_id(state: &AppState) -> Option<String> {
    let bar = build_status_bar(state, None);
    let interactive: Vec<&StatusBarSegment> = bar
        .right_segments
        .iter()
        .filter(|s| s.action_id.is_some())
        .collect();
    interactive
        .get(state.focused_status_idx)
        .and_then(|s| s.action_id.as_ref())
        .map(|id| id.as_str().to_string())
}

/// Dispatch a status-segment activation. In a real app the action_id
/// would map to a function pointer or enum variant; the demo just
/// updates the last_message for visual feedback.
pub fn handle_status_action(state: &mut AppState, id: &str) {
    state.last_message = match id {
        "status:dismiss" => "dismissed".into(),
        "status:encoding" => "encoding picker (mock)".into(),
        "status:line_ending" => "line-ending picker (mock)".into(),
        "status:goto" => format!("go-to-tab picker (active = {})", state.active_tab + 1),
        "status:language" => "language picker (mock)".into(),
        _ => format!("unknown action: {}", id),
    };
}

// ─── AppLogic impl ───────────────────────────────────────────────────────────

impl AppLogic for AppState {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        // Tab bar at the top.
        let tab_bar = build_tab_bar(self);
        let tab_h = 28.0; // hint — actual paint height = backend.current_line_height
        let tab_rect = Rect::new(0.0, 0.0, viewport.width, tab_h);
        let hits = backend.draw_tab_bar(tab_rect, &tab_bar, None);
        // Per the TabBar contract, write the corrected scroll offset
        // back so the next frame paints with the right starting tab.
        // Differs from this frame's input only on resize / tab open /
        // tab close transitions.
        if hits.correct_scroll_offset != self.tab_scroll_offset.get() {
            self.tab_scroll_offset.set(hits.correct_scroll_offset);
        }
        // Status bar at the bottom — mirrors the placement in the
        // minimal apps and the established vim convention.
        let focused = focused_segment_id(self);
        let status_bar = build_status_bar(self, focused.as_deref());
        let status_h = 28.0;
        let status_rect = Rect::new(0.0, viewport.height - status_h, viewport.width, status_h);
        let _hits = backend.draw_status_bar(status_rect, &status_bar);
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        let UiEvent::KeyPressed { key, .. } = event else {
            return match event {
                UiEvent::WindowResized { .. } => Reaction::Redraw,
                _ => Reaction::Continue,
            };
        };
        match key {
            Key::Char('q') | Key::Named(NamedKey::Escape) => Reaction::Exit,
            Key::Named(NamedKey::Right) => {
                self.next_tab();
                Reaction::Redraw
            }
            Key::Named(NamedKey::Left) => {
                self.prev_tab();
                Reaction::Redraw
            }
            Key::Char('n') => {
                self.open_tab();
                Reaction::Redraw
            }
            Key::Char('x') => {
                self.close_active();
                Reaction::Redraw
            }
            Key::Named(NamedKey::Tab) => {
                self.cycle_status_focus(1);
                Reaction::Redraw
            }
            Key::Named(NamedKey::BackTab) => {
                self.cycle_status_focus(-1);
                Reaction::Redraw
            }
            Key::Named(NamedKey::Enter) => {
                if let Some(id) = focused_segment_id(self) {
                    handle_status_action(self, &id);
                }
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }
}
