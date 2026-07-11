//! File-dialog demo — exercises [`PlatformServices::show_file_open_dialog`]
//! / `show_file_save_dialog` (#427).
//!
//! - **GTK** (`gtk_file_dialog`): opens a real native `gtk4::FileDialog`
//!   (via the nested-mainloop adapter in `gtk::services`) and blocks until
//!   the user picks a file/location or cancels. This is the primary manual
//!   smoke test for #427 — driving it headlessly isn't possible (no
//!   `GtkDriver` yet, #301), so exercise it by hand: run the example, press
//!   `o` / `s`, and confirm the native dialog appears, is parented to the
//!   demo window, and the status bar reflects the picked path (or
//!   "cancelled" on Escape/close).
//! - **TUI** (`tui_file_dialog`): `PlatformServices::show_file_open_dialog`
//!   /`show_file_save_dialog` are documented to always return `None` on
//!   TUI (apps should provide an in-TUI picker instead) — this demo
//!   exercises that documented contract and is covered by the
//!   `TuiDriver` test in `tests/tui_example_driver.rs`.
//!
//! ```sh
//! cargo run --example gtk_file_dialog --features gtk
//! cargo run --example tui_file_dialog --features tui
//! ```
//!
//! Controls:
//! - `o` — open-file dialog (filtered to `*.rs`)
//! - `s` — save-as dialog (initial name `untitled.txt`)
//! - `Esc` / `q` — quit

use quadraui::{
    AppLogic, Backend, Color, FileDialogOptions, Key, NamedKey, Reaction, Rect, StatusBar,
    StatusBarSegment, UiEvent, WidgetId,
};

pub struct FileDialogDemo {
    status: String,
}

impl FileDialogDemo {
    pub fn new() -> Self {
        Self {
            status: "o = open · s = save-as · Esc quits".to_string(),
        }
    }

    fn status_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("file-dialog-demo:status"),
            left_segments: vec![StatusBarSegment {
                text: " File dialog demo (#427) ".into(),
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

impl Default for FileDialogDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for FileDialogDemo {
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
                key: Key::Char('o'),
                ..
            } => {
                let opts = FileDialogOptions {
                    title: Some("Open File".to_string()),
                    filters: vec![("Rust files".to_string(), vec!["rs".to_string()])],
                    ..Default::default()
                };
                self.status = match backend.services().show_file_open_dialog(opts) {
                    Some(path) => format!("Opened: {}", path.display()),
                    None => "Open cancelled (or unsupported on this backend)".to_string(),
                };
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('s'),
                ..
            } => {
                let opts = FileDialogOptions {
                    title: Some("Save As".to_string()),
                    initial_filename: Some("untitled.txt".to_string()),
                    ..Default::default()
                };
                self.status = match backend.services().show_file_save_dialog(opts) {
                    Some(path) => format!("Save as: {}", path.display()),
                    None => "Save cancelled (or unsupported on this backend)".to_string(),
                };
                Reaction::Redraw
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
