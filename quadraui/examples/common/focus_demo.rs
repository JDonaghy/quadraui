//! Demo for the runner-owned [`quadraui::FocusManager`] (issue #830):
//! Tab / Shift+Tab cycle keyboard focus through a multi-widget screen,
//! and the backend paints a focus ring around whichever widget currently
//! owns it.
//!
//! The point of the demo is that **the app maintains no focus state of
//! its own**. It doesn't track "which pane is active", it doesn't have a
//! `FocusRing`, and it never sets `has_focus` on a primitive. All it does
//! is:
//!
//! 1. hand the runner this frame's tab order via
//!    [`AppLogic::tab_stops`] — which is literally
//!    [`ScreenLayout::tab_stops`]'s output, cached at paint time the same
//!    way `frame_demo` caches its [`FrameHitMap`]; and
//! 2. redraw when [`UiEvent::FocusChanged`] arrives.
//!
//! Everything else — cycling, wrap-around, "which rect does the ring go
//! around" — is the shared runner pipeline's job, identically on every
//! backend.
//!
//! Layout: two side-by-side lists on the top row plus a status bar along
//! the bottom, i.e. three tab stops whose rects are pairwise distinct in
//! *both* axes, so the ring visibly moves right, then down, then wraps.
//!
//! One deliberate placement: the "which widget is focused" echo lives on an
//! **interior row of the left list**, not in the status bar. The ring is
//! border-only, which means a widget only one or two rows tall (a TUI status
//! bar is exactly that) is *entirely* border while focused — its own text is
//! covered. Putting the echo on an interior row keeps it readable in every
//! focus state, and doubles as the demo's proof that the ring overlays
//! rather than clears.

use std::cell::RefCell;

use quadraui::{
    AppLogic, Backend, Color, Key, ListItem, ListView, NamedKey, Reaction, Rect, ScreenLayout,
    StatusBar, StatusBarSegment, StyledText, Surface, UiEvent, WidgetId,
};

/// Id of the left-hand list — the first tab stop in reading order.
pub const LEFT_ID: &str = "left";
/// Id of the right-hand list — the second tab stop.
pub const RIGHT_ID: &str = "right";
/// Id of the status bar — the third and last tab stop.
pub const STATUS_ID: &str = "status";

pub struct FocusDemo {
    right_items: Vec<String>,
    /// Last [`UiEvent::FocusChanged`] payload, echoed in the status bar so
    /// the event is observable in text as well as in the painted ring.
    focused: Option<WidgetId>,
    /// This frame's tab order, cached at paint time. `AppLogic::tab_stops`
    /// is `&self`-only and gets no backend, so — exactly like
    /// `frame_demo`'s `cached_hit_map` — the geometry-derived answer is
    /// computed inside `render` (where the viewport *is* available) and
    /// read back out here.
    cached_tab_stops: RefCell<Vec<(WidgetId, Rect)>>,
}

impl FocusDemo {
    pub fn new() -> Self {
        Self {
            right_items: vec!["one".into(), "two".into(), "three".into()],
            focused: None,
            cached_tab_stops: RefCell::new(Vec::new()),
        }
    }

    /// The left list's rows. Row 0 is a header (the ring's top border sits
    /// on it while this list is focused); row 1 is the live focus echo, on
    /// an interior row so no ring ever covers it — see the module doc.
    fn left_items(&self) -> Vec<String> {
        vec![
            "── focus ──".to_string(),
            self.focus_label(),
            "alpha".to_string(),
            "bravo".to_string(),
            "charlie".to_string(),
        ]
    }

    /// The id the runner currently reports as focused, rendered for the
    /// status bar. Deliberately reads the app's echo of the last
    /// `FocusChanged` rather than a second source of truth.
    fn focus_label(&self) -> String {
        match &self.focused {
            Some(id) => format!("focus: {}", id.as_str()),
            None => "focus: none".to_string(),
        }
    }

    fn list(&self, id: &str, items: &[String]) -> ListView {
        ListView {
            id: WidgetId::new(id),
            title: None,
            items: items
                .iter()
                .map(|name| ListItem {
                    text: StyledText::plain(name),
                    detail: None,
                    icon: None,
                    decoration: Default::default(),
                })
                .collect(),
            selected_idx: 0,
            scroll_offset: 0,
            // #830: deliberately `false` on both lists. Keyboard focus is
            // the runner's `FocusManager`, not a per-primitive bool — the
            // ring is what shows which one has it.
            has_focus: false,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
        }
    }

    fn status_bar(&self) -> StatusBar {
        let fg = Color::rgb(220, 220, 220);
        let bg = Color::rgb(40, 40, 60);
        StatusBar {
            id: WidgetId::new(STATUS_ID),
            left_segments: vec![StatusBarSegment {
                text: " quadraui #830 — runner-owned FocusManager ".to_string(),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " Tab / Shift-Tab = move focus | q = quit ".into(),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for FocusDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for FocusDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        let lh = backend.line_height();
        let status_h = (lh * 1.5).round();
        let body_h = (vp.height - status_h).max(0.0);
        let half_w = (vp.width / 2.0).round();

        let left = self.list(LEFT_ID, &self.left_items());
        let right = self.list(RIGHT_ID, &self.right_items);
        let status = self.status_bar();

        let mut frame = ScreenLayout::new();
        frame.push(Surface::List {
            rect: Rect::new(0.0, 0.0, half_w, body_h),
            list: &left,
        });
        frame.push(Surface::List {
            rect: Rect::new(half_w, 0.0, vp.width - half_w, body_h),
            list: &right,
        });
        frame.push(Surface::StatusBar {
            rect: Rect::new(0.0, body_h, vp.width, status_h),
            bar: &status,
            hovered: None,
            pressed: None,
        });

        // #830: the tab order is *derived from the layout* — the app never
        // hand-maintains a widget list. Cache it for `tab_stops` below.
        *self.cached_tab_stops.borrow_mut() = frame.tab_stops();

        frame.draw(backend);
    }

    fn tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
        self.cached_tab_stops.borrow().clone()
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            // #830: the app's entire focus responsibility. No cycling, no
            // wrap-around arithmetic, no per-primitive `has_focus` bools.
            UiEvent::FocusChanged(id) => {
                self.focused = id;
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('q') | Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            _ => Reaction::Continue,
        }
    }
}
