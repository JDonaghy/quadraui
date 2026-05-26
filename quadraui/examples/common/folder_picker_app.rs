//! Backend-agnostic app code for the `FolderPickerController` demo
//! ([`tui_folder_picker`] / [`gtk_folder_picker`]).
//!
//! [`FolderPickerApp`] demonstrates a self-contained `AppLogic` that:
//! - Opens the `FolderPickerController` on startup (rooted at `env::current_dir()`).
//! - Shows the picker as a centred palette modal.
//! - Confirms a selection (`Enter` on a non-`..` entry) and displays the
//!   chosen path in a status bar.
//! - Navigates into subdirectories or up with `..` / `-`.
//! - Dismisses the picker with `Esc`, then shows the last-confirmed path.
//!
//! Controls (while picker is open):
//! - Type to fuzzy-filter entries.
//! - `↑` / `k` and `↓` / `j` to move selection.
//! - `Enter` on `..` or `-` key — navigate up.
//! - `Enter` on any other entry — confirm that path.
//! - `Backspace` — delete last query character.
//! - `Esc` — dismiss picker.
//!
//! Controls (after dismiss):
//! - `o` — reopen picker.
//! - `q` / `Esc` — quit.

use std::path::PathBuf;

use quadraui::{
    AppLogic, Backend, Color, FolderPickerController, FolderPickerEvent, Key, NamedKey, Reaction,
    Rect, StatusBar, StatusBarSegment, UiEvent, WidgetId, PALETTE_CHROME_ROWS,
};

pub struct FolderPickerApp {
    picker: Option<FolderPickerController>,
    confirmed_path: Option<PathBuf>,
    status: String,
}

impl FolderPickerApp {
    pub fn new() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        let picker = FolderPickerController::new(cwd, vec![], false);
        Self {
            picker: Some(picker),
            confirmed_path: None,
            status: "Open Folder picker — navigate and press Enter to confirm".into(),
        }
    }

    /// Returns the popup rect centred in the viewport.
    ///
    /// Sizing mirrors vimcode's TUI picker: 60% width (min 50), 55% height
    /// (min 15). In pixel-unit backends the numbers are larger but the
    /// proportions are the same — the line_height is factored in via the
    /// AppLogic choosing cell-like units.
    fn popup_rect(backend: &dyn Backend) -> Rect {
        let vp = backend.viewport();
        let w = (vp.width * 0.6).max(50.0);
        let h = (vp.height * 0.55).max(15.0 * backend.line_height());
        let x = (vp.width - w) / 2.0;
        let y = (vp.height - h) / 2.0;
        Rect::new(x, y, w, h)
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
            right_segments: if let Some(ref p) = self.confirmed_path {
                vec![StatusBarSegment {
                    text: format!(" ✓ {} ", p.display()),
                    fg: Color::rgb(150, 240, 150),
                    bg: Color::rgb(30, 80, 30),
                    bold: false,
                    action_id: None,
                }]
            } else {
                vec![]
            },
        }
    }
}

impl Default for FolderPickerApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for FolderPickerApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        let lh = backend.line_height();
        // Status bar at the bottom.
        let bar_h = lh * 1.5;
        let bar_rect = Rect::new(0.0, vp.height - bar_h, vp.width, bar_h);
        let bar = self.status_bar();
        let _ = backend.draw_status_bar(bar_rect, &bar, None, None);

        // Folder picker modal (when open).
        if let Some(ref picker) = self.picker {
            let popup_rect = Self::popup_rect(backend);
            picker.render(popup_rect, backend);
        }
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        if let Some(ref mut picker) = self.picker {
            let popup_rect = Self::popup_rect(backend);
            let lh = backend.line_height();
            // Compute visible_rows from the popup height minus chrome.
            let popup_h_rows = if lh > 0.0 {
                (popup_rect.height / lh) as usize
            } else {
                24
            };
            let visible_rows = popup_h_rows.saturating_sub(PALETTE_CHROME_ROWS);

            let ev = picker.handle(&event, visible_rows);
            match ev {
                FolderPickerEvent::Confirmed { path } => {
                    self.confirmed_path = Some(path.clone());
                    self.status = format!(
                        "Confirmed: {}  (press 'o' to reopen, q/Esc to quit)",
                        path.display()
                    );
                    self.picker = None;
                    return Reaction::Redraw;
                }
                FolderPickerEvent::Cancelled => {
                    self.status = "Dismissed — press 'o' to reopen, q/Esc to quit".into();
                    self.picker = None;
                    return Reaction::Redraw;
                }
                FolderPickerEvent::Consumed => return Reaction::Redraw,
                FolderPickerEvent::Ignored => {}
            }
        } else {
            // Picker is closed — handle reopen / quit.
            if let UiEvent::KeyPressed { ref key, .. } = event {
                match key {
                    Key::Char('q') | Key::Named(NamedKey::Escape) => {
                        return Reaction::Exit;
                    }
                    Key::Char('o') => {
                        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
                        self.picker = Some(FolderPickerController::new(cwd, vec![], false));
                        self.status =
                            "Open Folder picker — navigate and press Enter to confirm".into();
                        return Reaction::Redraw;
                    }
                    _ => {}
                }
            }
        }

        match event {
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
