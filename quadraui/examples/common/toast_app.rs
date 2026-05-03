//! Backend-agnostic app code for the toast example
//! ([`tui_toast`] / [`gtk_toast`]).
//!
//! [`ToastApp`] demonstrates a [`ToastStack`] with toasts of varying
//! severity, dismiss, and action buttons.
//!
//! Controls:
//! - 1/2/3/4    add Info / Success / Warning / Error toast
//! - a          add toast with action button
//! - click ×    dismiss toast
//! - click action   log action
//! - q / Esc    quit

use quadraui::{
    AppLogic, Backend, Color, Key, NamedKey, Reaction, Rect, StatusBar, StatusBarSegment,
    ToastAction, ToastCorner, ToastHit, ToastItem, ToastSeverity, ToastStack, UiEvent, WidgetId,
};

pub struct ToastApp {
    toasts: Vec<ToastItem>,
    next_id: usize,
    last_message: String,
}

impl ToastApp {
    pub fn new() -> Self {
        Self {
            toasts: vec![ToastItem {
                id: WidgetId::new("welcome"),
                title: "Welcome".into(),
                body: "Press 1-4 to add toasts".into(),
                severity: ToastSeverity::Info,
                action: None,
                accent: None,
            }],
            next_id: 1,
            last_message: "Ready".into(),
        }
    }

    fn add_toast(&mut self, severity: ToastSeverity, action: Option<ToastAction>) {
        let label = match severity {
            ToastSeverity::Info => "Info",
            ToastSeverity::Success => "Success",
            ToastSeverity::Warning => "Warning",
            ToastSeverity::Error => "Error",
        };
        let id = format!("toast-{}", self.next_id);
        self.next_id += 1;
        self.toasts.push(ToastItem {
            id: WidgetId::new(&id),
            title: format!("{label} notification"),
            body: format!("Toast #{}", self.next_id - 1),
            severity,
            action,
            accent: None,
        });
        self.last_message = format!("Added {label} toast");
    }

    fn stack(&self) -> ToastStack {
        ToastStack {
            id: WidgetId::new("toasts"),
            corner: ToastCorner::BottomRight,
            toasts: self.toasts.clone(),
        }
    }

    fn status_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.last_message),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " 1-4=add | a=action | q=quit ".into(),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for ToastApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for ToastApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();

        // Status bar at bottom.
        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let _ = backend.draw_status_bar(status_rect, &self.status_bar());

        // Toast stack overlays the viewport.
        let overlay_rect = Rect::new(0.0, 0.0, viewport.width, viewport.height - lh);
        let _ = backend.draw_toast_stack(overlay_rect, &self.stack());
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
                key: Key::Char('1'),
                ..
            } => {
                self.add_toast(ToastSeverity::Info, None);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('2'),
                ..
            } => {
                self.add_toast(ToastSeverity::Success, None);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('3'),
                ..
            } => {
                self.add_toast(ToastSeverity::Warning, None);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('4'),
                ..
            } => {
                self.add_toast(ToastSeverity::Error, None);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('a'),
                ..
            } => {
                self.add_toast(
                    ToastSeverity::Error,
                    Some(ToastAction {
                        id: WidgetId::new("retry"),
                        label: "Retry".into(),
                    }),
                );
                Reaction::Redraw
            }
            UiEvent::MouseDown { position, .. } => {
                let viewport = backend.viewport();
                let lh = backend.line_height();
                let overlay_rect = Rect::new(0.0, 0.0, viewport.width, viewport.height - lh);
                let stack = self.stack();
                let layout = backend.toast_stack_layout(overlay_rect, &stack);
                match layout.hit_test(position.x, position.y) {
                    ToastHit::Dismiss(id) => {
                        self.toasts.retain(|t| t.id != id);
                        self.last_message = format!("Dismissed {}", id.as_str());
                    }
                    ToastHit::Action(id) => {
                        self.last_message = format!("Action: {}", id.as_str());
                    }
                    ToastHit::Body(id) => {
                        self.last_message = format!("Clicked {}", id.as_str());
                    }
                    ToastHit::Empty => {}
                }
                Reaction::Redraw
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
