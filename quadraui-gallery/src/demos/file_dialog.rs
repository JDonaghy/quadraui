//! `File Dialog` demo — adapted from
//! `quadraui/examples/common/file_dialog_demo.rs`.
//!
//! Exercises `PlatformServices::show_file_open_dialog` /
//! `show_file_save_dialog` / `show_folder_open_dialog` — the *native*
//! dialog layer, distinct from the gallery's own "File Picker" demo
//! (`Content` group), which drives `FilePickerController` /
//! `FolderPickerController` directly as an always-in-canvas widget. This
//! demo instead shows the one call an app makes and lets each backend
//! supply its own answer: a real native `gtk4::FileDialog` on GTK, and
//! (since quadraui#965) a real in-canvas `FilePickerController` driven
//! through a nested draw-and-read loop on TUI — same call either way,
//! `show_folder_open_dialog` being the one method #965 left returning
//! `None` unconditionally on TUI (see [`Self::caps_note`]).
//!
//! Rooted at a small synthetic directory tree this demo creates for
//! itself (see [`build_demo_tree`]), so the listing is a fixed set of
//! names on every machine and every checkout.

use std::path::PathBuf;

use quadraui::{
    Backend, BackendCaps, Color, FileDialogOptions, InteractionState, Key, Reaction, Rect,
    StatusBar, StatusBarSegment, UiEvent, WidgetId,
};
use tempfile::TempDir;

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("file_dialog.rs");

// gallery:begin
/// Materialise a small, fixed directory tree for the native dialogs to
/// list — see `file_picker.rs`'s `build_demo_tree` for why this is a
/// synthetic tempdir rather than this crate's own source tree or
/// `CARGO_MANIFEST_DIR`.
fn build_demo_tree() -> TempDir {
    let dir = tempfile::tempdir().expect("create a temp dir for the file dialog demo");
    let root = dir.path();
    std::fs::write(root.join("README.md"), b"# quadraui gallery demo tree\n")
        .expect("write README.md");
    std::fs::write(root.join("main.rs"), b"fn main() {}\n").expect("write main.rs");
    dir
}

pub struct FileDialogDemo {
    root_dir: TempDir,
    status: String,
    picked: Option<PathBuf>,
}

impl FileDialogDemo {
    pub fn new() -> Self {
        Self {
            root_dir: build_demo_tree(),
            status: "o = open · s = save-as · f = folder".to_string(),
            picked: None,
        }
    }

    fn base_opts(&self) -> FileDialogOptions {
        FileDialogOptions {
            initial_dir: Some(self.root_dir.path().to_path_buf()),
            ..Default::default()
        }
    }

    /// Record a dialog outcome. `path` is `None` when the user cancelled
    /// (or the backend can't offer that dialog at all).
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

    /// The outcome message goes in a **left** segment and only the
    /// picked path's final component goes in the right one — a right
    /// segment carrying a full absolute path clips at the bar's right
    /// edge and can blank the whole message; see `file_picker.rs`'s
    /// sibling `FileDialogDemo::status_bar` in `quadraui/examples` for
    /// the regression this split avoids.
    fn status_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("gallery:file-dialog:status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.status),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: match &self.picked {
                Some(p) => {
                    let leaf = p
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| p.display().to_string());
                    vec![StatusBarSegment {
                        text: format!(" \u{2713} {leaf} "),
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

impl Demo for FileDialogDemo {
    fn name(&self) -> &'static str {
        "File Dialog"
    }

    fn group(&self) -> &'static str {
        "Overlays"
    }

    fn render(&self, _variant: usize, backend: &mut dyn Backend, area: Rect) {
        let lh = backend.line_height();
        let status_rect = Rect::new(area.x, area.y + area.height - lh, area.width, lh);
        backend.draw_status_bar_interactive(
            status_rect,
            &self.status_bar(),
            &InteractionState::new(),
        );
    }

    fn handle(
        &mut self,
        _variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        _area: Rect,
    ) -> Reaction {
        match event {
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
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, _variant: usize) -> serde_json::Value {
        serde_json::json!({
            "root": self.root_dir.path().display().to_string(),
            "status": self.status,
            "picked": self.picked.as_ref().map(|p| p.display().to_string()),
        })
    }

    fn caps_note(&self, _variant: usize, caps: &BackendCaps) -> Option<String> {
        if caps.file_dialogs && caps.folder_dialogs {
            None
        } else {
            Some(
                "This backend has no native file-picker facility (BackendCaps::file_dialogs / \
                 folder_dialogs is false) — open/save still resolve a real path through an \
                 in-canvas FilePickerController (quadraui#965); only the folder picker ('f') \
                 degrades all the way to None, with no in-canvas fallback wired yet."
                    .into(),
            )
        }
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_tree_contains_the_fixed_synthetic_entries() {
        let dir = build_demo_tree();
        assert!(dir.path().join("README.md").is_file());
        assert!(dir.path().join("main.rs").is_file());
    }

    #[test]
    fn new_starts_with_the_hint_and_no_pick() {
        let demo = FileDialogDemo::new();
        assert!(demo.status.contains("o = open"));
        assert!(demo.picked.is_none());
    }
}
