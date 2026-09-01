//! Backend-agnostic app logic for the activity-bar keyboard-navigation demo
//! (`tui_activity_nav` / `gtk_activity_nav`).
//!
//! `ActivityNavApp` demonstrates the full keyboard-focus round-trip that
//! [`ActivityBar::is_keyboard_focused`] enables:
//!
//! 1. The "editor" area starts with focus (a simulated text cursor blinks).
//! 2. Press **Tab** → activity bar gains focus; cursor appears on the first
//!    item and the status bar shows "Activity bar focused".
//! 3. **j** / **↓** → cursor moves down the icon list.
//! 4. **k** / **↑** → cursor moves up.
//! 5. **l** / **Enter** → activates the highlighted item (prints its name in
//!    the status bar) and returns focus to the editor.
//! 6. **Esc** / **h** / **←** → dismisses the bar focus without activating.
//! 7. **q** → quit.
//!
//! Controls (when editor area is focused):
//!
//! | Key | Action |
//! |-----|--------|
//! | Tab | Focus the activity bar |
//! | q   | Quit |
//!
//! Controls (when activity bar is focused):
//!
//! | Key | Action |
//! |-----|--------|
//! | j / ↓ | Move cursor down |
//! | k / ↑ | Move cursor up |
//! | l / Enter | Activate item, return focus to editor |
//! | Esc / h / ← | Return focus to editor |

use quadraui::{
    ActivityBar, ActivityBarEvent, ActivityItem, AppLogic, Backend, Color, Key, NamedKey, Reaction,
    Rect, StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

/// Item descriptor for the demo bar.
struct NavItem {
    id: &'static str,
    icon: &'static str,
    label: &'static str,
}

const ITEMS: &[NavItem] = &[
    NavItem {
        id: "nav:explorer",
        icon: "⬡",
        label: "Explorer",
    },
    NavItem {
        id: "nav:search",
        icon: "⌕",
        label: "Search",
    },
    NavItem {
        id: "nav:git",
        icon: "⎇",
        label: "Source Control",
    },
    NavItem {
        id: "nav:debug",
        icon: "⬤",
        label: "Debug",
    },
    NavItem {
        id: "nav:extensions",
        icon: "⬡",
        label: "Extensions",
    },
];

pub struct ActivityNavApp {
    /// `true` when the activity bar has keyboard focus.
    bar_focused: bool,
    /// Index (into `ITEMS`) of the keyboard cursor inside the bar. Only
    /// meaningful while `bar_focused` is `true`.
    cursor: usize,
    /// Status-line message echoing the last action.
    message: String,
    /// Currently active (highlighted with accent) item index.
    active_idx: Option<usize>,
}

impl ActivityNavApp {
    pub fn new() -> Self {
        Self {
            bar_focused: false,
            cursor: 0,
            message: "Tab = focus bar  |  q = quit".into(),
            active_idx: Some(0),
        }
    }

    // ── Helpers ────────────────────────────────────────────────────────────

    fn bar(&self) -> ActivityBar {
        let items: Vec<ActivityItem> = ITEMS
            .iter()
            .enumerate()
            .map(|(i, ni)| ActivityItem {
                id: WidgetId::new(ni.id),
                icon: ni.icon.into(),
                tooltip: ni.label.into(),
                is_active: self.active_idx == Some(i),
                is_keyboard_selected: self.bar_focused && self.cursor == i,
            })
            .collect();

        ActivityBar {
            id: WidgetId::new("demo:activity-bar"),
            top_items: items,
            bottom_items: vec![],
            // JetBrains-style accent line. See issue #658 for the
            // VS-Code-style alternative (`active_bg` row fill).
            active_accent: Some(Color::rgb(100, 150, 255)),
            active_bg: None,
            selection_bg: Some(Color::rgb(70, 70, 100)),
            is_keyboard_focused: self.bar_focused,
        }
    }

    fn status_bar(&self) -> StatusBar {
        let left_text = format!("  {} ", self.message);
        let right_text = if self.bar_focused {
            " j/k=move  l/Enter=activate  Esc=back ".into()
        } else {
            " Tab=focus bar  q=quit ".into()
        };
        let left_bg = if self.bar_focused {
            Color::rgb(80, 60, 130)
        } else {
            Color::rgb(40, 80, 120)
        };
        StatusBar {
            id: WidgetId::new("demo:status"),
            left_segments: vec![StatusBarSegment {
                text: left_text,
                fg: Color::rgb(255, 255, 255),
                bg: left_bg,
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: right_text,
                fg: Color::rgb(200, 200, 200),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    fn move_cursor(&mut self, delta: i32) {
        let n = ITEMS.len() as i32;
        self.cursor = ((self.cursor as i32 + delta).rem_euclid(n)) as usize;
        self.message = format!("↕ cursor at {}", ITEMS[self.cursor].label);
    }

    fn activate(&mut self) {
        let label = ITEMS[self.cursor].label;
        self.active_idx = Some(self.cursor);
        self.message = format!("Activated: {}", label);
        self.bar_focused = false;
    }

    fn focus_bar(&mut self) {
        self.bar_focused = true;
        self.cursor = 0;
        self.message = format!("Activity bar focused  (cursor: {})", ITEMS[0].label);
    }

    fn focus_out(&mut self) {
        self.bar_focused = false;
        self.message = "Focus returned to editor".into();
    }

    /// Handle a key event while the activity bar is focused. Returns `true`
    /// if the event was consumed.
    fn handle_bar_key(&mut self, key_str: &str) -> bool {
        match key_str {
            "j" | "Down" => {
                self.move_cursor(1);
                true
            }
            "k" | "Up" => {
                self.move_cursor(-1);
                true
            }
            "l" | "Enter" => {
                self.activate();
                true
            }
            "Escape" | "h" | "Left" => {
                self.focus_out();
                true
            }
            _ => false,
        }
    }
}

impl Default for ActivityNavApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for ActivityNavApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        let lh = backend.line_height();

        // Activity bar: left strip, full height minus status bar.
        let bar_w = lh * 3.0;
        let bar_rect = Rect::new(0.0, 0.0, bar_w, vp.height - lh);
        let _ = backend.draw_activity_bar(bar_rect, &self.bar(), None);

        // "Editor" content area (placeholder).
        let editor_rect = Rect::new(bar_w, 0.0, vp.width - bar_w, vp.height - lh);
        let editor_label = if self.bar_focused {
            "  [ activity bar has focus — use j/k/l/Esc ]"
        } else {
            "  [ editor area has focus — press Tab to focus the bar ]"
        };
        let _ = backend.draw_status_bar(
            editor_rect,
            &StatusBar {
                id: WidgetId::new("demo:editor-area"),
                left_segments: vec![StatusBarSegment {
                    text: editor_label.into(),
                    fg: Color::rgb(180, 180, 180),
                    bg: Color::rgb(30, 30, 30),
                    bold: false,
                    action_id: None,
                }],
                right_segments: vec![],
            },
            None,
            None,
        );

        // Status bar at the bottom.
        let status_rect = Rect::new(0.0, vp.height - lh, vp.width, lh);
        let _ = backend.draw_status_bar(status_rect, &self.status_bar(), None, None);
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            // ── Quit ───────────────────────────────────────────────────────
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            }
            | UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } if !self.bar_focused => Reaction::Exit,

            // ── Tab → focus the activity bar ───────────────────────────────
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Tab),
                ..
            } if !self.bar_focused => {
                self.focus_bar();
                Reaction::Redraw
            }

            // ── Activity bar key events (emitted by the backend when
            //    is_keyboard_focused = true) ─────────────────────────────────
            UiEvent::ActivityBar(_id, ActivityBarEvent::KeyPressed { key, .. }) => {
                if self.handle_bar_key(&key) {
                    Reaction::Redraw
                } else {
                    Reaction::Continue
                }
            }

            // ── Activity bar click → activate immediately ──────────────────
            UiEvent::ActivityBar(_id, ActivityBarEvent::ItemClicked { id }) => {
                if let Some(idx) = ITEMS.iter().position(|ni| ni.id == id.as_str()) {
                    self.cursor = idx;
                    self.activate();
                    return Reaction::Redraw;
                }
                Reaction::Continue
            }

            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
