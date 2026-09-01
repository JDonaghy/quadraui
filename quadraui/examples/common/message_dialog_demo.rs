//! Message-dialog demo — exercises [`PlatformServices::show_message_dialog`]
//! / [`native_dialog_options`] (quadraui#666).
//!
//! - **GTK** (`gtk_message_dialog`): maps an in-canvas [`Dialog`]
//!   descriptor through `native_dialog_options` and opens a real native
//!   `gtk4::AlertDialog` (via the same nested-mainloop adapter
//!   `gtk::services` already uses for file dialogs), parented to this
//!   window, with GNOME-HIG button order (cancel leftmost, default
//!   rightmost). This is the primary manual smoke test for #666 —
//!   driving it headlessly isn't possible (no `GtkDriver` yet, #301, and
//!   `GtkDriver` paints Cairo — it never sees a native `AlertDialog`
//!   window at all), so exercise it by hand: run the example, press `m`,
//!   and confirm the native alert appears, is parented to the demo
//!   window, shows "Discard" left of "Keep Editing" (HIG cancel-left /
//!   default-right order), and the status bar reflects the chosen button
//!   (or "cancelled" on Escape/close). See `docs/TESTING.md`'s "What
//!   unit tests don't cover" for the write-up of this gap.
//! - **TUI** (`tui_message_dialog`): `PlatformServices::show_message_dialog`
//!   is documented to always return `None` on TUI (the in-canvas
//!   [`Dialog`] primitive / `Backend::draw_dialog` stays the TUI path) —
//!   this demo exercises that documented contract and is covered by the
//!   `TuiDriver` test in `tests/tui_example_driver.rs`.
//!
//! ```sh
//! cargo run --example gtk_message_dialog --features gtk
//! cargo run --example tui_message_dialog --features tui
//! ```
//!
//! Controls:
//! - `m` — show the "Discard unsaved changes?" message dialog
//! - `Esc` / `q` — quit

use quadraui::{
    native_dialog_options, AppLogic, Backend, Color, Dialog, DialogButton, DialogSeverity, Key,
    NamedKey, Reaction, Rect, StatusBar, StatusBarSegment, StyledText, UiEvent, WidgetId,
};

pub struct MessageDialogDemo {
    status: String,
}

impl MessageDialogDemo {
    pub fn new() -> Self {
        Self {
            status: "m = show dialog · Esc quits".to_string(),
        }
    }

    /// The same [`Dialog`] descriptor an app would pass to
    /// `Backend::draw_dialog` for the in-canvas fallback — mapped through
    /// [`native_dialog_options`] below to drive the native alert instead.
    /// Has no `table`/`input`, so the mapping always succeeds.
    fn dialog(&self) -> Dialog {
        Dialog {
            id: WidgetId::new("message-dialog-demo:confirm"),
            title: StyledText::plain("Discard unsaved changes?"),
            body: vec![StyledText::plain(
                "main.rs has unsaved changes. This cannot be undone.",
            )],
            buttons: vec![
                DialogButton {
                    id: WidgetId::new("message-dialog-demo:keep"),
                    label: "Keep Editing".to_string(),
                    is_default: true,
                    is_cancel: true,
                    tint: None,
                },
                DialogButton {
                    id: WidgetId::new("message-dialog-demo:discard"),
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

    fn status_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("message-dialog-demo:status"),
            left_segments: vec![StatusBarSegment {
                text: " Message dialog demo (#666) ".into(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: true,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.status),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
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

impl AppLogic for MessageDialogDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        backend.draw_status_bar(status_rect, &self.status_bar(), None, None);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape) | Key::Char('q'),
                ..
            } => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Char('m'),
                ..
            } => {
                let dialog = self.dialog();
                // Always `Some` here (no table/input on `dialog()` above) —
                // real callers must still check, and fall back to
                // `Backend::draw_dialog` when this returns `None`.
                let opts = native_dialog_options(&dialog)
                    .expect("demo dialog has no table/input — always natively expressible");
                self.status = match backend.services().show_message_dialog(opts) {
                    Some(id) if id == dialog.buttons[0].id => "Kept editing".to_string(),
                    Some(id) if id == dialog.buttons[1].id => "Discarded".to_string(),
                    Some(other) => format!("Unexpected button: {other:?}"),
                    None => "Cancelled (or unsupported on this backend)".to_string(),
                };
                Reaction::Redraw
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
