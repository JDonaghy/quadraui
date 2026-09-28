//! Backend-agnostic app code for the toast-actions example
//! ([`tui_toast_actions`] / [`gtk_toast_actions`]) — issue #1185.
//!
//! Demonstrates the VS Code-style actionable toast: a two-action toast
//! ("Install" primary + "Don't ask again" secondary, plus the dismiss
//! `×`) driven by [`quadraui::compose::ToastStackController`] for
//! keyboard focus, alongside the existing mouse hit-testing every toast
//! example already has.
//!
//! Controls:
//! - n          push a new "Install extension?" toast with two actions
//! - Tab        (while unfocused) give the toast stack keyboard focus
//! - Tab/Shift+Tab/Left/Right   (while focused) cycle the focused
//!   toast's buttons (dismiss included)
//! - Up/Down    (while focused) move between toasts
//! - Enter      (while focused) activate the focused button
//! - Escape     (while focused) dismiss the focused toast and return
//!   focus to the app; (while unfocused) quit
//! - click ×/action/body   same hit-testing as every other toast example
//! - q          quit

use quadraui::compose::{ToastStackController, ToastStackEvent};
use quadraui::{
    AppLogic, Backend, Color, InteractionState, Key, NamedKey, Reaction, Rect, StatusBar,
    StatusBarSegment, ToastAction, ToastCorner, ToastHit, ToastItem, ToastSeverity, ToastStack,
    UiEvent, WidgetId,
};

pub struct ToastActionsApp {
    toasts: Vec<ToastItem>,
    next_id: usize,
    last_message: String,
    controller: ToastStackController,
}

impl ToastActionsApp {
    pub fn new() -> Self {
        Self {
            toasts: vec![install_toast(0)],
            next_id: 1,
            last_message: "Ready — press n for a toast, Tab to focus it".into(),
            controller: ToastStackController::new(),
        }
    }

    fn stack(&self) -> ToastStack {
        ToastStack {
            id: WidgetId::new("toasts"),
            corner: ToastCorner::BottomRight,
            toasts: self.toasts.clone(),
            focus: self.controller.focus(),
        }
    }

    fn overlay_rect(&self, backend: &dyn Backend) -> Rect {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        Rect::new(0.0, 0.0, viewport.width, viewport.height - lh)
    }

    fn status_bar(&self) -> StatusBar {
        let hint = if self.controller.is_focused() {
            " Tab/Left/Right=cycle  Up/Down=toast  Enter=activate  Esc=dismiss "
        } else {
            " n=add toast  Tab=focus stack  q=quit "
        };
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
                text: hint.into(),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    fn dismiss(&mut self, id: WidgetId) {
        self.toasts.retain(|t| t.id != id);
        self.last_message = format!("Dismissed {}", id.as_str());
    }
}

fn install_toast(n: usize) -> ToastItem {
    ToastItem {
        id: WidgetId::new(format!("install-{n}")),
        title: "Install Markdown Language Server?".into(),
        body: "Recommended for .md files in this workspace.".into(),
        severity: ToastSeverity::Info,
        actions: vec![
            ToastAction {
                id: WidgetId::new(format!("install-{n}:install")),
                label: "Install".into(),
                primary: true,
            },
            ToastAction {
                id: WidgetId::new(format!("install-{n}:dont-ask")),
                label: "Don't ask again".into(),
                primary: false,
            },
        ],
        accent: None,
    }
}

impl Default for ToastActionsApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for ToastActionsApp {
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
        let overlay_rect = self.overlay_rect(backend);
        let _ = backend.draw_toast_stack(overlay_rect, &self.stack());
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        // The controller only ever consumes `KeyPressed` while the stack
        // has focus — every other event (including every key while
        // unfocused) comes back `Ignored`, so normal app routing below
        // still runs unimpeded.
        let stack = self.stack();
        match self.controller.handle(&event, &stack) {
            ToastStackEvent::Action(id) => {
                self.last_message = format!("Action: {}", id.as_str());
                // The demo's "Install"/"Don't ask again" both just
                // dismiss the toast they belong to, like a real app
                // would after acting on the choice.
                if let Some(t) = self
                    .toasts
                    .iter()
                    .find(|t| t.actions.iter().any(|a| a.id == id))
                {
                    let toast_id = t.id.clone();
                    self.dismiss(toast_id);
                }
                return Reaction::Redraw;
            }
            ToastStackEvent::Dismiss(id) => {
                self.dismiss(id);
                return Reaction::Redraw;
            }
            ToastStackEvent::Consumed | ToastStackEvent::FocusReturned => {
                return Reaction::Redraw;
            }
            ToastStackEvent::Ignored => {}
        }

        match event {
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            } => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Tab),
                ..
            } => {
                if self.controller.give_focus(&stack) {
                    self.last_message = "Toast stack focused".into();
                } else {
                    self.last_message = "No toasts to focus".into();
                }
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('n'),
                ..
            } => {
                let n = self.next_id;
                self.next_id += 1;
                self.toasts.push(install_toast(n));
                self.last_message = "Added install toast".into();
                Reaction::Redraw
            }
            UiEvent::MouseDown { position, .. } => {
                let overlay_rect = self.overlay_rect(backend);
                let layout = backend.toast_stack_layout(overlay_rect, &stack);
                match layout.hit_test(position.x, position.y) {
                    ToastHit::Dismiss(id) => self.dismiss(id),
                    ToastHit::Action(id) => {
                        self.last_message = format!("Action: {}", id.as_str());
                        if let Some(t) = self
                            .toasts
                            .iter()
                            .find(|t| t.actions.iter().any(|a| a.id == id))
                        {
                            let toast_id = t.id.clone();
                            self.dismiss(toast_id);
                        }
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
