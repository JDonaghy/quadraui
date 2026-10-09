//! `Message Dialog` demo — merges
//! `quadraui/examples/common/message_dialog_app.rs` (the
//! `MessageDialogController` used directly) and
//! `quadraui/examples/common/message_dialog_demo.rs` (the
//! `PlatformServices::show_message_dialog` + `native_dialog_options`
//! degrade path), as two variants of the same widget rather than two
//! modules (#1347).
//!
//! Both variants open a "Save changes?" confirm dialog over the
//! **entire gallery window**, not just the Demo tab's content area —
//! deliberately: a real app's unsaved-changes prompt covers its whole
//! window (sidebar included), and `MessageDialogController::render` /
//! `PlatformServices::show_message_dialog`'s in-canvas fallback both
//! centre on `Backend::viewport()` for exactly that reason. The gallery
//! chrome reappears underneath the instant the dialog resolves.
//!
//! - **Direct controller** drives [`MessageDialogController`] itself —
//!   the same shape every other "always in-canvas" compose controller in
//!   this gallery uses.
//! - **Native service** calls `backend.services().show_message_dialog`
//!   through [`native_dialog_options`] — the one call that resolves to a
//!   real native `gtk4::AlertDialog` on GTK and to the identical
//!   in-canvas controller (via a nested draw-and-read loop) on TUI, with
//!   no branch in this demo's own code. [`Demo::caps_note`] reports the
//!   degrade.

use quadraui::{
    native_dialog_options, Backend, BackendCaps, Color, Dialog, DialogButton, DialogSeverity,
    InteractionState, Key, MessageDialogButton, MessageDialogController, MessageDialogEvent,
    MessageDialogOptions, Reaction, Rect, StatusBar, StatusBarSegment, StyledText, UiEvent,
    WidgetId,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("message_dialog.rs");

// gallery:begin
pub struct MessageDialogDemo {
    dialog: Option<MessageDialogController>,
    status: String,
}

impl MessageDialogDemo {
    pub fn new() -> Self {
        Self {
            dialog: None,
            status: "press Enter to open the dialog".into(),
        }
    }

    fn controller_dialog() -> MessageDialogController {
        MessageDialogController::new(MessageDialogOptions {
            title: "Unsaved Changes".to_string(),
            body: "You have unsaved changes.\nDo you want to save before closing?".to_string(),
            buttons: vec![
                MessageDialogButton {
                    id: WidgetId::new("gallery:message-dialog:save"),
                    label: "Save".into(),
                    is_default: true,
                    is_cancel: false,
                },
                MessageDialogButton {
                    id: WidgetId::new("gallery:message-dialog:discard"),
                    label: "Discard".into(),
                    is_default: false,
                    is_cancel: true,
                },
            ],
            severity: Some(DialogSeverity::Warning),
        })
    }

    /// Same descriptor shape as [`Self::controller_dialog`], as a raw
    /// [`Dialog`] — [`native_dialog_options`] maps this to
    /// [`MessageDialogOptions`] for the native-service variant. Has no
    /// `table`/`input`, so the mapping always succeeds.
    fn native_dialog() -> Dialog {
        Dialog {
            id: WidgetId::new("gallery:message-dialog:native"),
            title: StyledText::plain("Discard unsaved changes?"),
            body: vec![StyledText::plain(
                "main.rs has unsaved changes. This cannot be undone.",
            )],
            buttons: vec![
                DialogButton {
                    id: WidgetId::new("gallery:message-dialog:keep"),
                    label: "Keep Editing".to_string(),
                    is_default: true,
                    is_cancel: true,
                    tint: None,
                },
                DialogButton {
                    id: WidgetId::new("gallery:message-dialog:discard-native"),
                    label: "Discard".to_string(),
                    is_default: false,
                    is_cancel: false,
                    tint: Some(Color::rgb(200, 60, 60)),
                },
            ],
            severity: Some(DialogSeverity::Warning),
            vertical_buttons: false,
            table: None,
            input: None,
        }
    }

    fn status_bar(&self, variant: usize) -> StatusBar {
        let variant_label = if variant == 0 {
            "Direct controller"
        } else {
            "Native service"
        };
        StatusBar {
            id: WidgetId::new("gallery:message-dialog:status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" [{variant_label}] {} ", self.status),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 60, 100),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " Enter=open ".into(),
                fg: Color::rgb(180, 180, 180),
                bg: Color::rgb(40, 60, 100),
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for MessageDialogDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for MessageDialogDemo {
    fn name(&self) -> &'static str {
        "Message Dialog"
    }

    fn group(&self) -> &'static str {
        "Overlays"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Direct controller", "Native service"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let lh = backend.line_height();
        let bar_rect = Rect::new(area.x, area.y, area.width, lh);
        backend.draw_status_bar_interactive(
            bar_rect,
            &self.status_bar(variant),
            &InteractionState::new(),
        );

        if variant == 0 {
            if let Some(dialog) = &self.dialog {
                dialog.render(backend);
            }
        }
        // The native-service variant (`variant == 1`) never has a
        // lingering `self.dialog` to render: `handle` below resolves it
        // synchronously inside the same call that opens it (either via a
        // real native alert on GTK, or the in-canvas nested loop on TUI),
        // so there is nothing left open by the time `render` runs again.
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        _area: Rect,
    ) -> Reaction {
        if variant == 0 {
            if let Some(ref mut dialog) = self.dialog {
                match dialog.handle(event, backend) {
                    MessageDialogEvent::Resolved(id) => {
                        self.status = format!("Resolved: {}", id.as_str());
                        self.dialog = None;
                        return Reaction::Redraw;
                    }
                    MessageDialogEvent::Consumed => return Reaction::Redraw,
                    MessageDialogEvent::Ignored => {}
                }
            } else if let UiEvent::KeyPressed {
                key: Key::Named(quadraui::NamedKey::Enter),
                ..
            } = event
            {
                self.dialog = Some(Self::controller_dialog());
                self.status = "Save changes? — choose Save or Discard".into();
                return Reaction::Redraw;
            }
            return Reaction::Continue;
        }

        // Native-service variant.
        if let UiEvent::KeyPressed {
            key: Key::Named(quadraui::NamedKey::Enter),
            ..
        } = event
        {
            let dialog = Self::native_dialog();
            let opts = native_dialog_options(&dialog)
                .expect("demo dialog has no table/input — always natively expressible");
            self.status = match backend.services().show_message_dialog(opts) {
                Some(id) if id == dialog.buttons[0].id => "Kept editing".to_string(),
                Some(id) if id == dialog.buttons[1].id => "Discarded".to_string(),
                Some(other) => format!("Unexpected button: {other:?}"),
                None => "Cancelled (or unsupported on this backend)".to_string(),
            };
            return Reaction::Redraw;
        }
        Reaction::Continue
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        serde_json::json!({
            "variant": if variant == 0 { "direct_controller" } else { "native_service" },
            "status": self.status,
            "open": self.dialog.is_some(),
        })
    }

    fn caps_note(&self, variant: usize, caps: &BackendCaps) -> Option<String> {
        if variant == 1 && !caps.native_dialogs {
            Some(
                "This backend has no native alert facility (BackendCaps::native_dialogs is \
                 false) — show_message_dialog drives the same MessageDialogController in-canvas \
                 instead, through a nested draw-and-read loop, and still returns a real chosen \
                 button."
                    .into(),
            )
        } else {
            None
        }
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_dialog_has_save_and_discard() {
        let dialog = MessageDialogDemo::controller_dialog();
        let _ = dialog; // constructs without panicking
    }

    #[test]
    fn native_dialog_maps_through_native_dialog_options() {
        let dialog = MessageDialogDemo::native_dialog();
        assert!(native_dialog_options(&dialog).is_some());
    }
}
