//! `Toast` demo — the gallery's seed exhibit, adapted from
//! `quadraui/examples/common/toast_app.rs` and extended (issue #1347)
//! with the keyboard-focus/action behaviour from
//! `quadraui/examples/common/toast_actions_app.rs` — `Tab` gives the
//! toast stack keyboard focus, `Tab`/`Shift+Tab`/`Left`/`Right` cycle
//! its buttons, `Up`/`Down` move between toasts, `Enter` activates the
//! focused button, and `Esc` returns focus to the demo. Extended rather
//! than duplicated into a second module, per #1347's instruction for
//! `toast_actions_app`.
//!
//! Exercises [`ToastOverlay`] with varying severities, dismiss, an
//! action button, and keyboard-driven focus, proving the gallery
//! harness end to end before the other groups are ported (separate
//! follow-up issues).

use quadraui::compose::{ToastStackController, ToastStackEvent};
use quadraui::{
    Backend, BackendCaps, Color, InteractionState, Key, NamedKey, Reaction, Rect, StatusBar,
    StatusBarSegment, Toast, ToastButton, ToastCorner, ToastHit, ToastOverlay, ToastSeverity,
    UiEvent, WidgetId,
};

use crate::demo::{extract_region, Demo};

// `include_str!` resolves relative to *this* file's own location
// (unlike `file!()`, which cargo reports relative to the invocation
// directory and can't be combined with `CARGO_MANIFEST_DIR` reliably
// inside a workspace) — the simplest form of the "module includes its
// own source" trick `Demo::source`'s doc describes.
const SOURCE: &str = include_str!("toast.rs");

// gallery:begin
pub struct ToastDemo {
    toasts: Vec<Toast>,
    next_id: usize,
    controller: ToastStackController,
}

impl ToastDemo {
    pub fn new() -> Self {
        Self {
            toasts: vec![Toast {
                id: WidgetId::new("gallery:toast:welcome"),
                title: "Welcome".into(),
                body: "1-4=add  a=add with action  Tab=focus stack".into(),
                severity: ToastSeverity::Info,
                actions: Vec::new(),
                accent: None,
            }],
            next_id: 1,
            controller: ToastStackController::new(),
        }
    }

    fn add(&mut self, severity: ToastSeverity, actions: Vec<ToastButton>) {
        let label = match severity {
            ToastSeverity::Info => "Info",
            ToastSeverity::Success => "Success",
            ToastSeverity::Warning => "Warning",
            ToastSeverity::Error => "Error",
        };
        let id = format!("gallery:toast:{}", self.next_id);
        self.next_id += 1;
        self.toasts.push(Toast {
            id: WidgetId::new(id),
            title: format!("{label} notification"),
            body: format!("Toast #{}", self.next_id - 1),
            severity,
            actions,
            accent: None,
        });
    }

    fn stack(&self) -> ToastOverlay {
        ToastOverlay {
            id: WidgetId::new("gallery:toast-stack"),
            corner: ToastCorner::BottomRight,
            toasts: self.toasts.clone(),
            focus: self.controller.focus(),
        }
    }

    fn hint_bar(&self) -> StatusBar {
        let text = if self.controller.is_focused() {
            " Tab/\u{2190}/\u{2192}=cycle buttons  \u{2191}/\u{2193}=toast  Enter=activate  Esc=unfocus "
        } else {
            " 1-4=add severity | a=add with action | Tab=focus stack | click \u{d7}=dismiss "
        };
        StatusBar {
            id: WidgetId::new("gallery:toast-hint"),
            left_segments: vec![StatusBarSegment {
                text: text.into(),
                fg: Color::rgb(190, 190, 190),
                bg: Color::rgb(30, 30, 30),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }

    /// Split `area` into the hint row (top line) and the toast overlay's
    /// own area (everything below it). Shared by `render` and `handle`
    /// so the two can never derive different geometry for the same
    /// frame.
    fn overlay_area(backend: &dyn Backend, area: Rect) -> Rect {
        let lh = backend.line_height();
        Rect::new(area.x, area.y + lh, area.width, (area.height - lh).max(0.0))
    }
}

impl Default for ToastDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for ToastDemo {
    fn name(&self) -> &'static str {
        "Toast"
    }

    fn group(&self) -> &'static str {
        "Overlays"
    }

    fn render(&self, _variant: usize, backend: &mut dyn Backend, area: Rect) {
        let lh = backend.line_height();
        let hint_rect = Rect::new(area.x, area.y, area.width, lh);
        let _ = backend.draw_status_bar_interactive(
            hint_rect,
            &self.hint_bar(),
            &InteractionState::new(),
        );

        let overlay_area = Self::overlay_area(backend, area);
        let _ = backend.draw_toast_overlay(overlay_area, &self.stack());
    }

    fn handle(
        &mut self,
        _variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        // The controller only ever consumes `KeyPressed` while the stack
        // has focus — every other event (including every key while
        // unfocused) comes back `Ignored`, so the severity/action keys
        // below still work unimpeded until `Tab` claims focus.
        let stack = self.stack();
        match self.controller.handle(event, &stack) {
            ToastStackEvent::Action(id) => {
                if let Some(t) = self
                    .toasts
                    .iter()
                    .find(|t| t.actions.iter().any(|a| a.id == id))
                {
                    let toast_id = t.id.clone();
                    self.toasts.retain(|t| t.id != toast_id);
                }
                return Reaction::Redraw;
            }
            ToastStackEvent::Dismiss(id) => {
                self.toasts.retain(|t| t.id != id);
                return Reaction::Redraw;
            }
            ToastStackEvent::Consumed | ToastStackEvent::FocusReturned => {
                return Reaction::Redraw;
            }
            ToastStackEvent::Ignored => {}
        }

        match event {
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Tab),
                ..
            } => {
                self.controller.give_focus(&stack);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('1'),
                ..
            } => {
                self.add(ToastSeverity::Info, Vec::new());
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('2'),
                ..
            } => {
                self.add(ToastSeverity::Success, Vec::new());
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('3'),
                ..
            } => {
                self.add(ToastSeverity::Warning, Vec::new());
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('4'),
                ..
            } => {
                self.add(ToastSeverity::Error, Vec::new());
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('a'),
                ..
            } => {
                self.add(
                    ToastSeverity::Error,
                    vec![ToastButton {
                        id: WidgetId::new("gallery:toast-action:retry"),
                        label: "Retry".into(),
                        primary: true,
                    }],
                );
                Reaction::Redraw
            }
            UiEvent::MouseDown { position, .. } => {
                let overlay_area = Self::overlay_area(backend, area);
                let stack = self.stack();
                let layout = backend.toast_stack_layout(overlay_area, &stack);
                match layout.hit_test(position.x, position.y) {
                    ToastHit::Dismiss(id) => {
                        self.toasts.retain(|t| t.id != id);
                        Reaction::Redraw
                    }
                    ToastHit::Action(_) | ToastHit::Body(_) => Reaction::Redraw,
                    ToastHit::Empty => Reaction::Continue,
                }
            }
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, _variant: usize) -> serde_json::Value {
        serde_json::to_value(self.stack()).unwrap_or(serde_json::Value::Null)
    }

    fn caps_note(&self, _variant: usize, caps: &BackendCaps) -> Option<String> {
        if caps.notifications {
            None
        } else {
            Some(
                "This backend has no native OS notification centre (BackendCaps::notifications \
                 is false) — quadraui::compose::notify_or_toast would fall back to exactly the \
                 in-app toast shown below."
                    .into(),
            )
        }
    }
}
// gallery:end
