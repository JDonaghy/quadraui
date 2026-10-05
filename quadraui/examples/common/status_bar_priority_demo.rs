//! Backend-agnostic app code for the status-bar priority-drop example
//! ([`tui_status_bar_priority_demo`] / [`gtk_status_bar_priority_demo`]).
//!
//! [`StatusBarPriorityDemo`] renders a [`StatusBar`] whose `right_segments`
//! follow the documented convention — least-important first, the
//! cursor-position segment *last* — and lets `d` toggle a "dirty" state
//! that grows the left segment, the same shape as an editor's `[+]`
//! unsaved-changes badge. At the demo's chosen width, toggling dirty
//! forces the bar's priority-drop to shed a low-priority right segment,
//! while the cursor-position segment keeps painting throughout.
//!
//! Controls:
//! - d       toggle the simulated "dirty" (unsaved changes) state
//! - q / Esc quit

use quadraui::{
    AppLogic, Backend, Color, InteractionState, Key, NamedKey, Reaction, Rect, StatusBar,
    StatusBarSegment, UiEvent, WidgetId,
};

pub struct StatusBarPriorityDemo {
    dirty: bool,
}

impl StatusBarPriorityDemo {
    pub fn new() -> Self {
        Self { dirty: false }
    }

    fn status_bar(&self) -> StatusBar {
        let mode_text = if self.dirty {
            // Simulates an editor's "[+]" unsaved-changes badge growing
            // the left segment enough to tip this bar's priority-drop.
            format!("NORMAL{}", " [+]".repeat(60))
        } else {
            "NORMAL".to_string()
        };
        let plain = |text: &str| StatusBarSegment {
            text: text.to_string(),
            fg: Color::rgb(220, 220, 220),
            bg: Color::rgb(40, 80, 120),
            bold: false,
            action_id: None,
        };
        StatusBar {
            id: WidgetId::new("priority-demo-status"),
            left_segments: vec![StatusBarSegment {
                text: mode_text,
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: true,
                action_id: None,
            }],
            // Least-important first, cursor-position segment last — the
            // documented convention (`primitives::status_bar`'s module
            // doc) that keeps it visible under priority-drop.
            right_segments: vec![
                plain("Spaces: 4"),
                plain("UTF-8"),
                plain("LF"),
                plain("Ln 1, Col 1"),
            ],
        }
    }
}

impl Default for StatusBarPriorityDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for StatusBarPriorityDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status_bar(),
            &InteractionState::new(),
        );
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            }
            | UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,

            UiEvent::KeyPressed {
                key: Key::Char('d'),
                ..
            } => {
                self.dirty = !self.dirty;
                Reaction::Redraw
            }

            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
