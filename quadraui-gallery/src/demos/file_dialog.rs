//! `File Dialog` demo — adapted from
//! `quadraui/examples/common/file_dialog_demo.rs`.
//!
//! Exercises `PlatformServices::show_file_open_dialog` /
//! `show_file_save_dialog` / `show_folder_open_dialog` — the *native*
//! dialog layer, distinct from the gallery's own "File Picker" demo
//! (`Content` group), which drives `FilePickerController` /
//! `FolderPickerController` directly as an always-in-canvas widget. This
//! demo instead shows the one call an app makes and lets each backend
//! supply its own answer: a real native `gtk4::FileDialog` on GTK, and a
//! real in-canvas `FilePickerController` driven through a nested
//! draw-and-read loop on TUI — same call either way, except
//! `show_folder_open_dialog`, which has no in-canvas fallback wired up
//! yet and so returns `None` unconditionally on TUI (see
//! [`Self::caps_note`]).
//!
//! Three variants — Open, Save as, Folder — map directly onto those
//! three calls; `Enter` triggers whichever one is active, and the
//! `o`/`s`/`f` keys reach all three directly regardless of the active
//! variant.
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
            status: "Enter (or o/s/f) opens the dialog".to_string(),
            picked: None,
        }
    }

    fn variant_label(variant: usize) -> &'static str {
        match variant {
            0 => "Open",
            1 => "Save as",
            _ => "Folder",
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
    fn status_bar(&self, variant: usize) -> StatusBar {
        StatusBar {
            id: WidgetId::new("gallery:file-dialog:status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" [{}] {} ", Self::variant_label(variant), self.status),
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

    fn open_file(&mut self, backend: &mut dyn Backend) {
        let opts = FileDialogOptions {
            title: Some("Open File".to_string()),
            filters: vec![("Rust files".to_string(), vec!["rs".to_string()])],
            ..self.base_opts()
        };
        let picked = backend.services().show_file_open_dialog(opts);
        self.record("Opened", "Open", picked);
    }

    fn save_file(&mut self, backend: &mut dyn Backend) {
        let opts = FileDialogOptions {
            title: Some("Save As".to_string()),
            initial_filename: Some("untitled.txt".to_string()),
            ..self.base_opts()
        };
        let picked = backend.services().show_file_save_dialog(opts);
        self.record("Save as", "Save", picked);
    }

    fn open_folder(&mut self, backend: &mut dyn Backend) {
        let opts = FileDialogOptions {
            title: Some("Open Folder".to_string()),
            ..self.base_opts()
        };
        let picked = backend.services().show_folder_open_dialog(opts);
        self.record("Folder", "Folder pick", picked);
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

    fn variants(&self) -> &'static [&'static str] {
        &["Open", "Save as", "Folder"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let lh = backend.line_height();
        let status_rect = Rect::new(area.x, area.y + area.height - lh, area.width, lh);
        let _ = backend.draw_status_bar_interactive(
            status_rect,
            &self.status_bar(variant),
            &InteractionState::new(),
        );
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        _area: Rect,
    ) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Char('o'),
                ..
            } => {
                self.open_file(backend);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('s'),
                ..
            } => {
                self.save_file(backend);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('f'),
                ..
            } => {
                self.open_folder(backend);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(quadraui::NamedKey::Enter),
                ..
            } => {
                match variant {
                    0 => self.open_file(backend),
                    1 => self.save_file(backend),
                    _ => self.open_folder(backend),
                }
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        serde_json::json!({
            "variant": Self::variant_label(variant),
            "root": self.root_dir.path().display().to_string(),
            "status": self.status,
            "picked": self.picked.as_ref().map(|p| p.display().to_string()),
        })
    }

    fn caps_note(&self, variant: usize, caps: &BackendCaps) -> Option<String> {
        match variant {
            0 | 1 if !caps.file_dialogs => Some(
                "This backend has no native file-picker facility (BackendCaps::file_dialogs is \
                 false) — open/save still resolve a real path through an in-canvas \
                 FilePickerController driven by a nested draw-and-read loop."
                    .into(),
            ),
            2 if !caps.folder_dialogs => Some(
                "This backend has no native folder-picker facility (BackendCaps::folder_dialogs \
                 is false) and no in-canvas fallback is wired up for it yet — the folder dialog \
                 degrades all the way to None."
                    .into(),
            ),
            _ => None,
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
        assert!(demo.status.contains("Enter"));
        assert!(demo.picked.is_none());
    }

    #[test]
    fn variant_label_covers_open_save_and_folder() {
        assert_eq!(FileDialogDemo::variant_label(0), "Open");
        assert_eq!(FileDialogDemo::variant_label(1), "Save as");
        assert_eq!(FileDialogDemo::variant_label(2), "Folder");
    }
}
