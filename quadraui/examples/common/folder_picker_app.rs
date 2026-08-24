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
    /// Directory the picker is (re)opened at — captured once so `o`
    /// reopens where the app started rather than re-reading the
    /// process's working directory.
    root: PathBuf,
}

impl FolderPickerApp {
    pub fn new() -> Self {
        // Fallback to "." (current dir relative) if `current_dir()` fails.
        // `PathBuf::from("/")` would be invalid on Windows, so we keep this
        // platform-neutral. The picker will still walk something sensible
        // from the process's working directory.
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::with_root(cwd)
    }

    /// Open the picker at an explicit `root` instead of the process's
    /// working directory.
    ///
    /// The runnable examples use [`Self::new`]; this exists so tests can
    /// drive the demo against a directory they control (path length,
    /// entry names) rather than inheriting whatever the checkout
    /// happens to sit under.
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let picker = FolderPickerController::new(root.clone(), vec![], false);
        Self {
            picker: Some(picker),
            confirmed_path: None,
            status: "Open Folder picker — navigate and press Enter to confirm".into(),
            root,
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
                // Only the final component — the full path is already in
                // the left segment. A right segment wider than the bar is
                // laid out at column 0 (see `StatusBar::layout`: the
                // highest-priority right segment is kept even when it
                // alone overflows) and painted *after* the left segments,
                // so an absolute path longer than the terminal is wide
                // would blank the "Confirmed: …" message entirely — which
                // is exactly what happens when the checkout lives under a
                // long directory.
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
                        // Reopen at the root this app was constructed with
                        // (the process's working directory for `new()`).
                        self.picker = Some(FolderPickerController::new(
                            self.root.clone(),
                            vec![],
                            false,
                        ));
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
