//! `File & Folder Picker` demo — adapted from
//! `quadraui/examples/common/file_picker_app.rs` and
//! `folder_picker_app.rs`.
//!
//! Three variants cover `FilePickerController`'s two modes plus
//! `FolderPickerController` — the in-app pickers every backend
//! (including TUI) gets for free, no native file-dialog support
//! required. All three are rooted at this crate's own `src/demos/`
//! directory, so the listing is identical on every machine this runs on.

use std::path::{Path, PathBuf};

use quadraui::{
    Backend, BackendCaps, Color, FilePickerController, FilePickerEvent, FilePickerMode,
    FolderPickerController, FolderPickerEvent, InteractionState, Reaction, Rect, StatusBar,
    StatusBarSegment, UiEvent, WidgetId, PALETTE_CHROME_ROWS,
};

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("file_picker.rs");

// gallery:begin
/// Deterministic picker root: this crate's own `src/demos/` directory,
/// so the listing is the same on every checkout rather than inheriting
/// whatever directory the gallery process happens to run from.
fn demo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/demos")
}

pub struct FilePickerDemo {
    open_picker: FilePickerController,
    save_picker: FilePickerController,
    folder_picker: FolderPickerController,
    confirmed: Option<String>,
}

impl FilePickerDemo {
    pub fn new() -> Self {
        let root = demo_root();
        Self {
            open_picker: FilePickerController::new(FilePickerMode::Open, root.clone(), vec![]),
            save_picker: FilePickerController::new(FilePickerMode::Save, root.clone(), vec![])
                .with_initial_filename("untitled.rs"),
            folder_picker: FolderPickerController::new(root, vec![], false),
            confirmed: None,
        }
    }

    fn popup_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let w = (area.width * 0.8).max(40.0);
        let h = (area.height * 0.8).max(12.0 * backend.line_height());
        let x = area.x + (area.width - w) / 2.0;
        let y = area.y + (area.height - h) / 2.0;
        Rect::new(x, y, w, h)
    }

    fn status_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("gallery:file-picker:status"),
            left_segments: vec![StatusBarSegment {
                text: format!(
                    " {} ",
                    self.confirmed
                        .as_deref()
                        .unwrap_or("navigate, Enter to confirm")
                ),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 60, 100),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }

    fn visible_rows(popup_rect: Rect, backend: &dyn Backend) -> usize {
        let lh = backend.line_height();
        let rows = if lh > 0.0 {
            (popup_rect.height / lh) as usize
        } else {
            24
        };
        rows.saturating_sub(PALETTE_CHROME_ROWS)
    }
}

impl Default for FilePickerDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for FilePickerDemo {
    fn name(&self) -> &'static str {
        "File Picker"
    }

    fn group(&self) -> &'static str {
        "Content"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Open file", "Save file", "Choose folder"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let lh = backend.line_height();
        let bar_h = lh * 1.5;
        let bar_rect = Rect::new(area.x, area.y + area.height - bar_h, area.width, bar_h);
        let _ = backend.draw_status_bar_interactive(
            bar_rect,
            &self.status_bar(),
            &InteractionState::new(),
        );

        let popup_rect = Self::popup_rect(area, backend);
        match variant {
            0 => self.open_picker.render(popup_rect, backend),
            1 => self.save_picker.render(popup_rect, backend),
            _ => self.folder_picker.render(popup_rect, backend),
        }
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        let popup_rect = Self::popup_rect(area, backend);
        let visible_rows = Self::visible_rows(popup_rect, backend);
        match variant {
            0 => match self.open_picker.handle(event, visible_rows) {
                FilePickerEvent::Confirmed { path } => {
                    self.confirmed = Some(format!("Opened: {}", path.display()));
                    Reaction::Redraw
                }
                FilePickerEvent::Cancelled => {
                    self.confirmed = Some("Dismissed".into());
                    Reaction::Redraw
                }
                FilePickerEvent::Consumed => Reaction::Redraw,
                FilePickerEvent::Ignored => Reaction::Continue,
            },
            1 => match self.save_picker.handle(event, visible_rows) {
                FilePickerEvent::Confirmed { path } => {
                    self.confirmed = Some(format!("Saved: {}", path.display()));
                    Reaction::Redraw
                }
                FilePickerEvent::Cancelled => {
                    self.confirmed = Some("Dismissed".into());
                    Reaction::Redraw
                }
                FilePickerEvent::Consumed => Reaction::Redraw,
                FilePickerEvent::Ignored => Reaction::Continue,
            },
            _ => match self.folder_picker.handle(event, visible_rows) {
                FolderPickerEvent::Confirmed { path } => {
                    self.confirmed = Some(format!("Chose folder: {}", path.display()));
                    Reaction::Redraw
                }
                FolderPickerEvent::Cancelled => {
                    self.confirmed = Some("Dismissed".into());
                    Reaction::Redraw
                }
                FolderPickerEvent::Consumed => Reaction::Redraw,
                FolderPickerEvent::Ignored => Reaction::Continue,
            },
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        let mode = match variant {
            0 => "open",
            1 => "save",
            _ => "folder",
        };
        serde_json::json!({
            "mode": mode,
            "root": demo_root().display().to_string(),
            "confirmed": self.confirmed,
        })
    }

    fn caps_note(&self, _variant: usize, _caps: &BackendCaps) -> Option<String> {
        // Both controllers are in-app pickers (quadraui's own
        // palette-rendered list, not a native OS dialog), so they work
        // identically everywhere — there's no `BackendCaps` flag to
        // check and no gap to report.
        None
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_root_is_this_crates_demos_directory() {
        let root = demo_root();
        assert!(root.ends_with("src/demos"));
        assert!(root.join("mod.rs").exists());
    }

    #[test]
    fn save_picker_starts_with_the_initial_filename() {
        let demo = FilePickerDemo::new();
        assert_eq!(demo.save_picker.mode(), FilePickerMode::Save);
    }
}
