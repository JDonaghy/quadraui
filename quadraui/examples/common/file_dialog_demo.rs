//! File-dialog demo — exercises [`PlatformServices::show_file_open_dialog`]
//! / `show_file_save_dialog` (#427) and, since quadraui#935,
//! [`PlatformServices::show_folder_open_dialog`].
//!
//! - **GTK** (`gtk_file_dialog`): opens a real native `gtk4::FileDialog`
//!   (via the nested-mainloop adapter in `gtk::services`) and blocks until
//!   the user picks a file/location/folder or cancels. This is the primary
//!   manual smoke test for #427/#935 — driving it headlessly isn't possible
//!   (no `GtkDriver` yet, #301), so exercise it by hand: run the example,
//!   press `o` / `s` / `f`, and confirm the native dialog appears, is
//!   parented to the demo window, and the status bar reflects the picked
//!   path (or "cancelled" on Escape/close).
//! - **TUI** (`tui_file_dialog`): since issue #965,
//!   `show_file_open_dialog`/`show_file_save_dialog` drive a real
//!   in-canvas `compose::FilePickerController` through a nested
//!   draw-and-read loop (see `tui::services`'s module doc) and return a
//!   genuine chosen path — press `o`/`s` and type/arrow through the
//!   picker like `tui_file_picker` does. `show_folder_open_dialog` is
//!   the one method here **not** touched by #965 (out of that issue's
//!   scope — see `tui::services`'s module doc) and still always returns
//!   `None` on TUI; apps should provide an in-canvas picker instead (see
//!   the separate `tui_folder_picker`/`gtk_folder_picker` examples). Both
//!   the open/save real-path round trip and the folder `None` degrade
//!   are covered by the `TuiDriver` tests in
//!   `tests/tui_example_driver.rs`.
//!
//! ```sh
//! cargo run --example gtk_file_dialog --features gtk
//! cargo run --example tui_file_dialog --features tui
//! ```
//!
//! Controls:
//! - `o` — open-file dialog (filtered to `*.rs`)
//! - `s` — save-as dialog (initial name `untitled.txt`)
//! - `f` — open-folder dialog (quadraui#935)
//! - `Esc` / `q` — quit
//!
//! The status bar shows the full confirmed path on the left and just its
//! final component on the right — see [`FileDialogDemo::status_bar`] for
//! why that split matters. [`FileDialogDemo::with_initial_dir`] roots
//! every dialog somewhere other than the backend's default.

use quadraui::{
    AppLogic, Backend, Color, FileDialogOptions, InteractionState, Key, NamedKey, Reaction, Rect,
    StatusBar, StatusBarSegment, UiEvent, WidgetId,
};
use std::path::PathBuf;

pub struct FileDialogDemo {
    /// The full outcome message, including the absolute path when one
    /// was picked. Painted as a *left* segment — see [`Self::status_bar`].
    status: String,
    /// The last confirmed path, if any. Only its final component is
    /// painted, as a *right* segment — see [`Self::status_bar`].
    picked: Option<PathBuf>,
    /// Overrides `FileDialogOptions::initial_dir` for all three dialogs.
    /// `None` (what both `*_file_dialog` runners use) leaves the field at
    /// its default, so each backend's `PlatformServices` roots the picker
    /// wherever it normally would — `std::env::current_dir()` on TUI.
    /// Mirrors `FolderPickerApp::with_root` / `FilePickerApp::with_root`:
    /// it lets a driver test point the demo at a directory it controls
    /// instead of inheriting the checkout's own location.
    initial_dir: Option<PathBuf>,
}

impl FileDialogDemo {
    pub fn new() -> Self {
        Self {
            status: "o = open · s = save-as · f = folder · Esc quits".to_string(),
            picked: None,
            initial_dir: None,
        }
    }

    /// Root every dialog this demo opens at `dir` instead of the
    /// backend's default — see [`Self::initial_dir`].
    pub fn with_initial_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            initial_dir: Some(dir.into()),
            ..Self::new()
        }
    }

    /// Base options shared by all three dialog keys, carrying
    /// [`Self::initial_dir`].
    fn base_opts(&self) -> FileDialogOptions {
        FileDialogOptions {
            initial_dir: self.initial_dir.clone(),
            ..Default::default()
        }
    }

    /// Record a dialog outcome. `path` is `None` when the user cancelled
    /// (or the backend can't offer that dialog at all) — `ok_prefix`
    /// labels the confirmed case, `cancel_label` the cancelled one.
    fn record(&mut self, ok_prefix: &str, cancel_label: &str, path: Option<PathBuf>) {
        match path {
            Some(p) => {
                self.status = format!("{ok_prefix}: {}", p.display());
                self.picked = Some(p);
            }
            None => {
                self.status = format!("{cancel_label} cancelled (or unsupported on this backend)");
                self.picked = None;
            }
        }
    }

    /// The outcome message goes in a **left** segment and only the picked
    /// path's final component goes in the right one. That split is
    /// load-bearing, not cosmetic: `StatusBar::layout` right-aligns the
    /// right group inside the bar and paints it *after* the left
    /// segments, so a right segment carrying a full absolute path is
    /// clipped at the bar's right edge — losing its tail, which is
    /// exactly the filename a reader (and this demo's driver tests) care
    /// about — and, once wider than the bar, lands at column 0 and blanks
    /// the message entirely. Since the path is rooted at the backend's
    /// `current_dir()` by default, whether that happened depended purely
    /// on how deep the checkout sat. `FolderPickerApp::status_bar` had
    /// the identical bug and the identical fix; the long message clips
    /// harmlessly from its tail on the left, and the short leaf stays
    /// pinned to the right edge at any path length.
    fn status_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("file-dialog-demo:status"),
            left_segments: vec![
                StatusBarSegment {
                    text: " File dialog demo (#427) ".into(),
                    fg: Color::rgb(255, 255, 255),
                    bg: Color::rgb(40, 80, 120),
                    bold: true,
                    action_id: None,
                },
                StatusBarSegment {
                    text: format!(" {} ", self.status),
                    fg: Color::rgb(220, 220, 220),
                    bg: Color::rgb(40, 80, 120),
                    bold: false,
                    action_id: None,
                },
            ],
            right_segments: match &self.picked {
                Some(p) => {
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
                }
                None => vec![],
            },
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
        backend.draw_status_bar_interactive(
            status_rect,
            &self.status_bar(),
            &InteractionState::new(),
        );
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
                    ..self.base_opts()
                };
                let picked = backend.services().show_file_open_dialog(opts);
                self.record("Opened", "Open", picked);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('s'),
                ..
            } => {
                let opts = FileDialogOptions {
                    title: Some("Save As".to_string()),
                    initial_filename: Some("untitled.txt".to_string()),
                    ..self.base_opts()
                };
                let picked = backend.services().show_file_save_dialog(opts);
                self.record("Save as", "Save", picked);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('f'),
                ..
            } => {
                let opts = FileDialogOptions {
                    title: Some("Open Folder".to_string()),
                    ..self.base_opts()
                };
                let picked = backend.services().show_folder_open_dialog(opts);
                self.record("Folder", "Folder pick", picked);
                Reaction::Redraw
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
