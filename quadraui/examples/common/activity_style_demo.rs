//! Minimal `AppLogic` painting an `ActivityBar` whose active item shows a
//! VS-Code-style row fill via [`ActivityBarStyle::active_bg`] (quadraui#658)
//! instead of the left-edge accent line `ActivityNavApp` (`tui_activity_nav`
//! / `gtk_activity_nav`) demonstrates.
//!
//! The point of the demo is the **sidecar**: the fill is requested through
//! [`Backend::draw_activity_bar_with_style`] — never a field on
//! [`ActivityBar`] itself, which is exactly the break #658 exists to avoid
//! (see [`quadraui::ActivityBarStyle`]'s doc for the full reasoning).
//! `active_accent` stays `None` throughout, so there is **zero**
//! accent-line rendering; the row fill is the only active-item indicator.
//!
//! Click an icon (or press **1**/**2**/**3**) to activate it, **q** to quit.

use quadraui::{
    ActivityBar, ActivityBarStyle, ActivityItem, AppLogic, Backend, Color, Key, Reaction, Rect,
    StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

/// Item labels, index-stable across activation (only `is_active` moves).
pub const LABELS: [&str; 3] = ["Explorer", "Search", "Source Control"];
const ICONS: [&str; 3] = ["E", "S", "G"];

fn item_id(i: usize) -> WidgetId {
    WidgetId::new(format!("activity-style-demo:{i}"))
}

pub struct ActivityStyleDemo {
    active: usize,
    last_action: String,
}

impl ActivityStyleDemo {
    pub fn new() -> Self {
        Self {
            active: 0,
            last_action: "ready".to_string(),
        }
    }

    /// `active_accent: None` — zero accent-line pixels. The row fill in
    /// [`Self::style`] is the only active-item indicator (#658).
    fn bar(&self) -> ActivityBar {
        let items = ICONS
            .iter()
            .enumerate()
            .map(|(i, icon)| ActivityItem {
                id: item_id(i),
                icon: (*icon).into(),
                tooltip: LABELS[i].to_string(),
                is_active: i == self.active,
                is_keyboard_selected: false,
            })
            .collect();
        ActivityBar {
            id: WidgetId::new("activity-style-demo:bar"),
            top_items: items,
            bottom_items: vec![],
            active_accent: None,
            selection_bg: None,
            is_keyboard_focused: false,
        }
    }

    /// VS-Code-style soft chip fill — the whole point of the demo.
    fn style(&self) -> ActivityBarStyle {
        ActivityBarStyle::new().with_active_bg(Color::rgb(49, 50, 51))
    }

    fn bar_rect(&self, backend: &dyn Backend) -> Rect {
        let vp = backend.viewport();
        let lh = backend.line_height();
        Rect::new(0.0, 0.0, lh * 3.0, vp.height - lh)
    }

    fn hint_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("activity-style-demo:hint"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.last_action),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: true,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " 1/2/3 = activate · q = quit ".to_string(),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    fn activate(&mut self, idx: usize) {
        self.active = idx;
        self.last_action = format!("activated {}", LABELS[idx]);
    }
}

impl Default for ActivityStyleDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for ActivityStyleDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        let lh = backend.line_height();
        let bar_rect = self.bar_rect(backend);
        let _ = backend.draw_activity_bar_with_style(bar_rect, &self.bar(), None, &self.style());
        let _ = backend.draw_status_bar(
            Rect::new(0.0, vp.height - lh, vp.width, lh),
            &self.hint_bar(),
            None,
            None,
        );
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            } => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Char(c @ '1'..='3'),
                ..
            } => {
                self.activate((c as usize) - ('1' as usize));
                Reaction::Redraw
            }
            UiEvent::MouseDown { position, .. } => {
                let bar_rect = self.bar_rect(backend);
                if position.x < bar_rect.x
                    || position.x >= bar_rect.x + bar_rect.width
                    || position.y < bar_rect.y
                    || position.y >= bar_rect.y + bar_rect.height
                {
                    return Reaction::Continue;
                }
                let hits = backend.activity_bar_layout(bar_rect, &self.bar());
                let rel_y = position.y - bar_rect.y;
                for hit in &hits {
                    if (rel_y as f64) >= hit.y_start && (rel_y as f64) < hit.y_end {
                        if let Some(idx) = (0..LABELS.len()).find(|&i| hit.id == item_id(i)) {
                            self.activate(idx);
                            return Reaction::Redraw;
                        }
                    }
                }
                Reaction::Continue
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
