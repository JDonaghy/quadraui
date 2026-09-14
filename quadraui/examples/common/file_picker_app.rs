//! Backend-agnostic app code for the `FilePickerController` demo
//! ([`tui_file_picker`] / [`gtk_file_picker`]).
//!
//! [`FilePickerApp`] demonstrates a self-contained `AppLogic` that:
//! - Opens the `FilePickerController` in [`FilePickerMode::Open`] on
//!   startup (rooted at `env::current_dir()`).
//! - Shows the picker as a centred palette modal.
//! - Confirms a selection (`Enter` on a file) and displays the chosen
//!   path in a status bar.
//! - Navigates into subdirectories with `Enter` on a directory row.
//! - Dismisses the picker with `Esc`.
//!
//! Controls (while picker is open):
//! - Type to fuzzy-filter entries (Save mode: also the destination name).
//! - `↑` / `↓` to move selection.
//! - `Enter` on `..` or a directory — navigate.
//! - `Enter` on a file (Open) / with a typed name (Save) — confirm.
//! - `Backspace` — delete last query character.
//! - `Esc` — dismiss picker.
//!
//! Controls (after dismiss):
//! - `o` — reopen in Open mode.
//! - `s` — reopen in Save mode.
//! - `q` / `Esc` — quit.

use std::path::PathBuf;

use quadraui::{
    AppLogic, Backend, Color, FilePickerController, FilePickerEvent, FilePickerMode,
    InteractionState, Key, NamedKey, Reaction, Rect, StatusBar, StatusBarSegment, UiEvent,
    WidgetId, PALETTE_CHROME_ROWS,
};

pub struct FilePickerApp {
    picker: Option<FilePickerController>,
    confirmed_path: Option<PathBuf>,
    status: String,
    root: PathBuf,
}

impl FilePickerApp {
    pub fn new() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::with_root(cwd)
    }

    /// Open the picker at an explicit `root` — lets tests drive the demo
    /// against a directory they control rather than inheriting whatever
    /// the checkout happens to sit under.
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let picker = FilePickerController::new(FilePickerMode::Open, root.clone(), vec![]);
        Self {
            picker: Some(picker),
            confirmed_path: None,
            status: "Open File picker — navigate and press Enter to confirm".into(),
            root,
        }
    }

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
                let leaf = p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| p.display().to_string());
                vec![StatusBarSegment {
                    text: format!(" ✓ {leaf} "),
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

impl Default for FilePickerApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for FilePickerApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        let lh = backend.line_height();
        let bar_h = lh * 1.5;
        let bar_rect = Rect::new(0.0, vp.height - bar_h, vp.width, bar_h);
        let bar = self.status_bar();
        let _ = backend.draw_status_bar_interactive(bar_rect, &bar, &InteractionState::new());

        if let Some(ref picker) = self.picker {
            let popup_rect = Self::popup_rect(backend);
            picker.render(popup_rect, backend);
        }
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        if let Some(ref mut picker) = self.picker {
            let popup_rect = Self::popup_rect(backend);
            let lh = backend.line_height();
            let popup_h_rows = if lh > 0.0 {
                (popup_rect.height / lh) as usize
            } else {
                24
            };
            let visible_rows = popup_h_rows.saturating_sub(PALETTE_CHROME_ROWS);

            match picker.handle(&event, visible_rows) {
                FilePickerEvent::Confirmed { path } => {
                    let verb = match picker.mode() {
                        FilePickerMode::Open => "Opened",
                        FilePickerMode::Save => "Saved",
                    };
                    self.confirmed_path = Some(path.clone());
                    self.status = format!(
                        "{verb}: {}  ('o'/'s' to reopen, q/Esc to quit)",
                        path.display()
                    );
                    self.picker = None;
                    return Reaction::Redraw;
                }
                FilePickerEvent::Cancelled => {
                    self.status = "Dismissed — 'o'/'s' to reopen, q/Esc to quit".into();
                    self.picker = None;
                    return Reaction::Redraw;
                }
                FilePickerEvent::Consumed => return Reaction::Redraw,
                FilePickerEvent::Ignored => {}
            }
        } else if let UiEvent::KeyPressed { ref key, .. } = event {
            match key {
                Key::Char('q') | Key::Named(NamedKey::Escape) => {
                    return Reaction::Exit;
                }
                Key::Char('o') => {
                    self.picker = Some(FilePickerController::new(
                        FilePickerMode::Open,
                        self.root.clone(),
                        vec![],
                    ));
                    self.status = "Open File picker — navigate and press Enter to confirm".into();
                    return Reaction::Redraw;
                }
                Key::Char('s') => {
                    self.picker = Some(
                        FilePickerController::new(FilePickerMode::Save, self.root.clone(), vec![])
                            .with_initial_filename("untitled.txt"),
                    );
                    self.status = "Save File picker — type a name and press Enter".into();
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
