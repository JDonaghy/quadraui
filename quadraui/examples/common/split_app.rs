//! Backend-agnostic app code for the split example
//! ([`tui_split`] / [`gtk_split`]).
//!
//! [`SplitApp`] demonstrates a draggable [`Split`] with two panes
//! and a [`StatusBar`] showing the current ratio.
//!
//! Controls:
//! - drag divider             resize panes
//! - v                        toggle Horizontal / Vertical
//! - r                        reset ratio to 0.5
//! - q / Esc                  quit

use quadraui::{
    AppLogic, Backend, Color, Key, NamedKey, Reaction, Rect, Split, SplitDirection, SplitHit,
    StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

pub struct SplitApp {
    ratio: f32,
    direction: SplitDirection,
    dragging: bool,
}

impl SplitApp {
    pub fn new() -> Self {
        Self {
            ratio: 0.5,
            direction: SplitDirection::Horizontal,
            dragging: false,
        }
    }

    fn split(&self) -> Split {
        Split {
            id: WidgetId::new("main-split"),
            direction: self.direction,
            ratio: self.ratio,
            first_min: 0.0,
            second_min: 0.0,
        }
    }

    fn status_bar(&self) -> StatusBar {
        let dir = match self.direction {
            SplitDirection::Horizontal => "H",
            SplitDirection::Vertical => "V",
        };
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" ratio: {:.0}% ({dir}) ", self.ratio * 100.0),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " drag divider | v=toggle | r=reset | q=quit ".into(),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    fn fill_pane(
        &self,
        backend: &mut dyn Backend,
        bounds: Rect,
        label: &str,
        fg: Color,
        bg: Color,
    ) {
        let bar = StatusBar {
            id: WidgetId::new("pane-label"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {label} "),
                fg,
                bg,
                bold: true,
                action_id: None,
            }],
            right_segments: vec![],
        };
        let _ = backend.draw_status_bar(bounds, &bar, None, None);
    }
}

impl Default for SplitApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for SplitApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let split_rect = Rect::new(0.0, 0.0, viewport.width, viewport.height - lh);
        let split = self.split();
        let layout = backend.draw_split(split_rect, &split);

        let first_label = Rect::new(
            layout.first_bounds.x,
            layout.first_bounds.y,
            layout.first_bounds.width,
            lh,
        );
        self.fill_pane(
            backend,
            first_label,
            "FIRST",
            Color::rgb(255, 255, 255),
            Color::rgb(60, 60, 100),
        );

        let second_label = Rect::new(
            layout.second_bounds.x,
            layout.second_bounds.y,
            layout.second_bounds.width,
            lh,
        );
        self.fill_pane(
            backend,
            second_label,
            "SECOND",
            Color::rgb(255, 255, 255),
            Color::rgb(100, 60, 60),
        );

        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let _ = backend.draw_status_bar(status_rect, &self.status_bar(), None, None);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
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
                key: Key::Char('v'),
                ..
            } => {
                self.direction = match self.direction {
                    SplitDirection::Horizontal => SplitDirection::Vertical,
                    SplitDirection::Vertical => SplitDirection::Horizontal,
                };
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('r'),
                ..
            } => {
                self.ratio = 0.5;
                Reaction::Redraw
            }
            UiEvent::MouseDown { position, .. } => {
                let viewport = backend.viewport();
                let lh = backend.line_height();
                let split_rect = Rect::new(0.0, 0.0, viewport.width, viewport.height - lh);
                let split = self.split();
                let layout = backend.split_layout(split_rect, &split);
                match layout.hit_test(position.x, position.y) {
                    SplitHit::Divider(_) => {
                        self.dragging = true;
                    }
                    _ => {}
                }
                Reaction::Continue
            }
            UiEvent::MouseMoved { position, .. } => {
                if !self.dragging {
                    return Reaction::Continue;
                }
                let viewport = backend.viewport();
                let lh = backend.line_height();
                let split_rect = Rect::new(0.0, 0.0, viewport.width, viewport.height - lh);
                let new_ratio = match self.direction {
                    SplitDirection::Horizontal => {
                        if split_rect.width > 0.0 {
                            ((position.x - split_rect.x) / split_rect.width).clamp(0.05, 0.95)
                        } else {
                            0.5
                        }
                    }
                    SplitDirection::Vertical => {
                        if split_rect.height > 0.0 {
                            ((position.y - split_rect.y) / split_rect.height).clamp(0.05, 0.95)
                        } else {
                            0.5
                        }
                    }
                };
                if (new_ratio - self.ratio).abs() > 0.001 {
                    self.ratio = new_ratio;
                    Reaction::Redraw
                } else {
                    Reaction::Continue
                }
            }
            UiEvent::MouseUp { .. } => {
                self.dragging = false;
                Reaction::Continue
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
