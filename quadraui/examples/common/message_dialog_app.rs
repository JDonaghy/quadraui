//! Backend-agnostic app code for the `MessageDialogController` demo
//! ([`tui_message_dialog`] / [`gtk_message_dialog`]).
//!
//! [`MessageDialogApp`] demonstrates a self-contained `AppLogic` that:
//! - Opens a "Save changes?" confirm dialog on startup, with Save
//!   (default) and Discard (cancel) buttons.
//! - Renders it via the `Dialog` primitive, centred in the viewport.
//! - Resolves a button (`Enter`, `←`/`→`/`Tab` to move focus, click, or
//!   `Esc`) and shows which one was chosen in a status bar.
//!
//! Controls (while dialog is open):
//! - `←` / `→` / `Tab` / `Shift+Tab` — move keyboard focus between buttons.
//! - `Enter` — activate the focused button.
//! - `Esc` — resolves to the cancel button (Discard here).
//! - Click a button — resolves it directly.
//!
//! Controls (after resolve):
//! - `o` — reopen the dialog.
//! - `q` / `Esc` — quit.

use quadraui::{
    AppLogic, Backend, Color, MessageDialogButton, MessageDialogController, MessageDialogEvent,
    MessageDialogOptions, Reaction, Rect, StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

pub struct MessageDialogApp {
    dialog: Option<MessageDialogController>,
    status: String,
}

impl MessageDialogApp {
    pub fn new() -> Self {
        Self {
            dialog: Some(Self::build_dialog()),
            status: "Save changes? — choose Save or Discard".into(),
        }
    }

    fn build_dialog() -> MessageDialogController {
        MessageDialogController::new(MessageDialogOptions {
            title: "Unsaved Changes".to_string(),
            body: "You have unsaved changes.\nDo you want to save before closing?".to_string(),
            buttons: vec![
                MessageDialogButton {
                    id: WidgetId::new("save"),
                    label: "Save".into(),
                    is_default: true,
                    is_cancel: false,
                },
                MessageDialogButton {
                    id: WidgetId::new("discard"),
                    label: "Discard".into(),
                    is_default: false,
                    is_cancel: true,
                },
            ],
            severity: Some(quadraui::DialogSeverity::Warning),
        })
    }

    fn status_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.status),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 60, 100),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }
}

impl Default for MessageDialogApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for MessageDialogApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        let lh = backend.line_height();
        let bar_h = lh * 1.5;
        let bar_rect = Rect::new(0.0, vp.height - bar_h, vp.width, bar_h);
        let bar = self.status_bar();
        let _ =
            backend.draw_status_bar_interactive(bar_rect, &bar, &quadraui::InteractionState::new());

        if let Some(ref dialog) = self.dialog {
            dialog.render(backend);
        }
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        if let Some(ref mut dialog) = self.dialog {
            match dialog.handle(&event, backend) {
                MessageDialogEvent::Resolved(id) => {
                    self.status = format!(
                        "Resolved: {}  (press 'o' to reopen, q/Esc to quit)",
                        id.as_str()
                    );
                    self.dialog = None;
                    return Reaction::Redraw;
                }
                MessageDialogEvent::Consumed => return Reaction::Redraw,
                MessageDialogEvent::Ignored => {}
            }
        } else if let UiEvent::KeyPressed { ref key, .. } = event {
            match key {
                quadraui::Key::Char('q') | quadraui::Key::Named(quadraui::NamedKey::Escape) => {
                    return Reaction::Exit;
                }
                quadraui::Key::Char('o') => {
                    self.dialog = Some(Self::build_dialog());
                    self.status = "Save changes? — choose Save or Discard".into();
                    return Reaction::Redraw;
                }
                _ => {}
            }
        }

        match event {
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
